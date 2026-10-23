//! AI backend abstraction. [`OpenAiBackend`] and [`GeminiBackend`] use those providers' image
//! models; [`SvgBackend`] has Claude, an OpenAI chat model, or a model behind vLLM draw SVG. [`make_backend`] picks
//! one. `BIOCARTOON_AI=placeholder` swaps in a deterministic offline backend for
//! development.

use image::{Rgba, RgbaImage};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::thread;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestyleStyle {
    Textbook,
    Watercolor,
    ColoredPencil,
    ThreeD,
    PenAndInk,
    LineArt,
    PencilSketch,
    Silhouette,
}

impl RestyleStyle {
    pub const ALL: [RestyleStyle; 8] = [
        RestyleStyle::Textbook,
        RestyleStyle::Watercolor,
        RestyleStyle::ColoredPencil,
        RestyleStyle::ThreeD,
        RestyleStyle::PenAndInk,
        RestyleStyle::LineArt,
        RestyleStyle::PencilSketch,
        RestyleStyle::Silhouette,
    ];
    pub fn label(self) -> &'static str {
        match self {
            RestyleStyle::Textbook => "Textbook",
            RestyleStyle::Watercolor => "Watercolor",
            RestyleStyle::ColoredPencil => "Colored pencil",
            RestyleStyle::ThreeD => "3D style",
            RestyleStyle::PenAndInk => "Pen & ink",
            RestyleStyle::LineArt => "Line art",
            RestyleStyle::PencilSketch => "Pencil sketch",
            RestyleStyle::Silhouette => "Silhouette",
        }
    }
}

#[derive(Clone, Debug)]
pub enum AiOp {
    Restyle(RestyleStyle),
    RemoveBackground,
    RemoveText,
    Custom(String),
}

impl AiOp {
    pub fn label(&self) -> String {
        match self {
            AiOp::Restyle(s) => format!("Restyle: {}", s.label()),
            AiOp::RemoveBackground => "Remove background".into(),
            AiOp::RemoveText => "Remove text".into(),
            AiOp::Custom(p) => format!("Custom edit: {p}"),
        }
    }
}

/// Model providers offered in the AI setup dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BackendKind {
    OpenAi,
    Gemini,
    Anthropic,
    /// A self-hosted model behind vLLM's OpenAI-compatible server.
    Vllm,
}

impl BackendKind {
    /// In the order the provider menu lists them, alphabetical by label.
    pub const ALL: [BackendKind; 4] = [BackendKind::Anthropic, BackendKind::Gemini, BackendKind::OpenAi, BackendKind::Vllm];
    pub fn label(self) -> &'static str {
        match self {
            BackendKind::OpenAi => "OpenAI",
            BackendKind::Gemini => "Google Gemini",
            BackendKind::Anthropic => "Anthropic",
            BackendKind::Vllm => "vLLM (self-hosted)",
        }
    }
    /// Suggested model ids; the field stays editable. OpenAI's chat models (anything that
    /// isn't a gpt-image model) draw SVG instead of painting. vLLM has none: the server
    /// answers with whatever model it was started with.
    pub fn models(self) -> &'static [&'static str] {
        match self {
            BackendKind::OpenAi => &[
                "gpt-image-1.5",
                "gpt-image-1",
                "gpt-image-1-mini",
                "gpt-image-2",
                "gpt-6-astra",
                "gpt-6-sol",
                "gpt-6-luna",
                "gpt-5.6-sol",
                "gpt-5.6-terra",
                "gpt-5.6-luna",
            ],
            BackendKind::Gemini => &["gemini-3.1-flash-image", "gemini-nano-banana-2.1", "gemini-3-pro-image"],
            BackendKind::Anthropic => &[
                "claude-opus-5-5",
                "claude-fable-5-1",
                "claude-sonnet-5-5",
                "claude-haiku-5-5",
                "claude-opus-5",
                "claude-fable-5",
                "claude-sonnet-5",
                "claude-opus-4-8",
            ],
            BackendKind::Vllm => &[],
        }
    }
    pub fn key_url(self) -> &'static str {
        match self {
            BackendKind::OpenAi => "https://platform.openai.com/api-keys",
            BackendKind::Gemini => "https://aistudio.google.com/apikey",
            BackendKind::Anthropic => "https://console.anthropic.com/settings/keys",
            BackendKind::Vllm => "https://docs.vllm.ai/en/latest/serving/openai_compatible_server.html",
        }
    }
    /// True when `model` writes SVG through a chat API and so reads the system prompts;
    /// OpenAI's gpt-image models and Gemini's image models paint and get neither.
    pub fn draws_svg(self, model: &str) -> bool {
        match self {
            BackendKind::OpenAi => !model.trim().starts_with("gpt-image"),
            BackendKind::Gemini => false,
            BackendKind::Anthropic | BackendKind::Vllm => true,
        }
    }
    /// A vLLM server needs a URL instead of a model and key.
    pub fn self_hosted(self) -> bool {
        self == BackendKind::Vllm
    }
}

/// What a Generate or Edit job made: its pixels, and the SVG they were rendered from when the
/// model drew one, which is saved as is so the image stays a vector on the page and in export.
#[derive(Debug)]
pub struct AiImage {
    pub image: RgbaImage,
    pub svg: Option<String>,
}

impl From<RgbaImage> for AiImage {
    fn from(image: RgbaImage) -> Self {
        Self { image, svg: None }
    }
}

pub trait AiBackend: Send + Sync {
    fn name(&self) -> String;
    /// Text-to-image. Returns an RGBA image with transparent background where possible.
    fn generate(&self, prompt: &str) -> Result<AiImage, String>;
    /// Image-to-image edit. `svg` is the image's source when it is an SVG.
    fn edit(&self, image: &RgbaImage, svg: Option<&str>, op: &AiOp) -> Result<AiImage, String>;
    /// Chat reply used by the Custom edit dialog.
    fn chat(&self, history: &[(bool, String)], message: &str) -> Result<String, String>;
    /// Runs `job` and hands the result to `done`. The default blocks on the methods above, which
    /// is fine on the desktop worker thread; a backend that has to work in the browser overrides it.
    fn run(&self, job: AiJob, done: Box<dyn FnOnce(AiResult) + Send>) {
        done(match job {
            AiJob::Generate { prompt } => {
                let result = self.generate(&prompt);
                AiResult::Generated { prompt, result }
            }
            AiJob::Edit { item, image, svg, op } => {
                let result = self.edit(&image, svg.as_deref(), &op);
                AiResult::Edited { item, op, result }
            }
            AiJob::Chat { item, history, message } => {
                let result = self.chat(&history, &message);
                AiResult::ChatReply { item, result }
            }
        })
    }
}

/// Produces deterministic placeholder output so the whole generate / edit / upload flow can be
/// exercised before a real model is connected.
pub struct PlaceholderBackend;

