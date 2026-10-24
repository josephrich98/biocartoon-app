//! Shared slide templates. Each template is a JSON file (`TemplateFile`) plus a thumbnail PNG,
//! listed in a local SQLite (`<app dir>/templates.sqlite`) that holds the user's own templates
//! and a copy of the published `catalog/templates.sqlite.gz` (built by data/publish_catalog.py).
//!
//! Shared templates are submitted to the review Worker (see `crate::submit`). Once approved they
//! live in the bucket under `templates/<id>/`: `template.json`, `thumb.png`, and `img-<n>.png` for
//! every image the slide uses that is not a catalog asset.

use crate::doc::{ImageSource, ItemKind, Slide};
use crate::submit::{self, Part, SubmitMeta};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

/// Web build: SQLite keeps its files in memory, so databases are copied in from
/// [`crate::storage`] when opened and back after every change.
#[cfg(target_arch = "wasm32")]
fn memvfs() -> sqlite_wasm_rs::MemVfsUtil<sqlite_wasm_rs::WasmOsCallback> {
    sqlite_wasm_rs::MemVfsUtil::new()
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TemplateMeta {
    pub id: String,
    pub title: String,
    pub description: String,
    pub category: String,
    pub tags: Vec<String>,
    pub author: String,
    pub license: String,
    pub created_at: String,
    pub thumb_key: String,
}

/// What `template.json` holds: the metadata, the page it was designed on, and one slide.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TemplateFile {
    pub meta: TemplateMeta,
    pub width: f32,
    pub height: f32,
    pub background: [u8; 4],
    pub slide: Slide,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TemplateRow {
    pub meta: TemplateMeta,
    pub json_key: String,
    /// Set for templates made on this computer (they work offline and before publishing).
    pub json_local: Option<String>,
    pub thumb_local: Option<String>,
}

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS templates (
    id TEXT PRIMARY KEY, title TEXT NOT NULL, description TEXT, category TEXT, author TEXT,
    license TEXT, created_at TEXT, json_key TEXT NOT NULL, thumb_key TEXT,
    json_local TEXT, thumb_local TEXT,
    tags TEXT NOT NULL DEFAULT '[]');
CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v TEXT);";

/// Whether `table` (e.g. `pub.templates`) has a `tags` column: lists made before templates had tags don't.
fn has_tags(c: &Connection, table: &str) -> bool {
    c.prepare(&format!("SELECT tags FROM {table} LIMIT 0")).is_ok()
}

pub struct TemplateStore {
    conn: Option<Connection>,
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    path: PathBuf,
}

impl TemplateStore {
    pub fn open(path: &Path) -> Self {
        #[cfg(target_arch = "wasm32")]
        if let Ok(bytes) = crate::storage::read(path) {
            let name = path.display().to_string();
            if !memvfs().exists(&name) {
                if let Err(e) = memvfs().import_db(&name, &bytes) {
                    log::error!("templates {name}: {e}");
                }
            }
        }
        let conn = Connection::open(path).and_then(|c| c.execute_batch(SCHEMA).map(|_| c)).and_then(|c| {
            if !has_tags(&c, "templates") {
                c.execute_batch("ALTER TABLE templates ADD COLUMN tags TEXT NOT NULL DEFAULT '[]';")?;
            }
            Ok(c)
        });
        match conn {
            Ok(c) => Self { conn: Some(c), path: path.to_path_buf() },
            Err(e) => {
                log::error!("templates {}: {e}", path.display());
                Self { conn: None, path: path.to_path_buf() }
            }
        }
    }

    /// Web build: copy the database back to storage after a change (a no-op on the desktop).
    fn persist(&self) {
        #[cfg(target_arch = "wasm32")]
        match memvfs().export_db(&self.path.display().to_string()) {
            Ok(bytes) => {
                if let Err(e) = crate::storage::write(&self.path, &bytes) {
                    log::error!("templates: {e}");
                }
            }
            Err(e) => log::error!("templates: {e}"),
        }
    }

