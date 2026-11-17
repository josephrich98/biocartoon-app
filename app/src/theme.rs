//! Colour themes (BioCartoon's own, orange, and editor themes from VS Code and RStudio) and egui visuals.

use egui::{Color32, CornerRadius, Stroke, Visuals};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};

/// Light, dark, or whatever the operating system uses.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeMode {
    pub const ALL: [ThemeMode; 3] = [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark];
    pub fn label(self) -> &'static str {
        match self {
            ThemeMode::System => "Match system",
            ThemeMode::Light => "Light",
            ThemeMode::Dark => "Dark",
        }
    }
    /// Whether this mode is dark, given the operating system's theme.
    pub fn is_dark(self, ctx: &egui::Context) -> bool {
        match self {
            ThemeMode::System => ctx.system_theme().unwrap_or(egui::Theme::Dark) == egui::Theme::Dark,
            ThemeMode::Light => false,
            ThemeMode::Dark => true,
        }
    }
}

/// Size of the interface text. Saved under names of its own: the sizes were renamed when Extra
/// large was added, and a size saved under its old name keeps its scale (the old default, Medium,
/// becomes the new default).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum TextSize {
    #[serde(rename = "XS", alias = "Small")]
    ExtraSmall,
    #[serde(rename = "S")]
    Small,
    #[default]
    #[serde(rename = "M", alias = "Medium", alias = "Large")]
    Medium,
    #[serde(rename = "L", alias = "ExtraLarge")]
    Large,
    #[serde(rename = "XL")]
    ExtraLarge,
    #[serde(rename = "XXL")]
    ExtraExtraLarge,
}

impl TextSize {
    pub const ALL: [TextSize; 6] = [TextSize::ExtraSmall, TextSize::Small, TextSize::Medium, TextSize::Large, TextSize::ExtraLarge, TextSize::ExtraExtraLarge];
    pub fn label(self) -> &'static str {
        match self {
            TextSize::ExtraSmall => "Extra small",
            TextSize::Small => "Small",
            TextSize::Medium => "Medium (default)",
            TextSize::Large => "Large",
            TextSize::ExtraLarge => "Extra large",
            TextSize::ExtraExtraLarge => "Extra extra large",
        }
    }
    pub fn scale(self) -> f32 {
        match self {
            TextSize::ExtraSmall => 0.9,
            TextSize::Small => 1.0,
            TextSize::Medium => 1.15,
            TextSize::Large => 1.3,
            TextSize::ExtraLarge => 1.45,
            TextSize::ExtraExtraLarge => 1.6,
        }
    }
}

/// The colours of a theme. Every interface colour comes from the current one.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    /// Behind everything: the home screen, the gaps between panels.
    pub bg: Color32,
    pub panel: Color32,
    /// Dialogs (dark themes), menus, hovered rows.
    pub raised: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub accent: Color32,
    /// Selection and pressed buttons.
    pub accent_dim: Color32,
    /// Around the page on the canvas.
    pub canvas: Color32,
    pub hover: Color32,
    pub toast: Color32,
    pub danger: Color32,
    /// Text fields.
    pub field: Color32,
    /// Checkbox and radio boxes, slider rails, scrollbar handles.
    pub widget: Color32,
}