fn hash(s: &str) -> u32 {
    s.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619))
}

fn hsv(h: f32, s: f32, v: f32) -> [u8; 3] {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    let (r, g, b) = match (i as i32).rem_euclid(6) {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    [(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8]
}

impl AiBackend for PlaceholderBackend {
    fn name(&self) -> String {
        "Placeholder (no model connected)".into()
    }

    fn generate(&self, prompt: &str) -> Result<AiImage, String> {
        let n = 512u32;
        let h = hash(prompt);
        let hue = (h % 360) as f32 / 360.0;
        let hue2 = ((h >> 8) % 360) as f32 / 360.0;
        let c1 = hsv(hue, 0.55, 0.85);
        let c2 = hsv(hue2, 0.6, 0.7);
        let mut img = RgbaImage::from_pixel(n, n, Rgba([0, 0, 0, 0]));
        let cx = n as f32 / 2.0;
        let cy = n as f32 / 2.0;
        let blobs = 3 + (h % 4) as usize;
        for b in 0..blobs {
            let a = b as f32 / blobs as f32 * std::f32::consts::TAU + (h as f32 * 0.001);
            let bx = cx + a.cos() * 90.0;
            let by = cy + a.sin() * 90.0;
            let r = 110.0 + ((h >> (b * 3)) % 40) as f32;
            for y in 0..n {
                for x in 0..n {
                    let d = ((x as f32 - bx).powi(2) + (y as f32 - by).powi(2)).sqrt();
                    if d < r {
                        let t = d / r;
                        let c = if b % 2 == 0 { c1 } else { c2 };
                        let shade = 1.0 - t * 0.35;
                        let px = img.get_pixel_mut(x, y);
                        *px = Rgba([
                            (c[0] as f32 * shade) as u8,
                            (c[1] as f32 * shade) as u8,
                            (c[2] as f32 * shade) as u8,
                            255,
                        ]);
                    }
                }
            }
        }
        // Outline: darken the blob edge for a "flat illustration" look.
        let src = img.clone();
        for y in 1..n - 1 {
            for x in 1..n - 1 {
                if src.get_pixel(x, y)[3] == 0 {
                    continue;
                }
                let edge = [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)]
                    .iter()
                    .any(|&(a, b)| src.get_pixel(a, b)[3] == 0);
                if edge {
                    img.put_pixel(x, y, Rgba([40, 40, 50, 255]));
                }
            }
        }
        Ok(img.into())
    }

    fn edit(&self, image: &RgbaImage, _svg: Option<&str>, op: &AiOp) -> Result<AiImage, String> {
        let mut out = image.clone();
        match op {
            AiOp::RemoveBackground => {
                for p in out.pixels_mut() {
                    if p[0] > 240 && p[1] > 240 && p[2] > 240 {
                        p[3] = 0;
                    }
                }
            }
            AiOp::Restyle(style) => match style {
                RestyleStyle::Silhouette => {
                    for p in out.pixels_mut() {
                        if p[3] > 0 {
                            *p = Rgba([30, 30, 36, p[3]]);
                        }
                    }
                }
                RestyleStyle::PencilSketch | RestyleStyle::LineArt | RestyleStyle::PenAndInk => {
                    for p in out.pixels_mut() {
                        let l = (0.3 * p[0] as f32 + 0.59 * p[1] as f32 + 0.11 * p[2] as f32) as u8;
                        let l = if matches!(style, RestyleStyle::PenAndInk) {
                            if l < 128 { 20 } else { 245 }
                        } else {
                            l
                        };
                        *p = Rgba([l, l, l, p[3]]);
                    }
                }
                RestyleStyle::Watercolor | RestyleStyle::ColoredPencil => {
                    for p in out.pixels_mut() {
                        for c in 0..3 {
                            p[c] = (p[c] as f32 * 0.7 + 255.0 * 0.3) as u8;
                        }
                    }
                }
                RestyleStyle::Textbook | RestyleStyle::ThreeD => {}
            },
            AiOp::RemoveText | AiOp::Custom(_) => {
                return Err(format!(
                    "'{}' needs a real model.",
                    op.label()
                ))
            }
        }
        Ok(out.into())
    }

    fn chat(&self, _history: &[(bool, String)], message: &str) -> Result<String, String> {
        Ok(format!(
            "No model is connected, so I can't apply \"{message}\" yet. The placeholder backend \
             only supports Remove background and the simple restyles."
        ))
    }
}

/// A chat model that draws: Claude through the Messages API, or an OpenAI chat model or a
/// self-hosted model behind vLLM, both through Chat Completions. None output raster images, so the model writes SVG,
/// which is rendered with resvg; edits send the current image and get back a redrawn SVG.
pub struct SvgBackend {
    api: Api,
    model: String,
    key: String,
    prompts: AiPrompts,
}

#[derive(Clone)]
enum Api {
    Anthropic,
    OpenAi,
    /// `base` is the server's URL, with or without the trailing `/v1`.
    Vllm { base: String },
}

/// One turn of a conversation; `image` is a base64 PNG.
struct Turn {
    user: bool,
    text: String,
    image: Option<String>,
}

const ANTHROPIC_URL: &str = "https://api.anthropic.com/v1/messages";
const OPENAI_CHAT_URL: &str = "https://api.openai.com/v1/chat/completions";
/// Longer side of a generated image, in pixels.
const GENERATE_SIDE: u32 = 1024;
/// Images sent to the model are scaled down to this longer side first; larger ones only cost tokens.
const UPLOAD_SIDE: u32 = 1568;

const ILLUSTRATOR: &str = "You are the illustrator inside BioCartoon, an editor for scientific figures.";

/// Appended to the user's illustrator instructions: the app can only use a reply that follows these.
pub const SVG_RULES: &str = "Reply with one complete, self-contained SVG document and nothing else: \
    no prose, no Markdown fences. Give the root <svg> a viewBox plus width and height. Leave the \
    background transparent unless asked for one. Use only shapes, paths, gradients and <text>; no \
    external references, images, scripts or fonts other than sans-serif.";

const GENERATE: &str = "A clean, flat illustration of {prompt}";
const EDIT: &str = "Edit this image: {prompt}";

/// The prompt text the user can change in the AI settings.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AiPrompts {
    /// Wraps what is typed on the Generate page; `{prompt}` marks where it goes.
    pub generate: String,
    /// System prompt of the SVG-drawing models, before [`SVG_RULES`].
    pub illustrator: String,
    /// Wraps a change asked for in the "Edit with AI" dialog; `{prompt}` marks where it goes.
    pub edit: String,
    /// System prompt of the "Edit with AI" dialog's text replies.
    pub chat: String,
}

