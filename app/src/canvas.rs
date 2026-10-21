//! The editing canvas: draws the current slide, handles selection, move / resize / rotate with
//! grid snapping, marquee selection, pan / zoom, on-canvas cropping and the right-click menu.

use crate::app::{Action, AlignKind, ArrangeKind, BioCartoon};
use crate::assets::{ImageCache, ImgState, IMAGE_EXTS};
use crate::doc::{c32, CodeLang, CodeTheme, Guide, Item, ItemId, ItemKind, LineFamily, LineHead, LineKind, ShapeKind, Slide, TextAlign, TextVAlign, Unit, DPI};
use crate::theme;
use egui::emath::Rot2;
use std::path::Path;
use egui::{Color32, FontId, Mesh, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2};
use egui_phosphor::regular as ph;

pub struct View {
    pub zoom: f32,
    pub pan: Vec2,
    pub fit_requested: bool,
    pub page_screen_rect: Rect,
}

impl Default for View {
    fn default() -> Self {
        Self { zoom: 1.0, pan: Vec2::ZERO, fit_requested: true, page_screen_rect: Rect::NOTHING }
    }
}

#[derive(Clone, Copy)]
pub struct Xform {
    pub origin: Pos2,
    pub zoom: f32,
}

impl Xform {
    pub fn to_screen(&self, p: Pos2) -> Pos2 {
        self.origin + p.to_vec2() * self.zoom
    }
    pub fn to_doc(&self, p: Pos2) -> Pos2 {
        ((p - self.origin) / self.zoom).to_pos2()
    }
    pub fn rect(&self, r: Rect) -> Rect {
        Rect::from_min_max(self.to_screen(r.min), self.to_screen(r.max))
    }
}

pub enum Drag {
    Move { start_rects: Vec<(ItemId, Rect)>, start_ptr: Pos2, sel_rect: Rect },
    Resize { handle: usize, start_sel: Rect, start_rects: Vec<(ItemId, Rect)>, single: Option<(ItemId, f32, Pos2)>, lock_aspect: bool },
    Rotate { id: ItemId, center: Pos2, start_rot: f32, start_angle: f32 },
    /// `path`: a freehand line's points when the drag began (empty for other lines).
    LineEnd { id: ItemId, end: bool, path: Vec<Pos2> },
    Marquee { start: Pos2, current: Pos2, additive: bool },
    /// A crop handle (0..8, as the resize handles) or, with 8, the image slid under its frame.
    Crop { handle: usize, start: CropGeom, start_ptr: Pos2 },
    /// A guide (index into `doc.guides`).
    Guide(usize),
    Pan,
}

/// The image being cropped, in its own unrotated frame: `frame` is the item's rect (the window
/// the crop leaves), `full` where the whole image lies; `center` and `rot` are the item's.
#[derive(Clone, Copy)]
pub struct CropGeom {
    id: ItemId,
    frame: Rect,
    full: Rect,
    center: Pos2,
    rot: f32,
}

impl CropGeom {
    fn of(it: &Item) -> Option<Self> {
        let ItemKind::Image { crop, .. } = &it.kind else { return None };
        let [u0, v0, u1, v1] = display_crop(*crop, it.flip_h, it.flip_v);
        let frame = it.rect();
        let size = Vec2::new(frame.width() / (u1 - u0).max(1e-4), frame.height() / (v1 - v0).max(1e-4));
        let full = Rect::from_min_size(frame.min - Vec2::new(u0 * size.x, v0 * size.y), size);
        Some(Self { id: it.id, frame, full, center: frame.center(), rot: it.rotation })
    }

    /// A point in this frame's local (unrotated) space, from document space.
    fn local(&self, d: Pos2) -> Pos2 {
        rotate_about(d, self.center, -self.rot)
    }

    fn screen(&self, xf: &Xform, p: Pos2) -> Pos2 {
        xf.to_screen(rotate_about(p, self.center, self.rot))
    }

    /// `r`, a rect in local space, as a fraction of the whole image (unflipped display order).
    fn uv(&self, r: Rect) -> [f32; 4] {
        let f = self.full;
        [(r.min.x - f.min.x) / f.width(), (r.min.y - f.min.y) / f.height(), (r.max.x - f.min.x) / f.width(), (r.max.y - f.min.y) / f.height()]
    }

    /// The image cropped to `frame` (local space) with the whole image at `full`.
    fn apply(&self, it: &mut Item, frame: Rect, full: Rect) {
        let g = Self { full, ..*self };
        let (fh, fv) = (it.flip_h, it.flip_v);
        if let ItemKind::Image { crop, .. } = &mut it.kind {
            *crop = display_crop(g.uv(frame), fh, fv).map(|v| v.clamp(0.0, 1.0));
        }
        let c = rotate_about(frame.center(), self.center, self.rot);
        it.set_rect(Rect::from_center_size(c, frame.size()));
    }
}

/// A crop rect (u0, v0, u1, v1) as it shows once the item's flips apply; its own inverse.
fn display_crop(c: [f32; 4], flip_h: bool, flip_v: bool) -> [f32; 4] {
    let (u0, u1) = if flip_h { (1.0 - c[2], 1.0 - c[0]) } else { (c[0], c[2]) };
    let (v0, v1) = if flip_v { (1.0 - c[3], 1.0 - c[1]) } else { (c[1], c[3]) };
    [u0, v0, u1, v1]
}

/// The 8 edge / corner points of `r`, NW N NE E SE S SW W.
fn rect_handles(r: Rect) -> [Pos2; 8] {
    [
        r.left_top(),
        Pos2::new(r.center().x, r.min.y),
        r.right_top(),
        Pos2::new(r.max.x, r.center().y),
        r.right_bottom(),
        Pos2::new(r.center().x, r.max.y),
        r.left_bottom(),
        Pos2::new(r.min.x, r.center().y),
    ]
}

fn handle_cursor(h: usize) -> egui::CursorIcon {
    match h {
        8 => egui::CursorIcon::Grab,
        0 | 4 => egui::CursorIcon::ResizeNwSe,
        2 | 6 => egui::CursorIcon::ResizeNeSw,
        1 | 5 => egui::CursorIcon::ResizeVertical,
        _ => egui::CursorIcon::ResizeHorizontal,
    }
}

const HANDLE_R: f32 = 5.0;

fn rotate_about(p: Pos2, c: Pos2, a: f32) -> Pos2 {
    c + Rot2::from_angle(a) * (p - c)
}

fn snap(v: f32, g: f32) -> f32 {
    (v / g).round() * g
}

/// The smallest shift (within `tol`) that puts one of `vals` on a vertical (or horizontal) guide.
fn guide_shift(guides: &[Guide], vertical: bool, vals: &[f32], tol: f32) -> Option<f32> {
    guides
        .iter()
        .filter(|g| g.vertical == vertical)
        .flat_map(|g| vals.iter().map(move |v| g.pos - v))
        .filter(|d| d.abs() <= tol)
        .min_by(|a, b| a.abs().total_cmp(&b.abs()))
}

/// A point pulled onto nearby guides.
fn snap_point_to_guides(guides: &[Guide], p: Pos2, tol: f32) -> Pos2 {
    Pos2::new(p.x + guide_shift(guides, true, &[p.x], tol).unwrap_or(0.0), p.y + guide_shift(guides, false, &[p.y], tol).unwrap_or(0.0))
}

/// Corner points of an item's (possibly rotated) rect in document space, NW NE SE SW.
pub(crate) fn corners(it: &Item) -> [Pos2; 4] {
    let r = it.rect();
    let c = r.center();
    [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()].map(|p| rotate_about(p, c, it.rotation))
}

/// Handle positions for a selection: 8 edge/corner points (+ rotate handle when single).
fn handle_points(it: Option<&Item>, sel: Rect, xf: &Xform) -> Vec<Pos2> {
    let (r, c, a) = match it {
        Some(it) => (it.rect(), it.center(), it.rotation),
        None => (sel, sel.center(), 0.0),
    };
    let pts = rect_handles(r);
    let mut v: Vec<Pos2> = pts.iter().map(|p| xf.to_screen(rotate_about(*p, c, a))).collect();
    if it.is_some() {
        let top = xf.to_screen(rotate_about(Pos2::new(r.center().x, r.min.y), c, a));
        let center = xf.to_screen(c);
        let dir = (top - center).normalized();
        v.push(top + dir * 24.0);
    }
    v
}

fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = if ab.length_sq() < 1e-6 { 0.0 } else { ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0) };
    (a + ab * t - p).length()
}

/// The two corners a line item's box runs between (its `pos` and `pos + size`), in document space.
fn line_box(it: &Item) -> [Pos2; 2] {
    let a = it.pos();
    [a, a + it.size()]
}

/// The two end points of a line item, in document space. These carry the end handles; for
/// circular lines they span the arc's bounding box, for freehand ones they are where the stroke
/// began and ended.
pub(crate) fn line_ends(it: &Item) -> [Pos2; 2] {
    match &it.kind {
        ItemKind::Line { kind: LineKind::Freehand, points, .. } if points.len() >= 2 => {
            let pts = line_points(it);
            [pts[0], pts[pts.len() - 1]]
        }
        _ => line_box(it),
    }
}

/// Make a freehand line run through these document points: its box becomes their bounds.
pub(crate) fn set_freehand_path(it: &mut Item, pts: &[Pos2]) {
    let r = Rect::from_points(pts);
    let frac = |v: f32, lo: f32, len: f32| if len > 0.0 { (v - lo) / len } else { 0.0 };
    if let ItemKind::Line { points, .. } = &mut it.kind {
        *points = pts.iter().map(|p| [frac(p.x, r.min.x, r.width()), frac(p.y, r.min.y, r.height())]).collect();
    }
    it.pos = [r.min.x, r.min.y];
    it.size = [r.width(), r.height()];
}

/// `path` with one end moved to `to` and the other kept: rotated and scaled about the fixed end so
/// the drawing keeps its shape. A closed stroke (both ends together) is bent instead, each point
/// moving by its share of the way along the stroke.
pub(crate) fn move_path_end(path: &[Pos2], end: bool, to: Pos2) -> Vec<Pos2> {
    let (Some(&first), Some(&last)) = (path.first(), path.last()) else { return Vec::new() };
    let (fixed, moving) = if end { (first, last) } else { (last, first) };
    let (old, new) = (moving - fixed, to - fixed);
    if old.length() >= 1.0 {
        // Complex multiplication by new / old.
        let d = old.length_sq();
        let (c, s) = ((new.x * old.x + new.y * old.y) / d, (new.y * old.x - new.x * old.y) / d);
        return path.iter().map(|p| {
            let v = *p - fixed;
            fixed + Vec2::new(c * v.x - s * v.y, s * v.x + c * v.y)
        }).collect();
    }
    let delta = to - moving;
    let n = (path.len() - 1).max(1) as f32;
    path.iter().enumerate().map(|(i, p)| {
        let t = i as f32 / n;
        *p + delta * if end { t } else { 1.0 - t }
    }).collect()
}

