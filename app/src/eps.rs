//! EPS (Encapsulated PostScript, Level 3) export: the same usvg tree the PDF export hands to
//! svg2pdf, written out as PostScript drawing operators. Shapes, strokes, clips and gradients stay
//! vector, text becomes outlines (usvg's flattened glyphs, so no fonts are needed), and pictures
//! are embedded as compressed images.
//!
//! PostScript has no transparency. The export draws every item at full strength (it ignores
//! item opacity), and within the artwork anything at most half opaque is left out and the rest
//! drawn solid: highlights and shadows drawn as see-through layers would otherwise cover what is
//! under them. A picture's pixels follow the same rule, through a 1-bit mask. The few things with
//! no PostScript form at all (SVG filters, masks and patterns) are drawn as pixels at
//! [`FALLBACK_DPI`].

use flate2::write::ZlibEncoder;
use resvg::tiny_skia;
use resvg::usvg::{self, tiny_skia_path::PathSegment, Node, Paint, Transform};
use std::fmt::Write as _;
use std::io::Write as _;

/// Resolution of the parts drawn as pixels because PostScript can't draw them.
pub const FALLBACK_DPI: f32 = 300.0;

/// Points per document pixel (the document is 96 px per inch, PostScript 72 points).
const PT: f32 = 72.0 / crate::doc::DPI;

/// Short names for the operators, and `BCimg`, which reads an image's data (ASCII85, then
/// Flate) from the file right after it and leaves the file just past the data's `~>`.
const PROLOG: &str = "/BCdict 32 dict def BCdict begin
/q /gsave load def /Q /grestore load def /cm /concat load def
/m /moveto load def /l /lineto load def /c /curveto load def /h /closepath load def
/n /newpath load def /f /fill load def /f* /eofill load def /S /stroke load def
/W /clip load def /W* /eoclip load def /rg /setrgbcolor load def
/w /setlinewidth load def /J /setlinecap load def /j /setlinejoin load def /M /setmiterlimit load def
/d /setdash load def /sh /shfill load def
/BCimg { % dict masked -> -
  /BCa currentfile /ASCII85Decode filter def
  /BCd BCa /FlateDecode filter def
  { dup /DataDict get } { dup } ifelse /DataSource BCd put
  /DeviceRGB setcolorspace image
  BCd flushfile BCa flushfile
} bind def
end
";

