//! Document model: a deck of slides, each a list of canvas items. Serialised as JSON
//! (`*.biocartoon`). Coordinates are document pixels at 96 dpi (1 in = 96 px).

use egui::{Color32, Pos2, Rect, Vec2};
use serde::{Deserialize, Serialize};

pub use crate::code::{CodeLang, CodeTheme};

pub const DPI: f32 = 96.0;
pub type ItemId = u64;

/// Largest font size a document holds, in document px: where the text panel's size field stops
/// (which clamps a larger size as soon as the item is selected).
pub const MAX_FONT_SIZE: f32 = 400.0;
/// Coordinates and sizes are kept within this many document px of the origin (about 10 m).
pub const MAX_COORD: f32 = 1.0e6;
/// The smallest page side, in document px (as the page size fields allow).
pub const MIN_PAGE: f32 = 48.0;
/// Largest glyph drawn on screen, in physical pixels. egui rasterises every glyph into one font
/// atlas and panics on a glyph wider than it (16384 px), and each large glyph grows the atlas, so
/// text zoomed in past this is drawn at this size instead.
const MAX_GLYPH_PX: f32 = 2048.0;

/// The font size to draw at on screen for `px` points: at least 1, and small enough for the
/// font atlas (see [`MAX_GLYPH_PX`]).
pub fn screen_font(ctx: &egui::Context, px: f32) -> f32 {
    px.clamp(1.0, MAX_GLYPH_PX / ctx.pixels_per_point().max(0.1))
}

/// Replace `v` with `default` when it is NaN or infinite (JSON has no spelling for those, so they
/// would save as `null` and the file would no longer load) or the [`NULL_NUMBER`] a damaged file
/// was read with, and keep it within `range` otherwise.
fn fix(v: &mut f32, default: f32, range: std::ops::RangeInclusive<f32>) {
    *v = if v.is_finite() && *v != NULL_NUMBER { v.clamp(*range.start(), *range.end()) } else { default };
}

/// What [`from_json`] reads a `null` number as; [`Document::upgrade`] replaces it.
const NULL_NUMBER: f32 = f32::MIN;

/// Parse JSON written by the editor. Versions up to 0.3.9 could save a NaN or infinite number
/// (from the control socket), which JSON writes as `null` and which then failed to parse as a
/// number; such a `null` is read as [`NULL_NUMBER`] so the file still opens, and
/// [`Document::upgrade`] puts a sensible value in its place.
pub fn from_json<T: serde::de::DeserializeOwned>(s: &str) -> serde_json::Result<T> {
    let mut s = std::borrow::Cow::Borrowed(s);
    loop {
        match serde_json::from_str(&s) {
            Err(e) if e.is_data() && e.to_string().starts_with("invalid type: null, expected f32") => {
                // serde_json reports the position of the null's last character.
                let line_start: usize = s.split_inclusive('\n').take(e.line().saturating_sub(1)).map(str::len).sum();
                let end = line_start + e.column();
                let Some(at) = s.get(..end).and_then(|head| head.rfind("null")).filter(|at| end - at <= 8) else { return Err(e) };
                s.to_mut().replace_range(at..at + 4, &format!("{NULL_NUMBER:e}"));
            }
            r => return r,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ImageSource {
    /// An asset from the catalog (`assets.sqlite`); `key` is the R2 object key of the PNG, and
    /// `svg` that of the same image as an SVG when the source has one (it is drawn and exported
    /// from that, with the PNG as the fallback). `credit` and `license` are copied from the
    /// catalog when it is placed, so the document can list its attributions on its own
    /// (documents from before then fill them in on load).
    Asset {
        id: String,
        key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        svg: Option<String>,
        title: String,
        source: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        credit: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        license: String,
    },
    /// A file on disk (an upload or an AI-generated image saved to the app data dir). `from` is
    /// the asset it was made from (an AI edit of a catalog asset), whose credit it still owes.
    File {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<Box<ImageSource>>,
    },
}

impl ImageSource {
    /// Cache key used by [`crate::assets::ImageCache`].
    pub fn cache_key(&self) -> String {
        match self {
            ImageSource::Asset { key, svg, .. } => format!("asset:{}", svg.as_deref().unwrap_or(key)),
            ImageSource::File { path, .. } => format!("file:{path}"),
        }
    }

    pub fn file(path: String) -> Self {
        ImageSource::File { path, from: None }
    }

    /// The catalog or template asset this image is, or was made from.
    pub fn origin(&self) -> Option<&ImageSource> {
        match self {
            ImageSource::Asset { .. } => Some(self),
            ImageSource::File { from, .. } => from.as_deref().and_then(ImageSource::origin),
        }
    }

    pub fn origin_mut(&mut self) -> Option<&mut ImageSource> {
        match self {
            ImageSource::Asset { .. } => Some(self),
            ImageSource::File { from, .. } => from.as_deref_mut().and_then(ImageSource::origin_mut),
        }
    }

    /// The source of an edited copy of this image: a file that remembers the original asset.
    pub fn derived(&self, path: String) -> Self {
        ImageSource::File { path, from: self.origin().map(|o| Box::new(o.clone())) }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ShapeKind {
    Rectangle,
    RoundedRectangle,
    Ellipse,
    Triangle,
    Diamond,
    Hexagon,
    Star,
    BlockArrow,
}

impl ShapeKind {
    pub const ALL: [ShapeKind; 8] = [
        ShapeKind::Rectangle,
        ShapeKind::RoundedRectangle,
        ShapeKind::Ellipse,
        ShapeKind::Triangle,
        ShapeKind::Diamond,
        ShapeKind::Hexagon,
        ShapeKind::Star,
        ShapeKind::BlockArrow,
    ];
    pub fn label(self) -> &'static str {
        match self {
            ShapeKind::Rectangle => "Rectangle",
            ShapeKind::RoundedRectangle => "Rounded rectangle",
            ShapeKind::Ellipse => "Ellipse",
            ShapeKind::Triangle => "Triangle",
            ShapeKind::Diamond => "Diamond",
            ShapeKind::Hexagon => "Hexagon",
            ShapeKind::Star => "Star",
            ShapeKind::BlockArrow => "Block arrow",
        }
    }
}

/// The connector family a [`LineKind`] belongs to: how the path runs from start to end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineFamily {
    Straight,
    /// Two right-angle turns through the middle.
    Bent,
    /// A smooth S-curve.
    Spline,
    /// An open arc around the ellipse the end points span.
    Circular,
    /// A path drawn by hand with the Draw tool (not in [`LineFamily::ALL`]: it can't be inserted
    /// from a tile, only drawn).
    Freehand,
}

impl LineFamily {
    pub const ALL: [LineFamily; 4] = [LineFamily::Straight, LineFamily::Bent, LineFamily::Spline, LineFamily::Circular];
    pub fn label(self) -> &'static str {
        match self {
            LineFamily::Straight => "Straight",
            LineFamily::Bent => "Bent",
            LineFamily::Spline => "Spline",
            LineFamily::Circular => "Circular",
            LineFamily::Freehand => "Freehand",
        }
    }
}

/// What is drawn at one end of a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineHead {
    None,
    Arrow,
    /// A bar across the end (inhibition).
    Bar,
    /// Half an arrowhead, on the outer side (equilibrium arrows).
    Harpoon,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum LineKind {
    Straight,
    Arrow,
    ReverseArrow,
    DoubleArrow,
    #[serde(alias = "Inhibit")]
    Inhibitor,
    /// Two harpoons, the forward one longer.
    Equilibrium,
    /// Two harpoons, the reverse one longer.
    ReverseEquilibrium,
    #[serde(alias = "Elbow")]
    Bent,
    #[serde(alias = "ElbowArrow")]
    BentArrow,
    BentReverseArrow,
    BentDoubleArrow,
    BentInhibitor,
    Spline,
    SplineArrow,
    SplineReverseArrow,
    SplineDoubleArrow,
    Circular,
    CircularArrow,
    CircularReverseArrow,
    CircularDoubleArrow,
    /// Drawn by hand: the path is the line's `points` (not in [`LineKind::ALL`]).
    Freehand,
    /// Saved by earlier versions, where dashing was a kind rather than a [`LineStyle`];
    /// [`Document::upgrade`] turns these into `Straight` / `Arrow` with `LineStyle::Dashed`.
    #[doc(hidden)]
    #[serde(rename = "Dashed")]
    LegacyDashed,
    #[doc(hidden)]
    #[serde(rename = "DashedArrow")]
    LegacyDashedArrow,
}

impl LineKind {
    pub const ALL: [LineKind; 20] = [
        LineKind::Straight,
        LineKind::Arrow,
        LineKind::ReverseArrow,
        LineKind::DoubleArrow,
        LineKind::Inhibitor,
        LineKind::Equilibrium,
        LineKind::ReverseEquilibrium,
        LineKind::Bent,
        LineKind::BentArrow,
        LineKind::BentReverseArrow,
        LineKind::BentDoubleArrow,
        LineKind::BentInhibitor,
        LineKind::Spline,
        LineKind::SplineArrow,
        LineKind::SplineReverseArrow,
        LineKind::SplineDoubleArrow,
        LineKind::Circular,
        LineKind::CircularArrow,
        LineKind::CircularReverseArrow,
        LineKind::CircularDoubleArrow,
    ];

    pub fn family(self) -> LineFamily {
        match self {
            LineKind::Bent | LineKind::BentArrow | LineKind::BentReverseArrow | LineKind::BentDoubleArrow | LineKind::BentInhibitor => LineFamily::Bent,
            LineKind::Spline | LineKind::SplineArrow | LineKind::SplineReverseArrow | LineKind::SplineDoubleArrow => LineFamily::Spline,
            LineKind::Circular | LineKind::CircularArrow | LineKind::CircularReverseArrow | LineKind::CircularDoubleArrow => LineFamily::Circular,
            LineKind::Freehand => LineFamily::Freehand,
            _ => LineFamily::Straight,
        }
    }

    /// The heads at the start and at the end of the line.
    pub fn heads(self) -> (LineHead, LineHead) {
        use LineHead::*;
        match self {
            LineKind::Straight | LineKind::Bent | LineKind::Spline | LineKind::Circular | LineKind::Freehand | LineKind::LegacyDashed => (None, None),
            LineKind::Arrow | LineKind::BentArrow | LineKind::SplineArrow | LineKind::CircularArrow | LineKind::LegacyDashedArrow => (None, Arrow),
            LineKind::ReverseArrow | LineKind::BentReverseArrow | LineKind::SplineReverseArrow | LineKind::CircularReverseArrow => (Arrow, None),
            LineKind::DoubleArrow | LineKind::BentDoubleArrow | LineKind::SplineDoubleArrow | LineKind::CircularDoubleArrow => (Arrow, Arrow),
            LineKind::Inhibitor | LineKind::BentInhibitor => (None, Bar),
            LineKind::Equilibrium | LineKind::ReverseEquilibrium => (Harpoon, Harpoon),
        }
    }

    pub fn is_equilibrium(self) -> bool {
        matches!(self, LineKind::Equilibrium | LineKind::ReverseEquilibrium)
    }

    /// Needs a box, not just a length, to be visible (bent, spline and circular lines).
    pub fn needs_height(self) -> bool {
        self.family() != LineFamily::Straight
    }

    /// The name within its family ("Reverse arrow"), for the Lines panel.
    pub fn short_label(self) -> &'static str {
        match self {
            LineKind::Straight | LineKind::Bent | LineKind::Spline | LineKind::Circular | LineKind::LegacyDashed => "Line",
            LineKind::Freehand => "Drawing",
            LineKind::Arrow | LineKind::BentArrow | LineKind::SplineArrow | LineKind::CircularArrow | LineKind::LegacyDashedArrow => "Arrow",
            LineKind::ReverseArrow | LineKind::BentReverseArrow | LineKind::SplineReverseArrow | LineKind::CircularReverseArrow => "Reverse arrow",
            LineKind::DoubleArrow | LineKind::BentDoubleArrow | LineKind::SplineDoubleArrow | LineKind::CircularDoubleArrow => "Double arrow",
            LineKind::Inhibitor | LineKind::BentInhibitor => "Inhibitor",
            LineKind::Equilibrium => "Forward equilibrium",
            LineKind::ReverseEquilibrium => "Reverse equilibrium",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LineKind::Straight | LineKind::LegacyDashed => "Line",
            LineKind::Arrow | LineKind::LegacyDashedArrow => "Arrow",
            LineKind::ReverseArrow => "Reverse arrow",
            LineKind::DoubleArrow => "Double arrow",
            LineKind::Inhibitor => "Inhibitor",
            LineKind::Equilibrium => "Forward equilibrium arrow",
            LineKind::ReverseEquilibrium => "Reverse equilibrium arrow",
            LineKind::Bent => "Bent line",
            LineKind::BentArrow => "Bent arrow",
            LineKind::BentReverseArrow => "Bent reverse arrow",
            LineKind::BentDoubleArrow => "Bent double arrow",
            LineKind::BentInhibitor => "Bent inhibitor",
            LineKind::Spline => "Spline line",
            LineKind::SplineArrow => "Spline arrow",
            LineKind::SplineReverseArrow => "Spline reverse arrow",
            LineKind::SplineDoubleArrow => "Spline double arrow",
            LineKind::Circular => "Circular line",
            LineKind::CircularArrow => "Circular arrow",
            LineKind::CircularReverseArrow => "Circular reverse arrow",
            LineKind::CircularDoubleArrow => "Circular double arrow",
            LineKind::Freehand => "Drawing",
        }
    }
}

/// Solid, dashed or dotted stroke, for lines.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum LineStyle {
    #[default]
    Solid,
    Dashed,
    Dotted,
}