impl Default for AiPrompts {
    fn default() -> Self {
        Self { generate: GENERATE.into(), illustrator: ILLUSTRATOR.into(), edit: EDIT.into(), chat: CHAT_SYSTEM.into() }
    }
}

/// `template` with `{prompt}` replaced by `text`, or followed by it when there's no `{prompt}`.
fn fill(template: &str, text: &str) -> String {
    let t = template.trim();
    let s = if t.contains("{prompt}") { t.replace("{prompt}", text) } else { format!("{t} {text}") };
    s.trim().to_string()
}

impl AiPrompts {
    /// What to draw for `prompt`: `generate` with `{prompt}` filled in, or followed by it.
    fn subject(&self, prompt: &str) -> String {
        fill(&self.generate, prompt).trim_end_matches('.').to_string()
    }

    /// The ask for a custom edit, filled in the same way.
    fn custom_edit(&self, change: &str) -> String {
        fill(&self.edit, change)
    }

    fn chat_system(&self) -> &str {
        if self.chat.trim().is_empty() { CHAT_SYSTEM } else { self.chat.trim() }
    }

    fn svg_system(&self) -> String {
        format!("{} {SVG_RULES}", self.illustrator.trim()).trim_start().to_string()
    }
}

pub const CHAT_SYSTEM: &str = "You are the assistant in BioCartoon's \"Edit with AI\" dialog, where a \
    user asks for changes to one image in a scientific figure. Each request is applied separately \
    by having a model redraw the image as SVG, so you don't make the edit yourself: reply in one \
    to three plain sentences saying what is being changed, and ask a short question if the \
    request is unclear.";

impl SvgBackend {
    pub fn anthropic(model: &str, key: &str) -> Self {
        Self { api: Api::Anthropic, model: model.trim().to_string(), key: key.trim().to_string(), prompts: AiPrompts::default() }
    }

    pub fn openai(model: &str, key: &str) -> Self {
        Self { api: Api::OpenAi, model: model.trim().to_string(), key: key.trim().to_string(), prompts: AiPrompts::default() }
    }

    /// The server picks the model it was started with, and isn't sent a key.
    pub fn vllm(base: &str) -> Self {
        let base = base.trim().trim_end_matches('/').to_string();
        Self { api: Api::Vllm { base }, model: String::new(), key: String::new(), prompts: AiPrompts::default() }
    }

    pub fn with_prompts(self, prompts: &AiPrompts) -> Self {
        Self { prompts: prompts.clone(), ..self }
    }

    fn provider(&self) -> &'static str {
        match self.api {
            Api::Anthropic => "Anthropic",
            Api::OpenAi => "OpenAI",
            Api::Vllm { .. } => "vLLM",
        }
    }

    fn generate_request(&self, prompt: &str) -> Result<Request, String> {
        let text = format!("{}.", self.prompts.subject(prompt));
        self.request(&self.prompts.svg_system(), vec![Turn { user: true, text, image: None }])
    }

    fn edit_request(&self, image: &RgbaImage, svg: Option<&str>, op: &AiOp) -> Result<Request, String> {
        let ask = match op {
            AiOp::Restyle(s) => format!("Redraw this image in a {} style, keeping its subject and layout.", s.label().to_lowercase()),
            AiOp::RemoveBackground => "Redraw only the main subject of this image, with a transparent background.".into(),
            AiOp::RemoveText => "Redraw this image without any of its text or labels, keeping everything else.".into(),
            AiOp::Custom(p) => format!("{}. Redraw the whole image with this change.", self.prompts.custom_edit(p).trim_end_matches('.')),
        };
        let (w, h) = image.dimensions();
        let mut text = format!("{ask} Keep the aspect ratio ({w} x {h}).");
        // The model drew it (or it is an SVG upload): it can change the drawing itself.
        if let Some(svg) = svg {
            text.push_str(&format!(" Its current SVG source:\n{svg}"));
        }
        self.request(&self.prompts.svg_system(), vec![Turn { user: true, text, image: Some(png_base64(image)?) }])
    }

    fn chat_request(&self, history: &[(bool, String)], message: &str) -> Result<Request, String> {
        // The dialog's history starts with its own greeting and usually ends with `message`
        // already; the APIs want the first turn to be the user's.
        let mut turns: Vec<Turn> = history
            .iter()
            .skip_while(|(user, _)| !user)
            .map(|(user, text)| Turn { user: *user, text: text.clone(), image: None })
            .collect();
        if !turns.last().is_some_and(|t| t.user && t.text == message) {
            turns.push(Turn { user: true, text: message.into(), image: None });
        }
        self.request(self.prompts.chat_system(), turns)
    }

    fn request(&self, system: &str, turns: Vec<Turn>) -> Result<Request, String> {
        if self.model.is_empty() && !matches!(self.api, Api::Vllm { .. }) {
            return Err(format!("{}: no model is set", self.provider()));
        }
        let mut headers = vec![("content-type", "application/json".to_string())];
        let (url, body) = match &self.api {
            Api::Anthropic => {
                headers.push(("x-api-key", self.key.clone()));
                headers.push(("anthropic-version", "2023-06-01".into()));
                let messages: Vec<_> = turns
                    .into_iter()
                    .map(|t| {
                        let mut content = Vec::new();
                        if let Some(data) = t.image {
                            content.push(serde_json::json!({ "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": data } }));
                        }
                        content.push(serde_json::json!({ "type": "text", "text": t.text }));
                        serde_json::json!({ "role": if t.user { "user" } else { "assistant" }, "content": content })
                    })
                    .collect();
                let mut body = serde_json::json!({
                    "model": self.model,
                    "max_tokens": 16000,
                    "system": system,
                    "messages": messages,
                });
                // Retry a safety decline on Anthropic's recommended model (see `fallback_beta`).
                if let Some(beta) = fallback_beta(&self.model) {
                    headers.push(("anthropic-beta", beta.into()));
                    body["fallbacks"] = "default".into();
                }
                (ANTHROPIC_URL.to_string(), body)
            }
            Api::OpenAi | Api::Vllm { .. } => {
                let url = match &self.api {
                    Api::Vllm { base } if base.is_empty() => return Err("vLLM: no server URL is set".into()),
                    Api::Vllm { base } => format!("{}/v1/chat/completions", base.strip_suffix("/v1").unwrap_or(base)),
                    _ => OPENAI_CHAT_URL.to_string(),
                };
                if !self.key.is_empty() {
                    headers.push(("authorization", format!("Bearer {}", self.key)));
                }
                let mut messages = vec![serde_json::json!({ "role": "system", "content": system })];
                messages.extend(turns.into_iter().map(|t| {
                    let role = if t.user { "user" } else { "assistant" };
                    match t.image {
                        Some(data) => serde_json::json!({ "role": role, "content": [
                            { "type": "image_url", "image_url": { "url": format!("data:image/png;base64,{data}") } },
                            { "type": "text", "text": t.text },
                        ] }),
                        None => serde_json::json!({ "role": role, "content": t.text }),
                    }
                }));
                // No max tokens: context windows vary, and both default to what's left.
                let mut body = serde_json::json!({ "messages": messages });
                // vLLM falls back to the model it serves.
                if !self.model.is_empty() {
                    body["model"] = self.model.clone().into();
                }
                (url, body)
            }
        };
        Ok(Request { url, headers, body: body.to_string().into_bytes() })
    }

    /// The text of a response, or the server's error message.
    fn reply_text(&self, status: u16, body: &str) -> Result<String, String> {
        let p = self.provider();
        let v: serde_json::Value = serde_json::from_str(body).map_err(|_| format!("{p}: HTTP {status}"))?;
        if !(200..300).contains(&status) {
            let msg = v["error"]["message"].as_str().or(v["message"].as_str()).or(v["detail"].as_str()).unwrap_or("request failed");
            return Err(format!("{p} ({status}): {msg}"));
        }
        let text = match self.api {
            Api::Anthropic => {
                match v["stop_reason"].as_str() {
                    Some("refusal") => return Err("Claude declined this request.".into()),
                    Some("max_tokens") => return Err("Claude's reply was cut off; try a simpler request.".into()),
                    _ => {}
                }
                v["content"].as_array().into_iter().flatten().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect()
            }
            Api::OpenAi | Api::Vllm { .. } => {
                let choice = &v["choices"][0];
                if choice["finish_reason"] == "length" {
                    return Err("The model's reply was cut off; try a simpler request or a model with a longer context.".into());
                }
                let text = choice["message"]["content"].as_str().unwrap_or_default();
                // Reasoning models may think out loud first; keep only the answer.
                text.rsplit_once("</think>").map_or(text, |(_, answer)| answer).to_string()
            }
        };
        if text.trim().is_empty() {
            return Err(format!("{p}: the model returned an empty reply."));
        }
        Ok(text)
    }

}

