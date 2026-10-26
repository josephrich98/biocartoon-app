//! Optional accounts (web app). Signing in with Google or GitHub keeps an online copy of every
//! project, and of Your library (see [`crate::library`]), on the submissions Worker's account routes (the biocartoon repo's
//! `worker/src/accounts.ts`), so work outlives the browser's storage. Nobody has to: guests keep
//! everything in [`crate::storage`] as before, and for a signed-in user the browser copy stays the
//! working one, mirrored online one job at a time:
//!
//! - on each start, the online list is compared with the local projects: projects missing here (a
//!   browser that lost its data) or newer online come down, and ones newer here go up;
//! - after that, an edited project goes up once it has been left alone for [`UPLOAD_QUIET_MS`]
//!   (or every [`UPLOAD_EVERY_MS`] while editing goes on);
//! - deleting a project here deletes its online copy, retried until the Worker confirms.
//!
//! Your library's items (own icons, saved templates, favorites) follow the same rules once the
//! projects are done, except that favorites never synced from this browser are merged with the
//! ones online rather than put over them.
//!
//! Signing in leaves the app (the browser goes to the provider and back), and the Worker hands the
//! session token back in the URL fragment, which [`Account::new`] reads and clears.

use crate::app::BioCartoon;
use crate::library::{Item, FAVORITES};
use crate::projects::ProjectMeta;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};

/// An edited project goes up once it has gone this long without changes...
pub const UPLOAD_QUIET_MS: i64 = 20_000;
/// ...or this long after its last upload, while editing continues.
pub const UPLOAD_EVERY_MS: i64 = 5 * 60_000;
/// After a failed request, wait this long before the next.
const RETRY_MS: i64 = 60_000;

/// What is kept in `account.json` while signed in.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Saved {
    pub token: String,
    pub provider: String,
    pub email: String,
    pub name: String,
    /// Project id -> the `modified` time of the copy online.
    pub uploaded: BTreeMap<String, i64>,
    /// Projects deleted here whose online copy is still to be deleted.
    pub deleting: Vec<String>,
    /// The same for Your library's items.
    pub lib_uploaded: BTreeMap<String, i64>,
    pub lib_deleting: Vec<String>,
}

/// A project online, or (with no name) an item of Your library.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct CloudProject {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub modified: i64,
    pub bytes: i64,
}

/// The next request to make.
#[derive(Debug, PartialEq)]
pub enum Job {
    Delete(String),
    Download(CloudProject),
    Upload(String),
}

/// Pick the next job, given what is here and what is online (`uploaded` and `deleting` are
/// [`Saved`]'s, for projects or for library items). `open` is the project in the editor, which
/// is never replaced under the user; `skip` maps projects whose last upload was refused (too
/// large, account full) to the `modified` time that was refused.
#[allow(clippy::too_many_arguments)]
pub fn next_job(local: &[ProjectMeta], cloud: &[CloudProject], uploaded: &BTreeMap<String, i64>, deleting: &[String], open: Option<&str>, skip: &HashMap<String, i64>, last_upload: &HashMap<String, i64>, now: i64) -> Option<Job> {
    if let Some(id) = deleting.first() {
        return Some(Job::Delete(id.clone()));
    }
    for c in cloud {
        let here = local.iter().find(|p| p.id == c.id);
        let up = uploaded.get(&c.id).copied();
        match here {
            None => return Some(Job::Download(c.clone())),
            // Newer online (edited in another browser) and not edited here since: bring it down.
            Some(p) if c.modified > p.modified && p.modified <= up.unwrap_or(0) && open != Some(&c.id) => return Some(Job::Download(c.clone())),
            _ => {}
        }
    }
    local
        .iter()
        .filter(|p| skip.get(&p.id) != Some(&p.modified))
        .filter(|p| {
            let online = uploaded.get(&p.id).copied().or_else(|| cloud.iter().find(|c| c.id == p.id).map(|c| c.modified));
            online.is_none_or(|t| p.modified > t)
        })
        .find(|p| now - p.modified >= UPLOAD_QUIET_MS || now - last_upload.get(&p.id).copied().unwrap_or(0) >= UPLOAD_EVERY_MS)
        .map(|p| Job::Upload(p.id.clone()))
}