const fn c(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

#[allow(clippy::too_many_arguments)]
const fn pal(bg: u32, panel: u32, raised: u32, border: u32, text: u32, text_dim: u32, accent: u32, accent_dim: u32, canvas: u32, hover: u32, toast: u32, danger: u32, field: u32, widget: u32) -> Palette {
    Palette {
        bg: c(bg), panel: c(panel), raised: c(raised), border: c(border), text: c(text), text_dim: c(text_dim), accent: c(accent), accent_dim: c(accent_dim),
        canvas: c(canvas), hover: c(hover), toast: c(toast), danger: c(danger), field: c(field), widget: c(widget),
    }
}

/// A colour theme, after the editor themes of VS Code and RStudio. Each is dark or light;
/// Appearance picks which of the two chosen themes (one dark, one light) is shown.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ColorTheme {
    // Dark
    BioCartoonDark,
    DarkModern,
    Monokai,
    Dracula,
    Nord,
    OneDark,
    SolarizedDark,
    TomorrowNight,
    Cobalt,
    GruvboxDark,
    CatppuccinMocha,
    TokyoNight,
    HighContrastDark,
    // Light
    BioCartoonLight,
    LightModern,
    GitHubLight,
    SolarizedLight,
    QuietLight,
    OneLight,
    Tomorrow,
    GruvboxLight,
    CatppuccinLatte,
    HighContrastLight,
}

impl ColorTheme {
    pub const ALL: [ColorTheme; 23] = [
        ColorTheme::BioCartoonDark, ColorTheme::DarkModern, ColorTheme::Monokai, ColorTheme::Dracula, ColorTheme::Nord, ColorTheme::OneDark,
        ColorTheme::SolarizedDark, ColorTheme::TomorrowNight, ColorTheme::Cobalt, ColorTheme::GruvboxDark, ColorTheme::CatppuccinMocha,
        ColorTheme::TokyoNight, ColorTheme::HighContrastDark,
        ColorTheme::BioCartoonLight, ColorTheme::LightModern, ColorTheme::GitHubLight, ColorTheme::SolarizedLight, ColorTheme::QuietLight,
        ColorTheme::OneLight, ColorTheme::Tomorrow, ColorTheme::GruvboxLight, ColorTheme::CatppuccinLatte, ColorTheme::HighContrastLight,
    ];
    pub const DEFAULT_DARK: ColorTheme = ColorTheme::BioCartoonDark;
    pub const DEFAULT_LIGHT: ColorTheme = ColorTheme::BioCartoonLight;

    /// The dark themes, or the light ones.
    pub fn of(dark: bool) -> impl Iterator<Item = ColorTheme> {
        Self::ALL.into_iter().filter(move |t| t.is_dark() == dark)
    }
    pub fn is_dark(self) -> bool {
        (self as usize) < (ColorTheme::BioCartoonLight as usize)
    }
    pub fn label(self) -> &'static str {
        match self {
            ColorTheme::BioCartoonDark => "BioCartoon Dark (default)",
            ColorTheme::DarkModern => "Dark Modern",
            ColorTheme::Monokai => "Monokai",
            ColorTheme::Dracula => "Dracula",
            ColorTheme::Nord => "Nord",
            ColorTheme::OneDark => "One Dark",
            ColorTheme::SolarizedDark => "Solarized Dark",
            ColorTheme::TomorrowNight => "Tomorrow Night",
            ColorTheme::Cobalt => "Cobalt",
            ColorTheme::GruvboxDark => "Gruvbox Dark",
            ColorTheme::CatppuccinMocha => "Catppuccin Mocha",
            ColorTheme::TokyoNight => "Tokyo Night",
            ColorTheme::HighContrastDark => "High Contrast Dark",
            ColorTheme::BioCartoonLight => "BioCartoon Light (default)",
            ColorTheme::LightModern => "Light Modern",
            ColorTheme::GitHubLight => "GitHub Light",
            ColorTheme::SolarizedLight => "Solarized Light",
            ColorTheme::QuietLight => "Quiet Light",
            ColorTheme::OneLight => "One Light",
            ColorTheme::Tomorrow => "Tomorrow",
            ColorTheme::GruvboxLight => "Gruvbox Light",
            ColorTheme::CatppuccinLatte => "Catppuccin Latte",
            ColorTheme::HighContrastLight => "High Contrast Light",
        }
    }
    /// A short name for `BIOCARTOON_SCRIPT=theme=<slug>`: the label in lower case without spaces.
    pub fn slug(self) -> String {
        self.label().split(" (").next().unwrap_or_default().to_lowercase().replace(' ', "")
    }
    pub const fn palette(self) -> &'static Palette {
        &PALETTES[self as usize]
    }
}

