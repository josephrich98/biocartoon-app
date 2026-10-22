//! Dark and light palettes (orange accent) and egui visuals.

use egui::{Color32, CornerRadius, Stroke, Visuals};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};

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

/// Size of the interface text.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum TextSize {
    Small,
    #[default]
    Medium,
    Large,
    ExtraLarge,
}

impl TextSize {
    pub const ALL: [TextSize; 4] = [TextSize::Small, TextSize::Medium, TextSize::Large, TextSize::ExtraLarge];
    pub fn label(self) -> &'static str {
        match self {
            TextSize::Small => "Small",
            TextSize::Medium => "Medium (default)",
            TextSize::Large => "Large",
            TextSize::ExtraLarge => "Extra large",
        }
    }
    pub fn scale(self) -> f32 {
        match self {
            TextSize::Small => 0.9,
            TextSize::Medium => 1.0,
            TextSize::Large => 1.15,
            TextSize::ExtraLarge => 1.3,
        }
    }
}

static DARK: AtomicBool = AtomicBool::new(true);

pub fn is_dark() -> bool {
    DARK.load(Ordering::Relaxed)
}

fn pick(dark: (u8, u8, u8), light: (u8, u8, u8)) -> Color32 {
    let (r, g, b) = if is_dark() { dark } else { light };
    Color32::from_rgb(r, g, b)
}

pub fn bg() -> Color32 { pick((0x1b, 0x1b, 0x1b), (0xec, 0xec, 0xef)) }
pub fn panel() -> Color32 { pick((0x25, 0x25, 0x27), (0xff, 0xff, 0xff)) }
pub fn panel_raised() -> Color32 { pick((0x2d, 0x2d, 0x30), (0xf2, 0xf2, 0xf5)) }
pub fn border() -> Color32 { pick((0x3a, 0x3a, 0x3e), (0xd6, 0xd6, 0xdc)) }
pub fn text() -> Color32 { pick((0xe8, 0xe8, 0xea), (0x1d, 0x1d, 0x20)) }
pub fn text_dim() -> Color32 { pick((0xa0, 0xa0, 0xa8), (0x6b, 0x6b, 0x74)) }
pub fn accent() -> Color32 { Color32::from_rgb(0xff, 0x6c, 0x0c) }
pub fn accent_dim() -> Color32 { pick((0x8c, 0x3d, 0x0a), (0xff, 0xd4, 0xb5)) }
/// Text drawn on an [`accent_dim`] fill.
pub fn on_accent_dim() -> Color32 { if is_dark() { Color32::WHITE } else { text() } }
pub fn canvas_bg() -> Color32 { pick((0x1e, 0x1e, 0x20), (0xe2, 0xe2, 0xe6)) }
pub fn handle() -> Color32 { accent() }
/// Fill of a hovered button or tile.
pub fn hover() -> Color32 { pick((0x3a, 0x3a, 0x40), (0xe2, 0xe2, 0xe8)) }
/// Fill of an informational toast.
pub fn toast() -> Color32 { pick((0x2a, 0x2a, 0x30), (0xff, 0xff, 0xff)) }
/// Fill of a banner saying something worked (white text on it).
pub fn success() -> Color32 { Color32::from_rgb(0x2e, 0x8b, 0x4a) }

/// Sets the palette and the egui visuals; `text_scale` multiplies egui's default text sizes.
pub fn apply(ctx: &egui::Context, dark: bool, text_scale: f32) {
    DARK.store(dark, Ordering::Relaxed);
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = panel();
    v.window_fill = if dark { panel_raised() } else { panel() };
    v.extreme_bg_color = if dark { Color32::from_rgb(0x1a, 0x1a, 0x1c) } else { Color32::from_rgb(0xf7, 0xf7, 0xf9) };
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
    v.widgets.inactive.bg_fill = panel_raised();
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

/// The virtual database holding BioCartoon's own assets: its templates, and the uploads
/// (source `user`) whose author is BioCartoon.
pub use biocartoon_catalog::catalog::{BIOCARTOON_AUTHOR, BIOCARTOON_SOURCE};

/// The database an asset is shown under: BioCartoon's own uploads count as BioCartoon.
pub fn asset_source<'a>(source: &'a str, author: &str) -> &'a str {
    if source == "user" && author == BIOCARTOON_AUTHOR { BIOCARTOON_SOURCE } else { source }
}

/// Colour + abbreviation badge for each asset database (lower-left corner of result tiles). The
/// codes and names are the catalog's ([`biocartoon_catalog::catalog::SOURCES`]), as the command
/// line lists them.
pub fn source_badge(source: &str) -> (&'static str, Color32, &'static str) {
    let color = match source {
        "biocartoon" => accent(),
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
