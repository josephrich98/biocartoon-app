//! Document model: a deck of slides, each a list of canvas items. Serialised as JSON
//! (`*.biocartoon`). Coordinates are document pixels at 96 dpi (1 in = 96 px).

use egui::{Color32, Pos2, Rect, Vec2};
use serde::{Deserialize, Serialize};

pub use crate::code::{CodeLang, CodeTheme};

pub const DPI: f32 = 96.0;
pub type ItemId = u64;

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
/// `assets/fonts` (see [`TextFont::EMBEDDED`]).
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
}

impl TextFont {
    pub const ALL: [TextFont; 12] = [
        TextFont::Sans,
        TextFont::Arimo,
        TextFont::Roboto,
        TextFont::OpenSans,
        TextFont::Lato,
        TextFont::Montserrat,
        TextFont::Tinos,
        TextFont::SourceSerif,
        TextFont::EbGaramond,
        TextFont::ComicNeue,
        TextFont::Mono,
        TextFont::Cousine,
    ];

    /// The embedded fonts: (variant, family name, TTF bytes). The family name is both the egui
    /// `FontFamily::Name` and the name in the font file, which the SVG export refers to.
    pub const EMBEDDED: [(TextFont, &'static str, &'static [u8]); 10] = [
        (TextFont::Arimo, "Arimo", include_bytes!("../assets/fonts/Arimo-Regular.ttf")),
        (TextFont::Tinos, "Tinos", include_bytes!("../assets/fonts/Tinos-Regular.ttf")),
        (TextFont::Cousine, "Cousine", include_bytes!("../assets/fonts/Cousine-Regular.ttf")),
        (TextFont::Roboto, "Roboto", include_bytes!("../assets/fonts/Roboto-Regular.ttf")),
        (TextFont::OpenSans, "Open Sans", include_bytes!("../assets/fonts/OpenSans-Regular.ttf")),
        (TextFont::Lato, "Lato", include_bytes!("../assets/fonts/Lato-Regular.ttf")),
        (TextFont::Montserrat, "Montserrat", include_bytes!("../assets/fonts/Montserrat-Regular.ttf")),
        (TextFont::SourceSerif, "Source Serif 4", include_bytes!("../assets/fonts/SourceSerif4-Regular.ttf")),
        (TextFont::EbGaramond, "EB Garamond", include_bytes!("../assets/fonts/EBGaramond-Regular.ttf")),
        (TextFont::ComicNeue, "Comic Neue", include_bytes!("../assets/fonts/ComicNeue-Regular.ttf")),
    ];

    pub fn label(self) -> &'static str {
        match self {
            TextFont::Sans => "Ubuntu",
            TextFont::Mono => "Hack (monospace)",
            TextFont::Arimo => "Arimo (Arial / Helvetica)",
            TextFont::Tinos => "Tinos (Times New Roman)",
            TextFont::Cousine => "Cousine (Courier New)",
            TextFont::Roboto => "Roboto",
            TextFont::OpenSans => "Open Sans",
            TextFont::Lato => "Lato",
            TextFont::Montserrat => "Montserrat",
            TextFont::SourceSerif => "Source Serif",
            TextFont::EbGaramond => "EB Garamond",
            TextFont::ComicNeue => "Comic Neue",
        }
    }

    /// The embedded font's family name, or `None` for egui's own two.
    pub fn embedded_name(self) -> Option<&'static str> {
        Self::EMBEDDED.iter().find(|(f, ..)| *f == self).map(|(_, name, _)| *name)
    }

    pub fn font_id(self, px: f32) -> egui::FontId {
        match self {
            TextFont::Sans => egui::FontId::proportional(px),
            TextFont::Mono => egui::FontId::monospace(px),
            f => egui::FontId::new(px, egui::FontFamily::Name(f.embedded_name().unwrap_or_default().into())),
        }
    }

    /// Adds the embedded fonts to `defs`, each falling back to egui's proportional fonts for
    /// glyphs it lacks. Every context that lays out text items needs this.
    pub fn install(defs: &mut egui::FontDefinitions) {
        let fallback = defs.families.get(&egui::FontFamily::Proportional).cloned().unwrap_or_default();
        for (_, name, bytes) in Self::EMBEDDED {
            defs.font_data.insert(name.into(), std::sync::Arc::new(egui::FontData::from_static(bytes)));
            let mut chain = vec![name.to_string()];
            chain.extend(fallback.iter().cloned());
            defs.families.insert(egui::FontFamily::Name(name.into()), chain);
        }
    }
}

fn one() -> f32 {
    1.0
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ItemKind {
    Image {
        source: ImageSource,
        /// Crop in uv space: [left, top, right, bottom], each 0..1.
        crop: [f32; 4],
        /// Colour overlay (rgb) and its strength (a). a = 0 means no overlay.
        overlay: [u8; 4],
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
        font: TextFont,
        /// Multiple of the font's natural row height.
        #[serde(default = "one")]
        line_spacing: f32,
        #[serde(default)]
        valign: TextVAlign,
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
        ItemKind::Text { text: text.into(), size, color, align, bold, italic: false, underline: false, font: TextFont::Sans, line_spacing: 1.0, valign: TextVAlign::Top }
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
        }
    }

    pub fn is_line(&self) -> bool {
        matches!(self.kind, ItemKind::Line { .. })
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
}

impl Default for Slide {
    fn default() -> Self {
        Self { name: "Untitled".into(), items: Vec::new() }
    }
}

impl Slide {
    /// See [`Document::upgrade`].
    pub fn upgrade(&mut self) {
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
    /// straight / arrow line with the dashed style. Call after deserializing.
    pub fn upgrade(&mut self) {
        for s in &mut self.slides {
            s.upgrade();
        }
    }

    pub fn alloc_id(&mut self) -> ItemId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn page_rect(&self) -> Rect {
        Rect::from_min_size(Pos2::ZERO, Vec2::new(self.width, self.height))
    }

    pub fn size_label(&self) -> String {
        let u = self.unit;
        match u {
            Unit::Px => format!("{} x {} px", self.width.round(), self.height.round()),
            _ => format!("{:.2} x {:.2} {}", u.from_px(self.width), u.from_px(self.height), u.label())
                .replace(".00 ", " "),
        }
    }
}

pub fn c32(c: [u8; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
}

pub fn c4(c: Color32) -> [u8; 4] {
    c.to_srgba_unmultiplied()
}

#[cfg(test)]
mod tests {
    use super::*;

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
