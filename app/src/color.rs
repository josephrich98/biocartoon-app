//! The colour button and its picker popup.
//!
//! egui's own picker can show values as F (0-1 linear) or U8 (0-255), but has no hex view, and its
//! alpha field could not be edited. This one cycles F → U8 → H (hex), and every field edits the same
//! colour the 2D area, hue bar and opacity box do, so they always agree. Its eyedropper takes a
//! colour from anywhere in the window. With transparency, the bar at the top is an opacity slider, drawn as the colour looks on white. Under the hue bar are the common colours and the ones
//! picked lately.

use egui::ecolor::{Hsva, HsvaGamma};
use egui::epaint::{CircleShape, Mesh};
use egui::{
    lerp, pos2, remap_clamp, vec2, Color32, DragValue, Id, Painter, Popup, PopupCloseBehavior, Rect, Response, Rgba,
    Sense, Shape, Stroke, StrokeKind, TextEdit, Ui, Vec2, Widget as _,
};

/// Vertices per dimension in the sliders; a multiple of 6 so the hue bar hits every peak hue.
const N: u32 = 6 * 6;
const SLIDER_WIDTH: f32 = 275.0;

/// The common colours, shown in every picker and beside the text colour fields.
pub const COMMON: [(&str, [u8; 3]); 11] = [
    ("White", [255, 255, 255]),
    ("Pink", [244, 143, 177]),
    ("Red", [220, 38, 38]),
    ("Orange", [249, 115, 22]),
    ("Yellow", [250, 204, 21]),
    ("Green", [22, 163, 74]),
    ("Cyan", [6, 182, 212]),
    ("Blue", [37, 99, 235]),
    ("Purple", [147, 51, 234]),
    ("Brown", [133, 77, 14]),
    ("Black", [0, 0, 0]),
];

/// How many recent colours are kept: one row's worth.
const RECENT_MAX: usize = COMMON.len();

fn recent_key() -> Id {
    Id::new("color_recent")
}

/// The colours picked lately, newest first; kept between sessions.
fn recent(ctx: &egui::Context) -> Vec<[u8; 4]> {
    ctx.data_mut(|d| d.get_persisted::<Vec<[u8; 4]>>(recent_key())).unwrap_or_default()
}

/// [`remember`], for the screenshot scripts.
pub fn remember_for_smoke(ctx: &egui::Context, c: [u8; 4]) {
    remember(ctx, c);
}

/// Arms an eyedropper, for the screenshot scripts (`BIOCARTOON_SCRIPT=dropper`).
pub fn arm_dropper_for_smoke(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(dropper_key(), Dropper::new(Id::new("smoke_dropper"))));
}

/// Puts `c` first among the recent colours.
fn remember(ctx: &egui::Context, c: [u8; 4]) {
    let mut list = recent(ctx);
    list.retain(|x| *x != c);
    list.insert(0, c);
    list.truncate(RECENT_MAX);
    ctx.data_mut(|d| d.insert_persisted(recent_key(), list));
}

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
    // Screenshot runs (`BIOCARTOON_SCRIPT=picker`) open the first picker drawn.
    if ui.data_mut(|d| d.remove_temp::<bool>(Id::new("smoke_open_picker"))).is_some() {
        Popup::open_id(ui.ctx(), popup_id);
    }
    let open = Popup::is_id_open(ui.ctx(), popup_id);
    let mut response = swatch_button(ui, *rgba, open);
    Popup::menu(&response)
        .id(popup_id)
        .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.spacing_mut().slider_width = SLIDER_WIDTH;
            if picker(ui, popup_id, rgba, alpha) {
                response.mark_changed();
                ui.data_mut(|d| d.insert_temp(popup_id.with("pending"), *rgba));
            }
        });
    // A colour the eyedropper picked for this button, as an opaque colour keeping the opacity.
    if let Some([r, g, b]) = ui.data_mut(|d| d.remove_temp::<[u8; 3]>(popup_id.with("dropped"))) {
        if [r, g, b] != [rgba[0], rgba[1], rgba[2]] {
            *rgba = [r, g, b, rgba[3]];
            response.mark_changed();
            ui.data_mut(|d| d.insert_temp(popup_id.with("pending"), *rgba));
        }
    }
    // Once the picker closes, the colour it was left on (if it was changed) becomes a recent one.
    if !Popup::is_id_open(ui.ctx(), popup_id) {
        if let Some(c) = ui.data_mut(|d| d.remove_temp::<[u8; 4]>(popup_id.with("pending"))) {
            remember(ui.ctx(), c);
        }
    }
    response
}

