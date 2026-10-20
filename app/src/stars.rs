//! Stars on catalog assets. Every tile shows its count (bottom right) and clicking the star adds
//! or removes the user's own. The counts live on the submissions Worker (the biocartoon repo's
//! `worker/src/stars.ts`, D1), which identifies this app install by a random `client_id` kept in Settings; the
//! published catalog carries a snapshot in `assets.stars` (`data/publish_catalog.py`), and the
//! latest counts the Worker reported are kept in the local `usage.stars` table
//! (`catalog::Req::SetStars`), so tiles are right even before the Worker answers.
//!
//!   GET  <url>/stars?ids=a,b&client=<id>   -> {"stars": {id: n}, "mine": [id]}
//!   POST <url>/star  {"id", "client", "on"} -> {"id", "stars", "starred"}

use crate::catalog::StarUpdate;
use std::sync::mpsc::{channel, Receiver, Sender};

pub enum StarsMsg {
    /// Counts for the ids asked about (ids with no stars come back as 0, not starred).
    Counts(Vec<StarUpdate>),
    /// The Worker's answer to a toggle; `Err` carries the id, what was asked for, and why.
    Toggled(Result<StarUpdate, (String, bool, String)>),
}

pub struct StarsClient {
    tx: Sender<StarsMsg>,
    rx: Receiver<StarsMsg>,
    ctx: Option<egui::Context>,
}

impl StarsClient {
    /// `ctx` is repainted when an answer arrives.
    pub fn new(ctx: Option<egui::Context>) -> Self {
        let (tx, rx) = channel();
        Self { tx, rx, ctx }
    }

    /// Ask for the counts of `ids` (a screen of results; the Worker caps one request at 300).
    pub fn fetch(&self, url: &str, client: &str, ids: Vec<String>) {
        if ids.is_empty() {
            return;
        }
        let ids: Vec<String> = ids.into_iter().filter(|i| !i.is_empty()).collect();
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        for chunk in ids.chunks(300) {
            let asked = chunk.to_vec();
            let q = format!("{}/stars?ids={}&client={}", url.trim_end_matches('/'), encode(&asked.join(",")), encode(client));
            let (tx, ctx) = (tx.clone(), ctx.clone());
            http(Http::Get(q), move |res| {
                // Anything but a proper answer (an old Worker says {"error": "Not found"}) is
                // ignored, so the counts already shown are not replaced by zeros.
                let v = match res.and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).map_err(|e| e.to_string())) {
                    Ok(v) if v["stars"].is_object() => v,
                    Ok(v) => {
                        log::info!("stars: {}", v["error"].as_str().unwrap_or("unexpected reply"));
                        return;
                    }
                    Err(e) => {
                        log::info!("stars: {e}");
                        return;
                    }
                };
                let updates = asked
                    .into_iter()
                    .map(|id| StarUpdate {
                        n: v["stars"][&id].as_i64().unwrap_or(0),
                        mine: v["mine"].as_array().is_some_and(|m| m.iter().any(|x| x.as_str() == Some(&id))),
                        id,
                    })
                    .collect();
                let _ = tx.send(StarsMsg::Counts(updates));
                if let Some(c) = ctx {
                    c.request_repaint();
                }
            });
        }
    }

    /// Add (`on`) or remove the user's star on `id`.
    pub fn toggle(&self, url: &str, client: &str, id: String, on: bool) {
        let body = serde_json::json!({ "id": id, "client": client, "on": on }).to_string();
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        http(Http::PostJson(format!("{}/star", url.trim_end_matches('/')), body), move |res| {
            let r = res.and_then(|b| {
                let v: serde_json::Value = serde_json::from_slice(&b).map_err(|_| "unexpected reply".to_string())?;
                if let Some(e) = v["error"].as_str() {
                    return Err(e.to_string());
                }
                Ok(StarUpdate { id: id.clone(), n: v["stars"].as_i64().ok_or("unexpected reply")?, mine: v["starred"].as_bool().unwrap_or(on) })
            });
            let _ = tx.send(StarsMsg::Toggled(r.map_err(|e| (id, on, e))));
            if let Some(c) = ctx {
                c.request_repaint();
            }
        });
    }

    pub fn poll(&self) -> Vec<StarsMsg> {
        let mut v = Vec::new();
        while let Ok(m) = self.rx.try_recv() {
            v.push(m);
        }
        v
    }
}

