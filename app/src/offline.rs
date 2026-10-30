//! Offline packs and bulk downloads.
//!
//! Desktop: images live under `<app data>/offline/assets/<key>` (an icon's PNG, and its SVG when
//! it has one). The image loader writes every image it fetches there and reads from there before
//! the network, so anything used once keeps working without internet; a pack fills the folder for
//! whole collections up front and adds `assets.sqlite` (a copy of the catalog) and `README.md`
//! next to it. Templates go there too: their JSON, thumbnail and every image they use.
//!
//! An icon is downloaded once all its files are there, a template once it and its images are;
//! that is what Settings > Offline counts and the Downloads filter lists.
//!
//! Web: the browser can't keep a folder, so the same pack is downloaded as a .zip with the
//! same layout under `biocartoon-offline/`. The zip is assembled in a Blob a few files at a
//! time, so a pack of a gigabyte or more never sits in wasm memory.

use crate::templates::TemplateRow;
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

/// The Templates row of Settings > Offline, which is not a collection of the catalog.
pub const TEMPLATES: &str = "templates";

/// One collection of the catalog (or [`TEMPLATES`]), and how much of it is downloaded.
#[derive(Clone, Debug, Default)]
pub struct Collection {
    pub source: String,
    pub name: String,
    pub count: usize,
    pub bytes: u64,
    pub licenses: String,
    /// Icons (or templates) already on this computer (always 0 in the browser).
    pub have: usize,
    pub have_bytes: u64,
    /// Some sizes aren't known, so `bytes` is short: a template's is known once it is downloaded.
    pub more: bool,
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

/// Whether the catalog's `assets` table has `col` (catalogs from before SVGs don't).
fn has_col(conn: &Connection, col: &str) -> bool {
    conn.query_row("SELECT count(*) FROM pragma_table_info('assets') WHERE name = ?1", [col], |r| r.get::<_, i64>(0)).map(|n| n > 0).unwrap_or(false)
}

/// The SQL for a row's SVG key (NULL when it has none) and for the bytes of all its files.
fn file_cols(conn: &Connection) -> (&'static str, &'static str) {
    match (has_col(conn, "svg_key"), has_col(conn, "svg_bytes")) {
        (true, true) => ("nullif(svg_key, '')", "coalesce(png_bytes, 0) + coalesce(svg_bytes, 0)"),
        (true, false) => ("nullif(svg_key, '')", "coalesce(png_bytes, 0)"),
        _ => ("NULL", "coalesce(png_bytes, 0)"),
    }
}

/// The collection a row belongs to: uploads accepted to BioCartoon count as BioCartoon (see
/// [`crate::theme::asset_source`]).
fn source_sql() -> String {
    use crate::theme::{BIOCARTOON_AUTHOR, BIOCARTOON_SOURCE};
    format!("CASE WHEN source = 'user' AND '{BIOCARTOON_AUTHOR}' IN (source_name, author) THEN '{BIOCARTOON_SOURCE}' ELSE source END")
}

/// Every collection (User-generated always listed, even when empty): BioCartoon first, then
/// the rest in order of the databases' two-letter badges.
fn collections(conn: &Connection) -> Result<Vec<Collection>, String> {
    let (_, bytes) = file_cols(conn);
    let mut st = conn
        .prepare(&format!(
            "SELECT {} AS src, max(source_name), count(*), coalesce(sum({bytes}), 0), group_concat(DISTINCT license) \
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
                ..Default::default()
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

/// One icon: its PNG (`key`), its SVG when it has one, and the bytes of both.
struct Item {
    source: String,
    key: String,
    svg: Option<String>,
    bytes: u64,
}

impl Item {
    fn files(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.key.as_str()).chain(self.svg.as_deref())
    }

    /// All its files are under `dir`.
    fn complete(&self, dir: &Path) -> bool {
        self.files().all(|k| have_file(dir, k))
    }
}

fn have_file(dir: &Path, key: &str) -> bool {
    cache_path(dir, key).is_some_and(|p| p.is_file())
}

/// Every icon of `sources` (all collections when empty).
fn items(conn: &Connection, sources: &[String]) -> Result<Vec<Item>, String> {
    let (svg, bytes) = file_cols(conn);
    let mut st = conn
        .prepare(&format!("SELECT {} AS src, png_key, {svg}, {bytes} FROM assets WHERE png_key IS NOT NULL AND png_key != '' ORDER BY src, png_key", source_sql()))
        .map_err(|e| e.to_string())?;
    let rows = st
        .query_map([], |r| Ok(Item { source: r.get(0)?, key: r.get(1)?, svg: r.get(2)?, bytes: r.get::<_, i64>(3)? as u64 }))
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

// ---------- templates ----------

/// The images a template's slide draws from the bucket: each one's PNG key, and SVG key when it has one.
fn template_images(t: &crate::templates::TemplateFile) -> Vec<(String, Option<String>)> {
    use crate::doc::{ImageSource, ItemKind};
    t.slide
        .items
        .iter()
        .filter_map(|it| match &it.kind {
            ItemKind::Image { source: ImageSource::Asset { key, svg, .. }, .. } if !key.is_empty() => Some((key.clone(), svg.clone().filter(|s| !s.is_empty()))),
            _ => None,
        })
        .collect()
}

/// Where a template's JSON and thumbnail are on this computer: its own files when it was made
/// here, else their places under `dir`.
fn template_files(dir: &Path, row: &TemplateRow) -> (Option<PathBuf>, Option<PathBuf>) {
    let json = row.json_local.as_deref().map(PathBuf::from).or_else(|| cache_path(dir, &row.json_key));
    let thumb = row.thumb_local.as_deref().map(PathBuf::from).or_else(|| cache_path(dir, &row.meta.thumb_key));
    (json, thumb)
}

/// The Templates row: how many of `rows` are downloaded under `dir` (none in the browser), and
/// their ids. `sizes` are the catalog icons' bytes by PNG key.
fn scan_templates(dir: Option<&Path>, rows: &[TemplateRow], sizes: &std::collections::HashMap<&str, u64>) -> (Collection, Vec<String>) {
    let mut c = Collection { source: TEMPLATES.into(), name: "Templates".into(), count: rows.len(), ..Default::default() };
    let mut ids = Vec::new();
    let file_len = |p: Option<PathBuf>| p.and_then(|p| std::fs::metadata(p).ok()).map_or(0, |m| m.len());
    for row in rows {
        let Some(dir) = dir else {
            c.more = true;
            continue;
        };
        let (json, thumb) = template_files(dir, row);
        // Its images are only known from its JSON.
        let Some(t) = json.and_then(|p| crate::storage::read(&p).ok()).and_then(|b| crate::templates::parse(&b).ok().map(|t| (t, b.len() as u64))) else {
            c.more = true;
            continue;
        };
        let (t, json_bytes) = t;
        let mut complete = row.meta.thumb_key.is_empty() && row.thumb_local.is_none() || thumb.as_ref().is_some_and(|p| p.is_file());
        let mut bytes = json_bytes + file_len(thumb);
        for (png, svg) in template_images(&t) {
            complete &= have_file(dir, &png) && svg.as_deref().is_none_or(|k| have_file(dir, k));
            bytes += sizes.get(png.as_str()).copied().unwrap_or_else(|| {
                // The template's own images aren't in the catalog: their size on disk, if there.
                std::iter::once(&png).chain(svg.as_ref()).map(|k| file_len(cache_path(dir, k))).sum()
            });
        }
        c.bytes += bytes;
        if complete {
            c.have += 1;
            c.have_bytes += bytes;
            ids.push(row.meta.id.clone());
        }
    }
    (c, ids)
}

/// Desktop: put `key`'s file under `dir` unless it is there already, fetching it from `base`
/// (trying again when the connection drops). The bytes fetched.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn ensure(base: &str, dir: &Path, key: &str, cancel: &AtomicBool) -> Result<u64, String> {
    let path = cache_path(dir, key).ok_or_else(|| format!("bad key {key:?}"))?;
    if path.is_file() {
        return Ok(0);
    }
    let req = crate::assets::Request::Url(format!("{}/{key}", base.trim_end_matches('/')));
    let mut res = crate::assets::fetch(&req);
    for n in 1..TRIES {
        if res.is_ok() || cancel.load(Ordering::Relaxed) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500 << n));
        res = crate::assets::fetch(&req);
    }
    let b = res?;
    // An error page served with a 200 is not the file.
    let ok = if key.ends_with(".json") { std::str::from_utf8(&b).is_ok() } else { b.starts_with(b"\x89PNG") || crate::assets::is_svg(&b) };
    if !ok {
        return Err(format!("{key}: unexpected response"));
    }
    store(dir, key, &b);
    Ok(b.len() as u64)
}

/// Desktop: download a template for offline use into `dir` (the offline folder's `assets/`):
/// its JSON and thumbnail, and every image it uses with its SVG. The bytes fetched.
#[cfg(not(target_arch = "wasm32"))]
pub fn download_template(base: &str, dir: &Path, row: &TemplateRow, cancel: &AtomicBool) -> Result<u64, String> {
    if base.is_empty() {
        return Err("No asset URL configured.".into());
    }
    let mut got = 0;
    let json = match &row.json_local {
        Some(p) => crate::storage::read(Path::new(p)).map_err(|e| format!("{p}: {e}"))?,
        None => {
            got += ensure(base, dir, &row.json_key, cancel)?;
            crate::storage::read(&cache_path(dir, &row.json_key).unwrap_or_default()).map_err(|e| e.to_string())?
        }
    };
    let t = crate::templates::parse(&json)?;
    if row.thumb_local.is_none() && !row.meta.thumb_key.is_empty() {
        got += ensure(base, dir, &row.meta.thumb_key, cancel)?;
    }
    for (png, svg) in template_images(&t) {
        got += ensure(base, dir, &png, cancel)?;
        if let Some(svg) = svg {
            got += ensure(base, dir, &svg, cancel)?;
        }
    }
    Ok(got)
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
    let name = |s: &String| match all.iter().find(|c| &c.source == s) {
        Some(c) => c.name.clone(),
        None if s == TEMPLATES => "Templates".into(),
        None => s.clone(),
    };
    let names: Vec<String> = sources.iter().map(name).collect();
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
    s.push_str("## Contents\n\n| Collection | Icons | Size | Licenses |\n|---|---|---|---|\n");
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
         - `assets/<png_key>`: the PNG of each row, for example `assets/bioart/2_626557.png`, and \
         `assets/<svg_key>` its SVG when it has one.\n\n\
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
/// What a scan of the offline folder found.
#[derive(Default)]
pub struct Scanned {
    /// The catalog's collections with what is downloaded of each.
    pub cols: Vec<Collection>,
    /// The PNG keys of the icons downloaded.
    pub keys: Vec<String>,
    /// The Templates row, and the ids of the templates downloaded.
    pub templates: Collection,
    pub template_ids: Vec<String>,
}

pub fn scan(db: PathBuf, cache_dir: Option<PathBuf>, templates: Vec<TemplateRow>) -> Receiver<Result<Scanned, String>> {
    let (tx, rx) = channel();
    crate::platform::spawn("offline-scan", move || {
        let _ = tx.send(scan_all(&db, cache_dir.as_deref(), &templates));
    });
    rx
}

#[cfg(not(target_arch = "wasm32"))]
fn scan_now(db: &Path, cache_dir: Option<&Path>) -> Result<Vec<Collection>, String> {
    scan_all(db, cache_dir, &[]).map(|s| s.cols)
}

fn scan_all(db: &Path, cache_dir: Option<&Path>, templates: &[TemplateRow]) -> Result<Scanned, String> {
    let conn = open_catalog(db)?;
    let mut cols = collections(&conn)?;
    let mut keys = Vec::new();
    let all = items(&conn, &[])?;
    if let Some(dir) = cache_dir {
        for it in &all {
            if it.complete(dir) {
                if let Some(c) = cols.iter_mut().find(|c| c.source == it.source) {
                    c.have += 1;
                    c.have_bytes += it.bytes;
                }
                keys.push(it.key.clone());
            }
        }
    }
    let sizes = all.iter().map(|it| (it.key.as_str(), it.bytes)).collect();
    let (templates, template_ids) = scan_templates(cache_dir, templates, &sizes);
    Ok(Scanned { cols, keys, templates, template_ids })
}

/// Desktop: download every missing file of `sources`' icons (all collections when empty) into
/// `offline/assets`, and `templates` when `sources` is empty or names [`TEMPLATES`]; then refresh
/// `offline/assets.sqlite` and `offline/README.md`.
#[cfg(not(target_arch = "wasm32"))]
pub fn start(ctx: egui::Context, db: PathBuf, base_url: String, offline_dir: PathBuf, sources: Vec<String>, all: &[Collection], templates: Vec<TemplateRow>) -> Job {
    let label = scope_label(&sources, all);
    let progress = Arc::new(Progress::default());
    let (tx, rx) = channel();
    let p = progress.clone();
    let what = label.clone();
    crate::platform::spawn("offline-pack", move || {
        let templates = if sources.is_empty() || sources.iter().any(|s| s == TEMPLATES) { templates } else { Vec::new() };
        let r = download_pack(&db, &base_url, &offline_dir, &sources, &templates, &p).map(|(got, failed)| {
            if p.cancel.load(Ordering::Relaxed) {
                return format!("Offline download stopped; the {got} downloaded so far are kept");
            }
            let mut msg = format!("Offline pack ready: {what} ({got} downloaded)");
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

/// One thing a pack downloads.
#[cfg(not(target_arch = "wasm32"))]
enum Task<'a> {
    Icon(Item),
    Template(&'a TemplateRow),
}

#[cfg(not(target_arch = "wasm32"))]
fn download_pack(db: &Path, base_url: &str, offline_dir: &Path, sources: &[String], templates: &[TemplateRow], p: &Progress) -> Result<(usize, usize), String> {
    use std::sync::Mutex;
    if base_url.is_empty() {
        return Err("No asset URL configured.".into());
    }
    let assets_dir = offline_dir.join("assets");
    let todo: Vec<Task> = {
        let conn = open_catalog(db)?;
        let all = items(&conn, sources)?;
        let (have, missing): (Vec<Item>, Vec<Item>) = all.into_iter().partition(|it| it.complete(&assets_dir));
        // A template's images are only known once its JSON is here, so each is checked as it goes.
        p.total.store(have.len() + missing.len() + templates.len(), Ordering::Relaxed);
        p.done.store(have.len(), Ordering::Relaxed);
        missing.into_iter().map(Task::Icon).chain(templates.iter().map(Task::Template)).collect()
    };
    let next = AtomicUsize::new(0);
    let got = AtomicUsize::new(0);
    let first_error: Mutex<Option<String>> = Mutex::new(None);
    std::thread::scope(|s| {
        for _ in 0..8 {
            s.spawn(|| loop {
                if p.cancel.load(Ordering::Relaxed) {
                    break;
                }
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(task) = todo.get(i) else { break };
                let res = match task {
                    Task::Icon(it) => it.files().try_fold(0, |n, k| ensure(base_url, &assets_dir, k, &p.cancel).map(|b| n + b)),
                    Task::Template(row) => download_template(base_url, &assets_dir, row, &p.cancel),
                };
                match res {
                    Ok(b) => {
                        p.bytes.fetch_add(b, Ordering::Relaxed);
                        if b > 0 {
                            got.fetch_add(1, Ordering::Relaxed);
                        }
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
        return Err(format!("couldn't download the files ({e})"));
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

/// Desktop: delete the downloaded files of `sources` (all, with the catalog copy and README,
/// when empty). [`TEMPLATES`] deletes the templates' JSON, thumbnails and own images, but not the
/// catalog icons they use. Only removes what a pack writes, since `offline_dir` may be a folder
/// the user chose.
#[cfg(not(target_arch = "wasm32"))]
pub fn remove(db: &Path, base_url: &str, offline_dir: &Path, sources: &[String], templates: &[TemplateRow]) -> Result<(), String> {
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
    let rm = |key: &str| {
        if let Some(p) = cache_path(&assets_dir, key) {
            let _ = std::fs::remove_file(p);
        }
    };
    for it in items(&conn, sources)? {
        it.files().for_each(rm);
    }
    if sources.iter().any(|s| s == TEMPLATES) {
        for row in templates.iter().filter(|r| r.json_local.is_none()) {
            if let Some(t) = cache_path(&assets_dir, &row.json_key).and_then(|p| crate::storage::read(&p).ok()).and_then(|b| crate::templates::parse(&b).ok()) {
                // Its own images (`templates/<id>/img-<n>.png`) go with it; catalog icons stay.
                for (png, svg) in template_images(&t) {
                    std::iter::once(png).chain(svg).filter(|k| k.starts_with("templates/")).for_each(|k| rm(&k));
                }
            }
            rm(&row.json_key);
            rm(&row.meta.thumb_key);
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
pub fn start(ctx: egui::Context, db: PathBuf, base_url: String, _offline_dir: PathBuf, sources: Vec<String>, all: &[Collection], _templates: Vec<TemplateRow>) -> Job {
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

    /// One file of the zip: an icon's PNG (`png`, which is what counts it), or its SVG.
    struct File {
        source: String,
        key: String,
        png: bool,
    }

    struct State {
        items: Vec<File>,
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
        let items: Vec<File> = items(&open_catalog(db)?, &sources)?
            .into_iter()
            .flat_map(|it| {
                let svg = it.svg.map(|key| File { source: it.source.clone(), key, png: false });
                std::iter::once(File { source: it.source, key: it.key, png: true }).chain(svg)
            })
            .collect();
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
            let (url, png) = (format!("{}/{}", s.base, s.items[i].key), s.items[i].png);
            let st = state.clone();
            ehttp::fetch(ehttp::Request::get(&url), move |res| {
                let bytes = res.ok().filter(|r| r.ok).map(|r| r.bytes).filter(|b| if png { b.starts_with(b"\x89PNG") } else { crate::assets::is_svg(b) });
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
                                        c.have += usize::from(png);
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
            return Err(format!("couldn't download any of the {failed} files"));
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
        let mut msg = format!("Downloaded {} ({got} icons)", s.file);
        if failed > 0 {
            msg.push_str(&format!("; {failed} couldn't be fetched"));
        }
        Ok(msg)
    }
}

/// Desktop: delete downloaded images (see [`remove`]) off the UI thread, then rescan.
#[cfg(not(target_arch = "wasm32"))]
fn remove_and_scan(db: PathBuf, base_url: String, offline_dir: PathBuf, sources: Vec<String>, templates: Vec<TemplateRow>) -> Receiver<Result<Scanned, String>> {
    let (tx, rx) = channel();
    crate::platform::spawn("offline-remove", move || {
        let r = remove(&db, &base_url, &offline_dir, &sources, &templates).and_then(|_| scan_all(&db, Some(&offline_dir.join("assets")), &templates));
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
    /// The Templates row.
    templates: Collection,
    /// The templates downloaded (the Downloads filter).
    pub(crate) template_ids: std::collections::HashSet<String>,
    error: Option<String>,
    scan: Option<Receiver<Result<Scanned, String>>>,
    /// The image loader kept new files in the folder since the last scan, and when that was
    /// (`egui` time, seconds).
    stale: bool,
    scanned_at: f64,
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

    /// Recount what is downloaded, and tell the catalog (the Downloads filter).
    pub(crate) fn offline_rescan(&mut self) {
        self.offline.stale = false;
        self.offline.scanned_at = self.ui_ctx().input(|i| i.time);
        let db = self.catalog.db_path.clone();
        self.offline.scanned = Some(db.clone());
        if db.as_os_str().is_empty() {
            self.offline.error = Some("The icon catalog hasn't downloaded yet.".into());
            return;
        }
        self.offline.scan = Some(scan(db, self.images.cache_dir(), self.template_rows.clone()));
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
        // Files the image loader keeps in the folder may complete an icon or template: count again,
        // at most every few seconds.
        if !self.images.take_stored().is_empty() {
            self.offline.stale = true;
        }
        let now = ctx.input(|i| i.time);
        if self.offline.stale && self.offline.scan.is_none() && self.offline.job.is_none() && now - self.offline.scanned_at > 5.0 && !self.catalog.db_path.as_os_str().is_empty() {
            self.offline_rescan();
        }
        if let Some(rx) = &self.offline.scan {
            match rx.try_recv() {
                Ok(r) => {
                    self.offline.scan = None;
                    match r {
                        Ok(found) => {
                            self.offline.collections = found.cols;
                            self.offline.templates = found.templates;
                            self.offline.template_ids = found.template_ids.into_iter().collect();
                            self.offline.error = None;
                            if self.images.cache_dir().is_some() {
                                self.catalog.send(crate::catalog::Req::SetDownloads { keys: found.keys });
                                // The Downloads filter's results follow.
                                if self.icons.filter.downloads {
                                    self.icons.last_sent_query = "\u{0}".into();
                                }
                            }
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
        let job = start(self.ui_ctx(), self.catalog.db_path.clone(), self.settings.asset_base_url.clone(), self.offline_dir(), sources, &self.offline.collections, self.template_rows.clone());
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
            "Download collections as a .zip: every icon as a PNG, and as an SVG when it has one, the icon catalog (assets.sqlite) and a README with license and credit details."
        } else {
            "Download collections to use BioCartoon without internet. Images you have used are kept automatically; a pack downloads whole collections ahead of time (each icon's PNG and SVG, and templates with the icons they use), with a copy of the icon catalog (assets.sqlite) and a README."
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
            // Templates after the icons' collections (not in the browser, whose zips are of icons).
            let templates = (!web && self.offline.templates.count > 0).then_some(&self.offline.templates);
            for c in self.offline.collections.iter().chain(templates) {
                let tip = if c.source == TEMPLATES { "Each template, its thumbnail and the icons it uses".to_string() } else { format!("{} · {}", c.source, c.licenses) };
                ui.label(&c.name).on_hover_text(tip);
                ui.label(if web { c.count.to_string() } else { format!("{} / {}", c.have, c.count) });
                let size = ui.label(format!("{}{}", human_bytes(c.bytes), if c.more { "+" } else { "" }));
                if c.more {
                    size.on_hover_text("A template's size is known once it is downloaded");
                }
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
                if !web && ui.add_enabled(!busy && c.have > 0, egui::Button::new(format!("{}  Delete", ph::TRASH)).small()).on_hover_text("Delete the downloaded files").clicked() {
                    remove_these = Some(vec![c.source.clone()]);
                }
                ui.end_row();
            }
            // The count is of icons; the size and the buttons take in the templates too.
            let (count, bytes): (usize, u64) = self.offline.collections.iter().fold((0, 0), |(n, b), c| (n + c.count, b + c.bytes));
            let bytes = bytes + templates.map_or(0, |t| t.bytes);
            let more = templates.is_some_and(|t| t.more);
            let have: usize = self.offline.collections.iter().map(|c| c.have).sum();
            let all_done = !web && have >= count && templates.is_none_or(|t| t.have >= t.count);
            let any = have > 0 || templates.is_some_and(|t| t.have > 0);
            // A double line across the grid, between the collections and their total. The cursor is
            // at the new row's top; the row spacing above it is between the rows.
            let span = ui.min_rect().x_range();
            let y = ui.cursor().top() - 3.0;
            for dy in [-1.5, 1.5] {
                ui.painter().hline(span, y + dy, egui::Stroke::new(1.0, theme::text()));
            }
            ui.label(egui::RichText::new("All").strong());
            ui.label(egui::RichText::new(if web { count.to_string() } else { format!("{have} / {count}") }).strong());
            ui.label(egui::RichText::new(format!("{}{}", human_bytes(bytes), if more { "+" } else { "" })).strong());
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
            if !web && ui.add_enabled(!busy && any, egui::Button::new(format!("{}  Delete", ph::TRASH)).small()).on_hover_text("Delete every downloaded file, the catalog copy and the README").clicked() {
                remove_these = Some(Vec::new());
            }
            ui.end_row();
        });
        ui.add_space(8.0);
        let have_bytes: u64 = self.offline.collections.iter().map(|c| c.have_bytes).sum::<u64>() + self.offline.templates.have_bytes;
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
            self.offline.scan = Some(remove_and_scan(self.catalog.db_path.clone(), self.settings.asset_base_url.clone(), self.offline_dir(), sources, self.template_rows.clone()));
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

    /// A template is downloaded once its JSON, its thumbnail and each image it uses (with that
    /// image's SVG) are in the folder; it opens from there, and deleting it keeps the icons.
    #[test]
    fn templates_are_downloaded_with_their_images() {
        use crate::doc::{ImageSource, Item, ItemKind, Slide};
        use crate::templates::{TemplateFile, TemplateMeta};
        let dir = std::env::temp_dir().join(format!("cc-offline-templates-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let assets = dir.join("assets");
        let image = |key: &str, svg: Option<&str>| ItemKind::Image {
            source: ImageSource::Asset { id: key.into(), key: key.into(), svg: svg.map(str::to_string), title: String::new(), source: String::new(), credit: String::new(), license: String::new() },
            crop: [0.0, 0.0, 1.0, 1.0],
            overlay: [255, 255, 255, 0],
            layers: Vec::new(),
        };
        let items = vec![
            Item::new(1, "cell", image("bioart/cell.png", Some("bioart/cell.svg")), egui::Pos2::ZERO, egui::Vec2::splat(10.0)),
            Item::new(2, "own", image("templates/t/img-0.png", None), egui::Pos2::ZERO, egui::Vec2::splat(10.0)),
        ];
        let meta = TemplateMeta { id: "t".into(), title: "T".into(), thumb_key: "templates/t/thumb.png".into(), ..Default::default() };
        let file = TemplateFile { meta: meta.clone(), width: 10.0, height: 10.0, background: [255; 4], slide: Slide { name: String::new(), items, comments: Vec::new(), background: None } };
        let row = TemplateRow { meta, json_key: "templates/t/template.json".into(), json_local: None, thumb_local: None };
        let put = |key: &str, b: &[u8]| store(&assets, key, b);
        let sizes = std::collections::HashMap::from([("bioart/cell.png", 30)]);
        let scanned = || scan_templates(Some(&assets), std::slice::from_ref(&row), &sizes);

        // Not even its JSON yet: its size isn't known.
        let (c, ids) = scanned();
        assert_eq!((c.count, c.have, c.more), (1, 0, true));
        assert!(ids.is_empty());
        let json = serde_json::to_vec(&file).unwrap();
        put(&row.json_key, &json);
        put(&row.meta.thumb_key, b"\x89PNG thumb");
        put("bioart/cell.png", b"\x89PNG");
        put("templates/t/img-0.png", b"\x89PNG img");
        // The icon's SVG is missing.
        let (c, _) = scanned();
        assert_eq!((c.have, c.more), (0, false));
        assert_eq!(c.bytes, json.len() as u64 + 10 + 30 + 8);
        put("bioart/cell.svg", b"<svg/>");
        let (c, ids) = scanned();
        assert_eq!((c.have, ids), (1, vec!["t".to_string()]));
        assert_eq!(crate::templates::load(&row, "", Some(&assets)).unwrap().meta.id, "t");

        let db = dir.join("assets.sqlite");
        Connection::open(&db).unwrap().execute_batch("CREATE TABLE assets (id TEXT, source TEXT, source_name TEXT, author TEXT, license TEXT, png_key TEXT, png_bytes INTEGER);").unwrap();
        remove(&db, "", &dir, &[TEMPLATES.to_string()], std::slice::from_ref(&row)).unwrap();
        assert_eq!(scanned().0.have, 0);
        assert!(have_file(&assets, "bioart/cell.png") && have_file(&assets, "bioart/cell.svg"));
        assert!(!have_file(&assets, "templates/t/img-0.png") && !have_file(&assets, &row.json_key));
        let _ = std::fs::remove_dir_all(&dir);
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

    /// A library icon kept as an SVG (an AI drawing) has it by absolute path, read from disk.
    #[test]
    fn local_library_svg() {
        use crate::assets::{fetch, Request};
        let dir = std::env::temp_dir().join(format!("biocartoon-user-svg-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let svg = dir.join("a.svg");
        std::fs::write(&svg, b"svg").unwrap();
        let images = crate::assets::ImageCache::new("https://example.invalid".into(), None);
        let src = crate::doc::ImageSource::Asset { id: String::new(), key: "user/a.png".into(), svg: Some(svg.display().to_string()), title: String::new(), source: String::new(), credit: String::new(), license: String::new() };
        let req = images.request_for(&src).unwrap();
        assert!(matches!(&req, Request::Either(s, _) if matches!(**s, Request::Path(ref p) if *p == svg)));
        assert_eq!(fetch(&req).unwrap(), b"svg");
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

