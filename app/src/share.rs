//! Share links. "Share" in the editor puts the open project online, encrypted on this device
//! (AES-256-GCM), at the submissions Worker's `/shares` routes (the biocartoon repo's
//! `worker/src/shares.ts`), and gives two links:
//!
//! - the **edit link** (`#share=<id>.<key>.<edit secret>`): whoever opens it gets the project as
//!   one of their own, and their saves go back to the same shared copy;
//! - the **view link** (`#share=<id>.<key>`): the same, but their changes stay with them.
//!
//! The key is only in the link's fragment, which browsers never send to a server, so the Worker
//! keeps bytes it can't read. The edit secret lets a holder save; the owner secret, which stays
//! in the sharer's app, lets them turn sharing off. Anyone can share, signed in or not.
//!
//! A project tied to a link keeps it in its `project.json` ([`ProjectMeta::share`]). While it is
//! open in the editor:
//!
//! - its online copy is checked on opening and every [`CHECK_EVERY_S`] seconds. On opening, a
//!   newer version comes straight in if nothing changed here; later, the editor asks first;
//! - with an edit link, changes go up once left alone for [`UPLOAD_QUIET_MS`] (or every
//!   [`UPLOAD_EVERY_S`] while editing goes on), each naming the version it was made from. If
//!   someone else saved in between, the Worker refuses it and the editor asks which to keep. The
//!   one not kept stays in the version history (here) or among the earlier versions (online).
//!
//! [`ProjectMeta::share`]: crate::projects::ProjectMeta

use crate::app::{BioCartoon, Screen};
use crate::doc::Document;
use crate::projects::{now_ms, Snapshot};
use crate::theme;
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::Engine;
use egui::{Color32, Vec2};
use egui_phosphor::regular as ph;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::sync::mpsc::{channel, Receiver, Sender};

/// Where links open when the app isn't the web app (the web app uses its own address).
pub const WEB_APP: &str = "https://app.biocartoon.com/";
/// One version of a shared project, encrypted. Keep in sync with the Worker's src/limits.ts.
pub const MAX_SHARE_BYTES: usize = 32 * 1024 * 1024;
/// How long a link nobody uses lasts (the Worker's SHARE_IDLE_DAYS), and how many versions it keeps.
pub const SHARE_IDLE_DAYS: i64 = 90;
pub const SHARE_VERSIONS: usize = 5;
/// An edited shared project goes up once it has gone this long without changes...
pub const UPLOAD_QUIET_MS: i64 = 20_000;
/// ...or this long after its last upload, while editing continues.
pub const UPLOAD_EVERY_S: f64 = 300.0;
/// How often the open project's online copy is checked for someone else's changes.
pub const CHECK_EVERY_S: f64 = 60.0;
const RETRY_S: f64 = 60.0;
/// Encrypted projects start with these bytes, then the nonce and the ciphertext.
const MAGIC: &[u8; 4] = b"BCS1";
const NONCE_LEN: usize = 12;
const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// A project's share link, as kept in its `project.json`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ShareLink {
    pub id: String,
    /// The AES-256 key, base64url. Never sent to the server.
    pub key: String,
    /// The edit secret: empty for a view link.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub edit: String,
    /// The owner secret, kept only by the app that shared: it can turn sharing off.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub owner: String,
    /// The online version this project's copy was last in step with.
    pub version: i64,
    /// The project's "last modified" then: anything later is a change made here since.
    pub synced: i64,
}

impl ShareLink {
    pub fn can_edit(&self) -> bool {
        !self.edit.is_empty()
    }

    pub fn is_owner(&self) -> bool {
        !self.owner.is_empty()
    }

    /// The link that lets its holder edit (only for a link that can itself edit).
    pub fn edit_url(&self, page: &str) -> Option<String> {
        self.can_edit().then(|| format!("{page}#share={}.{}.{}", self.id, self.key, self.edit))
    }

    pub fn view_url(&self, page: &str) -> String {
        format!("{page}#share={}.{}", self.id, self.key)
    }

    /// A link (pasted, or this page's fragment) as a `ShareLink` not yet tied to a project.
    pub fn parse(text: &str) -> Option<ShareLink> {
        let start = text.find("share=")? + "share=".len();
        let token: String = text[start..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.').collect();
        let mut parts = token.split('.');
        let (id, key) = (parts.next()?, parts.next()?);
        let edit = parts.next().unwrap_or_default();
        let ok = |s: &str, bytes: usize| B64.decode(s).is_ok_and(|b| b.len() == bytes);
        if parts.next().is_some() || !ok(id, 16) || !ok(key, 32) || !(edit.is_empty() || ok(edit, 16)) {
            return None;
        }
        Some(ShareLink { id: id.into(), key: key.into(), edit: edit.into(), ..Default::default() })
    }
}

/// `n` random bytes from the operating system (the browser's `crypto.getRandomValues` on the web).
pub fn random(n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    #[cfg(target_arch = "wasm32")]
    {
        let crypto = web_sys::window().and_then(|w| w.crypto().ok()).expect("crypto.getRandomValues");
        crypto.get_random_values_with_u8_array(&mut b).expect("crypto.getRandomValues");
    }
    #[cfg(not(target_arch = "wasm32"))]
    getrandom::fill(&mut b).expect("the system's random numbers");
    b
}

fn cipher(key: &str) -> Result<Aes256Gcm, String> {
    let key = B64.decode(key).ok().filter(|k| k.len() == 32).ok_or("bad share key")?;
    Ok(Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key)))
}