struct Request {
    url: String,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
}

/// The beta header for `fallbacks: "default"`, on the models that accept it.
fn fallback_beta(model: &str) -> Option<&'static str> {
    ["claude-fable-5-1", "claude-opus-5-5", "claude-opus-5", "claude-sonnet-5-5"]
        .contains(&model)
        .then_some("server-side-fallback-2026-07-01")
}

fn png_base64(image: &RgbaImage) -> Result<String, String> {
    use base64::Engine;
    Ok(base64::engine::general_purpose::STANDARD.encode(png_bytes(image)?))
}

/// `image` as a PNG, scaled down to [`UPLOAD_SIDE`] if it's larger.
fn png_bytes(image: &RgbaImage) -> Result<Vec<u8>, String> {
    let (w, h) = image.dimensions();
    let scaled;
    let image = if w.max(h) > UPLOAD_SIDE {
        let s = UPLOAD_SIDE as f32 / w.max(h) as f32;
        let (sw, sh) = (((w as f32 * s) as u32).max(1), ((h as f32 * s) as u32).max(1));
        scaled = image::imageops::resize(image, sw, sh, image::imageops::FilterType::Triangle);
        &scaled
    } else {
        image
    };
    let mut png = Vec::new();
    image.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(|e| format!("encode image: {e}"))?;
    Ok(png)
}

/// Pull the SVG out of a reply and render it so its longer side is `side` pixels, keeping the SVG.
fn render_reply(text: &str, side: u32) -> Result<AiImage, String> {
    let start = text.find("<svg").ok_or("The model did not return an SVG drawing.")?;
    let end = text.rfind("</svg>").map(|i| i + "</svg>".len()).ok_or("The model's SVG drawing was incomplete.")?;
    let svg = &text[start..end.max(start)];
    Ok(AiImage { image: crate::export::render_svg(svg.as_bytes(), side)?, svg: Some(svg.to_string()) })
}

fn edit_side(image: &RgbaImage) -> u32 {
    image.width().max(image.height()).clamp(512, 2048)
}

impl HttpBackend for SvgBackend {
    fn name(&self) -> String {
        match &self.api {
            Api::Vllm { base } => format!("vLLM at {base}"),
            _ => format!("{} {}", self.provider(), self.model),
        }
    }
    fn request(&self, job: &AiJob) -> Result<Request, String> {
        match job {
            AiJob::Generate { prompt } => self.generate_request(prompt),
            AiJob::Edit { image, svg, op, .. } => self.edit_request(image, svg.as_deref(), op),
            AiJob::Chat { history, message, .. } => self.chat_request(history, message),
        }
    }
    fn image(&self, job: &AiJob, status: u16, body: &[u8]) -> Result<AiImage, String> {
        let text = self.reply_text(status, &String::from_utf8_lossy(body))?;
        match job {
            AiJob::Edit { image, .. } => render_reply(&text, edit_side(image)),
            _ => render_reply(&text, GENERATE_SIDE),
        }
    }
    fn text(&self, status: u16, body: &[u8]) -> Result<String, String> {
        self.reply_text(status, &String::from_utf8_lossy(body))
    }
}

/// OpenAI's Images API (the gpt-image models).
pub struct OpenAiBackend {
    model: String,
    key: String,
    prompts: AiPrompts,
}

impl OpenAiBackend {
    pub fn new(model: &str, key: &str) -> Self {
        Self { model: model.trim().to_string(), key: key.trim().to_string(), prompts: AiPrompts::default() }
    }

    pub fn with_prompts(self, prompts: &AiPrompts) -> Self {
        Self { prompts: prompts.clone(), ..self }
    }

    /// gpt-image-2 doesn't take `background: transparent` (outside its preview); the others do.
    fn transparent(&self) -> bool {
        self.model.starts_with("gpt-image-1") || self.model.starts_with("gpt-image-2.5")
    }

    fn backdrop(&self) -> &'static str {
        if self.transparent() { "on a transparent background" } else { WHITE_BACKDROP }
    }
}

impl HttpBackend for OpenAiBackend {
    fn name(&self) -> String {
        format!("OpenAI {}", self.model)
    }
    const CHATS: bool = false;

