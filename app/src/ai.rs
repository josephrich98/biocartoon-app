//! AI backend abstraction. The hosted providers in [`BackendKind`] are not wired up yet; implement
//! [`AiBackend`] for one and return it from [`make_backend`]. `BIOCARTOON_AI=placeholder` swaps in
//! a deterministic offline backend for development.

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

/// Hosted model providers offered in the AI setup dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BackendKind {
    OpenAi,
    Gemini,
    Anthropic,
}

impl BackendKind {
    pub const ALL: [BackendKind; 3] = [BackendKind::OpenAi, BackendKind::Gemini, BackendKind::Anthropic];
    pub fn label(self) -> &'static str {
        match self {
            BackendKind::OpenAi => "OpenAI",
            BackendKind::Gemini => "Google Gemini",
            BackendKind::Anthropic => "Anthropic",
        }
    }
    /// Suggested model ids; the field stays editable.
    pub fn models(self) -> &'static [&'static str] {
        match self {
            BackendKind::OpenAi => &["gpt-image-1"],
            BackendKind::Gemini => &["gemini-2.5-flash-image"],
            BackendKind::Anthropic => &["claude-opus-5-5", "claude-sonnet-5-5"],
        }
    }
    pub fn key_url(self) -> &'static str {
        match self {
            BackendKind::OpenAi => "https://platform.openai.com/api-keys",
            BackendKind::Gemini => "https://aistudio.google.com/apikey",
            BackendKind::Anthropic => "https://console.anthropic.com/settings/keys",
        }
    }
}

pub trait AiBackend: Send + Sync {
    fn name(&self) -> String;
    /// Text-to-image. Returns an RGBA image with transparent background where possible.
    fn generate(&self, prompt: &str) -> Result<RgbaImage, String>;
    /// Image-to-image edit.
    fn edit(&self, image: &RgbaImage, op: &AiOp) -> Result<RgbaImage, String>;
    /// Chat reply used by the Custom edit dialog.
    fn chat(&self, history: &[(bool, String)], message: &str) -> Result<String, String>;
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

    fn generate(&self, prompt: &str) -> Result<RgbaImage, String> {
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
        Ok(img)
    }

    fn edit(&self, image: &RgbaImage, op: &AiOp) -> Result<RgbaImage, String> {
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
        Ok(out)
    }

    fn chat(&self, _history: &[(bool, String)], message: &str) -> Result<String, String> {
        Ok(format!(
            "No model is connected, so I can't apply \"{message}\" yet. The placeholder backend \
             only supports Remove background and the simple restyles."
        ))
    }
}

/// Not yet implemented backends return a clear error instead of pretending.
pub struct UnimplementedBackend(pub BackendKind);

impl AiBackend for UnimplementedBackend {
    fn name(&self) -> String {
        format!("{} (not connected yet)", self.0.label())
    }
    fn generate(&self, _: &str) -> Result<RgbaImage, String> {
        Err(format!("{} is not wired up yet. See app/src/ai.rs.", self.0.label()))
    }
    fn edit(&self, _: &RgbaImage, _: &AiOp) -> Result<RgbaImage, String> {
        Err(format!("{} is not wired up yet. See app/src/ai.rs.", self.0.label()))
    }
    fn chat(&self, _: &[(bool, String)], _: &str) -> Result<String, String> {
        Err(format!("{} is not wired up yet. See app/src/ai.rs.", self.0.label()))
    }
}

pub fn make_backend(kind: BackendKind) -> Arc<dyn AiBackend> {
    if std::env::var("BIOCARTOON_AI").as_deref() == Ok("placeholder") {
        return Arc::new(PlaceholderBackend);
    }
    Arc::new(UnimplementedBackend(kind))
}

pub enum AiJob {
    Generate { prompt: String },
    Edit { item: crate::doc::ItemId, image: RgbaImage, op: AiOp },
    Chat { item: crate::doc::ItemId, history: Vec<(bool, String)>, message: String },
}

pub enum AiResult {
    Generated { prompt: String, result: Result<RgbaImage, String> },
    Edited { item: crate::doc::ItemId, op: AiOp, result: Result<RgbaImage, String> },
    ChatReply { item: crate::doc::ItemId, result: Result<String, String> },
}

fn run_job(backend: &dyn AiBackend, job: AiJob) -> AiResult {
    match job {
        AiJob::Generate { prompt } => {
            let result = backend.generate(&prompt);
            AiResult::Generated { prompt, result }
        }
        AiJob::Edit { item, image, op } => {
            let result = backend.edit(&image, &op);
            AiResult::Edited { item, op, result }
        }
        AiJob::Chat { item, history, message } => {
            let result = backend.chat(&history, &message);
            AiResult::ChatReply { item, result }
        }
    }
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
    pub fn new(kind: BackendKind) -> Self {
        let (tx, jrx) = channel::<(Arc<dyn AiBackend>, AiJob)>();
        let (rtx, rx) = channel();
        // One long-lived worker; jobs run in order. (On the web, `spawn` runs this loop only
        // after `submit` has queued work, so there `submit` drives it instead, see below.)
        #[cfg(not(target_arch = "wasm32"))]
        thread::Builder::new()
            .name("ai-worker".into())
            .spawn(move || {
                for (backend, job) in jrx {
                    if rtx.send(run_job(&*backend, job)).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn ai worker");
        #[cfg(target_arch = "wasm32")]
        let _ = jrx;
        #[cfg(target_arch = "wasm32")]
        return Self { tx, rx, rtx, backend: make_backend(kind), busy: 0 };
        #[cfg(not(target_arch = "wasm32"))]
        Self { tx, rx, backend: make_backend(kind), busy: 0 }
    }

    pub fn set_backend(&mut self, kind: BackendKind) {
        self.backend = make_backend(kind);
    }

    pub fn submit(&mut self, job: AiJob) {
        self.busy += 1;
        #[cfg(not(target_arch = "wasm32"))]
        let _ = self.tx.send((self.backend.clone(), job));
        // No threads in the browser: run the job now (hosted providers will need async here).
        #[cfg(target_arch = "wasm32")]
        {
            let _ = &self.tx;
            let _ = self.rtx.send(run_job(&*self.backend, job));
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