/// [`next_job`] for Your library. Favorites never synced from this browser come down first, to be
/// merged with these.
pub fn next_library_job(local: &[ProjectMeta], cloud: &[CloudProject], saved: &Saved, skip: &HashMap<String, i64>, last_upload: &HashMap<String, i64>, now: i64) -> Option<Job> {
    if saved.lib_deleting.is_empty() && !saved.lib_uploaded.contains_key(FAVORITES) {
        if let Some(c) = cloud.iter().find(|c| c.id == FAVORITES) {
            return Some(Job::Download(c.clone()));
        }
    }
    next_job(local, cloud, &saved.lib_uploaded, &saved.lib_deleting, None, skip, last_upload, now)
}

enum Msg {
    Me(Result<serde_json::Value, Reply>),
    List(Result<Vec<CloudProject>, Reply>),
    Uploaded(String, i64, Result<serde_json::Value, Reply>),
    Downloaded(CloudProject, Result<Vec<u8>, Reply>),
    Deleted(String, Result<(), Reply>),
    LibList(Result<Vec<CloudProject>, Reply>),
    LibUploaded(String, i64, Result<serde_json::Value, Reply>),
    LibDownloaded(CloudProject, Result<Vec<u8>, Reply>),
    LibDeleted(String, Result<(), Reply>),
    SignedOut,
    AccountDeleted(Result<(), Reply>),
}

/// A failed request: the Worker's message, and whether the session is no longer valid.
#[derive(Debug)]
pub struct Reply {
    pub message: String,
    pub status: u16,
}

pub struct Account {
    path: PathBuf,
    /// The Worker's address (from Settings, set by [`BioCartoon::poll_account`]).
    base: String,
    /// The profile (name, email, space used) is still to be fetched.
    need_me: bool,
    /// `None` for a guest.
    pub saved: Option<Saved>,
    /// The online list, once fetched this run (kept current as jobs finish).
    pub cloud: Option<Vec<CloudProject>>,
    pub used: i64,
    pub quota: i64,
    /// The last problem, shown in the account menu until a request succeeds.
    pub error: Option<String>,
    /// A message for a toast (sign-in finished or failed), taken by the app.
    pub notice: Option<(String, bool)>,
    busy: bool,
    retry_at: i64,
    /// The local project list, re-read every few seconds (the home screen's copy goes stale in the editor).
    local: Vec<ProjectMeta>,
    local_at: i64,
    skip: HashMap<String, i64>,
    last_upload: HashMap<String, i64>,
    /// The same for Your library: the online list, what is here (as of `local_at`), refused and
    /// last uploads.
    pub lib_cloud: Option<Vec<CloudProject>>,
    lib_local: Vec<ProjectMeta>,
    lib_skip: HashMap<String, i64>,
    lib_last_upload: HashMap<String, i64>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    ctx: egui::Context,
}

impl Account {
    /// Load the saved session from `path`, then take a token (or an error) the Worker put in the
    /// page's URL after a sign-in.
    pub fn new(path: PathBuf, ctx: egui::Context) -> Self {
        let saved = crate::storage::read(&path).ok().and_then(|b| serde_json::from_slice::<Saved>(&b).ok()).filter(|s| !s.token.is_empty());
        let (tx, rx) = channel();
        let mut a = Self { path, base: String::new(), need_me: false, saved, cloud: None, used: 0, quota: 0, error: None, notice: None, busy: false, retry_at: 0, local: Vec::new(), local_at: 0, skip: HashMap::new(), last_upload: HashMap::new(), lib_cloud: None, lib_local: Vec::new(), lib_skip: HashMap::new(), lib_last_upload: HashMap::new(), tx, rx, ctx };
        match web::take_fragment() {
            Some(Ok(token)) => {
                // A new sign-in replaces any old one, and starts its bookkeeping afresh.
                a.saved = Some(Saved { token, ..Default::default() });
                a.save();
                a.notice = Some(("Signed in. Your projects and library are being saved online.".into(), false));
            }
            Some(Err(e)) => a.notice = Some((e, true)),
            None => {}
        }
        a.need_me = a.saved.is_some();
        a
    }

    pub fn signed_in(&self) -> bool {
        self.saved.is_some()
    }

    fn token(&self) -> String {
        self.saved.as_ref().map(|s| s.token.clone()).unwrap_or_default()
    }

    fn save(&self) {
        let r = match &self.saved {
            Some(s) => serde_json::to_vec_pretty(s).map_err(|e| e.to_string()).and_then(|b| crate::storage::write(&self.path, &b).map_err(|e| e.to_string())),
            None => crate::storage::write(&self.path, b"{}").map_err(|e| e.to_string()),
        };
        if let Err(e) = r {
            log::error!("account: {e}");
        }
    }