/// Encrypt a project for its share link: the `.biocartoon` file (images included), gzipped.
pub fn seal(doc: &Document, key: &str) -> Result<Vec<u8>, String> {
    let file = crate::projects::to_file(doc)?;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&file).map_err(|e| e.to_string())?;
    let plain = gz.finish().map_err(|e| e.to_string())?;
    let nonce = random(NONCE_LEN);
    let sealed = cipher(key)?.encrypt(Nonce::from_slice(&nonce), plain.as_slice()).map_err(|_| "couldn't encrypt the project".to_string())?;
    let out = [MAGIC.as_slice(), &nonce, &sealed].concat();
    if out.len() > MAX_SHARE_BYTES {
        return Err(format!("this project is over {} MB, too large to share", MAX_SHARE_BYTES >> 20));
    }
    Ok(out)
}

/// The `.biocartoon` file inside an encrypted project.
pub fn unseal(bytes: &[u8], key: &str) -> Result<Vec<u8>, String> {
    let body = bytes.strip_prefix(MAGIC.as_slice()).filter(|b| b.len() > NONCE_LEN).ok_or("this isn't a shared BioCartoon project")?;
    let (nonce, sealed) = body.split_at(NONCE_LEN);
    let plain = cipher(key)?.decrypt(Nonce::from_slice(nonce), sealed).map_err(|_| "the link's key doesn't open this project (was the link copied in full?)".to_string())?;
    let mut file = Vec::new();
    flate2::read::GzDecoder::new(plain.as_slice()).read_to_end(&mut file).map_err(|e| e.to_string())?;
    Ok(file)
}

/// What the Worker says about a share.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Remote {
    pub version: i64,
    pub updated: String,
    pub bytes: i64,
    pub expires: String,
    pub versions: Vec<RemoteVersion>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct RemoteVersion {
    pub version: i64,
    pub bytes: i64,
    pub updated: String,
}

/// An ISO time from the Worker as Unix milliseconds (0 if unreadable).
fn iso_ms(s: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(s).map(|t| t.timestamp_millis()).unwrap_or(0)
}

/// Why a version is being downloaded.
#[derive(Clone, Debug)]
enum Fetch {
    /// The newest version, into the open project.
    Load,
    /// An earlier version, into the open project (and then up again as the newest).
    Restore,
    /// A link being opened: into a new project.
    Open(ShareLink),
}

enum Msg {
    Created(String, i64, ShareLink, Result<Reply, Reply>),
    Info(String, Result<Reply, Reply>),
    Saved(String, i64, Result<Reply, Reply>),
    Fetched(Option<String>, Fetch, Result<Reply, Reply>),
    Stopped(String, Result<Reply, Reply>),
}

/// A finished request: its status, body and `x-share-version` header; or why it failed.
#[derive(Debug)]
struct Reply {
    status: u16,
    body: Vec<u8>,
    version: Option<i64>,
}

impl Reply {
    fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or_default()
    }

    fn message(&self) -> String {
        match self.status {
            0 => String::from_utf8_lossy(&self.body).to_string(),
            s => self.json()["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the server answered HTTP {s}")),
        }
    }
}

/// The share state of the open project, and the windows.
pub struct Sharer {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    busy: bool,
    retry_at: f64,
    /// The Share window, and its "Stop sharing" waiting for a second click.
    pub show: bool,
    confirm_stop: bool,
    /// The "Open a share link" window and what is typed in it.
    pub show_open: bool,
    pub open_text: String,
    /// A link to open once nothing else is under way (from the address or the Open window).
    pending_open: Option<ShareLink>,
    /// The project the rest is about, its link and "last modified" (re-read every couple of seconds).
    project: Option<String>,
    link: Option<ShareLink>,
    modified: i64,
    meta_at: f64,
    /// What the Worker last said about the link; whether it is gone (expired or turned off).
    pub remote: Option<Remote>,
    gone: bool,
    check_at: f64,
    /// Nothing has been checked since the project opened (a newer version then comes straight in).
    first_check: bool,
    /// A newer version online, not loaded here; the one the user said "Later" to.
    incoming: Option<i64>,
    dismissed: i64,
    /// The last upload was refused because someone else had saved first.
    conflict: bool,
    last_upload: f64,
    /// An upload refused as too large, by the "last modified" refused: not retried until it changes.
    refused: i64,
    pub error: Option<String>,
    ctx: egui::Context,
}

impl Sharer {
    pub fn new(ctx: egui::Context) -> Self {
        let (tx, rx) = channel();
        let pending_open = web::take_fragment();
        Self {
            tx,
            rx,
            busy: false,
            retry_at: 0.0,
            show: false,
            confirm_stop: false,
            show_open: false,
            open_text: String::new(),
            pending_open,
            project: None,
            link: None,
            modified: 0,
            meta_at: f64::NEG_INFINITY,
            remote: None,
            gone: false,
            check_at: 0.0,
            first_check: true,
            incoming: None,
            dismissed: 0,
            conflict: false,
            last_upload: 0.0,
            refused: 0,
            error: None,
            ctx,
        }
    }

    fn send(&self) -> impl Fn(Msg) + Send + 'static {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        move |m| {
            let _ = tx.send(m);
            ctx.request_repaint();
        }
    }

    /// Forget the per-project state (another project opened, or the editor closed).
    fn reset(&mut self, project: Option<String>) {
        self.project = project;
        self.link = None;
        self.remote = None;
        self.gone = false;
        self.check_at = 0.0;
        self.first_check = true;
        self.incoming = None;
        self.dismissed = 0;
        self.conflict = false;
        self.refused = 0;
        self.error = None;
        self.meta_at = f64::NEG_INFINITY;
        self.confirm_stop = false;
    }

    /// Re-read the project's link and "last modified" on the next poll.
    fn reread(&mut self) {
        self.meta_at = f64::NEG_INFINITY;
    }
}

