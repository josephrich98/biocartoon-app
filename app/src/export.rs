//! Slide export. The slide (or each image on it) is written out as SVG, which is then either saved
//! as-is, rasterised with resvg (PNG / JPEG / TIFF at any DPI) or converted with svg2pdf (PDF;
//! several slides go into one PDF, a page each).
//! Geometry mirrors `canvas::draw_item` at zoom 1, and text is laid out with egui's own fonts so
//! line breaks match the canvas. Runs on a worker thread.

use crate::assets::{fetch, Request};
use crate::canvas::{corners, head_geom, line_paths, shape_polygons, HeadGeom};
use crate::doc::{FontKind, Item, ItemKind, LayerStyle, Slide, TextAlign, TextFont, DPI};
use base64::Engine;
use egui::epaint::text::Glyph;
use egui::{Color32, FontId, Pos2, Rect, Vec2};
use image::RgbaImage;
use resvg::usvg::{self, fontdb};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, OnceLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportFormat {
    Png,
    Jpg,
    Tif,
    Pdf,
    Svg,
    /// The editable project file (see [`crate::projects::to_file`]), written by the app rather than
    /// rendered here.
    Biocartoon,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 6] = [ExportFormat::Png, ExportFormat::Jpg, ExportFormat::Tif, ExportFormat::Pdf, ExportFormat::Svg, ExportFormat::Biocartoon];
    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::Png => "PNG",
            ExportFormat::Jpg => "JPG",
            ExportFormat::Tif => "TIFF",
            ExportFormat::Pdf => "PDF",
            ExportFormat::Svg => "SVG",
            ExportFormat::Biocartoon => "BioCartoon",
        }
    }
    pub fn ext(self) -> &'static str {
        match self {
            ExportFormat::Png => "png",
            ExportFormat::Jpg => "jpg",
            ExportFormat::Tif => "tif",
            ExportFormat::Pdf => "pdf",
            ExportFormat::Svg => "svg",
            ExportFormat::Biocartoon => "biocartoon",
        }
    }
    pub fn is_raster(self) -> bool {
        matches!(self, ExportFormat::Png | ExportFormat::Jpg | ExportFormat::Tif)
    }
    pub fn from_ext(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "png" => Some(ExportFormat::Png),
            "jpg" | "jpeg" => Some(ExportFormat::Jpg),
            "tif" | "tiff" => Some(ExportFormat::Tif),
            "pdf" => Some(ExportFormat::Pdf),
            "svg" => Some(ExportFormat::Svg),
            "biocartoon" => Some(ExportFormat::Biocartoon),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportOptions {
    pub format: ExportFormat,
    /// Raster formats only.
    pub dpi: u32,
    /// PNG only: leave the slide background out.
    pub transparent: bool,
    /// One file per image item, cropped to its bounding box, instead of the whole slide.
    pub individual: bool,
    /// Whole-slide export only: crop the canvas to the combined bounds of the image items.
    pub crop: bool,
    /// Every slide of the document, one file (or set of image files) each, instead of just one.
    /// Starts off each session.
    #[serde(skip)]
    pub all_slides: bool,
    /// PDF of several slides: one file with a page per slide, instead of a file per slide.
    pub pdf_pages: bool,
    /// JPG only: 1 (smallest file) to 100 (best quality).
    pub jpg_quality: u8,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self { format: ExportFormat::Png, dpi: 300, transparent: false, individual: false, crop: false, all_slides: false, pdf_pages: true, jpg_quality: 90 }
    }
}

pub const MIN_DPI: u32 = 10;
pub const MAX_DPI: u32 = 2400;

/// Pixel size of a document-space rect rendered at `dpi`.
pub fn pixel_size(r: Rect, dpi: u32) -> [u32; 2] {
    let s = dpi as f32 / DPI;
    [((r.width() * s).round() as u32).max(1), ((r.height() * s).round() as u32).max(1)]
}

/// Combined bounds of the slide's image items, if it has any.
pub fn images_bounds(slide: &Slide) -> Option<Rect> {
    slide.items.iter().filter(|it| matches!(it.kind, ItemKind::Image { .. })).map(item_bounds).reduce(|a, b| a.union(b))
}

/// Axis-aligned document-space bounds of an item as drawn (rotation included where the canvas
/// applies it).
pub fn item_bounds(it: &Item) -> Rect {
    match it.kind {
        ItemKind::Line { .. } | ItemKind::Table { .. } | ItemKind::Code { .. } => it.rect(),
        _ => {
            let mut r = Rect::NOTHING;
            for p in corners(it) {
                r.extend_with(p);
            }
            r
        }
    }
}

pub struct ExportJob {
    pub path: PathBuf,
    pub opts: ExportOptions,
    /// The slides to export, each with its 1-based number. With more than one, each file's name
    /// gets `-slide<N>`.
    pub slides: Vec<(usize, Slide)>,
    pub page: Rect,
    pub background: [u8; 4],
    /// Cache key -> where to load the full-resolution original from.
    pub requests: HashMap<String, Request>,
    /// Cache key -> the decoded (possibly downscaled) copy, used if the original can't be loaded.
    pub fallbacks: HashMap<String, Arc<RgbaImage>>,
    /// Web build: cache key -> the original's bytes, which `start` downloads first.
    pub downloads: HashMap<String, Vec<u8>>,
}

/// Run an export on a worker thread. The receiver yields a status message when it is done.
#[cfg(not(target_arch = "wasm32"))]
pub fn start(ctx: egui::Context, job: ExportJob) -> Receiver<Result<String, String>> {
    let (tx, rx) = channel();
    crate::platform::spawn("export", move || {
        let res = run(&ctx, job);
        let _ = tx.send(res);
        ctx.request_repaint();
    });
    rx
}

/// Web build: there are no blocking fetches, so first download every full-resolution original
/// (in parallel), put them in place of the cached copies, then run the export.
#[cfg(target_arch = "wasm32")]
pub fn start(ctx: egui::Context, job: ExportJob) -> Receiver<Result<String, String>> {
    use std::sync::Mutex;
    let (tx, rx) = channel();
    let fetches: Vec<(String, Request)> = job
        .requests
        .iter()
        .filter(|(_, r)| !matches!(r, Request::Path(_)))
        .map(|(k, r)| (k.clone(), r.clone()))
        .collect();
    if fetches.is_empty() {
        let _ = tx.send(run(&ctx, job));
        return rx;
    }
    let pending = Arc::new(Mutex::new((Some((ctx, job, tx)), fetches.len())));
    for (key, req) in fetches {
        let pending = pending.clone();
        crate::assets::fetch_web(req, Box::new(move |res| {
            let mut g = pending.lock().unwrap();
            match (res, g.0.as_mut()) {
                (Ok(bytes), Some((_, job, _))) => {
                    job.downloads.insert(key, bytes);
                }
                (Err(e), _) => log::warn!("export: {e}; using the cached copy"),
                _ => {}
            }
            g.1 -= 1;
            if g.1 == 0 {
                if let Some((ctx, job, tx)) = g.0.take() {
                    drop(g);
                    let _ = tx.send(run(&ctx, job));
                    ctx.request_repaint();
                }
            }
        }));
    }
    rx
}

pub fn run(ctx: &egui::Context, job: ExportJob) -> Result<String, String> {
    let out = render(ctx, &job, None, &mut |path, bytes, _| {
        // A download in the browser.
        crate::platform::write_output(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
    })?;
    let mut msg = match out.files.as_slice() {
        [] => format!("Exported {} slides to {}", out.pages, job.path.display()),
        [one] => format!("Exported {}", one.display()),
        many => format!("Exported {} {} to {}", many.len(), if job.opts.individual { "images" } else { "slides" }, many[0].parent().map(|p| p.display().to_string()).unwrap_or_default()),
    };
    if out.missing > 0 {
        let _ = write!(msg, " ({} image(s) could not be loaded and were left out)", out.missing);
    }
    Ok(msg)
}

/// The size of what [`run`] would write, without writing it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SizeEstimate {
    pub bytes: u64,
    pub files: usize,
    /// Some image was rendered at a lower resolution and its size scaled up.
    pub approx: bool,
}

/// Raster images larger than this are measured at a lower resolution, and the size scaled.
const ESTIMATE_MAX_PIXELS: u64 = 12_000_000;

/// Encode everything `job` would export, in memory, and add up the sizes.
pub fn estimate(ctx: &egui::Context, job: &ExportJob) -> Result<SizeEstimate, String> {
    let mut est = SizeEstimate { bytes: 0, files: 0, approx: false };
    render(ctx, job, Some(ESTIMATE_MAX_PIXELS), &mut |_, bytes, scale| {
        est.bytes += (bytes.len() as f64 * scale).round() as u64;
        est.files += 1;
        est.approx |= scale != 1.0;
        Ok(())
    })?;
    Ok(est)
}

/// What [`render`] produced.
struct Rendered {
    /// The files, unless they were combined into one PDF.
    files: Vec<PathBuf>,
    /// The page count of the combined PDF, or 0.
    pages: usize,
    /// Images that could not be loaded.
    missing: usize,
}

/// Encode every file `job` exports and hand each to `out`, with its path and how much bigger the
/// real file is (raster images over `max_pixels` are encoded at a lower resolution, and this is
/// their pixel ratio; otherwise 1).
fn render(ctx: &egui::Context, job: &ExportJob, max_pixels: Option<u64>, out: &mut dyn FnMut(&Path, &[u8], f64) -> Result<(), String>) -> Result<Rendered, String> {
    let opts = &job.opts;
    let mut images: HashMap<String, Picture> = HashMap::new();
    let mut missing = 0;
    for (key, req) in &job.requests {
        let full = match job.downloads.get(key) {
            // Web build: `start` already downloaded the original.
            Some(b) => Picture::from_bytes(b),
            None => fetch(req).and_then(|b| Picture::from_bytes(&b)),
        };
        match full {
            Ok(img) => {
                images.insert(key.clone(), img);
            }
            Err(e) => match job.fallbacks.get(key) {
                Some(img) => {
                    log::warn!("export: using cached copy of {key}: {e}");
                    images.insert(key.clone(), Picture::Pixels((**img).clone()));
                }
                None => {
                    log::warn!("export: {key}: {e}");
                    missing += 1;
                }
            },
        }
    }

    let transparent = opts.format == ExportFormat::Png && opts.transparent;
    let mut written = Vec::new();
    let multi = job.slides.len() > 1;
    let dir = job.path.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = job.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "image".into());
    // One PDF with a page per slide: collect the pages, write them after the loop.
    let combine = multi && !opts.individual && opts.format == ExportFormat::Pdf && opts.pdf_pages;
    let mut pages = Vec::new();
    for (n, slide) in &job.slides {
        let bg = if transparent { None } else { Some(slide.background.unwrap_or(job.background)) };
        let stem = if multi { format!("{stem}-slide{n}") } else { stem.clone() };
        if opts.individual {
            let pics: Vec<&Item> = slide.items.iter().filter(|it| matches!(it.kind, ItemKind::Image { .. })).collect();
            for (i, it) in pics.iter().enumerate() {
                let region = item_bounds(it);
                let svg = build_svg(ctx, &[it], region, bg, &images);
                let name = crate::app::BioCartoon::slugify(&it.name);
                let path = dir.join(format!("{stem}-{:02}-{name}.{}", i + 1, opts.format.ext()));
                emit(&svg, region, opts, max_pixels, &path, &crate::credits::for_items(&[it], false), out)?;
                written.push(path);
            }
        } else {
            // Cropping to images skips the slides that have none.
            let region = if opts.crop {
                match images_bounds(slide) {
                    Some(r) => r,
                    None => continue,
                }
            } else {
                job.page
            };
            let items: Vec<&Item> = slide.items.iter().collect();
            let svg = build_svg(ctx, &items, region, bg, &images);
            if combine {
                pages.push(svg);
                continue;
            }
            let path = if multi { dir.join(format!("{stem}.{}", opts.format.ext())) } else { job.path.clone() };
            emit(&svg, region, opts, max_pixels, &path, &crate::credits::for_items(&items, false), out)?;
            written.push(path);
        }
    }
    if !pages.is_empty() {
        let bytes = pdf_pages(&pages).map_err(|e| format!("{}: {e}", job.path.display()))?;
        out(&job.path, &bytes, 1.0)?;
    }
    if written.is_empty() && pages.is_empty() {
        return Err(if multi { "There are no images on these slides" } else { "There are no images on this slide" }.into());
    }
    Ok(Rendered { files: written, pages: pages.len(), missing })
}