    fn send(&self) -> impl Fn(Msg) + Send + 'static {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        move |m| {
            let _ = tx.send(m);
            ctx.request_repaint();
        }
    }

    /// After a failed request: a rejected session signs out (projects stay here); anything else
    /// is retried a little later.
    fn failed(&mut self, e: Reply) {
        if e.status == 401 {
            (self.saved, self.cloud, self.lib_cloud) = (None, None, None);
            self.save();
            self.notice = Some((format!("{} Your projects are still in this browser.", e.message), true));
        } else {
            self.error = Some(e.message);
            self.retry_at = crate::projects::now_ms() + RETRY_MS;
        }
    }

    /// Projects and library items changed here since their online copy (for the account menu).
    pub fn pending(&self, local: &[ProjectMeta]) -> usize {
        let Some(s) = &self.saved else { return 0 };
        let changed = |local: &[ProjectMeta], up: &BTreeMap<String, i64>| local.iter().filter(|p| up.get(&p.id).is_none_or(|t| p.modified > *t)).count();
        changed(local, &s.uploaded) + s.deleting.len() + changed(&self.lib_local, &s.lib_uploaded) + s.lib_deleting.len()
    }
}

fn parse_json(b: Vec<u8>) -> Result<serde_json::Value, Reply> {
    serde_json::from_slice(&b).map_err(|_| Reply { message: "The server sent an unexpected reply".into(), status: 0 })
}

fn api(base: &str, path: &str) -> String {
    format!("{}{path}", base.trim_end_matches('/'))
}

/// Percent-encode a header value or query parameter.
pub fn encode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

impl BioCartoon {
    /// Run the sync: apply finished requests, then start the next job. Called every frame;
    /// cheap when there is nothing to do.
    pub fn poll_account(&mut self) {
        let Some(base) = self.submit_url() else { return };
        self.account.base = base.clone();
        if let Some((text, error)) = self.account.notice.take() {
            if error {
                self.toast_err(text);
            } else {
                self.toast(text);
            }
        }
        while let Ok(m) = self.account.rx.try_recv() {
            self.account_msg(m);
        }
        let now = crate::projects::now_ms();
        if self.account.saved.is_some() && now - self.account.local_at >= 5000 {
            self.account.lib_local = self.library_entries(now);
            (self.account.local, self.account.local_at) = (self.store.list(), now);
        }
        let a = &mut self.account;
        if a.busy || a.saved.is_none() || now < a.retry_at {
            return;
        }
        let token = a.token();
        let send = a.send();
        if a.need_me {
            a.need_me = false;
            request(api(&base, "/me"), "GET", &token, Vec::new(), &[], move |r| send(Msg::Me(r.and_then(parse_json))));
            return;
        }
        let Some(cloud) = &a.cloud else {
            a.busy = true;
            request(api(&base, "/cloud/projects"), "GET", &token, Vec::new(), &[], move |r| {
                let list = r.and_then(parse_json).and_then(|v| {
                    serde_json::from_value::<Vec<CloudProject>>(v["projects"].clone()).map_err(|_| Reply { message: "The server sent an unexpected list".into(), status: 0 })
                });
                send(Msg::List(list));
            });
            return;
        };
        let local = &a.local;
        let saved = a.saved.as_ref().unwrap();
        let job = next_job(local, cloud, &saved.uploaded, &saved.deleting, self.project.as_deref(), &a.skip, &a.last_upload, now);
        let Some(job) = job else { return self.poll_library_sync(&base, now) };
        a.busy = true;
        match job {
            Job::Delete(id) => {
                request(api(&base, &format!("/cloud/projects/{id}")), "DELETE", &token, Vec::new(), &[], move |r| send(Msg::Deleted(id, r.map(|_| ()))));
            }
            Job::Download(c) => {
                request(api(&base, &format!("/cloud/projects/{}", c.id)), "GET", &token, Vec::new(), &[], move |r| send(Msg::Downloaded(c, r)));
            }
            Job::Upload(id) => {
                let modified = local.iter().find(|p| p.id == id).map(|p| p.modified).unwrap_or(now);
                let file = self.store.load(&id).and_then(|d| crate::projects::to_file(&d).map(|f| (d.title, f)));
                match file {
                    Ok((name, bytes)) => {
                        a.last_upload.insert(id.clone(), now);
                        let headers = [("Content-Type", "application/x-biocartoon".to_string()), ("x-project-name", encode(&name)), ("x-project-modified", modified.to_string())];
                        request(api(&base, &format!("/cloud/projects/{id}")), "PUT", &token, bytes, &headers, move |r| send(Msg::Uploaded(id, modified, r.and_then(parse_json))));
                    }
                    Err(e) => {
                        a.busy = false;
                        a.skip.insert(id, modified);
                        a.error = Some(format!("Couldn't read a project to save it online: {e}"));
                    }
                }
            }
        }
    }

