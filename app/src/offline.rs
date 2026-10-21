//! Offline packs and bulk downloads.
//!
//! Desktop: images live under `<app data>/offline/assets/<png_key>`. The image loader writes
//! every PNG it fetches there and reads from there before the network, so anything used once
//! keeps working without internet; a pack fills the folder for whole collections up front and
//! adds `assets.sqlite` (a copy of the catalog) and `README.md` next to it.
//!
//! Web: the browser can't keep a folder, so the same pack is downloaded as a .zip with the
//! same layout under `biocartoon-offline/`. The zip is assembled in a Blob a few files at a
//! time, so a pack of a gigabyte or more never sits in wasm memory.

use rusqlite::Connection;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

/// Name of the folder (desktop) or the zip's top folder (web).
pub const DIR_NAME: &str = "offline";
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const ZIP_ROOT: &str = "biocartoon-offline";
/// Attempts per image before it counts as failed (connections drop on long downloads).
const TRIES: u32 = 4;

/// One collection of the catalog, and how much of it is downloaded.
#[derive(Clone, Debug, Default)]
pub struct Collection {
    pub source: String,
    pub name: String,
    pub count: usize,
    pub bytes: u64,
    pub licenses: String,
    /// Images already on this computer (always 0 in the browser).
    pub have: usize,
    pub have_bytes: u64,
}

impl Collection {
    pub fn complete(&self) -> bool {
        self.count > 0 && self.have >= self.count
    }
}

/// Where the cached copy of the asset `key` lives under `dir`; `None` for keys that would
/// escape it.
pub fn cache_path(dir: &Path, key: &str) -> Option<PathBuf> {
    let rel = Path::new(key);
    if key.is_empty() || !rel.components().all(|c| matches!(c, Component::Normal(_))) {
        return None;
    }
    Some(dir.join(rel))
}

/// Keep a fetched PNG under `dir` (written via a temp file, so a reader never sees half of it).
#[cfg(not(target_arch = "wasm32"))]
pub fn store(dir: &Path, key: &str, bytes: &[u8]) {
    let Some(p) = cache_path(dir, key) else { return };
    if let Err(e) = crate::storage::write(&p, bytes) {
        log::warn!("offline cache {}: {e}", p.display());
    }
}

pub fn human_bytes(b: u64) -> String {
    match b {
        b if b >= 1_000_000_000 => format!("{:.1} GB", b as f64 / 1e9),
        b if b >= 1_000_000 => format!("{:.0} MB", b as f64 / 1e6),
        0 => "0 KB".into(),
        b => format!("{:.0} KB", (b as f64 / 1e3).max(1.0)),
    }
}

// ---------- the catalog ----------

#[cfg(not(target_arch = "wasm32"))]
fn open_catalog(db: &Path) -> Result<Connection, String> {
    Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|e| format!("open {}: {e}", db.display()))
}

#[cfg(target_arch = "wasm32")]
fn open_catalog(db: &Path) -> Result<Connection, String> {
    let bytes = crate::storage::read(db).map_err(|e| format!("catalog: {e}"))?;
    let mut conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
    conn.deserialize_read_exact(rusqlite::MAIN_DB, &bytes[..], bytes.len(), true).map_err(|e| format!("catalog: {e}"))?;
    Ok(conn)
}

/// The collection a row belongs to: BioCartoon's own uploads count as BioCartoon (see
/// [`crate::theme::asset_source`]).
fn source_sql() -> String {
    use crate::theme::{BIOCARTOON_AUTHOR, BIOCARTOON_SOURCE};
    format!("CASE WHEN source = 'user' AND author = '{BIOCARTOON_AUTHOR}' THEN '{BIOCARTOON_SOURCE}' ELSE source END")
}

