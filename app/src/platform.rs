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
    return std::fs::write(path, bytes);
    #[cfg(target_arch = "wasm32")]
    {
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "download".into());
        web::download(&name, bytes).map_err(std::io::Error::other)
    }
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
