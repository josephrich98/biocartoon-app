//! Project store: every project is a folder under `<app data>/projects/<id>/` holding
//!
//! - `project.json`: name and created / modified / opened times (read by the home screen),
//! - `document.biocartoon`: the current document (same JSON as a saved file),
//! - `thumb.png`: a preview of the first slide,
//! - `versions/`: gzipped snapshots `<millis>.biocartoon.gz` plus `index.json` describing them.
//!
//! Version history is snapshot-based rather than git: documents are small, so a compressed copy
//! per version is cheap, needs no external tools and restores instantly.

use crate::doc::{Document, ImageSource, ItemKind};
use crate::storage;
use base64::Engine;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Autosaves within this many milliseconds of the last snapshot don't create a new version.
const SNAPSHOT_INTERVAL_MS: i64 = 10 * 60 * 1000;

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProjectMeta {
    #[serde(skip)]
    pub id: String,
    pub name: String,
    /// Unix milliseconds.
    pub created: i64,
    pub modified: i64,
    pub opened: i64,
    pub slides: usize,
    /// When the project was last downloaded as a `.biocartoon` file or in a backup (0: never).
    pub backed_up: i64,
}

impl Default for ProjectMeta {
    fn default() -> Self {
        let t = now_ms();
        Self { id: String::new(), name: "Untitled".into(), created: t, modified: t, opened: t, slides: 1, backed_up: 0 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Version {
    /// Unix milliseconds; also the snapshot's file name.
    pub time: i64,
    pub note: String,
    pub slides: usize,
    pub items: usize,
    pub hash: u64,
}

/// When a save should also record a version.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Snapshot {
    /// Only if the last version is older than [`SNAPSHOT_INTERVAL_MS`] (background autosave).
    IfDue,
    /// Whenever the content changed since the last version (explicit save, closing the project).
    IfChanged,
}

pub struct Store {
    pub root: PathBuf,
}

fn err(p: &Path, e: impl std::fmt::Display) -> String {
    format!("{}: {e}", p.display())
}

/// FNV-1a: stable across builds, unlike `DefaultHasher`.
fn hash(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

/// Atomic on the desktop (temp file + rename), so a crash never leaves a half-written document.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    storage::write(path, bytes).map_err(|e| err(path, e))
}

/// A random version-4 UUID. Locally an id is only a folder name, but it has to stay unique once
/// projects from several devices are uploaded into one account.
fn new_id() -> String {
    let mut b = random_bytes();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn random_bytes() -> [u8; 16] {
    let mut b = [0u8; 16];
    let crypto = web_sys::window().and_then(|w| w.crypto().ok());
    if crypto.and_then(|c| c.get_random_values_with_u8_array(&mut b).ok()).is_none() {
        b.iter_mut().for_each(|x| *x = (js_sys::Math::random() * 256.0) as u8);
    }
    b
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn random_bytes() -> [u8; 16] {
    use std::hash::{BuildHasher, Hasher};
    // RandomState is keyed from the OS; two hashers give 128 bits.
    let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0) as u64;
    let word = |salt: u64| {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(nanos ^ salt);
        h.finish().to_le_bytes()
    };
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&word(0x9e37_79b9_7f4a_7c15));
    b[8..].copy_from_slice(&word(0xbf58_476d_1ce4_e5b9));
    b
}

/// A `.biocartoon` file: the document plus the bytes of every local image it uses (uploads and
/// AI-generated pictures are otherwise only paths on the machine that made them), so the file
/// opens complete on another computer or in the browser. Older files are a bare document.
#[derive(Serialize, Deserialize)]
struct PortableFile {
    #[serde(flatten)]
    doc: Document,
    /// Image name (what the item's `File` path is replaced with) -> base64 bytes.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    images: BTreeMap<String, String>,
}

/// Serialise `doc` as a self-contained `.biocartoon` file. Images that can't be read are left as
/// paths, as before.
pub fn to_file(doc: &Document) -> Result<Vec<u8>, String> {
    let b64 = base64::engine::general_purpose::STANDARD;
    let mut doc = doc.clone();
    doc.sanitize();
    let mut names: BTreeMap<String, String> = BTreeMap::new(); // path -> name
    let mut images = BTreeMap::new();
    for it in doc.slides.iter_mut().flat_map(|s| s.items.iter_mut()) {
        let ItemKind::Image { source: ImageSource::File { path, .. }, .. } = &mut it.kind else { continue };
        if let Some(name) = names.get(path.as_str()) {
            *path = name.clone();
            continue;
        }
        let Ok(bytes) = storage::read(Path::new(path.as_str())) else {
            log::warn!("export: image {path} not found; keeping its path");
            continue;
        };
        let file_name = Path::new(path.as_str()).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let name = format!("img-{}-{file_name}", images.len() + 1);
        images.insert(name.clone(), b64.encode(bytes));
        names.insert(std::mem::replace(path, name.clone()), name);
    }
    serde_json::to_vec_pretty(&PortableFile { doc, images }).map_err(|e| e.to_string())
}

/// Read a `.biocartoon` file, writing its bundled images under `uploads_dir` and pointing the
/// document at them.
pub fn from_file(bytes: &[u8], uploads_dir: &Path) -> Result<Document, String> {
    let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
    let PortableFile { mut doc, images } = crate::doc::from_json(text).map_err(|e| e.to_string())?;
    doc.upgrade();
    if images.is_empty() {
        return Ok(doc);
    }
    let dir = uploads_dir.join(format!("import-{}", new_id()));
    let mut paths = BTreeMap::new();
    for (name, data) in &images {
        // Names come from the file, so keep only a plain file name.
        let safe = Path::new(name).file_name().ok_or_else(|| format!("bad image name {name:?}"))?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| format!("image {name}: {e}"))?;
        let p = dir.join(safe);
        write_atomic(&p, &bytes)?;
        paths.insert(name.as_str(), p.display().to_string());
    }
    for it in doc.slides.iter_mut().flat_map(|s| s.items.iter_mut()) {
        if let ItemKind::Image { source: ImageSource::File { path, .. }, .. } = &mut it.kind {
            if let Some(p) = paths.get(path.as_str()) {
                *path = p.clone();
            }
        }
    }
    Ok(doc)
}

/// A name for a new document that no project in `taken` has yet: "Untitled", then "Untitled 2",
/// "Untitled 3", ... so new projects can be told apart on the home screen.
pub fn untitled_name(taken: &[String]) -> String {
    let base = Document::default().title;
    std::iter::once(base.clone())
        .chain((2..).map(|n| format!("{base} {n}")))
        .find(|name| !taken.contains(name))
        .expect("an unused name")
}

fn to_json(doc: &Document) -> Result<String, String> {
    let mut doc = doc.clone();
    doc.sanitize();
    serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())
}

