//! The editor's side of the asset catalog: queries run on a worker thread (the UI polls for
//! responses), and the published catalog is downloaded in the background. The search itself is
//! [`biocartoon_catalog::catalog::Worker`], shared with the command line.

pub use biocartoon_catalog::catalog::{AssetRow, CategoryInfo, Facets, Filter, IconSort, SourceInfo, StarUpdate, TaxonCount, Worker};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
#[cfg(target_arch = "wasm32")]
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc::Sender;
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::thread;

#[derive(Clone, Debug)]
pub enum Req {
    /// An empty `query` lists everything `filter` keeps.
    Search { query: String, filter: Filter, offset: usize, limit: usize, sort: IconSort },
    /// The user's own uploads (source `user` with a local copy), not the published ones.
    Library { offset: usize, limit: usize, sort: IconSort },
    /// The assets with these ids, in this order (the user's favorites).
    Favorites { ids: Vec<String> },
    /// Ids of the assets this install starred, as the usage file remembers them.
    MyStars,
    Categories { source: String },
    /// The search filters' choices that still find icons under `filter` (see [`Worker::facets`]).
    Facets { filter: Filter },
    Sources,
    /// Style presets of the asset with this id.
    Variants { id: String },
    /// The rows with these ids (to fill in credits of documents saved before they were stored).
    Lookup { ids: Vec<String> },
    /// Record a user-generated asset (already written to `png_local`).
    InsertUser { row: AssetRow },
    /// Take one of the user's own assets back out of the local catalog.
    RemoveUser { id: String },
    /// An asset was placed on a canvas; feeds `IconSort::Popular`.
    RecordUse { source: String, title: String },
    /// Star counts the Worker reported (or the user's own click, ahead of the Worker's answer);
    /// remembered in `usage.stars`, where they override the catalog's snapshot.
    SetStars { updates: Vec<StarUpdate> },
}

pub enum Resp {
    /// `total` counts every match (one per icon, colour variants counted once), not just the
    /// `rows` returned. `colors` counts the colours of each row that comes in several, by row id.
    Assets { req_id: u64, rows: Vec<AssetRow>, more: bool, total: i64, colors: HashMap<String, usize> },
    Categories { req_id: u64, source: String, cats: Vec<CategoryInfo> },
    MyStars { req_id: u64, ids: Vec<String> },
    Facets { req_id: u64, facets: Facets },
    Sources { req_id: u64, sources: Vec<SourceInfo> },
    Variants { req_id: u64, rows: Vec<AssetRow> },
    Lookup { req_id: u64, rows: Vec<AssetRow> },
    Inserted { req_id: u64, result: Result<(), String> },
    Removed { req_id: u64, result: Result<(), String> },
    /// Reply to requests with nothing to report.
    Done { req_id: u64 },
    Error { req_id: u64, message: String },
}

pub struct CatalogClient {
    #[cfg(not(target_arch = "wasm32"))]
    tx: Sender<(u64, Req)>,
    #[cfg(not(target_arch = "wasm32"))]
    rx: Receiver<Resp>,
    /// Web build: no threads, so queries run in `send` and their replies wait here for `poll`.
    #[cfg(target_arch = "wasm32")]
    worker: Option<Worker>,
    #[cfg(target_arch = "wasm32")]
    pending: Vec<Resp>,
    next_id: u64,
    pub db_path: PathBuf,
    pub status: String,
    pub ok: bool,
}


/// A page of results, with how many colours its recoloured icons come in.
fn assets(w: &Worker, req_id: u64, rows: Vec<AssetRow>, more: bool, total: i64) -> Resp {
    let ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
    let colors = match w.recolors(&ids) {
        Ok(m) => m.into_iter().map(|(id, v)| (id, v.len())).collect(),
        Err(e) => {
            log::warn!("recolors: {e}");
            HashMap::new()
        }
    };
    Resp::Assets { req_id, rows, more, total, colors }
}