/// An armed eyedropper: the picker it returns a colour to, and the snapshot of the window it picks
/// from. The window can't change while the dropper covers it, so one snapshot serves the loupe and
/// the pick.
#[derive(Clone)]
struct Dropper {
    target: Id,
    /// Frames left before the snapshot is asked for, so the closed picker and any hover highlights
    /// are gone from it.
    settle: u8,
    /// Whether the snapshot has been asked for.
    asked: bool,
    shot: Option<Shot>,
}

#[derive(Clone)]
struct Shot {
    image: std::sync::Arc<egui::ColorImage>,
    texture: egui::TextureHandle,
}

impl Dropper {
    fn new(target: Id) -> Self {
        Self { target, settle: 2, asked: false, shot: None }
    }
}

/// Tags the eyedropper's screenshot, so other screenshots are left alone.
struct DropperShot;

fn dropper_key() -> Id {
    Id::new("color_dropper")
}

/// Pixels across the loupe, an odd number so one sits in the middle.
const LOUPE_PIXELS: i32 = 15;
/// Points per magnified pixel.
const LOUPE_CELL: f32 = 8.0;

/// Run once a frame, before the UI. While an eyedropper is armed it covers the window and shows a
/// loupe of the pixels under the pointer, like PowerPoint's: a click (or, on a touch screen, lifting
/// the finger) picks the middle one and reopens the picker with it; Esc or a right click cancels.
pub fn dropper_frame(ctx: &egui::Context) {
    let Some(mut d) = ctx.data(|m| m.get_temp::<Dropper>(dropper_key())) else { return };
    let target = d.target;
    let finish = |picked: Option<[u8; 3]>| {
        ctx.data_mut(|m| {
            m.remove::<Dropper>(dropper_key());
            if let Some(c) = picked {
                m.insert_temp(target.with("dropped"), c);
            }
        });
        Popup::open_id(ctx, target);
        ctx.request_repaint();
    };
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        finish(None);
        return;
    }
    if d.shot.is_none() {
        let img = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, user_data, .. }
                    if user_data.data.as_ref().is_some_and(|u| u.downcast_ref::<DropperShot>().is_some()) =>
                {
                    Some(image.clone())
                }
                _ => None,
            })
        });
        if let Some(image) = img {
            let texture = ctx.load_texture("color_dropper_shot", (*image).clone(), egui::TextureOptions::NEAREST);
            d.shot = Some(Shot { image, texture });
        } else if d.settle > 0 {
            d.settle -= 1;
        } else if !d.asked {
            d.asked = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(DropperShot)));
        }
        ctx.data_mut(|m| m.insert_temp(dropper_key(), d.clone()));
    }
    ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
    let screen = ctx.content_rect();
    let ppp = ctx.pixels_per_point();
    egui::Area::new(Id::new("color_dropper_overlay"))
        .order(egui::Order::Tooltip)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            let r = ui.allocate_response(screen.size(), Sense::click_and_drag());
            let Some(shot) = &d.shot else {
                return; // waiting for the snapshot; draw nothing that would land in it
            };
            if r.secondary_clicked() {
                finish(None);
                return;
            }
            // The physical pixel under a point, kept inside the snapshot.
            let [w, h] = shot.image.size;
            let pixel_at = |p: egui::Pos2| {
                let x = ((p.x * ppp).floor().max(0.0) as usize).min(w.saturating_sub(1));
                let y = ((p.y * ppp).floor().max(0.0) as usize).min(h.saturating_sub(1));
                (x, y)
            };
            let color_at = |(x, y): (usize, usize)| shot.image.pixels.get(y * w + x).copied().unwrap_or(Color32::BLACK);
            if r.clicked() || r.drag_stopped() {
                if let Some(p) = r.interact_pointer_pos().or_else(|| ui.ctx().pointer_latest_pos()) {
                    let c = color_at(pixel_at(p));
                    finish(Some([c.r(), c.g(), c.b()]));
                    return;
                }
            }
            let Some(p) = r.hover_pos().or_else(|| r.interact_pointer_pos()) else { return };
            let touch = ui.input(|i| i.any_touches());
            let (cx, cy) = pixel_at(p);
            let picked = color_at((cx, cy));
            draw_loupe(ui, screen, p, touch, shot, (cx, cy), picked);
        });
    ctx.request_repaint();
}

