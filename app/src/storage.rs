//! Byte-level storage for app data (projects, versions, thumbnails), addressed by path.
//!
//! On the desktop this is the file system. The web build keeps everything in memory, mirrored to
//! the browser's IndexedDB (a guest's storage); this is also where a server / R2 backend for
//! signed-in users plugs in.

use std::io;
use std::path::Path;

#[cfg(not(target_arch = "wasm32"))]
mod imp {
    use super::*;

    pub fn read(p: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(p)
    }

    /// Write via a temp file and rename, so a crash never leaves a half-written file.
    pub fn write(p: &Path, bytes: &[u8]) -> io::Result<()> {
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = p.with_extension("tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, p)
    }

    pub fn exists(p: &Path) -> bool {
        p.exists()
    }

    pub fn subdirs(p: &Path) -> Vec<String> {
        let Ok(rd) = std::fs::read_dir(p) else { return Vec::new() };
        rd.flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().to_string()).collect()
    }

    pub fn modified_ms(p: &Path) -> Option<i64> {
        let t = std::fs::metadata(p).ok()?.modified().ok()?;
        Some(chrono::DateTime::<chrono::Utc>::from(t).timestamp_millis())
    }

    pub fn copy(from: &Path, to: &Path) -> io::Result<()> {
        std::fs::copy(from, to).map(|_| ())
    }

    pub fn remove_dir_all(p: &Path) -> io::Result<()> {
        std::fs::remove_dir_all(p)
    }

    pub fn create_dir_all(p: &Path) -> io::Result<()> {
        std::fs::create_dir_all(p)
    }

    /// Files on disk stay until the user deletes them.
    pub fn persisted() -> Option<bool> {
        Some(true)
    }
}

#[cfg(target_arch = "wasm32")]
mod imp {
    use super::*;
    use idb::{Database, DatabaseEvent, Factory, ObjectStoreParams, TransactionMode};
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::sync::Mutex;
    use wasm_bindgen::JsValue;

    /// Every file, mirrored from IndexedDB at startup so reads stay synchronous.
    static FILES: Mutex<BTreeMap<PathBuf, (Vec<u8>, i64)>> = Mutex::new(BTreeMap::new());

    /// The browser's answer to [`request_persist`]: 0 not yet known, 1 granted, 2 refused.
    static PERSISTED: AtomicU8 = AtomicU8::new(0);

    const DB_NAME: &str = "biocartoon";
    const STORE: &str = "files";

    thread_local! {
        static DB: RefCell<Option<Rc<Database>>> = const { RefCell::new(None) };
    }

    /// Load everything saved in this browser. Call once before the app starts; without
    /// IndexedDB (some private windows) the app still runs, keeping files until the tab closes.
    pub async fn init() {
        match load().await {
            Ok((db, n)) => {
                log::info!("storage: loaded {n} files from IndexedDB");
                DB.with(|d| *d.borrow_mut() = Some(Rc::new(db)));
            }
            Err(e) => log::warn!("storage: IndexedDB unavailable, nothing will be kept after reload: {e}"),
        }
        // Not awaited: Firefox may ask the user, and the app shouldn't wait on the answer.
        wasm_bindgen_futures::spawn_local(request_persist());
    }

    /// Ask the browser to keep this site's storage. Without it IndexedDB is "best effort": the
    /// browser may clear it under storage pressure, and Safari clears it after about 7 days without
    /// a visit. Chrome and Edge grant it quietly to sites the user engages with (or has installed);
    /// Firefox may ask. Either way it never guards against the user clearing site data.
    async fn request_persist() {
        let Some(storage) = web_sys::window().map(|w| w.navigator().storage()) else { return };
        let ask = |p: Result<js_sys::Promise, JsValue>| async move { wasm_bindgen_futures::JsFuture::from(p.ok()?).await.ok()?.as_bool() };
        let granted = match ask(storage.persisted()).await {
            Some(true) => Some(true),
            _ => ask(storage.persist()).await,
        };
        log::info!("storage: persistent storage {}", match granted { Some(true) => "granted", Some(false) => "refused", None => "unavailable" });
        PERSISTED.store(match granted { Some(true) => 1, Some(false) => 2, None => 0 }, Ordering::Relaxed);
    }

