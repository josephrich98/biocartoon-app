//! The few things that differ between the desktop build and the web (wasm32) build.

use std::path::{Path, PathBuf};

/// Run a one-shot job off the UI thread. The browser build has no threads, so it runs the job
/// right away; callers already hand results back over channels, which works either way.
pub fn spawn(name: &str, f: impl FnOnce() + Send + 'static) {
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::Builder::new().name(name.into()).spawn(f).expect("spawn worker thread");
    #[cfg(target_arch = "wasm32")]
    {
        let _ = name;
        f();
    }
}

/// Ask the user for files to open. `done` gets their paths, readable through [`crate::storage`];
/// it isn't called if the user cancels. On the desktop this is a blocking dialog and `done` runs
/// before returning. In the browser it runs later, once the chosen files have been read and copied
/// into storage under `uploads/`, so callers hand the result back to the app through a channel.
pub fn pick_files(filter: &str, exts: &[&str], multiple: bool, done: impl FnOnce(Vec<PathBuf>) + 'static) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let dialog = ::rfd::FileDialog::new().add_filter(filter, exts);
        let picked = if multiple { dialog.pick_files() } else { dialog.pick_file().map(|p| vec![p]) };
        if let Some(paths) = picked.filter(|p| !p.is_empty()) {
            done(paths);
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = filter;
        web::pick_files(exts, multiple, done);
    }
}

/// Where to write a file the user is saving: a save dialog on the desktop (`None` if cancelled).
/// The browser can't pick a location, so it returns just `file_name`, and [`write_output`]
/// turns the write into a download.
pub fn save_path(filter: &str, ext: &str, file_name: &str) -> Option<PathBuf> {
    #[cfg(not(target_arch = "wasm32"))]
    return ::rfd::FileDialog::new().add_filter(filter, &[ext]).set_file_name(file_name).save_file();
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (filter, ext);
        Some(PathBuf::from(file_name))
    }
}

/// Write a file the user asked to save or export (a path from [`save_path`]). In the browser this
/// downloads `bytes` under the path's file name.
pub fn write_output(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::fs::write(path, bytes)?;
        #[cfg(target_os = "macos")]
        bounce_downloads(path);
        Ok(())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "download".into());
        web::download(&name, bytes).map_err(std::io::Error::other)
    }
}

/// If `path` is in ~/Downloads, bounce the Downloads stack in the Dock the way a browser does when a
/// download finishes.
#[cfg(target_os = "macos")]
fn bounce_downloads(path: &Path) {
    use objc2_foundation::{NSDistributedNotificationCenter, NSString};
    let Some(downloads) = dirs::download_dir() else { return };
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let downloads = downloads.canonicalize().unwrap_or(downloads);
    if !path.starts_with(&downloads) {
        return;
    }
    let name = NSString::from_str("com.apple.DownloadFileFinished");
    let file = NSString::from_str(&path.to_string_lossy());
    unsafe { NSDistributedNotificationCenter::defaultCenter().postNotificationName_object(&name, Some(&file)) };
}

/// Keep the browser from dropping unsaved work: ask for a save as soon as the tab is hidden or
/// closing, and have the browser confirm leaving while changes or storage writes are pending. The
/// desktop needs none of this, since `on_exit` saves. Call once at startup.
pub fn guard_unsaved(ctx: &egui::Context) {
    #[cfg(target_arch = "wasm32")]
    web::guard_unsaved(ctx);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = ctx;
}

/// Tell [`guard_unsaved`] whether the open document has changes not yet saved.
pub fn set_unsaved(unsaved: bool) {
    #[cfg(target_arch = "wasm32")]
    web::UNSAVED.with(|u| u.set(unsaved));
    #[cfg(not(target_arch = "wasm32"))]
    let _ = unsaved;
}

/// Whether the tab was hidden or started closing since the last call, so the app should save now.
pub fn take_save_request() -> bool {
    #[cfg(target_arch = "wasm32")]
    return web::SAVE_NOW.with(|s| s.replace(false));
    #[cfg(not(target_arch = "wasm32"))]
    false
}