/// `tree` as an EPS file, the drawing its full size (document pixels to points). `credits` goes
/// into the header as comments.
pub fn from_tree(tree: &usvg::Tree, credits: &str) -> Vec<u8> {
    let size = tree.size();
    let (w, h) = (size.width() * PT, size.height() * PT);
    let mut s = String::new();
    let _ = writeln!(s, "%!PS-Adobe-3.0 EPSF-3.0");
    let _ = writeln!(s, "%%Creator: BioCartoon");
    let _ = writeln!(s, "%%CreationDate: {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    let _ = writeln!(s, "%%BoundingBox: 0 0 {} {}", w.ceil() as i64, h.ceil() as i64);
    let _ = writeln!(s, "%%HiResBoundingBox: 0 0 {} {}", num(w), num(h));
    let _ = writeln!(s, "%%LanguageLevel: 3");
    let _ = writeln!(s, "%%Pages: 1");
    let _ = writeln!(s, "%%EndComments");
    if !credits.is_empty() {
        let _ = writeln!(s, "% Image credits:");
        for line in credits.lines() {
            let line: String = line.chars().filter(|c| !c.is_control()).collect();
            let _ = writeln!(s, "% {line}");
        }
    }
    let _ = write!(s, "%%BeginProlog\n{PROLOG}%%EndProlog\n");
    let _ = writeln!(s, "%%Page: 1 1");
    // Document pixels, y down, from here on; `BCbase` keeps that for pictures drawn as pixels.
    let _ = writeln!(s, "save BCdict begin\n0 {} translate {} {} scale\n/BCbase matrix currentmatrix def", num(h), num(PT), num(-PT));
    let mut eps = Eps { s };
    eps.group(tree.root(), None, Transform::identity(), Transform::identity(), 1.0);
    eps.s.push_str("end restore\nshowpage\n%%Trailer\n%%EOF\n");
    eps.s.into_bytes()
}

struct Eps {
    s: String,
}

impl Eps {
    /// `g` and its children. `ctm` maps the coordinates `g` sits in (before its own transform)
    /// to the page's pixels, and `tree_ctm` the root of the tree `g` belongs to (an SVG picture
    /// inside the figure is a tree of its own). `node` is `g` as a node, to draw it as pixels.
    /// `alpha` is the opacity of the groups around it.
    fn group(&mut self, g: &usvg::Group, node: Option<&Node>, ctm: Transform, tree_ctm: Transform, alpha: f32) {
        let alpha = alpha * g.opacity().get();
        if !shown(alpha) {
            return;
        }
        if let Some(node) = node.filter(|_| !g.filters().is_empty() || g.mask().is_some()) {
            return self.pixels(node, ctm, tree_ctm);
        }
        let t = g.transform();
        let clip = g.clip_path();
        let wrap = !t.is_identity() || clip.is_some();
        if wrap {
            self.s.push_str("q\n");
            if !t.is_identity() {
                self.concat(t);
            }
            if let Some(cp) = clip {
                self.clip(cp);
            }
        }
        let ctm = ctm.pre_concat(t);
        for child in g.children() {
            self.node(child, ctm, tree_ctm, alpha);
        }
        if wrap {
            self.s.push_str("Q\n");
        }
    }

    fn node(&mut self, node: &Node, ctm: Transform, tree_ctm: Transform, alpha: f32) {
        match node {
            Node::Group(g) => self.group(g, Some(node), ctm, tree_ctm, alpha),
            Node::Text(t) => self.group(t.flattened(), None, ctm, tree_ctm, alpha),
            Node::Path(p) => self.path(p, node, ctm, tree_ctm, alpha),
            Node::Image(img) if img.is_visible() => match img.kind() {
                // Drawn at its own size where the image sits, as resvg does.
                usvg::ImageKind::SVG(tree) => self.group(tree.root(), None, ctm, ctm, alpha),
                usvg::ImageKind::JPEG(b) | usvg::ImageKind::PNG(b) | usvg::ImageKind::GIF(b) | usvg::ImageKind::WEBP(b) => match image::load_from_memory(b) {
                    Ok(pic) => {
                        let pic = pic.to_rgba8();
                        let (w, h) = (pic.width() as f32, pic.height() as f32);
                        self.image(pic.width(), pic.height(), pic.as_raw(), [0.0, 0.0, w, h]);
                    }
                    Err(e) => log::warn!("eps: image left out: {e}"),
                },
            },
            Node::Image(_) => {}
        }
    }

    fn path(&mut self, p: &usvg::Path, node: &Node, ctm: Transform, tree_ctm: Transform, alpha: f32) {
        if !p.is_visible() {
            return;
        }
        let fill = p.fill().filter(|f| shown(alpha * f.opacity().get() * paint_opacity(f.paint())));
        let stroke = p.stroke().filter(|s| shown(alpha * s.opacity().get() * paint_opacity(s.paint())));
        if fill.is_none() && stroke.is_none() {
            return;
        }
        let pattern = |paint: &Paint| matches!(paint, Paint::Pattern(_));
        if fill.is_some_and(|f| pattern(f.paint())) || stroke.is_some_and(|s| pattern(s.paint())) {
            return self.pixels(node, ctm, tree_ctm);
        }
        self.path_data(p.data(), Transform::identity());
        let even_odd = fill.is_some_and(|f| f.rule() == usvg::FillRule::EvenOdd);
        let do_fill = |e: &mut Self| {
            if let Some(f) = fill {
                e.s.push_str("q ");
                match f.paint() {
                    Paint::Color(c) => {
                        e.color(*c);
                        e.s.push_str(if even_odd { "f*" } else { "f" });
                    }
                    paint => {
                        e.s.push_str(if even_odd { "W* n " } else { "W n " });
                        e.gradient(paint);
                    }
                }
                e.s.push_str(" Q\n");
            }
        };
        let do_stroke = |e: &mut Self| {
            if let Some(st) = stroke {
                e.s.push_str("q ");
                let cap = match st.linecap() {
                    usvg::LineCap::Butt => 0,
                    usvg::LineCap::Round => 1,
                    usvg::LineCap::Square => 2,
                };
                let join = match st.linejoin() {
                    usvg::LineJoin::Miter | usvg::LineJoin::MiterClip => 0,
                    usvg::LineJoin::Round => 1,
                    usvg::LineJoin::Bevel => 2,
                };
                let _ = write!(e.s, "{} w {cap} J {join} j {} M ", num(st.width().get()), num(st.miterlimit().get().max(1.0)));
                if let Some(dash) = st.dasharray() {
                    let dash: Vec<String> = dash.iter().map(|&v| num(v)).collect();
                    let _ = write!(e.s, "[{}] {} d ", dash.join(" "), num(st.dashoffset()));
                }
                match st.paint() {
                    Paint::Color(c) => {
                        e.color(*c);
                        e.s.push('S');
                    }
                    paint => {
                        e.s.push_str("strokepath W n ");
                        e.gradient(paint);
                    }
                }
                e.s.push_str(" Q\n");
            }
        };
        match p.paint_order() {
            usvg::PaintOrder::FillAndStroke => {
                do_fill(self);
                do_stroke(self);
            }
            usvg::PaintOrder::StrokeAndFill => {
                do_stroke(self);
                do_fill(self);
            }
        }
        self.s.push_str("n\n");
    }

    /// Restrict drawing to clip path `cp`, given in the current coordinates.
    fn clip(&mut self, cp: &usvg::ClipPath) {
        if let Some(inner) = cp.clip_path() {
            self.clip(inner);
        }
        let mut even_odd = None;
        self.clip_shapes(cp.root(), cp.transform(), &mut even_odd);
        self.s.push_str(if even_odd == Some(true) { "W* n\n" } else { "W n\n" });
    }

    /// Every shape in clip path group `g`, as one path; `even_odd` takes the first one's rule.
    fn clip_shapes(&mut self, g: &usvg::Group, t: Transform, even_odd: &mut Option<bool>) {
        let t = t.pre_concat(g.transform());
        for child in g.children() {
            match child {
                Node::Group(g) => self.clip_shapes(g, t, even_odd),
                Node::Text(text) => self.clip_shapes(text.flattened(), t, even_odd),
                Node::Path(p) if p.is_visible() => {
                    even_odd.get_or_insert(p.fill().is_some_and(|f| f.rule() == usvg::FillRule::EvenOdd));
                    self.path_data(p.data(), t);
                }
                _ => {}
            }
        }
    }

    /// The path's outline (its points through `t`), as the current path.
    fn path_data(&mut self, data: &usvg::tiny_skia_path::Path, t: Transform) {
        let pt = |p: usvg::tiny_skia_path::Point| {
            let mut p = [p];
            t.map_points(&mut p);
            p[0]
        };
        let mut last = usvg::tiny_skia_path::Point::zero();
        for seg in data.segments() {
            match seg {
                PathSegment::MoveTo(p) => {
                    let q = pt(p);
                    let _ = writeln!(self.s, "{} {} m", num(q.x), num(q.y));
                    last = p;
                }
                PathSegment::LineTo(p) => {
                    let q = pt(p);
                    let _ = writeln!(self.s, "{} {} l", num(q.x), num(q.y));
                    last = p;
                }
                PathSegment::QuadTo(c, p) => {
                    // The same curve as a cubic one.
                    let toward = |a: usvg::tiny_skia_path::Point| usvg::tiny_skia_path::Point::from_xy(a.x + (c.x - a.x) * 2.0 / 3.0, a.y + (c.y - a.y) * 2.0 / 3.0);
                    let (c1, c2) = (toward(last), toward(p));
                    let (c1, c2, q) = (pt(c1), pt(c2), pt(p));
                    let _ = writeln!(self.s, "{} {} {} {} {} {} c", num(c1.x), num(c1.y), num(c2.x), num(c2.y), num(q.x), num(q.y));
                    last = p;
                }
                PathSegment::CubicTo(c1, c2, p) => {
                    let (c1, c2, q) = (pt(c1), pt(c2), pt(p));
                    let _ = writeln!(self.s, "{} {} {} {} {} {} c", num(c1.x), num(c1.y), num(c2.x), num(c2.y), num(q.x), num(q.y));
                    last = p;
                }
                PathSegment::Close => self.s.push_str("h\n"),
            }
        }
    }

    fn color(&mut self, c: usvg::Color) {
        let _ = write!(self.s, "{} {} {} rg ", unit(c.red), unit(c.green), unit(c.blue));
    }

    fn concat(&mut self, t: Transform) {
        let _ = writeln!(self.s, "[{}] cm", [t.sx, t.ky, t.kx, t.sy, t.tx, t.ty].map(num_fine).join(" "));
    }

    /// Paint the current clip with gradient `paint` (pad spreading; repeat and reflect are drawn
    /// as pad).
    fn gradient(&mut self, paint: &Paint) {
        let (base, kind, coords) = match paint {
            Paint::LinearGradient(g) => (&**g as &usvg::BaseGradient, 2, [g.x1(), g.y1(), g.x2(), g.y2()].map(num).join(" ")),
            Paint::RadialGradient(g) => (&**g as &usvg::BaseGradient, 3, [g.fx(), g.fy(), 0.0, g.cx(), g.cy(), g.r().get()].map(num).join(" ")),
            _ => return,
        };
        let mut stops: Vec<(f32, [String; 3])> = base.stops().iter().map(|s| (s.offset().get(), [s.color().red, s.color().green, s.color().blue].map(unit))).collect();
        let Some(first) = stops.first().cloned() else { return };
        let last = stops.last().cloned().unwrap_or(first.clone());
        if first.0 > 0.0 {
            stops.insert(0, (0.0, first.1.clone()));
        }
        if last.0 < 1.0 {
            stops.push((1.0, last.1.clone()));
        }
        if stops.len() < 2 {
            stops.push((1.0, first.1));
        }
        let pieces: Vec<String> = stops.windows(2).map(|w| format!("<< /FunctionType 2 /Domain [0 1] /C0 [{}] /C1 [{}] /N 1 >>", w[0].1.join(" "), w[1].1.join(" "))).collect();
        // Bounds must rise strictly inside 0..1.
        let mut bounds = Vec::new();
        let mut prev = 0.0f32;
        for s in &stops[1..stops.len() - 1] {
            let b = s.0.max(prev + 1e-4).min(1.0 - 1e-4);
            bounds.push(num_fine(b));
            prev = b;
        }
        let encode = vec!["0 1"; pieces.len()].join(" ");
        if !base.transform().is_identity() {
            self.concat(base.transform());
        }
        let _ = write!(
            self.s,
            "<< /ShadingType {kind} /ColorSpace /DeviceRGB /Coords [{coords}] /Extend [true true] /Function << /FunctionType 3 /Domain [0 1] /Functions [{}] /Bounds [{}] /Encode [{encode}] >> >> sh",
            pieces.join(" "),
            bounds.join(" ")
        );
    }

    /// Draw `node` as pixels at [`FALLBACK_DPI`], for what PostScript has no way to draw.
    fn pixels(&mut self, node: &Node, ctm: Transform, tree_ctm: Transform) {
        let Some(bbox) = node.abs_layer_bounding_box() else { return };
        let Some(page) = bbox.to_rect().transform(tree_ctm) else { return };
        let scale = FALLBACK_DPI / crate::doc::DPI;
        let (pw, ph) = ((page.width() * scale).ceil().max(1.0) as u32, (page.height() * scale).ceil().max(1.0) as u32);
        let Some(mut pixmap) = tiny_skia::Pixmap::new(pw, ph) else { return };
        // render_node draws the node shifted by its bounding box's corner: undo that, then place
        // it through `ctm` relative to its region of the page.
        let t = Transform::from_scale(scale, scale).pre_translate(-page.x(), -page.y()).pre_concat(ctm).pre_translate(bbox.x(), bbox.y());
        if resvg::render_node(node, t, &mut pixmap.as_mut()).is_none() {
            return;
        }
        let rgba: Vec<u8> = pixmap.pixels().iter().flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        }).collect();
        self.s.push_str("q BCbase setmatrix\n");
        self.image(pw, ph, &rgba, [page.x(), page.y(), page.width(), page.height()]);
        self.s.push_str("Q\n");
    }

    /// An RGBA picture of `w` × `h` pixels filling `rect` (x, y, width, height) of the current
    /// coordinates. Pixels under half opaque are masked out; opacity is otherwise ignored.
    fn image(&mut self, w: u32, h: u32, rgba: &[u8], rect: [f32; 4]) {
        if w == 0 || h == 0 || rgba.len() < (w * h * 4) as usize {
            return;
        }
        let masked = rgba.chunks_exact(4).any(|p| p[3] < 255);
        let mut data = Vec::with_capacity((w * h * 3 + if masked { h * w.div_ceil(8) } else { 0 }) as usize);
        for row in rgba.chunks_exact(w as usize * 4) {
            if masked {
                // A row of the 1-bit mask (1 = drawn), then the row's colours.
                let mut bits = vec![0u8; w.div_ceil(8) as usize];
                for (x, p) in row.chunks_exact(4).enumerate() {
                    if p[3] >= 128 {
                        bits[x / 8] |= 0x80 >> (x % 8);
                    }
                }
                data.extend_from_slice(&bits);
            }
            for p in row.chunks_exact(4) {
                data.extend_from_slice(&p[..3]);
            }
        }
        let mut z = ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        let _ = z.write_all(&data);
        let data = z.finish().unwrap_or_default();

        let [x, y, rw, rh] = rect;
        let _ = writeln!(self.s, "q {} {} translate {} {} scale", num(x), num(y), num_fine(rw), num_fine(rh));
        let image = format!("/ImageType 1 /Width {w} /Height {h} /ImageMatrix [{w} 0 0 {h} 0 0]");
        if masked {
            let _ = writeln!(
                self.s,
                "<< /ImageType 3 /InterleaveType 2 /DataDict << {image} /BitsPerComponent 8 /Decode [0 1 0 1 0 1] >> /MaskDict << {image} /BitsPerComponent 1 /Decode [1 0] >> >> true BCimg"
            );
        } else {
            let _ = writeln!(self.s, "<< {image} /BitsPerComponent 8 /Decode [0 1 0 1 0 1] >> false BCimg");
        }
        ascii85(&mut self.s, &data);
        self.s.push_str("Q\n");
    }
}

