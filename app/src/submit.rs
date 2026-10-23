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
    pub author: String,
    pub license: String,
    /// Optional contact address for the reviewers; the Worker stores it privately and never publishes it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub email: String,
}

pub enum Part<'a> {
    Text(&'a str, String),
    File(&'a str, Vec<u8>, &'static str),
}

/// POST a multipart form to `<url>/submit`; returns the submission id.
pub fn submit(url: &str, parts: &[Part]) -> Result<String, String> {
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
    post(url, "submit", &format!("multipart/form-data; boundary={boundary}"), body)
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

/// POST a suggestion; returns its id.
pub fn suggest(url: &str, s: &Suggestion) -> Result<String, String> {
    if cfg!(target_arch = "wasm32") {
        return Err("Sending ideas isn't available in the web version yet.".into());
    }
    let body = serde_json::to_vec(s).map_err(|e| e.to_string())?;
    post(url, "suggest", "application/json", body)
}

/// Web build: submissions run inside blocking jobs, which the browser can't do yet.
#[cfg(target_arch = "wasm32")]
fn post(_url: &str, _path: &str, _content_type: &str, _body: Vec<u8>) -> Result<String, String> {
    Err("Submitting to the shared library isn't available in the web version yet; it was saved to your own library.".into())
}

#[cfg(not(target_arch = "wasm32"))]
fn post(url: &str, path: &str, content_type: &str, body: Vec<u8>) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(120))).http_status_as_error(false).build().into();
    let mut res = agent
        .post(&format!("{}/{path}", url.trim_end_matches('/')))
        .header("Content-Type", content_type)
        .send(&body[..])
        .map_err(|e| format!("Couldn't reach the submission server: {e}"))?;
    let status = res.status();
    let text = res.body_mut().with_config().limit(64 * 1024).read_to_string().unwrap_or_default();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    if status.is_success() {
        return v["id"].as_str().map(str::to_string).ok_or_else(|| "The submission server sent an unexpected reply".into());
    }
    Err(v["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("The submission server answered HTTP {status}")))
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