impl LineStyle {
    pub const ALL: [LineStyle; 3] = [LineStyle::Solid, LineStyle::Dashed, LineStyle::Dotted];
    pub fn label(self) -> &'static str {
        match self {
            LineStyle::Solid => "Solid",
            LineStyle::Dashed => "Dashed",
            LineStyle::Dotted => "Dotted",
        }
    }
    /// Dash and gap lengths for a stroke `w` wide, or `None` for a solid stroke.
    pub fn dash(self, w: f32) -> Option<(f32, f32)> {
        match self {
            LineStyle::Solid => None,
            LineStyle::Dashed => Some(((3.0 * w).max(6.0), (2.0 * w).max(4.0))),
            LineStyle::Dotted => Some((w.max(1.0), (1.5 * w).max(2.0))),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    Center,
    Right,
    Justify,
}

impl TextAlign {
    pub const ALL: [TextAlign; 4] = [TextAlign::Left, TextAlign::Center, TextAlign::Right, TextAlign::Justify];
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum TextVAlign {
    #[default]
    Top,
    Middle,
    Bottom,
}

impl TextVAlign {
    pub const ALL: [TextVAlign; 3] = [TextVAlign::Top, TextVAlign::Middle, TextVAlign::Bottom];
}

/// The typefaces a text box can use: the ones bundled with egui, plus free fonts embedded from
/// `assets/fonts` (see [`TextFont::EMBEDDED`]). New variants go at the end, so saved documents and
/// scripts keep their meaning; the menu order is [`TextFont::ALL`].
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum TextFont {
    #[default]
    Sans,
    Mono,
    Arimo,
    Tinos,
    Cousine,
    Roboto,
    OpenSans,
    Lato,
    Montserrat,
    SourceSerif,
    EbGaramond,
    ComicNeue,
    Carlito,
    Caladea,
    Gelasio,
    Inter,
    SourceSans,
    Poppins,
    Nunito,
    Raleway,
    WorkSans,
    IbmPlexSans,
    DmSans,
    Barlow,
    Atkinson,
    Lexend,
    Quicksand,
    Cabin,
    JosefinSans,
    Oswald,
    RobotoCondensed,
    BebasNeue,
    Lora,
    Playfair,
    LibreBaskerville,
    CrimsonText,
    IbmPlexSerif,
    RobotoSlab,
    Arvo,
    JetBrainsMono,
    SourceCodePro,
    IbmPlexMono,
    RobotoMono,
    CourierPrime,
    PermanentMarker,
    IndieFlower,
    PatrickHand,
    ShadowsIntoLight,
    ArchitectsDaughter,
}

/// The broad kind of a typeface, for the fallbacks an exported SVG names after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontKind {
    Sans,
    Serif,
    Mono,
    Handwriting,
}

impl TextFont {
    /// Every font, in the menu's (alphabetical) order.
    pub const ALL: [TextFont; 49] = [
        TextFont::ArchitectsDaughter,
        TextFont::Arimo,
        TextFont::Arvo,
        TextFont::Atkinson,
        TextFont::Barlow,
        TextFont::BebasNeue,
        TextFont::Cabin,
        TextFont::Caladea,
        TextFont::Carlito,
        TextFont::ComicNeue,
        TextFont::CourierPrime,
        TextFont::Cousine,
        TextFont::CrimsonText,
        TextFont::DmSans,
        TextFont::EbGaramond,
        TextFont::Gelasio,
        TextFont::Mono,
        TextFont::IbmPlexMono,
        TextFont::IbmPlexSans,
        TextFont::IbmPlexSerif,
        TextFont::IndieFlower,
        TextFont::Inter,
        TextFont::JetBrainsMono,
        TextFont::JosefinSans,
        TextFont::Lato,
        TextFont::Lexend,
        TextFont::LibreBaskerville,
        TextFont::Lora,
        TextFont::Montserrat,
        TextFont::Nunito,
        TextFont::OpenSans,
        TextFont::Oswald,
        TextFont::PatrickHand,
        TextFont::PermanentMarker,
        TextFont::Playfair,
        TextFont::Poppins,
        TextFont::Quicksand,
        TextFont::Raleway,
        TextFont::Roboto,
        TextFont::RobotoCondensed,
        TextFont::RobotoMono,
        TextFont::RobotoSlab,
        TextFont::ShadowsIntoLight,
        TextFont::SourceCodePro,
        TextFont::SourceSans,
        TextFont::SourceSerif,
        TextFont::Tinos,
        TextFont::Sans,
        TextFont::WorkSans,
    ];

    /// The embedded fonts: (variant, family name, kind, file). The family name is both the
    /// egui `FontFamily::Name` and the name in the font file, which the SVG export refers to.
    pub const EMBEDDED: [(TextFont, &'static str, FontKind, FontFile); 47] = [
        (TextFont::Arimo, "Arimo", FontKind::Sans, ttf!("Arimo-Regular.ttf")),
        (TextFont::Tinos, "Tinos", FontKind::Serif, ttf!("Tinos-Regular.ttf")),
        (TextFont::Cousine, "Cousine", FontKind::Mono, ttf!("Cousine-Regular.ttf")),
        (TextFont::Roboto, "Roboto", FontKind::Sans, ttf!("Roboto-Regular.ttf")),
        (TextFont::OpenSans, "Open Sans", FontKind::Sans, ttf!("OpenSans-Regular.ttf")),
        (TextFont::Lato, "Lato", FontKind::Sans, ttf!("Lato-Regular.ttf")),
        (TextFont::Montserrat, "Montserrat", FontKind::Sans, ttf!("Montserrat-Regular.ttf")),
        (TextFont::SourceSerif, "Source Serif 4", FontKind::Serif, ttf!("SourceSerif4-Regular.ttf")),
        (TextFont::EbGaramond, "EB Garamond", FontKind::Serif, ttf!("EBGaramond-Regular.ttf")),
        (TextFont::ComicNeue, "Comic Neue", FontKind::Handwriting, ttf!("ComicNeue-Regular.ttf")),
        (TextFont::Carlito, "Carlito", FontKind::Sans, ttf!("Carlito-Regular.ttf")),
        (TextFont::Caladea, "Caladea", FontKind::Serif, ttf!("Caladea-Regular.ttf")),
        (TextFont::Gelasio, "Gelasio", FontKind::Serif, ttf!("Gelasio-Regular.ttf")),
        (TextFont::Inter, "Inter", FontKind::Sans, ttf!("Inter-Regular.ttf")),
        (TextFont::SourceSans, "Source Sans 3", FontKind::Sans, ttf!("SourceSans3-Regular.ttf")),
        (TextFont::Poppins, "Poppins", FontKind::Sans, ttf!("Poppins-Regular.ttf")),
        (TextFont::Nunito, "Nunito", FontKind::Sans, ttf!("Nunito-Regular.ttf")),
        (TextFont::Raleway, "Raleway", FontKind::Sans, ttf!("Raleway-Regular.ttf")),
        (TextFont::WorkSans, "Work Sans", FontKind::Sans, ttf!("WorkSans-Regular.ttf")),
        (TextFont::IbmPlexSans, "IBM Plex Sans", FontKind::Sans, ttf!("IBMPlexSans-Regular.ttf")),
        (TextFont::DmSans, "DM Sans", FontKind::Sans, ttf!("DMSans-Regular.ttf")),
        (TextFont::Barlow, "Barlow", FontKind::Sans, ttf!("Barlow-Regular.ttf")),
        (TextFont::Atkinson, "Atkinson Hyperlegible", FontKind::Sans, ttf!("AtkinsonHyperlegible-Regular.ttf")),
        (TextFont::Lexend, "Lexend", FontKind::Sans, ttf!("Lexend-Regular.ttf")),
        (TextFont::Quicksand, "Quicksand", FontKind::Sans, ttf!("Quicksand-Regular.ttf")),
        (TextFont::Cabin, "Cabin", FontKind::Sans, ttf!("Cabin-Regular.ttf")),
        (TextFont::JosefinSans, "Josefin Sans", FontKind::Sans, ttf!("JosefinSans-Regular.ttf")),
        (TextFont::Oswald, "Oswald", FontKind::Sans, ttf!("Oswald-Regular.ttf")),
        (TextFont::RobotoCondensed, "Roboto Condensed", FontKind::Sans, ttf!("RobotoCondensed-Regular.ttf")),
        (TextFont::BebasNeue, "Bebas Neue", FontKind::Sans, ttf!("BebasNeue-Regular.ttf")),
        (TextFont::Lora, "Lora", FontKind::Serif, ttf!("Lora-Regular.ttf")),
        (TextFont::Playfair, "Playfair Display", FontKind::Serif, ttf!("PlayfairDisplay-Regular.ttf")),
        (TextFont::LibreBaskerville, "Libre Baskerville", FontKind::Serif, ttf!("LibreBaskerville-Regular.ttf")),
        (TextFont::CrimsonText, "Crimson Text", FontKind::Serif, ttf!("CrimsonText-Regular.ttf")),
        (TextFont::IbmPlexSerif, "IBM Plex Serif", FontKind::Serif, ttf!("IBMPlexSerif-Regular.ttf")),
        (TextFont::RobotoSlab, "Roboto Slab", FontKind::Serif, ttf!("RobotoSlab-Regular.ttf")),
        (TextFont::Arvo, "Arvo", FontKind::Serif, ttf!("Arvo-Regular.ttf")),
        (TextFont::JetBrainsMono, "JetBrains Mono", FontKind::Mono, ttf!("JetBrainsMono-Regular.ttf")),
        (TextFont::SourceCodePro, "Source Code Pro", FontKind::Mono, ttf!("SourceCodePro-Regular.ttf")),
        (TextFont::IbmPlexMono, "IBM Plex Mono", FontKind::Mono, ttf!("IBMPlexMono-Regular.ttf")),
        (TextFont::RobotoMono, "Roboto Mono", FontKind::Mono, ttf!("RobotoMono-Regular.ttf")),
        (TextFont::CourierPrime, "Courier Prime", FontKind::Mono, ttf!("CourierPrime-Regular.ttf")),
        (TextFont::PermanentMarker, "Permanent Marker", FontKind::Handwriting, ttf!("PermanentMarker-Regular.ttf")),
        (TextFont::IndieFlower, "Indie Flower", FontKind::Handwriting, ttf!("IndieFlower-Regular.ttf")),
        (TextFont::PatrickHand, "Patrick Hand", FontKind::Handwriting, ttf!("PatrickHand-Regular.ttf")),
        (TextFont::ShadowsIntoLight, "Shadows Into Light", FontKind::Handwriting, ttf!("ShadowsIntoLight-Regular.ttf")),
        (TextFont::ArchitectsDaughter, "Architects Daughter", FontKind::Handwriting, ttf!("ArchitectsDaughter-Regular.ttf")),
    ];

    pub fn label(self) -> &'static str {
        match self {
            TextFont::Sans => "Ubuntu",
            TextFont::Mono => "Hack (monospace)",
            TextFont::Arimo => "Arimo (Arial / Helvetica)",
            TextFont::Tinos => "Tinos (Times New Roman)",
            TextFont::Cousine => "Cousine (Courier New)",
            TextFont::SourceSerif => "Source Serif",
            TextFont::Carlito => "Carlito (Calibri)",
            TextFont::Caladea => "Caladea (Cambria)",
            TextFont::Gelasio => "Gelasio (Georgia)",
            TextFont::SourceSans => "Source Sans",
            f => f.embedded_name().unwrap_or_default(),
        }
    }

    pub fn kind(self) -> FontKind {
        match self {
            TextFont::Sans => FontKind::Sans,
            TextFont::Mono => FontKind::Mono,
            f => Self::EMBEDDED.iter().find(|(e, ..)| *e == f).map_or(FontKind::Sans, |(_, _, k, _)| *k),
        }
    }

    /// The embedded font's family name, or `None` for egui's own two.
    pub fn embedded_name(self) -> Option<&'static str> {
        Self::EMBEDDED.iter().find(|(f, ..)| *f == self).map(|(_, name, ..)| *name)
    }

    pub fn font_id(self, px: f32) -> egui::FontId {
        match self {
            TextFont::Sans => egui::FontId::proportional(px),
            TextFont::Mono => egui::FontId::monospace(px),
            f => egui::FontId::new(px, egui::FontFamily::Name(f.embedded_name().unwrap_or_default().into())),
        }
    }

    /// The embedded font's TTF bytes; `None` for egui's own two, and on the web for a font
    /// [`load_fonts`] couldn't fetch.
    pub fn bytes(self) -> Option<&'static [u8]> {
        let i = Self::EMBEDDED.iter().position(|(f, ..)| *f == self)?;
        #[cfg(not(target_arch = "wasm32"))]
        let bytes = Some(Self::EMBEDDED[i].3.bytes);
        #[cfg(target_arch = "wasm32")]
        let bytes = LOADED.get().and_then(|l| l.get(i)).map(|b| b.as_slice());
        bytes.filter(|b| !b.is_empty())
    }

    /// Adds the embedded fonts to `defs`, each falling back to egui's proportional fonts for
    /// glyphs it lacks. Every context that lays out text items needs this. A font that is
    /// missing (on the web, one that failed to load) still gets its family, of the fallbacks
    /// alone, since egui panics on an unknown family.
    pub fn install(defs: &mut egui::FontDefinitions) {
        // egui's Ubuntu and Hack lack glyphs like ⁺ ⁻ → (and Hack ₂): Arimo, last in their
        // chains, covers them without changing any glyph they do have.
        if let Some(bytes) = TextFont::Arimo.bytes() {
            defs.font_data.insert("Arimo".into(), std::sync::Arc::new(egui::FontData::from_static(bytes)));
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                defs.families.entry(family).or_default().push("Arimo".into());
            }
        }
        let fallback = defs.families.get(&egui::FontFamily::Proportional).cloned().unwrap_or_default();
        for (f, name, ..) in Self::EMBEDDED {
            let mut chain = Vec::new();
            if let Some(bytes) = f.bytes() {
                defs.font_data.insert(name.into(), std::sync::Arc::new(egui::FontData::from_static(bytes)));
                chain.push(name.to_string());
            }
            chain.extend(fallback.iter().cloned());
            defs.families.insert(egui::FontFamily::Name(name.into()), chain);
        }
    }
}