/// Whether something this opaque is drawn (solid) rather than left out.
fn shown(opacity: f32) -> bool {
    opacity > 0.5
}

/// A gradient's average stop opacity (1 for other paints).
fn paint_opacity(paint: &Paint) -> f32 {
    let stops = match paint {
        Paint::LinearGradient(g) => g.stops(),
        Paint::RadialGradient(g) => g.stops(),
        _ => return 1.0,
    };
    if stops.is_empty() {
        return 1.0;
    }
    stops.iter().map(|s| s.opacity().get()).sum::<f32>() / stops.len() as f32
}

/// `data` in ASCII85, in lines of 76 characters, ending in `~>`.
fn ascii85(out: &mut String, data: &[u8]) {
    let mut col = 0;
    let mut put = |out: &mut String, s: &str| {
        for ch in s.chars() {
            out.push(ch);
            col += 1;
            if col == 76 {
                out.push('\n');
                col = 0;
            }
        }
    };
    for chunk in data.chunks(4) {
        let mut b = [0u8; 4];
        b[..chunk.len()].copy_from_slice(chunk);
        let mut v = u32::from_be_bytes(b);
        if chunk.len() == 4 && v == 0 {
            put(out, "z");
            continue;
        }
        let mut digits = [0u8; 5];
        for d in digits.iter_mut().rev() {
            *d = (v % 85) as u8 + b'!';
            v /= 85;
        }
        put(out, std::str::from_utf8(&digits[..chunk.len() + 1]).unwrap_or(""));
    }
    out.push_str("~>\n");
}