/// Every collection (User-generated always listed, even when empty): BioCartoon first, then
/// the rest in order of the databases' two-letter badges.
fn collections(conn: &Connection) -> Result<Vec<Collection>, String> {
    let mut st = conn
        .prepare(&format!(
            "SELECT {} AS src, max(source_name), count(*), coalesce(sum(png_bytes), 0), group_concat(DISTINCT license) \
             FROM assets WHERE png_key IS NOT NULL AND png_key != '' GROUP BY src",
            source_sql()
        ))
        .map_err(|e| e.to_string())?;
    let rows = st
        .query_map([], |r| {
            Ok(Collection {
                source: r.get(0)?,
                name: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                count: r.get::<_, i64>(2)? as usize,
                bytes: r.get::<_, i64>(3)? as u64,
                licenses: r.get::<_, Option<String>>(4)?.unwrap_or_default().replace(',', ", "),
                have: 0,
                have_bytes: 0,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut cols: Vec<Collection> = rows.collect::<Result<_, _>>().map_err(|e| e.to_string())?;
    if !cols.iter().any(|c| c.source == "user") {
        cols.push(Collection { source: "user".into(), ..Default::default() });
    }
    for c in &mut cols {
        let (_, _, name) = crate::theme::source_badge(&c.source);
        if name != "Unknown" || c.name.is_empty() {
            c.name = name.into();
        }
    }
    cols.sort_by_key(|c| (c.source != crate::theme::BIOCARTOON_SOURCE, crate::theme::source_badge(&c.source).0));
    Ok(cols)
}

struct Item {
    source: String,
    key: String,
    bytes: u64,
}

/// Every image of `sources` (all collections when empty).
fn items(conn: &Connection, sources: &[String]) -> Result<Vec<Item>, String> {
    let mut st = conn
        .prepare(&format!("SELECT {} AS src, png_key, coalesce(png_bytes, 0) FROM assets WHERE png_key IS NOT NULL AND png_key != '' ORDER BY src, png_key", source_sql()))
        .map_err(|e| e.to_string())?;
    let rows = st
        .query_map([], |r| Ok(Item { source: r.get(0)?, key: r.get(1)?, bytes: r.get::<_, i64>(2)? as u64 }))
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        let it = r.map_err(|e| e.to_string())?;
        if sources.is_empty() || sources.contains(&it.source) {
            out.push(it);
        }
    }
    Ok(out)
}

/// The catalog's version, from its file name (`assets-<version>.sqlite`).
fn catalog_version(db: &Path) -> String {
    let stem = db.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    stem.strip_prefix("assets-").unwrap_or(&stem).to_string()
}

fn scope_label(sources: &[String], all: &[Collection]) -> String {
    if sources.is_empty() {
        return "all collections".into();
    }
    let names: Vec<String> = sources.iter().map(|s| all.iter().find(|c| &c.source == s).map(|c| c.name.clone()).unwrap_or_else(|| s.clone())).collect();
    names.join(", ")
}

/// The pack's README. `included` lists each collection with the images actually in the pack.
fn readme(included: &[Collection], version: &str, base_url: &str, zip: bool) -> String {
    let mut s = String::new();
    s.push_str("# BioCartoon offline pack\n\n");
    s.push_str(&format!(
        "Made {} from {base_url} (catalog version {version}).\n\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M")
    ));
    s.push_str("## Contents\n\n| Collection | Images | Size | Licenses |\n|---|---|---|---|\n");
    let (mut n, mut b) = (0, 0);
    for c in included.iter().filter(|c| c.have > 0) {
        let partial = if c.have < c.count { format!(" (of {})", c.count) } else { String::new() };
        s.push_str(&format!("| {} (`{}`) | {}{partial} | {} | {} |\n", c.name, c.source, c.have, human_bytes(c.have_bytes), c.licenses));
        n += c.have;
        b += c.have_bytes;
    }
    s.push_str(&format!("| **Total** | **{n}** | **{}** | |\n\n", human_bytes(b)));
    s.push_str(
        "## Layout\n\n\
         - `README.md`: this file.\n\
         - `assets.sqlite`: the icon catalog (SQLite), one row per image in the `assets` table, with its \
         `title`, `tags`, `category`, `license`, `credit`, `source_url` and `png_key`. It lists every \
         collection in the catalog, including any whose images are not in this pack.\n\
         - `assets/<png_key>`: the PNG of each row, for example `assets/bioart/2_626557.png`.\n\n\
         ## Licenses and credit\n\n\
         Every image keeps the license of the collection it came from (the `license` and `license_url` \
         columns). Where `attribution_required` is 1, credit the author with the ready-made `credit` line \
         when you publish a figure; where `share_alike` is 1, work that adapts the image must use the \
         same license.\n\n\
         ```sh\nsqlite3 assets.sqlite \"SELECT title, png_key, license, credit FROM assets WHERE title LIKE '%neuron%' LIMIT 20\"\n```\n\n\
         ## Using it\n\n",
    );
    if zip {
        s.push_str(
            "This pack was downloaded from the BioCartoon web app. The desktop app can make the same pack \
             and use it without internet (Settings > Offline): https://biocartoon.com/download\n",
        );
    } else {
        s.push_str(
            "The BioCartoon desktop app reads this folder automatically: images found here load without \
             internet, and every image the app downloads is added here too. Manage it in Settings > Offline.\n",
        );
    }
    s
}

// ---------- jobs ----------

#[derive(Default)]
pub struct Progress {
    pub total: AtomicUsize,
    pub done: AtomicUsize,
    pub failed: AtomicUsize,
    pub bytes: AtomicU64,
    pub cancel: AtomicBool,
}

/// A running pack download (desktop) or zip download (web).
pub struct Job {
    pub label: String,
    pub progress: Arc<Progress>,
    rx: Receiver<Result<String, String>>,
}

impl Job {
    /// The finished result (a message for the user), once there is one.
    pub fn poll(&self) -> Option<Result<String, String>> {
        self.rx.try_recv().ok()
    }

    pub fn cancel(&self) {
        self.progress.cancel.store(true, Ordering::Relaxed);
    }
}

/// Read the catalog's collections and how much of each is under `cache_dir` (desktop) .
pub fn scan(db: PathBuf, cache_dir: Option<PathBuf>) -> Receiver<Result<Vec<Collection>, String>> {
    let (tx, rx) = channel();
    crate::platform::spawn("offline-scan", move || {
        let _ = tx.send(scan_now(&db, cache_dir.as_deref()));
    });
    rx
}

fn scan_now(db: &Path, cache_dir: Option<&Path>) -> Result<Vec<Collection>, String> {
    let conn = open_catalog(db)?;
    let mut cols = collections(&conn)?;
    if let Some(dir) = cache_dir {
        for it in items(&conn, &[])? {
            if cache_path(dir, &it.key).is_some_and(|p| p.is_file()) {
                if let Some(c) = cols.iter_mut().find(|c| c.source == it.source) {
                    c.have += 1;
                    c.have_bytes += it.bytes;
                }
            }
        }
    }
    Ok(cols)
}

/// Desktop: download every missing image of `sources` (all when empty) into `offline/assets`,
/// then refresh `offline/assets.sqlite` and `offline/README.md`.
#[cfg(not(target_arch = "wasm32"))]
pub fn start(ctx: egui::Context, db: PathBuf, base_url: String, offline_dir: PathBuf, sources: Vec<String>, all: &[Collection]) -> Job {
    let label = scope_label(&sources, all);
    let progress = Arc::new(Progress::default());
    let (tx, rx) = channel();
    let p = progress.clone();
    let what = label.clone();
    crate::platform::spawn("offline-pack", move || {
        let r = download_pack(&db, &base_url, &offline_dir, &sources, &p).map(|(got, failed)| {
            if p.cancel.load(Ordering::Relaxed) {
                return format!("Offline download stopped; the {got} images downloaded so far are kept");
            }
            let mut msg = format!("Offline pack ready: {what} ({got} images downloaded)");
            if failed > 0 {
                msg.push_str(&format!("; {failed} failed, run it again to retry"));
            }
            msg
        });
        let _ = tx.send(r);
        ctx.request_repaint();
    });
    Job { label, progress, rx }
}

#[cfg(not(target_arch = "wasm32"))]
fn download_pack(db: &Path, base_url: &str, offline_dir: &Path, sources: &[String], p: &Progress) -> Result<(usize, usize), String> {
    use std::sync::Mutex;
    if base_url.is_empty() {
        return Err("No asset URL configured.".into());
    }
    let assets_dir = offline_dir.join("assets");
    let todo: Vec<Item> = {
        let conn = open_catalog(db)?;
        let all = items(&conn, sources)?;
        p.total.store(all.len(), Ordering::Relaxed);
        let (have, missing): (Vec<Item>, Vec<Item>) = all.into_iter().partition(|it| cache_path(&assets_dir, &it.key).is_some_and(|p| p.is_file()));
        p.done.store(have.len(), Ordering::Relaxed);
        missing
    };
    let next = AtomicUsize::new(0);
    let got = AtomicUsize::new(0);
    let base = base_url.trim_end_matches('/');
    let first_error: Mutex<Option<String>> = Mutex::new(None);
    std::thread::scope(|s| {
        for _ in 0..8 {
            s.spawn(|| loop {
                if p.cancel.load(Ordering::Relaxed) {
                    break;
                }
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(it) = todo.get(i) else { break };
                let req = crate::assets::Request::Url(format!("{base}/{}", it.key));
                let mut res = crate::assets::fetch(&req);
                for n in 1..TRIES {
                    if res.is_ok() || p.cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500 << n));
                    res = crate::assets::fetch(&req);
                }
                match res {
                    Ok(b) if b.starts_with(b"\x89PNG") => {
                        store(&assets_dir, &it.key, &b);
                        p.bytes.fetch_add(b.len() as u64, Ordering::Relaxed);
                        got.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(_) => {
                        p.failed.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(e) => {
                        p.failed.fetch_add(1, Ordering::Relaxed);
                        first_error.lock().unwrap().get_or_insert(e);
                    }
                }
                p.done.fetch_add(1, Ordering::Relaxed);
            });
        }
    });
    let got = got.into_inner();
    let failed = p.failed.load(Ordering::Relaxed);
    if got == 0 && failed > 0 {
        let e = first_error.into_inner().unwrap().unwrap_or_else(|| "unexpected response".into());
        return Err(format!("couldn't download the images ({e})"));
    }
    write_extras(db, base_url, offline_dir)?;
    Ok((got, failed))
}

/// Copy the catalog to `offline/assets.sqlite` (a consistent snapshot, even while the app has
/// it open) and write the README for what is now on disk.
#[cfg(not(target_arch = "wasm32"))]
fn write_extras(db: &Path, base_url: &str, offline_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(offline_dir).map_err(|e| e.to_string())?;
    let dest = offline_dir.join("assets.sqlite");
    let tmp = offline_dir.join("assets.sqlite.tmp");
    let _ = std::fs::remove_file(&tmp);
    open_catalog(db)?
        .execute("VACUUM INTO ?1", [tmp.display().to_string()])
        .map_err(|e| format!("copy catalog: {e}"))?;
    std::fs::rename(&tmp, &dest).map_err(|e| e.to_string())?;
    let cols = scan_now(db, Some(&offline_dir.join("assets")))?;
    let text = readme(&cols, &catalog_version(db), base_url, false);
    crate::storage::write(&offline_dir.join("README.md"), text.as_bytes()).map_err(|e| e.to_string())
}

/// Desktop: delete the downloaded images of `sources` (all, with the catalog copy and README,
/// when empty). Only removes what a pack writes, since `offline_dir` may be a folder the user chose.
#[cfg(not(target_arch = "wasm32"))]
pub fn remove(db: &Path, base_url: &str, offline_dir: &Path, sources: &[String]) -> Result<(), String> {
    if sources.is_empty() {
        let gone = |r: std::io::Result<()>| match r {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        };
        gone(std::fs::remove_dir_all(offline_dir.join("assets")))?;
        gone(std::fs::remove_file(offline_dir.join("assets.sqlite")))?;
        return gone(std::fs::remove_file(offline_dir.join("README.md")));
    }
    let assets_dir = offline_dir.join("assets");
    let conn = open_catalog(db)?;
    for it in items(&conn, sources)? {
        if let Some(p) = cache_path(&assets_dir, &it.key) {
            let _ = std::fs::remove_file(p);
        }
    }
    for s in sources {
        // Leftover empty folders (keys are `<source>/...`).
        if let Some(d) = cache_path(&assets_dir, s) {
            let _ = std::fs::remove_dir(&d);
        }
    }
    if offline_dir.join("README.md").exists() {
        write_extras(db, base_url, offline_dir)?;
    }
    Ok(())
}

// ---------- web: zip downloads ----------

/// A zip archive written front to back: each entry's header is returned for the caller to emit
/// before the entry's bytes, and `finish` returns the central directory. Entries are stored
/// uncompressed (PNGs are compressed already). No ZIP64: packs stay under 4 GB and 65,535 files.
#[derive(Default)]
pub struct ZipWriter {
    offset: u64,
    central: Vec<u8>,
    count: usize,
}

impl ZipWriter {
    fn dos_time() -> (u16, u16) {
        use chrono::{Datelike, Timelike};
        let t = chrono::Local::now();
        let time = (t.hour() << 11 | t.minute() << 5 | t.second() / 2) as u16;
        let date = (((t.year() - 1980).max(0) as u32) << 9 | t.month() << 5 | t.day()) as u16;
        (time, date)
    }

    /// The local header for `name`, holding `data`.
    pub fn entry(&mut self, name: &str, data: &[u8]) -> Result<Vec<u8>, String> {
        let mut crc = flate2::Crc::new();
        crc.update(data);
        let (crc, len) = (crc.sum(), data.len() as u64);
        if self.offset + len + 30 + name.len() as u64 > u32::MAX as u64 || self.count >= 65_000 {
            return Err("this download is too big for one zip; download the collections one at a time".into());
        }
        let (time, date) = Self::dos_time();
        let mut fixed = Vec::with_capacity(26);
        fixed.extend_from_slice(&20u16.to_le_bytes()); // version needed
        fixed.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 names
        fixed.extend_from_slice(&0u16.to_le_bytes()); // stored
        fixed.extend_from_slice(&time.to_le_bytes());
        fixed.extend_from_slice(&date.to_le_bytes());
        fixed.extend_from_slice(&crc.to_le_bytes());
        fixed.extend_from_slice(&(len as u32).to_le_bytes());
        fixed.extend_from_slice(&(len as u32).to_le_bytes());
        fixed.extend_from_slice(&(name.len() as u16).to_le_bytes());
        fixed.extend_from_slice(&0u16.to_le_bytes()); // extra length

        let mut local = 0x04034b50u32.to_le_bytes().to_vec();
        local.extend_from_slice(&fixed);
        local.extend_from_slice(name.as_bytes());

        self.central.extend_from_slice(&0x02014b50u32.to_le_bytes());
        self.central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        self.central.extend_from_slice(&fixed);
        self.central.extend_from_slice(&0u16.to_le_bytes()); // comment length
        self.central.extend_from_slice(&0u16.to_le_bytes()); // disk
        self.central.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
        self.central.extend_from_slice(&0u32.to_le_bytes()); // external attributes
        self.central.extend_from_slice(&(self.offset as u32).to_le_bytes());
        self.central.extend_from_slice(name.as_bytes());

        self.offset += local.len() as u64 + len;
        self.count += 1;
        Ok(local)
    }

    /// The central directory and end record; nothing may be added after.
    pub fn finish(self) -> Vec<u8> {
        let mut out = self.central;
        let size = out.len() as u32;
        out.extend_from_slice(&0x06054b50u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(self.count as u16).to_le_bytes());
        out.extend_from_slice(&(self.count as u16).to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(self.offset as u32).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }
}

/// Web: download `sources` (all when empty) as one .zip.
#[cfg(target_arch = "wasm32")]
pub fn start(ctx: egui::Context, db: PathBuf, base_url: String, _offline_dir: PathBuf, sources: Vec<String>, all: &[Collection]) -> Job {
    let label = scope_label(&sources, all);
    let progress = Arc::new(Progress::default());
    let (tx, rx) = channel();
    if let Err(e) = web::start(ctx, &db, base_url, sources, all.to_vec(), progress.clone(), tx.clone()) {
        let _ = tx.send(Err(e));
    }
    Job { label, progress, rx }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use super::*;
    use std::cell::RefCell;
    use std::sync::mpsc::Sender;
    use std::sync::Mutex;

    /// Fetches in flight at once.
    const FETCHES: usize = 6;
    /// Bytes gathered in JS arrays before they are folded into the Blob (which the browser may
    /// keep on disk).
    const FLUSH_BYTES: usize = 32 << 20;

    /// The zip being assembled. JS values aren't `Send`, so they stay on the (only) thread here
    /// while the `Send` fetch callbacks hold the rest.
    struct Parts {
        blob: Option<web_sys::Blob>,
        pending: js_sys::Array,
        pending_bytes: usize,
    }

    thread_local! {
        static PARTS: RefCell<Option<Parts>> = const { RefCell::new(None) };
    }

    fn push(bytes: &[u8]) {
        PARTS.with(|p| {
            let mut p = p.borrow_mut();
            let Some(p) = p.as_mut() else { return };
            p.pending.push(&js_sys::Uint8Array::from(bytes));
            p.pending_bytes += bytes.len();
            if p.pending_bytes >= FLUSH_BYTES {
                fold(p);
            }
        });
    }

    fn fold(p: &mut Parts) {
        let seq = js_sys::Array::new();
        if let Some(b) = p.blob.take() {
            seq.push(&b);
        }
        for v in p.pending.iter() {
            seq.push(&v);
        }
        p.blob = web_sys::Blob::new_with_u8_array_sequence(&seq).ok();
        p.pending = js_sys::Array::new();
        p.pending_bytes = 0;
    }

    struct State {
        items: Vec<Item>,
        next: usize,
        in_flight: usize,
        /// Failed fetches waiting out their back-off, and those ready to try again.
        waiting: usize,
        retry: Vec<usize>,
        tries: Vec<u32>,
        zip: ZipWriter,
        base: String,
        db: PathBuf,
        cols: Vec<Collection>,
        file: String,
        progress: Arc<Progress>,
        tx: Sender<Result<String, String>>,
        ctx: egui::Context,
        error: Option<String>,
        finished: bool,
    }

    pub fn start(ctx: egui::Context, db: &Path, base_url: String, sources: Vec<String>, all: Vec<Collection>, progress: Arc<Progress>, tx: Sender<Result<String, String>>) -> Result<(), String> {
        if base_url.is_empty() {
            return Err("No asset URL configured.".into());
        }
        if PARTS.with(|p| p.borrow().is_some()) {
            return Err("A download is already running".into());
        }
        let items = items(&open_catalog(db)?, &sources)?;
        progress.total.store(items.len(), Ordering::Relaxed);
        let mut cols: Vec<Collection> = all.into_iter().filter(|c| sources.is_empty() || sources.contains(&c.source)).collect();
        for c in &mut cols {
            c.have = 0;
            c.have_bytes = 0;
        }
        let scope = if sources.len() == 1 { sources[0].clone() } else if sources.is_empty() { "all".into() } else { "collections".into() };
        let file = format!("biocartoon-offline-{scope}-{}.zip", chrono::Local::now().format("%Y%m%d"));
        PARTS.with(|p| *p.borrow_mut() = Some(Parts { blob: None, pending: js_sys::Array::new(), pending_bytes: 0 }));
        let tries = vec![0; items.len()];
        let state = Arc::new(Mutex::new(State {
            items,
            next: 0,
            in_flight: 0,
            waiting: 0,
            retry: Vec::new(),
            tries,
            zip: ZipWriter::default(),
            base: base_url.trim_end_matches('/').to_string(),
            db: db.to_path_buf(),
            cols,
            file,
            progress,
            tx,
            ctx,
            error: None,
            finished: false,
        }));
        pump(&state);
        Ok(())
    }

    /// Start fetches up to the limit, or finish once everything has come back.
    fn pump(state: &Arc<Mutex<State>>) {
        let mut s = state.lock().unwrap();
        let stop = s.progress.cancel.load(Ordering::Relaxed) || s.error.is_some();
        while !stop && s.in_flight < FETCHES && (!s.retry.is_empty() || s.next < s.items.len()) {
            let i = match s.retry.pop() {
                Some(i) => i,
                None => {
                    s.next += 1;
                    s.next - 1
                }
            };
            s.tries[i] += 1;
            s.in_flight += 1;
            let url = format!("{}/{}", s.base, s.items[i].key);
            let st = state.clone();
            ehttp::fetch(ehttp::Request::get(&url), move |res| {
                let bytes = res.ok().filter(|r| r.ok).map(|r| r.bytes).filter(|b| b.starts_with(b"\x89PNG"));
                {
                    let mut s = st.lock().unwrap();
                    s.in_flight -= 1;
                    if bytes.is_none() && s.tries[i] < TRIES && !s.progress.cancel.load(Ordering::Relaxed) {
                        s.waiting += 1;
                        let delay = 500 << s.tries[i];
                        drop(s);
                        let st2 = st.clone();
                        retry_after(delay, move || {
                            {
                                let mut s = st2.lock().unwrap();
                                s.waiting -= 1;
                                s.retry.push(i);
                            }
                            pump(&st2);
                        });
                        pump(&st);
                        return;
                    }
                    s.progress.done.fetch_add(1, Ordering::Relaxed);
                    match bytes {
                        Some(b) => {
                            let (key, source) = (s.items[i].key.clone(), s.items[i].source.clone());
                            match s.zip.entry(&format!("{ZIP_ROOT}/assets/{key}"), &b) {
                                Ok(header) => {
                                    push(&header);
                                    push(&b);
                                    s.progress.bytes.fetch_add(b.len() as u64, Ordering::Relaxed);
                                    if let Some(c) = s.cols.iter_mut().find(|c| c.source == source) {
                                        c.have += 1;
                                        c.have_bytes += b.len() as u64;
                                    }
                                }
                                Err(e) => s.error = Some(e),
                            }
                        }
                        None => {
                            s.progress.failed.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    s.ctx.request_repaint();
                }
                pump(&st);
            });
        }
        let done = stop || (s.next >= s.items.len() && s.retry.is_empty() && s.waiting == 0);
        if done && s.in_flight == 0 && !s.finished {
            s.finished = true;
            let r = finish(&mut s);
            let _ = s.tx.send(r);
            s.ctx.request_repaint();
        }
    }

    fn retry_after(ms: i32, f: impl FnOnce() + 'static) {
        use wasm_bindgen::JsCast;
        let cb = wasm_bindgen::closure::Closure::once_into_js(f);
        if let Some(w) = web_sys::window() {
            let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(cb.unchecked_ref(), ms);
        }
    }

    fn finish(s: &mut State) -> Result<String, String> {
        let parts = PARTS.with(|p| p.borrow_mut().take());
        if let Some(e) = s.error.take() {
            return Err(e);
        }
        if s.progress.cancel.load(Ordering::Relaxed) {
            return Ok("Download cancelled".into());
        }
        let got: usize = s.cols.iter().map(|c| c.have).sum();
        let failed = s.progress.failed.load(Ordering::Relaxed);
        if got == 0 {
            return Err(format!("couldn't download any of the {failed} images"));
        }
        let mut parts = parts.ok_or("download state lost")?;
        let catalog = crate::storage::read(&s.db).map_err(|e| format!("catalog: {e}"))?;
        let text = readme(&s.cols, &catalog_version(&s.db), &s.base, true);
        let mut zip = std::mem::take(&mut s.zip);
        for (name, data) in [("assets.sqlite", &catalog[..]), ("README.md", text.as_bytes())] {
            let header = zip.entry(&format!("{ZIP_ROOT}/{name}"), data)?;
            parts.pending.push(&js_sys::Uint8Array::from(&header[..]));
            parts.pending.push(&js_sys::Uint8Array::from(data));
        }
        parts.pending.push(&js_sys::Uint8Array::from(&zip.finish()[..]));
        fold(&mut parts);
        let blob = parts.blob.ok_or("couldn't assemble the zip")?;
        crate::platform::download_blob(&s.file, &blob)?;
        let mut msg = format!("Downloaded {} ({got} images)", s.file);
        if failed > 0 {
            msg.push_str(&format!("; {failed} couldn't be fetched"));
        }
        Ok(msg)
    }
}

/// Desktop: delete downloaded images (see [`remove`]) off the UI thread, then rescan.
#[cfg(not(target_arch = "wasm32"))]
fn remove_and_scan(db: PathBuf, base_url: String, offline_dir: PathBuf, sources: Vec<String>) -> Receiver<Result<Vec<Collection>, String>> {
    let (tx, rx) = channel();
    crate::platform::spawn("offline-remove", move || {
        let r = remove(&db, &base_url, &offline_dir, &sources).and_then(|_| scan_now(&db, Some(&offline_dir.join("assets"))));
        let _ = tx.send(r);
    });
    rx
}

#[cfg(not(target_arch = "wasm32"))]
fn show_folder(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(target_os = "windows")]
    let opener = "explorer";
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let opener = "xdg-open";
    let _ = std::process::Command::new(opener).arg(dir).spawn();
}

// ---------- the Settings > Offline (web: Bulk download) tab ----------

#[derive(Default)]
pub struct OfflineState {
    pub job: Option<Job>,
    collections: Vec<Collection>,
    error: Option<String>,
    scan: Option<Receiver<Result<Vec<Collection>, String>>>,
    /// The catalog file `collections` was read from.
    scanned: Option<PathBuf>,
    /// Sync pressed: a catalog check is running and its result should be reported.
    pub(crate) syncing: bool,
}

/// The tab's name: the browser can't work offline, it only downloads zips.
pub const TAB_LABEL: &str = if cfg!(target_arch = "wasm32") { "Bulk download" } else { "Offline" };

impl crate::app::BioCartoon {
    fn offline_dir(&self) -> PathBuf {
        self.images.cache_dir().and_then(|d| d.parent().map(Path::to_path_buf)).unwrap_or_default()
    }

    /// Ask for a folder to keep offline packs and fetched images in; `None` when cancelled.
    #[cfg(not(target_arch = "wasm32"))]
    fn choose_offline_dir(&mut self) -> Option<PathBuf> {
        let dir = rfd::FileDialog::new().set_title("Choose where to keep downloaded images").set_directory(self.offline_dir()).pick_folder()?;
        self.settings.offline_dir = dir.display().to_string();
        self.images.set_cache_dir(dir.join("assets"));
        self.images.retry_failed();
        self.offline_rescan();
        Some(dir)
    }

    fn offline_rescan(&mut self) {
        let db = self.catalog.db_path.clone();
        self.offline.scanned = Some(db.clone());
        if db.as_os_str().is_empty() {
            self.offline.error = Some("The icon catalog hasn't downloaded yet.".into());
            return;
        }
        self.offline.scan = Some(scan(db, self.images.cache_dir()));
    }

    /// Fetch the catalog's current version from the asset URL (installing it if it is newer) and
    /// recount what is downloaded.
    fn offline_sync(&mut self) {
        if self.settings.catalog_path.is_empty() {
            let dir = self.app_dir.join("catalog");
            self.catalog_sync = Some(crate::catalog::CatalogSync::start(&self.settings.asset_base_url, dir));
            self.offline.syncing = true;
        }
        self.offline_rescan();
    }

    /// Collect scan and download results. Call once per frame.
    pub(crate) fn poll_offline(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.offline.scan {
            match rx.try_recv() {
                Ok(r) => {
                    self.offline.scan = None;
                    match r {
                        Ok(c) => {
                            self.offline.collections = c;
                            self.offline.error = None;
                        }
                        Err(e) => self.offline.error = Some(e),
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.offline.scan = None,
            }
        }
        let Some(job) = &self.offline.job else { return };
        match job.poll() {
            Some(r) => {
                self.offline.job = None;
                match r {
                    Ok(m) => self.toast(m),
                    Err(e) => self.toast_err(e),
                }
                self.images.retry_failed();
                self.offline_rescan();
            }
            None => ctx.request_repaint_after(std::time::Duration::from_millis(200)),
        }
    }

    fn offline_start(&mut self, sources: Vec<String>) {
        if self.offline.job.is_some() {
            return self.toast("A download is already running");
        }
        let job = start(self.ui_ctx(), self.catalog.db_path.clone(), self.settings.asset_base_url.clone(), self.offline_dir(), sources, &self.offline.collections);
        self.offline.job = Some(job);
    }

    pub(crate) fn offline_tab(&mut self, ui: &mut egui::Ui) {
        use crate::theme;
        use egui_phosphor::regular as ph;
        let web = cfg!(target_arch = "wasm32");
        if self.offline.scan.is_none() && self.offline.scanned.as_ref() != Some(&self.catalog.db_path) {
            self.offline_rescan();
        }
        let dim = |t: &str| egui::RichText::new(t).small().color(theme::text_dim());
        ui.label(if web {
            "Download collections as a .zip: every image as a PNG, the icon catalog (assets.sqlite) and a README with license and credit details."
        } else {
            "Download collections to use BioCartoon without internet. Images you have used are kept automatically; a pack downloads whole collections ahead of time, with a copy of the icon catalog (assets.sqlite) and a README."
        });
        ui.add_space(8.0);
        if let Some(e) = &self.offline.error {
            ui.label(egui::RichText::new(e).color(theme::text_dim()));
            return;
        }
        if self.offline.collections.is_empty() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Reading the catalog...");
            });
            return;
        }
        let busy = self.offline.job.is_some();
        let mut start_with: Option<Vec<String>> = None;
        let mut remove_these: Option<Vec<String>> = None;
        egui::Grid::new("offline_packs").num_columns(if web { 4 } else { 5 }).spacing([16.0, 6.0]).striped(true).show(ui, |ui| {
            ui.label(dim("Collection"));
            ui.label(dim(if web { "Images" } else { "Downloaded / Total" }));
            ui.label(dim("Size"));
            ui.label("");
            if !web {
                ui.label("");
            }
            ui.end_row();
            for c in &self.offline.collections {
                ui.label(&c.name).on_hover_text(format!("{} · {}", c.source, c.licenses));
                ui.label(if web { c.count.to_string() } else { format!("{} / {}", c.have, c.count) });
                ui.label(human_bytes(c.bytes));
                let download = if web {
                    egui::Button::new(format!("{}  .zip", ph::DOWNLOAD_SIMPLE))
                } else if c.complete() {
                    egui::Button::new(egui::RichText::new(format!("{}  Download", ph::CHECK_CIRCLE)).color(theme::accent()))
                } else {
                    egui::Button::new(format!("{}  Download", ph::DOWNLOAD_SIMPLE))
                };
                let can_download = !busy && c.count > 0 && !c.complete();
                let r = ui.add_enabled(can_download, download.small());
                let r = if c.complete() { r.on_disabled_hover_text("Already downloaded") } else { r };
                if r.clicked() {
                    start_with = Some(vec![c.source.clone()]);
                }
                if !web && ui.add_enabled(!busy && c.have > 0, egui::Button::new(format!("{}  Delete", ph::TRASH)).small()).on_hover_text("Delete the downloaded images").clicked() {
                    remove_these = Some(vec![c.source.clone()]);
                }
                ui.end_row();
            }
            let (count, bytes): (usize, u64) = self.offline.collections.iter().fold((0, 0), |(n, b), c| (n + c.count, b + c.bytes));
            let have: usize = self.offline.collections.iter().map(|c| c.have).sum();
            let all_done = !web && have >= count;
            // A line across the grid, between the collections and their total. The cursor is at
            // the new row's top; half the row spacing above it is between the rows.
            let span = ui.min_rect().x_range();
            ui.painter().hline(span, ui.cursor().top() - 3.0, egui::Stroke::new(1.0, theme::border()));
            ui.label(egui::RichText::new("All").strong());
            ui.label(egui::RichText::new(if web { count.to_string() } else { format!("{have} / {count}") }).strong());
            ui.label(egui::RichText::new(human_bytes(bytes)).strong());
            let download = if web {
                egui::Button::new(format!("{}  .zip", ph::DOWNLOAD_SIMPLE))
            } else if all_done {
                egui::Button::new(egui::RichText::new(format!("{}  Download", ph::CHECK_CIRCLE)).color(theme::accent()))
            } else {
                egui::Button::new(format!("{}  Download", ph::DOWNLOAD_SIMPLE))
            };
            let r = ui.add_enabled(!busy && !all_done, download.small());
            let r = if all_done { r.on_disabled_hover_text("Everything is downloaded") } else { r };
            if r.clicked() {
                #[cfg(not(target_arch = "wasm32"))]
                if self.choose_offline_dir().is_some() {
                    start_with = Some(Vec::new());
                }
                #[cfg(target_arch = "wasm32")]
                {
                    start_with = Some(Vec::new());
                }
            }
            if !web && ui.add_enabled(!busy && have > 0, egui::Button::new(format!("{}  Delete", ph::TRASH)).small()).on_hover_text("Delete every downloaded image, the catalog copy and the README").clicked() {
                remove_these = Some(Vec::new());
            }
            ui.end_row();
        });
        ui.add_space(8.0);
        let have_bytes: u64 = self.offline.collections.iter().map(|c| c.have_bytes).sum();
        ui.horizontal(|ui| {
            let syncing = self.offline.syncing || self.offline.scan.is_some();
            let sync = egui::Button::new(format!("{}  {}", ph::ARROWS_CLOCKWISE, if syncing { "Syncing..." } else { "Sync" }));
            if ui.add_enabled(!busy && !syncing, sync).on_hover_text("Check for a newer icon catalog and recount the downloaded images").clicked() {
                self.offline_sync();
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                if ui.add_enabled(!busy, egui::Button::new(format!("{}  Choose directory", ph::FOLDER_SIMPLE))).on_hover_text("Where downloaded and cached images are kept").clicked() {
                    self.choose_offline_dir();
                }
                if ui.button(format!("{}  Show folder", ph::FOLDER_OPEN)).clicked() {
                    show_folder(&self.offline_dir());
                }
            }
        });
        if !web {
            ui.label(dim(&format!("Using {} in {}", human_bytes(have_bytes), self.offline_dir().display())));
        } else {
            ui.label(dim("Large downloads take a while; keep this tab open until the zip is saved."));
        }
        if let Some(job) = &self.offline.job {
            ui.add_space(8.0);
            let p = &job.progress;
            let (done, total) = (p.done.load(Ordering::Relaxed), p.total.load(Ordering::Relaxed));
            let failed = p.failed.load(Ordering::Relaxed);
            let frac = if total == 0 { 0.0 } else { done as f32 / total as f32 };
            let mut text = format!("{}: {done} of {total} · {}", job.label, human_bytes(p.bytes.load(Ordering::Relaxed)));
            if failed > 0 {
                text.push_str(&format!(" · {failed} failed"));
            }
            ui.add(egui::ProgressBar::new(frac).text(text).animate(true));
            let stopping = p.cancel.load(Ordering::Relaxed);
            if ui.add_enabled(!stopping, egui::Button::new(if stopping { "Stopping..." } else { "Cancel" })).clicked() {
                job.cancel();
            }
        }
        if let Some(sources) = start_with {
            self.offline_start(sources);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(sources) = remove_these {
            self.offline.scan = Some(remove_and_scan(self.catalog.db_path.clone(), self.settings.asset_base_url.clone(), self.offline_dir(), sources));
            self.images.retry_failed();
        }
        #[cfg(target_arch = "wasm32")]
        let _ = remove_these;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_path_stays_inside() {
        let d = Path::new("/x");
        assert_eq!(cache_path(d, "bioart/a.png"), Some(PathBuf::from("/x/bioart/a.png")));
        assert_eq!(cache_path(d, "../a.png"), None);
        assert_eq!(cache_path(d, "/etc/passwd"), None);
        assert_eq!(cache_path(d, ""), None);
    }

    /// Export (`request_for`) and the canvas read the offline copy instead of the network.
    #[test]
    fn offline_copy_is_used_first() {
        let dir = std::env::temp_dir().join(format!("biocartoon-offline-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("bioart")).unwrap();
        std::fs::write(dir.join("bioart/a.png"), b"x").unwrap();
        let images = crate::assets::ImageCache::new("https://example.invalid".into(), None);
        images.set_cache_dir(dir.clone());
        let src = |key: &str| crate::doc::ImageSource::Asset { id: String::new(), key: key.into(), svg: None, title: String::new(), source: String::new(), credit: String::new(), license: String::new() };
        assert!(matches!(images.request_for(&src("bioart/a.png")), Ok(crate::assets::Request::Path(p)) if p == dir.join("bioart/a.png")));
        assert!(matches!(images.request_for(&src("bioart/b.png")), Ok(crate::assets::Request::Url(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An asset with an SVG loads that, and falls back to the PNG when it can't be fetched (as
    /// offline with a pack, which holds only PNGs).
    #[test]
    fn svg_first_then_png() {
        use crate::assets::{fetch, Request};
        let dir = std::env::temp_dir().join(format!("biocartoon-svg-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("bioart")).unwrap();
        std::fs::write(dir.join("bioart/a.png"), b"png").unwrap();
        let images = crate::assets::ImageCache::new("https://example.invalid".into(), None);
        images.set_cache_dir(dir.clone());
        let src = crate::doc::ImageSource::Asset { id: String::new(), key: "bioart/a.png".into(), svg: Some("bioart/a.svg".into()), title: String::new(), source: String::new(), credit: String::new(), license: String::new() };
        assert_eq!(src.cache_key(), "asset:bioart/a.svg");
        let req = images.request_for(&src).unwrap();
        assert!(matches!(&req, Request::Either(svg, png) if matches!(**svg, Request::Url(ref u) if u.ends_with("/bioart/a.svg")) && matches!(**png, Request::Path(_))));
        assert_eq!(fetch(&req).unwrap(), b"png");
        // Once the SVG is on disk it is read from there.
        std::fs::write(dir.join("bioart/a.svg"), b"svg").unwrap();
        assert_eq!(fetch(&images.request_for(&src).unwrap()).unwrap(), b"svg");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn zip_round_trip() {
        let mut z = ZipWriter::default();
        let mut out = Vec::new();
        for (n, d) in [("p/a.txt", &b"hello"[..]), ("p/b/c.bin", &[0u8, 1, 2, 255][..])] {
            out.extend(z.entry(n, d).unwrap());
            out.extend_from_slice(d);
        }
        out.extend(z.finish());
        let dir = std::env::temp_dir().join(format!("biocartoon-zip-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("t.zip");
        std::fs::write(&f, &out).unwrap();
        let ok = std::process::Command::new("unzip").arg("-tq").arg(&f).status();
        let _ = std::fs::remove_dir_all(&dir);
        if let Ok(st) = ok {
            assert!(st.success(), "unzip -t rejected the archive");
        }
    }
}