impl Store {
    pub fn new(root: PathBuf) -> Self {
        let _ = storage::create_dir_all(&root);
        Self { root }
    }

    pub fn dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    pub fn thumb_path(&self, id: &str) -> PathBuf {
        self.dir(id).join("thumb.png")
    }

    fn doc_path(&self, id: &str) -> PathBuf {
        self.dir(id).join("document.biocartoon")
    }

    fn versions_dir(&self, id: &str) -> PathBuf {
        self.dir(id).join("versions")
    }

    fn version_path(&self, id: &str, time: i64) -> PathBuf {
        self.versions_dir(id).join(format!("{time}.biocartoon.gz"))
    }

    /// All projects, in no particular order. Folders without a readable document are skipped.
    pub fn list(&self) -> Vec<ProjectMeta> {
        storage::subdirs(&self.root).iter().filter_map(|id| self.meta(id).ok()).collect()
    }

    pub fn meta(&self, id: &str) -> Result<ProjectMeta, String> {
        let p = self.dir(id).join("project.json");
        let mut m: ProjectMeta = match storage::read_to_string(&p) {
            Ok(s) => serde_json::from_str(&s).map_err(|e| err(&p, e))?,
            // Rebuild a lost project.json from the document itself.
            Err(_) => {
                let doc = self.load(id)?;
                let t = storage::modified_ms(&self.doc_path(id)).unwrap_or_else(now_ms);
                ProjectMeta { id: id.into(), name: doc.title.clone(), created: t, modified: t, opened: t, slides: doc.slides.len(), backed_up: 0 }
            }
        };
        m.id = id.into();
        Ok(m)
    }