/// The loupe beside the pointer: the pixels around it magnified, the middle one outlined, and its
/// colour as a swatch with its hex and RGB values. On a touch screen it sits above the finger.
fn draw_loupe(ui: &Ui, screen: Rect, p: egui::Pos2, touch: bool, shot: &Shot, (cx, cy): (usize, usize), picked: Color32) {
    let painter = ui.painter();
    let visuals = ui.visuals();
    let side = LOUPE_PIXELS as f32 * LOUPE_CELL;
    let font = egui::FontId::monospace(12.0);
    let text = format!("{}  RGB({}, {}, {})", to_hex([picked.r(), picked.g(), picked.b(), 255], false), picked.r(), picked.g(), picked.b());
    let galley = painter.layout_no_wrap(text, font, visuals.strong_text_color());
    let hint = painter.layout_no_wrap(
        if touch { "Lift to pick · Esc to cancel" } else { "Click to pick · Esc to cancel" }.into(),
        egui::FontId::proportional(11.0),
        visuals.weak_text_color(),
    );
    let pad = 6.0;
    let swatch = galley.size().y;
    let width = side.max(swatch + 6.0 + galley.size().x).max(hint.size().x);
    let size = vec2(width, side + 6.0 + galley.size().y + 4.0 + hint.size().y) + Vec2::splat(2.0 * pad);

    // Below and to the right of the pointer, flipped at the window's edges; above a finger.
    let gap = 20.0;
    let mut min = if touch { pos2(p.x - size.x / 2.0, p.y - 60.0 - size.y) } else { p + vec2(gap, gap) };
    if min.x + size.x > screen.right() {
        min.x = if touch { screen.right() - size.x } else { p.x - gap - size.x };
    }
    if min.y + size.y > screen.bottom() {
        min.y = p.y - gap - size.y;
    }
    min = min.max(screen.min);
    let frame = Rect::from_min_size(min, size);
    painter.rect_filled(frame.expand(1.0), 6.0, visuals.window_shadow.color);
    painter.rect_filled(frame, 6.0, visuals.window_fill);
    painter.rect_stroke(frame, 6.0, visuals.window_stroke, StrokeKind::Inside);

    // The magnified pixels; where the loupe runs past the window's edge, the backdrop shows.
    let x0 = frame.left() + pad + (width - side) / 2.0;
    let mag = Rect::from_min_size(pos2(x0, frame.top() + pad), Vec2::splat(side));
    painter.rect_filled(mag, 0.0, Color32::from_gray(40));
    let [w, h] = shot.image.size;
    let half = LOUPE_PIXELS / 2;
    let (left, top) = (cx as i32 - half, cy as i32 - half);
    let (vx0, vy0) = (left.max(0), top.max(0));
    let (vx1, vy1) = ((left + LOUPE_PIXELS).min(w as i32), (top + LOUPE_PIXELS).min(h as i32));
    if vx1 > vx0 && vy1 > vy0 {
        let cell = |x: i32, y: i32| mag.min + vec2((x - left) as f32, (y - top) as f32) * LOUPE_CELL;
        let rect = Rect::from_min_max(cell(vx0, vy0), cell(vx1, vy1));
        let uv = Rect::from_min_max(
            pos2(vx0 as f32 / w as f32, vy0 as f32 / h as f32),
            pos2(vx1 as f32 / w as f32, vy1 as f32 / h as f32),
        );
        painter.image(shot.texture.id(), rect, uv, Color32::WHITE);
    }
    let middle = Rect::from_min_size(mag.min + Vec2::splat(half as f32 * LOUPE_CELL), Vec2::splat(LOUPE_CELL));
    painter.rect_stroke(middle.expand(1.0), 0.0, Stroke::new(1.0, contrast(picked)), StrokeKind::Inside);
    painter.rect_stroke(mag, 0.0, visuals.window_stroke, StrokeKind::Outside);

    // The colour it will pick.
    let row_y = mag.bottom() + 6.0;
    let sw = Rect::from_min_size(pos2(frame.left() + pad, row_y), Vec2::splat(swatch));
    painter.rect_filled(sw, 2.0, picked);
    painter.rect_stroke(sw, 2.0, visuals.window_stroke, StrokeKind::Inside);
    let hint_y = row_y + galley.size().y + 4.0;
    painter.galley(pos2(sw.right() + 6.0, row_y), galley, visuals.strong_text_color());
    painter.galley(pos2(frame.left() + pad, hint_y), hint, visuals.weak_text_color());
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

        if alpha {
            // The colour from clear to solid as it looks on a white page: drag for its opacity.
            let [r, g, b, a] = *rgba;
            let mut t = a as f32 / 255.0;
            let on_white = |t: f32| {
                let mix = |c: u8| (255.0 + (c as f32 - 255.0) * t).round() as u8;
                Color32::from_rgb(mix(r), mix(g), mix(b))
            };
            slider_1d(ui, &mut t, on_white).on_hover_text("Opacity (as it looks on white)");
            let a2 = (t * 255.0).round() as u8;
            if a2 != a {
                rgba[3] = a2;
                hsvag.a = t;
            }
        } else {
            let size = vec2(ui.spacing().slider_width, ui.spacing().interact_size.y);
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            show_srgba(ui.painter(), *rgba, rect);
        }

        let opaque = HsvaGamma { a: 1.0, ..hsvag };
        let hsv = hsvag;
        let HsvaGamma { h, s, v, .. } = &mut hsvag;
        slider_2d(ui, s, v, |s, v| HsvaGamma { s, v, ..opaque }.into());
        slider_1d(ui, h, |h| HsvaGamma { h, s: 1.0, v: 1.0, a: 1.0 }.into()).on_hover_text("Hue");
        if hsvag != hsv {
            let [r, g, b, _] = Hsva::from(hsvag).to_srgba_unmultiplied();
            *rgba = [r, g, b, rgba[3]];
        }

        if let Some(c) = swatch_rows(ui, *rgba, alpha) {
            *rgba = c;
            hsvag = HsvaGamma::from(Hsva::from_srgba_unmultiplied(c));
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

/// The common colours, then the recent ones (when there are any). Returns the one clicked.
fn swatch_rows(ui: &mut Ui, current: [u8; 4], alpha: bool) -> Option<[u8; 4]> {
    let mut picked = None;
    // As wide as the gradient: one row of `COMMON.len()` squares, 4 apart.
    let n = COMMON.len() as f32;
    let size = ((ui.spacing().slider_width - 4.0 * (n - 1.0)) / n).floor();
    let caption = |ui: &mut Ui, text: &str| {
        ui.add_space(2.0);
        ui.label(egui::RichText::new(text).small().color(crate::theme::text_dim()));
    };
    caption(ui, "Common");
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(4.0);
        for (name, [r, g, b]) in COMMON {
            if swatch(ui, size, [r, g, b, 255], current[..3] == [r, g, b]).on_hover_text(name).clicked() {
                // Keep the opacity, unless it is clear and the colour would not show.
                picked = Some([r, g, b, if current[3] == 0 { 255 } else { current[3] }]);
            }
        }
    });
    let recent = recent(ui.ctx());
    if !recent.is_empty() {
        caption(ui, "Recent");
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(4.0);
            for c in recent {
                let c = if alpha { c } else { [c[0], c[1], c[2], 255] };
                if swatch(ui, size, c, current == c).on_hover_text(to_hex(c, alpha)).clicked() {
                    picked = Some(c);
                }
            }
        });
    }
    ui.add_space(2.0);
    picked
}