/// A font file in `assets/fonts`. The desktop build compiles it in; the web build fetches it at
/// startup ([`load_fonts`]) from the copy index.html puts in `dist/fonts`, which keeps the .wasm
/// under the 25 MiB Cloudflare serves.
pub struct FontFile {
    pub file: &'static str,
    #[cfg(not(target_arch = "wasm32"))]
    bytes: &'static [u8],
}

macro_rules! ttf {
    ($file:literal) => {
        FontFile {
            file: $file,
            #[cfg(not(target_arch = "wasm32"))]
            bytes: include_bytes!(concat!("../assets/fonts/", $file)),
        }
    };
}
use ttf;

/// The fetched font files, in [`TextFont::EMBEDDED`]'s order; empty for one that failed.
#[cfg(target_arch = "wasm32")]
static LOADED: std::sync::OnceLock<Vec<Vec<u8>>> = std::sync::OnceLock::new();

/// Fetches the embedded fonts (all at once). The web build awaits this before starting the app,
/// so the canvas and the exporter see them from the first frame.
#[cfg(target_arch = "wasm32")]
pub async fn load_fonts() {
    let fetches = TextFont::EMBEDDED.iter().map(|(.., file)| async move {
        match ehttp::fetch_async(ehttp::Request::get(format!("fonts/{}", file.file))).await {
            Ok(r) if r.ok => r.bytes,
            Ok(r) => {
                log::warn!("font {}: HTTP {}", file.file, r.status);
                Vec::new()
            }
            Err(e) => {
                log::warn!("font {}: {e}", file.file);
                Vec::new()
            }
        }
    });
    let _ = LOADED.set(futures_util::future::join_all(fetches).await);
}