/// In the order of [`ColorTheme`]. Fields: bg, panel, raised, border, text, text_dim, accent,
/// accent_dim, canvas, hover, toast, danger, field, widget.
static PALETTES: [Palette; 23] = [
    // BioCartoon Dark
    pal(0x1b1b1b, 0x252527, 0x2d2d30, 0x3a3a3e, 0xe8e8ea, 0xa0a0a8, 0xff6c0c, 0x8c3d0a, 0x1e1e20, 0x3a3a40, 0x2a2a30, 0xff6b6b, 0x1a1a1c, 0x4a4a50),
    // Dark Modern (VS Code)
    pal(0x181818, 0x1f1f1f, 0x2b2b2b, 0x3c3c3c, 0xcccccc, 0x9d9d9d, 0x0078d4, 0x04395e, 0x181818, 0x37373d, 0x252526, 0xf85149, 0x313131, 0x4a4a4a),
    // Monokai
    pal(0x1e1f1c, 0x272822, 0x3e3d32, 0x49483e, 0xf8f8f2, 0x90908a, 0xf92672, 0x6b1a35, 0x1e1f1c, 0x49483e, 0x3e3d32, 0xfd971f, 0x1e1f1c, 0x5b5a4e),
    // Dracula
    pal(0x21222c, 0x282a36, 0x343746, 0x44475a, 0xf8f8f2, 0xa0a4c0, 0xbd93f9, 0x4d3d6e, 0x1e1f29, 0x44475a, 0x343746, 0xff5555, 0x21222c, 0x565970),
    // Nord
    pal(0x2e3440, 0x3b4252, 0x434c5e, 0x4c566a, 0xeceff4, 0xa3abbd, 0x88c0d0, 0x3f5f6e, 0x2b303b, 0x4c566a, 0x434c5e, 0xe5757e, 0x2e3440, 0x5a657c),
    // One Dark
    pal(0x21252b, 0x282c34, 0x2c313a, 0x3e4451, 0xd7dae0, 0x8b919c, 0x61afef, 0x2c4a66, 0x1e2227, 0x3a3f4b, 0x2c313a, 0xe06c75, 0x1d2025, 0x4b5263),
    // Solarized Dark
    pal(0x00212b, 0x002b36, 0x073642, 0x0e4a59, 0xeee8d5, 0x93a1a1, 0x268bd2, 0x0d4a6b, 0x00252f, 0x0e4a59, 0x073642, 0xf05a4f, 0x001e26, 0x2a5a66),
    // Tomorrow Night
    pal(0x1d1f21, 0x232528, 0x282a2e, 0x373b41, 0xe0e2e0, 0x969896, 0x81a2be, 0x3a4c5c, 0x1a1b1d, 0x373b41, 0x282a2e, 0xe07a7a, 0x1a1b1d, 0x4b5058),
    // Cobalt (RStudio)
    pal(0x001b33, 0x002240, 0x0b3358, 0x1f4662, 0xffffff, 0x9fb5c9, 0xffc600, 0x6b5500, 0x00172b, 0x1f4662, 0x0b3358, 0xff628c, 0x001629, 0x2d5a80),
    // Gruvbox Dark
    pal(0x1d2021, 0x282828, 0x32302f, 0x504945, 0xebdbb2, 0xa89984, 0xfabd2f, 0x6b5012, 0x1d2021, 0x3c3836, 0x32302f, 0xfb4934, 0x1d2021, 0x665c54),
    // Catppuccin Mocha
    pal(0x181825, 0x1e1e2e, 0x313244, 0x45475a, 0xcdd6f4, 0xa6adc8, 0xcba6f7, 0x54436e, 0x11111b, 0x45475a, 0x313244, 0xf38ba8, 0x181825, 0x585b70),
    // Tokyo Night
    pal(0x16161e, 0x1a1b26, 0x24283b, 0x2f3549, 0xc0caf5, 0x8c93b8, 0x7aa2f7, 0x2e3f66, 0x13131a, 0x292e42, 0x24283b, 0xf7768e, 0x16161e, 0x414868),
    // High Contrast Dark
    pal(0x000000, 0x000000, 0x0c0c0c, 0x6fc3df, 0xffffff, 0xd0d0d0, 0xf38518, 0x7a3d00, 0x000000, 0x333333, 0x0c0c0c, 0xff6b6b, 0x000000, 0x6b6b6b),
    // BioCartoon Light
    pal(0xececef, 0xffffff, 0xf2f2f5, 0xd6d6dc, 0x1d1d20, 0x6b6b74, 0xff6c0c, 0xffd4b5, 0xe2e2e6, 0xe2e2e8, 0xffffff, 0xc62828, 0xf7f7f9, 0xf2f2f5),
    // Light Modern (VS Code)
    pal(0xf8f8f8, 0xffffff, 0xf8f8f8, 0xd4d4d4, 0x1f1f1f, 0x616161, 0x005fb8, 0xcce3f7, 0xeeeeee, 0xe8e8e8, 0xffffff, 0xc72e0f, 0xf8f8f8, 0xf0f0f0),
    // GitHub Light
    pal(0xf6f8fa, 0xffffff, 0xf6f8fa, 0xd0d7de, 0x1f2328, 0x656d76, 0x0969da, 0xddf4ff, 0xeaeef2, 0xeaeef2, 0xffffff, 0xcf222e, 0xf6f8fa, 0xe6eaef),
    // Solarized Light
    pal(0xeee8d5, 0xfdf6e3, 0xf5efdc, 0xd9d2bc, 0x073642, 0x657b83, 0x268bd2, 0xd0e3ea, 0xe6dfca, 0xe8e1cc, 0xfdf6e3, 0xdc322f, 0xfbf4e0, 0xe3dcc6),
    // Quiet Light
    pal(0xececec, 0xf5f5f5, 0xededf2, 0xd4d4d4, 0x333333, 0x6e6e6e, 0x705697, 0xe0d8ee, 0xe4e4e4, 0xe4e6f1, 0xf5f5f5, 0xc4302b, 0xffffff, 0xe0e0e0),
    // One Light
    pal(0xeaeaeb, 0xfafafa, 0xf0f0f1, 0xd3d3d5, 0x383a42, 0x696c77, 0x4078f2, 0xd4e1fc, 0xe5e5e6, 0xe5e5e6, 0xfafafa, 0xe45649, 0xffffff, 0xf0f0f1),
    // Tomorrow
    pal(0xefefef, 0xffffff, 0xf5f5f5, 0xd6d6d6, 0x4d4d4c, 0x6f716d, 0x4271ae, 0xd3dfef, 0xe8e8e8, 0xe8e8e8, 0xffffff, 0xc82829, 0xfafafa, 0xf5f5f5),
    // Gruvbox Light
    pal(0xebdbb2, 0xfbf1c7, 0xf2e5bc, 0xd5c4a1, 0x3c3836, 0x7c6f64, 0x076678, 0xc6dccf, 0xe5d5ab, 0xebdbb2, 0xfbf1c7, 0x9d0006, 0xf9f5d7, 0xf2e5bc),
    // Catppuccin Latte
    pal(0xdce0e8, 0xeff1f5, 0xe6e9ef, 0xbcc0cc, 0x4c4f69, 0x6c6f85, 0x8839ef, 0xe1d1fb, 0xdce0e8, 0xccd0da, 0xeff1f5, 0xd20f39, 0xe6e9ef, 0xe6e9ef),
    // High Contrast Light
    pal(0xffffff, 0xffffff, 0xffffff, 0x0f4a85, 0x000000, 0x292929, 0x0f4a85, 0xcfe0f5, 0xf0f0f0, 0xe0e0e0, 0xffffff, 0xb5200d, 0xffffff, 0xd8d8d8),
];

