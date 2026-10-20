//! Rasterizes the app icon (assets/icon.svg) into:
//!   assets/icon.icns            16-1024 px, the macOS app icon (built with iconutil, so macOS only)
//!   assets/icon.png             1024 px, the desktop window icon and the Linux package icon
//!   assets/apple-touch-icon.png  180 px, the web build's bookmark / home-screen icon, on white since
//!                                Safari puts transparent touch icons on a gray backing (copy it to site/public/ too)
//!   assets/icon.ico              256 px, the Windows installer icon (see [package.metadata.packager])
//! The web build uses logo.svg itself as the tab favicon (see index.html).
//! Re-run with `cargo run --bin make_icon` after changing the icon.

use resvg::{tiny_skia, usvg};

fn rasterize(tree: &usvg::Tree, px: u32) -> tiny_skia::Pixmap {
    let mut pixmap = tiny_skia::Pixmap::new(px, px).unwrap();
    let size = tree.size();
    let scale = px as f32 / size.width().max(size.height());
    resvg::render(tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    pixmap
}

fn render(tree: &usvg::Tree, px: u32, out: &str) {
    rasterize(tree, px).save_png(out).unwrap();
    println!("wrote {out}");
}

fn render_on_white(tree: &usvg::Tree, px: u32, out: &str) {
    // Safari rounds the corners itself, so fill the whole square, and scale the art to its drawn bounds
    // (found from a transparent render) so it nearly touches the edges.
    let probe = rasterize(tree, 1024);
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for (i, p) in probe.pixels().iter().enumerate() {
        if p.alpha() > 8 {
            let (x, y) = (i as u32 % 1024, i as u32 / 1024);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
    }
    let size = tree.size();
    let unit = 1024.0 / size.width().max(size.height()); // probe pixels per SVG unit
    let (bw, bh) = ((x1 - x0 + 1) as f32 / unit, (y1 - y0 + 1) as f32 / unit);
    let scale = 0.94 * px as f32 / bw.max(bh);
    let (tx, ty) = ((px as f32 - scale * bw) / 2.0 - scale * x0 as f32 / unit, (px as f32 - scale * bh) / 2.0 - scale * y0 as f32 / unit);
    let mut pixmap = tiny_skia::Pixmap::new(px, px).unwrap();
    pixmap.fill(tiny_skia::Color::WHITE);
    resvg::render(tree, tiny_skia::Transform::from_scale(scale, scale).post_translate(tx, ty), &mut pixmap.as_mut());
    pixmap.save_png(out).unwrap();
    println!("wrote {out}");
}

fn render_ico(tree: &usvg::Tree, px: u32, out: &str) {
    // tiny-skia pixels are premultiplied; .ico wants straight alpha.
    let pixmap = rasterize(tree, px);
    let rgba: Vec<u8> = pixmap.pixels().iter().flat_map(|p| { let c = p.demultiply(); [c.red(), c.green(), c.blue(), c.alpha()] }).collect();
    let img = image::RgbaImage::from_raw(px, px, rgba).unwrap();
    img.save_with_format(out, image::ImageFormat::Ico).unwrap();
    println!("wrote {out}");
}

fn render_icns(tree: &usvg::Tree, out: &str) {
    let iconset = std::env::temp_dir().join("biocartoon-icon.iconset");
    let _ = std::fs::remove_dir_all(&iconset);
    std::fs::create_dir_all(&iconset).unwrap();
    for s in [16, 32, 128, 256, 512] {
        rasterize(tree, s).save_png(iconset.join(format!("icon_{s}x{s}.png"))).unwrap();
        rasterize(tree, s * 2).save_png(iconset.join(format!("icon_{s}x{s}@2x.png"))).unwrap();
    }
    match std::process::Command::new("iconutil").args(["-c", "icns"]).arg(&iconset).args(["-o", out]).status() {
        Ok(st) if st.success() => println!("wrote {out}"),
        _ => eprintln!("skipped {out}: iconutil (macOS) failed or is missing"),
    }
    let _ = std::fs::remove_dir_all(&iconset);
}

fn main() {
    let svg = std::fs::read("assets/icon.svg").expect("run from app/: assets/icon.svg");
    let tree = usvg::Tree::from_data(&svg, &usvg::Options::default()).expect("parse assets/icon.svg");
    render_icns(&tree, "assets/icon.icns");
    render(&tree, 1024, "assets/icon.png");
    render_on_white(&tree, 180, "assets/apple-touch-icon.png");
    render_ico(&tree, 256, "assets/icon.ico");
}