    fn write_meta(&self, m: &ProjectMeta) -> Result<(), String> {
        let s = serde_json::to_string_pretty(m).map_err(|e| e.to_string())?;
        write_atomic(&self.dir(&m.id).join("project.json"), s.as_bytes())
    }

    pub fn load(&self, id: &str) -> Result<Document, String> {
        let p = self.doc_path(id);
        let s = storage::read_to_string(&p).map_err(|e| err(&p, e))?;
        let mut doc: Document = crate::doc::from_json(&s).map_err(|e| err(&p, e))?;
        doc.upgrade();
        Ok(doc)
    }

    /// Create a project holding `doc` (with a first version).
    pub fn create(&self, doc: &Document) -> Result<ProjectMeta, String> {
        let id = new_id();
        let mut m = ProjectMeta { id: id.clone(), name: doc.title.clone(), slides: doc.slides.len(), ..Default::default() };
        self.write_meta(&m)?;
        self.save(&id, doc, Snapshot::IfChanged, "Created")?;
        m.modified = self.meta(&id)?.modified;
        Ok(m)
    }

    /// Write the document, update the metadata and record a version according to `snapshot`.
    pub fn save(&self, id: &str, doc: &Document, snapshot: Snapshot, note: &str) -> Result<(), String> {
        let json = to_json(doc)?;
        write_atomic(&self.doc_path(id), json.as_bytes())?;
        let mut m = self.meta(id)?;
        m.name = doc.title.clone();
        m.modified = now_ms();
        m.slides = doc.slides.len();
        self.write_meta(&m)?;
        self.snapshot(id, doc, &json, snapshot, note)
    }

    fn snapshot(&self, id: &str, doc: &Document, json: &str, when: Snapshot, note: &str) -> Result<(), String> {
        let mut versions = self.versions(id);
        let h = hash(json);
        let now = now_ms();
        if let Some(last) = versions.first() {
            if last.hash == h || (when == Snapshot::IfDue && now - last.time < SNAPSHOT_INTERVAL_MS) {
                return Ok(());
            }
        }
        let dir = self.versions_dir(id);
        let time = versions.first().map_or(now, |v| now.max(v.time + 1));
        let mut gz = GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(json.as_bytes()).and_then(|_| gz.flush()).map_err(|e| e.to_string())?;
        let bytes = gz.finish().map_err(|e| e.to_string())?;
        write_atomic(&self.version_path(id, time), &bytes)?;
        let items = doc.slides.iter().map(|s| s.items.len()).sum();
        versions.insert(0, Version { time, note: note.into(), slides: doc.slides.len(), items, hash: h });
        let s = serde_json::to_string_pretty(&versions).map_err(|e| e.to_string())?;
        write_atomic(&dir.join("index.json"), s.as_bytes())
    }

    /// Versions, newest first.
    pub fn versions(&self, id: &str) -> Vec<Version> {
        let p = self.versions_dir(id).join("index.json");
        let mut v: Vec<Version> = storage::read_to_string(&p).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        v.retain(|x| storage::exists(&self.version_path(id, x.time)));
        v.sort_by(|a, b| b.time.cmp(&a.time));
        v
    }