/// Encode one file and hand it to `out`, at a lower resolution if it has more than `max_pixels`.
fn emit(svg: &str, region: Rect, opts: &ExportOptions, max_pixels: Option<u64>, path: &Path, credits: &str, out: &mut dyn FnMut(&Path, &[u8], f64) -> Result<(), String>) -> Result<(), String> {
    let mut dpi = opts.dpi.clamp(MIN_DPI, MAX_DPI);
    let mut scale = 1.0;
    if let (true, Some(max)) = (opts.format.is_raster(), max_pixels) {
        let px = |dpi| pixel_size(region, dpi).iter().map(|&v| v as u64).product::<u64>();
        let full = px(dpi);
        if full > max {
            dpi = ((dpi as f64 * (max as f64 / full as f64).sqrt()) as u32).max(1);
            scale = full as f64 / px(dpi) as f64;
        }
    }
    let bytes = encode(svg, region, opts, dpi, credits).map_err(|e| format!("{}: {e}", path.display()))?;
    out(path, &bytes, scale)
}

/// `items` cropped to their bounds on a transparent background, at twice screen resolution (the
/// longest side capped at `max_side` px), for copying to the clipboard. Images come from `cached`;
/// ones not loaded yet are left out.
pub fn snapshot(ctx: &egui::Context, items: &[&Item], cached: &HashMap<String, Arc<RgbaImage>>, max_side: u32) -> Result<RgbaImage, String> {
    let region = items.iter().map(|it| item_bounds(it)).reduce(|a, b| a.union(b)).ok_or("Nothing to copy")?;
    let images: HashMap<String, Picture> = cached.iter().map(|(k, img)| (k.clone(), Picture::Pixels((**img).clone()))).collect();
    let svg = build_svg(ctx, items, region, None, &images);
    let dpi = (DPI * 2.0).min(max_side as f32 * DPI / region.width().max(region.height()).max(1.0)).round().max(1.0) as u32;
    rasterise(&parse(&svg)?, region, dpi)
}

/// A small preview of a slide (longest side `max_side` px) for project tiles and version history.
/// Images come from `cached` when present, otherwise from `requests`; missing ones are left out.
pub fn thumbnail(
    ctx: &egui::Context,
    slide: &Slide,
    page: Rect,
    background: [u8; 4],
    requests: &HashMap<String, Request>,
    cached: &HashMap<String, Arc<RgbaImage>>,
    max_side: u32,
) -> Result<RgbaImage, String> {
    let mut images: HashMap<String, Picture> = HashMap::new();
    for (key, req) in requests {
        let img = match cached.get(key) {
            Some(img) => Ok(Picture::Pixels((**img).clone())),
            None => fetch(req).and_then(|b| Picture::from_bytes(&b)),
        };
        match img {
            Ok(img) => {
                images.insert(key.clone(), img);
            }
            Err(e) => log::warn!("thumbnail: {key}: {e}"),
        }
    }
    let items: Vec<&Item> = slide.items.iter().collect();
    let svg = build_svg(ctx, &items, page, Some(background), &images);
    let dpi = ((max_side as f32 * DPI / page.width().max(page.height()).max(1.0)).round() as u32).max(1);
    rasterise(&parse(&svg)?, page, dpi)
}

// ---------- SVG ----------

fn n(v: f32) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