/// The main path of a line item from start to end, in document space: two points for a
/// straight line, the elbow corners of a bent one, or the samples of a spline / arc.
pub(crate) fn line_points(it: &Item) -> Vec<Pos2> {
    let [a, b] = line_box(it);
    let (family, freehand) = match &it.kind {
        ItemKind::Line { kind, points, .. } => (kind.family(), &points[..]),
        _ => (LineFamily::Straight, &[][..]),
    };
    match family {
        LineFamily::Straight => vec![a, b],
        LineFamily::Freehand if freehand.len() >= 2 => {
            let s = b - a;
            freehand.iter().map(|[u, v]| Pos2::new(a.x + u * s.x, a.y + v * s.y)).collect()
        }
        LineFamily::Freehand => vec![a, b],
        LineFamily::Bent => {
            let mx = (a.x + b.x) / 2.0;
            vec![a, Pos2::new(mx, a.y), Pos2::new(mx, b.y), b]
        }
        LineFamily::Spline => {
            // Cubic Bézier with horizontal tangents at both ends (an S-curve).
            let mx = (a.x + b.x) / 2.0;
            let (c1, c2) = (Pos2::new(mx, a.y), Pos2::new(mx, b.y));
            const N: usize = 32;
            (0..=N)
                .map(|i| {
                    let t = i as f32 / N as f32;
                    let u = 1.0 - t;
                    let p = a.to_vec2() * (u * u * u) + c1.to_vec2() * (3.0 * u * u * t) + c2.to_vec2() * (3.0 * u * t * t) + b.to_vec2() * (t * t * t);
                    p.to_pos2()
                })
                .collect()
        }
        LineFamily::Circular => {
            // A 300° arc of the ellipse the two ends span, open at the top, clockwise.
            let r = Rect::from_two_pos(a, b);
            let c = r.center();
            let (rx, ry) = (r.width() / 2.0, r.height() / 2.0);
            let start = -std::f32::consts::FRAC_PI_3;
            let sweep = 5.0 * std::f32::consts::FRAC_PI_3;
            const N: usize = 48;
            (0..=N)
                .map(|i| {
                    let t = start + sweep * i as f32 / N as f32;
                    Pos2::new(c.x + rx * t.cos(), c.y + ry * t.sin())
                })
                .collect()
        }
    }
}

/// One stroke of a line item: its points in document space and the head at each end.
pub(crate) struct LinePath {
    pub pts: Vec<Pos2>,
    pub start: LineHead,
    pub end: LineHead,
}

/// Every stroke of a line item: one path, or the two half-arrows of an equilibrium arrow (the
/// favoured direction drawn full length, the other shortened).
pub(crate) fn line_paths(it: &Item) -> Vec<LinePath> {
    let ItemKind::Line { kind, width, .. } = &it.kind else { return Vec::new() };
    let (start, end) = kind.heads();
    if !kind.is_equilibrium() {
        return vec![LinePath { pts: line_points(it), start, end }];
    }
    let [a, b] = line_ends(it);
    let len = (b - a).length();
    if len < 1e-3 {
        return Vec::new();
    }
    let d = (b - a) / len;
    let n = Vec2::new(-d.y, d.x) * (width * 1.5 + 2.0);
    let inset = d * len * 0.2;
    let (top, bottom) = if *kind == LineKind::Equilibrium {
        ([a - n, b - n], [b + n - inset, a + n + inset])
    } else {
        ([a - n + inset, b - n - inset], [b + n, a + n])
    };
    vec![
        LinePath { pts: top.to_vec(), start: LineHead::None, end: LineHead::Harpoon },
        LinePath { pts: bottom.to_vec(), start: LineHead::None, end: LineHead::Harpoon },
    ]
}

/// A head's geometry, in the same space as the points it was made from.
pub(crate) enum HeadGeom {
    Polygon(Vec<Pos2>),
    Segment(Pos2, Pos2),
}

/// The head at one end of `pts` (`at_end`: the last point, else the first), `size` long.
pub(crate) fn head_geom(pts: &[Pos2], at_end: bool, head: LineHead, size: f32) -> Option<HeadGeom> {
    if pts.len() < 2 || head == LineHead::None {
        return None;
    }
    let (tip, from) = if at_end { (pts[pts.len() - 1], pts[pts.len() - 2]) } else { (pts[0], pts[1]) };
    let d = (tip - from).normalized();
    if !d.is_finite() || d == Vec2::ZERO {
        return None;
    }
    let n = Vec2::new(-d.y, d.x);
    let base = tip - d * size;
    Some(match head {
        LineHead::None => return None,
        LineHead::Arrow => HeadGeom::Polygon(vec![tip, base + n * size * 0.5, base - n * size * 0.5]),
        LineHead::Harpoon => HeadGeom::Polygon(vec![tip, base - n * size * 0.6, base]),
        LineHead::Bar => HeadGeom::Segment(tip + n * size * 0.8, tip - n * size * 0.8),
    })
}

pub fn hit_test(it: &Item, p: Pos2, tol: f32) -> bool {
    match &it.kind {
        ItemKind::Line { width, .. } => {
            line_paths(it).iter().any(|path| path.pts.windows(2).any(|w| dist_to_segment(p, w[0], w[1]) <= tol.max(*width)))
        }
        _ => {
            let r = it.rect();
            let lp = rotate_about(p, r.center(), -it.rotation);
            r.expand(tol * 0.5).contains(lp)
        }
    }
}

pub(crate) fn shape_polygons(kind: ShapeKind, r: Rect) -> (Vec<Vec<Pos2>>, Vec<Pos2>) {
    let c = r.center();
    let (w, h) = (r.width(), r.height());
    let outline: Vec<Pos2>;
    let mut fills: Vec<Vec<Pos2>> = Vec::new();
    match kind {
        ShapeKind::Rectangle => {
            outline = vec![r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()];
            fills.push(outline.clone());
        }
        ShapeKind::RoundedRectangle => {
            let rad = (w.min(h) * 0.18).max(1.0);
            let mut pts = Vec::new();
            let corners = [
                (Pos2::new(r.max.x - rad, r.min.y + rad), -std::f32::consts::FRAC_PI_2),
                (Pos2::new(r.max.x - rad, r.max.y - rad), 0.0),
                (Pos2::new(r.min.x + rad, r.max.y - rad), std::f32::consts::FRAC_PI_2),
                (Pos2::new(r.min.x + rad, r.min.y + rad), std::f32::consts::PI),
            ];
            for (cc, a0) in corners {
                for i in 0..=6 {
                    let a = a0 + i as f32 / 6.0 * std::f32::consts::FRAC_PI_2;
                    pts.push(cc + Vec2::new(a.cos(), a.sin()) * rad);
                }
            }
            outline = pts;
            fills.push(outline.clone());
        }
        ShapeKind::Ellipse => {
            let n = 64;
            outline = (0..n)
                .map(|i| {
                    let a = i as f32 / n as f32 * std::f32::consts::TAU;
                    c + Vec2::new(a.cos() * w / 2.0, a.sin() * h / 2.0)
                })
                .collect();
            fills.push(outline.clone());
        }
        ShapeKind::Triangle => {
            outline = vec![Pos2::new(c.x, r.min.y), r.right_bottom(), r.left_bottom()];
            fills.push(outline.clone());
        }
        ShapeKind::Diamond => {
            outline = vec![Pos2::new(c.x, r.min.y), Pos2::new(r.max.x, c.y), Pos2::new(c.x, r.max.y), Pos2::new(r.min.x, c.y)];
            fills.push(outline.clone());
        }
        ShapeKind::Hexagon => {
            outline = (0..6)
                .map(|i| {
                    let a = i as f32 / 6.0 * std::f32::consts::TAU;
                    c + Vec2::new(a.cos() * w / 2.0, a.sin() * h / 2.0)
                })
                .collect();
            fills.push(outline.clone());
        }
        ShapeKind::Star => {
            let mut pts = Vec::new();
            let mut inner = Vec::new();
            for i in 0..5 {
                let a = -std::f32::consts::FRAC_PI_2 + i as f32 / 5.0 * std::f32::consts::TAU;
                let b = a + std::f32::consts::PI / 5.0;
                let o = c + Vec2::new(a.cos() * w / 2.0, a.sin() * h / 2.0);
                let inn = c + Vec2::new(b.cos() * w * 0.19, b.sin() * h * 0.19);
                pts.push(o);
                pts.push(inn);
                inner.push(inn);
            }
            fills.push(inner.clone());
            for i in 0..5 {
                fills.push(vec![inner[(i + 4) % 5], pts[i * 2], inner[i]]);
            }
            outline = pts;
        }
        ShapeKind::BlockArrow => {
            let head = w * 0.35;
            let bh = h * 0.5;
            let body = Rect::from_min_max(Pos2::new(r.min.x, c.y - bh / 2.0), Pos2::new(r.max.x - head, c.y + bh / 2.0));
            fills.push(vec![body.left_top(), body.right_top(), body.right_bottom(), body.left_bottom()]);
            fills.push(vec![Pos2::new(r.max.x - head, r.min.y), Pos2::new(r.max.x, c.y), Pos2::new(r.max.x - head, r.max.y)]);
            outline = vec![
                body.left_top(),
                body.right_top(),
                Pos2::new(r.max.x - head, r.min.y),
                Pos2::new(r.max.x, c.y),
                Pos2::new(r.max.x - head, r.max.y),
                body.right_bottom(),
                body.left_bottom(),
            ];
        }
    }
    (fills, outline)
}

