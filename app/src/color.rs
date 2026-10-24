//! The colour button and its picker popup.
//!
//! egui's own picker can show values as F (0-1 linear) or U8 (0-255), but has no hex view, and its
//! alpha field could not be edited. This one cycles F → U8 → H (hex), and every field edits the same
//! colour the 2D area, hue bar and opacity box do, so they always agree.

use egui::ecolor::{Hsva, HsvaGamma};
use egui::epaint::{CircleShape, Mesh};
use egui::{
    lerp, pos2, remap_clamp, vec2, Color32, DragValue, Id, Painter, Popup, PopupCloseBehavior, Rect, Response, Rgba,
    Sense, Shape, Stroke, StrokeKind, TextEdit, Ui, Vec2, Widget as _,
};

/// Vertices per dimension in the sliders; a multiple of 6 so the hue bar hits every peak hue.
const N: u32 = 6 * 6;
const SLIDER_WIDTH: f32 = 275.0;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Mode {
    /// 0-1, linear.
    F,
    /// 0-255.
    #[default]
    U8,
    /// `#RRGGBB` or `#RRGGBBAA`.
    Hex,
}

impl Mode {
    fn label(self) -> &'static str {
        match self {
            Mode::F => "F",
            Mode::U8 => "U8",
            Mode::Hex => "H",
        }
    }

    fn tooltip(self) -> &'static str {
        match self {
            Mode::F => "Showing values as 0-1 (linear); click for 0-255",
            Mode::U8 => "Showing values as 0-255; click for hex",
            Mode::Hex => "Showing a hex code; click for 0-1",
        }
    }

    fn next(self) -> Self {
        match self {
            Mode::F => Mode::U8,
            Mode::U8 => Mode::Hex,
            Mode::Hex => Mode::F,
        }
    }
}

/// A colour button with transparency (unmultiplied sRGBA, as the document stores it); click it for
/// the picker.
pub fn edit_button(ui: &mut Ui, rgba: &mut [u8; 4]) -> Response {
    button(ui, rgba, true)
}

/// A colour button for an opaque colour.
pub fn edit_button_rgb(ui: &mut Ui, rgb: &mut [u8; 3]) -> Response {
    let mut c = [rgb[0], rgb[1], rgb[2], 255];
    let r = button(ui, &mut c, false);
    *rgb = [c[0], c[1], c[2]];
    r
}

fn button(ui: &mut Ui, rgba: &mut [u8; 4], alpha: bool) -> Response {
    let popup_id = ui.auto_id_with("popup");
    let open = Popup::is_id_open(ui.ctx(), popup_id);
    let mut response = swatch_button(ui, *rgba, open);
    Popup::menu(&response)
        .id(popup_id)
        .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.spacing_mut().slider_width = SLIDER_WIDTH;
            if picker(ui, popup_id, rgba, alpha) {
                response.mark_changed();
            }
        });
    response
}