/// One more than `limit` ids of the user's own uploads from `offset` (to tell whether there are
/// more), and how many there are in all. Popular falls back to newest first.
fn library(w: &Worker, offset: usize, limit: usize, sort: IconSort) -> rusqlite::Result<(Vec<String>, i64)> {
    let own = "source='user' AND png_local IS NOT NULL AND png_key IS NOT NULL";
    let order = match sort {
        IconSort::Alphabetical => "lower(coalesce(title,'')), id",
        IconSort::ReverseAlphabetical => "lower(coalesce(title,'')) DESC, id",
        _ => "rowid DESC",
    };
    let mut st = w.conn.prepare(&format!("SELECT id FROM assets WHERE {own} ORDER BY {order} LIMIT ?1 OFFSET ?2"))?;
    let ids = st.query_map(rusqlite::params![limit as i64 + 1, offset as i64], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
    let total = w.conn.query_row(&format!("SELECT count(*) FROM assets WHERE {own}"), [], |r| r.get(0))?;
    Ok((ids, total))
}

fn handle(w: &Worker, req_id: u64, req: Req) -> Resp {
    let err = |e: rusqlite::Error| Resp::Error { req_id, message: e.to_string() };
    match req {
        Req::Search { query, filter, offset, limit, sort } => match w.search_filtered(&query, &filter, offset, limit + 1, sort) {
            Ok(mut rows) => {
                let more = rows.len() > limit;
                rows.truncate(limit);
                let total = w.search_filtered_total(&query, &filter).unwrap_or((offset + rows.len()) as i64).max((offset + rows.len()) as i64);
                assets(w, req_id, rows, more, total)
            }
            Err(e) => err(e),
        },
        Req::Library { offset, limit, sort } => match library(w, offset, limit, sort) {
            Ok((ids, total)) => match w.by_ids(&ids) {
                Ok(mut rows) => {
                    let more = rows.len() > limit;
                    rows.truncate(limit);
                    assets(w, req_id, rows, more, total)
                }
                Err(e) => err(e),
            },
            Err(e) => err(e),
        },
        Req::Favorites { ids } => match w.by_ids(&ids) {
            Ok(rows) => {
                let total = rows.len() as i64;
                assets(w, req_id, rows, false, total)
            }
            Err(e) => err(e),
        },
        // No usage file (or one from before stars): none.
        Req::MyStars => {
            let ids = w.conn.prepare("SELECT id FROM usage.stars WHERE mine = 1").and_then(|mut st| st.query_map([], |r| r.get(0))?.collect()).unwrap_or_default();
            Resp::MyStars { req_id, ids }
        }
        Req::Categories { source } => match w.categories(&source) {
            Ok(cats) => Resp::Categories { req_id, source, cats },
            Err(e) => err(e),
        },
        Req::Facets { filter } => match w.facets(&filter) {
            Ok(facets) => Resp::Facets { req_id, facets },
            Err(e) => err(e),
        },
        Req::Sources => match w.sources() {
            Ok(sources) => Resp::Sources { req_id, sources },
            Err(e) => err(e),
        },
        Req::Variants { id } => match w.variants(&id) {
            Ok(rows) => Resp::Variants { req_id, rows },
            Err(e) => err(e),
        },
        Req::Lookup { ids } => match w.by_ids(&ids) {
            Ok(rows) => Resp::Lookup { req_id, rows },
            Err(e) => err(e),
        },
        Req::InsertUser { row } => Resp::Inserted { req_id, result: w.insert_user(&row).map_err(|e| e.to_string()) },
        Req::RemoveUser { id } => Resp::Removed { req_id, result: w.remove_user(&id).map(|_| ()).map_err(|e| e.to_string()) },
        Req::RecordUse { source, title } => {
            if let Err(e) = w.record_use(&source, &title) {
                log::warn!("record use: {e}");
            }
            Resp::Done { req_id }
        }
        Req::SetStars { updates } => {
            if let Err(e) = w.set_stars(&updates) {
                log::warn!("set stars: {e}");
            }
            Resp::Done { req_id }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl CatalogClient {
    pub fn new(db_path: PathBuf, usage_path: PathBuf) -> Self {
        let (tx, jrx) = channel::<(u64, Req)>();
        let (rtx, rx) = channel::<Resp>();
        let ok = db_path.exists();
        let status = if ok {
            format!("Catalog: {}", db_path.display())
        } else {
            "Downloading the icon catalog...".to_string()
        };
        let path = db_path.clone();
        thread::Builder::new()
            .name("catalog".into())
            .spawn(move || {
                // Checked first: SQLite treats an empty path as a temporary database.
                let worker = match path.exists().then(|| Worker::open(&path, &usage_path)).transpose() {
                    Ok(w) => w,
                    Err(e) => {
                        log::error!("{e}");
                        None
                    }
                };
                for (req_id, req) in jrx {
                    let resp = match &worker {
                        Some(w) => handle(w, req_id, req),
                        None => Resp::Error { req_id, message: format!("catalog unavailable: {}", path.display()) },
                    };
                    if rtx.send(resp).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn catalog worker");
        Self { tx, rx, next_id: 1, db_path, status, ok }
    }

    pub fn send(&mut self, req: Req) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let _ = self.tx.send((id, req));
        id
    }

    pub fn poll(&mut self) -> Vec<Resp> {
        let mut v = Vec::new();
        while let Ok(r) = self.rx.try_recv() {
            v.push(r);
        }
        v
    }
}

/// Downloads the catalog published by the biocartoon repository's `data/publish_catalog.py`
/// (`<base>/catalog/version.txt` and `<base>/catalog/assets.sqlite.gz`) into `dir` in the
/// background; see [`biocartoon_catalog::catalog::sync_catalog`].
pub struct CatalogSync {
    rx: Receiver<Result<Option<PathBuf>, String>>,
    pub bytes: Arc<AtomicU64>,
    pub running: bool,
}

/// The catalog file most recently downloaded into `dir`, if any.
pub fn cached_catalog(dir: &Path) -> Option<PathBuf> {
    let name = crate::storage::read_to_string(&dir.join("current.txt")).ok()?;
    let p = dir.join(name.trim());
    crate::storage::exists(&p).then_some(p)
}

#[cfg(not(target_arch = "wasm32"))]
impl CatalogSync {
    pub fn start(base_url: &str, dir: PathBuf) -> Self {
        let (tx, rx) = channel();
        let bytes = Arc::new(AtomicU64::new(0));
        let base = base_url.trim_end_matches('/').to_string();
        let b = bytes.clone();
        thread::Builder::new()
            .name("catalog-sync".into())
            .spawn(move || {
                let _ = tx.send(biocartoon_catalog::catalog::sync_catalog(&base, &dir, &b));
            })
            .expect("spawn catalog sync");
        Self { rx, bytes, running: true }
    }

    /// `Some(Ok(Some(path)))` when a new catalog was installed, `Some(Ok(None))` when the cached
    /// one is current, `None` while still running.
    pub fn poll(&mut self) -> Option<Result<Option<PathBuf>, String>> {
        let r = self.rx.try_recv().ok()?;
        self.running = false;
        Some(r)
    }
}

/// Web build: the catalog is downloaded into [`crate::storage`] and queried on the UI thread
/// (the published file is ~20 MB with a prebuilt FTS index, so queries take milliseconds).
#[cfg(target_arch = "wasm32")]
impl CatalogClient {
    pub fn new(db_path: PathBuf, _usage_path: PathBuf) -> Self {
        let (worker, status) = if db_path.as_os_str().is_empty() {
            (None, "Downloading the icon catalog...".to_string())
        } else {
            match crate::storage::read(&db_path).map_err(|e| e.to_string()).and_then(|b| Worker::from_bytes(&b)) {
                Ok(w) => (Some(w), "Catalog: downloaded copy".to_string()),
                Err(e) => {
                    log::error!("{e}");
                    (None, format!("catalog unavailable: {e}"))
                }
            }
        };
        let ok = worker.is_some();
        Self { worker, pending: Vec::new(), next_id: 1, db_path, status, ok }
    }

    pub fn send(&mut self, req: Req) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let resp = match &self.worker {
            Some(w) => handle(w, id, req),
            None => Resp::Error { req_id: id, message: self.status.clone() },
        };
        self.pending.push(resp);
        id
    }

    pub fn poll(&mut self) -> Vec<Resp> {
        std::mem::take(&mut self.pending)
    }
}

#[cfg(target_arch = "wasm32")]
impl CatalogSync {
    pub fn start(base_url: &str, dir: PathBuf) -> Self {
        let (tx, rx) = channel();
        let bytes = Arc::new(AtomicU64::new(0));
        let base = base_url.trim_end_matches('/').to_string();
        if base.is_empty() {
            let _ = tx.send(Err("No asset URL configured".into()));
            return Self { rx, bytes, running: true };
        }
        let b = bytes.clone();
        ehttp::fetch(ehttp::Request::get(format!("{base}/catalog/version.txt")), move |res| {
            let version = match http_ok(res) {
                Ok(r) => r.text().unwrap_or_default().trim().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect::<String>(),
                Err(e) => {
                    let _ = tx.send(Err(format!("catalog version: {e}")));
                    return;
                }
            };
            if version.is_empty() {
                let _ = tx.send(Err("catalog version is empty".into()));
                return;
            }
            let name = format!("assets-{version}.sqlite");
            let dest = dir.join(&name);
            if cached_catalog(&dir).as_deref() == Some(dest.as_path()) {
                let _ = tx.send(Ok(None));
                return;
            }
            ehttp::fetch(ehttp::Request::get(format!("{base}/catalog/assets.sqlite.gz")), move |res| {
                let r = http_ok(res).map_err(|e| format!("catalog download: {e}")).and_then(|r| {
                    b.store(r.bytes.len() as u64, Ordering::Relaxed);
                    let mut db = Vec::new();
                    std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(&r.bytes[..]), &mut db)
                        .map_err(|e| format!("catalog download: {e}"))?;
                    // Only one copy is kept: the previous version (if any) is dropped.
                    let _ = crate::storage::remove_dir_all(&dir);
                    crate::storage::write(&dest, &db).map_err(|e| e.to_string())?;
                    crate::storage::write(&dir.join("current.txt"), name.as_bytes()).map_err(|e| e.to_string())?;
                    Ok(Some(dest))
                });
                let _ = tx.send(r);
            });
        });
        Self { rx, bytes, running: true }
    }

    pub fn poll(&mut self) -> Option<Result<Option<PathBuf>, String>> {
        let r = self.rx.try_recv().ok()?;
        self.running = false;
        Some(r)
    }
}

#[cfg(target_arch = "wasm32")]
fn http_ok(res: ehttp::Result<ehttp::Response>) -> Result<ehttp::Response, String> {
    let r = res?;
    if r.ok { Ok(r) } else { Err(format!("HTTP {} {}", r.status, r.status_text)) }
}