/// The page links open on: this one in the web app, the web app elsewhere.
fn link_page() -> String {
    web::here().unwrap_or_else(|| WEB_APP.to_string())
}

impl BioCartoon {
    /// Run the open project's share link: apply finished requests, check for changes, send
    /// changes up. Called every frame; cheap when there is nothing to do.
    pub fn poll_share(&mut self) {
        while let Ok(m) = self.share.rx.try_recv() {
            self.share_msg(m);
        }
        if let Some(link) = web::poll_fragment() {
            self.share.pending_open = Some(link);
        }
        let Some(base) = self.submit_url() else { return };
        if !self.share.busy {
            if let Some(link) = self.share.pending_open.take() {
                return self.open_share_link(&base, link);
            }
        }
        let open = self.project.clone().filter(|_| self.screen == Screen::Editor);
        if open != self.share.project {
            self.share.reset(open.clone());
        }
        let Some(id) = open else { return };
        let now = self.ctx_time();
        if now - self.share.meta_at >= 2.0 {
            self.share.meta_at = now;
            match self.store.meta(&id) {
                Ok(m) => (self.share.link, self.share.modified) = (m.share, m.modified),
                Err(_) => self.share.link = None,
            }
        }
        let s = &mut self.share;
        let Some(link) = s.link.clone() else { return };
        // Egui only draws on input: wake up for the next check or upload.
        self.ctx.request_repaint_after(std::time::Duration::from_secs(5));
        if s.busy || s.gone || now < s.retry_at {
            return;
        }
        let send = s.send();
        if now >= s.check_at {
            s.check_at = now + CHECK_EVERY_S;
            s.busy = true;
            let url = format!("{base}/shares/{}", link.id);
            return request("GET", url, Vec::new(), Vec::new(), move |r| send(Msg::Info(id, r)));
        }
        let up_to_date = s.remote.as_ref().is_some_and(|r| r.version == link.version);
        let changed = s.modified > link.synced && s.modified != s.refused;
        let due = now_ms() - s.modified >= UPLOAD_QUIET_MS || now - s.last_upload >= UPLOAD_EVERY_S;
        if !(link.can_edit() && up_to_date && changed && due && !s.conflict && s.incoming.is_none()) {
            return;
        }
        let modified = s.modified;
        match self.store.load(&id).and_then(|d| seal(&d, &link.key)) {
            Ok(bytes) => {
                let s = &mut self.share;
                (s.busy, s.last_upload) = (true, now);
                let headers = vec![("Content-Type", "application/octet-stream".to_string()), ("x-share-edit", link.edit.clone()), ("x-share-base", link.version.to_string())];
                request("PUT", format!("{base}/shares/{}", link.id), headers, bytes, move |r| send(Msg::Saved(id, modified, r)));
            }
            Err(e) => {
                self.share.refused = modified;
                self.share.error = Some(format!("Couldn't save the shared copy: {e}"));
            }
        }
    }

    fn ctx_time(&self) -> f64 {
        self.ctx.input(|i| i.time)
    }

    fn share_msg(&mut self, m: Msg) {
        self.share.busy = false;
        match m {
            Msg::Created(id, modified, mut link, Ok(r)) => {
                link.id = r.json()["id"].as_str().unwrap_or_default().to_string();
                (link.version, link.synced) = (1, modified);
                if let Err(e) = self.store.set_share(&id, Some(link.clone())) {
                    return self.toast_err(format!("Couldn't keep the share link: {e}"));
                }
                self.share.reread();
                self.share.check_at = 0.0;
                if let Some(url) = link.edit_url(&link_page()) {
                    self.ctx.copy_text(url);
                }
                self.toast("Share link copied. Anyone with it can open and edit this project.");
            }
            Msg::Info(id, Ok(r)) if self.share.project.as_ref() == Some(&id) => {
                let Ok(remote) = serde_json::from_slice::<Remote>(&r.body) else { return };
                let s = &mut self.share;
                let first = std::mem::replace(&mut s.first_check, false);
                s.error = None;
                if let Some(link) = s.link.clone().filter(|l| remote.version > l.version) {
                    let edited_here = s.modified > link.synced || self.dirty;
                    if first && !edited_here {
                        s.remote = Some(remote.clone());
                        return self.fetch_share_version(remote.version, Fetch::Load);
                    }
                    s.incoming = Some(remote.version);
                } else {
                    s.incoming = None;
                }
                s.remote = Some(remote);
            }
            Msg::Info(id, Err(e)) if self.share.project.as_ref() == Some(&id) => {
                if e.status == 404 {
                    (self.share.gone, self.share.remote, self.share.incoming) = (true, None, None);
                    self.share.error = Some("This project's share link has expired or was turned off. The project is still here.".into());
                } else {
                    self.share_failed(e);
                }
            }
            Msg::Saved(id, modified, Ok(r)) => {
                let version = r.json()["version"].as_i64().unwrap_or(0);
                if let Ok(mut m) = self.store.meta(&id) {
                    if let Some(link) = &mut m.share {
                        (link.version, link.synced) = (version, modified);
                        let _ = self.store.set_share(&id, m.share.clone());
                    }
                }
                let s = &mut self.share;
                if s.project.as_ref() == Some(&id) {
                    if let Some(r) = &mut s.remote {
                        r.version = version;
                    }
                    s.error = None;
                    s.check_at = 0.0; // fetch the new list of versions
                    s.reread();
                }
            }
            Msg::Saved(_, _, Err(e)) if e.status == 409 => {
                let s = &mut self.share;
                s.conflict = true;
                s.incoming = e.json()["version"].as_i64().or(s.incoming);
                s.dismissed = 0;
                s.check_at = 0.0;
            }
            Msg::Saved(_, modified, Err(e)) if e.status == 413 || e.status == 400 => {
                self.share.refused = modified;
                self.share.error = Some(format!("Couldn't save the shared copy: {}", e.message()));
            }
            Msg::Saved(_, _, Err(e)) => self.share_failed(e),
            Msg::Fetched(id, how, Ok(r)) => self.share_fetched(id, how, r),
            Msg::Fetched(_, Fetch::Open(_), Err(e)) => self.toast_err(format!("Couldn't open the share link: {}", e.message())),
            Msg::Fetched(_, _, Err(e)) => {
                self.toast_err(format!("Couldn't download the shared project: {}", e.message()));
                self.share_failed(e);
            }
            Msg::Stopped(id, Ok(_)) => self.forget_share(&id, "Sharing turned off. The links no longer open this project."),
            Msg::Stopped(id, Err(e)) if e.status == 404 => self.forget_share(&id, "Sharing turned off."),
            Msg::Stopped(_, Err(e)) => self.toast_err(format!("Couldn't turn off sharing: {}", e.message())),
            Msg::Created(_, _, _, Err(e)) => self.toast_err(format!("Couldn't make a share link: {}", e.message())),
            // About a project no longer open.
            Msg::Info(..) => {}
        }
    }