/// The theme shown, as its index in [`ColorTheme::ALL`].
static THEME: AtomicU8 = AtomicU8::new(ColorTheme::DEFAULT_DARK as u8);

pub fn current() -> ColorTheme {
    ColorTheme::ALL[THEME.load(Ordering::Relaxed) as usize]
}

fn p() -> &'static Palette {
    current().palette()
}

pub fn is_dark() -> bool {
    current().is_dark()
}

/// The text size's scale, as f32 bits.
static TEXT_SCALE: AtomicU32 = AtomicU32::new(0x3f80_0000);

/// The text size's scale, for what is drawn at a fixed size (the rulers, the rail).
pub fn text_scale() -> f32 {
    f32::from_bits(TEXT_SCALE.load(Ordering::Relaxed))
}

/// Enter, with no modifier keys, to press a dialog's orange button. Only while no text field or
/// button has keyboard focus: a multi-line field keeps Enter for new lines, a button tabbed to
/// presses itself, and a one-line field gives up focus on Enter, so Enter there submits. The key
/// is consumed, so nothing else answers it too.
pub fn enter_pressed(ui: &egui::Ui) -> bool {
    ui.memory(|m| m.focused().is_none()) && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
}

/// How long a button pressed with Enter is shown pressed before it acts.
const ENTER_PRESS_SECS: f64 = 0.15;