/// Draw one item. `images` is needed for textures; `faded` dims the item (used for drag previews).
pub fn draw_item(painter: &egui::Painter, xf: &Xform, it: &Item, images: &mut ImageCache, local_dir_hint: Option<&str>) {
    let z = xf.zoom;
    let op = it.opacity.clamp(0.0, 1.0);
    match &it.kind {
        ItemKind::Image { source, crop, overlay } => {
            let rs = xf.rect(it.rect());
            let key = source.cache_key();
            let state = match source {
                crate::doc::ImageSource::Asset { key, svg, .. } => images.asset_image(key, svg.as_deref(), local_dir_hint),
                crate::doc::ImageSource::File { path, .. } => images.file(path),
            };
            match state {
                ImgState::Ready { tex, .. } => {
                    let (mut u0, mut u1) = (crop[0], crop[2]);
                    let (mut v0, mut v1) = (crop[1], crop[3]);
                    if it.flip_h {
                        std::mem::swap(&mut u0, &mut u1);
                    }
                    if it.flip_v {
                        std::mem::swap(&mut v0, &mut v1);
                    }
                    let uv = Rect::from_min_max(Pos2::new(u0, v0), Pos2::new(u1, v1));
                    let strength = overlay[3] as f32 / 255.0;
                    let tint = Color32::from_rgb(
                        (255.0 * (1.0 - strength) + overlay[0] as f32 * strength) as u8,
                        (255.0 * (1.0 - strength) + overlay[1] as f32 * strength) as u8,
                        (255.0 * (1.0 - strength) + overlay[2] as f32 * strength) as u8,
                    )
                    .gamma_multiply(op);
                    let mut mesh = Mesh::with_texture(tex.id());
                    mesh.add_rect_with_uv(rs, uv, tint);
                    if it.rotation != 0.0 {
                        mesh.rotate(Rot2::from_angle(it.rotation), rs.center());
                    }
                    painter.add(Shape::mesh(mesh));
                }
                ImgState::Loading => {
                    painter.rect(rs, 4.0 * z, Color32::from_rgba_unmultiplied(200, 200, 210, 60), Stroke::new(1.0, Color32::from_gray(170)), StrokeKind::Inside);
                    painter.text(rs.center(), egui::Align2::CENTER_CENTER, "Loading...", FontId::proportional((12.0 * z).clamp(8.0, 20.0)), Color32::from_gray(120));
                }
                ImgState::Failed(e) => {
                    painter.rect(rs, 4.0 * z, Color32::from_rgba_unmultiplied(230, 200, 200, 90), Stroke::new(1.0, Color32::from_rgb(200, 120, 120)), StrokeKind::Inside);
                    let msg = if rs.width() > 80.0 { format!("{}\n(not loaded)", it.name) } else { "!".into() };
                    painter.text(rs.center(), egui::Align2::CENTER_CENTER, msg, FontId::proportional((11.0 * z).clamp(8.0, 16.0)), Color32::from_rgb(150, 60, 60));
                    let _ = (key, e);
                }
            }
        }
        ItemKind::Text { color, bold, .. } => {
            let col = c32(*color).gamma_multiply(op);
            let Some((galley, pos, c)) = text_layout(painter, xf, it, col) else { return };
            let p = rotate_about(pos, c, it.rotation);
            let mut ts = egui::epaint::TextShape::new(p, galley.clone(), col).with_angle(it.rotation);
            if *bold {
                let p2 = rotate_about(pos + Vec2::new(0.6 * z.max(1.0), 0.0), c, it.rotation);
                painter.add(Shape::Text(egui::epaint::TextShape::new(p2, galley, col).with_angle(it.rotation)));
            }
            ts.override_text_color = Some(col);
            painter.add(Shape::Text(ts));
        }
        ItemKind::Shape { shape, fill, stroke, stroke_width } => {
            let r = it.rect();
            let (fills, outline) = shape_polygons(*shape, r);
            let c = r.center();
            let map = |p: &Pos2| {
                let mut q = *p;
                if it.flip_h {
                    q.x = 2.0 * c.x - q.x;
                }
                if it.flip_v {
                    q.y = 2.0 * c.y - q.y;
                }
                xf.to_screen(rotate_about(q, c, it.rotation))
            };
            let fc = c32(*fill).gamma_multiply(op);
            for poly in &fills {
                painter.add(Shape::convex_polygon(poly.iter().map(map).collect(), fc, Stroke::NONE));
            }
            if *stroke_width > 0.0 && stroke[3] > 0 {
                painter.add(Shape::closed_line(outline.iter().map(map).collect(), Stroke::new(stroke_width * z, c32(*stroke).gamma_multiply(op))));
            }
        }
        ItemKind::Line { color, width, style, .. } => {
            let col = c32(*color).gamma_multiply(op);
            let w = (width * z).max(0.5);
            let stroke = Stroke::new(w, col);
            let head = (w * 4.0).max(6.0);
            let dz = z.max(0.5);
            for path in line_paths(it) {
                let pts: Vec<Pos2> = path.pts.iter().map(|p| xf.to_screen(*p)).collect();
                if pts.len() < 2 {
                    continue;
                }
                match style.dash(*width) {
                    Some((dash, gap)) => {
                        for s in Shape::dashed_line(&pts, stroke, dash * dz, gap * dz) {
                            painter.add(s);
                        }
                    }
                    None => {
                        painter.add(Shape::line(pts.clone(), stroke));
                    }
                }
                for (at_end, h) in [(false, path.start), (true, path.end)] {
                    match head_geom(&pts, at_end, h, head) {
                        Some(HeadGeom::Polygon(poly)) => {
                            painter.add(Shape::convex_polygon(poly, col, Stroke::NONE));
                        }
                        Some(HeadGeom::Segment(p, q)) => {
                            painter.line_segment([p, q], Stroke::new(w * 1.2, col));
                        }
                        None => {}
                    }
                }
            }
        }
        ItemKind::Table { rows, cols, cells, header, border, fill, header_fill, text_color, font_size } => {
            let rs = xf.rect(it.rect());
            let (rows, cols) = ((*rows).max(1), (*cols).max(1));
            let cw = rs.width() / cols as f32;
            let ch = rs.height() / rows as f32;
            painter.rect_filled(rs, 0.0, c32(*fill).gamma_multiply(op));
            if *header {
                painter.rect_filled(Rect::from_min_size(rs.min, Vec2::new(rs.width(), ch)), 0.0, c32(*header_fill).gamma_multiply(op));
            }
            let bs = Stroke::new((1.0 * z).max(0.5), c32(*border).gamma_multiply(op));
            for r in 0..=rows {
                let y = rs.min.y + r as f32 * ch;
                painter.line_segment([Pos2::new(rs.min.x, y), Pos2::new(rs.max.x, y)], bs);
            }
            for c in 0..=cols {
                let x = rs.min.x + c as f32 * cw;
                painter.line_segment([Pos2::new(x, rs.min.y), Pos2::new(x, rs.max.y)], bs);
            }
            let font = FontId::proportional((font_size * z).max(1.0));
            let tc = c32(*text_color).gamma_multiply(op);
            for r in 0..rows {
                for c in 0..cols {
                    let Some(t) = cells.get(r * cols + c) else { continue };
                    if t.is_empty() {
                        continue;
                    }
                    let cell = Rect::from_min_size(Pos2::new(rs.min.x + c as f32 * cw, rs.min.y + r as f32 * ch), Vec2::new(cw, ch));
                    let p = painter.with_clip_rect(cell.intersect(painter.clip_rect()));
                    let (galley, pos) = cell_layout(&p, cell, t, font.clone(), tc, z);
                    p.galley(pos, galley, tc);
                }
            }
        }
        ItemKind::Code { code, language, theme, font_size, line_numbers, window, rounded } => {
            draw_code(painter, xf.rect(it.rect()), z, op, code, *language, *theme, *font_size, *line_numbers, *window, *rounded);
        }
    }
}

/// A code block: themed window, optional title-bar dots and line numbers, highlighted code
/// clipped to the item's rect.
fn draw_code(painter: &egui::Painter, rs: Rect, z: f32, op: f32, code: &str, language: CodeLang, theme: CodeTheme, font_size: f32, line_numbers: bool, window: bool, rounded: bool) {
    let pal = theme.palette();
    let col = |c: [u8; 3]| Color32::from_rgb(c[0], c[1], c[2]).gamma_multiply(op);
    let fs = (font_size * z).max(1.0);
    let font = FontId::monospace(fs);
    let (char_w, line_h) = painter.fonts_mut(|f| (f.glyph_width(&font, '0'), f.row_height(&font)));
    let lines = crate::code::highlight(code, language.resolve(code));
    let lay = crate::code::BlockLayout::new(fs, lines.len(), line_numbers, window, char_w, line_h);
    painter.rect_filled(rs, if rounded { lay.radius } else { 0.0 }, col(pal.bg));
    if window {
        for (c, r, rgb) in lay.dots(fs) {
            painter.circle_filled(rs.min + Vec2::new(c[0], c[1]), r, col(rgb));
        }
    }
    let p = painter.with_clip_rect(rs.shrink(1.0).intersect(painter.clip_rect()));
    let o = lay.text_origin();
    for (i, spans) in lines.iter().enumerate() {
        let y = rs.min.y + o[1] + i as f32 * line_h;
        if y > p.clip_rect().max.y {
            break;
        }
        if y + line_h < p.clip_rect().min.y {
            continue;
        }
        if line_numbers {
            let g = p.layout_no_wrap((i + 1).to_string(), font.clone(), col(pal.line_number));
            p.galley(Pos2::new(rs.min.x + o[0] - fs * 1.4 - g.size().x, y), g, Color32::PLACEHOLDER);
        }
        let mut job = egui::text::LayoutJob::default();
        for (t, k) in spans {
            job.append(t, 0.0, egui::TextFormat::simple(font.clone(), col(pal.color(*k))));
        }
        p.galley(Pos2::new(rs.min.x + o[0], y), p.layout_job(job), Color32::PLACEHOLDER);
    }
}

/// The layout job for a text item's text at `zoom`, wrapped to `width` screen px. The canvas, the
/// on-canvas editor and the exports all lay text out with it, so they break lines the same way.
pub(crate) fn text_job(ctx: &egui::Context, text: &str, kind: &ItemKind, zoom: f32, width: f32, col: Color32) -> egui::text::LayoutJob {
    let ItemKind::Text { size, align, italic, underline, font, line_spacing, .. } = kind else { return Default::default() };
    let px = (size * zoom).max(1.0);
    let font_id = font.font_id(px);
    let line_height = (*line_spacing != 1.0).then(|| ctx.fonts_mut(|f| f.row_height(&font_id)) * line_spacing.max(0.1));
    let format = egui::TextFormat {
        font_id,
        color: col,
        italics: *italic,
        underline: if *underline { Stroke::new((px / 14.0).max(1.0), col) } else { Stroke::NONE },
        line_height,
        ..Default::default()
    };
    let mut job = egui::text::LayoutJob::single_section(text.to_string(), format);
    job.wrap.max_width = width.max(4.0);
    job.halign = text_halign(*align);
    job.justify = *align == TextAlign::Justify;
    job
}

pub(crate) fn text_halign(align: TextAlign) -> egui::Align {
    match align {
        TextAlign::Left | TextAlign::Justify => egui::Align::LEFT,
        TextAlign::Center => egui::Align::Center,
        TextAlign::Right => egui::Align::RIGHT,
    }
}

pub(crate) fn text_valign(valign: TextVAlign) -> egui::Align {
    match valign {
        TextVAlign::Top => egui::Align::TOP,
        TextVAlign::Middle => egui::Align::Center,
        TextVAlign::Bottom => egui::Align::BOTTOM,
    }
}

/// Where to paint `galley` so it sits in `r` the way a text item's alignment asks (the same spot
/// the on-canvas `TextEdit` puts it).
pub(crate) fn text_origin(galley: &egui::Galley, r: Rect, kind: &ItemKind) -> Pos2 {
    let ItemKind::Text { align, valign, .. } = kind else { return r.min };
    let a = egui::Align2([text_halign(*align), text_valign(*valign)]);
    a.align_size_within_rect(galley.size(), r).min - Vec2::new(galley.rect.left(), 0.0)
}