/// Browser build: save a Blob (e.g. a zip assembled in parts) as a download.
#[cfg(target_arch = "wasm32")]
pub fn download_blob(name: &str, blob: &web_sys::Blob) -> Result<(), String> {
    web::download_blob(name, blob)
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::path::PathBuf;
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast;

    fn err(e: wasm_bindgen::JsValue) -> String {
        format!("{e:?}")
    }

    thread_local! {
        pub static UNSAVED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        pub static SAVE_NOW: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    pub fn guard_unsaved(ctx: &egui::Context) {
        let Some(window) = web_sys::window() else { return };
        let ctx = ctx.clone();
        let save_now = move || {
            SAVE_NOW.with(|s| s.set(true));
            ctx.request_repaint();
        };
        // Hiding is the last reliable moment on phones, which often discard background tabs.
        let on_hide = {
            let save_now = save_now.clone();
            Closure::<dyn FnMut()>::new(move || {
                if web_sys::window().and_then(|w| w.document()).is_some_and(|d| d.hidden()) {
                    save_now();
                }
            })
        };
        // No frame runs once the page unloads, so a save that isn't done yet needs the prompt;
        // if the user stays, the requested save runs on the next frame.
        let on_unload = Closure::<dyn FnMut(web_sys::BeforeUnloadEvent)>::new(move |e: web_sys::BeforeUnloadEvent| {
            save_now();
            if UNSAVED.with(std::cell::Cell::get) || crate::storage::writes_pending() {
                e.prevent_default();
            }
        });
        if let Some(doc) = window.document() {
            let _ = doc.add_event_listener_with_callback("visibilitychange", on_hide.as_ref().unchecked_ref());
        }
        let _ = window.add_event_listener_with_callback("beforeunload", on_unload.as_ref().unchecked_ref());
        on_hide.forget();
        on_unload.forget();
    }

    /// Clicks a hidden `<input type="file">`. Browsers only open pickers shortly after a user
    /// gesture; egui handles clicks on the next frame, which is well within that window.
    pub fn pick_files(exts: &[&str], multiple: bool, done: impl FnOnce(Vec<PathBuf>) + 'static) {
        let res = (|| -> Result<(), String> {
            let doc = web_sys::window().and_then(|w| w.document()).ok_or("no document")?;
            let input: web_sys::HtmlInputElement = doc.create_element("input").map_err(err)?.dyn_into().map_err(|_| "not an input")?;
            input.set_type("file");
            input.set_multiple(multiple);
            input.set_accept(&exts.iter().map(|e| format!(".{e}")).collect::<Vec<_>>().join(","));
            let inp = input.clone();
            let on_change = Closure::once(move || {
                let files: Vec<web_sys::File> = inp.files().map(|l| (0..l.length()).filter_map(|i| l.get(i)).collect()).unwrap_or_default();
                wasm_bindgen_futures::spawn_local(async move {
                    let stamp = crate::projects::now_ms();
                    let mut paths = Vec::new();
                    for (i, f) in files.into_iter().enumerate() {
                        match wasm_bindgen_futures::JsFuture::from(f.array_buffer()).await {
                            Ok(buf) => {
                                let bytes = js_sys::Uint8Array::new(&buf).to_vec();
                                let path = PathBuf::from("uploads").join(format!("{stamp}-{i}")).join(f.name());
                                match crate::storage::write(&path, &bytes) {
                                    Ok(()) => paths.push(path),
                                    Err(e) => log::error!("upload {}: {e}", f.name()),
                                }
                            }
                            Err(e) => log::error!("reading {}: {e:?}", f.name()),
                        }
                    }
                    if !paths.is_empty() {
                        done(paths);
                    }
                });
            });
            input.set_onchange(Some(on_change.as_ref().unchecked_ref()));
            on_change.forget(); // never called (and leaked) if the user cancels; it's tiny
            input.click();
            Ok(())
        })();
        if let Err(e) = res {
            log::error!("file picker: {e}");
        }
    }

    /// Save `bytes` through the browser's download mechanism (a temporary blob: link).
    pub fn download(name: &str, bytes: &[u8]) -> Result<(), String> {
        let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
        let blob = web_sys::Blob::new_with_u8_array_sequence(&parts).map_err(err)?;
        download_blob(name, &blob)
    }

    pub fn download_blob(name: &str, blob: &web_sys::Blob) -> Result<(), String> {
        let doc = web_sys::window().and_then(|w| w.document()).ok_or("no document")?;
        let url = web_sys::Url::create_object_url_with_blob(blob).map_err(err)?;
        let a: web_sys::HtmlAnchorElement = doc.create_element("a").map_err(err)?.dyn_into().map_err(|_| "not an anchor")?;
        a.set_href(&url);
        a.set_download(name);
        a.click();
        // Revoking right away can cancel the download in some browsers; give it a moment.
        let revoke = Closure::once_into_js(move || {
            let _ = web_sys::Url::revoke_object_url(&url);
        });
        if let Some(w) = web_sys::window() {
            let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(revoke.unchecked_ref(), 10_000);
        }
        Ok(())
    }
}

/// Web stand-in for the `rfd` crate's blocking dialogs, for the few desktop-only settings that
/// still use them (a local catalog file or folder): every picker is cancelled.
/// `use crate::platform::rfd;` shadows the real crate on wasm so call sites stay unchanged.
#[cfg(target_arch = "wasm32")]
pub mod rfd {
    use std::path::PathBuf;

    #[derive(Default)]
    pub struct FileDialog;

    impl FileDialog {
        pub fn new() -> Self {
            Self
        }
        pub fn add_filter(self, _name: &str, _ext: &[&str]) -> Self {
            self
        }
        pub fn set_file_name(self, _name: impl Into<String>) -> Self {
            self
        }
        pub fn set_directory(self, _dir: impl AsRef<std::path::Path>) -> Self {
            self
        }
        pub fn set_title(self, _title: impl Into<String>) -> Self {
            self
        }
        pub fn pick_file(self) -> Option<PathBuf> {
            log::warn!("file dialogs are not available in the web build yet");
            None
        }
        pub fn pick_files(self) -> Option<Vec<PathBuf>> {
            self.pick_file().map(|p| vec![p])
        }
        pub fn pick_folder(self) -> Option<PathBuf> {
            self.pick_file()
        }
        pub fn save_file(self) -> Option<PathBuf> {
            self.pick_file()
        }
    }
}