/// Draw a dialog's orange button with `add`, and press it on Enter ([`enter_pressed`]) when `enter`
/// holds. The button looks pressed, as it does under a click, for a moment before the press
/// counts, so there is something to see before the dialog closes or a file picker opens. Returns
/// what `add` returns and whether an Enter press has just finished.
pub fn press_on_enter<R>(ui: &mut egui::Ui, enter: bool, add: impl FnOnce(&mut egui::Ui) -> R) -> (R, bool) {
    let key = ui.id().with("enter_press");
    let now = ui.input(|i| i.time);
    let frame = ui.ctx().cumulative_frame_nr();
    // (when Enter went down, the last frame drawn pressed). A press from a dialog that closed
    // mid-press is stale and dropped, so it doesn't fire when the dialog next opens.
    let pressing = ui.data(|d| d.get_temp::<(f64, u64)>(key)).filter(|&(_, seen)| seen + 1 >= frame);
    let out = if pressing.is_some() {
        let saved = ui.visuals().widgets.clone();
        let w = &mut ui.visuals_mut().widgets;
        w.inactive = w.active;
        w.hovered = w.active;
        let out = add(ui);
        ui.visuals_mut().widgets = saved;
        out
    } else {
        add(ui)
    };
    let fired = match pressing {
        Some((since, _)) if now - since >= ENTER_PRESS_SECS => {
            ui.data_mut(|d| d.remove::<(f64, u64)>(key));
            true
        }
        Some((since, _)) => {
            ui.data_mut(|d| d.insert_temp(key, (since, frame)));
            ui.ctx().request_repaint();
            false
        }
        None => {
            if enter && enter_pressed(ui) {
                ui.data_mut(|d| d.insert_temp(key, (now, frame)));
                ui.ctx().request_repaint();
            } else {
                ui.data_mut(|d| d.remove::<(f64, u64)>(key));
            }
            false
        }
    };
    (out, fired)
}