    pub fn list(&self) -> Vec<TemplateRow> {
        let Some(c) = &self.conn else { return Vec::new() };
        let q = || -> rusqlite::Result<Vec<TemplateRow>> {
            let mut st = c.prepare(
                "SELECT id, title, coalesce(description,''), coalesce(category,''), coalesce(author,''), \
                 coalesce(license,''), coalesce(created_at,''), coalesce(thumb_key,''), json_key, json_local, thumb_local, \
                 coalesce(tags,'[]') FROM templates ORDER BY json_local IS NULL, lower(title), id",
            )?;
            let rows = st
                .query_map([], |r| {
                    Ok(TemplateRow {
                        meta: TemplateMeta {
                            id: r.get(0)?,
                            title: r.get(1)?,
                            description: r.get(2)?,
                            category: r.get(3)?,
                            tags: crate::catalog::tags_from_json(&r.get::<_, String>(11)?),
                            author: r.get(4)?,
                            license: r.get(5)?,
                            created_at: r.get(6)?,
                            thumb_key: r.get(7)?,
                        },
                        json_key: r.get(8)?,
                        json_local: r.get(9)?,
                        thumb_local: r.get(10)?,
                    })
                })?
                .collect();
            rows
        };
        q().unwrap_or_else(|e| {
            log::warn!("list templates: {e}");
            Vec::new()
        })
    }

    pub fn insert(&self, row: &TemplateRow) -> Result<(), String> {
        let Some(c) = &self.conn else { return Err("template library unavailable".into()) };
        let m = &row.meta;
        c.execute(
            "INSERT OR REPLACE INTO templates (id, title, description, category, author, license, created_at, json_key, thumb_key, \
             json_local, thumb_local, tags) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                m.id,
                m.title,
                m.description,
                m.category,
                m.author,
                m.license,
                m.created_at,
                row.json_key,
                m.thumb_key,
                row.json_local,
                row.thumb_local,
                serde_json::to_string(&m.tags).map_err(|e| e.to_string())?
            ],
        )
        .map_err(|e| e.to_string())?;
        self.persist();
        Ok(())
    }

    pub fn remove_local(&self, id: &str) -> Result<(), String> {
        let Some(c) = &self.conn else { return Ok(()) };
        c.execute("DELETE FROM templates WHERE id=?1 AND json_local IS NOT NULL", params![id]).map_err(|e| e.to_string())?;
        self.persist();
        Ok(())
    }

    fn version(&self) -> Option<String> {
        self.conn.as_ref()?.query_row("SELECT v FROM meta WHERE k='published_version'", [], |r| r.get(0)).ok()
    }

    /// Replace the published templates with those in `downloaded` (own templates are kept, and
    /// win over a published copy with the same id). Removed-on-publish templates disappear.
    pub fn merge_published(&self, downloaded: &Path, version: &str) -> Result<(), String> {
        let Some(c) = &self.conn else { return Err("template library unavailable".into()) };
        c.execute("ATTACH DATABASE ?1 AS pub", params![downloaded.display().to_string()]).map_err(|e| e.to_string())?;
        let tags = if has_tags(c, "pub.templates") { "coalesce(tags,'[]')" } else { "'[]'" };
        let r = c.execute_batch(&format!(
            "BEGIN; DELETE FROM templates WHERE json_local IS NULL; \
             INSERT OR IGNORE INTO templates (id, title, description, category, author, license, created_at, json_key, thumb_key, tags) \
             SELECT id, title, description, category, author, license, created_at, json_key, thumb_key, {tags} FROM pub.templates; COMMIT;"
        ));
        if r.is_err() {
            let _ = c.execute_batch("ROLLBACK;");
        }
        let _ = c.execute_batch("DETACH DATABASE pub;");
        r.map_err(|e| e.to_string())?;
        c.execute("INSERT OR REPLACE INTO meta VALUES ('published_version', ?1)", params![version]).map_err(|e| e.to_string())?;
        self.persist();
        Ok(())
    }
}

/// Background download of the published template list.
pub struct TemplateSync {
    rx: Receiver<Result<Option<(PathBuf, String)>, String>>,
}

impl TemplateSync {
    pub fn start(base_url: &str, store: &TemplateStore, tmp_dir: PathBuf) -> Self {
        let (tx, rx) = channel();
        let base = base_url.trim_end_matches('/').to_string();
        let have = store.version();
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::Builder::new()
            .name("template-sync".into())
            .spawn(move || {
                let _ = tx.send(download_published(&base, have.as_deref(), &tmp_dir));
            })
            .expect("spawn template sync");
        #[cfg(target_arch = "wasm32")]
        download_published_web(base, have, tmp_dir, tx);
        Self { rx }
    }

