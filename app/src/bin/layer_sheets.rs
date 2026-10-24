//! Renders the pictures the catalog's layer namer (biocartoon: data/name_layers.py) shows a
//! model: for every SVG under a folder, the drawing itself and, for each colour layer the editor
//! offers (the same colours, read by the same code, as the Layers panel), the drawing with that
//! layer in a highlight colour and the rest faded.
//!
//!   cargo run --release --bin layer_sheets -- <svg dir> <out dir> [--side 384]
//!
//! `<svg dir>/<source>/<slug>.svg` gives `<out dir>/<source>/<slug>/` holding `drawing.png`,
//! `layer-1.png`, ... and `meta.json` (`{"colors": ["#rrggbb", ...]}`, layer 1 first). A drawing
//! with fewer than two colours gets a `meta.json` with no colours. Assets already done are skipped.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use biocartoon::doc::LayerStyle;
use biocartoon::export::Vector;
use image::{Rgba, RgbaImage};

/// As many layers as the Layers panel offers from a drawing's own colours (panels.rs).
const MAX_LAYERS: usize = 8;
/// How much of its own opacity the rest of the drawing keeps around a highlighted layer.
const FADE: f32 = 0.15;

fn svgs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            svgs(&p, out);
        } else if p.extension().is_some_and(|x| x == "svg") {
            out.push(p);
        }
    }
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

/// Magenta, unless the layer is near it already.
fn highlight(c: [u8; 3]) -> [u8; 3] {
    let d = |a: [u8; 3], b: [u8; 3]| (0..3).map(|i| (a[i] as i32 - b[i] as i32).pow(2)).sum::<i32>();
    if d(c, [255, 0, 255]) < 120 * 120 {
        [0, 200, 83]
    } else {
        [255, 0, 255]
    }
}

/// `img` over a flat background, so the model sees what a transparent area is.
fn flatten(img: &RgbaImage, bg: [u8; 3]) -> RgbaImage {
    RgbaImage::from_fn(img.width(), img.height(), |x, y| {
        let p = img.get_pixel(x, y).0;
        let a = p[3] as f32 / 255.0;
        let mix = |i: usize| (p[i] as f32 * a + bg[i] as f32 * (1.0 - a)).round() as u8;
        Rgba([mix(0), mix(1), mix(2), 255])
    })
}

fn sheet(svg: &Path, dir: &Path, side: u32) -> Result<(), String> {
    let meta = dir.join("meta.json");
    if meta.exists() {
        return Ok(());
    }
    let vector = Vector::parse(&std::fs::read(svg).map_err(|e| e.to_string())?)?;
    let all = vector.colors();
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let offered: &[[u8; 3]] = if all.len() < 2 { &[] } else { &all[..all.len().min(MAX_LAYERS)] };
    if !offered.is_empty() {
        let save = |img: RgbaImage, name: &str, bg| flatten(&img, bg).save(dir.join(name)).map_err(|e| e.to_string());
        // Light gray, so white parts of the drawing show.
        save(vector.render(side)?, "drawing.png", [225, 225, 225])?;
        for (i, &c) in offered.iter().enumerate() {
            let layers: Vec<LayerStyle> = all
                .iter()
                .map(|&o| if o == c { LayerStyle { colors: vec![o], color: Some(highlight(c)), opacity: 1.0 } } else { LayerStyle { colors: vec![o], color: None, opacity: FADE } })
                .collect();
            let v = vector.restyled(&layers).ok_or("could not restyle")?;
            save(v.render(side)?, &format!("layer-{}.png", i + 1), [255, 255, 255])?;
        }
    }
    let colors: Vec<String> = offered.iter().map(|&c| hex(c)).collect();
    std::fs::write(&meta, serde_json::json!({ "colors": colors }).to_string()).map_err(|e| e.to_string())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let side = args.iter().position(|a| a == "--side").and_then(|i| args.get(i + 1)).and_then(|s| s.parse().ok()).unwrap_or(384);
    let pos: Vec<&String> = args.iter().enumerate().filter(|(i, a)| !a.starts_with("--") && (*i == 0 || args[i - 1] != "--side")).map(|(_, a)| a).collect();
    let [src, out] = pos[..] else {
        eprintln!("usage: layer_sheets <svg dir> <out dir> [--side 384]");
        std::process::exit(2);
    };
    let (src, out) = (PathBuf::from(src), PathBuf::from(out));
    let mut files = Vec::new();
    svgs(&src, &mut files);
    files.sort();
    let next = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(svg) = files.get(i) else { break };
                let rel = svg.strip_prefix(&src).unwrap().with_extension("");
                if let Err(e) = sheet(svg, &out.join(&rel), side) {
                    eprintln!("{}: {e}", rel.display());
                    failed.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
    });
    eprintln!("{} drawings, {} failed", files.len(), failed.load(Ordering::Relaxed));
}