    fn share_failed(&mut self, e: Reply) {
        self.share.error = Some(format!("Couldn't reach the shared copy: {}", e.message()));
        self.share.retry_at = self.ctx_time() + RETRY_S;
    }

    fn forget_share(&mut self, id: &str, message: &str) {
        if let Err(e) = self.store.set_share(id, None) {
            return self.toast_err(e);
        }
        self.share.reset(self.share.project.clone());
        self.share.show = false;
        self.toast(message);
    }

    /// Download version `version` of the open project's link.
    fn fetch_share_version(&mut self, version: i64, how: Fetch) {
        let (Some(base), Some(link), Some(id)) = (self.submit_url(), self.share.link.clone(), self.share.project.clone()) else { return };
        self.share.busy = true;
        let send = self.share.send();
        request("GET", format!("{base}/shares/{}/{version}", link.id), Vec::new(), Vec::new(), move |r| send(Msg::Fetched(Some(id), how, r)));
    }

    fn share_fetched(&mut self, id: Option<String>, how: Fetch, r: Reply) {
        let key = match &how {
            Fetch::Open(link) => link.key.clone(),
            _ => match &self.share.link {
                Some(l) => l.key.clone(),
                None => return,
            },
        };
        let uploads = self.app_dir.join("uploads");
        let doc = unseal(&r.body, &key).and_then(|f| crate::projects::from_file(&f, &uploads));
        let doc = match doc {
            Ok(d) => d,
            Err(e) => return self.toast_err(format!("Couldn't open the shared project: {e}")),
        };
        let version = r.version.unwrap_or(0);
        match (how, id) {
            (Fetch::Open(mut link), _) => {
                let made = self.store.create(&doc).and_then(|m| {
                    (link.version, link.synced) = (version, m.modified);
                    self.store.set_share(&m.id, Some(link.clone())).map(|_| m)
                });
                match made {
                    Ok(m) => {
                        self.store.mark_backed_up(&m.id);
                        self.close_project();
                        self.load_into_editor(doc.clone(), Some(m.id.clone()));
                        self.request_thumb(&m.id, &doc);
                        self.refresh_projects();
                        self.share.reset(Some(m.id));
                        self.share.first_check = false;
                        self.toast(if link.can_edit() {
                            format!("Opened \"{}\" from a share link. Your changes are saved to the shared copy too.", doc.title)
                        } else {
                            format!("Opened \"{}\" from a view link. Changes you make stay on this device.", doc.title)
                        });
                    }
                    Err(e) => self.toast_err(format!("Couldn't open the shared project: {e}")),
                }
            }
            (how, Some(id)) => {
                let restore = matches!(how, Fetch::Restore);
                let note = if restore { format!("Earlier shared version {version}") } else { "Changes from the share link".to_string() };
                let (latest, mut link) = match (self.share.remote.as_ref().map(|r| r.version), self.share.link.clone()) {
                    (Some(v), Some(l)) => (v, l),
                    _ => return,
                };
                let Ok(modified) = self.store.replace_with(&id, &doc, &note) else { return self.toast_err("Couldn't save the shared changes here") };
                if restore {
                    // Goes up again as the newest version (with an edit link) once left alone.
                    link.version = latest;
                } else {
                    (link.version, link.synced) = (version, modified);
                }
                let _ = self.store.set_share(&id, Some(link));
                if self.project.as_deref() == Some(id.as_str()) && self.screen == Screen::Editor {
                    self.commit(); // Cmd+Z goes back to what was on screen
                    self.doc = doc.clone();
                    self.saved_doc = Some(doc.clone());
                    self.cur_slide = self.cur_slide.min(doc.slides.len().saturating_sub(1));
                    self.selection.clear();
                    self.selected_guide = None;
                    self.dirty = false;
                }
                self.request_thumb(&id, &doc);
                let s = &mut self.share;
                (s.incoming, s.conflict) = (None, false);
                s.reread();
                s.check_at = 0.0;
                self.toast(match (restore, version) {
                    (true, v) => format!("Restored shared version {v}. What was here is kept in Version history."),
                    _ => "Loaded the latest changes to this shared project. What was here is kept in Version history.".to_string(),
                });
            }
            _ => {}
        }
    }