    pub fn load_version(&self, id: &str, time: i64) -> Result<Document, String> {
        let p = self.version_path(id, time);
        let bytes = storage::read(&p).map_err(|e| err(&p, e))?;
        let mut s = String::new();
        GzDecoder::new(bytes.as_slice()).read_to_string(&mut s).map_err(|e| err(&p, e))?;
        let mut doc: Document = crate::doc::from_json(&s).map_err(|e| err(&p, e))?;
        doc.upgrade();
        Ok(doc)
    }

    /// Make an old version current. The current document is kept as a version first, so a
    /// restore can itself be undone from the history. Returns the restored document.
    pub fn restore(&self, id: &str, time: i64) -> Result<Document, String> {
        let mut doc = self.load_version(id, time)?;
        let current = self.load(id)?;
        self.save(id, &current, Snapshot::IfChanged, "Before restoring")?;
        doc.title = current.title; // a restore brings back content, not an old name
        self.save(id, &doc, Snapshot::IfChanged, &format!("Restored version from {}", format_time(time)))?;
        Ok(doc)
    }

    pub fn touch_opened(&self, id: &str) {
        if let Ok(mut m) = self.meta(id) {
            m.opened = now_ms();
            let _ = self.write_meta(&m);
        }
    }

    /// Write `doc` as project `id` (made if missing) dated `modified`: how an online copy comes
    /// back. Replacing a project keeps its old content in the version history.
    pub fn put_as(&self, id: &str, doc: &Document, modified: i64) -> Result<(), String> {
        if id.is_empty() || id.contains(['/', '\\']) || id.starts_with('.') {
            return Err(format!("bad project id {id:?}"));
        }
        if self.meta(id).is_err() {
            self.write_meta(&ProjectMeta { id: id.into(), name: doc.title.clone(), created: modified, modified, opened: modified, slides: doc.slides.len(), backed_up: 0 })?;
        }
        self.save(id, doc, Snapshot::IfChanged, "From your account")?;
        let mut m = self.meta(id)?;
        m.modified = modified;
        self.write_meta(&m)
    }

    /// Record that the project's current content was just downloaded.
    pub fn mark_backed_up(&self, id: &str) {
        if let Ok(mut m) = self.meta(id) {
            m.backed_up = now_ms();
            let _ = self.write_meta(&m);
        }
    }

    pub fn rename(&self, id: &str, name: &str) -> Result<(), String> {
        let mut doc = self.load(id)?;
        doc.title = name.to_string();
        let modified = self.meta(id)?.modified;
        self.save(id, &doc, Snapshot::IfChanged, "Renamed")?;
        // Renaming isn't an edit: keep "last modified" ordering stable.
        let mut m = self.meta(id)?;
        m.modified = modified;
        self.write_meta(&m)
    }

    /// Copy a project's current document (and thumbnail) into a new project named "<name> copy".
    pub fn duplicate(&self, id: &str) -> Result<ProjectMeta, String> {
        let mut doc = self.load(id)?;
        doc.title = format!("{} copy", doc.title);
        let m = self.create(&doc)?;
        let _ = storage::copy(&self.thumb_path(id), &self.thumb_path(&m.id));
        Ok(m)
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        if id.is_empty() || id.contains(['/', '\\']) || id.starts_with('.') {
            return Err(format!("refusing to delete project {id:?}"));
        }
        let d = self.dir(id);
        storage::remove_dir_all(&d).map_err(|e| err(&d, e))
    }
}