fn one() -> f32 {
    1.0
}

/// One colour layer of an SVG image restyled: every shape the file paints in one of `colors`
/// (rgb) is painted in `color` instead (its own colours when `None`), at `opacity` of its own.
/// The layers come from the catalog's `layers` (named by hand) or from the drawing's colours.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LayerStyle {
    pub colors: Vec<[u8; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[u8; 3]>,
    #[serde(default = "one")]
    pub opacity: f32,
}

impl LayerStyle {
    /// Whether it changes nothing.
    pub fn is_plain(&self) -> bool {
        self.color.is_none() && self.opacity >= 1.0
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ItemKind {
    Image {
        source: ImageSource,
        /// Crop in uv space: [left, top, right, bottom], each 0..1.
        crop: [f32; 4],
        /// Colour overlay (rgb) and its strength (a). a = 0 means no overlay.
        overlay: [u8; 4],
        /// Parts of an SVG drawn in another colour or faded (see [`LayerStyle`]); only the
        /// restyled ones.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        layers: Vec<LayerStyle>,
    },
    Text {
        text: String,
        size: f32,
        color: [u8; 4],
        align: TextAlign,
        bold: bool,
        #[serde(default)]
        italic: bool,
        #[serde(default)]
        underline: bool,
        #[serde(default)]
        strike: bool,
        #[serde(default)]
        font: TextFont,
        /// Multiple of the font's natural row height.
        #[serde(default = "one")]
        line_spacing: f32,
        #[serde(default)]
        valign: TextVAlign,
        /// Characters styled apart from the rest of the box (see [`TextRun`]).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        runs: Vec<TextRun>,
    },
    Shape {
        shape: ShapeKind,
        fill: [u8; 4],
        stroke: [u8; 4],
        stroke_width: f32,
    },
    Line {
        kind: LineKind,
        color: [u8; 4],
        width: f32,
        #[serde(default)]
        style: LineStyle,
        /// A freehand line's path, each point as a fraction (0..1) of the way across the box from
        /// `pos` to `pos + size`, so it follows the line when moved or resized. Empty otherwise.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        points: Vec<[f32; 2]>,
    },
    Table {
        rows: usize,
        cols: usize,
        cells: Vec<String>,
        header: bool,
        border: [u8; 4],
        fill: [u8; 4],
        header_fill: [u8; 4],
        text_color: [u8; 4],
        font_size: f32,
    },
    /// A syntax-highlighted code block, drawn like a carbon.now.sh window.
    Code {
        code: String,
        language: CodeLang,
        theme: CodeTheme,
        font_size: f32,
        line_numbers: bool,
        /// The title bar with the three window dots.
        window: bool,
        /// Rounded corners (square when off).
        #[serde(default)]
        rounded: bool,
    },
}

impl ItemKind {
    /// A plain text box: upright, sans, single-spaced, top-aligned.
    pub fn text(text: impl Into<String>, size: f32, color: [u8; 4], align: TextAlign, bold: bool) -> Self {
        ItemKind::Text { text: text.into(), size, color, align, bold, italic: false, underline: false, strike: false, font: TextFont::Sans, line_spacing: 1.0, valign: TextVAlign::Top, runs: Vec::new() }
    }
}

/// What some of a text box's characters set over the box's own bold, italic, underline,
/// strikethrough and colour; `None` keeps the box's.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct RunStyle {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub underline: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strike: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[u8; 4]>,
}

/// Characters `start..end` (counted in chars, not bytes) of a text box, styled by `style`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct TextRun {
    pub start: usize,
    pub end: usize,
    #[serde(flatten)]
    pub style: RunStyle,
}

/// How a character of a text box is drawn: the box's style with its run's on top.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CharLook {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub color: [u8; 4],
}

/// One style per character of a `len`-character text from its runs.
pub fn expand_runs(runs: &[TextRun], len: usize) -> Vec<RunStyle> {
    let mut out = vec![RunStyle::default(); len];
    for r in runs {
        for s in out.iter_mut().take(r.end.min(len)).skip(r.start) {
            *s = r.style;
        }
    }
    out
}