/// The picker itself. Returns `true` on change.
///
/// The fields and the opacity box set the bytes exactly; only the 2D area and hue bar go through
/// HSV, so editing one value never nudges the others.
fn picker(ui: &mut Ui, id: Id, rgba: &mut [u8; 4], alpha: bool) -> bool {
    if !alpha {
        rgba[3] = 255;
    }
    // Keep the full HSV between frames, so a grey or black doesn't lose its hue.
    let state_id = id.with("hsva");
    let mut hsvag = ui
        .data(|d| d.get_temp::<([u8; 4], HsvaGamma)>(state_id))
        .filter(|(c, _)| c == rgba)
        .map(|(_, h)| h)
        .unwrap_or_else(|| HsvaGamma::from(Hsva::from_srgba_unmultiplied(*rgba)));
    let before = *rgba;

    ui.vertical(|ui| {
        if let Some(c) = numeric_row(ui, id, *rgba, alpha) {
            *rgba = c;
            hsvag = HsvaGamma::from(Hsva::from_srgba_unmultiplied(c));
        }

        let size = vec2(ui.spacing().slider_width, ui.spacing().interact_size.y);
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        show_srgba(ui.painter(), *rgba, rect);

        let opaque = HsvaGamma { a: 1.0, ..hsvag };
        let hsv = hsvag;
        let HsvaGamma { h, s, v, .. } = &mut hsvag;
        slider_2d(ui, s, v, |s, v| HsvaGamma { s, v, ..opaque }.into());
        slider_1d(ui, h, |h| HsvaGamma { h, s: 1.0, v: 1.0, a: 1.0 }.into()).on_hover_text("Hue");
        if hsvag != hsv {
            let [r, g, b, _] = Hsva::from(hsvag).to_srgba_unmultiplied();
            *rgba = [r, g, b, rgba[3]];
        }

        if alpha {
            ui.horizontal(|ui| {
                ui.label("Opacity");
                let mut pct = rgba[3] as f32 / 255.0 * 100.0;
                if DragValue::new(&mut pct).speed(1.0).range(0.0..=100.0).max_decimals(0).suffix("%").ui(ui).changed() {
                    rgba[3] = (pct / 100.0 * 255.0).round() as u8;
                    hsvag.a = rgba[3] as f32 / 255.0;
                }
            });
        }
    });

    ui.data_mut(|d| d.insert_temp(state_id, (*rgba, hsvag)));
    *rgba != before
}

/// The mode toggle, a copy button and the fields for the current mode. Returns the edited colour.
fn numeric_row(ui: &mut Ui, id: Id, srgba: [u8; 4], alpha: bool) -> Option<[u8; 4]> {
    // One mode for every picker, remembered for the session.
    let mode_id = Id::new("color_picker_mode");
    let mut mode = ui.data(|d| d.get_temp::<Mode>(mode_id)).unwrap_or_default();
    let mut out = None;
    ui.horizontal(|ui| {
        if ui.button(mode.label()).on_hover_text(mode.tooltip()).clicked() {
            mode = mode.next();
            ui.data_mut(|d| d.insert_temp(mode_id, mode));
        }
        let linear = Hsva::from_srgba_unmultiplied(srgba).to_rgba_unmultiplied();
        if ui.button(egui_phosphor::regular::COPY).on_hover_text("Copy the color values").clicked() {
            ui.copy_text(match mode {
                Mode::F if alpha => format!("{:.03}, {:.03}, {:.03}, {:.03}", linear[0], linear[1], linear[2], linear[3]),
                Mode::F => format!("{:.03}, {:.03}, {:.03}", linear[0], linear[1], linear[2]),
                Mode::U8 if alpha => format!("{}, {}, {}, {}", srgba[0], srgba[1], srgba[2], srgba[3]),
                Mode::U8 => format!("{}, {}, {}", srgba[0], srgba[1], srgba[2]),
                Mode::Hex => to_hex(srgba, alpha),
            });
        }
        let fields = if alpha { 4 } else { 3 };
        match mode {
            Mode::F => {
                let mut v = linear;
                let mut edited = false;
                for (i, p) in ["R ", "G ", "B ", "A "].iter().enumerate().take(fields) {
                    edited |= DragValue::new(&mut v[i])
                        .speed(0.003)
                        .prefix(*p)
                        .range(0.0..=1.0)
                        .custom_formatter(|n, _| format!("{n:.03}"))
                        .ui(ui)
                        .changed();
                }
                if edited {
                    out = Some(Hsva::from_rgba_unmultiplied(v[0], v[1], v[2], v[3]).to_srgba_unmultiplied());
                }
            }
            Mode::U8 => {
                let mut v = srgba;
                let mut edited = false;
                for (i, p) in ["R ", "G ", "B ", "A "].iter().enumerate().take(fields) {
                    edited |= DragValue::new(&mut v[i]).speed(0.5).prefix(*p).range(0..=255).ui(ui).changed();
                }
                if edited {
                    out = Some(v);
                }
            }
            Mode::Hex => {
                // Keep what is being typed while the field has focus; show the colour otherwise.
                let text_id = id.with("hex");
                let edit_id = text_id.with("edit");
                let focused = ui.memory(|m| m.has_focus(edit_id));
                let current = to_hex(srgba, alpha);
                let mut hex = if focused { ui.data(|d| d.get_temp::<String>(text_id)) } else { None }.unwrap_or(current);
                let r = TextEdit::singleline(&mut hex)
                    .id(edit_id)
                    .desired_width(90.0)
                    .char_limit(if alpha { 9 } else { 7 })
                    .ui(ui)
                    .on_hover_text(if alpha { "#RRGGBB or #RRGGBBAA" } else { "#RRGGBB" });
                if r.changed() {
                    if let Some(mut c) = parse_hex(&hex) {
                        if !alpha {
                            c[3] = 255;
                        }
                        if c != srgba {
                            out = Some(c);
                        }
                    }
                }
                ui.data_mut(|d| d.insert_temp(text_id, hex));
            }
        }
    });
    out
}