fn hex(c: [u8; 4]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

/// `fill="#rrggbb" fill-opacity="a"` (or `stroke`), folding in the item opacity.
fn paint(attr: &str, c: [u8; 4], op: f32) -> String {
    let a = c[3] as f32 / 255.0 * op;
    if a >= 0.999 {
        format!(r#"{attr}="{}""#, hex(c))
    } else {
        format!(r#"{attr}="{}" {attr}-opacity="{}""#, hex(c), n(a))
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn points(pts: &[Pos2]) -> String {
    pts.iter().map(|p| format!("{},{}", n(p.x), n(p.y))).collect::<Vec<_>>().join(" ")
}

fn rotate_attr(it: &Item, c: Pos2) -> String {
    if it.rotation == 0.0 {
        String::new()
    } else {
        format!(r#" transform="rotate({} {} {})""#, n(it.rotation.to_degrees()), n(c.x), n(c.y))
    }
}

/// Font family names used in the SVG: the egui font first (it is what the canvas shows and what
/// the PDF / raster renderers load), then common fallbacks for other SVG viewers.
fn font_family() -> String {
    format!("'{}', Ubuntu, Helvetica, Arial, sans-serif", fonts().family)
}

/// Lay out text with egui (at zoom 1) and emit one `<text>` per row.
fn text_rows(out: &mut String, ctx: &egui::Context, text: &str, size: f32, wrap: f32, origin: Pos2, align: TextAlign, box_w: f32) {
    let galley = ctx.fonts_mut(|f| f.layout(text.to_string(), FontId::proportional(size.max(1.0)), Color32::WHITE, wrap.max(4.0)));
    let gw = galley.size().x;
    let x0 = match align {
        TextAlign::Left | TextAlign::Justify => origin.x,
        TextAlign::Center => origin.x + box_w / 2.0 - gw / 2.0,
        TextAlign::Right => origin.x + box_w - gw,
    };
    for prow in &galley.rows {
        let glyphs = real_glyphs(&prow.row.glyphs);
        let Some(g0) = glyphs.first() else { continue };
        let (s, xs) = logical_text(&glyphs, x0 + prow.pos.x);
        let x = x0 + prow.pos.x + g0.pos.x;
        let y = origin.y + prow.pos.y + g0.pos.y;
        let _ = writeln!(out, r#"<text x="{}" y="{}">{}</text>"#, n(x), n(y), row_markup(TextFont::Sans, &s, &xs));
    }
}

/// Strongly right-to-left characters (Hebrew, Arabic and their neighbours).
fn is_rtl(c: char) -> bool {
    matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF)
}

/// A row's glyphs without the copies egui adds after a right-to-left run: its shaper meets the
/// run's clusters in reverse and appends the run's characters after the first again, as
/// zero-width "continuation" glyphs (epaint 0.36's `emit_continuation_glyphs`).
fn real_glyphs(glyphs: &[Glyph]) -> Vec<&Glyph> {
    let mut out: Vec<&Glyph> = Vec::with_capacity(glyphs.len());
    let mut i = 0;
    while i < glyphs.len() {
        let k = glyphs[i..].iter().take_while(|g| g.advance_width == 0.0 && g.chr != '\n').count();
        // The copies, reversed, are the glyphs just before them less the last (the run's first).
        if k > 0 && out.len() > k && glyphs[i..i + k].iter().any(|g| is_rtl(g.chr)) && glyphs[i..i + k].iter().rev().zip(&out[out.len() - k - 1..]).all(|(a, b)| a.chr == b.chr) {
            i += k;
            continue;
        }
        out.push(&glyphs[i]);
        i += 1;
    }
    out
}

/// Glyphs as text in logical order, with each character's x (`x0` plus the glyph's): egui shapes
/// each word of right-to-left text on its own and leaves it in the order it is drawn, which
/// resvg would reverse a second time. A reversed word's characters all take its left edge,
/// where its chunk starts.
fn logical_text(glyphs: &[&Glyph], x0: f32) -> (String, Vec<f32>) {
    let mut chars: Vec<char> = glyphs.iter().map(|g| g.chr).collect();
    let mut xs: Vec<f32> = glyphs.iter().map(|g| x0 + g.pos.x).collect();
    let mut i = 0;
    while i < chars.len() {
        let j = i + chars[i..].iter().take_while(|c| is_rtl(**c)).count();
        if j > i {
            chars[i..j].reverse();
            let left = xs[i];
            xs[i..j].fill(left);
        }
        i = j.max(i + 1);
    }
    (chars.into_iter().collect(), xs)
}

/// Whether `font`'s face has a glyph for each of `s`'s characters.
fn covered(font: TextFont, s: &str) -> Vec<bool> {
    let all = vec![true; s.chars().count()];
    let family = match font {
        TextFont::Sans => fonts().family.as_str(),
        TextFont::Mono => "Hack",
        f => f.embedded_name().unwrap_or_default(),
    };
    let db = &fonts().db;
    let Some(id) = db.query(&fontdb::Query { families: &[fontdb::Family::Name(family)], ..Default::default() }) else { return all };
    db.with_face_data(id, |data, index| {
        let face = ttf_parser::Face::parse(data, index).ok()?;
        Some(s.chars().map(|c| face.glyph_index(c).is_some()).collect())
    })
    .flatten()
    .unwrap_or(all)
}

/// The family to draw `c` in when a text's font lacks it: egui's own fallbacks first, as on the
/// canvas, then any system font that has it; None when no font does.
fn fallback_family(c: char) -> Option<String> {
    static CACHE: OnceLock<std::sync::Mutex<HashMap<char, Option<String>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(f) = cache.lock().ok().and_then(|m| m.get(&c).cloned()) {
        return f;
    }
    let f = fonts();
    let has = |id: fontdb::ID| f.db.with_face_data(id, |data, index| ttf_parser::Face::parse(data, index).is_ok_and(|face| face.glyph_index(c).is_some())).unwrap_or(false);
    // Hidden system faces are no fallback: ".LastResort" draws every character as a box.
    let system = f.db.faces().filter(|face| !face.families.iter().any(|(n, _)| n.starts_with('.'))).map(|face| face.id);
    let found = f.bundled.iter().copied().chain(system).find(|id| has(*id)).and_then(|id| f.db.face(id)).and_then(|face| face.families.first().map(|(n, _)| n.clone()));
    if let Ok(mut m) = cache.lock() {
        m.insert(c, found.clone());
    }
    found
}

/// A row's text for its `<text>`, with each run of characters the item's font lacks (Greek in
/// many of the bundled fonts, emoji and CJK in all of them) in a `<tspan>` of a font that has
/// them. resvg shapes a whole text chunk with one font and, when that font lacks a character,
/// redraws the chunk in a fallback that has them all, unlike the canvas, which falls back glyph
/// by glyph; so every run after the first starts a chunk of its own, at the x (`xs`, one per
/// character) where egui put it.
fn row_markup(font: TextFont, s: &str, xs: &[f32]) -> String {
    let mut out = String::new();
    let mut run = String::new();
    let mut run_x = None;
    let mut run_family: Option<String> = None;
    let mut flush = |run: &mut String, x: Option<f32>, family: &Option<String>| {
        if run.is_empty() {
            return;
        }
        let x = x.map(|x| format!(r#" x="{}""#, n(x))).unwrap_or_default();
        let family = match family {
            Some(f) if *f == fonts().family => format!(r#" font-family="{}""#, esc(&font_family())),
            Some(f) => format!(r#" font-family="{}""#, esc(&format!("'{f}', {}", font_family()))),
            None => String::new(),
        };
        if x.is_empty() && family.is_empty() {
            out.push_str(&esc(run));
        } else {
            let _ = write!(out, "<tspan{x}{family}>{}</tspan>", esc(run));
        }
        run.clear();
    };
    for (i, (c, ok)) in s.chars().zip(covered(font, s)).enumerate() {
        let family = (!ok && !c.is_whitespace()).then(|| fallback_family(c).unwrap_or_else(|| fonts().family.clone()));
        if family != run_family && !run.is_empty() {
            flush(&mut run, run_x, &run_family);
            run_x = xs.get(i).copied();
        }
        run_family = family;
        run.push(c);
    }
    flush(&mut run, run_x, &run_family);
    out
}

/// A text item's rows, laid out as the canvas lays them out: a `<text>` for each stretch of a
/// row whose characters look alike, carrying its own colour and decoration when they are not
/// the box's (`op` is the item's opacity).
fn text_item_rows(out: &mut String, ctx: &egui::Context, it: &Item, op: f32) {
    let ItemKind::Text { text, color, align, font, .. } = &it.kind else { return };
    let r = it.rect();
    let job = crate::canvas::text_job(ctx, text, &it.kind, 1.0, r.width(), 1.0);
    let galley = ctx.fonts_mut(|f| f.layout_job(job));
    let o = crate::canvas::text_origin(&galley, r, &it.kind);
    let looks = crate::doc::char_looks(&it.kind, text);
    let last = galley.rows.len().saturating_sub(1);
    let mut first_char = 0;
    for (i, prow) in galley.rows.iter().enumerate() {
        let glyphs = real_glyphs(&prow.row.glyphs);
        let start = first_char;
        first_char += glyphs.len() + usize::from(prow.ends_with_newline);
        let mut a = 0;
        while a < glyphs.len() {
            let look = looks.get(start + a).copied();
            let b = a + glyphs[a..].iter().enumerate().skip(1).find(|(k, _)| looks.get(start + a + k).copied() != look).map_or(glyphs.len() - a, |(k, _)| k);
            let seg = &glyphs[a..b];
            let (g0, g1) = (seg[0], seg[seg.len() - 1]);
            let (s, xs) = logical_text(seg, o.x + prow.pos.x);
            let x = o.x + prow.pos.x + g0.pos.x;
            let y = o.y + prow.pos.y + g0.pos.y;
            // Justified rows are stretched to the box, as egui stretches them.
            let fit = if *align == TextAlign::Justify && i != last && !prow.ends_with_newline {
                format!(r#" textLength="{}" lengthAdjust="spacing""#, n(g1.max_x() - g0.pos.x))
            } else {
                String::new()
            };
            let mut style = String::new();
            if let Some(l) = look {
                if l.color != *color {
                    let _ = write!(style, " {}", paint("fill", l.color, op));
                }
                // Bold is a thin outline in the text's colour: the bundled font has one weight.
                if l.bold {
                    let _ = write!(style, r#" {} stroke-width="0.6" stroke-linejoin="round""#, paint("stroke", l.color, op));
                }
                let deco: Vec<&str> = [(l.underline, "underline"), (l.strike, "line-through")].into_iter().filter(|d| d.0).map(|d| d.1).collect();
                if !deco.is_empty() {
                    let _ = write!(style, r#" text-decoration="{}""#, deco.join(" "));
                }
                // Slant rather than font-style="italic": the bundled font has no italic face.
                if l.italic {
                    let _ = write!(style, r#" transform="translate({} {}) skewX(-12) translate({} {})""#, n(x), n(y), n(-x), n(-y));
                }
            }
            let _ = writeln!(out, r#"<text x="{}" y="{}"{fit}{style}>{}</text>"#, n(x), n(y), row_markup(*font, &s, &xs));
            a = b;
        }
    }
}

/// Crop, flip and tint an image the way the canvas draws it, as PNG bytes.
fn prepared_png(img: &RgbaImage, crop: [f32; 4], overlay: [u8; 4], flip_h: bool, flip_v: bool) -> Option<Vec<u8>> {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    let span = |a: f32, b: f32, len: u32| {
        let lo = ((a.clamp(0.0, 1.0) * len as f32).round() as u32).min(len - 1);
        let hi = ((b.clamp(0.0, 1.0) * len as f32).round() as u32).clamp(lo + 1, len);
        (lo, hi - lo)
    };
    let (x0, cw) = span(crop[0], crop[2], w);
    let (y0, ch) = span(crop[1], crop[3], h);
    let mut out = image::imageops::crop_imm(img, x0, y0, cw, ch).to_image();
    if flip_h {
        image::imageops::flip_horizontal_in_place(&mut out);
    }
    if flip_v {
        image::imageops::flip_vertical_in_place(&mut out);
    }
    if overlay[3] > 0 {
        let s = overlay[3] as f32 / 255.0;
        let tint = [0, 1, 2].map(|i| (1.0 - s) + overlay[i] as f32 / 255.0 * s);
        for p in out.pixels_mut() {
            for i in 0..3 {
                p.0[i] = (p.0[i] as f32 * tint[i]).round() as u8;
            }
        }
    }
    let mut buf = Vec::new();
    out.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png).ok()?;
    Some(buf)
}

/// An image item's picture as export draws it: decoded pixels, or an uploaded SVG kept as
/// vector shapes.
pub enum Picture {
    Pixels(RgbaImage),
    Vector(Box<Vector>),
}

impl Picture {
    /// Decode an image file; SVGs are parsed, not rasterised.
    fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        if crate::assets::is_svg(bytes) {
            Vector::parse(bytes).map(|v| Picture::Vector(Box::new(v)))
        } else {
            image::load_from_memory(bytes).map(|i| Picture::Pixels(i.to_rgba8())).map_err(|e| e.to_string())
        }
    }
}

/// An SVG image item drawn as vector shapes: the file's contents (IDs prefixed with `id`) in a
/// nested `<svg>` whose view box is the drawing, placed so the crop window lands on the item's
/// rect, clipped to it, mirrored for flips and tinted with a colour matrix (the same multiply
/// as [`prepared_png`]). An item from before SVGs were trimmed to their drawing is moved onto
/// the drawing first, as the canvas does when it opens such a document.
fn vector_image(s: &mut String, vector: &Vector, it: &Item, crop: [f32; 4], overlay: [u8; 4], id: &str) {
    let content = vector.content();
    let (r, crop) = if it.fitted { None } else { crate::doc::fit_to_drawing(it, vector.artboard(), content) }.unwrap_or((it.rect(), crop));
    let c = r.center();
    let (cw, ch) = ((crop[2] - crop[0]).max(1e-3), (crop[3] - crop[1]).max(1e-3));
    let full = Rect::from_min_size(Pos2::new(r.min.x - crop[0] / cw * r.width(), r.min.y - crop[1] / ch * r.height()), Vec2::new(r.width() / cw, r.height() / ch));
    let xml = vector.tree.to_string(&usvg::WriteOptions { id_prefix: Some(format!("{id}-")), indent: usvg::Indent::None, ..Default::default() });
    // Keep what is inside the file's root <svg> element; it gets our own below.
    let body = xml.find("<svg").and_then(|i| xml[i..].find('>').map(|j| i + j + 1)).zip(xml.rfind("</svg>")).map_or("", |(a, b)| &xml[a..b]);

    let op = it.opacity.clamp(0.0, 1.0);
    let op_attr = if op < 1.0 { format!(r#" opacity="{}""#, n(op)) } else { String::new() };
    let _ = writeln!(s, r#"<g{op_attr}{}>"#, rotate_attr(it, c));
    let _ = writeln!(s, r#"<clipPath id="{id}"><rect x="{}" y="{}" width="{}" height="{}"/></clipPath>"#, n(r.min.x), n(r.min.y), n(r.width()), n(r.height()));
    let mut filter = String::new();
    if overlay[3] > 0 {
        let a = overlay[3] as f32 / 255.0;
        let [tr, tg, tb] = [0, 1, 2].map(|i| n((1.0 - a) + overlay[i] as f32 / 255.0 * a));
        let _ = writeln!(
            s,
            r#"<filter id="{id}-tint" x="0" y="0" width="1" height="1" color-interpolation-filters="sRGB"><feColorMatrix type="matrix" values="{tr} 0 0 0 0 0 {tg} 0 0 0 0 0 {tb} 0 0 0 0 0 1 0"/></filter>"#
        );
        filter = format!(r#" filter="url(#{id}-tint)""#);
    }
    let _ = writeln!(s, r#"<g clip-path="url(#{id})"{filter}>"#);
    let (sx, sy) = (if it.flip_h { -1.0 } else { 1.0 }, if it.flip_v { -1.0 } else { 1.0 });
    if it.flip_h || it.flip_v {
        let _ = writeln!(s, r#"<g transform="translate({} {}) scale({sx} {sy}) translate({} {})">"#, n(c.x), n(c.y), n(-c.x), n(-c.y));
    }
    let _ = writeln!(
        s,
        r#"<svg x="{}" y="{}" width="{}" height="{}" viewBox="{} {} {} {}" preserveAspectRatio="none" overflow="visible">{body}</svg>"#,
        n(full.min.x),
        n(full.min.y),
        n(full.width()),
        n(full.height()),
        n(content.min.x),
        n(content.min.y),
        n(content.width()),
        n(content.height())
    );
    if it.flip_h || it.flip_v {
        s.push_str("</g>\n");
    }
    s.push_str("</g>\n</g>\n");
}

pub fn build_svg(ctx: &egui::Context, items: &[&Item], region: Rect, bg: Option<[u8; 4]>, images: &HashMap<String, Picture>) -> String {
    let mut s = String::new();
    let (w, h) = (region.width(), region.height());
    let _ = writeln!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="{}in" height="{}in" viewBox="{} {} {} {}">"#,
        n(w / DPI),
        n(h / DPI),
        n(region.min.x),
        n(region.min.y),
        n(w),
        n(h)
    );
    if let Some(c) = bg.filter(|c| c[3] > 0) {
        let _ = writeln!(s, r#"<rect x="{}" y="{}" width="{}" height="{}" {}/>"#, n(region.min.x), n(region.min.y), n(w), n(h), paint("fill", c, 1.0));
    }
    // Clip to the region, as the canvas clips to the page.
    let _ = writeln!(s, r#"<clipPath id="page"><rect x="{}" y="{}" width="{}" height="{}"/></clipPath>"#, n(region.min.x), n(region.min.y), n(w), n(h));
    let _ = writeln!(s, r#"<g clip-path="url(#page)">"#);
    let family = font_family();
    let mut clip_id = 0;
    for it in items {
        let op = it.opacity.clamp(0.0, 1.0);
        let r = it.rect();
        let c = r.center();
        match &it.kind {
            ItemKind::Image { source, crop, overlay, layers } => {
                let png = match images.get(&source.cache_key()) {
                    Some(Picture::Vector(vector)) => {
                        clip_id += 1;
                        let restyled = vector.restyled(layers);
                        vector_image(&mut s, restyled.as_ref().unwrap_or(vector), it, *crop, *overlay, &format!("svg{clip_id}"));
                        continue;
                    }
                    Some(Picture::Pixels(img)) => prepared_png(img, *crop, *overlay, it.flip_h, it.flip_v),
                    None => None,
                };
                let Some(png) = png else { continue };
                let op_attr = if op < 1.0 { format!(r#" opacity="{}""#, n(op)) } else { String::new() };
                let _ = writeln!(
                    s,
                    r#"<image x="{}" y="{}" width="{}" height="{}" preserveAspectRatio="none"{op_attr}{} xlink:href="data:image/png;base64,{}"/>"#,
                    n(r.min.x),
                    n(r.min.y),
                    n(r.width()),
                    n(r.height()),
                    rotate_attr(it, c),
                    base64::engine::general_purpose::STANDARD.encode(png)
                );
            }
            ItemKind::Text { size, color, font, .. } => {
                let family = match font {
                    TextFont::Sans => family.clone(),
                    TextFont::Mono => "Hack, Menlo, Consolas, monospace".into(),
                    TextFont::Tinos => "Tinos, 'Times New Roman', Times, serif".into(),
                    TextFont::Cousine => "Cousine, 'Courier New', Courier, monospace".into(),
                    TextFont::Carlito => format!("Carlito, Calibri, {family}"),
                    TextFont::Caladea => "Caladea, Cambria, Georgia, serif".into(),
                    TextFont::Gelasio => "Gelasio, Georgia, serif".into(),
                    f => {
                        let name = f.embedded_name().unwrap_or_default();
                        match f.kind() {
                            FontKind::Sans => format!("'{name}', {family}"),
                            FontKind::Serif => format!("'{name}', Georgia, serif"),
                            FontKind::Mono => format!("'{name}', Menlo, Consolas, monospace"),
                            FontKind::Handwriting => format!("'{name}', cursive"),
                        }
                    }
                };
                let _ = writeln!(
                    s,
                    r#"<g{} font-family="{}" font-size="{}" font-weight="300" {} xml:space="preserve">"#,
                    rotate_attr(it, c),
                    esc(&family),
                    n(*size),
                    paint("fill", *color, op)
                );
                text_item_rows(&mut s, ctx, it, op);
                s.push_str("</g>\n");
            }
            ItemKind::Shape { shape, fill, stroke, stroke_width } => {
                let (_, outline) = shape_polygons(*shape, r);
                let pts: Vec<Pos2> = outline
                    .iter()
                    .map(|p| Pos2::new(if it.flip_h { 2.0 * c.x - p.x } else { p.x }, if it.flip_v { 2.0 * c.y - p.y } else { p.y }))
                    .collect();
                let st = if *stroke_width > 0.0 && stroke[3] > 0 {
                    format!(r#" {} stroke-width="{}""#, paint("stroke", *stroke, op), n(*stroke_width))
                } else {
                    String::new()
                };
                let fl = if fill[3] > 0 { paint("fill", *fill, op) } else { r#"fill="none""#.into() };
                let _ = writeln!(s, r#"<polygon points="{}" {fl}{st}{}/>"#, points(&pts), rotate_attr(it, c));
            }
            ItemKind::Line { color, width, style, .. } => {
                let w = width.max(0.5);
                let dash = style.dash(w).map(|(d, g)| format!(r#" stroke-dasharray="{} {}""#, n(d), n(g))).unwrap_or_default();
                let head = (w * 4.0).max(6.0);
                for path in line_paths(it) {
                    if path.pts.len() < 2 {
                        continue;
                    }
                    let _ = writeln!(s, r#"<polyline points="{}" fill="none" {} stroke-width="{}"{dash}/>"#, points(&path.pts), paint("stroke", *color, op), n(w));
                    for (at_end, h) in [(false, path.start), (true, path.end)] {
                        match head_geom(&path.pts, at_end, h, head) {
                            Some(HeadGeom::Polygon(tri)) => {
                                let _ = writeln!(s, r#"<polygon points="{}" {}/>"#, points(&tri), paint("fill", *color, op));
                            }
                            Some(HeadGeom::Segment(a, b)) => {
                                let _ = writeln!(s, r#"<line x1="{}" y1="{}" x2="{}" y2="{}" {} stroke-width="{}"/>"#, n(a.x), n(a.y), n(b.x), n(b.y), paint("stroke", *color, op), n(w * 1.2));
                            }
                            None => {}
                        }
                    }
                }
            }
            ItemKind::Table { rows, cols, cells, header, border, fill, header_fill, text_color, font_size } => {
                let (rows, cols) = ((*rows).max(1), (*cols).max(1));
                let cw = r.width() / cols as f32;
                let ch = r.height() / rows as f32;
                let _ = writeln!(s, r#"<rect x="{}" y="{}" width="{}" height="{}" {}/>"#, n(r.min.x), n(r.min.y), n(r.width()), n(r.height()), paint("fill", *fill, op));
                if *header {
                    let _ = writeln!(s, r#"<rect x="{}" y="{}" width="{}" height="{}" {}/>"#, n(r.min.x), n(r.min.y), n(r.width()), n(ch), paint("fill", *header_fill, op));
                }
                let bs = paint("stroke", *border, op);
                for i in 0..=rows {
                    let y = r.min.y + i as f32 * ch;
                    let _ = writeln!(s, r#"<line x1="{}" y1="{}" x2="{}" y2="{}" {bs} stroke-width="1"/>"#, n(r.min.x), n(y), n(r.max.x), n(y));
                }
                for j in 0..=cols {
                    let x = r.min.x + j as f32 * cw;
                    let _ = writeln!(s, r#"<line x1="{}" y1="{}" x2="{}" y2="{}" {bs} stroke-width="1"/>"#, n(x), n(r.min.y), n(x), n(r.max.y));
                }
                for i in 0..rows {
                    for j in 0..cols {
                        let Some(t) = cells.get(i * cols + j).filter(|t| !t.is_empty()) else { continue };
                        let cell = Rect::from_min_size(Pos2::new(r.min.x + j as f32 * cw, r.min.y + i as f32 * ch), Vec2::new(cw, ch));
                        clip_id += 1;
                        let _ = writeln!(s, r#"<clipPath id="cell{clip_id}"><rect x="{}" y="{}" width="{}" height="{}"/></clipPath>"#, n(cell.min.x), n(cell.min.y), n(cw), n(ch));
                        let wrap = (cw - 8.0).max(4.0);
                        let gh = ctx.fonts_mut(|f| f.layout(t.clone(), FontId::proportional(font_size.max(1.0)), Color32::WHITE, wrap)).size().y;
                        let origin = Pos2::new(cell.min.x + 4.0, cell.center().y - gh / 2.0);
                        let _ = writeln!(
                            s,
                            r#"<g clip-path="url(#cell{clip_id})" font-family="{}" font-size="{}" font-weight="300" {} xml:space="preserve">"#,
                            esc(&family),
                            n(*font_size),
                            paint("fill", *text_color, op)
                        );
                        text_rows(&mut s, ctx, t, *font_size, wrap, origin, TextAlign::Left, wrap);
                        s.push_str("</g>\n");
                    }
                }
            }
            ItemKind::Code { code, language, theme, font_size, line_numbers, window, rounded } => {
                let pal = theme.palette();
                let fill = |c: [u8; 3]| paint("fill", [c[0], c[1], c[2], 255], op);
                let fs = font_size.max(1.0);
                let font = FontId::monospace(fs);
                let (char_w, line_h, baseline) = ctx.fonts_mut(|f| {
                    let g = f.layout_no_wrap("0".into(), font.clone(), Color32::WHITE);
                    (f.glyph_width(&font, '0'), f.row_height(&font), g.rows.first().and_then(|r| r.row.glyphs.first()).map_or(fs * 0.8, |gl| gl.pos.y))
                });
                let lines = crate::code::highlight(code, language.resolve(code));
                let lay = crate::code::BlockLayout::new(fs, lines.len(), *line_numbers, *window, char_w, line_h);
                let _ = writeln!(s, r#"<rect x="{}" y="{}" width="{}" height="{}" rx="{}" {}/>"#, n(r.min.x), n(r.min.y), n(r.width()), n(r.height()), n(if *rounded { lay.radius } else { 0.0 }), fill(pal.bg));
                if *window {
                    for (c, rad, rgb) in lay.dots(fs) {
                        let _ = writeln!(s, r#"<circle cx="{}" cy="{}" r="{}" {}/>"#, n(r.min.x + c[0]), n(r.min.y + c[1]), n(rad), fill(rgb));
                    }
                }
                clip_id += 1;
                let _ = writeln!(s, r#"<clipPath id="cell{clip_id}"><rect x="{}" y="{}" width="{}" height="{}"/></clipPath>"#, n(r.min.x + 1.0), n(r.min.y + 1.0), n((r.width() - 2.0).max(0.0)), n((r.height() - 2.0).max(0.0)));
                let _ = writeln!(s, r#"<g clip-path="url(#cell{clip_id})" font-family="Hack, Menlo, Consolas, monospace" font-size="{}" xml:space="preserve">"#, n(fs));
                let o = lay.text_origin();
                for (i, spans) in lines.iter().enumerate() {
                    let y = r.min.y + o[1] + i as f32 * line_h;
                    if y > r.max.y {
                        break;
                    }
                    if *line_numbers {
                        let _ = writeln!(s, r#"<text x="{}" y="{}" text-anchor="end" {}>{}</text>"#, n(r.min.x + o[0] - fs * 1.4), n(y + baseline), fill(pal.line_number), i + 1);
                    }
                    // Place each run at its column so the layout matches the canvas whatever font the viewer substitutes.
                    let mut col = 0usize;
                    for (t, k) in spans {
                        let body = t.trim_start();
                        let lead = t.chars().count() - body.chars().count();
                        if !body.is_empty() {
                            let _ = writeln!(s, r#"<text x="{}" y="{}" {}>{}</text>"#, n(r.min.x + o[0] + (col + lead) as f32 * char_w), n(y + baseline), fill(pal.color(*k)), esc(body));
                        }
                        col += lead + body.chars().count();
                    }
                }
                s.push_str("</g>\n");
            }
        }
    }
    s.push_str("</g>\n</svg>\n");
    s
}

// ---------- rendering ----------

struct Fonts {
    db: Arc<fontdb::Database>,
    family: String,
    /// egui's fonts in the order it falls back through them.
    bundled: Vec<fontdb::ID>,
}

/// egui's bundled fonts (so text matches the canvas), plus system fonts for missing glyphs.
fn fonts() -> &'static Fonts {
    static FONTS: OnceLock<Fonts> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let defs = egui::FontDefinitions::default();
        let mut family = "Ubuntu".to_string();
        let mut bundled = Vec::new();
        for name in ["Ubuntu-Light", "Hack", "NotoEmoji-Regular", "emoji-icon-font"] {
            let Some(data) = defs.font_data.get(name) else { continue };
            let ids = db.load_font_source(fontdb::Source::Binary(Arc::new(data.font.to_vec())));
            if name != "Hack" {
                bundled.extend(ids.iter().copied());
            }
            if name == "Ubuntu-Light" {
                if let Some(f) = ids.first().and_then(|id| db.face(*id)).and_then(|f| f.families.first()) {
                    family = f.0.clone();
                }
            }
        }
        for bytes in TextFont::EMBEDDED.iter().filter_map(|(f, ..)| f.bytes()) {
            db.load_font_source(fontdb::Source::Binary(Arc::new(bytes.to_vec())));
        }
        alias_core_fonts(&mut db);
        db.set_sans_serif_family(&family);
        Fonts { db: Arc::new(db), family, bundled }
    })
}

/// Register the bundled metric-compatible fonts under the core font names that SVGs commonly ask
/// for (e.g. Reactome icons use `font-family="Arial"`), wherever the system lacks them. The web
/// build has no system fonts, so without this such text matches no face and is dropped.
fn alias_core_fonts(db: &mut fontdb::Database) {
    let has = |db: &fontdb::Database, name: &str| db.faces().any(|f| f.families.iter().any(|(n, _)| n == name));
    let pairs: [(&str, &[&str]); 6] = [
        ("Arimo", &["Arial", "Helvetica"]),
        ("Tinos", &["Times New Roman", "Times"]),
        ("Cousine", &["Courier New", "Courier"]),
        ("Carlito", &["Calibri"]),
        ("Caladea", &["Cambria"]),
        ("Gelasio", &["Georgia"]),
    ];
    for (bundled, aliases) in pairs {
        let Some(face) = db.faces().find(|f| f.families.iter().any(|(n, _)| n == bundled)).cloned() else { continue };
        for alias in aliases {
            if !has(db, alias) {
                let mut info = face.clone();
                info.families = vec![(alias.to_string(), fontdb::Language::English_UnitedStates)];
                db.push_face_info(info);
            }
        }
    }
}

fn svg_options() -> usvg::Options<'static> {
    let f = fonts();
    usvg::Options { fontdb: f.db.clone(), font_family: f.family.clone(), ..Default::default() }
}

fn parse(svg: &str) -> Result<usvg::Tree, String> {
    usvg::Tree::from_str(svg, &svg_options()).map_err(|e| format!("svg: {e}"))
}

/// An SVG file's drawing's size in user units (see [`Vector`]).
pub fn svg_size(bytes: &[u8]) -> Option<(f32, f32)> {
    let size = Vector::parse(bytes).ok()?.size();
    Some((size.x, size.y))
}

/// Rasterise an SVG file (an upload) so its longer side is `max_side` pixels.
pub fn render_svg(bytes: &[u8], max_side: u32) -> Result<RgbaImage, String> {
    Vector::parse(bytes)?.render(max_side)
}

/// Longer side, in pixels, of the rendering that finds an SVG's drawing.
const TRIM_SIDE: u32 = 512;

/// A parsed SVG, drawn trimmed to its drawing: the part of the artboard that has anything
/// visible on it.
///
/// Files often have an artboard bigger than the drawing (BioArt exports every icon on a 500 x
/// 500 board, while its PNG of the same icon is cropped to the drawing), so an image shown with
/// the whole artboard would be padded and, placed with its PNG's proportions, squashed. The
/// drawing is found by rendering, since a geometric bounding box counts shapes hidden by clip
/// paths (BioArt fills a flask by clipping a big rectangle to the glass).
pub struct Vector {
    tree: usvg::Tree,
    /// The drawing's box on the artboard, in the file's user units.
    content: Rect,
    /// The colours it paints with, most used first (see [`Vector::colors`]).
    colors: Vec<[u8; 3]>,
}

impl Vector {
    /// Parse an SVG file (plain or gzipped) with the app's fonts.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let tree = usvg::Tree::from_data(bytes, &svg_options()).map_err(|e| format!("svg: {e}"))?;
        let content = content_rect(&tree);
        let colors = paint_colors(&tree);
        Ok(Self { tree, content, colors })
    }

    /// The flat colours its shapes are filled and stroked with (gradient stops included), most
    /// used first by the area of the shapes painted with them: the layers an image offers when
    /// the catalog names none.
    pub fn colors(&self) -> &[[u8; 3]] {
        &self.colors
    }

    /// The file as SVG text (as usvg writes it: every style resolved into attributes).
    pub fn to_svg(&self) -> String {
        self.tree.to_string(&usvg::WriteOptions::default())
    }

    /// The drawing with `layers` restyled: every fill, stroke and gradient stop in one of a
    /// layer's colours painted in its colour and opacity instead. `None` when no layer changes
    /// anything (or the restyled file fails to parse). Keeps the drawing's box, so a layer faded
    /// out does not resize the image.
    pub fn restyled(&self, layers: &[LayerStyle]) -> Option<Vector> {
        if layers.iter().all(LayerStyle::is_plain) {
            return None;
        }
        let xml = restyle_xml(&self.to_svg(), layers);
        match parse(&xml) {
            Ok(tree) => Some(Self { tree, content: self.content, colors: self.colors.clone() }),
            Err(e) => {
                log::warn!("restyled {e}");
                None
            }
        }
    }

    /// The file's own size (its artboard), in user units.
    pub fn artboard(&self) -> Vec2 {
        let s = self.tree.size();
        Vec2::new(s.width(), s.height())
    }

    /// The drawing's box on the artboard, in user units.
    pub fn content(&self) -> Rect {
        self.content
    }

    /// The drawing's size, in user units: what the image's pixels are proportioned to.
    pub fn size(&self) -> Vec2 {
        self.content.size()
    }

    /// Rasterise the drawing so its longer side is `max_side` pixels. The canvas calls this again
    /// whenever an image is shown larger than its last rendering, so SVG assets stay sharp at
    /// any zoom.
    pub fn render(&self, max_side: u32) -> Result<RgbaImage, String> {
        let c = self.content;
        let s = max_side as f32 / c.width().max(c.height());
        let (w, h) = (((c.width() * s).round() as u32).max(1), ((c.height() * s).round() as u32).max(1));
        let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h).ok_or_else(|| format!("{w} x {h} px is too large to render"))?;
        resvg::render(&self.tree, resvg::tiny_skia::Transform::from_scale(s, s).pre_translate(-c.min.x, -c.min.y), &mut pixmap.as_mut());
        pixmap_rgba(&pixmap)
    }
}

/// See [`Vector::colors`].
fn paint_colors(tree: &usvg::Tree) -> Vec<[u8; 3]> {
    fn rgb(c: usvg::Color) -> [u8; 3] {
        [c.red, c.green, c.blue]
    }
    fn add(paint: &usvg::Paint, area: f32, out: &mut Vec<([u8; 3], f32)>) {
        let mut one = |c: [u8; 3], a: f32| match out.iter_mut().find(|(o, _)| *o == c) {
            Some((_, w)) => *w += a,
            None => out.push((c, a)),
        };
        match paint {
            usvg::Paint::Color(c) => one(rgb(*c), area),
            usvg::Paint::LinearGradient(g) => g.stops().iter().for_each(|s| one(rgb(s.color()), area / g.stops().len() as f32)),
            usvg::Paint::RadialGradient(g) => g.stops().iter().for_each(|s| one(rgb(s.color()), area / g.stops().len() as f32)),
            usvg::Paint::Pattern(_) => {}
        }
    }
    fn walk(g: &usvg::Group, out: &mut Vec<([u8; 3], f32)>) {
        for node in g.children() {
            match node {
                usvg::Node::Group(g) => walk(g, out),
                usvg::Node::Text(t) => walk(t.flattened(), out),
                usvg::Node::Path(p) if p.is_visible() => {
                    let b = p.abs_bounding_box();
                    let area = b.width() * b.height();
                    if let Some(f) = p.fill() {
                        add(f.paint(), area, out);
                    }
                    // A stroke covers a band along the outline, not the box: count it lightly.
                    if let Some(s) = p.stroke() {
                        add(s.paint(), area * 0.1, out);
                    }
                }
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(tree.root(), &mut out);
    out.sort_by(|a, b| b.1.total_cmp(&a.1));
    out.into_iter().map(|(c, _)| c).collect()
}

/// [`Vector::restyled`]'s rewrite of a file as usvg writes it, which states every paint as an
/// attribute (`fill="#rrggbb"`, `fill-opacity`, and so on) on each element that uses it.
fn restyle_xml(xml: &str, layers: &[LayerStyle]) -> String {
    const PAINTS: [(&str, &str); 3] = [("fill", "fill-opacity"), ("stroke", "stroke-opacity"), ("stop-color", "stop-opacity")];
    let hex = |v: &str| -> Option<[u8; 3]> {
        let h = v.strip_prefix('#').filter(|h| h.len() == 6)?;
        let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
        Some([b(0)?, b(2)?, b(4)?])
    };
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        rest = &rest[lt..];
        let Some(gt) = rest.find('>') else { break };
        let (tag, after) = (&rest[..=gt], &rest[gt + 1..]);
        rest = after;
        if tag.starts_with("</") || tag.starts_with("<?") || tag.starts_with("<!") {
            out.push_str(tag);
            continue;
        }
        // `<name a="v" ...>` or `.../>`: usvg quotes every value with `"` and escapes any inside.
        let body = tag[1..tag.len() - 1].trim_end_matches('/');
        let close = &tag[1 + body.len()..];
        let name_end = body.find(char::is_whitespace).unwrap_or(body.len());
        let mut attrs: Vec<(String, String)> = Vec::new();
        let mut a = &body[name_end..];
        while let Some(eq) = a.find("=\"") {
            let key = a[..eq].trim().to_string();
            let Some(end) = a[eq + 2..].find('"') else { break };
            attrs.push((key, a[eq + 2..eq + 2 + end].to_string()));
            a = &a[eq + 2 + end + 1..];
        }
        let mut changed = false;
        for (paint, opacity) in PAINTS {
            let Some(i) = attrs.iter().position(|(k, _)| k == paint) else { continue };
            let Some(layer) = hex(&attrs[i].1).and_then(|c| layers.iter().find(|l| l.colors.contains(&c))) else { continue };
            if let Some([r, g, b]) = layer.color {
                attrs[i].1 = format!("#{r:02x}{g:02x}{b:02x}");
            }
            if layer.opacity < 1.0 {
                let o = layer.opacity.clamp(0.0, 1.0);
                match attrs.iter_mut().find(|(k, _)| k == opacity) {
                    Some((_, v)) => *v = n(v.parse::<f32>().unwrap_or(1.0) * o),
                    None => attrs.push((opacity.to_string(), n(o))),
                }
            }
            changed = true;
        }
        if !changed {
            out.push_str(tag);
            continue;
        }
        out.push('<');
        out.push_str(&body[..name_end]);
        for (k, v) in &attrs {
            let _ = write!(out, r#" {k}="{v}""#);
        }
        out.push_str(close);
    }
    out.push_str(rest);
    out
}

/// The box of everything visible on an SVG's artboard, from a rendering of it `TRIM_SIDE` pixels
/// on the longer side: the pixels with any opacity, converted back to user units (so the edges
/// are at most a pixel of that rendering generous). The whole artboard when nothing is visible.
fn content_rect(tree: &usvg::Tree) -> Rect {
    let size = tree.size();
    let full = Rect::from_min_size(Pos2::ZERO, Vec2::new(size.width(), size.height()));
    let s = TRIM_SIDE as f32 / size.width().max(size.height());
    let (w, h) = (((size.width() * s).ceil() as u32).max(1), ((size.height() * s).ceil() as u32).max(1));
    let Some(mut pixmap) = resvg::tiny_skia::Pixmap::new(w, h) else { return full };
    resvg::render(tree, resvg::tiny_skia::Transform::from_scale(s, s), &mut pixmap.as_mut());
    let px = pixmap.pixels();
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for y in 0..h {
        let row = &px[(y * w) as usize..((y + 1) * w) as usize];
        let Some(first) = row.iter().position(|p| p.alpha() > 0) else { continue };
        let last = row.iter().rposition(|p| p.alpha() > 0).unwrap_or(first);
        x0 = x0.min(first as u32);
        x1 = x1.max(last as u32);
        y0 = y0.min(y);
        y1 = y1.max(y);
    }
    if x0 > x1 {
        return full;
    }
    let (l, t) = ((x0 as f32 / s).max(0.0), (y0 as f32 / s).max(0.0));
    let (r, b) = (((x1 + 1) as f32 / s).min(size.width()), ((y1 + 1) as f32 / s).min(size.height()));
    Rect::from_min_max(Pos2::new(l, t), Pos2::new(r, b))
}

fn pixmap_rgba(pixmap: &resvg::tiny_skia::Pixmap) -> Result<RgbaImage, String> {
    let mut raw = Vec::with_capacity((pixmap.width() * pixmap.height() * 4) as usize);
    for p in pixmap.pixels() {
        let c = p.demultiply();
        raw.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    RgbaImage::from_raw(pixmap.width(), pixmap.height(), raw).ok_or_else(|| "render failed".to_string())
}

fn rasterise(tree: &usvg::Tree, region: Rect, dpi: u32) -> Result<RgbaImage, String> {
    let [pw, ph] = pixel_size(region, dpi);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(pw, ph).ok_or_else(|| format!("{pw} x {ph} px is too large to render; lower the DPI"))?;
    let size = tree.size();
    let t = resvg::tiny_skia::Transform::from_scale(pw as f32 / size.width(), ph as f32 / size.height());
    resvg::render(tree, t, &mut pixmap.as_mut());
    pixmap_rgba(&pixmap)
}

/// Insert a pHYs chunk (pixels per metre) right after IHDR so viewers and journals see the DPI.
fn png_with_dpi(png: Vec<u8>, dpi: u32) -> Vec<u8> {
    const IHDR_END: usize = 8 + 8 + 13 + 4;
    if png.len() < IHDR_END {
        return png;
    }
    let ppm = (dpi as f64 / 0.0254).round() as u32;
    let mut chunk = Vec::with_capacity(21);
    chunk.extend_from_slice(&9u32.to_be_bytes());
    let mut body = b"pHYs".to_vec();
    body.extend_from_slice(&ppm.to_be_bytes());
    body.extend_from_slice(&ppm.to_be_bytes());
    body.push(1); // unit: metre
    let mut crc = flate2::Crc::new();
    crc.update(&body);
    chunk.extend_from_slice(&body);
    chunk.extend_from_slice(&crc.sum().to_be_bytes());
    let mut out = Vec::with_capacity(png.len() + chunk.len());
    out.extend_from_slice(&png[..IHDR_END]);
    out.extend_from_slice(&chunk);
    out.extend_from_slice(&png[IHDR_END..]);
    out
}

/// Composite onto white (for formats without alpha).
fn flatten(img: &RgbaImage) -> image::RgbImage {
    image::RgbImage::from_fn(img.width(), img.height(), |x, y| {
        let p = img.get_pixel(x, y).0;
        let a = p[3] as f32 / 255.0;
        image::Rgb([0, 1, 2].map(|i| (p[i] as f32 * a + 255.0 * (1.0 - a)).round() as u8))
    })
}

/// Put the credits of the artwork in a figure into the SVG, as its `<desc>`.
fn svg_with_credits(svg: &str, credits: &str) -> String {
    let Some(end) = svg.find('>') else { return svg.to_string() };
    format!("{}\n<desc>Image credits:\n{}</desc>{}", &svg[..=end], esc(credits), &svg[end + 1..])
}

/// Add an iTXt chunk (UTF-8 text, keyword "Comment") with the credits before IEND.
fn png_with_credits(png: Vec<u8>, credits: &str) -> Vec<u8> {
    if png.len() < 12 {
        return png;
    }
    let mut body = b"iTXt".to_vec();
    body.extend_from_slice(b"Comment\0\0\0\0\0"); // keyword, no compression, no language or translation
    body.extend_from_slice(format!("Image credits:\n{credits}").as_bytes());
    let mut crc = flate2::Crc::new();
    crc.update(&body);
    let iend = png.len() - 12;
    let mut out = Vec::with_capacity(png.len() + body.len() + 8);
    out.extend_from_slice(&png[..iend]);
    out.extend_from_slice(&((body.len() - 4) as u32).to_be_bytes());
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc.sum().to_be_bytes());
    out.extend_from_slice(&png[iend..]);
    out
}

/// One PDF with a page per SVG, each page the size of its SVG.
fn pdf_pages(svgs: &[String]) -> Result<Vec<u8>, String> {
    use pdf_writer::{Content, Finish, Name, Pdf, Rect as PdfRect, Ref};
    let mut alloc = Ref::new(1);
    let catalog = alloc.bump();
    let tree_ref = alloc.bump();
    let mut pdf = Pdf::new();
    let mut kids = Vec::new();
    for svg in svgs {
        let tree = parse(svg)?;
        // Document pixels to points.
        let (w, h) = (tree.size().width() * 72.0 / DPI, tree.size().height() * 72.0 / DPI);
        let (chunk, svg_ref) = svg2pdf::to_chunk(&tree, svg2pdf::ConversionOptions::default()).map_err(|e| e.to_string())?;
        let mut map = HashMap::new();
        let chunk = chunk.renumber(|old| *map.entry(old).or_insert_with(|| alloc.bump()));
        let svg_ref = map[&svg_ref];
        let (page_ref, content_ref) = (alloc.bump(), alloc.bump());
        let mut page = pdf.page(page_ref);
        page.media_box(PdfRect::new(0.0, 0.0, w, h));
        page.parent(tree_ref);
        page.contents(content_ref);
        page.resources().x_objects().pair(Name(b"S"), svg_ref);
        page.finish();
        // The SVG's form XObject is a unit square: scale it to the page.
        let mut content = Content::new();
        content.transform([w, 0.0, 0.0, h, 0.0, 0.0]).x_object(Name(b"S"));
        pdf.stream(content_ref, &content.finish());
        pdf.extend(&chunk);
        kids.push(page_ref);
    }
    pdf.catalog(catalog).pages(tree_ref);
    pdf.pages(tree_ref).count(kids.len() as i32).kids(kids);
    Ok(pdf.finish())
}

/// The file's bytes. `credits` (one per line, possibly empty) go into SVG and PNG files as
/// metadata.
fn encode(svg: &str, region: Rect, opts: &ExportOptions, dpi: u32, credits: &str) -> Result<Vec<u8>, String> {
    let err = |e: &dyn std::fmt::Display| e.to_string();
    Ok(match opts.format {
        ExportFormat::Svg if !credits.is_empty() => svg_with_credits(svg, credits).into_bytes(),
        ExportFormat::Svg => svg.as_bytes().to_vec(),
        ExportFormat::Biocartoon => return Err("A .biocartoon file is a project, not a rendered slide".into()),
        ExportFormat::Pdf => {
            let tree = parse(svg)?;
            svg2pdf::to_pdf(&tree, svg2pdf::ConversionOptions::default(), svg2pdf::PageOptions { dpi: DPI }).map_err(|e| err(&e))?
        }
        ExportFormat::Png => {
            let img = rasterise(&parse(svg)?, region, dpi)?;
            let mut buf = Vec::new();
            img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png).map_err(|e| err(&e))?;
            let png = png_with_dpi(buf, dpi);
            if credits.is_empty() { png } else { png_with_credits(png, credits) }
        }
        ExportFormat::Jpg => {
            let img = flatten(&rasterise(&parse(svg)?, region, dpi)?);
            let mut buf = Vec::new();
            let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, opts.jpg_quality.clamp(1, 100));
            enc.set_pixel_density(image::codecs::jpeg::PixelDensity::dpi(dpi.min(u16::MAX as u32) as u16));
            enc.encode_image(&img).map_err(|e| err(&e))?;
            buf
        }
        ExportFormat::Tif => {
            use tiff::encoder::{colortype, Compression, Rational, TiffEncoder};
            let img = rasterise(&parse(svg)?, region, dpi)?;
            let mut buf = std::io::Cursor::new(Vec::new());
            let mut enc = TiffEncoder::new(&mut buf).map_err(|e| err(&e))?.with_compression(Compression::Lzw);
            let (w, h) = img.dimensions();
            let res = Rational { n: dpi, d: 1 };
            if img.pixels().all(|p| p.0[3] == 255) {
                let rgb = flatten(&img);
                let mut im = enc.new_image::<colortype::RGB8>(w, h).map_err(|e| err(&e))?;
                im.resolution(tiff::tags::ResolutionUnit::Inch, res);
                im.write_data(rgb.as_raw()).map_err(|e| err(&e))?;
            } else {
                let mut im = enc.new_image::<colortype::RGBA8>(w, h).map_err(|e| err(&e))?;
                im.resolution(tiff::tags::ResolutionUnit::Inch, res);
                im.write_data(img.as_raw()).map_err(|e| err(&e))?;
            }
            buf.into_inner()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{ImageSource, ShapeKind};

    /// Characters a text item's font lacks get a fallback `<tspan>` of their own, so the rest of
    /// the row keeps its font.
    #[test]
    fn missing_glyphs_fall_back_alone() {
        let xs: Vec<f32> = (0..7).map(|i| i as f32 * 10.0).collect();
        assert_eq!(row_markup(TextFont::Lato, "β-sheet", &xs), format!(r#"<tspan font-family="{}">β</tspan><tspan x="10">-sheet</tspan>"#, esc(&font_family())));
        assert_eq!(row_markup(TextFont::Lato, "a < b", &xs), "a &lt; b");
        assert_eq!(row_markup(TextFont::Sans, "β-sheet", &xs), "β-sheet");
        // Ubuntu has no emoji: only the emoji leaves it, not the whole row.
        assert!(row_markup(TextFont::Sans, "DNA 🧬 x", &xs).starts_with(r#"DNA <tspan x="40" font-family="#), "{}", row_markup(TextFont::Sans, "DNA 🧬 x", &xs));
    }

    /// egui lays right-to-left words out reversed and repeats their letters as zero-width
    /// glyphs; the SVG gets each word once, in logical order.
    #[test]
    fn rtl_text_exports_once() {
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(Default::default(), |_| {});
        out.textures_delta.clear();
        let it = Item::new(1, "T", ItemKind::text("DNA עברית שלום", 32.0, [0, 0, 0, 255], TextAlign::Left, false), Pos2::ZERO, Vec2::new(900.0, 45.0));
        let svg = build_svg(&ctx, &[&it], Rect::from_min_size(Pos2::ZERO, Vec2::new(960.0, 100.0)), None, &HashMap::new());
        let text: String = svg.lines().filter(|l| l.contains("<text")).collect();
        assert_eq!(text.matches("עברית").count(), 1, "{text}");
        assert_eq!(text.matches("שלום").count(), 1, "{text}");
        assert!(!text.contains("תירבע"), "{text}");
        assert_eq!(text.matches("<text").count(), 1, "the repeated letters split the row:\n{text}");
    }

    /// With only the bundled fonts (as on the web), text naming Arial still renders, via Arimo.
    #[test]
    fn core_font_names_resolve_without_system_fonts() {
        let mut db = fontdb::Database::new();
        for bytes in TextFont::EMBEDDED.iter().filter_map(|(f, ..)| f.bytes()) {
            db.load_font_source(fontdb::Source::Binary(Arc::new(bytes.to_vec())));
        }
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><text font-family="Arial" font-size="12" font-weight="bold" x="8" y="16">THF</text></svg>"#;
        let has_text = |db: &fontdb::Database| {
            let tree = usvg::Tree::from_str(svg, &usvg::Options { fontdb: Arc::new(db.clone()), ..Default::default() }).unwrap();
            tree.root().has_children()
        };
        assert!(!has_text(&db));
        alias_core_fonts(&mut db);
        assert!(has_text(&db));
    }

    /// An uploaded SVG loads as pixels at the requested size, keeping its aspect ratio.
    #[test]
    fn svg_uploads() {
        let svg = br##"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" width="200" height="100"><rect width="200" height="100" fill="#f00"/></svg>"##;
        assert!(crate::assets::is_svg(svg));
        assert_eq!(crate::assets::image_size(svg), Some((200.0, 100.0)));
        let img = crate::assets::load_image(svg, 400).unwrap();
        assert_eq!(img.dimensions(), (400, 200));
        assert_eq!(img.get_pixel(10, 10).0, [255, 0, 0, 255]);
        assert!(!crate::assets::is_svg(crate::ICON_PNG));
    }

    /// An SVG is trimmed to its drawing: a 100 x 100 artboard with a 50 x 25 rectangle on it
    /// has that rectangle's size and renders as just the rectangle, as an upload and in export.
    /// An item placed for the whole artboard (an older document) is moved onto the drawing in
    /// export, one that frames the drawing already is not.
    #[test]
    fn svg_trimmed_to_drawing() {
        let file = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect x="25" y="50" width="50" height="25" fill="#f00"/></svg>"##;
        let v = Vector::parse(file).unwrap();
        assert_eq!(v.artboard(), Vec2::splat(100.0));
        let c = v.content();
        assert!((c.min - Pos2::new(25.0, 50.0)).length() < 0.3 && (c.max - Pos2::new(75.0, 75.0)).length() < 0.3, "{c:?}");
        let (w, h) = crate::assets::image_size(file).unwrap();
        assert!((w - 50.0).abs() < 0.3 && (h - 25.0).abs() < 0.3, "{w} x {h}");
        let img = crate::assets::load_image(file, 100).unwrap();
        assert_eq!(img.dimensions(), (100, 50));
        assert_eq!(img.get_pixel(2, 2).0, [255, 0, 0, 255], "the drawing fills its rendering");
        assert_eq!(img.get_pixel(97, 47).0, [255, 0, 0, 255]);

        let source = ImageSource::file("padded.svg".into());
        let pics = HashMap::from([(source.cache_key(), Picture::from_bytes(file).unwrap())]);
        let kind = ItemKind::Image { source: source.clone(), crop: [0.0, 0.0, 1.0, 1.0], overlay: [0; 4], layers: Vec::new() };
        let old = Item::new(1, "old", kind.clone(), Pos2::ZERO, Vec2::splat(200.0));
        let mut new = Item::new(2, "new", kind, Pos2::new(300.0, 0.0), Vec2::new(200.0, 100.0));
        new.fitted = true;
        let ctx = egui::Context::default();
        let svg = build_svg(&ctx, &[&old, &new], Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 300.0)), None, &pics);
        assert!(svg.contains(r#"<svg x="50" y="100" width="100" height="50" viewBox="25 50 50 25""#), "the old item's drawing stays where it was:\n{svg}");
        assert!(svg.contains(r#"<svg x="300" y="0" width="200" height="100" viewBox="25 50 50 25""#), "the new item is the drawing:\n{svg}");
    }

    /// Uploaded SVGs export as vector shapes, with crop, flip and tint applied as on the canvas.
    #[test]
    fn svg_stays_vector() {
        // Left half white, right half blue.
        let file = br##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100"><defs><linearGradient id="g"><stop stop-color="#00f"/><stop offset="1" stop-color="#00f" stop-opacity="0.999"/></linearGradient></defs><rect width="100" height="100" fill="#fff"/><rect x="100" width="100" height="100" fill="url(#g)"/></svg>"##;
        let source = ImageSource::file("upload.svg".into());
        let pics = HashMap::from([(source.cache_key(), Picture::from_bytes(file).unwrap())]);
        let image = |id, crop, overlay, pos: Pos2, size: Vec2| Item::new(id, "Upload", ItemKind::Image { source: source.clone(), crop, overlay, layers: Vec::new() }, pos, size);
        let cropped = image(1, [0.5, 0.0, 1.0, 1.0], [0; 4], Pos2::new(0.0, 0.0), Vec2::splat(100.0));
        let mut flipped = image(2, [0.0, 0.0, 1.0, 1.0], [0; 4], Pos2::new(200.0, 0.0), Vec2::new(200.0, 100.0));
        flipped.flip_h = true;
        let tinted = image(3, [0.0, 0.0, 0.5, 1.0], [0, 255, 0, 255], Pos2::new(0.0, 200.0), Vec2::splat(100.0));
        let region = Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0));
        let ctx = egui::Context::default();

        let svg = build_svg(&ctx, &[&cropped, &flipped, &tinted], region, Some([0, 0, 0, 255]), &pics);
        assert!(!svg.contains("data:image/png"), "SVG uploads must not be rasterised");
        // Each copy's IDs are prefixed, so the gradient isn't defined twice.
        assert_eq!(svg.matches(r#"id="svg1-g""#).count(), 1);

        let img = rasterise(&parse(&svg).unwrap(), region, DPI as u32).unwrap();
        let px = |x: f32, y: f32| img.get_pixel((x * img.width() as f32 / 400.0) as u32, (y * img.height() as f32 / 300.0) as u32).0;
        assert_eq!(px(50.0, 50.0), [0, 0, 255, 255], "crop shows the right half");
        assert_eq!(px(250.0, 50.0), [0, 0, 255, 255], "flip puts the right half on the left");
        assert_eq!(px(350.0, 50.0), [255, 255, 255, 255]);
        assert_eq!(px(50.0, 250.0), [0, 255, 0, 255], "tint multiplies white to green");
        assert_eq!(px(150.0, 50.0), [0, 0, 0, 255], "crop clips the rest");

        // Untinted, the PDF has no raster images at all.
        let svg = build_svg(&ctx, &[&cropped, &flipped], region, None, &pics);
        let pdf = svg2pdf::to_pdf(&parse(&svg).unwrap(), svg2pdf::ConversionOptions::default(), svg2pdf::PageOptions { dpi: DPI }).unwrap();
        assert!(!pdf.windows(13).any(|w| w == b"/Subtype /Ima" || w == b"/Subtype/Imag"), "PDF should be vector only");
    }

    /// A drawing's colours are its layers: found through CSS classes as BioArt paints, most used
    /// first, and restyled one at a time on the canvas's vector form and in exports.
    #[test]
    fn layers_recolor_and_fade() {
        // A flask: pink liquid filling most of it, a white glass band and a dark outline.
        let file = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><style>.liquid{fill:#f49dc0}.glass{fill:#fff;stroke:#303030;stroke-width:2}</style>
            <rect class="glass" x="0" y="0" width="100" height="20"/><rect class="liquid" x="0" y="20" width="100" height="80"/></svg>"##;
        let v = Vector::parse(file).unwrap();
        assert_eq!(v.colors(), [[0xf4, 0x9d, 0xc0], [255, 255, 255], [0x30, 0x30, 0x30]]);
        let liquid = |color, opacity| LayerStyle { colors: vec![[0xf4, 0x9d, 0xc0]], color, opacity };
        assert!(v.restyled(&[liquid(None, 1.0)]).is_none(), "nothing to restyle");

        let px = |v: &Vector, y: u32| v.render(100).unwrap().get_pixel(50, y).0;
        let blue = v.restyled(&[liquid(Some([0, 0, 255]), 1.0)]).unwrap();
        assert_eq!(px(&blue, 60), [0, 0, 255, 255]);
        assert_eq!(px(&blue, 10), [255, 255, 255, 255], "the glass keeps its colour");
        assert!(blue.to_svg().contains("#0000ff") && !blue.to_svg().contains("#f49dc0"), "its text has the new colour too");
        let faded = v.restyled(&[liquid(None, 0.0)]).unwrap();
        assert_eq!(px(&faded, 60)[3], 0);
        assert_eq!(faded.content(), v.content(), "a faded layer keeps the image's box");
        // A stroke keeps its opacity's share: half of an already half-opaque outline.
        let half = restyle_xml(r##"<path fill="#303030" stroke="#303030" stroke-opacity="0.5" d="M0 0L1 1"/>"##, &[LayerStyle { colors: vec![[0x30, 0x30, 0x30]], color: Some([1, 2, 3]), opacity: 0.5 }]);
        assert_eq!(half, r##"<path fill="#010203" stroke="#010203" stroke-opacity="0.25" d="M0 0L1 1" fill-opacity="0.5"/>"##);

        let source = ImageSource::file("flask.svg".into());
        let pics = HashMap::from([(source.cache_key(), Picture::from_bytes(file).unwrap())]);
        let it = Item::new(1, "Flask", ItemKind::Image { source, crop: [0.0, 0.0, 1.0, 1.0], overlay: [0; 4], layers: vec![liquid(Some([0, 0, 255]), 1.0)] }, Pos2::ZERO, Vec2::splat(100.0));
        let region = Rect::from_min_size(Pos2::ZERO, Vec2::splat(100.0));
        let svg = build_svg(&egui::Context::default(), &[&it], region, None, &pics);
        assert!(svg.contains("#0000ff") && !svg.contains("#f49dc0"), "the export has the layer's new colour");
    }

    /// Exports a slide with every item kind in every format. Set BIOCARTOON_EXPORT_TEST_DIR to
    /// keep the files for inspection.
    #[test]
    fn exports_every_format() {
        let ctx = egui::Context::default();
        let mut defs = egui::FontDefinitions::default();
        TextFont::install(&mut defs);
        ctx.set_fonts(defs);
        ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
        let dir = std::env::var("BIOCARTOON_EXPORT_TEST_DIR").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir().join("biocartoon-export-test"));
        std::fs::create_dir_all(&dir).unwrap();
        let icon = dir.join("icon.png");
        std::fs::write(&icon, crate::ICON_PNG).unwrap();

        let mut slide = Slide::default();
        let src = ImageSource::file(icon.display().to_string());
        // An AI edit of a catalog asset: exports still credit the original.
        let catalog = ImageSource::Asset { id: "servier/cell".into(), key: "x".into(), svg: None, title: "Cell".into(), source: "servier".into(), credit: "\"Cell\", Servier Medical Art, CC-BY-4.0 & co".into(), license: "CC-BY-4.0".into() };
        let edited = catalog.derived(icon.display().to_string());
        let mut img = Item::new(1, "Cell icon", ItemKind::Image { source: edited, crop: [0.0, 0.0, 1.0, 1.0], overlay: [0; 4], layers: Vec::new() }, Pos2::new(40.0, 40.0), Vec2::splat(200.0));
        img.rotation = 0.3;
        slide.items.push(img);
        slide.items.push(Item::new(2, "Tinted", ItemKind::Image { source: src.clone(), crop: [0.25, 0.25, 0.75, 0.75], overlay: [255, 0, 0, 128], layers: Vec::new() }, Pos2::new(300.0, 40.0), Vec2::splat(120.0)));
        // Some of its letters styled apart: "fairly" plain red, "long" struck, "wraps" italic underlined.
        let mut title = ItemKind::text("Hello BioCartoon, a fairly long title that wraps", 28.0, [20, 20, 20, 255], TextAlign::Center, true);
        crate::doc::set_flag(&mut title, Some(20..26), crate::doc::TextFlag::Bold, false);
        crate::doc::set_color(&mut title, Some(20..26), [220, 38, 38, 255]);
        crate::doc::set_flag(&mut title, Some(27..31), crate::doc::TextFlag::Strike, true);
        crate::doc::set_flag(&mut title, Some(43..48), crate::doc::TextFlag::Italic, true);
        crate::doc::set_flag(&mut title, Some(43..48), crate::doc::TextFlag::Underline, true);
        slide.items.push(Item::new(3, "Title", title, Pos2::new(450.0, 40.0), Vec2::new(400.0, 80.0)));
        slide.items.push(Item::new(4, "Star", ItemKind::Shape { shape: ShapeKind::Star, fill: [47, 128, 237, 255], stroke: [0, 0, 0, 255], stroke_width: 2.0 }, Pos2::new(450.0, 200.0), Vec2::splat(150.0)));
        slide.items.push(Item::new(5, "Arrow", ItemKind::Line { kind: crate::doc::LineKind::BentArrow, color: [200, 50, 50, 255], width: 3.0, style: Default::default(), points: Vec::new() }, Pos2::new(100.0, 400.0), Vec2::new(300.0, 150.0)));
        slide.items.push(Item::new(6, "Inhibit", ItemKind::Line { kind: crate::doc::LineKind::Inhibitor, color: [0, 0, 0, 255], width: 2.0, style: Default::default(), points: Vec::new() }, Pos2::new(100.0, 600.0), Vec2::new(200.0, 0.0)));
        slide.items.push(Item::new(7, "Table", ItemKind::Table { rows: 2, cols: 2, cells: vec!["Gene".into(), "Fold".into(), "TP53".into(), "2.4".into()], header: true, border: [100, 100, 100, 255], fill: [255, 255, 255, 255], header_fill: [220, 230, 250, 255], text_color: [0, 0, 0, 255], font_size: 14.0 }, Pos2::new(650.0, 450.0), Vec2::new(240.0, 80.0)));
        slide.items.push(Item::new(8, "Code", ItemKind::Code { code: "def f(x):\n    return x * 2  # double\n".into(), language: crate::doc::CodeLang::Auto, theme: crate::doc::CodeTheme::Dracula, font_size: 14.0, line_numbers: true, window: true, rounded: true }, Pos2::new(600.0, 200.0), Vec2::new(300.0, 140.0)));

        let page = Rect::from_min_size(Pos2::ZERO, Vec2::new(960.0, 672.0));
        let mut requests = HashMap::new();
        requests.insert(src.cache_key(), Request::Path(icon.clone()));
        let job = |format, dpi, transparent, individual, name: &str| ExportJob {
            path: dir.join(name),
            opts: ExportOptions { format, dpi, transparent, individual, crop: false, all_slides: false, pdf_pages: true, jpg_quality: 90 },
            slides: vec![(1, slide.clone())],
            page,
            background: [250, 245, 235, 255],
            requests: requests.clone(),
            fallbacks: HashMap::new(),
            downloads: HashMap::new(),
        };
        for (f, name) in [(ExportFormat::Png, "slide.png"), (ExportFormat::Jpg, "slide.jpg"), (ExportFormat::Tif, "slide.tif"), (ExportFormat::Pdf, "slide.pdf"), (ExportFormat::Svg, "slide.svg")] {
            run(&ctx, job(f, 150, false, false, name)).unwrap();
        }
        // The estimate is the written size, until the image is too big to render whole.
        for (f, name) in [(ExportFormat::Png, "slide.png"), (ExportFormat::Jpg, "slide.jpg"), (ExportFormat::Pdf, "slide.pdf"), (ExportFormat::Svg, "slide.svg")] {
            let est = estimate(&ctx, &job(f, 150, false, false, name)).unwrap();
            assert_eq!(est, SizeEstimate { bytes: std::fs::metadata(dir.join(name)).unwrap().len(), files: 1, approx: false }, "{name}");
        }
        let big = estimate(&ctx, &job(ExportFormat::Png, 1800, false, false, "big.png")).unwrap();
        assert!(big.approx && big.bytes > std::fs::metadata(dir.join("slide.png")).unwrap().len());
        // Lower JPG quality, smaller file.
        let mut low = job(ExportFormat::Jpg, 150, false, false, "low.jpg");
        low.opts.jpg_quality = 30;
        assert!(estimate(&ctx, &low).unwrap().bytes < std::fs::metadata(dir.join("slide.jpg")).unwrap().len());
        run(&ctx, job(ExportFormat::Png, 300, true, false, "transparent.png")).unwrap();
        let msg = run(&ctx, job(ExportFormat::Png, 300, true, true, "each.png")).unwrap();
        assert!(msg.contains("2 images"), "{msg}");

        let png = image::open(dir.join("slide.png")).unwrap();
        assert_eq!((png.width(), png.height()), (1500, 1050));
        let mut crop = job(ExportFormat::Png, 150, false, false, "cropped.png");
        crop.opts.crop = true;
        run(&ctx, crop).unwrap();
        let [w, h] = pixel_size(images_bounds(&slide).unwrap(), 150);
        let cropped = image::open(dir.join("cropped.png")).unwrap();
        assert_eq!((cropped.width(), cropped.height()), (w, h));
        assert!(w < 1500 && h < 1050);
        let t = image::open(dir.join("transparent.png")).unwrap().to_rgba8();
        assert_eq!(t.get_pixel(5, 5).0[3], 0, "background should be transparent");
        // The rotated image's tight box is larger than its unrotated 200 x 200.
        let each = image::open(dir.join("each-01-cell-icon.png")).unwrap();
        assert!(each.width() > 625 && each.width() == each.height());
        let each2 = image::open(dir.join("each-02-tinted.png")).unwrap();
        assert_eq!((each2.width(), each2.height()), (375, 375));
        let bytes = std::fs::read(dir.join("slide.png")).unwrap();
        assert!(bytes.windows(4).any(|w| w == b"pHYs"));
        assert!(bytes.windows(12).any(|w| w == b"Servier Medi"), "PNG should carry the credits");
        let svg = std::fs::read_to_string(dir.join("slide.svg")).unwrap();
        assert!(svg.contains("<desc>Image credits:\n&quot;Cell&quot;, Servier Medical Art, CC-BY-4.0 &amp; co</desc>"), "{}", &svg[..300]);
        // The credits chunk leaves a valid PNG.
        assert_eq!(image::open(dir.join("transparent.png")).unwrap().width(), 3000);

        // All slides: one file per slide; cropping skips the slide without images.
        let text_only = Slide { name: "Text".into(), items: vec![slide.items[2].clone()], comments: Vec::new(), background: None };
        let mut all = job(ExportFormat::Png, 72, false, false, "deck.png");
        all.slides = vec![(1, slide.clone()), (2, text_only.clone())];
        let msg = run(&ctx, all).unwrap();
        assert!(msg.contains("2 slides"), "{msg}");
        assert!(dir.join("deck-slide1.png").exists() && dir.join("deck-slide2.png").exists());
        let mut each = job(ExportFormat::Png, 72, false, true, "deckeach.png");
        each.slides = vec![(1, slide.clone()), (2, text_only.clone())];
        run(&ctx, each).unwrap();
        assert!(dir.join("deckeach-slide1-01-cell-icon.png").exists());
        let mut crop = job(ExportFormat::Png, 72, false, false, "deckcrop.png");
        crop.slides = vec![(1, slide.clone()), (2, text_only)];
        crop.opts.crop = true;
        run(&ctx, crop).unwrap();
        assert!(dir.join("deckcrop-slide1.png").exists() && !dir.join("deckcrop-slide2.png").exists());

        // PDF of all slides: one file with a page each, unless asked for a file per slide.
        let mut pdf = job(ExportFormat::Pdf, 72, false, false, "deck.pdf");
        pdf.slides = vec![(1, slide.clone()), (2, slide.clone()), (3, slide.clone())];
        let msg = run(&ctx, pdf).unwrap();
        assert!(msg.contains("3 slides"), "{msg}");
        let bytes = std::fs::read(dir.join("deck.pdf")).unwrap();
        assert!(bytes.windows(8).any(|w| w == b"/Count 3"));
        assert_eq!(bytes.windows(10).filter(|w| w == b"/Type /Pag").count(), 4, "a page tree and three pages");
        assert!(!dir.join("deck-slide1.pdf").exists());
        let mut each = job(ExportFormat::Pdf, 72, false, false, "split.pdf");
        each.slides = vec![(1, slide.clone()), (2, slide.clone())];
        each.opts.pdf_pages = false;
        run(&ctx, each).unwrap();
        assert!(dir.join("split-slide1.pdf").exists() && dir.join("split-slide2.pdf").exists() && !dir.join("split.pdf").exists());
    }
}