    /// Whether the browser promised to keep this site's storage (`None` until it answers, or if it
    /// can't say).
    pub fn persisted() -> Option<bool> {
        match PERSISTED.load(Ordering::Relaxed) {
            1 => Some(true),
            2 => Some(false),
            _ => None,
        }
    }

    async fn load() -> Result<(Database, usize), idb::Error> {
        let mut open = Factory::new()?.open(DB_NAME, Some(1))?;
        open.on_upgrade_needed(|event| {
            if let Ok(db) = event.database() {
                if let Err(e) = db.create_object_store(STORE, ObjectStoreParams::new()) {
                    log::error!("storage: create object store: {e}");
                }
            }
        });
        let db = open.await?;
        let tx = db.transaction(&[STORE], TransactionMode::ReadOnly)?;
        let store = tx.object_store(STORE)?;
        // Both come back in key order, so they pair up.
        let keys = store.get_all_keys(None, None)?.await?;
        let values = store.get_all(None, None)?.await?;
        let now = crate::projects::now_ms();
        let mut files = FILES.lock().unwrap();
        for (k, v) in keys.into_iter().zip(values) {
            if let Some(k) = k.as_string() {
                files.insert(PathBuf::from(k), (js_sys::Uint8Array::new(&v).to_vec(), now));
            }
        }
        Ok((db, files.len()))
    }

    /// Mirror a change to IndexedDB in the background (`None` deletes).
    fn persist(key: &Path, value: Option<Vec<u8>>) {
        let Some(db) = DB.with(|d| d.borrow().clone()) else { return };
        let key = key.to_string_lossy().to_string();
        wasm_bindgen_futures::spawn_local(async move {
            let res: Result<(), idb::Error> = async {
                let tx = db.transaction(&[STORE], TransactionMode::ReadWrite)?;
                let store = tx.object_store(STORE)?;
                let k = JsValue::from_str(&key);
                match value {
                    Some(bytes) => {
                        store.put(&js_sys::Uint8Array::from(&bytes[..]).into(), Some(&k))?.await?;
                    }
                    None => store.delete(k)?.await?,
                }
                tx.commit()?.await?;
                Ok(())
            }
            .await;
            if let Err(e) = res {
                log::error!("storage: saving {key}: {e}");
            }
        });
    }

    fn not_found(p: &Path) -> io::Error {
        io::Error::new(io::ErrorKind::NotFound, format!("{} not found", p.display()))
    }

    pub fn read(p: &Path) -> io::Result<Vec<u8>> {
        FILES.lock().unwrap().get(p).map(|(b, _)| b.clone()).ok_or_else(|| not_found(p))
    }

    pub fn write(p: &Path, bytes: &[u8]) -> io::Result<()> {
        FILES.lock().unwrap().insert(p.to_path_buf(), (bytes.to_vec(), crate::projects::now_ms()));
        persist(p, Some(bytes.to_vec()));
        Ok(())
    }

    /// A path exists if it is a file or the parent of one.
    pub fn exists(p: &Path) -> bool {
        FILES.lock().unwrap().keys().any(|k| k.starts_with(p))
    }

    pub fn subdirs(p: &Path) -> Vec<String> {
        let files = FILES.lock().unwrap();
        let mut out: Vec<String> = files
            .keys()
            .filter_map(|k| k.strip_prefix(p).ok())
            .filter(|rest| rest.components().count() > 1)
            .filter_map(|rest| rest.components().next())
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect();
        out.dedup();
        out
    }

    pub fn modified_ms(p: &Path) -> Option<i64> {
        FILES.lock().unwrap().get(p).map(|(_, t)| *t)
    }

    pub fn copy(from: &Path, to: &Path) -> io::Result<()> {
        let bytes = read(from)?;
        write(to, &bytes)
    }

    /// Folders exist implicitly as the parents of files, so there is nothing to create.
    pub fn create_dir_all(_: &Path) -> io::Result<()> {
        Ok(())
    }

    pub fn remove_dir_all(p: &Path) -> io::Result<()> {
        let gone: Vec<PathBuf> = {
            let mut files = FILES.lock().unwrap();
            let gone = files.keys().filter(|k| k.starts_with(p)).cloned().collect();
            files.retain(|k, _| !k.starts_with(p));
            gone
        };
        for k in gone {
            persist(&k, None);
        }
        Ok(())
    }
}

pub use imp::*;

pub fn read_to_string(p: &Path) -> io::Result<String> {
    String::from_utf8(read(p)?).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}