    /// `Some(Ok(Some((file, version))))` when a new list was downloaded; merge it, then delete it.
    pub fn poll(&mut self) -> Option<Result<Option<(PathBuf, String)>, String>> {
        self.rx.try_recv().ok()
    }
}

/// Web build: the same download with ehttp; the list goes into SQLite's in-memory files, where
/// `merge_published` attaches it.
#[cfg(target_arch = "wasm32")]
fn download_published_web(base: String, have: Option<String>, dir: PathBuf, tx: std::sync::mpsc::Sender<Result<Option<(PathBuf, String)>, String>>) {
    if base.is_empty() {
        let _ = tx.send(Err("No asset URL configured".into()));
        return;
    }
    ehttp::fetch(ehttp::Request::get(format!("{base}/catalog/templates_version.txt")), move |res| {
        let version = match res {
            Ok(r) if r.status == 404 => return drop(tx.send(Ok(None))), // nothing published yet
            Ok(r) if r.ok => r.text().unwrap_or_default().trim().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect::<String>(),
            Ok(r) => return drop(tx.send(Err(format!("templates version: HTTP {}", r.status)))),
            Err(e) => return drop(tx.send(Err(format!("templates version: {e}")))),
        };
        if version.is_empty() || have.as_deref() == Some(version.as_str()) {
            let _ = tx.send(Ok(None));
            return;
        }
        ehttp::fetch(ehttp::Request::get(format!("{base}/catalog/templates.sqlite.gz")), move |res| {
            let r = res.and_then(|r| if r.ok { Ok(r.bytes) } else { Err(format!("HTTP {}", r.status)) }).and_then(|gz| {
                let mut db = Vec::new();
                std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(&gz[..]), &mut db).map_err(|e| e.to_string())?;
                let dest = dir.join(format!("templates-{version}.sqlite"));
                let name = dest.display().to_string();
                memvfs().delete_db(&name);
                memvfs().import_db(&name, &db).map_err(|e| e.to_string())?;
                Ok(Some((dest, version)))
            });
            let _ = tx.send(r.map_err(|e| format!("templates download: {e}")));
        });
    });
}

/// Delete a list downloaded by [`TemplateSync`] once it has been merged.
pub fn remove_download(file: &Path) {
    #[cfg(not(target_arch = "wasm32"))]
    let _ = std::fs::remove_file(file);
    #[cfg(target_arch = "wasm32")]
    memvfs().delete_db(&file.display().to_string());
}

#[cfg(not(target_arch = "wasm32"))]
fn download_published(base: &str, have: Option<&str>, dir: &Path) -> Result<Option<(PathBuf, String)>, String> {
    if base.is_empty() {
        return Err("No asset URL configured".into());
    }
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(60))).build().into();
    let version = match agent.get(&format!("{base}/catalog/templates_version.txt")).call() {
        Ok(mut r) => r.body_mut().read_to_string().map_err(|e| e.to_string())?,
        // Nothing published yet.
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(e) => return Err(format!("templates version: {e}")),
    };
    let version: String = version.trim().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    if version.is_empty() || have == Some(version.as_str()) {
        return Ok(None);
    }
    let mut res = agent.get(&format!("{base}/catalog/templates.sqlite.gz")).call().map_err(|e| format!("templates download: {e}"))?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let dest = dir.join(format!("templates-{version}.sqlite"));
    let mut gz = flate2::read::GzDecoder::new(res.body_mut().with_config().limit(64 * 1024 * 1024).reader());
    let mut f = std::fs::File::create(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    std::io::copy(&mut gz, &mut f).map_err(|e| format!("templates download: {e}"))?;
    Ok(Some((dest, version)))
}

/// Everything needed to save (and optionally upload) a template, gathered on the UI thread.
pub struct SaveJob {
    pub meta: TemplateMeta,
    pub file: TemplateFile,
    /// Where the template's local copy goes: `<dir>/template.json`, `<dir>/thumb.png`.
    pub dir: PathBuf,
    pub thumb: Result<image::RgbaImage, String>,
    /// Also submit it for the shared library: the Worker URL and the public metadata.
    pub submit: Option<(String, SubmitMeta)>,
}

/// The local row, and the submission's outcome if it was submitted. A successful submission
/// gives the row the server's id, so the published copy later merges with it.
pub type SaveResult = Result<(TemplateRow, Option<Result<(), String>>), String>;