    fn request(&self, job: &AiJob) -> Result<Request, String> {
        let auth = ("authorization", format!("Bearer {}", self.key));
        match job {
            AiJob::Generate { prompt } => {
                let mut body = serde_json::json!({
                    "model": self.model,
                    "prompt": format!("{}, {}.", self.prompts.subject(prompt), self.backdrop()),
                    "size": "1024x1024",
                    "output_format": "png",
                    "n": 1,
                });
                if self.transparent() {
                    body["background"] = "transparent".into();
                }
                Ok(Request {
                    url: "https://api.openai.com/v1/images/generations".into(),
                    headers: vec![auth, ("content-type", "application/json".into())],
                    body: body.to_string().into_bytes(),
                })
            }
            AiJob::Edit { image, op, .. } => {
                let clear = wants_clear(job);
                let prompt = match op {
                    AiOp::RemoveBackground => format!("Keep only the main subject of this image, exactly as it is, {}.", self.backdrop()),
                    _ if clear => format!("{} Keep it {}.", edit_ask(op, &self.prompts), self.backdrop()),
                    _ => edit_ask(op, &self.prompts),
                };
                let mut form = Multipart::new();
                form.text("model", &self.model);
                form.text("prompt", &prompt);
                form.text("size", "auto");
                form.text("output_format", "png");
                if clear && self.transparent() {
                    form.text("background", "transparent");
                }
                // Keeps the subject recognizable; gpt-image-1-mini only takes "low".
                if self.model.starts_with("gpt-image-1") && !self.model.contains("mini") {
                    form.text("input_fidelity", "high");
                }
                form.file("image", "image.png", "image/png", &png_bytes(image)?);
                let (content_type, body) = form.finish();
                Ok(Request { url: "https://api.openai.com/v1/images/edits".into(), headers: vec![auth, ("content-type", content_type)], body })
            }
            AiJob::Chat { .. } => Err("OpenAI image models don't chat".into()),
        }
    }

    fn image(&self, job: &AiJob, status: u16, body: &[u8]) -> Result<AiImage, String> {
        let v = json_reply("OpenAI", status, body)?;
        let b64 = v["data"][0]["b64_json"].as_str().ok_or("OpenAI returned no image.")?;
        finish_image(job, decode_image(b64)?).map(Into::into)
    }
}

/// Gemini's native image models through `generateContent`. They don't make transparent images,
/// so a transparent result is drawn on white and the white cleared afterwards.
pub struct GeminiBackend {
    model: String,
    key: String,
    prompts: AiPrompts,
}

impl GeminiBackend {
    pub fn new(model: &str, key: &str) -> Self {
        Self { model: model.trim().to_string(), key: key.trim().to_string(), prompts: AiPrompts::default() }
    }

    pub fn with_prompts(self, prompts: &AiPrompts) -> Self {
        Self { prompts: prompts.clone(), ..self }
    }
}

impl HttpBackend for GeminiBackend {
    fn name(&self) -> String {
        format!("Gemini {}", self.model)
    }
    const CHATS: bool = false;

    fn request(&self, job: &AiJob) -> Result<Request, String> {
        let parts = match job {
            AiJob::Generate { prompt } => {
                vec![serde_json::json!({ "text": format!("Draw {}, {WHITE_BACKDROP}.", lower_first(&self.prompts.subject(prompt))) })]
            }
            AiJob::Edit { image, op, .. } => {
                let text = match op {
                    AiOp::RemoveBackground => format!("Keep only the main subject of this image, exactly as it is, {WHITE_BACKDROP}."),
                    _ if wants_clear(job) => format!("{} Keep it {WHITE_BACKDROP}.", edit_ask(op, &self.prompts)),
                    _ => edit_ask(op, &self.prompts),
                };
                vec![
                    serde_json::json!({ "inline_data": { "mime_type": "image/png", "data": png_base64(image)? } }),
                    serde_json::json!({ "text": text }),
                ]
            }
            AiJob::Chat { .. } => return Err("Gemini image models don't chat".into()),
        };
        let body = serde_json::json!({
            "contents": [{ "role": "user", "parts": parts }],
            "generationConfig": { "responseModalities": ["TEXT", "IMAGE"] },
        });
        Ok(Request {
            url: format!("https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent", self.model),
            headers: vec![("x-goog-api-key", self.key.clone()), ("content-type", "application/json".into())],
            body: body.to_string().into_bytes(),
        })
    }

    fn image(&self, job: &AiJob, status: u16, body: &[u8]) -> Result<AiImage, String> {
        let v = json_reply("Gemini", status, body)?;
        if let Some(reason) = v["promptFeedback"]["blockReason"].as_str() {
            return Err(format!("Gemini blocked this request ({reason})."));
        }
        let parts = v["candidates"][0]["content"]["parts"].as_array().cloned().unwrap_or_default();
        // The response is camelCase; accept snake_case too.
        let data = parts.iter().find_map(|p| p["inlineData"]["data"].as_str().or(p["inline_data"]["data"].as_str()));
        match data {
            Some(b64) => finish_image(job, decode_image(b64)?).map(Into::into),
            None => {
                let said: String = parts.iter().filter_map(|p| p["text"].as_str()).collect();
                let reason = v["candidates"][0]["finishReason"].as_str().unwrap_or_default();
                Err(match (said.trim(), reason) {
                    ("", "") => "Gemini returned no image.".into(),
                    ("", r) => format!("Gemini returned no image ({r})."),
                    (s, _) => format!("Gemini returned no image: {s}"),
                })
            }
        }
    }
}

pub const WHITE_BACKDROP: &str = "on a plain, pure white (#FFFFFF) background with nothing else on it";

/// "A cell" to "a cell", so it reads after "Draw".
fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) if !c.as_str().starts_with(|n: char| n.is_uppercase()) => f.to_lowercase().chain(c).collect(),
        _ => s.to_string(),
    }
}

fn edit_ask(op: &AiOp, prompts: &AiPrompts) -> String {
    match op {
        AiOp::Restyle(s) => format!("Redraw this image in a {} style, keeping its subject and layout.", s.label().to_lowercase()),
        AiOp::RemoveBackground => "Keep only the main subject of this image, exactly as it is.".into(),
        AiOp::RemoveText => "Remove all text and labels from this image, keeping everything else exactly as it is.".into(),
        AiOp::Custom(p) => prompts.custom_edit(p),
    }
}

/// Whether the result should come back with a transparent background: always for new images
/// and Remove background, and for edits of an image that has one.
fn wants_clear(job: &AiJob) -> bool {
    match job {
        AiJob::Generate { .. } => true,
        AiJob::Edit { op: AiOp::RemoveBackground, .. } => true,
        AiJob::Edit { image, .. } => has_clear_corner(image),
        AiJob::Chat { .. } => false,
    }
}

fn has_clear_corner(img: &RgbaImage) -> bool {
    let (w, h) = img.dimensions();
    w > 0 && h > 0 && [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)].iter().any(|&(x, y)| img.get_pixel(x, y)[3] < 128)
}

/// Clears the backdrop of an opaque result when the job wanted transparency.
fn finish_image(job: &AiJob, mut img: RgbaImage) -> Result<RgbaImage, String> {
    if wants_clear(job) && !has_clear_corner(&img) {
        clear_backdrop(&mut img);
    }
    Ok(img)
}