/// A text item laid out on screen: the galley, its top-left before rotation, and the centre it
/// rotates about.
fn text_layout(painter: &egui::Painter, xf: &Xform, it: &Item, col: Color32) -> Option<(std::sync::Arc<egui::Galley>, Pos2, Pos2)> {
    let ItemKind::Text { text, .. } = &it.kind else { return None };
    let rs = xf.rect(it.rect());
    let galley = painter.layout_job(text_job(painter.ctx(), text, &it.kind, xf.zoom, rs.width(), col));
    Some((galley.clone(), text_origin(&galley, rs, &it.kind), rs.center()))
}

/// The (row-major) cell of table `it` under page point `d`.
pub(crate) fn table_cell_at(it: &Item, d: Pos2) -> Option<usize> {
    let ItemKind::Table { rows, cols, .. } = &it.kind else { return None };
    let r = it.rect();
    if !r.contains(d) || r.width() <= 0.0 || r.height() <= 0.0 {
        return None;
    }
    let (rows, cols) = ((*rows).max(1), (*cols).max(1));
    let c = (((d.x - r.min.x) / r.width() * cols as f32) as usize).min(cols - 1);
    let row = (((d.y - r.min.y) / r.height() * rows as f32) as usize).min(rows - 1);
    Some(row * cols + c)
}

/// Where cell `i` of a `rows` x `cols` table drawn in `rs` sits.
fn cell_rect(rs: Rect, rows: usize, cols: usize, i: usize) -> Rect {
    let (rows, cols) = (rows.max(1), cols.max(1));
    let (cw, ch) = (rs.width() / cols as f32, rs.height() / rows as f32);
    Rect::from_min_size(Pos2::new(rs.min.x + (i % cols) as f32 * cw, rs.min.y + (i / cols) as f32 * ch), Vec2::new(cw, ch))
}

/// A table cell's text laid out in `cell` (screen space), and where it is drawn.
fn cell_layout(painter: &egui::Painter, cell: Rect, text: &str, font: FontId, col: Color32, z: f32) -> (std::sync::Arc<egui::Galley>, Pos2) {
    let galley = painter.layout(text.to_string(), font, col, (cell.width() - 8.0 * z).max(4.0));
    let pos = Pos2::new(cell.min.x + 4.0 * z, cell.center().y - galley.size().y / 2.0);
    (galley, pos)
}

/// Spelling and grammar squiggles under the text of `slide`'s text boxes and tables, but not under
/// `skip` (as for [`draw_slide_except`]).
fn draw_squiggles(painter: &egui::Painter, xf: &Xform, slide: &Slide, spell: &crate::spell::SpellState, skip: Option<(ItemId, usize)>) {
    let z = xf.zoom;
    for it in &slide.items {
        let skipped = skip.filter(|(id, _)| *id == it.id).map(|(_, c)| c);
        match &it.kind {
            ItemKind::Text { text, .. } => {
                if skipped.is_some() || spell.at(it.id, None).next().is_none() {
                    continue;
                }
                let Some((galley, pos, c)) = text_layout(painter, xf, it, Color32::WHITE) else { continue };
                crate::spell::paint_squiggles(painter, &galley, text, spell.at(it.id, None), z, |p| rotate_about(pos + p.to_vec2(), c, it.rotation));
            }
            ItemKind::Table { rows, cols, cells, font_size, .. } => {
                let rs = xf.rect(it.rect());
                for (i, t) in cells.iter().enumerate().take((*rows).max(1) * (*cols).max(1)) {
                    if skipped == Some(i) || spell.at(it.id, Some(i)).next().is_none() {
                        continue;
                    }
                    let cell = cell_rect(rs, *rows, *cols, i);
                    let p = painter.with_clip_rect(cell.intersect(painter.clip_rect()));
                    let (galley, pos) = cell_layout(&p, cell, t, FontId::proportional((font_size * z).max(1.0)), Color32::WHITE, z);
                    crate::spell::paint_squiggles(&p, &galley, t, spell.at(it.id, Some(i)), z, |q| pos + q.to_vec2());
                }
            }
            _ => {}
        }
    }
}

pub fn draw_slide(painter: &egui::Painter, xf: &Xform, slide: &Slide, page: Rect, bg: Color32, images: &mut ImageCache) {
    draw_slide_except(painter, xf, slide, page, bg, images, None, true);
}

/// [`draw_slide`] leaving out `skip`: the text box being edited, or for a table the text of the
/// cell being edited, which its editor draws.
/// Unless `clip_to_page`, items hanging off the page stay visible (they just don't export).
fn draw_slide_except(painter: &egui::Painter, xf: &Xform, slide: &Slide, page: Rect, bg: Color32, images: &mut ImageCache, skip: Option<(ItemId, usize)>, clip_to_page: bool) {
    let rs = xf.rect(page);
    painter.rect_filled(rs, 0.0, bg);
    let p = if clip_to_page { painter.with_clip_rect(rs.intersect(painter.clip_rect())) } else { painter.clone() };
    for it in &slide.items {
        match skip {
            Some((id, cell)) if id == it.id => {
                if let ItemKind::Table { .. } = it.kind {
                    let mut t = it.clone();
                    if let ItemKind::Table { cells, .. } = &mut t.kind {
                        if let Some(c) = cells.get_mut(cell) {
                            c.clear();
                        }
                    }
                    draw_item(&p, xf, &t, images, None);
                }
            }
            _ => draw_item(&p, xf, it, images, None),
        }
    }
}

