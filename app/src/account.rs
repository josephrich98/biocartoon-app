//! Optional accounts (web app). Signing in with Google or GitHub keeps an online copy of every
//! project on the submissions Worker's account routes (the biocartoon repo's
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
//! Signing in leaves the app (the browser goes to the provider and back), and the Worker hands the
//! session token back in the URL fragment, which [`Account::new`] reads and clears.

use crate::app::BioCartoon;
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
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct CloudProject {
    pub id: String,
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

/// Pick the next job, given what is here and what is online. `open` is the project in the
/// editor, which is never replaced under the user; `skip` maps projects whose last upload was
/// refused (too large, account full) to the `modified` time that was refused.
pub fn next_job(local: &[ProjectMeta], cloud: &[CloudProject], saved: &Saved, open: Option<&str>, skip: &HashMap<String, i64>, last_upload: &HashMap<String, i64>, now: i64) -> Option<Job> {
    if let Some(id) = saved.deleting.first() {
        return Some(Job::Delete(id.clone()));
    }
    for c in cloud {
        let here = local.iter().find(|p| p.id == c.id);
        let up = saved.uploaded.get(&c.id).copied();
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
            let online = saved.uploaded.get(&p.id).copied().or_else(|| cloud.iter().find(|c| c.id == p.id).map(|c| c.modified));
            online.is_none_or(|t| p.modified > t)
        })
        .find(|p| now - p.modified >= UPLOAD_QUIET_MS || now - last_upload.get(&p.id).copied().unwrap_or(0) >= UPLOAD_EVERY_MS)
        .map(|p| Job::Upload(p.id.clone()))
}

enum Msg {
    Me(Result<serde_json::Value, Reply>),
    List(Result<Vec<CloudProject>, Reply>),
    Uploaded(String, i64, Result<serde_json::Value, Reply>),
    Downloaded(CloudProject, Result<Vec<u8>, Reply>),
    Deleted(String, Result<(), Reply>),
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
        let mut a = Self { path, base: String::new(), need_me: false, saved, cloud: None, used: 0, quota: 0, error: None, notice: None, busy: false, retry_at: 0, local: Vec::new(), local_at: 0, skip: HashMap::new(), last_upload: HashMap::new(), tx, rx, ctx };
        match web::take_fragment() {
            Some(Ok(token)) => {
                // A new sign-in replaces any old one, and starts its bookkeeping afresh.
                a.saved = Some(Saved { token, ..Default::default() });
                a.save();
                a.notice = Some(("Signed in. Your projects are being saved online.".into(), false));
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
            (self.saved, self.cloud) = (None, None);
            self.save();
            self.notice = Some((format!("{} Your projects are still in this browser.", e.message), true));
        } else {
            self.error = Some(e.message);
            self.retry_at = crate::projects::now_ms() + RETRY_MS;
        }
    }

    /// Projects changed here since their online copy (for the account menu).
    pub fn pending(&self, local: &[ProjectMeta]) -> usize {
        let Some(s) = &self.saved else { return 0 };
        local.iter().filter(|p| s.uploaded.get(&p.id).is_none_or(|t| p.modified > *t)).count() + s.deleting.len()
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
        if now - a.local_at >= 5000 {
            (a.local, a.local_at) = (self.store.list(), now);
        }
        let local = &a.local;
        let job = next_job(local, cloud, a.saved.as_ref().unwrap(), self.project.as_deref(), &a.skip, &a.last_upload, now);
        let Some(job) = job else { return };
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
            Msg::Me(Err(e)) => a.failed(e),
            Msg::List(Err(e)) | Msg::Uploaded(_, _, Err(e)) | Msg::Downloaded(_, Err(e)) | Msg::Deleted(_, Err(e)) => {
                a.busy = false;
                a.failed(e);
            }
            Msg::SignedOut => {}
            Msg::AccountDeleted(Ok(())) => self.toast("Your account and its online copies were deleted. Your projects are still in this browser."),
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

    /// Sign out here: the session ends on the server, and projects stay in this browser.
    pub fn account_sign_out(&mut self) {
        let Some(base) = self.submit_url() else { return };
        let a = &mut self.account;
        let send = a.send();
        request(api(&base, "/auth/logout"), "POST", &a.token(), Vec::new(), &[], move |_| send(Msg::SignedOut));
        (a.saved, a.cloud, a.error) = (None, None, None);
        a.save();
        self.toast("Signed out. Your projects are still in this browser.");
    }

    /// Delete the account and every online copy (projects in this browser stay).
    pub fn account_delete(&mut self) {
        let Some(base) = self.submit_url() else { return };
        let a = &mut self.account;
        let send = a.send();
        request(api(&base, "/me"), "DELETE", &a.token(), Vec::new(), &[], move |r| send(Msg::AccountDeleted(r.map(|_| ()))));
        (a.saved, a.cloud, a.error) = (None, None, None);
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
        assert_eq!(next_job(&[], &[cloud("a", 5)], &saved, None, &none, &none, now), Some(Job::Download(cloud("a", 5))));
        // Projects never uploaded go up; ones online and unchanged don't.
        assert_eq!(next_job(&[meta("a", 5)], &[], &saved, None, &none, &none, now), Some(Job::Upload("a".into())));
        assert_eq!(next_job(&[meta("a", 5)], &[cloud("a", 5)], &saved, None, &none, &none, now), None);
        saved.uploaded.insert("a".into(), 5);
        assert_eq!(next_job(&[meta("a", 5)], &[cloud("a", 5)], &saved, None, &none, &none, now), None);

        // Being edited right now: wait for a pause, unless the last upload is old.
        let editing = meta("a", now - 1000);
        assert_eq!(next_job(std::slice::from_ref(&editing), &[cloud("a", 5)], &saved, Some("a"), &none, &HashMap::from([("a".to_string(), now - 60_000)]), now), None);
        assert_eq!(next_job(std::slice::from_ref(&editing), &[cloud("a", 5)], &saved, Some("a"), &none, &HashMap::from([("a".to_string(), now - UPLOAD_EVERY_MS)]), now), Some(Job::Upload("a".into())));

        // Newer online and untouched here: comes down, except into the open editor.
        assert_eq!(next_job(&[meta("a", 5)], &[cloud("a", 9)], &saved, None, &none, &none, now), Some(Job::Download(cloud("a", 9))));
        assert_eq!(next_job(&[meta("a", 5)], &[cloud("a", 9)], &saved, Some("a"), &none, &none, now), None);
        // Edited on both sides: this browser's copy wins.
        assert_eq!(next_job(&[meta("a", 7)], &[cloud("a", 9)], &saved, None, &none, &none, now), Some(Job::Upload("a".into())));

        // A refused upload isn't retried until the project changes again.
        let skip = HashMap::from([("a".to_string(), 7)]);
        assert_eq!(next_job(&[meta("a", 7)], &[cloud("a", 9)], &saved, None, &skip, &none, now), None);
        assert_eq!(next_job(&[meta("a", 8)], &[], &saved, None, &skip, &none, now), Some(Job::Upload("a".into())));

        // Deletions go first, and a deleted project isn't downloaded back.
        saved.deleting.push("b".into());
        assert_eq!(next_job(&[], &[cloud("b", 1)], &saved, None, &none, &none, now), Some(Job::Delete("b".into())));
    }

    #[test]
    fn header_values_are_encoded() {
        assert_eq!(encode("Cell diagram (v2)"), "Cell%20diagram%20%28v2%29");
        assert_eq!(encode("é"), "%C3%A9");
    }
}