    fn account_msg(&mut self, m: Msg) {
        let a = &mut self.account;
        match m {
            Msg::Me(Ok(v)) => {
                let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
                if let Some(saved) = &mut a.saved {
                    (saved.provider, saved.email, saved.name) = (s("provider"), s("email"), s("name"));
                }
                (a.used, a.quota) = (v["used"].as_i64().unwrap_or(0), v["quota"].as_i64().unwrap_or(0));
                a.save();
            }
            Msg::List(Ok(list)) => {
                a.busy = false;
                // Forget uploads the server no longer has (deleted elsewhere, or lost): they go up again.
                if let Some(s) = &mut a.saved {
                    s.uploaded.retain(|id, _| list.iter().any(|c| &c.id == id));
                }
                a.cloud = Some(list);
                a.error = None;
            }
            Msg::Uploaded(id, modified, Ok(v)) => {
                a.busy = false;
                (a.used, a.quota) = (v["used"].as_i64().unwrap_or(a.used), v["quota"].as_i64().unwrap_or(a.quota));
                if let Some(s) = &mut a.saved {
                    s.uploaded.insert(id.clone(), modified);
                }
                if let Some(cloud) = &mut a.cloud {
                    let bytes = v["bytes"].as_i64().unwrap_or(0);
                    match cloud.iter_mut().find(|c| c.id == id) {
                        Some(c) => (c.modified, c.bytes) = (modified, bytes),
                        None => cloud.push(CloudProject { id, name: String::new(), modified, bytes }),
                    }
                }
                a.error = None;
                a.save();
            }
            // Too large, or the account is full: don't retry until the project changes.
            Msg::Uploaded(id, modified, Err(e)) if e.status == 413 => {
                a.busy = false;
                a.skip.insert(id, modified);
                a.error = Some(e.message);
            }
            Msg::Downloaded(c, Ok(bytes)) => {
                a.busy = false;
                let uploads = self.app_dir.join("uploads");
                match crate::projects::from_file(&bytes, &uploads).and_then(|d| self.store.put_as(&c.id, &d, c.modified).map(|_| d)) {
                    Ok(doc) => {
                        if let Some(s) = &mut self.account.saved {
                            s.uploaded.insert(c.id.clone(), c.modified);
                        }
                        self.account.local_at = 0; // re-read the list, so it isn't downloaded again
                        self.account.save();
                        self.store.mark_backed_up(&c.id);
                        self.request_thumb(&c.id, &doc);
                        self.refresh_projects();
                    }
                    Err(e) => {
                        // Leave it online; drop it from this run's list so it isn't fetched again and again.
                        if let Some(cloud) = &mut self.account.cloud {
                            cloud.retain(|x| x.id != c.id);
                        }
                        self.account.error = Some(format!("Couldn't restore \"{}\" from your account: {e}", c.name));
                    }
                }
            }
            // Already gone online counts as done.
            Msg::Deleted(id, Ok(()) | Err(Reply { status: 404, .. })) => {
                a.busy = false;
                if let Some(s) = &mut a.saved {
                    s.deleting.retain(|x| x != &id);
                    s.uploaded.remove(&id);
                }
                if let Some(cloud) = &mut a.cloud {
                    cloud.retain(|c| c.id != id);
                }
                a.save();
            }
            Msg::LibList(Ok(list)) => {
                a.busy = false;
                if let Some(s) = &mut a.saved {
                    s.lib_uploaded.retain(|id, _| list.iter().any(|c| &c.id == id));
                }
                a.lib_cloud = Some(list);
                a.error = None;
            }
            Msg::LibUploaded(id, modified, Ok(v)) => {
                a.busy = false;
                (a.used, a.quota) = (v["used"].as_i64().unwrap_or(a.used), v["quota"].as_i64().unwrap_or(a.quota));
                if let Some(s) = &mut a.saved {
                    s.lib_uploaded.insert(id.clone(), modified);
                }
                if let Some(cloud) = &mut a.lib_cloud {
                    let bytes = v["bytes"].as_i64().unwrap_or(0);
                    match cloud.iter_mut().find(|c| c.id == id) {
                        Some(c) => (c.modified, c.bytes) = (modified, bytes),
                        None => cloud.push(CloudProject { id, name: String::new(), modified, bytes }),
                    }
                }
                a.error = None;
                a.save();
            }
            Msg::LibUploaded(id, modified, Err(e)) if e.status == 413 => {
                a.busy = false;
                a.lib_skip.insert(id, modified);
                a.error = Some(e.message);
            }
            Msg::LibDownloaded(c, Ok(bytes)) => {
                a.busy = false;
                match self.restore_library_item(&c, &bytes) {
                    Ok(()) => {
                        if let Some(s) = &mut self.account.saved {
                            s.lib_uploaded.insert(c.id.clone(), c.modified);
                        }
                        self.account.local_at = 0;
                        self.account.save();
                    }
                    Err(e) => {
                        if let Some(cloud) = &mut self.account.lib_cloud {
                            cloud.retain(|x| x.id != c.id);
                        }
                        self.account.error = Some(format!("Couldn't restore an item of your library from your account: {e}"));
                    }
                }
            }
            Msg::LibDeleted(id, Ok(()) | Err(Reply { status: 404, .. })) => {
                a.busy = false;
                if let Some(s) = &mut a.saved {
                    s.lib_deleting.retain(|x| x != &id);
                    s.lib_uploaded.remove(&id);
                }
                if let Some(cloud) = &mut a.lib_cloud {
                    cloud.retain(|c| c.id != id);
                }
                a.save();
            }
            Msg::Me(Err(e)) => a.failed(e),
            Msg::LibList(Err(e)) | Msg::LibUploaded(_, _, Err(e)) | Msg::LibDownloaded(_, Err(e)) | Msg::LibDeleted(_, Err(e)) => {
                a.busy = false;
                a.failed(e);
            }
            Msg::List(Err(e)) | Msg::Uploaded(_, _, Err(e)) | Msg::Downloaded(_, Err(e)) | Msg::Deleted(_, Err(e)) => {
                a.busy = false;
                a.failed(e);
            }
            Msg::SignedOut => {}
            Msg::AccountDeleted(Ok(())) => self.toast("Your account and its online copies were deleted. Your projects and library are still in this browser."),
            Msg::AccountDeleted(Err(e)) => self.toast_err(format!("Couldn't delete your account: {}", e.message)),
        }
    }