/// Per-character styles packed back into runs, leaving out unstyled characters.
pub fn pack_runs(styles: &[RunStyle]) -> Vec<TextRun> {
    let mut out: Vec<TextRun> = Vec::new();
    for (i, s) in styles.iter().enumerate() {
        if *s == RunStyle::default() {
            continue;
        }
        match out.last_mut() {
            Some(r) if r.end == i && r.style == *s => r.end = i + 1,
            _ => out.push(TextRun { start: i, end: i + 1, style: *s }),
        }
    }
    out
}

/// `runs` for `old` moved onto `new`, an edit of it: the characters both keep at the start and
/// end keep their styles, and typed ones take the style of the first character they replaced
/// or, failing that, the one before them.
pub fn remap_runs(runs: &[TextRun], old: &str, new: &str) -> Vec<TextRun> {
    if runs.is_empty() || old == new {
        return runs.to_vec();
    }
    let (a, b): (Vec<char>, Vec<char>) = (old.chars().collect(), new.chars().collect());
    let pre = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let max_suf = a.len().min(b.len()) - pre;
    let suf = a.iter().rev().zip(b.iter().rev()).take(max_suf).take_while(|(x, y)| x == y).count();
    let styles = expand_runs(runs, a.len());
    let typed = if a.len() - suf > pre { styles.get(pre) } else { pre.checked_sub(1).and_then(|i| styles.get(i)) }.copied().unwrap_or_default();
    let mut out = styles[..pre].to_vec();
    out.extend(std::iter::repeat_n(typed, b.len() - suf - pre));
    out.extend_from_slice(&styles[a.len() - suf..]);
    pack_runs(&out)
}

/// Every character's look in a text box (`kind`, a text item), its runs taken as styling `text`.
pub fn char_looks(kind: &ItemKind, text: &str) -> Vec<CharLook> {
    let ItemKind::Text { text: own, color, bold, italic, underline, strike, runs, .. } = kind else { return Vec::new() };
    let runs = if own == text { runs.clone() } else { remap_runs(runs, own, text) };
    let base = CharLook { bold: *bold, italic: *italic, underline: *underline, strike: *strike, color: *color };
    expand_runs(&runs, text.chars().count())
        .into_iter()
        .map(|s| CharLook {
            bold: s.bold.unwrap_or(base.bold),
            italic: s.italic.unwrap_or(base.italic),
            underline: s.underline.unwrap_or(base.underline),
            strike: s.strike.unwrap_or(base.strike),
            color: s.color.unwrap_or(base.color),
        })
        .collect()
}

/// A text box's text split where its look changes: byte ranges and their looks. Never empty: an
/// empty box is one empty section in its own look.
pub fn text_sections(kind: &ItemKind, text: &str) -> Vec<(std::ops::Range<usize>, CharLook)> {
    let ItemKind::Text { color, bold, italic, underline, strike, .. } = kind else { return Vec::new() };
    let mut out: Vec<(std::ops::Range<usize>, CharLook)> = Vec::new();
    for ((b, c), look) in text.char_indices().zip(char_looks(kind, text)) {
        match out.last_mut() {
            Some((r, l)) if *l == look => r.end = b + c.len_utf8(),
            _ => out.push((b..b + c.len_utf8(), look)),
        }
    }
    if out.is_empty() {
        out.push((0..0, CharLook { bold: *bold, italic: *italic, underline: *underline, strike: *strike, color: *color }));
    }
    out
}

/// A text style that is on or off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextFlag {
    Bold,
    Italic,
    Underline,
    Strike,
}

impl TextFlag {
    fn of(self, l: &CharLook) -> bool {
        match self {
            TextFlag::Bold => l.bold,
            TextFlag::Italic => l.italic,
            TextFlag::Underline => l.underline,
            TextFlag::Strike => l.strike,
        }
    }

    fn slot(self, s: &mut RunStyle) -> &mut Option<bool> {
        match self {
            TextFlag::Bold => &mut s.bold,
            TextFlag::Italic => &mut s.italic,
            TextFlag::Underline => &mut s.underline,
            TextFlag::Strike => &mut s.strike,
        }
    }
}

/// The characters (`range`, in chars) of a text box a style change goes to: all of them for `None`.
fn looks_in(kind: &ItemKind, range: Option<std::ops::Range<usize>>) -> Vec<CharLook> {
    let ItemKind::Text { text, .. } = kind else { return Vec::new() };
    let looks = char_looks(kind, text);
    match range {
        Some(r) => looks.get(r.start.min(looks.len())..r.end.min(looks.len())).unwrap_or_default().to_vec(),
        None => looks,
    }
}

/// Whether every character of `range` (the whole box for `None`) of text box `kind` has `flag`;
/// an empty box answers for itself.
pub fn flag_all(kind: &ItemKind, range: Option<std::ops::Range<usize>>, flag: TextFlag) -> bool {
    let looks = looks_in(kind, range);
    if looks.is_empty() {
        return text_sections(kind, "").first().is_some_and(|(_, l)| flag.of(l));
    }
    looks.iter().all(|l| flag.of(l))
}

/// The colour of the first character of `range` (the box's own for `None` or an empty range).
pub fn color_at(kind: &ItemKind, range: Option<std::ops::Range<usize>>) -> [u8; 4] {
    match looks_in(kind, range.clone()).first() {
        Some(l) if range.is_some() => l.color,
        _ => match kind {
            ItemKind::Text { color, .. } => *color,
            _ => [0; 4],
        },
    }
}

/// Change the style of the characters in `range` of text box `kind` with `f`, which gets the
/// box's look so it can leave out what matches it. With `None` the box's own style changes
/// instead (`whole`) and `f` clears every run's say over it.
fn restyle(kind: &mut ItemKind, range: Option<std::ops::Range<usize>>, whole: impl FnOnce(&mut ItemKind), f: impl Fn(&mut RunStyle, &CharLook)) {
    let base = text_sections(kind, "").first().map(|s| s.1);
    let Some(base) = base else { return };
    let ItemKind::Text { text, runs, .. } = kind else { return };
    let n = text.chars().count();
    let mut styles = expand_runs(runs, n);
    let r = range.clone().unwrap_or(0..n);
    for s in styles.iter_mut().take(r.end.min(n)).skip(r.start) {
        f(s, &base);
    }
    *runs = pack_runs(&styles);
    if range.is_none() {
        whole(kind);
    }
}

/// Turn `flag` on or off for the characters in `range` (the whole box for `None`).
pub fn set_flag(kind: &mut ItemKind, range: Option<std::ops::Range<usize>>, flag: TextFlag, on: bool) {
    let whole = range.is_none();
    restyle(
        kind,
        range,
        |k| {
            if let ItemKind::Text { bold, italic, underline, strike, .. } = k {
                *match flag {
                    TextFlag::Bold => bold,
                    TextFlag::Italic => italic,
                    TextFlag::Underline => underline,
                    TextFlag::Strike => strike,
                } = on;
            }
        },
        |s, base| *flag.slot(s) = (!whole && flag.of(base) != on).then_some(on),
    );
}

/// Colour the characters in `range` (the whole box for `None`).
pub fn set_color(kind: &mut ItemKind, range: Option<std::ops::Range<usize>>, c: [u8; 4]) {
    let whole = range.is_none();
    restyle(
        kind,
        range,
        |k| {
            if let ItemKind::Text { color, .. } = k {
                *color = c;
            }
        },
        |s, base| s.color = (!whole && base.color != c).then_some(c),
    );
}