    /// Make a share link for the open project, and copy its edit link.
    pub fn create_share(&mut self) {
        let Some(base) = self.submit_url() else { return self.toast_err("No server is set up for share links (Settings)") };
        if !self.save_current(Snapshot::IfChanged, "Shared") {
            return;
        }
        let Some(id) = self.project.clone() else { return self.toast("Make a change first: there is nothing to share yet") };
        let secret = |n| B64.encode(random(n));
        let link = ShareLink { key: secret(32), edit: secret(16), owner: secret(16), ..Default::default() };
        let modified = self.store.meta(&id).map(|m| m.modified).unwrap_or_else(|_| now_ms());
        let bytes = match self.store.load(&id).and_then(|d| seal(&d, &link.key)) {
            Ok(b) => b,
            Err(e) => return self.toast_err(format!("Couldn't share: {e}")),
        };
        let mut headers = vec![("Content-Type", "application/octet-stream".to_string()), ("x-share-edit", link.edit.clone()), ("x-share-owner", link.owner.clone())];
        if let Some(t) = self.account.bearer() {
            headers.push(("Authorization", format!("Bearer {t}")));
        }
        self.share.busy = true;
        let send = self.share.send();
        request("POST", format!("{base}/shares"), headers, bytes, move |r| send(Msg::Created(id, modified, link, r)));
    }

    /// Turn sharing off (the owner) or untie this project from the link (anyone else).
    fn stop_share(&mut self) {
        let (Some(base), Some(link), Some(id)) = (self.submit_url(), self.share.link.clone(), self.share.project.clone()) else { return };
        if !link.is_owner() || self.share.gone {
            return self.forget_share(&id, "This project is no longer tied to the share link. It stays here as it is.");
        }
        self.share.busy = true;
        let send = self.share.send();
        request("DELETE", format!("{base}/shares/{}", link.id), vec![("x-share-owner", link.owner.clone())], Vec::new(), move |r| send(Msg::Stopped(id, r)));
    }

    /// Open a link: the project already tied to it, or a new project from its newest version.
    fn open_share_link(&mut self, base: &str, link: ShareLink) {
        let here = self.store.list().into_iter().find(|m| m.share.as_ref().is_some_and(|l| l.id == link.id));
        if let Some(mut m) = here {
            // An edit link for a project opened from a view link lets it edit from now on.
            if let Some(l) = m.share.as_mut().filter(|l| !l.can_edit() && link.can_edit()) {
                l.edit = link.edit.clone();
                let _ = self.store.set_share(&m.id, m.share.clone());
            }
            self.open_project(&m.id);
            return self.toast("This share link's project was already here: it is open, and checked for changes.");
        }
        self.share.busy = true;
        let send = self.share.send();
        request("GET", format!("{base}/shares/{}/latest", link.id), Vec::new(), Vec::new(), move |r| send(Msg::Fetched(None, Fetch::Open(link), r)));
    }