/// A colour channel, 0 to 1.
fn unit(c: u8) -> String {
    num_fine(c as f32 / 255.0)
}

/// A coordinate, to 1/1000 of a pixel.
fn num(v: f32) -> String {
    trimmed(format!("{:.3}", if v.is_finite() { v } else { 0.0 }))
}

/// A scale factor or fraction, to five places.
fn num_fine(v: f32) -> String {
    trimmed(format!("{:.5}", if v.is_finite() { v } else { 0.0 }))
}

fn trimmed(s: String) -> String {
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
    if s == "-0" { "0".into() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eps(svg: &str) -> String {
        let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).unwrap();
        String::from_utf8(from_tree(&tree, "Icon by someone (CC BY 4.0)")).unwrap()
    }

    #[test]
    fn header_and_bounding_box_in_points() {
        let s = eps(r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100"><rect x="10" y="10" width="50" height="20" fill="#ff0000"/></svg>"##);
        assert!(s.starts_with("%!PS-Adobe-3.0 EPSF-3.0\n"));
        assert!(s.contains("%%BoundingBox: 0 0 150 75\n"), "{s}");
        assert!(s.contains("% Icon by someone (CC BY 4.0)\n"));
        assert!(s.contains("1 0 0 rg f"), "red fill: {s}");
        assert!(s.trim_end().ends_with("%%EOF"));
    }

    #[test]
    fn strokes_gradients_and_clips_stay_vector() {
        let s = eps(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
            <defs><linearGradient id="g"><stop offset="0" stop-color="#000"/><stop offset="0.5" stop-color="#f00"/><stop offset="1" stop-color="#fff"/></linearGradient>
            <clipPath id="c"><circle cx="50" cy="50" r="40"/></clipPath></defs>
            <g clip-path="url(#c)"><rect width="100" height="100" fill="url(#g)"/></g>
            <path d="M0 0 Q 50 100 100 0" fill="none" stroke="#00f" stroke-width="3" stroke-dasharray="4 2"/>
            </svg>"##,
        );
        assert!(s.contains("/ShadingType 2"), "{s}");
        assert!(s.contains("W n\n"), "clip: {s}");
        assert!(s.contains("[4 2] 0 d"), "dashes: {s}");
        assert!(s.contains(" S Q"), "stroke: {s}");
        assert!(!s.contains("BCimg\n"), "nothing as pixels: {s}");
    }

    #[test]
    fn transparent_pixels_are_masked_and_filters_become_pixels() {
        let mut png = Vec::new();
        let img = image::RgbaImage::from_fn(4, 4, |x, _| image::Rgba([255, 0, 0, if x < 2 { 255 } else { 0 }]));
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &png);
        let s = eps(&format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><defs><filter id="b"><feGaussianBlur stdDeviation="2"/></filter></defs>
            <image width="40" height="40" href="data:image/png;base64,{b64}"/>
            <rect x="50" y="50" width="20" height="20" fill="#0a0" filter="url(#b)"/></svg>"##
        ));
        assert!(s.contains("/ImageType 3"), "masked picture: {s}");
        assert!(s.contains("BCbase setmatrix"), "filter as pixels: {s}");
    }

    #[test]
    fn see_through_layers_are_left_out_or_drawn_solid() {
        let s = eps(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
            <rect width="10" height="10" fill="#ff0000" opacity="0.5"/>
            <rect width="10" height="10" fill="#00ff00" fill-opacity="0.8"/>
            <g opacity="0.7"><rect width="10" height="10" fill="#0000ff" fill-opacity="0.6"/></g></svg>"##,
        );
        assert!(!s.contains("1 0 0 rg"), "half-opaque red left out: {s}");
        assert!(s.contains("0 1 0 rg f"), "mostly opaque green drawn solid: {s}");
        assert!(!s.contains("0 0 1 rg"), "0.7 x 0.6 blue left out: {s}");
    }

    #[test]
    fn ascii85_matches_the_reference() {
        let mut s = String::new();
        ascii85(&mut s, b"Man is distinguished");
        assert_eq!(s, "9jqo^BlbD-BleB1DJ+*+F(f,q~>\n");
        let mut s = String::new();
        ascii85(&mut s, &[0, 0, 0, 0, 1]);
        assert_eq!(s, "z!<~>\n");
    }
}