pub use biocartoon_catalog::catalog::format_stars;

/// A random id for this app install (32 hex digits); made once and kept in Settings.
pub fn new_client_id() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let draw = || (js_sys::Math::random() * (1u64 << 53) as f64) as u64;
        format!("{:016x}{:016x}", draw(), draw())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::hash::{BuildHasher, Hasher};
        // RandomState is seeded from the OS; two differently keyed hashers give 128 bits.
        let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0) as u64;
        let word = |salt: u64| {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u64(nanos ^ salt);
            h.write_u64(std::process::id() as u64);
            h.finish()
        };
        format!("{:016x}{:016x}", word(0x9e37_79b9_7f4a_7c15), word(0xbf58_476d_1ce4_e5b9))
    }
}

fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'/' | b',' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

enum Http {
    Get(String),
    PostJson(String, String),
}

#[cfg(not(target_arch = "wasm32"))]
fn http(req: Http, done: impl FnOnce(Result<Vec<u8>, String>) + Send + 'static) {
    crate::platform::spawn("stars", move || {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(20)))
            .http_status_as_error(false)
            .build()
            .into();
        let res = match &req {
            Http::Get(url) => agent.get(url).call(),
            Http::PostJson(url, body) => agent.post(url).header("Content-Type", "application/json").send(body.as_bytes()),
        };
        let r = res.map_err(|e| format!("Couldn't reach the submission server: {e}")).and_then(|mut res| {
            let bytes = res.body_mut().with_config().limit(4 * 1024 * 1024).read_to_vec().map_err(|e| e.to_string())?;
            // Error replies carry JSON too ({"error": ...}); only an empty one needs the status.
            if res.status().is_success() || !bytes.is_empty() {
                Ok(bytes)
            } else {
                Err(format!("The submission server answered HTTP {}", res.status()))
            }
        });
        done(r);
    });
}

#[cfg(target_arch = "wasm32")]
fn http(req: Http, done: impl FnOnce(Result<Vec<u8>, String>) + Send + 'static) {
    let r = match req {
        Http::Get(url) => ehttp::Request::get(url),
        Http::PostJson(url, body) => {
            let mut r = ehttp::Request::post(url, body.into_bytes());
            r.headers.insert("Content-Type", "application/json");
            r
        }
    };
    ehttp::fetch(r, move |res| {
        done(res.map_err(|e| format!("Couldn't reach the submission server: {e}")).and_then(|r| {
            if r.ok || !r.bytes.is_empty() {
                Ok(r.bytes)
            } else {
                Err(format!("The submission server answered HTTP {}", r.status))
            }
        }))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_are_compact() {
        let cases = [
            (0, "0"),
            (1, "1"),
            (999, "999"),
            (1000, "1k"),
            (1099, "1k"),
            (1100, "1.1k"),
            (1999, "1.9k"),
            (9999, "9.9k"),
            (10_000, "10k"),
            (10_999, "10k"),
            (99_999, "99k"),
            (100_000, "100k"),
            (999_999, "999k"),
            (1_000_000, "1M"),
            (1_150_000, "1.1M"),
            (10_000_000, "10M"),
            (2_500_000_000, "2.5B"),
            (-3, "0"),
        ];
        for (n, want) in cases {
            assert_eq!(format_stars(n), want, "{n}");
        }
    }

    #[test]
    fn client_ids_are_hex_and_distinct() {
        let a = new_client_id();
        let b = new_client_id();
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_ne!(a, b);
    }

    #[test]
    fn query_values_are_escaped() {
        assert_eq!(encode("bioart/2_626557,user/a-b"), "bioart/2_626557,user/a-b");
        assert_eq!(encode("a b&c"), "a%20b%26c");
    }
}