    /// The Share window.
    pub fn share_window(&mut self, ctx: &egui::Context) {
        if !self.share.show {
            return;
        }
        let mut open = true;
        let mut act: Option<&str> = None;
        let mut restore: Option<i64> = None;
        let page = link_page();
        let link = self.share.link.clone();
        // A modal, like Export: a click outside closes it. It fits the screen's width, scrolls if
        // it is too tall, and pinches to zoom (the whole app, until it closes).
        let zoom_id = egui::Id::new("share_base_zoom");
        crate::app::dialog_zoom(ctx, zoom_id);
        let width = crate::app::dialog_width(ctx, 420.0);
        let modal = egui::Modal::new(egui::Id::new("share_modal")).show(ctx, |ui| {
            crate::app::dialog_body(ui, egui::Id::new("share_body"), |ui| {
                ui.set_min_width(width);
                ui.set_max_width(width);
                ui.horizontal(|ui| {
                    ui.heading(format!("{}  Share", ph::SHARE_NETWORK));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(egui::Button::new(egui::RichText::new(ph::X).size(16.0)).frame(false)).on_hover_text("Close").clicked() {
                            open = false;
                        }
                    });
                });
                ui.add_space(4.0);
                let dim = |t: &str| egui::RichText::new(t).color(theme::text_dim());
                let Some(link) = &link else {
                    ui.label("Get a link to this project to send to a collaborator. Whoever opens it gets the project in their own BioCartoon, and their edits come back here.");
                    ui.add_space(4.0);
                    ui.label(dim(&format!(
                        "The project is encrypted on this device before it is uploaded, and the key is only in the link, so BioCartoon's server can't see it. Anyone with the link can open it. Links last until nobody has used them for {SHARE_IDLE_DAYS} days."
                    )));
                    ui.add_space(8.0);
                    let busy = self.share.busy;
                    if ui.add_enabled(!busy, egui::Button::new(egui::RichText::new(format!("{}  Create share link", ph::LINK)).color(Color32::WHITE)).fill(theme::accent())).clicked() || (!busy && theme::enter_pressed(ui)) {
                        act = Some("create");
                    }
                    if busy {
                        ui.label(dim("Uploading..."));
                    }
                    return;
                };
                let mut copy_row = |ui: &mut egui::Ui, title: &str, about: &str, url: String| {
                    ui.label(egui::RichText::new(title).strong());
                    ui.label(dim(about));
                    ui.horizontal(|ui| {
                        let mut shown = url.clone();
                        // The link takes what the Copy button leaves.
                        let copy = format!("{}  Copy", ph::COPY);
                        let button = ui.fonts_mut(|f| f.layout_no_wrap(copy.clone(), egui::TextStyle::Button.resolve(ui.style()), Color32::WHITE)).size().x + 2.0 * ui.spacing().button_padding.x;
                        let field = (ui.available_width() - button - ui.spacing().item_spacing.x - 8.0).max(80.0);
                        ui.add(egui::TextEdit::singleline(&mut shown).desired_width(field).font(egui::TextStyle::Monospace));
                        if ui.button(copy).clicked() {
                            ui.ctx().copy_text(url);
                            act = Some("copied");
                        }
                    });
                    ui.add_space(6.0);
                };
                let gone = self.share.gone;
                if let Some(url) = link.edit_url(&page).filter(|_| !gone) {
                    copy_row(ui, "Edit link", "Opens this project for editing. Their saves come back here, and yours go to them.", url);
                }
                if !gone {
                    copy_row(ui, "View link", "Opens a copy that follows your changes. Changes made with it stay with whoever made them.", link.view_url(&page));
                }
                if !link.can_edit() && !gone {
                    ui.label(egui::RichText::new(format!("{}  You opened a view link: your changes stay on this device.", ph::EYE)).color(theme::text_dim()));
                }
                ui.separator();
                if let Some(e) = &self.share.error {
                    ui.label(egui::RichText::new(e).color(theme::danger()));
                }
                match &self.share.remote {
                    Some(r) => {
                        let pending = self.share.modified > link.synced && link.can_edit();
                        let state = if r.version > link.version {
                            "Someone else's newer version hasn't been loaded here."
                        } else if pending {
                            "Changes here are waiting to go up."
                        } else {
                            "Up to date."
                        };
                        ui.label(dim(&format!("Shared version {}, saved {}. {state}", r.version, crate::projects::format_time(iso_ms(&r.updated)))));
                        ui.label(dim(&format!("Expires {} unless someone uses it before then.", crate::projects::format_date(iso_ms(&r.expires)))));
                        let earlier: Vec<&RemoteVersion> = r.versions.iter().filter(|v| v.version != r.version).collect();
                        if !earlier.is_empty() {
                            egui::CollapsingHeader::new(format!("Earlier versions online ({})", earlier.len())).show(ui, |ui| {
                                ui.label(dim(&format!("The last {SHARE_VERSIONS} versions are kept, so an overwrite can be undone.")));
                                for v in earlier {
                                    ui.horizontal(|ui| {
                                        ui.label(format!("Version {} · {}", v.version, crate::projects::format_time(iso_ms(&v.updated))));
                                        if ui.add_enabled(!self.share.busy, egui::Button::new("Restore")).on_hover_text("Bring this version back. What is here now is kept in Version history.").clicked() {
                                            restore = Some(v.version);
                                        }
                                    });
                                }
                            });
                        }
                    }
                    None if !self.share.gone => {
                        ui.label(dim("Checking the shared copy..."));
                    }
                    None => {}
                }
                ui.add_space(6.0);
                let (label, tip) = if link.is_owner() && !self.share.gone {
                    ("Stop sharing", "Delete the shared copy online: the links stop working. The project stays here.")
                } else {
                    ("Unlink this project", "Stop following the share link. The project stays here as it is.")
                };
                let label = if self.share.confirm_stop { format!("{}  Click again to confirm", ph::WARNING) } else { format!("{}  {label}", ph::LINK_BREAK) };
                if ui.add_enabled(!self.share.busy, egui::Button::new(label)).on_hover_text(tip).clicked() {
                    act = Some(if self.share.confirm_stop { "stop" } else { "confirm" });
                }
            });
        });
        if modal.should_close() {
            open = false;
        }
        match act {
            Some("create") => self.create_share(),
            Some("copied") => self.toast("Link copied"),
            Some("confirm") => self.share.confirm_stop = true,
            Some("stop") => {
                self.share.confirm_stop = false;
                self.stop_share();
            }
            _ => {}
        }
        if let Some(v) = restore {
            self.fetch_share_version(v, Fetch::Restore);
        }
        self.share.show &= open;
        if !self.share.show {
            self.share.confirm_stop = false;
            crate::app::end_dialog_zoom(ctx, zoom_id);
        }
    }

    /// Someone else's changes, or a clash with them: a bar under the top bar.
    pub fn share_banner(&mut self, ctx: &egui::Context) {
        let s = &self.share;
        let Some(version) = s.incoming.filter(|v| s.conflict || *v != s.dismissed) else { return };
        let edited_here = s.link.as_ref().is_some_and(|l| s.modified > l.synced) || self.dirty;
        let (text, load, keep) = if s.conflict || edited_here {
            ("Someone else changed this shared project while you were editing it.", "Use theirs", Some("Keep mine"))
        } else {
            ("Someone else changed this shared project.", "Load their changes", None)
        };
        let can_keep = s.link.as_ref().is_some_and(|l| l.can_edit());
        let busy = s.busy;
        let mut choice = None;
        egui::Area::new(egui::Id::new("share_banner")).anchor(egui::Align2::CENTER_TOP, Vec2::new(0.0, 52.0)).order(egui::Order::Foreground).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
                ui.set_max_width(520.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!("{}  {text}", ph::USERS));
                    if ui.add_enabled(!busy, egui::Button::new(egui::RichText::new(load).color(Color32::WHITE)).fill(theme::accent())).on_hover_text("What is here now is kept in Version history").clicked() {
                        choice = Some("load");
                    }
                    if let Some(k) = keep.filter(|_| can_keep) {
                        if ui.add_enabled(!busy, egui::Button::new(k)).on_hover_text("Replace their version online with yours. Theirs is kept among the earlier versions (Share)").clicked() {
                            choice = Some("keep");
                        }
                    } else if ui.button("Later").clicked() {
                        choice = Some("later");
                    }
                });
            });
        });
        match choice {
            Some("load") => self.fetch_share_version(version, Fetch::Load),
            Some("keep") => {
                // Yours goes up next, made from the newest version, so the Worker takes it.
                if let (Some(id), Some(mut link)) = (self.share.project.clone(), self.share.link.clone()) {
                    link.version = version;
                    let _ = self.store.set_share(&id, Some(link.clone()));
                    let s = &mut self.share;
                    if let Some(r) = &mut s.remote {
                        r.version = version;
                    }
                    (s.link, s.incoming, s.conflict, s.last_upload) = (Some(link), None, false, 0.0);
                    self.save_current(Snapshot::IfChanged, "Edited");
                    self.share.reread();
                    self.toast("Your version will replace theirs online. Theirs is kept among the earlier versions in Share.");
                }
            }
            Some("later") => self.share.dismissed = version,
            _ => {}
        }
    }

    /// The "Open a share link" window (from the home screen).
    pub fn open_link_window(&mut self, ctx: &egui::Context) {
        if !self.share.show_open {
            return;
        }
        let mut open = true;
        let mut go = false;
        egui::Window::new(format!("{}  Open a share link", ph::LINK)).open(&mut open).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
            ui.label("Paste a link someone shared with you:");
            let r = ui.add(egui::TextEdit::singleline(&mut self.share.open_text).desired_width(360.0).hint_text("https://app.biocartoon.com/#share=..."));
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let link = ShareLink::parse(&self.share.open_text);
            if !self.share.open_text.trim().is_empty() && link.is_none() {
                ui.label(egui::RichText::new("That isn't a whole BioCartoon share link.").color(theme::danger()));
            }
            if ui.add_enabled(link.is_some(), egui::Button::new(egui::RichText::new("Open").color(Color32::WHITE)).fill(theme::accent())).clicked() || (enter && link.is_some()) {
                go = true;
            }
        });
        if go {
            self.share.pending_open = ShareLink::parse(&self.share.open_text);
            self.share.open_text.clear();
            open = false;
        }
        self.share.show_open = open;
    }
}

