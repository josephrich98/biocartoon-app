//! Contributing to the shared library: assets and templates are POSTed to the submissions Worker
//! (`worker/` in the biocartoon repository), which validates them, filters profanity and rate
//! limits, and holds them for review. Nothing reaches other users until it is approved and the catalog is published.

#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

/// The deployed `biocartoon-submit` Worker. `BIOCARTOON_SUBMIT_URL` overrides it.
pub const DEFAULT_SUBMIT_URL: &str = "https://api.biocartoon.com";

/// Limits the Worker enforces (worker/src/limits.ts); checked here first for clearer errors.
/// The same limits hold for Your library: one image (an icon, or a template's picture) as
/// PNG or SVG, and a whole submission (a template with its thumbnail and images).
pub const MAX_PNG_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_SVG_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_IMAGE_SIDE: u32 = 4096;
pub const MAX_TEMPLATE_JSON_BYTES: usize = 1024 * 1024;
pub const MAX_TEMPLATE_ITEMS: usize = 300;
pub const MAX_TEMPLATE_IMAGES: usize = 24;
/// Submissions per rolling hour from one running app (the Worker allows 20 a day per address).
pub const MAX_SUBMISSIONS_PER_HOUR: usize = 10;

/// The metadata the Worker expects in the `meta` field.
#[derive(Clone, Debug, serde::Serialize)]
pub struct SubmitMeta {
    pub title: String,
    pub description: String,
    pub category: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// The SVG's layers as the contributor named them (the catalog's `layers`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<crate::catalog::AssetLayer>,
    pub author: String,
    pub license: String,
    /// The contributor's answer to "Was generative AI used to make this artwork?" (`None`: not
    /// specified). The Worker only takes CC BY 4.0 with `Some(false)`.
    pub generative_ai_used: Option<bool>,
    /// How generative AI was used, in the contributor's words (only with `Some(true)`; optional).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub generative_ai_details: String,
    /// Optional contact address for the reviewers; the Worker stores it privately and never publishes it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub email: String,
    /// The contributor asked for it to go in the official BioCartoon library; a reviewer
    /// decides (it is still credited to them either way).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub official: bool,
}

pub enum Part<'a> {
    Text(&'a str, String),
    File(&'a str, Vec<u8>, &'static str),
}