    /// Leave for the provider's sign-in page. The open project is saved first.
    pub fn account_sign_in(&mut self, provider: &str) {
        let Some(base) = self.submit_url() else { return };
        if self.screen == crate::app::Screen::Editor {
            self.save_current(crate::projects::Snapshot::IfChanged, "Edited");
        }
        match web::here() {
            Some(page) => web::go(&format!("{}/auth/{provider}/start?return={}", base, encode(&page))),
            None => self.toast_err("Signing in is only available in the web app"),
        }
    }

    /// Delete a project's online copy too, now or once back online.
    pub fn account_forget(&mut self, id: &str) {
        if let Some(s) = &mut self.account.saved {
            if s.uploaded.contains_key(id) || self.account.cloud.as_ref().is_some_and(|c| c.iter().any(|c| c.id == id)) {
                s.deleting.push(id.to_string());
            }
            s.uploaded.remove(id);
            self.account.save();
        }
    }

    /// Delete a library item's online copy too (see [`crate::library::item_id`]).
    pub fn account_forget_item(&mut self, id: &str) {
        if let Some(s) = &mut self.account.saved {
            if s.lib_uploaded.contains_key(id) || self.account.lib_cloud.as_ref().is_some_and(|c| c.iter().any(|c| c.id == id)) {
                s.lib_deleting.push(id.to_string());
            }
            s.lib_uploaded.remove(id);
            self.account.save();
            self.account.local_at = 0;
        }
    }

    /// Your library's items here, by online id, with when each last changed. Notes a change to the
    /// favorites first.
    fn library_entries(&mut self, now: i64) -> Vec<ProjectMeta> {
        use crate::library::{item_id, template_modified};
        self.library.note_favorites(&self.settings.favorites, now);
        let entry = |id: String, modified: i64| ProjectMeta { id, modified, ..Default::default() };
        let mut out: Vec<ProjectMeta> = self.library.m.assets.iter().map(|a| entry(item_id("asset", &a.id), a.modified)).collect();
        out.extend(self.templates.list().iter().filter(|t| t.json_local.is_some()).map(|t| entry(item_id("template", &t.meta.id), template_modified(&t.meta))));
        if self.library.m.favorites_modified > 0 {
            out.push(entry(FAVORITES.into(), self.library.m.favorites_modified));
        }
        out
    }