/// A request to the Worker. Runs in the background; `done` gets the reply, or a status-0 reply
/// whose body says why the server couldn't be reached.
fn request(method: &str, url: String, headers: Vec<(&'static str, String)>, body: Vec<u8>, done: impl FnOnce(Result<Reply, Reply>) + Send + 'static) {
    let unreachable = |e: String| Reply { status: 0, body: format!("couldn't reach the server ({e})").into_bytes(), version: None };
    let finish = |status: u16, body: Vec<u8>, version: Option<i64>| {
        let r = Reply { status, body, version };
        if (200..300).contains(&status) {
            Ok(r)
        } else {
            Err(r)
        }
    };
    #[cfg(target_arch = "wasm32")]
    {
        let mut r = ehttp::Request::get(url);
        r.method = method.to_string();
        r.body = body;
        for (k, v) in headers {
            r.headers.insert(k, v);
        }
        ehttp::fetch(r, move |res| {
            done(match res {
                Ok(r) => finish(r.status, r.bytes.clone(), r.headers.get("x-share-version").and_then(|v| v.parse().ok())),
                Err(e) => Err(unreachable(e)),
            })
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let method = method.to_string();
        std::thread::spawn(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(180))).http_status_as_error(false).build().into();
            let res = match method.as_str() {
                "POST" | "PUT" => {
                    let mut req = if method == "POST" { agent.post(&url) } else { agent.put(&url) };
                    for (k, v) in &headers {
                        req = req.header(*k, v);
                    }
                    req.send(&body[..])
                }
                _ => {
                    let mut req = if method == "DELETE" { agent.delete(&url) } else { agent.get(&url) };
                    for (k, v) in &headers {
                        req = req.header(*k, v);
                    }
                    req.call()
                }
            };
            done(match res {
                Ok(mut r) => {
                    let status = r.status().as_u16();
                    let version = r.headers().get("x-share-version").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok());
                    match r.body_mut().with_config().limit(MAX_SHARE_BYTES as u64 + 1024).read_to_vec() {
                        Ok(b) => finish(status, b, version),
                        Err(e) => Err(unreachable(e.to_string())),
                    }
                }
                Err(e) => Err(unreachable(e.to_string())),
            })
        });
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use super::ShareLink;
    use wasm_bindgen::JsValue;

    /// This page's address without its fragment: where links open.
    pub fn here() -> Option<String> {
        let loc = web_sys::window()?.location();
        Some(format!("{}{}{}", loc.origin().ok()?, loc.pathname().ok()?, loc.search().ok()?))
    }

    /// A share link in the page's address, removed from the address bar and history so its
    /// secrets aren't left there.
    pub fn take_fragment() -> Option<ShareLink> {
        let w = web_sys::window()?;
        let hash = w.location().hash().ok()?;
        if !hash.starts_with("#share=") {
            return None;
        }
        if let (Ok(h), Some(page)) = (w.history(), here()) {
            let _ = h.replace_state_with_url(&JsValue::NULL, "", Some(&page));
        }
        ShareLink::parse(&hash)
    }

    /// The same, for a link pasted into the address bar of the open app (no reload then).
    pub fn poll_fragment() -> Option<ShareLink> {
        take_fragment()
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod web {
    use super::ShareLink;
    pub fn here() -> Option<String> {
        None
    }
    pub fn take_fragment() -> Option<ShareLink> {
        None
    }
    pub fn poll_fragment() -> Option<ShareLink> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link() -> ShareLink {
        ShareLink { id: B64.encode(random(16)), key: B64.encode(random(32)), edit: B64.encode(random(16)), owner: B64.encode(random(16)), version: 3, synced: 9 }
    }

    #[test]
    fn links_round_trip() {
        let l = link();
        let edit = ShareLink::parse(&l.edit_url(WEB_APP).unwrap()).unwrap();
        assert_eq!((edit.id.as_str(), edit.key.as_str(), edit.edit.as_str()), (l.id.as_str(), l.key.as_str(), l.edit.as_str()));
        assert!(edit.owner.is_empty(), "the owner secret is never in a link");
        let view = ShareLink::parse(&l.view_url(WEB_APP)).unwrap();
        assert!(!view.can_edit());
        assert_eq!(view.key, l.key);
        // Pasted with text around it, or as just the fragment.
        assert!(ShareLink::parse(&format!("see {} thanks", l.view_url(WEB_APP))).is_some());
        assert!(ShareLink::parse(&format!("#share={}.{}", l.id, l.key)).is_some());
        // Cut short, or with junk: refused rather than half-read.
        let url = l.view_url(WEB_APP);
        assert!(ShareLink::parse(&url[..url.len() - 3]).is_none());
        assert!(ShareLink::parse(&format!("{url}.x.y")).is_none());
        assert!(ShareLink::parse("https://app.biocartoon.com/").is_none());
        // Only the edit link edits; nobody else can stop sharing.
        assert!(l.edit_url(WEB_APP).is_some() && view.edit_url(WEB_APP).is_none());
    }

    #[test]
    fn sealed_projects_open_only_with_their_key() {
        let mut doc = Document::default();
        doc.title = "Shared cell".into();
        let l = link();
        let sealed = seal(&doc, &l.key).unwrap();
        assert!(sealed.starts_with(MAGIC));
        assert!(!String::from_utf8_lossy(&sealed).contains("Shared cell"), "the title isn't readable online");
        let file = unseal(&sealed, &l.key).unwrap();
        let back = crate::projects::from_file(&file, std::path::Path::new("/nonexistent")).unwrap();
        assert_eq!(back.title, "Shared cell");
        assert!(unseal(&sealed, &link().key).is_err(), "another key");
        let mut tampered = sealed.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(unseal(&tampered, &l.key).is_err(), "changed in transit");
        // Two seals of the same project differ (a fresh nonce each time).
        assert_ne!(seal(&doc, &l.key).unwrap(), sealed);
    }

    #[test]
    fn worker_times() {
        assert_eq!(iso_ms("2026-01-01T00:00:00Z"), 1_767_225_600_000);
        assert_eq!(iso_ms("junk"), 0);
    }
}

/// The desktop requests against a running Worker: `BIOCARTOON_TEST_WORKER=http://localhost:8787
/// cargo test --lib share::live -- --ignored` (with `npm run dev` in the biocartoon repo's worker/).
#[cfg(all(test, not(target_arch = "wasm32")))]
mod live {
    use super::*;

    fn call(method: &str, url: String, headers: Vec<(&'static str, String)>, body: Vec<u8>) -> Result<Reply, Reply> {
        let (tx, rx) = channel();
        request(method, url, headers, body, move |r| tx.send(r).unwrap());
        rx.recv().unwrap()
    }

    #[test]
    #[ignore]
    fn share_round_trip() {
        let base = std::env::var("BIOCARTOON_TEST_WORKER").expect("BIOCARTOON_TEST_WORKER");
        let secret = |n| B64.encode(random(n));
        let link = ShareLink { key: secret(32), edit: secret(16), owner: secret(16), ..Default::default() };
        let mut doc = Document::default();
        doc.title = "Live".into();
        let made = call("POST", format!("{base}/shares"), vec![("x-share-edit", link.edit.clone()), ("x-share-owner", link.owner.clone())], seal(&doc, &link.key).unwrap()).unwrap();
        let id = made.json()["id"].as_str().unwrap().to_string();
        doc.title = "Live 2".into();
        let put = |base_version: i64, doc: &Document| call("PUT", format!("{base}/shares/{id}"), vec![("x-share-edit", link.edit.clone()), ("x-share-base", base_version.to_string())], seal(doc, &link.key).unwrap());
        assert_eq!(put(1, &doc).unwrap().json()["version"], 2);
        assert_eq!(put(1, &doc).unwrap_err().status, 409, "made from an old version");
        let got = call("GET", format!("{base}/shares/{id}/latest"), Vec::new(), Vec::new()).unwrap();
        assert_eq!(got.version, Some(2));
        let file = unseal(&got.body, &link.key).unwrap();
        assert_eq!(crate::projects::from_file(&file, std::path::Path::new("/nonexistent")).unwrap().title, "Live 2");
        let info: Remote = serde_json::from_slice(&call("GET", format!("{base}/shares/{id}"), Vec::new(), Vec::new()).unwrap().body).unwrap();
        assert_eq!((info.version, info.versions.len()), (2, 2));
        assert_eq!(call("DELETE", format!("{base}/shares/{id}"), vec![("x-share-owner", link.edit.clone())], Vec::new()).unwrap_err().status, 403);
        call("DELETE", format!("{base}/shares/{id}"), vec![("x-share-owner", link.owner.clone())], Vec::new()).unwrap();
        assert_eq!(call("GET", format!("{base}/shares/{id}"), Vec::new(), Vec::new()).unwrap_err().status, 404);
    }
}