/// One clickable colour square, ringed when it is the current colour.
fn swatch(ui: &mut Ui, size: f32, c: [u8; 4], on: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    if ui.is_rect_visible(rect) {
        show_srgba(ui.painter(), c, rect.shrink(1.0));
        let ring = if on {
            Stroke::new(2.0, crate::theme::accent())
        } else if resp.hovered() {
            Stroke::new(1.5, crate::theme::text())
        } else {
            Stroke::new(1.0, crate::theme::border())
        };
        ui.painter().rect_stroke(rect, 3.0, ring, StrokeKind::Inside);
    }
    resp
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
        if ui.button(egui_phosphor::regular::EYEDROPPER).on_hover_text("Pick a color from the screen").clicked() {
            ui.data_mut(|d| d.insert_temp(dropper_key(), Dropper::new(id)));
            // Out of the way, so it doesn't hide what is being picked; it reopens with the colour.
            Popup::close_id(ui.ctx(), id);
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

    /// Arming asks for one snapshot once the picker is gone; a click then picks that pixel from it
    /// and the picker reopens.
    #[test]
    fn dropper_picks_the_clicked_pixel() {
        let ctx = egui::Context::default();
        let target = Id::new("picker");
        ctx.data_mut(|d| d.insert_temp(dropper_key(), Dropper::new(target)));
        let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(200.0, 100.0));
        let frame = |events: Vec<egui::Event>| -> bool {
            let input = egui::RawInput { screen_rect: Some(screen), events, ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| dropper_frame(ui.ctx()));
            out.textures_delta.clear();
            let cmds = out.viewport_output.values().flat_map(|v| v.commands.iter());
            cmds.into_iter().any(|c| matches!(c, egui::ViewportCommand::Screenshot(_)))
        };
        let asks: Vec<bool> = (0..5).map(|_| frame(vec![])).collect();
        assert_eq!(asks.iter().filter(|a| **a).count(), 1, "one snapshot is asked for: {asks:?}");
        assert!(!asks[0], "not before the picker has gone");

        let mut img = egui::ColorImage::filled([200, 100], Color32::WHITE);
        img.pixels[60 * 200 + 50] = Color32::from_rgb(10, 20, 30);
        let image = std::sync::Arc::new(img);
        let other = egui::Event::Screenshot { viewport_id: egui::ViewportId::ROOT, user_data: egui::UserData::new(String::from("smoke")), image: image.clone() };
        frame(vec![other]);
        assert!(ctx.data(|d| d.get_temp::<Dropper>(dropper_key())).is_some_and(|d| d.shot.is_none()), "another screenshot is ignored");
        frame(vec![egui::Event::Screenshot { viewport_id: egui::ViewportId::ROOT, user_data: egui::UserData::new(DropperShot), image }]);
        assert!(ctx.data(|d| d.get_temp::<Dropper>(dropper_key())).is_some_and(|d| d.shot.is_some()));

        let at = egui::pos2(50.5, 60.5);
        let press = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(vec![egui::Event::PointerMoved(at)]);
        assert!(ctx.data(|d| d.get_temp::<[u8; 3]>(target.with("dropped"))).is_none(), "hovering shows the loupe but picks nothing");
        frame(vec![egui::Event::PointerMoved(at), press(true)]);
        frame(vec![press(false)]);
        assert_eq!(ctx.data(|d| d.get_temp::<[u8; 3]>(target.with("dropped"))), Some([10, 20, 30]));
        assert!(ctx.data(|d| d.get_temp::<Dropper>(dropper_key())).is_none());
        assert!(Popup::is_id_open(&ctx, target));
    }

    #[test]
    fn dropper_escape_cancels() {
        let ctx = egui::Context::default();
        let target = Id::new("picker");
        ctx.data_mut(|d| d.insert_temp(dropper_key(), Dropper::new(target)));
        let key = egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() };
        let mut out = ctx.run_ui(egui::RawInput { events: vec![key], ..Default::default() }, |ui| dropper_frame(ui.ctx()));
        out.textures_delta.clear();
        assert!(ctx.data(|d| d.get_temp::<Dropper>(dropper_key())).is_none());
        assert!(ctx.data(|d| d.get_temp::<[u8; 3]>(target.with("dropped"))).is_none());
    }

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