    /// The next library job, once the projects have none.
    fn poll_library_sync(&mut self, base: &str, now: i64) {
        let a = &mut self.account;
        let token = a.token();
        let send = a.send();
        let Some(cloud) = &a.lib_cloud else {
            a.busy = true;
            request(api(base, "/cloud/library"), "GET", &token, Vec::new(), &[], move |r| {
                let list = r.and_then(parse_json).and_then(|v| {
                    serde_json::from_value::<Vec<CloudProject>>(v["items"].clone()).map_err(|_| Reply { message: "The server sent an unexpected list".into(), status: 0 })
                });
                send(Msg::LibList(list));
            });
            return;
        };
        let Some(job) = next_library_job(&a.lib_local, cloud, a.saved.as_ref().unwrap(), &a.lib_skip, &a.lib_last_upload, now) else { return };
        a.busy = true;
        match job {
            Job::Delete(id) => request(api(base, &format!("/cloud/library/{id}")), "DELETE", &token, Vec::new(), &[], move |r| send(Msg::LibDeleted(id, r.map(|_| ())))),
            Job::Download(c) => request(api(base, &format!("/cloud/library/{}", c.id)), "GET", &token, Vec::new(), &[], move |r| send(Msg::LibDownloaded(c, r))),
            Job::Upload(id) => {
                let modified = a.lib_local.iter().find(|p| p.id == id).map(|p| p.modified).unwrap_or(now);
                match self.library_item(&id).and_then(|item| serde_json::to_vec(&item).map_err(|e| e.to_string())) {
                    Ok(bytes) => {
                        let a = &mut self.account;
                        a.lib_last_upload.insert(id.clone(), now);
                        let headers = [("Content-Type", "application/json".to_string()), ("x-item-modified", modified.to_string())];
                        request(api(base, &format!("/cloud/library/{id}")), "PUT", &token, bytes, &headers, move |r| send(Msg::LibUploaded(id, modified, r.and_then(parse_json))));
                    }
                    Err(e) => {
                        let a = &mut self.account;
                        a.busy = false;
                        a.lib_skip.insert(id, modified);
                        a.error = Some(format!("Couldn't read an item of your library to save it online: {e}"));
                    }
                }
            }
        }
    }

    /// The library item with this online id, as it is sent.
    fn library_item(&self, id: &str) -> Result<Item, String> {
        use crate::library::item_id;
        if id == FAVORITES {
            return Ok(Item::Favorites { favorites: self.settings.favorites.clone() });
        }
        if let Some(a) = self.library.m.assets.iter().find(|a| item_id("asset", &a.id) == id) {
            return Item::asset(a);
        }
        match self.templates.list().into_iter().find(|t| t.json_local.is_some() && item_id("template", &t.meta.id) == id) {
            Some(t) => Item::template(&t),
            None => Err("it is no longer here".into()),
        }
    }

    /// Put a downloaded library item in place.
    fn restore_library_item(&mut self, c: &CloudProject, bytes: &[u8]) -> Result<(), String> {
        let item: Item = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        match item {
            Item::Asset { asset, png, svg } => {
                let asset = Item::restore_asset(&self.app_dir, asset, &png, svg, c.modified)?;
                let row = asset.to_row();
                self.library.add(&row, c.modified);
                self.catalog.send(crate::catalog::Req::InsertUser { row });
            }
            Item::Template { meta, json_key, dir, file, thumb } => {
                let row = Item::restore_template(&self.app_dir, meta, json_key, &dir, &file, &thumb)?;
                self.templates.insert(&row)?;
                self.template_rows = self.templates.list();
            }
            Item::Favorites { favorites } => {
                // Never synced from here: keep this browser's too (and send the merged list up).
                let first = self.account.saved.as_ref().is_some_and(|s| !s.lib_uploaded.contains_key(FAVORITES));
                let mut merged = favorites.clone();
                if first {
                    merged.extend(self.settings.favorites.iter().filter(|f| !favorites.contains(f)).cloned());
                }
                self.settings.favorites = merged.clone();
                self.library.m.favorites = favorites;
                self.library.m.favorites_modified = c.modified;
                self.library.note_favorites(&merged, crate::projects::now_ms());
            }
        }
        Ok(())
    }