pub fn bg() -> Color32 { p().bg }
pub fn panel() -> Color32 { p().panel }
pub fn panel_raised() -> Color32 { p().raised }
pub fn border() -> Color32 { p().border }
pub fn text() -> Color32 { p().text }
pub fn text_dim() -> Color32 { p().text_dim }
pub fn accent() -> Color32 { p().accent }
pub fn accent_dim() -> Color32 { p().accent_dim }
/// Text drawn on an [`accent`] fill (the main button of a dialog): always white on BioCartoon's
/// own orange; elsewhere white, or near-black on a light accent (Nord's frost, Cobalt's yellow).
pub fn on_accent() -> Color32 {
    match current() {
        ColorTheme::BioCartoonDark | ColorTheme::BioCartoonLight => Color32::WHITE,
        _ => readable_on(accent()),
    }
}
/// Text drawn on an [`accent_dim`] fill.
pub fn on_accent_dim() -> Color32 { readable_on(accent_dim()) }
pub fn canvas_bg() -> Color32 { p().canvas }
pub fn handle() -> Color32 { accent() }
/// Fill of a hovered button or tile.
pub fn hover() -> Color32 { p().hover }
/// Fill of an informational toast.
pub fn toast() -> Color32 { p().toast }
/// Text saying something is wrong (an image too large to submit).
pub fn danger() -> Color32 { p().danger }
/// Fill of a banner saying something worked (white text on it).
pub fn success() -> Color32 { Color32::from_rgb(0x2e, 0x8b, 0x4a) }

/// White, or near-black when white would be hard to read on `fill`. Prefers white: it is the
/// one used unless the fill is light enough that black contrasts clearly better.
fn readable_on(fill: Color32) -> Color32 {
    let lin = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    let l = 0.2126 * lin(fill.r()) + 0.7152 * lin(fill.g()) + 0.0722 * lin(fill.b());
    let (white, black) = (1.05 / (l + 0.05), (l + 0.05) / 0.05);
    if black > 1.6 * white { Color32::from_rgb(0x1d, 0x1d, 0x20) } else { Color32::WHITE }
}

/// A small swatch of a theme (its background, panel, text and accent), for the theme pickers.
pub fn swatch(ui: &mut egui::Ui, t: ColorTheme) -> egui::Response {
    let pal = t.palette();
    let h = ui.spacing().interact_size.y - 6.0;
    let (rect, r) = ui.allocate_exact_size(egui::vec2(h * 2.0, h), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect(rect, 3.0, pal.bg, Stroke::new(1.0, border()), egui::StrokeKind::Outside);
    let inner = rect.shrink(2.0);
    let w = inner.width() / 3.0;
    for (i, col) in [pal.panel, pal.text, pal.accent].into_iter().enumerate() {
        let x = inner.min.x + w * i as f32;
        painter.rect_filled(egui::Rect::from_min_max(egui::pos2(x, inner.min.y), egui::pos2(x + w, inner.max.y)), 2.0, col);
    }
    r
}

/// Sets the theme and the egui visuals; `text_scale` multiplies egui's default text sizes.
pub fn apply(ctx: &egui::Context, t: ColorTheme, text_scale: f32) {
    THEME.store(t as u8, Ordering::Relaxed);
    let dark = t.is_dark();
    TEXT_SCALE.store(text_scale.to_bits(), Ordering::Relaxed);
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = panel();
    v.window_fill = if dark { panel_raised() } else { panel() };
    v.extreme_bg_color = p().field;
    v.faint_bg_color = panel_raised();
    v.override_text_color = Some(text());
    v.window_stroke = Stroke::new(1.0, border());
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(8);
    v.selection.bg_fill = accent_dim();
    v.selection.stroke = Stroke::new(1.0, accent());
    v.hyperlink_color = accent();
    v.widgets.noninteractive.bg_fill = panel();
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, border());
    // Checkbox and radio boxes, slider rails and scrollbar handles (buttons use `weak_bg_fill`):
    // lighter than the dialogs' fill in the dark theme, or an unticked box doesn't show.
    v.widgets.inactive.bg_fill = p().widget;
    v.widgets.inactive.weak_bg_fill = panel_raised();
    v.widgets.inactive.bg_stroke = if dark { Stroke::NONE } else { Stroke::new(1.0, border()) };
    v.widgets.inactive.corner_radius = CornerRadius::same(6);
    v.widgets.hovered.bg_fill = hover();
    v.widgets.hovered.weak_bg_fill = hover();
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, border());
    v.widgets.hovered.corner_radius = CornerRadius::same(6);
    v.widgets.active.bg_fill = accent_dim();
    v.widgets.active.weak_bg_fill = accent_dim();
    v.widgets.active.corner_radius = CornerRadius::same(6);
    v.widgets.open.bg_fill = panel_raised();
    v.widgets.open.weak_bg_fill = panel_raised();
    v.widgets.open.corner_radius = CornerRadius::same(6);
    let theme = if dark { egui::Theme::Dark } else { egui::Theme::Light };
    ctx.options_mut(|o| o.theme_preference = theme.into());
    ctx.set_visuals_of(theme, v);
    let text_styles = egui::Style::default().text_styles;
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6.0, 6.0);
        s.spacing.button_padding = egui::vec2(8.0, 4.0);
        s.spacing.interact_size.y = 24.0;
        s.text_styles = text_styles.iter().map(|(k, f)| (k.clone(), egui::FontId::new(f.size * text_scale, f.family.clone()))).collect();
    });
}