pub fn validate(file: &TemplateFile) -> Result<(), String> {
    if file.meta.title.trim().is_empty() {
        return Err("Give the template a name first".into());
    }
    if file.slide.items.is_empty() {
        return Err("The slide is empty".into());
    }
    if file.slide.items.len() > submit::MAX_TEMPLATE_ITEMS {
        return Err(format!("Templates can have at most {} items", submit::MAX_TEMPLATE_ITEMS));
    }
    let images = file.slide.items.iter().filter(|i| matches!(i.kind, ItemKind::Image { .. })).count();
    if images > submit::MAX_TEMPLATE_IMAGES {
        return Err(format!("Templates can have at most {} images", submit::MAX_TEMPLATE_IMAGES));
    }
    Ok(())
}

/// Read and re-encode a local image as a PNG within the size limits.
pub fn png_bytes(path: &Path) -> Result<(Vec<u8>, u32, u32), String> {
    let bytes = crate::storage::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let img = crate::assets::load_image(&bytes, submit::MAX_IMAGE_SIDE).map_err(|e| format!("{}: {e}", path.display()))?;
    encode_png(img)
}

/// Downscale to `MAX_IMAGE_SIDE` if needed and encode; fails above `MAX_PNG_BYTES`.
pub fn encode_png(mut img: image::RgbaImage) -> Result<(Vec<u8>, u32, u32), String> {
    let (w, h) = img.dimensions();
    if w.max(h) > submit::MAX_IMAGE_SIDE {
        let s = submit::MAX_IMAGE_SIDE as f32 / w.max(h) as f32;
        img = image::imageops::resize(&img, ((w as f32 * s) as u32).max(1), ((h as f32 * s) as u32).max(1), image::imageops::FilterType::Lanczos3);
    }
    let (w, h) = img.dimensions();
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    let out = out.into_inner();
    if out.len() > submit::MAX_PNG_BYTES {
        return Err(format!("Image is {:.1} MB as PNG; the limit is {} MB", out.len() as f64 / 1e6, submit::MAX_PNG_BYTES / (1024 * 1024)));
    }
    Ok((out, w, h))
}

/// Runs on a worker thread: writes the local copy, then submits it if asked to.
pub fn save(job: SaveJob) -> SaveResult {
    let SaveJob { meta, file, dir, thumb, submit } = job;
    validate(&file)?;
    let thumb = thumb.map_err(|e| format!("thumbnail: {e}"))?;
    let (thumb_png, _, _) = encode_png(thumb)?;
    let thumb_path = dir.join("thumb.png");
    crate::storage::write(&thumb_path, &thumb_png).map_err(|e| e.to_string())?;
    let local_json = serde_json::to_vec_pretty(&file).map_err(|e| e.to_string())?;
    let json_path = dir.join("template.json");
    crate::storage::write(&json_path, &local_json).map_err(|e| e.to_string())?;
    let mut row = TemplateRow {
        meta: meta.clone(),
        json_key: format!("templates/{}/template.json", meta.id),
        json_local: Some(json_path.display().to_string()),
        thumb_local: Some(thumb_path.display().to_string()),
    };
    let Some((url, smeta)) = submit else { return Ok((row, None)) };
    let submitted = submit_template(&url, &smeta, file, thumb_png).map(|id| {
        row.json_key = format!("templates/{id}/template.json");
        row.meta.thumb_key = format!("templates/{id}/thumb.png");
        row.meta.id = id;
    });
    Ok((row, Some(submitted)))
}