fn to_hex([r, g, b, a]: [u8; 4], alpha: bool) -> String {
    if alpha && a != 255 {
        format!("#{r:02X}{g:02X}{b:02X}{a:02X}")
    } else {
        format!("#{r:02X}{g:02X}{b:02X}")
    }
}

/// `#RRGGBB` (opaque) or `#RRGGBBAA`, with or without the `#`.
fn parse_hex(s: &str) -> Option<[u8; 4]> {
    let h = s.trim().trim_start_matches('#');
    if !h.is_ascii() || !(h.len() == 6 || h.len() == 8) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?, if h.len() == 8 { byte(6)? } else { 255 }])
}

fn swatch_button(ui: &mut Ui, srgba: [u8; 4], open: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(ui.spacing().interact_size, Sense::click());
    if ui.is_rect_visible(rect) {
        let visuals = if open { &ui.visuals().widgets.open } else { ui.style().interact(&response) };
        let rect = rect.expand(visuals.expansion);
        show_srgba(ui.painter(), srgba, rect.shrink(1.0));
        // Fill as the stroke is intentional: the default style has no border (as egui's button does).
        ui.painter().rect_stroke(rect, visuals.corner_radius.at_most(2), (1.0, visuals.bg_fill), StrokeKind::Inside);
    }
    response
}

/// A colour over checkers: the left half as it blends, the right half opaque.
fn show_srgba(painter: &Painter, [r, g, b, a]: [u8; 4], rect: Rect) {
    if a == 255 {
        painter.rect_filled(rect, 0.0, Color32::from_rgb(r, g, b));
    } else {
        checkers(painter, rect);
        let left = Rect::from_min_max(rect.left_top(), rect.center_bottom());
        let right = Rect::from_min_max(rect.center_top(), rect.right_bottom());
        painter.rect_filled(left, 0.0, Color32::from_rgba_unmultiplied(r, g, b, a));
        painter.rect_filled(right, 0.0, Color32::from_rgb(r, g, b));
    }
}

fn checkers(painter: &Painter, rect: Rect) {
    let rect = rect.shrink(0.5);
    if !rect.is_positive() {
        return;
    }
    let size = Vec2::splat(rect.height() / 2.0);
    let n = (rect.width() / size.x).round() as u32;
    let mut mesh = Mesh::default();
    mesh.add_colored_rect(rect, Color32::from_gray(32));
    for i in 0..n {
        let x = lerp(rect.left()..=rect.right(), i as f32 / n as f32);
        let y = if i % 2 == 0 { rect.top() } else { rect.center().y };
        mesh.add_colored_rect(Rect::from_min_size(pos2(x, y), size), Color32::from_gray(128));
    }
    painter.add(Shape::mesh(mesh));
}

fn contrast(c: Color32) -> Color32 {
    if Rgba::from(c).intensity() < 0.5 { Color32::WHITE } else { Color32::BLACK }
}