impl BioCartoon {
    /// Images dragged in from the OS: outline the page while they hover, and on drop add them to
    /// Uploads and the page. The desktop gets file paths; in the browser the bytes are read and saved
    /// to app storage like a picked upload.
    fn image_drop(&mut self, ui: &egui::Ui, rect: Rect, page_s: Rect, xf: &Xform, painter: &egui::Painter) {
        let (hovering, dropped, ptr) = ui.input(|i| (!i.raw.hovered_files.is_empty(), i.raw.dropped_files.clone(), i.pointer.hover_pos()));
        if hovering {
            painter.rect_stroke(page_s.expand(4.0), 4.0, Stroke::new(3.0, theme::accent()), StrokeKind::Outside);
            painter.text(page_s.center_top() + Vec2::new(0.0, -8.0), egui::Align2::CENTER_BOTTOM, "Drop images to add them", FontId::proportional(13.0), theme::accent());
        }
        if dropped.is_empty() {
            return;
        }
        let is_image = |p: &Path| p.extension().and_then(|e| e.to_str()).is_some_and(|e| IMAGE_EXTS.contains(&e.to_ascii_lowercase().as_str()));
        let (images, other): (Vec<_>, Vec<_>) = dropped.into_iter().partition(|f| is_image(f.path()));
        if !other.is_empty() {
            self.toast(format!("Only images can be dropped ({})", IMAGE_EXTS.join(", ")));
        }
        if images.is_empty() {
            return;
        }
        let at = ptr.filter(|p| rect.contains(*p)).map(|p| xf.to_doc(p));
        #[cfg(not(target_arch = "wasm32"))]
        self.actions.push(Action::DropImages(images.iter().map(|f| f.path().to_path_buf()).collect(), at));
        #[cfg(target_arch = "wasm32")]
        {
            let (tx, ctx) = (self.action_tx.clone(), ui.ctx().clone());
            wasm_bindgen_futures::spawn_local(async move {
                let stamp = crate::projects::now_ms();
                let mut paths = Vec::new();
                for (i, f) in images.iter().enumerate() {
                    let name = f.path().file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "image.png".into());
                    let path = std::path::PathBuf::from("uploads").join(format!("{stamp}-{i}")).join(&name);
                    match f.bytes_async().await.and_then(|b| crate::storage::write(&path, &b).map_err(|e| e.to_string())) {
                        Ok(()) => paths.push(path),
                        Err(e) => log::error!("dropped {name}: {e}"),
                    }
                }
                if !paths.is_empty() {
                    let _ = tx.send(Action::DropImages(paths, at));
                    ctx.request_repaint();
                }
            });
        }
    }

    pub fn canvas_ui(&mut self, ui: &mut egui::Ui) {
        let frame = egui::Frame::new().fill(theme::canvas_bg());
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            let rect = ui.max_rect();
            let response = ui.allocate_rect(rect, Sense::click_and_drag());
            let page = self.doc.page_rect();

            if self.view.fit_requested {
                self.view.fit_requested = false;
                let margin = 80.0;
                self.view.zoom = ((rect.width() - margin) / page.width()).min((rect.height() - margin) / page.height()).clamp(0.05, 8.0);
                self.view.pan = Vec2::ZERO;
            }

            // Pan / zoom with scroll and pinch.
            if response.hovered() {
                let (scroll, zoom_delta, ptr) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.pointer.hover_pos()));
                let cmd = ui.input(|i| i.modifiers.command);
                if zoom_delta != 1.0 || (cmd && scroll.y != 0.0) {
                    let factor = if zoom_delta != 1.0 { zoom_delta } else { (1.0 + scroll.y * 0.01).clamp(0.5, 2.0) };
                    let z = self.view.zoom;
                    let nz = (z * factor).clamp(0.05, 8.0);
                    if let Some(p) = ptr {
                        let rel = p - rect.center();
                        self.view.pan = rel - (rel - self.view.pan) * (nz / z);
                    }
                    self.view.zoom = nz;
                } else if scroll != Vec2::ZERO {
                    self.view.pan += scroll;
                }
            }

            let xf = Xform { origin: rect.center() + self.view.pan - page.size() * self.view.zoom / 2.0, zoom: self.view.zoom };
            let page_s = xf.rect(page);
            self.view.page_screen_rect = page_s;
            let painter = ui.painter_at(rect);

            // Page shadow + background
            painter.rect_filled(page_s.translate(Vec2::new(0.0, 3.0)).expand(2.0), 2.0, Color32::from_black_alpha(90));
            let slide_idx = self.cur_slide;
            let bg = c32(self.doc.background);
            {
                let slide = &self.doc.slides[slide_idx];
                let local_dir = self.settings.local_data_dir.clone();
                let _ = local_dir;
                let editing = self.editing_text.map(|id| (id, self.editing_cell));
                draw_slide_except(&painter, &xf, slide, page, bg, &mut self.images, editing, false);
                if self.spelling_visible() {
                    draw_squiggles(&painter, &xf, slide, &self.spell, editing);
                }
            }
            self.text_editor(ui, rect, &xf);
            if self.settings.show_grid && self.settings.grid_size * xf.zoom >= 4.0 {
                let p = painter.with_clip_rect(page_s.intersect(rect));
                let g = self.settings.grid_size;
                let mut x = 0.0;
                while x <= page.width() {
                    let sx = xf.to_screen(Pos2::new(x, 0.0)).x;
                    p.line_segment([Pos2::new(sx, page_s.min.y), Pos2::new(sx, page_s.max.y)], Stroke::new(1.0, Color32::from_rgba_unmultiplied(100, 140, 220, 50)));
                    x += g;
                }
                let mut y = 0.0;
                while y <= page.height() {
                    let sy = xf.to_screen(Pos2::new(0.0, y)).y;
                    p.line_segment([Pos2::new(page_s.min.x, sy), Pos2::new(page_s.max.x, sy)], Stroke::new(1.0, Color32::from_rgba_unmultiplied(100, 140, 220, 50)));
                    y += g;
                }
            }
            self.draw_guides(&painter, rect, &xf);
            painter.text(page_s.left_top() + Vec2::new(0.0, -8.0), egui::Align2::LEFT_BOTTOM, self.doc.size_label(), FontId::proportional(11.0), theme::text_dim());
            self.draw_rulers(&painter, rect, &xf);
            self.image_drop(ui, rect, page_s, &xf, &painter);

            // Interaction
            self.canvas_interaction(ui, &response, rect, &xf);

            // Selection outlines, handles, marquee
            self.draw_selection(&painter, &xf);
            self.draw_crop(&painter, &xf);
            if self.stroke.len() >= 2 {
                let pts = self.stroke.iter().map(|p| xf.to_screen(*p)).collect();
                painter.add(Shape::line(pts, Stroke::new((self.pen_width * xf.zoom).max(0.5), c32(self.pen_color))));
            }
            if let Some(Drag::Marquee { start, current, .. }) = &self.drag {
                let r = xf.rect(Rect::from_two_pos(*start, *current));
                painter.rect(r, 0.0, Color32::from_rgba_unmultiplied(47, 128, 237, 30), Stroke::new(1.0, theme::accent()), StrokeKind::Inside);
            }
        });
    }

    /// The text box being edited: a `TextEdit` laid over it (upright), laid out exactly as the
    /// canvas draws it, so typing happens in place.
    fn text_editor(&mut self, ui: &mut egui::Ui, canvas: Rect, xf: &Xform) {
        let Some(id) = self.editing_text else { return };
        let Some(before) = self.item(id).filter(|i| matches!(i.kind, ItemKind::Text { .. } | ItemKind::Table { .. }) && self.selection.contains(&id)).cloned() else {
            self.finish_text_edit();
            return;
        };
        if let ItemKind::Table { .. } = before.kind {
            return self.cell_editor(ui, canvas, xf, before);
        }
        let z = xf.zoom;
        let rs = xf.rect(before.rect());
        let mut edited = before.clone();
        let ItemKind::Text { text, color, bold, align, valign, .. } = &mut edited.kind else { return };
        let col = c32(*color).gamma_multiply(before.opacity.clamp(0.0, 1.0));
        let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _wrap: f32| ui.painter().layout_job(text_job(ui.ctx(), buf.as_str(), &before.kind, z, rs.width(), col));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rs).layout(egui::Layout::top_down(egui::Align::Min)));
        child.set_clip_rect(canvas.intersect(ui.clip_rect()));
        // A blinking caret in the text's own colour (the theme's would vanish on a light page).
        let caret = &mut child.visuals_mut().text_cursor;
        caret.stroke = Stroke::new((1.5 * z.sqrt()).clamp(1.0, 3.0), c32(*color));
        caret.blink = true;
        let out = egui::TextEdit::multiline(text)
            .id(egui::Id::new(("text_editor", id)))
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO)
            .desired_width(rs.width())
            .min_size(rs.size())
            .horizontal_align(text_halign(*align))
            .vertical_align(text_valign(*valign))
            .layouter(&mut layouter)
            .show(&mut child);
        if std::mem::take(&mut self.text_edit_focus) {
            out.response.request_focus();
        }
        // A light outline marks the box while typing (selection outline and handles are hidden meanwhile).
        child.painter().rect_stroke(rs.expand(2.0), 0.0, Stroke::new(1.0, Color32::from_gray(160)), StrokeKind::Outside);
        let pos = out.galley_pos - Vec2::new(out.galley.rect.left(), 0.0);
        if *bold {
            child.painter().galley(pos + Vec2::new(0.6 * z.max(1.0), 0.0), out.galley.clone(), col);
        }
        if self.spelling_visible() {
            crate::spell::paint_squiggles(child.painter(), &out.galley, text, self.spell.at(id, None), z, |q| pos + q.to_vec2());
        }
        // Grow the box to fit what has been typed.
        let need = out.galley.size().y / z;
        if need > edited.size[1] {
            edited.size[1] = need;
        }
        if out.response.lost_focus() && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.finish_text_edit();
        }
        if edited != before {
            self.update_item(&before, edited);
        }
    }

    /// The table cell being edited: a `TextEdit` laid over it, laid out as the canvas draws cells.
    /// Tab and Shift+Tab move to the next and previous cell.
    fn cell_editor(&mut self, ui: &mut egui::Ui, canvas: Rect, xf: &Xform, before: Item) {
        let (id, i, z) = (before.id, self.editing_cell, xf.zoom);
        let mut edited = before.clone();
        let ItemKind::Table { rows, cols, cells, text_color, font_size, .. } = &mut edited.kind else { return };
        let n = ((*rows).max(1) * (*cols).max(1)).min(cells.len());
        if i >= n {
            self.finish_text_edit();
            return;
        }
        let cell = cell_rect(xf.rect(before.rect()), *rows, *cols, i);
        let col = c32(*text_color).gamma_multiply(before.opacity.clamp(0.0, 1.0));
        let font = FontId::proportional((*font_size * z).max(1.0));
        let wrap = (cell.width() - 8.0 * z).max(4.0);
        let te_id = egui::Id::new(("text_editor", id));
        // Taken before the editor sees it, which would type a tab.
        let step = if ui.memory(|m| m.has_focus(te_id)) {
            ui.input_mut(|inp| {
                if inp.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab) {
                    Some(n - 1)
                } else if inp.consume_key(egui::Modifiers::NONE, egui::Key::Tab) {
                    Some(1)
                } else {
                    None
                }
            })
        } else {
            None
        };
        let text = &mut cells[i];
        let focus = std::mem::take(&mut self.text_edit_focus);
        if focus {
            // Every cell shares the editor's id: start with the caret at the end of this one.
            let mut state = egui::text_edit::TextEditState::load(ui.ctx(), te_id).unwrap_or_default();
            state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(text.chars().count()))));
            state.store(ui.ctx(), te_id);
        }
        let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _wrap: f32| ui.painter().layout(buf.as_str().to_string(), font.clone(), col, wrap);
        let inner = Rect::from_min_size(cell.min + Vec2::new(4.0 * z, 0.0), Vec2::new(wrap, cell.height()));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
        child.set_clip_rect(cell.intersect(canvas).intersect(ui.clip_rect()));
        let caret = &mut child.visuals_mut().text_cursor;
        caret.stroke = Stroke::new((1.5 * z.sqrt()).clamp(1.0, 3.0), c32(*text_color));
        caret.blink = true;
        let out = egui::TextEdit::multiline(text)
            .id(te_id)
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO)
            .desired_width(wrap)
            .min_size(inner.size())
            .vertical_align(egui::Align::Center)
            .lock_focus(true)
            .layouter(&mut layouter)
            .show(&mut child);
        if focus {
            out.response.request_focus();
        }
        ui.painter().with_clip_rect(canvas).rect_stroke(cell, 0.0, Stroke::new(2.0, theme::accent()), StrokeKind::Inside);
        if self.spelling_visible() {
            crate::spell::paint_squiggles(child.painter(), &out.galley, text, self.spell.at(id, Some(i)), z, |q| out.galley_pos + q.to_vec2());
        }
        if out.response.lost_focus() && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.finish_text_edit();
        }
        if edited != before {
            self.update_item(&before, edited);
        }
        if let Some(s) = step {
            self.editing_cell = (i + s) % n;
            self.text_edit_focus = true;
        }
    }

    /// Guides run across the whole canvas; the picked one is drawn thicker.
    fn draw_guides(&self, painter: &egui::Painter, rect: Rect, xf: &Xform) {
        for (i, g) in self.doc.guides.iter().enumerate() {
            let picked = self.selected_guide == Some(i);
            let stroke = if picked { Stroke::new(2.0, theme::accent()) } else { Stroke::new(1.0, Color32::from_rgb(236, 72, 153)) };
            let ends = if g.vertical {
                let x = xf.to_screen(Pos2::new(g.pos, 0.0)).x;
                [Pos2::new(x, rect.min.y), Pos2::new(x, rect.max.y)]
            } else {
                let y = xf.to_screen(Pos2::new(0.0, g.pos)).y;
                [Pos2::new(rect.min.x, y), Pos2::new(rect.max.x, y)]
            };
            painter.line_segment(ends, stroke);
            // Its position while it's being dragged.
            if matches!(self.drag, Some(Drag::Guide(d)) if d == i) {
                let unit = self.doc.unit;
                let text = format!("{:.2} {}", unit.from_px(g.pos), unit.label());
                let at = if g.vertical { Pos2::new(ends[0].x + 6.0, rect.min.y + 22.0) } else { Pos2::new(rect.min.x + 22.0, ends[0].y - 6.0) };
                let align = if g.vertical { egui::Align2::LEFT_TOP } else { egui::Align2::LEFT_BOTTOM };
                painter.text(at, align, text, FontId::proportional(11.0), theme::accent());
            }
        }
    }

    /// The guide under the screen point, if any (topmost first).
    fn guide_at(&self, p: Pos2, xf: &Xform) -> Option<usize> {
        self.doc.guides.iter().enumerate().rev().find(|(_, g)| {
            let s = xf.to_screen(Pos2::new(g.pos, g.pos));
            (if g.vertical { p.x - s.x } else { p.y - s.y }).abs() <= 4.0
        }).map(|(i, _)| i)
    }

    /// Pick a guide (and nothing else).
    fn select_guide(&mut self, i: usize) {
        self.select(Vec::new());
        self.selected_guide = Some(i);
    }

    fn draw_rulers(&self, painter: &egui::Painter, rect: Rect, xf: &Xform) {
        let unit = if self.doc.unit == Unit::Px { Unit::Inch } else { self.doc.unit };
        let step = unit.to_px(1.0) * xf.zoom;
        if step < 12.0 {
            return;
        }
        let col = theme::border();
        let txt = theme::text_dim();
        let h = Rect::from_min_size(rect.min, Vec2::new(rect.width(), 16.0));
        let v = Rect::from_min_size(rect.min, Vec2::new(16.0, rect.height()));
        painter.rect_filled(h, 0.0, theme::bg());
        painter.rect_filled(v, 0.0, theme::bg());
        let origin = xf.to_screen(Pos2::ZERO);
        let first = ((rect.min.x - origin.x) / step).floor() as i32;
        let last = ((rect.max.x - origin.x) / step).ceil() as i32;
        for i in first..=last {
            let x = origin.x + i as f32 * step;
            painter.line_segment([Pos2::new(x, h.min.y + 10.0), Pos2::new(x, h.max.y)], Stroke::new(1.0, col));
            painter.text(Pos2::new(x + 2.0, h.min.y + 1.0), egui::Align2::LEFT_TOP, i.to_string(), FontId::proportional(9.0), txt);
        }
        let first = ((rect.min.y - origin.y) / step).floor() as i32;
        let last = ((rect.max.y - origin.y) / step).ceil() as i32;
        for i in first..=last {
            let y = origin.y + i as f32 * step;
            painter.line_segment([Pos2::new(v.min.x + 10.0, y), Pos2::new(v.max.x, y)], Stroke::new(1.0, col));
            painter.text(Pos2::new(v.min.x + 2.0, y + 2.0), egui::Align2::LEFT_TOP, i.to_string(), FontId::proportional(9.0), txt);
        }
    }

    fn draw_selection(&self, painter: &egui::Painter, xf: &Xform) {
        // The text box being typed in shows only the editor's own outline.
        if self.selection.is_empty() || self.editing_text.is_some() || self.crop_geom().is_some() {
            return;
        }
        let items = self.selected_items();
        let hover_stroke = Stroke::new(1.5, theme::accent());
        for it in &items {
            if it.is_line() {
                let pts: Vec<Pos2> = line_points(it).iter().map(|p| xf.to_screen(*p)).collect();
                painter.add(Shape::line(pts, Stroke::new(1.0, theme::accent())));
                for p in line_ends(it).map(|p| xf.to_screen(p)) {
                    painter.circle_filled(p, HANDLE_R, Color32::WHITE);
                    painter.add(Shape::circle_stroke(p, HANDLE_R, Stroke::new(1.5, theme::handle())));
                }
                continue;
            }
            let pts: Vec<Pos2> = corners(it).iter().map(|p| xf.to_screen(*p)).collect();
            painter.add(Shape::closed_line(pts, hover_stroke));
        }
        let single = if items.len() == 1 && !items[0].is_line() { Some(items[0]) } else { None };
        if items.len() == 1 && items[0].is_line() {
            return;
        }
        let sel = self.selection_rect().unwrap();
        if single.is_none() {
            painter.rect_stroke(xf.rect(sel), 0.0, Stroke::new(1.0, theme::accent()), StrokeKind::Outside);
        }
        let locked = items.iter().any(|i| i.locked);
        let hp = handle_points(single, sel, xf);
        for (i, p) in hp.iter().enumerate() {
            if i == 8 {
                painter.line_segment([hp[1], *p], Stroke::new(1.0, theme::accent()));
                painter.circle_filled(*p, HANDLE_R + 1.0, Color32::WHITE);
                painter.add(Shape::circle_stroke(*p, HANDLE_R + 1.0, Stroke::new(1.5, theme::handle())));
                painter.text(*p, egui::Align2::CENTER_CENTER, ph::ARROW_CLOCKWISE, FontId::proportional(9.0), theme::handle());
            } else if locked {
                painter.rect_filled(Rect::from_center_size(*p, Vec2::splat(7.0)), 1.0, Color32::from_gray(160));
            } else {
                painter.rect_filled(Rect::from_center_size(*p, Vec2::splat(2.0 * HANDLE_R)), 1.0, Color32::WHITE);
                painter.rect_stroke(Rect::from_center_size(*p, Vec2::splat(2.0 * HANDLE_R)), 1.0, Stroke::new(1.5, theme::handle()), StrokeKind::Inside);
            }
        }
    }

    fn canvas_interaction(&mut self, ui: &mut egui::Ui, response: &egui::Response, rect: Rect, xf: &Xform) {
        let ptr = response.interact_pointer_pos().or_else(|| response.hover_pos());
        let shift = ui.input(|i| i.modifiers.shift);
        let space = ui.input(|i| i.key_down(egui::Key::Space));
        let middle = ui.input(|i| i.pointer.middle_down());
        let g = self.settings.grid_size;
        let alt = ui.input(|i| i.modifiers.alt);
        let snapping = self.settings.grid_snap && !alt;
        let guides: Vec<Guide> = if self.settings.guide_snap && !alt { self.doc.guides.clone() } else { Vec::new() };
        let tol = 6.0 / xf.zoom;

        // Cursor feedback
        if let (Some(p), None) = (ptr, &self.drag) {
            if let Some((h, _)) = self.crop_handle_at(p, xf) {
                ui.ctx().set_cursor_icon(handle_cursor(h));
            } else if let Some(h) = self.handle_at(p, xf) {
                ui.ctx().set_cursor_icon(handle_cursor(h));
            } else if let Some(i) = self.guide_at(p, xf).filter(|_| !space) {
                ui.ctx().set_cursor_icon(if self.doc.guides[i].vertical { egui::CursorIcon::ResizeHorizontal } else { egui::CursorIcon::ResizeVertical });
            } else if space {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            }
        }

        if self.placing_text {
            if response.contains_pointer() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
            if response.clicked() {
                if let Some(p) = ptr {
                    self.actions.push(Action::PlaceText(xf.to_doc(p)));
                }
            }
            return;
        }
        if self.drawing && !space && !middle {
            if response.contains_pointer() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
            if response.drag_started() {
                self.stroke.clear();
                if let Some(p) = ui.input(|i| i.pointer.press_origin()).or(ptr) {
                    self.stroke.push(xf.to_doc(p));
                }
            }
            if response.dragged() {
                if let Some(p) = ptr.map(|p| xf.to_doc(p)) {
                    // Skip points closer than a screen pixel or so to the last one.
                    if self.stroke.last().is_none_or(|q| (p - *q).length() * xf.zoom >= 1.5) {
                        self.stroke.push(p);
                    }
                }
            }
            if response.drag_stopped() && !self.stroke.is_empty() {
                self.actions.push(Action::PlaceDrawing(std::mem::take(&mut self.stroke)));
            }
            return;
        }
        // A press anywhere on the canvas but the text being edited ends the edit; a click on another
        // cell of the table being edited moves to it.
        if let Some(eid) = self.editing_text {
            if response.clicked() || response.drag_started() || response.secondary_clicked() {
                let cell = ptr.filter(|_| response.clicked()).and_then(|p| self.item(eid).and_then(|it| table_cell_at(it, xf.to_doc(p))));
                match cell {
                    Some(c) => self.start_cell_edit(eid, c),
                    None => self.finish_text_edit(),
                }
            }
        }

        if response.drag_started() {
            // Hit-test where the press began: a touch drag only starts once the finger has moved.
            let touch = ui.input(|i| i.any_touches());
            if let Some(p) = ui.input(|i| i.pointer.press_origin()).or(ptr) {
                let d = xf.to_doc(p);
                if space || middle {
                    self.drag = Some(Drag::Pan);
                } else if let Some((h, g)) = self.crop_handle_at(p, xf) {
                    self.commit();
                    self.drag = Some(Drag::Crop { handle: h, start: g, start_ptr: g.local(d) });
                } else if let Some(h) = self.handle_at(p, xf) {
                    self.commit();
                    let items = self.selected_items();
                    if h == 8 {
                        let it = items[0];
                        let c = it.center();
                        self.drag = Some(Drag::Rotate { id: it.id, center: c, start_rot: it.rotation, start_angle: (d - c).angle() });
                    } else if items.len() == 1 && items[0].is_line() {
                        let path = if matches!(items[0].kind, ItemKind::Line { kind: LineKind::Freehand, .. }) { line_points(items[0]) } else { Vec::new() };
                        self.drag = Some(Drag::LineEnd { id: items[0].id, end: h == 4, path });
                    } else {
                        let single = if items.len() == 1 { Some((items[0].id, items[0].rotation, items[0].center())) } else { None };
                        let lock_aspect = items.len() > 1 || matches!(items[0].kind, ItemKind::Image { .. }) || shift;
                        self.drag = Some(Drag::Resize {
                            handle: h,
                            start_sel: self.selection_rect().unwrap(),
                            start_rects: items.iter().map(|i| (i.id, i.rect())).collect(),
                            single,
                            lock_aspect,
                        });
                    }
                } else if let Some(i) = self.guide_at(p, xf) {
                    self.commit();
                    self.select_guide(i);
                    self.drag = Some(Drag::Guide(i));
                } else if let Some(id) = self.item_at(d, tol) {
                    if !self.selection.contains(&id) {
                        if shift {
                            self.selection.push(id);
                            self.expand_groups();
                        } else if touch {
                            // On a phone the Edit panel would cover the canvas mid-drag; a tap opens it.
                            self.selection = vec![id];
                            self.expand_groups();
                            self.crop_mode = false;
                        } else {
                            self.select(vec![id]);
                        }
                    }
                    let locked = self.selected_items().iter().any(|i| i.locked);
                    if !locked {
                        self.commit();
                        let start_rects = self.selected_items().iter().map(|i| (i.id, i.rect())).collect();
                        self.drag = Some(Drag::Move { start_rects, start_ptr: d, sel_rect: self.selection_rect().unwrap() });
                    }
                } else if touch {
                    // One finger on empty canvas pans; there's no marquee on touch screens.
                    self.drag = Some(Drag::Pan);
                } else {
                    self.drag = Some(Drag::Marquee { start: d, current: d, additive: shift });
                    if !shift {
                        self.select(Vec::new());
                    }
                }
            }
        }

        if response.dragged() {
            if let Some(p) = ptr {
                let d = xf.to_doc(p);
                let delta = response.drag_delta();
                match &mut self.drag {
                    Some(Drag::Pan) => self.view.pan += delta,
                    Some(Drag::Guide(i)) => {
                        let i = *i;
                        if let Some(guide) = self.doc.guides.get_mut(i) {
                            let v = if guide.vertical { d.x } else { d.y };
                            guide.pos = if snapping { snap(v, g) } else { v };
                        }
                    }
                    Some(Drag::Marquee { current, .. }) => *current = d,
                    Some(Drag::Crop { handle, start, start_ptr }) => {
                        let (h, g) = (*handle, *start);
                        let lp = g.local(d);
                        let (f, full) = (g.frame, g.full);
                        let (frame, full) = if h == 8 {
                            // Slide the image under the frame, never uncovering it.
                            let dv = lp - *start_ptr;
                            let dx = dv.x.max(f.max.x - full.max.x).min(f.min.x - full.min.x);
                            let dy = dv.y.max(f.max.y - full.max.y).min(f.min.y - full.min.y);
                            (f, full.translate(Vec2::new(dx, dy)))
                        } else {
                            let min_size = 4.0;
                            let (mut min, mut max) = (f.min, f.max);
                            if matches!(h, 0 | 6 | 7) { min.x = lp.x.min(max.x - min_size).max(full.min.x); }
                            if matches!(h, 2 | 3 | 4) { max.x = lp.x.max(min.x + min_size).min(full.max.x); }
                            if matches!(h, 0 | 1 | 2) { min.y = lp.y.min(max.y - min_size).max(full.min.y); }
                            if matches!(h, 4 | 5 | 6) { max.y = lp.y.max(min.y + min_size).min(full.max.y); }
                            (Rect::from_min_max(min, max), full)
                        };
                        if let Some(it) = self.item_mut(g.id) {
                            g.apply(it, frame, full);
                        }
                    }
                    Some(Drag::Move { start_rects, start_ptr, sel_rect }) => {
                        let mut dv = d - *start_ptr;
                        if snapping {
                            let nm = sel_rect.min + dv;
                            dv = Pos2::new(snap(nm.x, g), snap(nm.y, g)) - sel_rect.min;
                        }
                        // A guide within reach of an edge or the centre wins over the grid.
                        let r = sel_rect.translate(dv);
                        if let Some(s) = guide_shift(&guides, true, &[r.min.x, r.center().x, r.max.x], tol) { dv.x += s; }
                        if let Some(s) = guide_shift(&guides, false, &[r.min.y, r.center().y, r.max.y], tol) { dv.y += s; }
                        if shift {
                            if dv.x.abs() > dv.y.abs() { dv.y = 0.0 } else { dv.x = 0.0 }
                        }
                        let moves: Vec<(ItemId, Rect)> = start_rects.iter().map(|(id, r)| (*id, r.translate(dv))).collect();
                        for (id, r) in moves {
                            if let Some(it) = self.item_mut(id) {
                                it.set_rect(r);
                            }
                        }
                    }
                    Some(Drag::Rotate { id, center, start_rot, start_angle }) => {
                        let mut a = *start_rot + ((d - *center).angle() - *start_angle);
                        if shift {
                            let s = 15f32.to_radians();
                            a = (a / s).round() * s;
                        }
                        let id = *id;
                        if let Some(it) = self.item_mut(id) {
                            it.rotation = a;
                        }
                    }
                    Some(Drag::LineEnd { id, end, path }) => {
                        let id = *id;
                        let end = *end;
                        let path = std::mem::take(path);
                        let mut q = d;
                        if snapping {
                            q = Pos2::new(snap(q.x, g), snap(q.y, g));
                        }
                        q = snap_point_to_guides(&guides, q, tol);
                        if let Some(it) = self.item_mut(id) {
                            if !path.is_empty() {
                                set_freehand_path(it, &move_path_end(&path, end, q));
                            } else if end {
                                it.size = [q.x - it.pos[0], q.y - it.pos[1]];
                            } else {
                                let e = it.pos() + it.size();
                                it.pos = [q.x, q.y];
                                it.size = [e.x - q.x, e.y - q.y];
                            }
                        }
                        // Keep the starting path for the next frame (taken above to free `self`).
                        if let Some(Drag::LineEnd { path: p, .. }) = &mut self.drag {
                            *p = path;
                        }
                    }
                    Some(Drag::Resize { handle, start_sel, start_rects, single, lock_aspect }) => {
                        let handle = *handle;
                        let start_sel = *start_sel;
                        let lock = *lock_aspect;
                        // Work in the item's local frame when a single rotated item is selected.
                        let (local_ptr, local_start) = match single {
                            Some((_, rot, c)) => (rotate_about(d, *c, -*rot), start_sel),
                            None => (d, start_sel),
                        };
                        let mut lp = local_ptr;
                        if single.map(|s| s.1 == 0.0).unwrap_or(true) {
                            if snapping {
                                lp = Pos2::new(snap(lp.x, g), snap(lp.y, g));
                            }
                            lp = snap_point_to_guides(&guides, lp, tol);
                        }
                        let mut min = local_start.min;
                        let mut max = local_start.max;
                        match handle {
                            0 => { min = lp; }
                            1 => { min.y = lp.y; }
                            2 => { min.y = lp.y; max.x = lp.x; }
                            3 => { max.x = lp.x; }
                            4 => { max = lp; }
                            5 => { max.y = lp.y; }
                            6 => { min.x = lp.x; max.y = lp.y; }
                            _ => { min.x = lp.x; }
                        }
                        let mut w = (max.x - min.x).max(4.0);
                        let mut h = (max.y - min.y).max(4.0);
                        if lock && matches!(handle, 0 | 2 | 4 | 6) {
                            let ar = local_start.width() / local_start.height().max(0.001);
                            if w / h > ar { h = w / ar } else { w = h * ar }
                        }
                        let (nx, ny) = (
                            if matches!(handle, 0 | 6 | 7) { max.x - w } else { min.x },
                            if matches!(handle, 0 | 1 | 2) { max.y - h } else { min.y },
                        );
                        let new_sel = Rect::from_min_size(Pos2::new(nx, ny), Vec2::new(w, h));
                        let sx = new_sel.width() / local_start.width().max(0.001);
                        let sy = new_sel.height() / local_start.height().max(0.001);
                        let updates: Vec<(ItemId, Rect)> = start_rects
                            .iter()
                            .map(|(id, r)| {
                                let nr = Rect::from_min_size(
                                    Pos2::new(new_sel.min.x + (r.min.x - local_start.min.x) * sx, new_sel.min.y + (r.min.y - local_start.min.y) * sy),
                                    Vec2::new(r.width() * sx, r.height() * sy),
                                );
                                (*id, nr)
                            })
                            .collect();
                        let single = *single;
                        for (id, mut nr) in updates {
                            if let Some((_, rot, c)) = single {
                                // Convert local-frame rect back: rotate its center about the old center.
                                let nc = rotate_about(nr.center(), c, rot);
                                nr = Rect::from_center_size(nc, nr.size());
                            }
                            if let Some(it) = self.item_mut(id) {
                                if let ItemKind::Text { size, .. } = &mut it.kind {
                                    if lock && matches!(handle, 0 | 2 | 4 | 6) {
                                        *size = (*size * sy).max(4.0);
                                    }
                                }
                                it.set_rect(nr);
                            }
                        }
                    }
                    None => {}
                }
            }
        }

        if response.drag_stopped() {
            if let Some(Drag::Marquee { start, current, additive }) = &self.drag {
                let r = Rect::from_two_pos(*start, *current);
                let mut ids: Vec<ItemId> = self.slide().items.iter().filter(|i| r.intersects(i.rect())).map(|i| i.id).collect();
                if *additive {
                    ids.extend(self.selection.iter().copied());
                    ids.dedup();
                }
                self.select(ids);
            }
            self.drag = None;
        }

        // A click on the image being cropped (or the part cropped away) keeps cropping.
        if response.clicked() && !ptr.is_some_and(|p| self.crop_handle_at(p, xf).is_some()) {
            if let Some(p) = ptr {
                let d = xf.to_doc(p);
                if let Some(i) = self.guide_at(p, xf) {
                    self.select_guide(i);
                } else {
                    match self.item_at(d, tol) {
                        Some(id) if shift => self.actions.push(Action::ToggleSelect(id)),
                        Some(id) => self.select(vec![id]),
                        None => self.select(Vec::new()),
                    }
                }
            }
        }
        if response.double_clicked() {
            if let Some(p) = ptr {
                let d = xf.to_doc(p);
                if let Some(id) = self.item_at(d, tol) {
                    self.select(vec![id]);
                    self.rail = Some(crate::app::RailTab::Edit);
                    match self.item(id).and_then(|it| table_cell_at(it, d)) {
                        Some(cell) => self.actions.push(Action::EditCell(id, cell)),
                        None => self.actions.push(Action::EditText(id)),
                    }
                }
            }
        }
        // What the right-click menu is for: a guide, the item(s) under the pointer, or the canvas.
        if response.secondary_clicked() {
            if let Some(p) = ptr {
                if let Some(i) = self.guide_at(p, xf) {
                    self.select_guide(i);
                } else {
                    match self.item_at(xf.to_doc(p), tol) {
                        Some(id) if self.selection.contains(&id) => self.selected_guide = None,
                        Some(id) => self.select(vec![id]),
                        None => self.select(Vec::new()),
                    }
                }
            }
        }
        response.context_menu(|ui| self.context_menu_ui(ui));
        let _ = rect;
    }

    fn item_at(&self, d: Pos2, tol: f32) -> Option<ItemId> {
        self.slide().items.iter().rev().find(|it| hit_test(it, d, tol)).map(|it| it.id)
    }

    /// Which handle (0..8 edges/corners, 8 rotate) is under the screen point, if any.
    fn handle_at(&self, p: Pos2, xf: &Xform) -> Option<usize> {
        if self.selection.is_empty() || self.editing_text.is_some() || self.crop_geom().is_some() {
            return None;
        }
        let items = self.selected_items();
        if items.iter().any(|i| i.locked) {
            return None;
        }
        if items.len() == 1 && items[0].is_line() {
            let [a, b] = line_ends(items[0]).map(|p| xf.to_screen(p));
            if (p - a).length() <= HANDLE_R + 3.0 {
                return Some(0);
            }
            if (p - b).length() <= HANDLE_R + 3.0 {
                return Some(4);
            }
            return None;
        }
        let single = if items.len() == 1 { Some(items[0]) } else { None };
        let hp = handle_points(single, self.selection_rect()?, xf);
        hp.iter().enumerate().find(|(_, h)| (p - **h).length() <= HANDLE_R + 3.0).map(|(i, _)| i)
    }

    /// The right-click menu for empty canvas, in the Page panel's order.
    fn canvas_menu_ui(&mut self, ui: &mut egui::Ui) {
        let any = !self.slide().items.is_empty();
        if ui.add_enabled(any, egui::Button::new(format!("{}  Select all", ph::SELECTION_ALL)).shortcut_text("⌘A")).clicked() {
            self.actions.push(Action::Select(self.slide().items.iter().map(|x| x.id).collect()));
            ui.close();
        }
        ui.separator();
        if ui.button(format!("{}  Add vertical guide", ph::LINE_VERTICAL)).clicked() {
            self.actions.push(Action::AddGuide(true));
            ui.close();
        }
        if ui.button(format!("{}  Add horizontal guide", ph::MINUS)).clicked() {
            self.actions.push(Action::AddGuide(false));
            ui.close();
        }
        // Buttons rather than ui.checkbox so the box sits on the right, as in the main menu.
        let check = |on: bool| if on { ph::CHECK_SQUARE } else { ph::SQUARE };
        if ui.add(egui::Button::new(format!("{}  Snap to guides", ph::MAGNET)).right_text(check(self.settings.guide_snap))).on_hover_text("Hold Alt (Option) while dragging to place freely").clicked() {
            self.settings.guide_snap = !self.settings.guide_snap;
        }
        if ui.add_enabled(!self.doc.guides.is_empty(), egui::Button::new(format!("{}  Clear all guides", ph::TRASH))).clicked() {
            self.actions.push(Action::ClearGuides);
            ui.close();
        }
        ui.separator();
        if ui.add(egui::Button::new(format!("{}  Show gridlines", ph::GRID_FOUR)).right_text(check(self.settings.show_grid))).clicked() {
            self.settings.show_grid = !self.settings.show_grid;
        }
        if ui.add(egui::Button::new(format!("{}  Snap to grid", ph::MAGNET)).right_text(check(self.settings.grid_snap))).on_hover_text("Hold Alt (Option) while dragging to place freely").clicked() {
            self.settings.grid_snap = !self.settings.grid_snap;
        }
        ui.separator();
        if ui.button(format!("{}  Show credits", ph::QUOTES)).on_hover_text("Credits for the library artwork on this slide").clicked() {
            self.show_attributions = true;
            self.attributions_slide = Some(self.cur_slide);
            ui.close();
        }
    }

    /// The right-click menu for the selection.
    fn context_menu_ui(&mut self, ui: &mut egui::Ui) {
        if self.selected_guide.is_some() {
            if ui.add(egui::Button::new(format!("{}  Delete guide", ph::TRASH)).shortcut_text("⌫")).clicked() {
                self.actions.push(Action::Delete);
                ui.close();
            }
            return;
        }
        if self.selection.is_empty() {
            self.canvas_menu_ui(ui);
            return;
        }
        let items = self.selected_items();
        let has = !items.is_empty();
        let single_image = match items[..] {
            [it] => matches!(it.kind, ItemKind::Image { .. }),
            _ => false,
        };
        let locked = has && items.iter().all(|i| i.locked);
        let grouped = self.is_grouped();
        let multi = items.len() > 1;
        // A single library image (or an edit of one) has a credit to show.
        let (credited, icon_name) = match items[..] {
            [it] => self.item_credit(it).map(|(title, _)| (Some(it.id), Some(title))).unwrap_or_default(),
            _ => (None, None),
        };
        let asset_id = match items[..] {
            [it] => self.item_asset_id(it),
            _ => None,
        };
        let mut acts: Vec<Action> = Vec::new();
        let mut item = |ui: &mut egui::Ui, enabled: bool, icon: &str, label: &str, shortcut: &str, act: Action| {
            let b = egui::Button::new(format!("{icon}  {label}")).shortcut_text(shortcut);
            if ui.add_enabled(enabled, b).clicked() {
                acts.push(act);
                ui.close();
            }
        };
        item(ui, has, ph::SCISSORS, "Cut", "⌘X", Action::Cut);
        item(ui, has, ph::COPY, "Copy", "⌘C", Action::Copy);
        item(ui, !self.clipboard.is_empty(), ph::CLIPBOARD, "Paste", "⌘V", Action::Paste);
        item(ui, has, ph::COPY_SIMPLE, "Duplicate", "⌘D", Action::Duplicate);
        item(ui, has, ph::TRASH, "Delete", "⌫", Action::Delete);
        ui.separator();
        ui.add_enabled_ui(has, |ui| {
            ui.menu_button(format!("{}  Align", ph::ALIGN_LEFT), |ui| {
                for (l, k, ic) in [
                    ("Left", AlignKind::Left, ph::ALIGN_LEFT),
                    ("Center", AlignKind::CenterH, ph::ALIGN_CENTER_HORIZONTAL),
                    ("Right", AlignKind::Right, ph::ALIGN_RIGHT),
                    ("Top", AlignKind::Top, ph::ALIGN_TOP),
                    ("Middle", AlignKind::Middle, ph::ALIGN_CENTER_VERTICAL),
                    ("Bottom", AlignKind::Bottom, ph::ALIGN_BOTTOM),
                ] {
                    item(ui, true, ic, l, "", Action::Align(k));
                }
            });
        });
        if grouped {
            item(ui, has, ph::SELECTION_SLASH, "Ungroup", "⇧⌘G", Action::Ungroup);
        } else {
            item(ui, multi, ph::SELECTION_ALL, "Group", "⌘G", Action::Group);
        }
        ui.add_enabled_ui(has, |ui| {
            ui.menu_button(format!("{}  Arrange", ph::STACK), |ui| {
                for (l, k, ic) in [("Bring to front", ArrangeKind::Front, ph::ARROW_LINE_UP), ("Bring forward", ArrangeKind::Forward, ph::ARROW_UP), ("Send backward", ArrangeKind::Backward, ph::ARROW_DOWN), ("Send to back", ArrangeKind::Back, ph::ARROW_LINE_DOWN)] {
                    item(ui, true, ic, l, "", Action::Arrange(k));
                }
            });
            ui.menu_button(format!("{}  Flip", ph::FLIP_HORIZONTAL), |ui| {
                item(ui, true, ph::FLIP_HORIZONTAL, "Flip horizontal", "", Action::FlipH);
                item(ui, true, ph::FLIP_VERTICAL, "Flip vertical", "", Action::FlipV);
            });
        });
        item(ui, single_image && !locked, ph::CROP, "Crop", "", Action::SetCrop(true));
        item(ui, has, if locked { ph::LOCK_OPEN } else { ph::LOCK }, if locked { "Unlock" } else { "Lock" }, "⌘L", Action::ToggleLock);
        ui.separator();
        item(ui, single_image, ph::SPARKLE, "AI", "", Action::OpenAiEdit);
        self.actions.extend(acts);
        ui.separator();
        let fav = asset_id.as_ref().is_some_and(|id| self.settings.favorites.contains(id));
        let tip = if asset_id.is_some() { "Favorites are listed in the Icons panel" } else { "Only library artwork can be a favorite" };
        let b = egui::Button::new(format!("{}  {}", ph::STAR, if fav { "Unfavorite" } else { "Favorite" }));
        if ui.add_enabled(asset_id.is_some(), b).on_hover_text(tip).on_disabled_hover_text(tip).clicked() {
            self.actions.extend(asset_id.map(Action::ToggleFavorite));
            ui.close();
        }
        let tip = if icon_name.is_some() { "Copy this icon's name to the clipboard" } else { "Only library artwork has a name" };
        if ui.add_enabled(icon_name.is_some(), egui::Button::new(format!("{}  Copy name", ph::COPY))).on_hover_text(tip).on_disabled_hover_text(tip).clicked() {
            ui.ctx().copy_text(icon_name.unwrap_or_default());
            ui.close();
        }
        let b = egui::Button::new(format!("{}  Show credit", ph::QUOTES));
        let tip = if credited.is_some() { "Who made this artwork, ready to copy" } else { "Only library artwork has a credit" };
        if ui.add_enabled(credited.is_some(), b).on_hover_text(tip).on_disabled_hover_text(tip).clicked() {
            self.credit_item = credited;
            ui.close();
        }
    }

    /// The selected image, when it is being cropped.
    fn crop_geom(&self) -> Option<CropGeom> {
        if !self.crop_mode || self.editing_text.is_some() {
            return None;
        }
        self.single_selected().filter(|it| !it.locked).and_then(CropGeom::of)
    }

    /// The crop handle under the screen point: 0..8 the frame's edges and corners, 8 anywhere
    /// on the whole image (to slide it under the frame).
    fn crop_handle_at(&self, p: Pos2, xf: &Xform) -> Option<(usize, CropGeom)> {
        let g = self.crop_geom()?;
        if let Some(h) = rect_handles(g.frame).iter().position(|h| (g.screen(xf, *h) - p).length() <= 10.0) {
            return Some((h, g));
        }
        g.full.contains(g.local(xf.to_doc(p))).then_some((8, g))
    }

    /// Cropping, as in PowerPoint: the cropped-away part of the image shows faded around the
    /// frame, whose edges and corners carry black crop handles.
    fn draw_crop(&mut self, painter: &egui::Painter, xf: &Xform) {
        let Some(g) = self.crop_geom() else { return };
        let Some(it) = self.item(g.id).cloned() else { return };
        let ItemKind::Image { source, .. } = &it.kind else { return };
        let state = match source {
            crate::doc::ImageSource::Asset { key, svg, .. } => self.images.asset_image(key, svg.as_deref(), None),
            crate::doc::ImageSource::File { path, .. } => self.images.file(path),
        };
        let (f, full) = (g.frame, g.full);
        let center = xf.to_screen(g.center);
        if let ImgState::Ready { tex, .. } = state {
            let mut mesh = Mesh::with_texture(tex.id());
            let strips = [
                Rect::from_min_max(full.min, Pos2::new(full.max.x, f.min.y)),
                Rect::from_min_max(Pos2::new(full.min.x, f.max.y), full.max),
                Rect::from_min_max(Pos2::new(full.min.x, f.min.y), Pos2::new(f.min.x, f.max.y)),
                Rect::from_min_max(Pos2::new(f.max.x, f.min.y), Pos2::new(full.max.x, f.max.y)),
            ];
            for s in strips.into_iter().filter(|s| s.width() > 0.0 && s.height() > 0.0) {
                let [mut u0, mut v0, mut u1, mut v1] = g.uv(s);
                if it.flip_h {
                    (u0, u1) = (1.0 - u0, 1.0 - u1);
                }
                if it.flip_v {
                    (v0, v1) = (1.0 - v0, 1.0 - v1);
                }
                mesh.add_rect_with_uv(xf.rect(s), Rect::from_min_max(Pos2::new(u0, v0), Pos2::new(u1, v1)), Color32::WHITE.gamma_multiply(0.35));
            }
            if g.rot != 0.0 {
                mesh.rotate(Rot2::from_angle(g.rot), center);
            }
            painter.add(Shape::mesh(mesh));
        }
        let outline = |r: Rect| rect_handles(r).iter().step_by(2).map(|p| g.screen(xf, *p)).collect::<Vec<_>>();
        painter.add(Shape::closed_line(outline(full), Stroke::new(1.0, Color32::from_gray(150))));
        painter.add(Shape::closed_line(outline(f), Stroke::new(1.0, Color32::from_gray(40))));
        // Each handle is an L at a corner or a bar along an edge, inside the frame.
        let rot = Rot2::from_angle(g.rot);
        let (ux, uy) = (rot * Vec2::X, rot * Vec2::Y);
        let len = 14.0f32.min(f.width() * xf.zoom / 3.0).min(f.height() * xf.zoom / 3.0).max(6.0);
        for (i, h) in rect_handles(f).iter().enumerate() {
            let p = g.screen(xf, *h);
            let segs: Vec<[Pos2; 2]> = match i {
                0 | 2 | 4 | 6 => {
                    let sx = if matches!(i, 0 | 6) { 1.0 } else { -1.0 };
                    let sy = if matches!(i, 0 | 2) { 1.0 } else { -1.0 };
                    vec![[p, p + ux * sx * len], [p, p + uy * sy * len]]
                }
                1 | 5 => vec![[p - ux * len / 2.0, p + ux * len / 2.0]],
                _ => vec![[p - uy * len / 2.0, p + uy * len / 2.0]],
            };
            for [a, b] in &segs {
                painter.line_segment([*a, *b], Stroke::new(6.0, Color32::WHITE));
            }
            for [a, b] in &segs {
                painter.line_segment([*a, *b], Stroke::new(4.0, Color32::BLACK));
            }
        }
    }
}

/// Icon-over-label button used by the rail.
pub fn tool_button(ui: &mut egui::Ui, icon: &str, label: &str, selected: bool) -> egui::Response {
    let size = Vec2::new(50.0, 42.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter();
    if selected || resp.hovered() {
        painter.rect_filled(rect, 6.0, if selected { theme::accent_dim() } else { theme::hover() });
    }
    let col = if selected { theme::on_accent_dim() } else { theme::text() };
    painter.text(Pos2::new(rect.center().x, rect.min.y + 13.0), egui::Align2::CENTER_CENTER, icon, FontId::proportional(17.0), col);
    painter.text(Pos2::new(rect.center().x, rect.max.y - 9.0), egui::Align2::CENTER_CENTER, label, FontId::proportional(9.5), col);
    resp
}

pub fn grid_label(g: f32) -> String {
    format!("{:.3} in", g / DPI)
}