    /// Sign out here: the session ends on the server, and projects stay in this browser.
    pub fn account_sign_out(&mut self) {
        let Some(base) = self.submit_url() else { return };
        let a = &mut self.account;
        let send = a.send();
        request(api(&base, "/auth/logout"), "POST", &a.token(), Vec::new(), &[], move |_| send(Msg::SignedOut));
        (a.saved, a.cloud, a.lib_cloud, a.error) = (None, None, None, None);
        a.save();
        self.toast("Signed out. Your projects and library are still in this browser.");
    }

    /// Delete the account and every online copy (projects in this browser stay).
    pub fn account_delete(&mut self) {
        let Some(base) = self.submit_url() else { return };
        let a = &mut self.account;
        let send = a.send();
        request(api(&base, "/me"), "DELETE", &a.token(), Vec::new(), &[], move |r| send(Msg::AccountDeleted(r.map(|_| ()))));
        (a.saved, a.cloud, a.lib_cloud, a.error) = (None, None, None, None);
        a.save();
    }
}

/// A request to the Worker with the session token. Error replies carry `{"error": ...}`.
fn request(url: String, method: &str, token: &str, body: Vec<u8>, headers: &[(&str, String)], done: impl FnOnce(Result<Vec<u8>, Reply>) + Send + 'static) {
    #[cfg(target_arch = "wasm32")]
    {
        let mut r = ehttp::Request::get(url);
        r.method = method.to_string();
        r.body = body;
        if !token.is_empty() {
            r.headers.insert("Authorization", format!("Bearer {token}"));
        }
        for (k, v) in headers {
            r.headers.insert(*k, v);
        }
        ehttp::fetch(r, move |res| {
            done(match res {
                Ok(r) if r.ok => Ok(r.bytes),
                Ok(r) => {
                    let v: serde_json::Value = serde_json::from_slice(&r.bytes).unwrap_or_default();
                    Err(Reply { message: v["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("The server answered HTTP {}", r.status)), status: r.status })
                }
                Err(e) => Err(Reply { message: format!("Couldn't reach the server to save your projects online ({e})"), status: 0 }),
            })
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // The desktop app keeps projects on disk and has no sign-in.
        let _ = (url, method, token, body, headers);
        done(Err(Reply { message: "Accounts are only available in the web app".into(), status: 0 }));
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use wasm_bindgen::JsValue;

    /// This page's address without its fragment: where sign-in comes back to.
    pub fn here() -> Option<String> {
        let loc = web_sys::window()?.location();
        Some(format!("{}{}{}", loc.origin().ok()?, loc.pathname().ok()?, loc.search().ok()?))
    }

    /// Go to `url`, a moment from now, so storage writes just made reach IndexedDB first.
    pub fn go(url: &str) {
        let url = url.to_string();
        let f = wasm_bindgen::closure::Closure::once_into_js(move || {
            if let Some(w) = web_sys::window() {
                let _ = w.location().set_href(&url);
            }
        });
        if let Some(w) = web_sys::window() {
            let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(wasm_bindgen::JsCast::unchecked_ref(&f), 300);
        }
    }

    /// The session token (or error) the Worker put in the fragment after sign-in, removing it from
    /// the address bar and history so it can't be copied or bookmarked.
    pub fn take_fragment() -> Option<Result<String, String>> {
        let w = web_sys::window()?;
        let hash = w.location().hash().ok()?;
        let frag = hash.strip_prefix('#')?;
        let (key, value) = frag.split_once('=')?;
        let r = match key {
            "biocartoon_session" if !value.is_empty() => Ok(value.to_string()),
            "biocartoon_error" => Err(js_sys::decode_uri_component(value).ok().and_then(|s| s.as_string()).unwrap_or_else(|| "Sign-in failed".into())),
            _ => return None,
        };
        if let (Ok(h), Some(page)) = (w.history(), here()) {
            let _ = h.replace_state_with_url(&JsValue::NULL, "", Some(&page));
        }
        Some(r)
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod web {
    pub fn here() -> Option<String> {
        None
    }
    pub fn go(_: &str) {}
    pub fn take_fragment() -> Option<Result<String, String>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: &str, modified: i64) -> ProjectMeta {
        ProjectMeta { id: id.into(), modified, ..Default::default() }
    }
    fn cloud(id: &str, modified: i64) -> CloudProject {
        CloudProject { id: id.into(), name: id.into(), modified, bytes: 10 }
    }

    #[test]
    fn sync_decisions() {
        let none = HashMap::new();
        let now = 1_000_000;
        let mut saved = Saved::default();

        // A browser that lost its data gets everything back.
        assert_eq!(next_job(&[], &[cloud("a", 5)], &saved.uploaded, &saved.deleting, None, &none, &none, now), Some(Job::Download(cloud("a", 5))));
        // Projects never uploaded go up; ones online and unchanged don't.
        assert_eq!(next_job(&[meta("a", 5)], &[], &saved.uploaded, &saved.deleting, None, &none, &none, now), Some(Job::Upload("a".into())));
        assert_eq!(next_job(&[meta("a", 5)], &[cloud("a", 5)], &saved.uploaded, &saved.deleting, None, &none, &none, now), None);
        saved.uploaded.insert("a".into(), 5);
        assert_eq!(next_job(&[meta("a", 5)], &[cloud("a", 5)], &saved.uploaded, &saved.deleting, None, &none, &none, now), None);

        // Being edited right now: wait for a pause, unless the last upload is old.
        let editing = meta("a", now - 1000);
        assert_eq!(next_job(std::slice::from_ref(&editing), &[cloud("a", 5)], &saved.uploaded, &saved.deleting, Some("a"), &none, &HashMap::from([("a".to_string(), now - 60_000)]), now), None);
        assert_eq!(next_job(std::slice::from_ref(&editing), &[cloud("a", 5)], &saved.uploaded, &saved.deleting, Some("a"), &none, &HashMap::from([("a".to_string(), now - UPLOAD_EVERY_MS)]), now), Some(Job::Upload("a".into())));

        // Newer online and untouched here: comes down, except into the open editor.
        assert_eq!(next_job(&[meta("a", 5)], &[cloud("a", 9)], &saved.uploaded, &saved.deleting, None, &none, &none, now), Some(Job::Download(cloud("a", 9))));
        assert_eq!(next_job(&[meta("a", 5)], &[cloud("a", 9)], &saved.uploaded, &saved.deleting, Some("a"), &none, &none, now), None);
        // Edited on both sides: this browser's copy wins.
        assert_eq!(next_job(&[meta("a", 7)], &[cloud("a", 9)], &saved.uploaded, &saved.deleting, None, &none, &none, now), Some(Job::Upload("a".into())));

        // A refused upload isn't retried until the project changes again.
        let skip = HashMap::from([("a".to_string(), 7)]);
        assert_eq!(next_job(&[meta("a", 7)], &[cloud("a", 9)], &saved.uploaded, &saved.deleting, None, &skip, &none, now), None);
        assert_eq!(next_job(&[meta("a", 8)], &[], &saved.uploaded, &saved.deleting, None, &skip, &none, now), Some(Job::Upload("a".into())));

        // Deletions go first, and a deleted project isn't downloaded back.
        saved.deleting.push("b".into());
        assert_eq!(next_job(&[], &[cloud("b", 1)], &saved.uploaded, &saved.deleting, None, &none, &none, now), Some(Job::Delete("b".into())));
    }

    #[test]
    fn library_sync_decisions() {
        let none = HashMap::new();
        let now = 1_000_000;
        let mut saved = Saved::default();
        let favs = cloud(FAVORITES, 9);
        // Favorites never synced from here come down first, even when this browser's are newer...
        assert_eq!(next_library_job(&[meta(FAVORITES, 20)], &[favs.clone()], &saved, &none, &none, now), Some(Job::Download(favs.clone())));
        // ...then follow the same rules as projects.
        saved.lib_uploaded.insert(FAVORITES.into(), 9);
        assert_eq!(next_library_job(&[meta(FAVORITES, 20)], &[favs.clone()], &saved, &none, &none, now), Some(Job::Upload(FAVORITES.into())));
        assert_eq!(next_library_job(&[meta(FAVORITES, 9)], &[favs.clone()], &saved, &none, &none, now), None);
        // An icon added in another browser comes down; one removed here is deleted online.
        assert_eq!(next_library_job(&[meta(FAVORITES, 9)], &[favs.clone(), cloud("asset.x", 3)], &saved, &none, &none, now), Some(Job::Download(cloud("asset.x", 3))));
        saved.lib_deleting.push("asset.x".into());
        assert_eq!(next_library_job(&[meta(FAVORITES, 9)], &[favs, cloud("asset.x", 3)], &saved, &none, &none, now), Some(Job::Delete("asset.x".into())));
    }

    #[test]
    fn header_values_are_encoded() {
        assert_eq!(encode("Cell diagram (v2)"), "Cell%20diagram%20%28v2%29");
        assert_eq!(encode("é"), "%C3%A9");
    }
}