fn slider_1d(ui: &mut Ui, value: &mut f32, color_at: impl Fn(f32) -> Color32) -> Response {
    let size = vec2(ui.spacing().slider_width, ui.spacing().interact_size.y);
    let (rect, response) = ui.allocate_at_least(size, Sense::click_and_drag());
    if let Some(p) = response.interact_pointer_pos() {
        *value = remap_clamp(p.x, rect.left()..=rect.right(), 0.0..=1.0);
    }
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        checkers(ui.painter(), rect);
        let mut mesh = Mesh::default();
        for i in 0..=N {
            let t = i as f32 / N as f32;
            let c = color_at(t);
            let x = lerp(rect.left()..=rect.right(), t);
            mesh.colored_vertex(pos2(x, rect.top()), c);
            mesh.colored_vertex(pos2(x, rect.bottom()), c);
            if i < N {
                mesh.add_triangle(2 * i, 2 * i + 1, 2 * i + 2);
                mesh.add_triangle(2 * i + 1, 2 * i + 2, 2 * i + 3);
            }
        }
        ui.painter().add(Shape::mesh(mesh));
        ui.painter().rect_stroke(rect, 0.0, visuals.bg_stroke, StrokeKind::Inside);
        let x = lerp(rect.left()..=rect.right(), *value);
        let r = rect.height() / 4.0;
        let picked = color_at(*value);
        ui.painter().add(Shape::convex_polygon(
            vec![pos2(x, rect.center().y), pos2(x + r, rect.bottom()), pos2(x - r, rect.bottom())],
            picked,
            Stroke::new(visuals.fg_stroke.width, contrast(picked)),
        ));
    }
    response
}

/// Saturation across, value up.
fn slider_2d(ui: &mut Ui, x_value: &mut f32, y_value: &mut f32, color_at: impl Fn(f32, f32) -> Color32) -> Response {
    let (rect, response) = ui.allocate_at_least(Vec2::splat(ui.spacing().slider_width), Sense::click_and_drag());
    if let Some(p) = response.interact_pointer_pos() {
        *x_value = remap_clamp(p.x, rect.left()..=rect.right(), 0.0..=1.0);
        *y_value = remap_clamp(p.y, rect.bottom()..=rect.top(), 0.0..=1.0);
    }
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        let mut mesh = Mesh::default();
        for xi in 0..=N {
            for yi in 0..=N {
                let (xt, yt) = (xi as f32 / N as f32, yi as f32 / N as f32);
                let x = lerp(rect.left()..=rect.right(), xt);
                let y = lerp(rect.bottom()..=rect.top(), yt);
                mesh.colored_vertex(pos2(x, y), color_at(xt, yt));
                if xi < N && yi < N {
                    let tl = yi * (N + 1) + xi;
                    mesh.add_triangle(tl, tl + 1, tl + N + 1);
                    mesh.add_triangle(tl + 1, tl + N + 1, tl + N + 2);
                }
            }
        }
        ui.painter().add(Shape::mesh(mesh));
        ui.painter().rect_stroke(rect, 0.0, visuals.bg_stroke, StrokeKind::Inside);
        let picked = color_at(*x_value, *y_value);
        ui.painter().add(CircleShape {
            center: pos2(lerp(rect.left()..=rect.right(), *x_value), lerp(rect.bottom()..=rect.top(), *y_value)),
            radius: rect.width() / 12.0,
            fill: picked,
            stroke: Stroke::new(visuals.fg_stroke.width, contrast(picked)),
        });
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        assert_eq!(parse_hex("#1A2B3C"), Some([0x1A, 0x2B, 0x3C, 255]));
        assert_eq!(parse_hex("1a2b3c80"), Some([0x1A, 0x2B, 0x3C, 0x80]));
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("#GG0000"), None);
        assert_eq!(to_hex([0x1A, 0x2B, 0x3C, 255], true), "#1A2B3C");
        assert_eq!(to_hex([0x1A, 0x2B, 0x3C, 0x80], true), "#1A2B3C80");
        assert_eq!(to_hex([0x1A, 0x2B, 0x3C, 0x80], false), "#1A2B3C");
    }
}