/// Make the near-white region connected to the image's border transparent, with a soft edge.
fn clear_backdrop(img: &mut RgbaImage) {
    let (w, h) = img.dimensions();
    let white = |p: &Rgba<u8>| p[0].min(p[1]).min(p[2]) >= 235;
    let mut seen = vec![false; (w * h) as usize];
    let mut stack: Vec<(u32, u32)> = Vec::new();
    for x in 0..w {
        stack.extend([(x, 0), (x, h - 1)]);
    }
    for y in 0..h {
        stack.extend([(0, y), (w - 1, y)]);
    }
    while let Some((x, y)) = stack.pop() {
        let i = (y * w + x) as usize;
        if seen[i] || !white(img.get_pixel(x, y)) {
            continue;
        }
        seen[i] = true;
        img.get_pixel_mut(x, y)[3] = 0;
        if x > 0 { stack.push((x - 1, y)); }
        if x + 1 < w { stack.push((x + 1, y)); }
        if y > 0 { stack.push((x, y - 1)); }
        if y + 1 < h { stack.push((x, y + 1)); }
    }
    // Light pixels along the cut are anti-aliasing against white: fade them by how light they are.
    for y in 0..h {
        for x in 0..w {
            if seen[(y * w + x) as usize] {
                continue;
            }
            let touches = [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)]
                .iter()
                .any(|&(a, b)| a < w && b < h && seen[(b * w + a) as usize]);
            let p = img.get_pixel_mut(x, y);
            let light = p[0].min(p[1]).min(p[2]);
            if touches && light > 180 {
                p[3] = ((255 - light) as u32 * 255 / 75).min(255) as u8;
            }
        }
    }
}

fn decode_image(b64: &str) -> Result<RgbaImage, String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64.trim()).map_err(|e| format!("decode image: {e}"))?;
    Ok(image::load_from_memory(&bytes).map_err(|e| format!("decode image: {e}"))?.to_rgba8())
}

/// Parse a JSON response, turning an HTTP error into the API's own message.
fn json_reply(provider: &str, status: u16, body: &[u8]) -> Result<serde_json::Value, String> {
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|_| format!("{provider}: HTTP {status}"))?;
    if !(200..300).contains(&status) {
        let msg = v["error"]["message"].as_str().unwrap_or("request failed");
        return Err(format!("{provider} ({status}): {msg}"));
    }
    Ok(v)
}

/// A multipart/form-data body, for OpenAI's image edits.
struct Multipart {
    boundary: String,
    body: Vec<u8>,
}

impl Multipart {
    fn new() -> Self {
        let boundary = format!("biocartoon-{:016x}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default());
        Self { boundary, body: Vec::new() }
    }
    fn text(&mut self, name: &str, value: &str) {
        let part = format!("--{}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n", self.boundary);
        self.body.extend_from_slice(part.as_bytes());
    }
    fn file(&mut self, name: &str, filename: &str, mime: &str, bytes: &[u8]) {
        let head = format!(
            "--{}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\nContent-Type: {mime}\r\n\r\n",
            self.boundary
        );
        self.body.extend_from_slice(head.as_bytes());
        self.body.extend_from_slice(bytes);
        self.body.extend_from_slice(b"\r\n");
    }
    /// The content-type header and the body.
    fn finish(mut self) -> (String, Vec<u8>) {
        self.body.extend_from_slice(format!("--{}--\r\n", self.boundary).as_bytes());
        (format!("multipart/form-data; boundary={}", self.boundary), self.body)
    }
}

/// A backend that answers each job with one HTTP request. [`Http`] turns it into an
/// [`AiBackend`], sending the request from the worker thread on desktop and with ehttp in the
/// browser.
trait HttpBackend: Send + Sync + 'static {
    fn name(&self) -> String;
    /// False for image-only models: the Edit with AI chat then answers locally.
    const CHATS: bool = true;
    fn request(&self, job: &AiJob) -> Result<Request, String>;
    /// The image a Generate or Edit job's response holds.
    fn image(&self, job: &AiJob, status: u16, body: &[u8]) -> Result<AiImage, String>;
    /// The reply a Chat job's response holds.
    fn text(&self, _status: u16, _body: &[u8]) -> Result<String, String> {
        Err("this model doesn't chat".into())
    }
}

struct Http<B>(Arc<B>);

impl<B: HttpBackend> Http<B> {
    fn finish(backend: &B, job: AiJob, res: Result<(u16, Vec<u8>), String>) -> AiResult {
        match job {
            AiJob::Generate { ref prompt } => {
                let result = res.and_then(|(status, body)| backend.image(&job, status, &body));
                AiResult::Generated { prompt: prompt.clone(), result }
            }
            AiJob::Edit { item, ref op, .. } => {
                let result = res.and_then(|(status, body)| backend.image(&job, status, &body));
                AiResult::Edited { item, op: op.clone(), result }
            }
            AiJob::Chat { item, .. } => AiResult::ChatReply { item, result: res.and_then(|(status, body)| backend.text(status, &body)) },
        }
    }

    /// Desktop: run `job` on this thread.
    fn block(&self, job: AiJob) -> AiResult {
        let (tx, rx) = channel();
        self.run(job, Box::new(move |r| {
            let _ = tx.send(r);
        }));
        rx.recv().expect("ai job finished")
    }
}

impl<B: HttpBackend> AiBackend for Http<B> {
    fn name(&self) -> String {
        self.0.name()
    }
    fn generate(&self, prompt: &str) -> Result<AiImage, String> {
        match self.block(AiJob::Generate { prompt: prompt.into() }) {
            AiResult::Generated { result, .. } => result,
            _ => unreachable!(),
        }
    }
    fn edit(&self, image: &RgbaImage, svg: Option<&str>, op: &AiOp) -> Result<AiImage, String> {
        match self.block(AiJob::Edit { item: Default::default(), image: image.clone(), svg: svg.map(Into::into), op: op.clone() }) {
            AiResult::Edited { result, .. } => result,
            _ => unreachable!(),
        }
    }
    fn chat(&self, history: &[(bool, String)], message: &str) -> Result<String, String> {
        match self.block(AiJob::Chat { item: Default::default(), history: history.to_vec(), message: message.into() }) {
            AiResult::ChatReply { result, .. } => result,
            _ => unreachable!(),
        }
    }

    fn run(&self, job: AiJob, done: Box<dyn FnOnce(AiResult) + Send>) {
        if let AiJob::Chat { item, message, .. } = &job {
            if !B::CHATS {
                let reply = format!("Working on it: \"{message}\". The edited image replaces this one when it's ready.");
                return done(AiResult::ChatReply { item: *item, result: Ok(reply) });
            }
        }
        let req = match self.0.request(&job) {
            Ok(r) => r,
            Err(e) => return done(Self::finish(&self.0, job, Err(e))),
        };
        let backend = self.0.clone();
        send(req, Box::new(move |res| done(Self::finish(&backend, job, res))));
    }
}