/// Local images go along as `img-<n>.png`; the Worker re-keys them under `templates/<id>/`.
fn submit_template(url: &str, meta: &SubmitMeta, mut file: TemplateFile, thumb_png: Vec<u8>) -> Result<String, String> {
    let mut images: Vec<(String, Vec<u8>)> = Vec::new();
    for it in &mut file.slide.items {
        if let ItemKind::Image { source, .. } = &mut it.kind {
            if let ImageSource::File { path, .. } = source {
                let (png, _, _) = png_bytes(Path::new(path.as_str()))?;
                let name = format!("img-{}.png", images.len() + 1);
                let credit = crate::credits::template_credit(&it.name, &meta.author, &meta.license, &meta.title);
                *source = ImageSource::Asset { id: name.clone(), key: name.clone(), svg: None, title: it.name.clone(), source: "template".into(), credit, license: meta.license.clone() };
                images.push((name, png));
            }
        }
    }
    let json = serde_json::to_vec(&file).map_err(|e| e.to_string())?;
    if json.len() > submit::MAX_TEMPLATE_JSON_BYTES {
        return Err(format!("Template is too large ({} KB)", json.len() / 1024));
    }
    let mut parts = vec![
        Part::Text("kind", "template".into()),
        Part::Text("meta", serde_json::to_string(meta).map_err(|e| e.to_string())?),
        Part::File("template", json, "application/json"),
        Part::File("thumb", thumb_png, "image/png"),
    ];
    for (name, png) in &images {
        parts.push(Part::File(name, png.clone(), "image/png"));
    }
    let total: usize = parts.iter().map(|p| match p { Part::Text(_, t) => t.len(), Part::File(_, b, _) => b.len() }).sum();
    if total > submit::MAX_REQUEST_BYTES {
        return Err(format!(
            "it is {} with its images; templates can be up to {}. Use smaller images on the slide",
            crate::offline::human_bytes(total as u64),
            crate::offline::human_bytes(submit::MAX_REQUEST_BYTES as u64)
        ));
    }
    submit::submit(url, &parts)
}

/// Fetch and parse a template (from its local copy when there is one).
pub fn load(row: &TemplateRow, base_url: &str) -> Result<TemplateFile, String> {
    let req = match row.json_local.as_deref().map(PathBuf::from).filter(|p| crate::storage::exists(p)) {
        Some(p) => crate::assets::Request::Path(p),
        None if !base_url.is_empty() => crate::assets::Request::Url(format!("{}/{}", base_url.trim_end_matches('/'), row.json_key)),
        None => return Err("No asset URL configured.".into()),
    };
    parse(&crate::assets::fetch(&req)?)
}