/// POST a multipart form to `<url>/submit` in the background (desktop: a thread; web: a fetch);
/// `done` gets the submission id.
pub fn submit(url: &str, parts: &[Part], done: impl FnOnce(Result<String, String>) + Send + 'static) {
    let boundary = format!("biocartoon-{:x}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));
    let mut body = Vec::new();
    for p in parts {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match p {
            Part::Text(name, value) => {
                body.extend_from_slice(format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes());
                body.extend_from_slice(value.as_bytes());
            }
            Part::File(name, bytes, mime) => {
                body.extend_from_slice(format!("Content-Disposition: form-data; name=\"{name}\"; filename=\"{name}\"\r\nContent-Type: {mime}\r\n\r\n").as_bytes());
                body.extend_from_slice(bytes);
            }
        }
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    post_async(url, "submit", format!("multipart/form-data; boundary={boundary}"), body, done)
}

/// Kinds of idea the "Submit an idea" form offers: (value sent, label).
pub const IDEA_KINDS: [(&str, &str); 3] = [("icon", "Icon"), ("template", "Template"), ("other", "Other")];

/// An idea for the BioCartoon team (an icon or template the library lacks, or anything else),
/// sent to `<url>/suggest`.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Suggestion {
    /// One of [`IDEA_KINDS`].
    pub kind: &'static str,
    pub title: String,
    pub description: String,
    pub name: String,
    /// Optional contact address, seen only by the reviewers.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub email: String,
}

/// A plausible email address, or empty (the field is optional); the Worker checks the same way.
pub fn check_email(s: &str) -> Result<String, String> {
    let s = s.trim();
    let ok = s.is_empty()
        || (s.chars().count() <= 254 && !s.chars().any(char::is_whitespace) && s.split_once('@').is_some_and(|(user, host)| !user.is_empty() && !host.contains('@') && host.contains('.') && !host.starts_with('.') && !host.ends_with('.')));
    if ok { Ok(s.to_string()) } else { Err(format!("\"{s}\" doesn't look like an email address")) }
}

/// POST a suggestion; `done` gets its id. Runs in the background (desktop: a thread; web: a fetch).
pub fn suggest(url: &str, s: &Suggestion, done: impl FnOnce(Result<String, String>) + Send + 'static) {
    match serde_json::to_vec(s) {
        Ok(body) => post_async(url, "suggest", "application/json".into(), body, done),
        Err(e) => done(Err(e.to_string())),
    }
}

/// A request to take an icon out of the BioCartoon library, sent to `<url>/removal`. A reviewer
/// reads it; nothing is removed automatically.
#[derive(Clone, Debug, serde::Serialize)]
pub struct RemovalRequest {
    /// The icon's catalog id (for one of the user's own submissions, its submission id).
    pub asset_id: String,
    pub title: String,
    pub reason: String,
    pub name: String,
    /// Optional contact address, seen only by the reviewers.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub email: String,
}

/// POST a removal request; `done` gets its id. Runs in the background, as [`suggest`] does.
pub fn request_removal(url: &str, r: &RemovalRequest, done: impl FnOnce(Result<String, String>) + Send + 'static) {
    match serde_json::to_vec(r) {
        Ok(body) => post_async(url, "removal", "application/json".into(), body, done),
        Err(e) => done(Err(e.to_string())),
    }
}

/// POST `body` to `<url>/<path>` in the background; `done` gets the id the Worker answers with.
#[cfg(not(target_arch = "wasm32"))]
fn post_async(url: &str, path: &str, content_type: String, body: Vec<u8>, done: impl FnOnce(Result<String, String>) + Send + 'static) {
    let (url, path) = (url.to_string(), path.to_string());
    crate::platform::spawn("submit", move || done(post(&url, &path, &content_type, body)));
}

/// Web: the same with fetch (the Worker answers the app's origin for these routes).
#[cfg(target_arch = "wasm32")]
fn post_async(url: &str, path: &str, content_type: String, body: Vec<u8>, done: impl FnOnce(Result<String, String>) + Send + 'static) {
    let mut req = ehttp::Request::post(format!("{}/{path}", url.trim_end_matches('/')), body);
    req.headers.insert("Content-Type", content_type);
    ehttp::fetch(req, move |res| {
        done(match res {
            Err(e) => Err(format!("Couldn't reach the submission server: {e}")),
            Ok(r) => reply(r.status, &r.bytes),
        })
    });
}

/// The Worker's answer: the new id, or its error message.
fn reply(status: u16, body: &[u8]) -> Result<String, String> {
    let v: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
    if (200..300).contains(&status) {
        return v["id"].as_str().map(str::to_string).ok_or_else(|| "The submission server sent an unexpected reply".into());
    }
    Err(v["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("The submission server answered HTTP {status}")))
}

#[cfg(not(target_arch = "wasm32"))]
fn post(url: &str, path: &str, content_type: &str, body: Vec<u8>) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(120))).http_status_as_error(false).build().into();
    let mut res = agent
        .post(&format!("{}/{path}", url.trim_end_matches('/')))
        .header("Content-Type", content_type)
        .send(&body[..])
        .map_err(|e| format!("Couldn't reach the submission server: {e}"))?;
    let status = res.status().as_u16();
    let text = res.body_mut().with_config().limit(64 * 1024).read_to_string().unwrap_or_default();
    reply(status, text.as_bytes())
}

/// Whether the Worker refused an asset for its SVG's content (scripts, links to other files, not
/// well-formed, too large), the one refusal its PNG may get past. Its messages for those all name
/// the SVG (see `checkSvg` in the Worker's validate.ts); a network error, a rate limit or a refused
/// title would only fail again.
pub fn is_svg_refusal(e: &str) -> bool {
    e.starts_with("SVG ") || e.starts_with("Not an SVG")
}

/// Sliding one-hour window of submission times, so a stuck button can't burn the daily quota.
#[derive(Default)]
pub struct RateLimit {
    times: Vec<f64>,
}

impl RateLimit {
    pub fn try_take(&mut self, now: f64) -> Result<(), String> {
        self.times.retain(|t| now - t < 3600.0);
        if self.times.len() >= MAX_SUBMISSIONS_PER_HOUR {
            let wait = 3600.0 - (now - self.times[0]);
            return Err(format!("Too many submissions; try again in {} min", (wait / 60.0).ceil()));
        }
        self.times.push(now);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_refusals() {
        assert!(is_svg_refusal("SVG files can't contain scripts"));
        assert!(is_svg_refusal("SVG is not well-formed XML"));
        assert!(is_svg_refusal("Not an SVG file (the root element must be <svg>)"));
        assert!(!is_svg_refusal("Limit reached: 20 submissions a day. Please try again tomorrow."));
        assert!(!is_svg_refusal("Couldn't reach the submission server: timeout"));
    }

    #[test]
    fn emails() {
        assert_eq!(check_email("  ").unwrap(), "");
        assert_eq!(check_email(" ann@lab.org ").unwrap(), "ann@lab.org");
        for bad in ["ann", "ann@lab", "@lab.org", "ann@@lab.org", "a b@lab.org", "ann@.org", "ann@lab."] {
            assert!(check_email(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn rate_limit_window() {
        let mut r = RateLimit::default();
        for i in 0..MAX_SUBMISSIONS_PER_HOUR {
            r.try_take(i as f64).unwrap();
        }
        assert!(r.try_take(100.0).is_err());
        assert!(r.try_take(3600.5).is_ok());
    }
}