impl Item {
    /// After this text box's text was changed from `old`, move its runs onto the new text.
    pub fn remap_text_runs(&mut self, old: &str) {
        if let ItemKind::Text { text, runs, .. } = &mut self.kind {
            *runs = remap_runs(runs, old, text);
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Item {
    pub id: ItemId,
    pub name: String,
    pub kind: ItemKind,
    pub pos: [f32; 2],
    /// For lines this is the vector from start to end (may be negative).
    pub size: [f32; 2],
    pub rotation: f32,
    pub flip_h: bool,
    pub flip_v: bool,
    pub locked: bool,
    pub opacity: f32,
    pub group: Option<u64>,
    /// Images: the rect frames the picture as it is drawn, an SVG trimmed to its drawing
    /// ([`crate::export::Vector`]). Unset for items placed before their picture had loaded and
    /// for documents from before SVGs were trimmed; the editor fits those once the picture is
    /// loaded (see [`fit_to_drawing`]).
    #[serde(default)]
    pub fitted: bool,
}

impl Item {
    pub fn new(id: ItemId, name: impl Into<String>, kind: ItemKind, pos: Pos2, size: Vec2) -> Self {
        Self {
            id,
            name: name.into(),
            kind,
            pos: [pos.x, pos.y],
            size: [size.x, size.y],
            rotation: 0.0,
            flip_h: false,
            flip_v: false,
            locked: false,
            opacity: 1.0,
            group: None,
            fitted: false,
        }
    }

    pub fn is_line(&self) -> bool {
        matches!(self.kind, ItemKind::Line { .. })
    }

    /// Put every number back in range: NaN and infinities get a default (see [`fix`]), font
    /// sizes stay within [`MAX_FONT_SIZE`] and coordinates within [`MAX_COORD`].
    pub fn sanitize(&mut self) {
        let c = -MAX_COORD..=MAX_COORD;
        for v in &mut self.pos {
            fix(v, 0.0, c.clone());
        }
        let size = if self.is_line() { 0.0 } else { 1.0 };
        for v in &mut self.size {
            fix(v, size, c.clone());
        }
        fix(&mut self.rotation, 0.0, -1.0e4..=1.0e4);
        fix(&mut self.opacity, 1.0, 0.0..=1.0);
        let font = 1.0..=MAX_FONT_SIZE;
        match &mut self.kind {
            ItemKind::Image { crop, .. } => {
                for (v, d) in crop.iter_mut().zip([0.0, 0.0, 1.0, 1.0]) {
                    fix(v, d, 0.0..=1.0);
                }
            }
            ItemKind::Text { size, line_spacing, .. } => {
                fix(size, 24.0, font);
                fix(line_spacing, 1.0, 0.1..=10.0);
            }
            ItemKind::Shape { stroke_width, .. } => fix(stroke_width, 2.0, 0.0..=MAX_FONT_SIZE),
            ItemKind::Line { width, points, .. } => {
                fix(width, 3.0, 0.0..=MAX_FONT_SIZE);
                points.retain(|p| p.iter().all(|v| v.is_finite() && *v != NULL_NUMBER));
                for v in points.iter_mut().flatten() {
                    *v = v.clamp(-MAX_COORD, MAX_COORD);
                }
            }
            ItemKind::Table { font_size, .. } | ItemKind::Code { font_size, .. } => fix(font_size, 14.0, font),
        }
    }

    pub fn pos(&self) -> Pos2 {
        Pos2::new(self.pos[0], self.pos[1])
    }

    pub fn size(&self) -> Vec2 {
        Vec2::new(self.size[0], self.size[1])
    }

    /// Axis-aligned bounding rect in document space (ignores rotation). Lines are normalised.
    pub fn rect(&self) -> Rect {
        let p = self.pos();
        let s = self.size();
        if self.is_line() {
            Rect::from_two_pos(p, p + s)
        } else {
            Rect::from_min_size(p, s)
        }
    }

    pub fn set_rect(&mut self, r: Rect) {
        if self.is_line() {
            // Keep the line's direction, fit it into the new rect.
            let sx = if self.size[0] < 0.0 { -1.0 } else { 1.0 };
            let sy = if self.size[1] < 0.0 { -1.0 } else { 1.0 };
            let start = Pos2::new(
                if sx > 0.0 { r.min.x } else { r.max.x },
                if sy > 0.0 { r.min.y } else { r.max.y },
            );
            self.pos = [start.x, start.y];
            self.size = [r.width() * sx, r.height() * sy];
        } else {
            self.pos = [r.min.x, r.min.y];
            self.size = [r.width(), r.height()];
        }
    }

    pub fn center(&self) -> Pos2 {
        self.rect().center()
    }

    pub fn icon(&self) -> &'static str {
        use egui_phosphor::regular as ph;
        match self.kind {
            ItemKind::Image { .. } => ph::IMAGE,
            ItemKind::Text { .. } => ph::TEXT_T,
            ItemKind::Shape { .. } => ph::SHAPES,
            ItemKind::Line { .. } => ph::LINE_SEGMENT,
            ItemKind::Table { .. } => ph::TABLE,
            ItemKind::Code { .. } => ph::CODE,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Slide {
    pub name: String,
    pub items: Vec<Item>,
    /// Notes pinned to the page; shown while editing, never drawn or exported.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<Comment>,
    /// This slide's page colour; `None` uses the document's (see [`Document::background_of`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<[u8; 4]>,
}

impl Default for Slide {
    fn default() -> Self {
        Self { name: "Untitled".into(), items: Vec::new(), comments: Vec::new(), background: None }
    }
}

/// A comment pinned to a point on the page (document px), shown as a speech bubble.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Comment {
    pub id: ItemId,
    pub pos: [f32; 2],
    pub text: String,
    /// The signed-in name of whoever wrote it, if any.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author: String,
    /// When it was posted or last edited, in ms since the epoch.
    #[serde(default)]
    pub modified: i64,
}

impl Comment {
    pub fn pos(&self) -> Pos2 {
        Pos2::new(self.pos[0], self.pos[1])
    }
}

impl Slide {
    /// See [`Item::sanitize`].
    pub fn sanitize(&mut self) {
        for it in &mut self.items {
            it.sanitize();
        }
        for c in &mut self.comments {
            for v in &mut c.pos {
                fix(v, 0.0, -MAX_COORD..=MAX_COORD);
            }
        }
    }

    /// See [`Document::upgrade`].
    pub fn upgrade(&mut self) {
        self.sanitize();
        for it in &mut self.items {
            if let ItemKind::Line { kind, style, .. } = &mut it.kind {
                match kind {
                    LineKind::LegacyDashed => { *kind = LineKind::Straight; *style = LineStyle::Dashed; }
                    LineKind::LegacyDashedArrow => { *kind = LineKind::Arrow; *style = LineStyle::Dashed; }
                    _ => {}
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Unit {
    Inch,
    Cm,
    Px,
}

impl Unit {
    pub const ALL: [Unit; 3] = [Unit::Inch, Unit::Cm, Unit::Px];
    pub fn label(self) -> &'static str {
        match self {
            Unit::Inch => "in",
            Unit::Cm => "cm",
            Unit::Px => "px",
        }
    }
    pub fn from_px(self, px: f32) -> f32 {
        match self {
            Unit::Inch => px / DPI,
            Unit::Cm => px / DPI * 2.54,
            Unit::Px => px,
        }
    }
    pub fn to_px(self, v: f32) -> f32 {
        match self {
            Unit::Inch => v * DPI,
            Unit::Cm => v / 2.54 * DPI,
            Unit::Px => v,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Document {
    pub title: String,
    pub slides: Vec<Slide>,
    pub width: f32,
    pub height: f32,
    pub unit: Unit,
    /// The page colour of slides that don't have their own.
    pub background: [u8; 4],
    pub next_id: ItemId,
    /// Ruler guides, shared by every slide. Shown only while editing.
    #[serde(default)]
    pub guides: Vec<Guide>,
}

/// A guide line across the page: at `pos` px from the left (vertical) or top (horizontal).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct Guide {
    pub vertical: bool,
    pub pos: f32,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            title: "Untitled".into(),
            slides: vec![Slide::default()],
            width: 10.0 * DPI,
            height: 7.0 * DPI,
            unit: Unit::Inch,
            background: [255, 255, 255, 255],
            next_id: 1,
            guides: Vec::new(),
        }
    }
}

impl Document {
    /// Bring a document saved by an earlier version up to date: dashed line kinds become a
    /// straight / arrow line with the dashed style, and numbers out of range are put back in it
    /// (see [`Self::sanitize`]). Call after deserializing.
    pub fn upgrade(&mut self) {
        self.sanitize_page();
        for s in &mut self.slides {
            s.upgrade();
        }
    }

    /// Put every number back in range (see [`Item::sanitize`]). Done on load and before saving,
    /// so a document never holds a number its JSON can't round-trip.
    pub fn sanitize(&mut self) {
        self.sanitize_page();
        for s in &mut self.slides {
            s.sanitize();
        }
    }

    fn sanitize_page(&mut self) {
        let d = Document::default();
        fix(&mut self.width, d.width, MIN_PAGE..=MAX_COORD);
        fix(&mut self.height, d.height, MIN_PAGE..=MAX_COORD);
        for g in &mut self.guides {
            fix(&mut g.pos, 0.0, -MAX_COORD..=MAX_COORD);
        }
    }

    pub fn alloc_id(&mut self) -> ItemId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// The page colour of `slide`.
    pub fn background_of(&self, slide: &Slide) -> [u8; 4] {
        slide.background.unwrap_or(self.background)
    }

    pub fn page_rect(&self) -> Rect {
        Rect::from_min_size(Pos2::ZERO, Vec2::new(self.width, self.height))
    }
}

pub fn c32(c: [u8; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
}

pub fn c4(c: Color32) -> [u8; 4] {
    c.to_srgba_unmultiplied()
}

/// How far an image item's proportions may be from a picture's and still count as sized for it.
const ASPECT_TOLERANCE: f32 = 0.02;

/// Where an image item placed when its SVG was drawn whole (the artboard and all) goes now that
/// the file is drawn trimmed to `content`, the drawing's box on the `artboard` (both in the
/// file's units): the part of the item's rect that the drawing covers, with the crop
/// re-expressed on the drawing, so that the drawing stays exactly where it was. The rect is
/// moved so that turning it about its own centre, as the canvas does, lands where the old
/// rect's rotation put it. `None` when the item is not sized for the whole artboard (it already
/// frames the drawing, or was stretched on purpose), or when the crop shows none of the drawing.
pub fn fit_to_drawing(it: &Item, artboard: Vec2, content: Rect) -> Option<(Rect, [f32; 4])> {
    let ItemKind::Image { crop, .. } = &it.kind else { return None };
    let r = it.rect();
    let (cw, ch) = ((crop[2] - crop[0]).max(1e-3), (crop[3] - crop[1]).max(1e-3));
    if r.width() <= 0.0 || r.height() <= 0.0 || artboard.min_elem() <= 0.0 || content.width() <= 0.0 || content.height() <= 0.0 {
        return None;
    }
    let for_artboard = (artboard.x * cw) / (artboard.y * ch);
    if ((r.width() / r.height()) / for_artboard - 1.0).abs() > ASPECT_TOLERANCE {
        return None;
    }
    let x = fit_axis(r.min.x, r.width(), crop[0], crop[2], it.flip_h, artboard.x, content.min.x, content.max.x)?;
    let y = fit_axis(r.min.y, r.height(), crop[1], crop[3], it.flip_v, artboard.y, content.min.y, content.max.y)?;
    let shown = Rect::from_min_size(Pos2::new(x.0, y.0), Vec2::new(x.1, y.1));
    let center = r.center() + egui::emath::Rot2::from_angle(it.rotation) * (shown.center() - r.center());
    Some((Rect::from_center_size(center, shown.size()), [x.2, y.2, x.3, y.3]))
}

/// One axis of [`fit_to_drawing`]: the item spans `len` from `min` and shows texture
/// coordinates `c0..c1` of the artboard (mirrored when `flip`); the drawing spans `d0..d1` of
/// the artboard's `art`. The part of the item the drawing covers (start, length) and the crop
/// of the drawing it shows.
fn fit_axis(min: f32, len: f32, c0: f32, c1: f32, flip: bool, art: f32, d0: f32, d1: f32) -> Option<(f32, f32, f32, f32)> {
    let cl = (c1 - c0).max(1e-3);
    // Page position of a texture coordinate.
    let page = |t: f32| if flip { min + (c1 - t) / cl * len } else { min + (t - c0) / cl * len };
    let (mut a, mut b) = (page(d0 / art), page(d1 / art));
    if flip {
        std::mem::swap(&mut a, &mut b);
    }
    let (s0, s1) = (a.max(min), b.min(min + len));
    if s1 - s0 <= 1e-3 {
        return None;
    }
    let dl = b - a;
    let crop = if flip { ((b - s1) / dl, (b - s0) / dl) } else { ((s0 - a) / dl, (s1 - a) / dl) };
    Some((s0, s1 - s0, crop.0, crop.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bold_chars(kind: &ItemKind) -> String {
        let ItemKind::Text { text, .. } = kind else { return String::new() };
        text.chars().zip(char_looks(kind, text)).filter(|(_, l)| l.bold).map(|(c, _)| c).collect()
    }

    /// Bolding some letters bolds only them; typing keeps them bold and moves them along.
    #[test]
    fn letter_styles_follow_edits() {
        let mut it = Item::new(1, "t", ItemKind::text("hello world", 20.0, [0, 0, 0, 255], TextAlign::Left, false), Pos2::ZERO, Vec2::splat(100.0));
        set_flag(&mut it.kind, Some(6..11), TextFlag::Bold, true);
        assert_eq!(bold_chars(&it.kind), "world");
        assert!(flag_all(&it.kind, Some(7..9), TextFlag::Bold) && !flag_all(&it.kind, None, TextFlag::Bold));
        for (old, new) in [("hello world", "oh, hello world"), ("oh, hello world", "oh, hello worlds")] {
            let ItemKind::Text { text, .. } = &mut it.kind else { unreachable!() };
            *text = new.into();
            it.remap_text_runs(old);
        }
        // The 's' typed after the bold word takes its style.
        assert_eq!(bold_chars(&it.kind), "worlds");
        set_flag(&mut it.kind, Some(10..12), TextFlag::Bold, false);
        assert_eq!(bold_chars(&it.kind), "rlds");
        // Styling the whole box overrides the letters'.
        set_flag(&mut it.kind, None, TextFlag::Bold, false);
        assert_eq!(bold_chars(&it.kind), "");
        assert!(matches!(&it.kind, ItemKind::Text { runs, .. } if runs.is_empty()));
        set_color(&mut it.kind, Some(0..2), [255, 0, 0, 255]);
        assert_eq!(color_at(&it.kind, Some(0..1)), [255, 0, 0, 255]);
        assert_eq!(color_at(&it.kind, None), [0, 0, 0, 255]);
        let ItemKind::Text { text, .. } = &it.kind else { unreachable!() };
        assert_eq!(text_sections(&it.kind, text).len(), 2);
    }

    /// The Font menu is alphabetical and offers every font once.
    #[test]
    fn font_menu_is_alphabetical_and_complete() {
        let labels: Vec<String> = TextFont::ALL.iter().map(|f| f.label().to_lowercase()).collect();
        assert!(labels.windows(2).all(|w| w[0] < w[1]), "{labels:?}");
        assert_eq!(TextFont::ALL.len(), TextFont::EMBEDDED.len() + 2);
        assert!(TextFont::EMBEDDED.iter().all(|(f, ..)| TextFont::ALL.contains(f)));
    }

    fn image_item(pos: Pos2, size: Vec2, crop: [f32; 4]) -> Item {
        Item::new(1, "i", ItemKind::Image { source: ImageSource::file("a.svg".into()), crop, overlay: [0; 4], layers: Vec::new() }, pos, size)
    }

    fn close(r: Rect, want: Rect) -> bool {
        (r.min - want.min).length() < 1e-3 && (r.max - want.max).length() < 1e-3
    }

    /// An item sized for a 100 x 100 artboard whose drawing is the left half: the item's rect
    /// shrinks onto the drawing and its crop is re-expressed on the drawing; flips and rotation
    /// keep the drawing where it was; an item not sized for the artboard is left alone.
    #[test]
    fn fits_items_onto_the_drawing() {
        let art = Vec2::splat(100.0);
        let drawing = Rect::from_min_max(Pos2::ZERO, Pos2::new(50.0, 100.0));
        let full = [0.0, 0.0, 1.0, 1.0];

        let (r, crop) = fit_to_drawing(&image_item(Pos2::ZERO, Vec2::splat(200.0), full), art, drawing).unwrap();
        assert!(close(r, Rect::from_min_max(Pos2::ZERO, Pos2::new(100.0, 200.0))), "{r:?}");
        assert_eq!(crop, full);

        // The right half of the artboard shown (crop), of which the drawing covers nothing.
        assert!(fit_to_drawing(&image_item(Pos2::ZERO, Vec2::new(100.0, 200.0), [0.5, 0.0, 1.0, 1.0]), art, drawing).is_none());
        // The middle half shown: the drawing covers the left half of the item, i.e. its right half.
        let (r, crop) = fit_to_drawing(&image_item(Pos2::ZERO, Vec2::new(100.0, 200.0), [0.25, 0.0, 0.75, 1.0]), art, drawing).unwrap();
        assert!(close(r, Rect::from_min_max(Pos2::ZERO, Pos2::new(50.0, 200.0))), "{r:?}");
        assert_eq!(crop, [0.5, 0.0, 1.0, 1.0]);

        // Mirrored: the drawing is on the item's right.
        let mut flipped = image_item(Pos2::ZERO, Vec2::splat(200.0), full);
        flipped.flip_h = true;
        let (r, crop) = fit_to_drawing(&flipped, art, drawing).unwrap();
        assert!(close(r, Rect::from_min_max(Pos2::new(100.0, 0.0), Pos2::new(200.0, 200.0))), "{r:?}");
        assert_eq!(crop, full);
        flipped.kind = ItemKind::Image { source: ImageSource::file("a.svg".into()), crop: [0.25, 0.0, 0.75, 1.0], overlay: [0; 4], layers: Vec::new() };
        flipped.size = [100.0, 200.0];
        let (r, crop) = fit_to_drawing(&flipped, art, drawing).unwrap();
        assert!(close(r, Rect::from_min_max(Pos2::new(50.0, 0.0), Pos2::new(100.0, 200.0))), "{r:?}");
        assert_eq!(crop, [0.5, 0.0, 1.0, 1.0]);

        // Turned a quarter: the drawing's centre, (50, 100) before turning about (100, 100), is
        // at (100, 50) on the page, and that is the new rect's centre.
        let mut turned = image_item(Pos2::ZERO, Vec2::splat(200.0), full);
        turned.rotation = std::f32::consts::FRAC_PI_2;
        let (r, _) = fit_to_drawing(&turned, art, drawing).unwrap();
        assert!(close(r, Rect::from_center_size(Pos2::new(100.0, 50.0), Vec2::new(100.0, 200.0))), "{r:?}");

        // Already the drawing's shape, or stretched: not sized for the artboard.
        assert!(fit_to_drawing(&image_item(Pos2::ZERO, Vec2::new(100.0, 200.0), full), art, drawing).is_none());
        assert!(fit_to_drawing(&image_item(Pos2::ZERO, Vec2::new(150.0, 200.0), full), art, drawing).is_none());
        // Within tolerance of the artboard counts.
        assert!(fit_to_drawing(&image_item(Pos2::ZERO, Vec2::new(201.0, 200.0), full), art, drawing).is_some());
        // Not an image.
        assert!(fit_to_drawing(&Item::new(2, "t", ItemKind::text("x", 12.0, [0; 4], TextAlign::Left, false), Pos2::ZERO, Vec2::splat(100.0)), art, drawing).is_none());
    }

    /// Documents saved before line styles existed used dashed / elbow / inhibit kinds.
    #[test]
    fn legacy_line_kinds_upgrade() {
        let json = r#"{"title":"t","slides":[{"name":"s","items":[
            {"id":1,"name":"a","kind":{"Line":{"kind":"Dashed","color":[0,0,0,255],"width":2.0}},"pos":[0,0],"size":[10,0],"rotation":0,"flip_h":false,"flip_v":false,"locked":false,"opacity":1,"group":null},
            {"id":2,"name":"b","kind":{"Line":{"kind":"DashedArrow","color":[0,0,0,255],"width":2.0}},"pos":[0,0],"size":[10,0],"rotation":0,"flip_h":false,"flip_v":false,"locked":false,"opacity":1,"group":null},
            {"id":3,"name":"c","kind":{"Line":{"kind":"ElbowArrow","color":[0,0,0,255],"width":2.0}},"pos":[0,0],"size":[10,5],"rotation":0,"flip_h":false,"flip_v":false,"locked":false,"opacity":1,"group":null},
            {"id":4,"name":"d","kind":{"Line":{"kind":"Inhibit","color":[0,0,0,255],"width":2.0}},"pos":[0,0],"size":[10,0],"rotation":0,"flip_h":false,"flip_v":false,"locked":false,"opacity":1,"group":null}
        ]}],"width":960,"height":672,"unit":"Inch","background":[255,255,255,255],"next_id":5}"#;
        let mut doc: Document = serde_json::from_str(json).unwrap();
        doc.upgrade();
        let kinds: Vec<(LineKind, LineStyle)> = doc.slides[0]
            .items
            .iter()
            .map(|i| match i.kind {
                ItemKind::Line { kind, style, .. } => (kind, style),
                _ => panic!("line"),
            })
            .collect();
        assert_eq!(kinds[0], (LineKind::Straight, LineStyle::Dashed));
        assert_eq!(kinds[1], (LineKind::Arrow, LineStyle::Dashed));
        assert_eq!(kinds[2], (LineKind::BentArrow, LineStyle::Solid));
        assert_eq!(kinds[3], (LineKind::Inhibitor, LineStyle::Solid));
        // Round-trips under the new names.
        let again: Document = serde_json::from_str(&serde_json::to_string(&doc).unwrap()).unwrap();
        assert_eq!(again, doc);
    }

    /// An infinite number saved as `null` used to make the document fail to load.
    #[test]
    fn null_numbers_load_and_never_save() {
        let mut it = Item::new(1, "r", ItemKind::Shape { shape: ShapeKind::Rectangle, fill: [0; 4], stroke: [0; 4], stroke_width: 2.0 }, Pos2::new(5.0, 6.0), Vec2::splat(50.0));
        it.rotation = f32::INFINITY;
        it.opacity = f32::NAN;
        it.pos[1] = f32::NEG_INFINITY;
        let mut doc = Document::default();
        doc.slides[0].items.push(it);
        let damaged = serde_json::to_string_pretty(&doc).unwrap();
        assert!(damaged.contains("null") && serde_json::from_str::<Document>(&damaged).is_err());

        let mut loaded: Document = from_json(&damaged).unwrap();
        loaded.upgrade();
        let it = &loaded.slides[0].items[0];
        assert_eq!((it.rotation, it.opacity, it.pos), (0.0, 1.0, [5.0, 0.0]));
        // `group: null` is a real null and stays one.
        assert_eq!(it.group, None);

        doc.sanitize();
        let json = serde_json::to_string(&doc).unwrap();
        assert_eq!(serde_json::from_str::<Document>(&json).unwrap(), doc);
        // Other errors still fail.
        assert!(from_json::<Document>(r#"{"title": null}"#).is_err());
    }

    #[test]
    fn sanitize_clamps() {
        let mut doc = Document { width: 1.0, ..Default::default() };
        doc.slides[0].items.push(Item::new(1, "t", ItemKind::text("x", 1.0e5, [0; 4], TextAlign::Left, false), Pos2::new(1.0e30, 0.0), Vec2::splat(10.0)));
        doc.sanitize();
        assert_eq!(doc.width, MIN_PAGE);
        let it = &doc.slides[0].items[0];
        assert!(matches!(it.kind, ItemKind::Text { size, .. } if size == MAX_FONT_SIZE));
        assert_eq!(it.pos[0], MAX_COORD);
    }

    #[test]
    fn freehand_follows_its_box() {
        let kind = ItemKind::Line { kind: LineKind::Freehand, color: [0, 0, 0, 255], width: 2.0, style: LineStyle::Solid, points: vec![[0.0, 0.0], [0.5, 1.0], [1.0, 0.25]] };
        let mut it = Item::new(1, "d", kind, Pos2::new(10.0, 20.0), Vec2::new(100.0, 40.0));
        assert_eq!(crate::canvas::line_points(&it), vec![Pos2::new(10.0, 20.0), Pos2::new(60.0, 60.0), Pos2::new(110.0, 30.0)]);
        it.set_rect(Rect::from_min_size(Pos2::ZERO, Vec2::new(200.0, 80.0)));
        assert_eq!(crate::canvas::line_points(&it)[1], Pos2::new(100.0, 80.0));
        let again: Item = serde_json::from_str(&serde_json::to_string(&it).unwrap()).unwrap();
        assert_eq!(again, it);
        // The handles sit on the stroke's ends; moving one turns and scales the drawing about the other.
        let [a, b] = crate::canvas::line_ends(&it);
        assert_eq!((a, b), (Pos2::new(0.0, 0.0), Pos2::new(200.0, 20.0)));
        let moved = crate::canvas::move_path_end(&crate::canvas::line_points(&it), true, Pos2::new(-20.0, 200.0));
        assert_eq!(moved[0], a);
        assert!((moved[2] - Pos2::new(-20.0, 200.0)).length() < 1e-3);
        assert!((moved[1] - Pos2::new(-80.0, 100.0)).length() < 1e-3);
        // Other lines don't save an empty path.
        let line = Item::new(2, "l", ItemKind::Line { kind: LineKind::Arrow, color: [0, 0, 0, 255], width: 2.0, style: LineStyle::Solid, points: Vec::new() }, Pos2::ZERO, Vec2::X);
        assert!(!serde_json::to_string(&line).unwrap().contains("points"));
    }
}