/// POST `req` and hand `done` the status and body, or why it couldn't be sent.
#[cfg(not(target_arch = "wasm32"))]
fn send(req: Request, done: Box<dyn FnOnce(Result<(u16, Vec<u8>), String>) + Send>) {
    let host = req.url.split('/').nth(2).unwrap_or_default().to_string();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(600)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut post = agent.post(&req.url);
    for (k, v) in req.headers {
        post = post.header(k, v);
    }
    done(post.send(req.body).map_err(|e| format!("{host}: {e}")).and_then(|mut res| {
        let status = res.status().as_u16();
        let body = res.body_mut().with_config().limit(256 * 1024 * 1024).read_to_vec().map_err(|e| format!("{host}: {e}"))?;
        Ok((status, body))
    }));
}

#[cfg(target_arch = "wasm32")]
fn send(req: Request, done: Box<dyn FnOnce(Result<(u16, Vec<u8>), String>) + Send>) {
    let host = req.url.split('/').nth(2).unwrap_or_default().to_string();
    let mut post = ehttp::Request::post(&req.url, req.body);
    for (k, v) in req.headers {
        post.headers.insert(k, v);
    }
    if host == "api.anthropic.com" {
        // Lets the API accept a key sent from a web page (it is the user's own key).
        post.headers.insert("anthropic-dangerous-direct-browser-access", "true");
    }
    ehttp::fetch(post, move |res| done(res.map(|r| (r.status, r.bytes)).map_err(|e| format!("{host}: {e}"))));
}

/// `base_url` is used only by vLLM, which ignores `model` and `key`.
pub fn make_backend(kind: BackendKind, model: &str, key: &str, base_url: &str, prompts: &AiPrompts) -> Arc<dyn AiBackend> {
    if std::env::var("BIOCARTOON_AI").as_deref() == Ok("placeholder") {
        return Arc::new(PlaceholderBackend);
    }
    match kind {
        BackendKind::OpenAi if !kind.draws_svg(model) => Arc::new(Http(Arc::new(OpenAiBackend::new(model, key).with_prompts(prompts)))),
        BackendKind::OpenAi => Arc::new(Http(Arc::new(SvgBackend::openai(model, key).with_prompts(prompts)))),
        BackendKind::Gemini => Arc::new(Http(Arc::new(GeminiBackend::new(model, key).with_prompts(prompts)))),
        BackendKind::Anthropic => Arc::new(Http(Arc::new(SvgBackend::anthropic(model, key).with_prompts(prompts)))),
        BackendKind::Vllm => Arc::new(Http(Arc::new(SvgBackend::vllm(base_url).with_prompts(prompts)))),
    }
}

pub enum AiJob {
    Generate { prompt: String },
    /// `svg` is the image's source when it is an SVG, for the models that draw SVG.
    Edit { item: crate::doc::ItemId, image: RgbaImage, svg: Option<String>, op: AiOp },
    Chat { item: crate::doc::ItemId, history: Vec<(bool, String)>, message: String },
}

pub enum AiResult {
    Generated { prompt: String, result: Result<AiImage, String> },
    Edited { item: crate::doc::ItemId, op: AiOp, result: Result<AiImage, String> },
    ChatReply { item: crate::doc::ItemId, result: Result<String, String> },
}

/// Runs AI jobs on a worker thread.
pub struct AiClient {
    tx: Sender<(Arc<dyn AiBackend>, AiJob)>,
    rx: Receiver<AiResult>,
    #[cfg(target_arch = "wasm32")]
    rtx: Sender<AiResult>,
    pub backend: Arc<dyn AiBackend>,
    pub busy: usize,
}

impl AiClient {
    pub fn new(kind: BackendKind, model: &str, key: &str, base_url: &str, prompts: &AiPrompts) -> Self {
        let (tx, jrx) = channel::<(Arc<dyn AiBackend>, AiJob)>();
        let (rtx, rx) = channel();
        // One long-lived worker; jobs run in order. (On the web, `spawn` runs this loop only
        // after `submit` has queued work, so there `submit` drives it instead, see below.)
        #[cfg(not(target_arch = "wasm32"))]
        thread::Builder::new()
            .name("ai-worker".into())
            .spawn(move || {
                for (backend, job) in jrx {
                    let rtx = rtx.clone();
                    backend.run(job, Box::new(move |r| {
                        let _ = rtx.send(r);
                    }));
                }
            })
            .expect("spawn ai worker");
        #[cfg(target_arch = "wasm32")]
        let _ = jrx;
        #[cfg(target_arch = "wasm32")]
        return Self { tx, rx, rtx, backend: make_backend(kind, model, key, base_url, prompts), busy: 0 };
        #[cfg(not(target_arch = "wasm32"))]
        Self { tx, rx, backend: make_backend(kind, model, key, base_url, prompts), busy: 0 }
    }

    pub fn set_backend(&mut self, kind: BackendKind, model: &str, key: &str, base_url: &str, prompts: &AiPrompts) {
        self.backend = make_backend(kind, model, key, base_url, prompts);
    }

    pub fn submit(&mut self, job: AiJob) {
        self.busy += 1;
        #[cfg(not(target_arch = "wasm32"))]
        let _ = self.tx.send((self.backend.clone(), job));
        // No threads in the browser: the backend runs the job now, or asynchronously.
        #[cfg(target_arch = "wasm32")]
        {
            let _ = &self.tx;
            let rtx = self.rtx.clone();
            self.backend.run(job, Box::new(move |r| {
                let _ = rtx.send(r);
            }));
        }
    }