/// Makes the interact height as tall as a text field or dropdown at the current text size, so
/// a label in a row or grid sits level with the control beside it: egui centers each widget on
/// the row's height as it began (the interact height), and a control taller than that pushes
/// the row down from under the labels already placed. Cheap, and run every frame, since the
/// fonts (and so the text's height) can change after the style is set.
pub fn fit_row_height(ctx: &egui::Context) {
    let style = ctx.global_style();
    let fonts = [egui::TextStyle::Button.resolve(&style), egui::TextStyle::Body.resolve(&style)];
    let text = ctx.fonts_mut(|f| fonts.iter().map(|font| f.row_height(font)).fold(0.0, f32::max));
    let want = (text + 2.0 * style.spacing.button_padding.y).ceil().max(24.0);
    if (style.spacing.interact_size.y - want).abs() > 0.5 {
        ctx.all_styles_mut(|s| s.spacing.interact_size.y = want);
    }
}

/// The virtual database holding BioCartoon's own assets: its templates, and the uploads
/// (source `user`) a reviewer accepted to it, whose `source_name` is BioCartoon (or, from before
/// authors were kept, whose author is).
pub use biocartoon_catalog::catalog::{BIOCARTOON_AUTHOR, BIOCARTOON_SOURCE};

/// The database an asset is shown under: uploads accepted to BioCartoon count as BioCartoon.
pub fn asset_source(row: &crate::catalog::AssetRow) -> &str {
    if row.source == "user" && (row.source_name == BIOCARTOON_AUTHOR || row.author == BIOCARTOON_AUTHOR) { BIOCARTOON_SOURCE } else { &row.source }
}

/// Colour + abbreviation badge for each asset database (lower-left corner of result tiles). The
/// codes and names are the catalog's ([`biocartoon_catalog::catalog::SOURCES`]), as the command
/// line lists them.
pub fn source_badge(source: &str) -> (&'static str, Color32, &'static str) {
    let color = match source {
        "biocartoon" => Color32::from_rgb(0xff, 0x6c, 0x0c),
        "bioart" => Color32::from_rgb(0x2f, 0x80, 0xed),
        "bioicons" => Color32::from_rgb(0x27, 0xae, 0x60),
        "servier" => Color32::from_rgb(0x8e, 0x44, 0xad),
        "scidraw" => Color32::from_rgb(0xe6, 0x7e, 0x22),
        "healthicons" => Color32::from_rgb(0x16, 0xa0, 0x85),
        "reactome" => Color32::from_rgb(0xc0, 0x39, 0x2b),
        "openmoji" => Color32::from_rgb(0xf1, 0xc4, 0x0f),
        "user" => Color32::from_rgb(0x9e, 0x9e, 0x9e),
        _ => Color32::GRAY,
    };
    match biocartoon_catalog::catalog::SOURCES.iter().find(|s| s.0 == source) {
        Some((_, code, name, _)) => (code, color, name),
        None => ("??", color, "Unknown"),
    }
}