/// Parse a template's JSON.
pub fn parse(bytes: &[u8]) -> Result<TemplateFile, String> {
    if bytes.len() > submit::MAX_TEMPLATE_JSON_BYTES {
        return Err("template is too large".into());
    }
    let text = std::str::from_utf8(bytes).map_err(|e| format!("template: {e}"))?;
    let mut t: TemplateFile = crate::doc::from_json(text).map_err(|e| format!("template: {e}"))?;
    t.slide.upgrade();
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_save_round_trips() {
        use crate::doc::{Item, ShapeKind};
        let dir = std::env::temp_dir().join(format!("cc-template-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let meta = TemplateMeta { id: "t".into(), title: "T".into(), thumb_key: "templates/t/thumb.png".into(), ..Default::default() };
        let kind = ItemKind::Shape { shape: ShapeKind::Ellipse, fill: [0; 4], stroke: [0; 4], stroke_width: 1.0 };
        let slide = Slide { name: "T".into(), items: vec![Item::new(1, "s", kind, egui::Pos2::ZERO, egui::Vec2::splat(10.0))], comments: Vec::new(), background: None };
        let file = TemplateFile { meta: meta.clone(), width: 100.0, height: 50.0, background: [255; 4], slide };
        let job = SaveJob { meta, file, dir: dir.clone(), thumb: Ok(image::RgbaImage::new(4, 2)), submit: None };
        let (row, submitted) = save(job).unwrap();
        assert!(submitted.is_none());
        assert!(Path::new(row.thumb_local.as_deref().unwrap()).exists());
        let back = load(&row, "").unwrap();
        assert_eq!((back.width, back.slide.items.len(), back.meta.title.as_str()), (100.0, 1, "T"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Against a running submissions Worker: `cd worker && npm run dev`, then
    /// `BIOCARTOON_TEST_SUBMIT_URL=http://localhost:8787 cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn submits_to_worker() {
        use crate::doc::Item;
        let url = std::env::var("BIOCARTOON_TEST_SUBMIT_URL").expect("BIOCARTOON_TEST_SUBMIT_URL");
        let dir = std::env::temp_dir().join(format!("cc-template-submit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pic = dir.join("pic.png");
        image::RgbaImage::from_pixel(40, 30, image::Rgba([200, 60, 60, 255])).save(&pic).unwrap();
        let meta = SubmitMeta { title: "Rust test template".into(), description: String::new(), category: "test".into(), tags: vec!["test".into()], author: "tester".into(), license: "CC0-1.0".into(), email: String::new() };
        let image = ItemKind::Image { source: ImageSource::file(pic.display().to_string()), crop: [0.0, 0.0, 1.0, 1.0], overlay: [255, 255, 255, 0], layers: Vec::new() };
        let text = ItemKind::text("Step 1", 20.0, [0, 0, 0, 255], crate::doc::TextAlign::Left, true);
        let slide = Slide { name: "x".into(), items: vec![Item::new(1, "pic", image, egui::Pos2::ZERO, egui::Vec2::splat(50.0)), Item::new(2, "t", text, egui::Pos2::ZERO, egui::Vec2::splat(50.0))], comments: Vec::new(), background: None };
        let tmeta = TemplateMeta { id: "local-id".into(), title: meta.title.clone(), thumb_key: "templates/local-id/thumb.png".into(), ..Default::default() };
        let file = TemplateFile { meta: tmeta.clone(), width: 960.0, height: 672.0, background: [255; 4], slide };
        let job = SaveJob { meta: tmeta, file, dir: dir.join("t"), thumb: Ok(image::RgbaImage::new(48, 32)), submit: Some((url.clone(), meta.clone())) };
        let (row, submitted) = save(job).unwrap();
        submitted.unwrap().unwrap();
        assert!(row.meta.id.starts_with("rust-test-template-"), "{}", row.meta.id);
        assert_eq!(row.json_key, format!("templates/{}/template.json", row.meta.id));
        // An asset, and a profane one that must be refused with the server's message.
        let png = std::fs::read(&pic).unwrap();
        let asset = |title: &str| {
            let m = serde_json::to_string(&SubmitMeta { title: title.into(), ..meta.clone() }).unwrap();
            submit::submit(&url, &[Part::Text("kind", "asset".into()), Part::Text("meta", m), Part::File("image", png.clone(), "image/png")])
        };
        assert!(asset("Rust test cell").unwrap().starts_with("rust-test-cell-"));
        assert!(asset("shit cell").unwrap_err().contains("language"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn own_templates_survive_publish_merge() {
        let dir = std::env::temp_dir().join(format!("cc-templates-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = TemplateStore::open(&dir.join("templates.sqlite"));
        let row = |id: &str, local: bool| TemplateRow {
            meta: TemplateMeta { id: id.into(), title: id.into(), tags: vec![format!("{id} tag")], ..Default::default() },
            json_key: format!("templates/{id}/template.json"),
            json_local: local.then(|| "/x/template.json".into()),
            thumb_local: None,
        };
        store.insert(&row("mine", true)).unwrap();
        store.insert(&row("old-published", false)).unwrap();
        let published = TemplateStore::open(&dir.join("pub.sqlite"));
        published.insert(&row("mine", false)).unwrap();
        published.insert(&row("new-published", false)).unwrap();
        drop(published);
        store.merge_published(&dir.join("pub.sqlite"), "v1").unwrap();
        let got: Vec<(String, bool)> = store.list().into_iter().map(|r| (r.meta.id, r.json_local.is_some())).collect();
        assert_eq!(got, vec![("mine".into(), true), ("new-published".into(), false)]);
        assert_eq!(store.list()[1].meta.tags, ["new-published tag"]);
        assert_eq!(store.version().as_deref(), Some("v1"));
        // A published list from before templates had tags.
        let old = Connection::open(dir.join("old.sqlite")).unwrap();
        old.execute_batch(
            "CREATE TABLE templates (id TEXT PRIMARY KEY, title TEXT NOT NULL, description TEXT, category TEXT, author TEXT, \
             license TEXT, created_at TEXT, json_key TEXT NOT NULL, thumb_key TEXT, json_local TEXT, thumb_local TEXT); \
             INSERT INTO templates (id, title, json_key) VALUES ('old', 'Old', 'templates/old/template.json');",
        )
        .unwrap();
        drop(old);
        store.merge_published(&dir.join("old.sqlite"), "v2").unwrap();
        assert!(store.list().iter().any(|r| r.meta.id == "old" && r.meta.tags.is_empty()));
        // And a local list from before: opening it adds the column.
        drop(store);
        Connection::open(dir.join("templates.sqlite")).unwrap().execute_batch("ALTER TABLE templates DROP COLUMN tags;").unwrap();
        assert_eq!(TemplateStore::open(&dir.join("templates.sqlite")).list().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