    pub fn poll(&mut self) -> Vec<AiResult> {
        let mut out = Vec::new();
        while let Ok(r) = self.rx.try_recv() {
            self.busy = self.busy.saturating_sub(1);
            out.push(r);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_reply_parsing() {
        let ok = r#"{"stop_reason":"end_turn","content":[{"type":"thinking","thinking":""},{"type":"text","text":"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 20 10\" width=\"20\" height=\"10\"><rect width=\"20\" height=\"10\" fill=\"red\"/></svg>"}]}"#;
        let err = r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
        let b = SvgBackend::anthropic("claude-opus-5-5", "k");
        let made = render_reply(&b.reply_text(200, ok).unwrap(), 200).unwrap();
        assert_eq!(made.image.dimensions(), (200, 100));
        // The drawing is kept, to be saved as an SVG.
        let svg = made.svg.unwrap();
        assert!(svg.starts_with("<svg ") && svg.ends_with("</svg>"));
        assert_eq!(b.reply_text(401, err).unwrap_err(), "Anthropic (401): invalid x-api-key");
        let req = b.chat_request(&[(false, "Hi".into()), (true, "make it blue".into())], "make it blue").unwrap();
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["messages"].as_array().unwrap().len(), 1);
        assert_eq!(body["fallbacks"], "default");
    }

    #[test]
    fn vllm_request_and_reply() {
        let b = SvgBackend::vllm("http://localhost:8000/v1/");
        let req = b.edit_request(&RgbaImage::new(4, 2), Some("<svg/>"), &AiOp::RemoveText).unwrap();
        assert_eq!(req.url, "http://localhost:8000/v1/chat/completions");
        assert!(req.headers.iter().all(|(k, _)| *k != "authorization"));
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert!(body.get("model").is_none());
        assert_eq!(body["messages"][0]["role"], "system");
        assert!(body["messages"][1]["content"][0]["image_url"]["url"].as_str().unwrap().starts_with("data:image/png;base64,"));
        assert!(body["messages"][1]["content"][1]["text"].as_str().unwrap().ends_with("Its current SVG source:\n<svg/>"));
        let ok = r#"{"choices":[{"finish_reason":"stop","message":{"content":"<think>maybe <svg></think><svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\"><rect width=\"10\" height=\"10\"/></svg>"}}]}"#;
        assert!(render_reply(&b.reply_text(200, ok).unwrap(), 10).is_ok());
        let err = r#"{"object":"error","message":"The model `x` does not exist.","code":404}"#;
        assert_eq!(b.reply_text(404, err).unwrap_err(), "vLLM (404): The model `x` does not exist.");
    }

    #[test]
    fn openai_chat_models_draw_svg() {
        assert_eq!(make_backend(BackendKind::OpenAi, "gpt-image-1.5", "k", "", &AiPrompts::default()).name(), "OpenAI gpt-image-1.5");
        assert_eq!(make_backend(BackendKind::Vllm, "", "", "http://gpu:8000", &AiPrompts::default()).name(), "vLLM at http://gpu:8000");
        let b = SvgBackend::openai("gpt-6-sol", "k");
        let req = b.generate_request("a cell").unwrap();
        assert_eq!(req.url, OPENAI_CHAT_URL);
        assert!(req.headers.iter().any(|(k, v)| *k == "authorization" && v == "Bearer k"));
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["model"], "gpt-6-sol");
    }

    #[test]
    fn openai_requests_and_reply() {
        let b = OpenAiBackend::new("gpt-image-1.5", "k");
        let req = b.request(&AiJob::Generate { prompt: "a cell".into() }).unwrap();
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["background"], "transparent");
        let mut icon = RgbaImage::new(4, 4);
        icon.put_pixel(1, 1, Rgba([255, 0, 0, 255]));
        let job = AiJob::Edit { item: 1, image: icon, svg: None, op: AiOp::RemoveText };
        let req = b.request(&job).unwrap();
        let ct = &req.headers.iter().find(|(k, _)| *k == "content-type").unwrap().1;
        let boundary = ct.strip_prefix("multipart/form-data; boundary=").unwrap();
        let text = String::from_utf8_lossy(&req.body);
        assert!(text.contains("name=\"background\"\r\n\r\ntransparent\r\n"));
        assert!(text.contains("name=\"image\"; filename=\"image.png\"\r\nContent-Type: image/png"));
        assert!(text.ends_with(&format!("--{boundary}--\r\n")));
        // An opaque result for a job that wanted transparency gets its white backdrop cleared.
        let mut png = Vec::new();
        RgbaImage::from_pixel(8, 8, Rgba([255, 255, 255, 255])).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        use base64::Engine;
        let reply = format!(r#"{{"data":[{{"b64_json":"{}"}}]}}"#, base64::engine::general_purpose::STANDARD.encode(png));
        let made = b.image(&job, 200, reply.as_bytes()).unwrap();
        assert_eq!(made.image.get_pixel(4, 4)[3], 0);
        assert!(made.svg.is_none());
        let err = br#"{"error":{"message":"Your organization must be verified","type":"invalid_request_error"}}"#;
        assert_eq!(b.image(&job, 403, err).unwrap_err(), "OpenAI (403): Your organization must be verified");
    }

    #[test]
    fn gemini_reply() {
        let b = GeminiBackend::new("gemini-3.1-flash-image", "k");
        let job = AiJob::Generate { prompt: "a cell".into() };
        let req = b.request(&job).unwrap();
        assert!(req.url.ends_with("/models/gemini-3.1-flash-image:generateContent"));
        let blocked = br#"{"promptFeedback":{"blockReason":"SAFETY"}}"#;
        assert_eq!(b.image(&job, 200, blocked).unwrap_err(), "Gemini blocked this request (SAFETY).");
        let text_only = br#"{"candidates":[{"content":{"parts":[{"text":"I can't draw that."}]},"finishReason":"STOP"}]}"#;
        assert_eq!(b.image(&job, 200, text_only).unwrap_err(), "Gemini returned no image: I can't draw that.");
    }

    #[test]
    fn prompts_wrap_the_subject() {
        let mut p = AiPrompts::default();
        assert_eq!(p.subject("a cell"), "A clean, flat illustration of a cell");
        p.generate = "A biology figure.".into();
        assert_eq!(p.subject("a pump"), "A biology figure. a pump");
        p.generate = "Textbook drawing of {prompt}.".into();
        assert_eq!(p.subject("a pump"), "Textbook drawing of a pump");
        p.illustrator = "Draw for biologists.".into();
        let b = SvgBackend::anthropic("m", "k").with_prompts(&p);
        let body: serde_json::Value = serde_json::from_slice(&b.generate_request("a pump").unwrap().body).unwrap();
        assert!(body["system"].as_str().unwrap().starts_with("Draw for biologists. Reply with one complete"));
        assert_eq!(body["messages"][0]["content"][0]["text"], "Textbook drawing of a pump.");
        let req = GeminiBackend::new("g", "k").with_prompts(&AiPrompts::default()).request(&AiJob::Generate { prompt: "a cell".into() }).unwrap();
        assert!(String::from_utf8_lossy(&req.body).contains("Draw a clean, flat illustration of a cell, on a plain"));
        p.edit = "Make this change, no labels: {prompt}".into();
        assert_eq!(edit_ask(&AiOp::Custom("bigger".into()), &p), "Make this change, no labels: bigger");
        p.chat = "Be brief.".into();
        let req = b.with_prompts(&p).chat_request(&[], "hi").unwrap();
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["system"], "Be brief.");
    }

    #[test]
    fn backdrop_clearing_keeps_the_subject() {
        let mut img = RgbaImage::from_pixel(10, 10, Rgba([255, 255, 255, 255]));
        for (x, y) in [(4, 4), (5, 4), (4, 5), (5, 5)] {
            img.put_pixel(x, y, Rgba([20, 40, 200, 255]));
        }
        clear_backdrop(&mut img);
        assert_eq!(img.get_pixel(0, 0)[3], 0);
        assert_eq!(img.get_pixel(4, 4)[3], 255);
    }
}