/// The files in a .zip archive as (name, bytes), skipping folders. Reads the central directory, so
/// it handles archives re-zipped by the system's own tools (deflated, with data descriptors) as
/// well as the stored ones the backup download writes. No ZIP64 or encryption.
pub fn read_zip(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let u16_at = |i: usize| bytes.get(i..i + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize).ok_or("truncated zip");
    let u32_at = |i: usize| bytes.get(i..i + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize).ok_or("truncated zip");
    // The end record sits in the last 22 bytes plus a comment of up to 64 KB.
    let floor = bytes.len().saturating_sub(22 + 0xffff);
    let end = (floor..=bytes.len().saturating_sub(22)).rev().find(|&i| bytes[i..i + 4] == [0x50, 0x4b, 0x05, 0x06]).ok_or("not a zip file")?;
    let (count, mut at) = (u16_at(end + 10)?, u32_at(end + 16)?);
    let mut out = Vec::new();
    for _ in 0..count {
        if u32_at(at)? != 0x02014b50 {
            return Err("damaged zip directory".into());
        }
        let (flags, method, size) = (u16_at(at + 8)?, u16_at(at + 10)?, u32_at(at + 20)?);
        let (name_len, extra_len, comment_len, local) = (u16_at(at + 28)?, u16_at(at + 30)?, u16_at(at + 32)?, u32_at(at + 42)?);
        let name = String::from_utf8_lossy(bytes.get(at + 46..at + 46 + name_len).ok_or("truncated zip")?).to_string();
        at += 46 + name_len + extra_len + comment_len;
        if name.ends_with('/') {
            continue;
        }
        if flags & 1 != 0 {
            return Err(format!("{name} is encrypted"));
        }
        let start = local + 30 + u16_at(local + 26)? + u16_at(local + 28)?;
        let data = bytes.get(start..start + size).ok_or("truncated zip")?;
        let data = match method {
            0 => data.to_vec(),
            8 => {
                let mut v = Vec::new();
                flate2::read::DeflateDecoder::new(data).read_to_end(&mut v).map_err(|e| format!("{name}: {e}"))?;
                v
            }
            m => return Err(format!("{name}: unsupported zip compression ({m})")),
        };
        out.push((name, data));
    }
    Ok(out)
}