#[cfg(test)]
mod tests {
    use super::TextSize;

    /// A text size saved under its old name keeps its scale; the old default becomes the new one.
    #[test]
    fn old_text_size_names_load() {
        let load = |s: &str| serde_json::from_str::<TextSize>(&format!("\"{s}\"")).unwrap();
        assert_eq!(load("Small"), TextSize::ExtraSmall);
        assert_eq!(load("Medium"), TextSize::default());
        assert_eq!(load("Large").scale(), 1.15);
        assert_eq!(load("ExtraLarge"), TextSize::Large);
        for t in TextSize::ALL {
            assert_eq!(load(serde_json::to_string(&t).unwrap().trim_matches('"')), t);
        }
    }

    /// Each theme's slot (dark or light) matches its palette, and the script names are unique.
    #[test]
    fn themes_are_consistent() {
        use super::ColorTheme;
        let lum = |c: egui::Color32| c.r() as u32 + c.g() as u32 + c.b() as u32;
        let mut slugs = std::collections::HashSet::new();
        for t in ColorTheme::ALL {
            let p = t.palette();
            assert_eq!(t.is_dark(), lum(p.bg) < lum(p.text), "{t:?}");
            assert!(slugs.insert(t.slug()), "{t:?}");
            // Slider rails and unticked boxes sit on the background and the panels.
            assert!(p.widget != p.bg && p.widget != p.panel, "{t:?}");
        }
        assert!(ColorTheme::DEFAULT_DARK.is_dark() && !ColorTheme::DEFAULT_LIGHT.is_dark());
        assert_eq!(ColorTheme::of(true).count() + ColorTheme::of(false).count(), ColorTheme::ALL.len());
    }

    /// Enter presses a dialog's orange button, except while a multi-line field (a new line) or a
    /// button (pressing that one) has keyboard focus. A one-line field gives up focus on Enter.
    #[test]
    fn enter_presses_the_orange_button() {
        use egui::{Event, Key, Modifiers, RawInput};
        let enter = || RawInput {
            events: vec![Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }],
            ..Default::default()
        };
        // `field`: 0 none, 1 one-line, 2 multi-line. Runs a frame to focus it, then one with Enter.
        let run = |field: u8| {
            let ctx = egui::Context::default();
            let mut text = String::from("abc");
            let mut fired = false;
            for (frame, input) in [RawInput::default(), enter()].into_iter().enumerate() {
                let mut out = ctx.run_ui(input, |ui| {
                    let r = match field {
                        1 => Some(ui.text_edit_singleline(&mut text)),
                        2 => Some(ui.text_edit_multiline(&mut text)),
                        _ => None,
                    };
                    if let Some(r) = r {
                        if frame == 0 { r.request_focus(); }
                    }
                    fired |= super::enter_pressed(ui);
                });
                out.textures_delta.clear();
            }
            fired
        };
        assert!(run(0));
        assert!(run(1));
        assert!(!run(2));
    }

    /// Enter shows the orange button pressed for a moment, then presses it once.
    #[test]
    fn enter_shows_the_press_before_acting() {
        use egui::{Event, Key, Modifiers, RawInput};
        let ctx = egui::Context::default();
        let frame = |time: f64, enter: bool| {
            let events = if enter { vec![Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }] } else { vec![] };
            let mut fired = false;
            let mut out = ctx.run_ui(RawInput { time: Some(time), events, ..Default::default() }, |ui| {
                fired = super::press_on_enter(ui, true, |ui| ui.button("Go").clicked()).1;
            });
            out.textures_delta.clear();
            fired
        };
        assert!(!frame(0.0, false));
        assert!(!frame(0.1, true));
        assert!(!frame(0.15, false));
        assert!(frame(0.3, false));
        assert!(!frame(0.4, false));
    }
}