/// "Oct 14th, 2026" in local time.
pub fn format_date(ms: i64) -> String {
    use chrono::{Datelike, TimeZone};
    let Some(t) = chrono::Local.timestamp_millis_opt(ms).single() else { return String::new() };
    let d = t.day();
    let suffix = match (d % 10, d % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{} {d}{suffix}, {}", t.format("%b"), t.year())
}

/// "Oct 14th, 2026, 3:04 PM" in local time.
pub fn format_time(ms: i64) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_millis_opt(ms).single() {
        Some(t) => format!("{}, {}", format_date(ms), t.format("%-I:%M %p")),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untitled_names() {
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(untitled_name(&[]), "Untitled");
        assert_eq!(untitled_name(&names(&["Untitled", "Cells"])), "Untitled 2");
        assert_eq!(untitled_name(&names(&["Untitled", "Untitled 3"])), "Untitled 2");
        assert_eq!(untitled_name(&names(&["Untitled", "Untitled 2"])), "Untitled 3");
    }

    #[test]
    fn save_version_restore() {
        let root = std::env::temp_dir().join(format!("biocartoon-projects-test-{}", now_ms()));
        let store = Store::new(root.clone());
        let mut doc = Document::default();
        doc.title = "Cells".into();
        let m = store.create(&doc).unwrap();
        assert_eq!(store.versions(&m.id).len(), 1);

        // Autosave right after a version doesn't add one; an explicit save does.
        doc.slides.push(Default::default());
        store.save(&m.id, &doc, Snapshot::IfDue, "").unwrap();
        assert_eq!(store.versions(&m.id).len(), 1);
        store.save(&m.id, &doc, Snapshot::IfChanged, "Saved").unwrap();
        assert_eq!(store.versions(&m.id).len(), 2);
        store.save(&m.id, &doc, Snapshot::IfChanged, "Saved").unwrap();
        assert_eq!(store.versions(&m.id).len(), 2, "unchanged content makes no version");

        let first = store.versions(&m.id).last().unwrap().time;
        store.rename(&m.id, "Renamed").unwrap();
        let restored = store.restore(&m.id, first).unwrap();
        assert_eq!(restored.slides.len(), 1);
        assert_eq!(restored.title, "Renamed");
        assert_eq!(store.load(&m.id).unwrap().slides.len(), 1);
        assert_eq!(store.meta(&m.id).unwrap().name, "Renamed");

        let copy = store.duplicate(&m.id).unwrap();
        assert_eq!(copy.name, "Renamed copy");
        assert_eq!(store.list().len(), 2);
        store.delete(&copy.id).unwrap();
        assert_eq!(store.list().len(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn file_bundles_local_images() {
        use crate::doc::{Item, ItemKind};
        let root = std::env::temp_dir().join(format!("biocartoon-file-test-{}", new_id()));
        let pic = root.join("a.png");
        storage::write(&pic, b"png bytes").unwrap();
        let image = |path: String| ItemKind::Image { source: ImageSource::file(path), crop: [0.0, 0.0, 1.0, 1.0], overlay: [0; 4] };
        let mut doc = Document::default();
        for (id, path) in [(1, pic.display().to_string()), (2, pic.display().to_string()), (3, "/missing.png".to_string())] {
            doc.slides[0].items.push(Item::new(id, "pic", image(path), egui::Pos2::ZERO, egui::Vec2::splat(10.0)));
        }

        let bytes = to_file(&doc).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["images"].as_object().unwrap().len(), 1, "one copy per distinct image");
        assert_eq!(json["title"], "Untitled", "the document stays at the top level");

        let back = from_file(&bytes, &root.join("uploads")).unwrap();
        let paths: Vec<String> = back.slides[0].items.iter().map(|it| match &it.kind {
            ItemKind::Image { source: ImageSource::File { path, .. }, .. } => path.clone(),
            _ => unreachable!(),
        }).collect();
        assert_eq!(paths[0], paths[1]);
        assert!(paths[0].starts_with(&root.join("uploads").display().to_string()));
        assert_eq!(storage::read(Path::new(&paths[0])).unwrap(), b"png bytes");
        assert_eq!(paths[2], "/missing.png");

        // A bare document (older files) still opens.
        assert_eq!(from_file(&serde_json::to_vec(&doc).unwrap(), &root).unwrap(), doc);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn zip_round_trip() {
        let mut z = crate::offline::ZipWriter::default();
        let mut bytes = Vec::new();
        for (name, data) in [("backup/a.biocartoon", &b"{}"[..]), ("backup/b.biocartoon", &b"hello"[..])] {
            bytes.extend(z.entry(name, data).unwrap());
            bytes.extend_from_slice(data);
        }
        bytes.extend(z.finish());
        let files = read_zip(&bytes).unwrap();
        assert_eq!(files, vec![("backup/a.biocartoon".to_string(), b"{}".to_vec()), ("backup/b.biocartoon".to_string(), b"hello".to_vec())]);
        assert!(read_zip(b"not a zip at all, just some text").is_err());
    }

    #[test]
    fn ids_are_unique_uuids() {
        let ids: std::collections::HashSet<String> = (0..1000).map(|_| new_id()).collect();
        assert_eq!(ids.len(), 1000);
        for id in &ids {
            let parts: Vec<&str> = id.split('-').collect();
            assert_eq!(parts.iter().map(|p| p.len()).collect::<Vec<_>>(), [8, 4, 4, 4, 12], "{id}");
            assert!(parts[2].starts_with('4') && "89ab".contains(&parts[3][..1]), "{id}");
        }
    }

    #[test]
    fn ordinal_dates() {
        use chrono::TimeZone;
        let ms = |d| chrono::Local.with_ymd_and_hms(2026, 10, d, 12, 0, 0).unwrap().timestamp_millis();
        assert_eq!(format_date(ms(1)), "Oct 1st, 2026");
        assert_eq!(format_date(ms(12)), "Oct 12th, 2026");
        assert_eq!(format_date(ms(22)), "Oct 22nd, 2026");
    }
}
