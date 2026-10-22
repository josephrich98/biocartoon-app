//! Application state, top-level layout, commands (undo, clipboard, file IO, AI dispatch).

use crate::ai::{AiClient, AiJob, AiOp, AiPrompts, AiResult, BackendKind};
use crate::assets::{ImageCache, ImgState};
use crate::catalog::{cached_catalog, AssetRow, CatalogClient, CatalogSync, CategoryInfo, Facets, Filter, IconSort, Req, Resp, SourceInfo, StarUpdate};
use crate::stars::{StarsClient, StarsMsg};
use crate::export::{ExportFormat, ExportOptions};
use crate::doc::{CodeLang, CodeTheme, Document, ImageSource, Item, ItemId, ItemKind, LineKind, LineStyle, ShapeKind, Slide, TextAlign, TextFont, Unit, DPI};
use crate::home::{HomeState, HomeView, SortKey};
use crate::submit::{RateLimit, SubmitMeta, DEFAULT_SUBMIT_URL};
use crate::templates::{TemplateFile, TemplateMeta, TemplateRow, TemplateStore, TemplateSync};
use crate::projects::{Snapshot, Store};
use crate::theme;
use egui::{Color32, Pos2, Rect, Vec2};
use egui_phosphor::regular as ph;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Public URL of the R2 bucket: images at `<url>/<png_key>`, the catalog under `<url>/catalog/`.
pub use biocartoon_catalog::DEFAULT_ASSET_URL;
/// Bumped when stored path settings must be reset to the new defaults.
const SETTINGS_VERSION: u32 = 4;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Missing in settings saved before versioning, so those read as 0.
    #[serde(default)]
    pub version: u32,
    pub asset_base_url: String,
    /// Empty: use the catalog downloaded from `asset_base_url`. Otherwise a local `assets.sqlite`.
    pub catalog_path: String,
    /// Optional folder holding the downloaded catalog files (`png_local` paths); used before the URL.
    pub local_data_dir: String,
    /// Desktop: where offline packs and fetched images are kept (empty: `<app data>/offline`).
    pub offline_dir: String,
    pub ai_provider: BackendKind,
    pub ai_model: String,
    pub api_key: String,
    /// The vLLM server's URL (only used with `BackendKind::Vllm`).
    pub ai_base_url: String,
    /// The prompt text sent with AI requests, editable in the AI settings.
    pub ai_prompts: AiPrompts,
    /// Every image generated with AI, most recent first (the Generate page's history).
    pub generated: Vec<GeneratedImage>,
    /// Generated images go into the user's library as soon as they arrive.
    pub auto_library_generated: bool,
    pub uploads: Vec<String>,
    /// Ids of the assets the user starred (their favorites), most recent first. This is what
    /// counts as "mine": the star counts on the Worker follow it (see `pending_stars`).
    pub favorites: Vec<String>,
    /// Star changes the Worker has not confirmed yet (asset id, starred), sent again when it can
    /// be reached, so favorites work offline.
    pub pending_stars: Vec<(String, bool)>,
    pub grid_snap: bool,
    /// Moved and resized items snap to guides.
    pub guide_snap: bool,
    pub show_grid: bool,
    pub grid_size: f32,
    /// Unit of the custom grid spacing.
    pub grid_unit: Unit,
    /// The grid spacing is a custom value rather than one of `GRID_PRESETS`.
    pub grid_custom: bool,
    pub theme: theme::ThemeMode,
    pub text_size: theme::TextSize,
    /// The Icons panel on the left is open.
    pub show_icons: bool,
    /// Number of tile columns in the Icons panel (1 to 5).
    pub icon_columns: usize,
    /// Icon tiles show their database's badge (lower left).
    pub tile_database: bool,
    /// Icon tiles that come in several colours show a colour wheel (lower right).
    pub tile_colors: bool,
    /// Icon tiles show their star count (lower right).
    pub tile_stars: bool,
    pub export: ExportOptions,
    pub home_sort: SortKey,
    pub home_ascending: bool,
    pub home_view: HomeView,
    /// Order of browse results.
    pub icon_sort: IconSort,
    /// Order of search results.
    pub search_sort: IconSort,
    /// Overrides the submission Worker's URL (empty: `BIOCARTOON_SUBMIT_URL`, then the default).
    pub submit_url: String,
    /// Public credit for contributions (empty: "anonymous").
    pub contributor_name: String,
    /// Random id of this install, so the Worker counts its stars once (made on first run).
    pub client_id: String,
    /// Words added to the spelling dictionary ("Add to dictionary").
    pub custom_words: Vec<String>,
    /// The spelling check also looks for grammar mistakes.
    pub check_grammar: bool,
    /// Squiggles under spelling and grammar mistakes while editing (main menu).
    pub show_spelling: bool,
    /// Web: the home screen's backup reminder stays hidden until then (Unix milliseconds).
    pub backup_snoozed_until: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            asset_base_url: std::env::var("BIOCARTOON_ASSET_URL").unwrap_or_else(|_| DEFAULT_ASSET_URL.into()),
            catalog_path: std::env::var("BIOCARTOON_DB").unwrap_or_default(),
            local_data_dir: String::new(),
            offline_dir: String::new(),
            ai_provider: BackendKind::Anthropic,
            ai_model: BackendKind::Anthropic.models()[0].into(),
            api_key: String::new(),
            ai_base_url: String::new(),
            ai_prompts: AiPrompts::default(),
            generated: Vec::new(),
            auto_library_generated: false,
            uploads: Vec::new(),
            favorites: Vec::new(),
            pending_stars: Vec::new(),
            grid_snap: false,
            guide_snap: true,
            show_grid: false,
            grid_size: DPI / 4.0,
            grid_unit: Unit::Inch,
            grid_custom: false,
            theme: theme::ThemeMode::System,
            text_size: theme::TextSize::Medium,
            show_icons: true,
            icon_columns: 3,
            tile_database: true,
            tile_colors: true,
            tile_stars: true,
            export: ExportOptions::default(),
            home_sort: SortKey::LastOpened,
            home_ascending: false,
            home_view: HomeView::LargeTiles,
            icon_sort: IconSort::Alphabetical,
            search_sort: IconSort::Relevance,
            submit_url: String::new(),
            contributor_name: String::new(),
            client_id: String::new(),
            custom_words: Vec::new(),
            check_grammar: true,
            show_spelling: false,
            backup_snoozed_until: 0,
        }
    }
}

/// An image generated with AI, saved under `<app data>/generated`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneratedImage {
    pub path: String,
    pub prompt: String,
    /// Its id in the user's library, once added there.
    pub library_id: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Screen {
    Home,
    Editor,
}

/// A finished background thumbnail render: cache key, and the image or an error.
type ThumbDone = (String, Result<image::RgbaImage, String>);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingsTab {
    General,
    Ai,
    /// Offline packs (desktop) / bulk .zip downloads (web).
    Offline,
}

/// The "Connect an AI model" dialog, shown the first time an AI tool is used.
pub struct AiSetup {
    pub provider: BackendKind,
    pub model: String,
    pub key: String,
    pub base_url: String,
    /// The AI action that opened the dialog; it runs once a model is connected.
    pub pending: Option<Action>,
}

/// The tabs of the rail on the right: tools that add to the slide, the properties of the
/// selection, and the slide list. The Icons panel on the left is separate (`Settings::show_icons`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RailTab {
    Edit,
    Text,
    Lines,
    Shapes,
    Table,
    Code,
    Slides,
    /// The page's size and colour, and the grid.
    Page,
}

impl RailTab {
    pub const ALL: [RailTab; 8] = [RailTab::Edit, RailTab::Text, RailTab::Lines, RailTab::Shapes, RailTab::Table, RailTab::Code, RailTab::Slides, RailTab::Page];
    pub fn label(self) -> &'static str {
        match self {
            RailTab::Text => "Text",
            RailTab::Lines => "Lines",
            RailTab::Shapes => "Shapes",
            RailTab::Table => "Table",
            RailTab::Code => "Code",
            RailTab::Edit => "Edit",
            RailTab::Slides => "Slides",
            RailTab::Page => "Page",
        }
    }
    pub fn icon(self) -> &'static str {
        match self {
            RailTab::Text => ph::TEXT_T,
            RailTab::Lines => ph::ARROW_RIGHT,
            RailTab::Shapes => ph::SHAPES,
            RailTab::Table => ph::TABLE,
            RailTab::Code => ph::CODE,
            RailTab::Edit => ph::SLIDERS_HORIZONTAL,
            RailTab::Slides => ph::PRESENTATION,
            RailTab::Page => ph::FRAME_CORNERS,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum IconsMode {
    Home,
    Search,
    Generate,
    /// Icons the user starred as favorites (kept in [`Settings::favorites`]).
    Favorites,
    /// "Your library": the user's own uploads and saved templates.
    Library,
    /// Image files from this computer.
    Uploads,
    /// Pick uploads and slides to submit to the shared library.
    Contribute,
    /// Ask the BioCartoon team for an icon or template.
    Suggest,
}

/// What a browse or search shows: catalog icons, saved templates, or both.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Include {
    Both,
    Icons,
    Templates,
}

impl Include {
    pub const ALL: [Include; 3] = [Include::Both, Include::Icons, Include::Templates];
    pub fn label(self) -> &'static str {
        match self {
            Include::Both => "Icons and templates",
            Include::Icons => "Icons",
            Include::Templates => "Templates",
        }
    }
    pub fn icons(self) -> bool {
        self != Include::Templates
    }
    pub fn templates(self) -> bool {
        self != Include::Icons
    }
}

/// The filters under the search box's funnel. Each one keeps what matches any of its entries
/// (none keeps everything), and a result has to pass all of them.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct SearchFilter {
    pub sources: Vec<String>,
    /// Parts of author names, separated by commas.
    pub authors: String,
    /// Categories of `crate::taxonomy`.
    pub cats: Vec<String>,
    /// (category, subcategory) of `crate::taxonomy`: narrows its category to these subcategories.
    pub subs: Vec<(String, String)>,
    /// A database's own categories: (database, category), shown as "BA: Viruses".
    pub db_cats: Vec<(String, String)>,
    pub licenses: Vec<String>,
    /// File types, `catalog::FORMAT_SVG` or `catalog::FORMAT_PNG`.
    pub formats: Vec<String>,
}

impl SearchFilter {
    pub fn authors(&self) -> Vec<String> {
        self.authors.split(',').map(|a| a.trim().to_lowercase()).filter(|a| !a.is_empty()).collect()
    }

    /// How many of the filters are set.
    pub fn active(&self) -> usize {
        [!self.sources.is_empty(), !self.authors().is_empty(), !self.cats.is_empty() || !self.subs.is_empty(), !self.db_cats.is_empty(), !self.licenses.is_empty(), !self.formats.is_empty()]
            .iter()
            .filter(|&&on| on)
            .count()
    }

    /// The catalog's form: a category with some of its subcategories picked is just those.
    pub fn catalog(&self) -> Filter {
        let mut taxa = Vec::new();
        for (cat, _) in crate::taxonomy::TAXONOMY {
            let subs: Vec<_> = self.subs.iter().filter(|(c, _)| c == cat).cloned().collect();
            if !subs.is_empty() {
                taxa.extend(subs);
            } else if self.cats.iter().any(|c| c == cat) {
                taxa.push((cat.to_string(), String::new()));
            }
        }
        Filter {
            sources: self.sources.clone(),
            authors: self.authors(),
            licenses: self.licenses.clone(),
            taxa,
            db_cats: self.db_cats.clone(),
            formats: self.formats.clone(),
        }
    }
}

/// How many icons a search or browse loads at a time; more load as the list scrolls.
const RESULTS_PAGE: usize = 120;

pub struct IconsState {
    pub mode: IconsMode,
    pub query: String,
    pub last_sent_query: String,
    pub filter: SearchFilter,
    pub last_sent_filter: SearchFilter,
    /// The filters are open under the search box.
    pub show_filters: bool,
    pub results: Vec<AssetRow>,
    /// How many colours the `results` that come in several have, by result id (the others are
    /// picked in Edit once one is placed).
    pub colors: HashMap<String, usize>,
    pub results_req: u64,
    pub more: bool,
    /// Every match of the current search or browse, not just the loaded `results`.
    pub total: Option<i64>,
    /// The filters' choices that find icons, for their dropdowns (kept while newer ones load).
    pub facets: Option<Facets>,
    /// The query and filter `facets` are for (or are loading for); `None` loads them again.
    pub facets_for: Option<(String, SearchFilter)>,
    pub facets_req: u64,
    pub browse_offset: usize,
    pub categories: HashMap<String, Vec<CategoryInfo>>,
    pub sources: Vec<SourceInfo>,
    pub loading: bool,
    pub error: Option<String>,
    /// Results: icons, templates or both.
    pub include: Include,
}

impl Default for IconsState {
    fn default() -> Self {
        Self {
            mode: IconsMode::Home,
            query: String::new(),
            last_sent_query: "\u{0}".into(),
            filter: SearchFilter::default(),
            last_sent_filter: SearchFilter::default(),
            show_filters: false,
            results: Vec::new(),
            colors: HashMap::new(),
            results_req: 0,
            more: false,
            total: None,
            facets: None,
            facets_for: None,
            facets_req: 0,
            browse_offset: 0,
            categories: HashMap::new(),
            sources: Vec::new(),
            loading: false,
            error: None,
            include: Include::Both,
        }
    }
}

#[derive(Default)]
pub struct GenerateState {
    pub prompt: String,
    pub status: String,
    pub busy: bool,
    pub result: Option<(image::RgbaImage, egui::TextureHandle)>,
    /// Where the shown result was saved (its entry in `Settings::generated`).
    pub result_path: Option<String>,
}

/// What the "add to library" dialog uploads.
#[derive(Clone, Debug, PartialEq)]
pub enum UploadTarget {
    /// An image file (from the Uploads tab).
    File(PathBuf),
    /// An icon of the user's library (with a local copy); the new copy replaces it.
    Library(Box<AssetRow>),
    /// A slide, as a template.
    Template(Box<TemplateSlide>),
}

/// A slide to save as a template, with the page it was drawn on.
#[derive(Clone, Debug, PartialEq)]
pub struct TemplateSlide {
    pub slide: Slide,
    pub width: f32,
    pub height: f32,
    pub background: [u8; 4],
    /// The saved template this is, when it comes from the user's library: its details fill in
    /// the library dialog, and the new copy replaces it.
    pub saved: Option<TemplateMeta>,
}

impl UploadTarget {
    /// The image file of an upload or library icon.
    pub fn image_path(&self) -> Option<String> {
        match self {
            UploadTarget::File(p) => Some(p.display().to_string()),
            UploadTarget::Library(row) => row.png_local.clone(),
            _ => None,
        }
    }
}

/// Read and decode an image to add to the library.
fn read_image(p: &Path) -> Result<image::RgbaImage, String> {
    crate::storage::read(p).map_err(|e| e.to_string()).and_then(|b| crate::assets::load_image(&b, crate::submit::MAX_IMAGE_SIDE)).map_err(|e| format!("{}: {e}", p.display()))
}

impl TemplateSlide {
    pub fn page_rect(&self) -> Rect {
        Rect::from_min_size(Pos2::ZERO, Vec2::new(self.width, self.height))
    }
}

/// The Icons panel's "Submit a suggestion" form.
#[derive(Default)]
pub struct SuggestState {
    pub template: bool,
    pub title: String,
    pub description: String,
    pub name: String,
    pub busy: bool,
}

/// The Icons panel's "Contribute artwork" page: what is picked to submit.
#[derive(Default)]
pub struct ContributeState {
    /// Picked uploads, by path.
    pub icons: HashSet<String>,
    /// Picked icons of the user's library, by id.
    pub library: HashSet<String>,
    /// Picked templates of the user's library, by id.
    pub library_templates: HashSet<String>,
    /// Picked slides of the open document, by index.
    pub slides: HashSet<usize>,
    /// Slides from .biocartoon files picked under Templates (kept for this session).
    pub templates: Vec<TemplateSlide>,
    /// Picked entries of `templates`, by index.
    pub picked_templates: HashSet<usize>,
}

/// The first is the default: CC BY, so artists are credited unless they choose not to be.
pub const UPLOAD_LICENSES: [(&str, &str); 2] = [("CC-BY-4.0", "CC BY 4.0 (users must credit you)"), ("CC0-1.0", "CC0 (no rights reserved, no credit needed)")];

pub struct UploadDialog {
    pub target: UploadTarget,
    pub name: String,
    pub category: String,
    pub description: String,
    pub license: &'static str,
    /// Also submit it to the shared library (reviewed before it is published).
    pub share: bool,
    /// Public credit name for a submission.
    pub credit: String,
    /// Picked items still to go through the dialog after this one.
    pub queue: Vec<UploadTarget>,
    /// Where this one is in the batch: (1-based position, batch size).
    pub step: (usize, usize),
    /// The preview's zoom (1: fit) and offset from centered, in screen points.
    pub view_zoom: f32,
    pub view_pan: Vec2,
}

/// Results of library work done on worker threads.
pub enum LibraryMsg {
    /// A submitted asset: its local row, and the submission id or the error.
    /// A submitted asset, the submission's id or error, and the library icon it replaces.
    AssetSubmitted { row: AssetRow, result: Result<String, String>, replaces: Option<String> },
    /// A saved template, and the library template it replaces.
    TemplateSaved(crate::templates::SaveResult, Option<String>),
    TemplateLoaded(Result<TemplateFile, String>, bool),
    /// A sent suggestion: its title, and the id or the error.
    SuggestionSent(String, Result<String, String>),
}

/// The Page panel's own state; the page's size, unit and colour live in the document.
#[derive(Clone, Default)]
pub struct PageState {
    pub lock_aspect: bool,
    /// Page width, height, unit and colour right after the panel's last edit, so that a burst of
    /// edits makes one undo step.
    pub last_edit: Option<(f32, f32, Unit, [u8; 4])>,
}

/// Where the "Edit with AI" conversation is: offering the quick edits, asking for a restyle's
/// style, or free text.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AiChatStage {
    Start,
    PickStyle,
}

/// The "Edit with AI" chat about one image.
pub struct AiEdit {
    pub item: ItemId,
    pub messages: Vec<(bool, String)>, // (from_user, text)
    pub input: String,
    pub waiting: bool,
    pub stage: AiChatStage,
}

pub const AI_EDIT_GREETING: &str = "How can I help?";

pub struct Toast {
    pub text: String,
    pub until: f64,
    pub error: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AlignKind {
    Left,
    CenterH,
    Right,
    Top,
    Middle,
    Bottom,
    DistributeH,
    DistributeV,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArrangeKind {
    Front,
    Forward,
    Backward,
    Back,
}

/// Deferred commands pushed by UI code and applied after the frame's layout.
#[derive(Clone, Debug)]
pub enum Action {
    InsertAsset(AssetRow),
    /// Insert an icon picked at random from the whole library.
    InsertRandom,
    /// Add or remove this install's star on an asset.
    /// Remove one of the user's own assets from their library (its PNG stays on disk).
    RemoveUserAsset(AssetRow),
    /// Star the asset with this id (add it to Favorites), or take the star back.
    ToggleFavorite(String),
    InsertFile(PathBuf),
    InsertText { text: String, size: f32, bold: bool },
    /// Arm (or disarm) the Text tool: the next click on the canvas places a text box.
    ArmText(bool),
    /// Place an empty text box with its first line at this page point and start typing in it.
    PlaceText(Pos2),
    /// Type into this text box on the canvas.
    EditText(ItemId),
    /// Type into a table's cell (row-major index) on the canvas.
    EditCell(ItemId, usize),
    InsertShape(ShapeKind),
    InsertLine(LineKind),
    /// Arm (or disarm) the Draw tool: drags on the canvas draw freehand lines until disarmed.
    ArmDraw(bool),
    /// Add a freehand line through these page points (a finished Draw stroke).
    PlaceDrawing(Vec<Pos2>),
    InsertTable(usize, usize),
    InsertCode { code: String, language: CodeLang, theme: CodeTheme, line_numbers: bool, window: bool, rounded: bool },
    UseTemplate(usize),
    /// A saved or shared template (from `templates.sqlite`).
    UseSavedTemplate(TemplateRow),
    RemoveTemplate(String),
    OpenUploadDialog(UploadTarget),
    PickLibraryFile,
    /// The image picked by `PickLibraryFile`.
    LibraryFilePicked(PathBuf),
    SubmitUpload,
    Select(Vec<ItemId>),
    ToggleSelect(ItemId),
    ClearSelection,
    Delete,
    Duplicate,
    Copy,
    Cut,
    Paste,
    Group,
    Ungroup,
    ToggleLock,
    Align(AlignKind),
    Arrange(ArrangeKind),
    FlipH,
    FlipV,
    /// Start (or end) cropping the selected image on the canvas.
    SetCrop(bool),
    Ai(AiOp),
    OpenAiEdit,
    SendChat(String),
    StartReplace,
    Undo,
    Redo,
    NewDoc,
    Open,
    Save,
    SaveAs,
    GoHome,
    OpenProject(String),
    RenameProject(String, String),
    CopyProject(String),
    ExportProject(String),
    SaveProjectFile(String),
    EmailProject(String),
    DeleteProject(String),
    RestoreVersion(String, i64),
    VersionHistory,
    OpenExport,
    Export(PathBuf),
    /// A `.biocartoon` file picked to import (from [`crate::platform::pick_files`]).
    OpenFile(PathBuf),
    /// Several files, or a backup .zip, to import as projects without opening any.
    ImportFiles(Vec<PathBuf>),
    /// Download every project as `.biocartoon` files in one .zip.
    BackupAll,
    /// Images picked for the Uploads panel.
    AddUploads(Vec<PathBuf>),
    /// Images dropped on the canvas: added to Uploads and placed on the page, centred on the
    /// drop point (page coordinates) when there is one.
    DropImages(Vec<PathBuf>, Option<Pos2>),
    /// Ask for .biocartoon files whose slides to offer under Contribute artwork > Templates.
    PickTemplateFiles,
    AddTemplateFiles(Vec<PathBuf>),
    /// Open the library dialog for everything picked under Contribute artwork, one at a time.
    ContributeSelected,
    /// Open Submit a suggestion, with this title when it is not empty.
    OpenSuggest(String),
    SubmitSuggestion,
    AddSlide,
    DuplicateSlide(usize),
    DeleteSlide(usize),
    MoveSlide(usize, isize),
    GoSlide(usize),
    Generate(String),
    /// Add the generated image at this path to the library (and, with `favorite`, Favorites).
    GeneratedToLibrary { path: String, favorite: bool },
    /// Open Contribute artwork with the generated image at this path picked.
    ContributeGenerated(String),
    PickUploads,
    RemoveUpload(usize),
    ZoomFit,
    ZoomTo(f32),
    Toast(String),
    SyncCatalog,
    SetRail(Option<RailTab>),
    Commit,
    /// Add a guide across the middle of the page (vertical or horizontal) and pick it.
    AddGuide(bool),
    ClearGuides,
}

/// How an image item placed before its picture had loaded is sized once it has: by the
/// picture's longer side (an insert) or by its area (a replacement keeps the old picture's).
enum Fit {
    Side(f32),
    Area(f32),
}

pub struct BioCartoon {
    pub doc: Document,
    pub cur_slide: usize,
    pub selection: Vec<ItemId>,
    pub history: Vec<(Document, usize)>,
    pub future: Vec<(Document, usize)>,
    pub clipboard: Vec<Item>,
    pub paste_count: u32,
    /// The primary button went down with Ctrl held on macOS, so it is being treated as a right click.
    ctrl_click: bool,
    /// A paste event came in since ⌘V went down. With an image on the system clipboard there is
    /// none, so the key's release pastes instead.
    pasted: bool,
    pub rail: Option<RailTab>,
    pub icons: IconsState,
    pub generate: GenerateState,
    pub catalog: CatalogClient,
    pub stars: StarsClient,
    /// Assets whose star change is on its way to the Worker.
    pub stars_in_flight: HashSet<String>,
    /// The stars from before favorites were kept in Settings have been carried over.
    stars_migrated: bool,
    /// Web: the optional account that keeps projects online.
    pub account: crate::account::Account,
    pub images: ImageCache,
    pub ai: AiClient,
    pub settings: Settings,
    pub show_settings: bool,
    /// The theme last applied: dark or not, and the text size.
    applied_theme: Option<(bool, theme::TextSize)>,
    pub show_about: bool,
    /// Spelling and grammar problems, kept up to date while squiggles or the window are shown.
    pub spell: crate::spell::SpellState,
    /// The Spelling and grammar window is open (it also shows squiggles while open).
    pub show_spell_window: bool,
    pub settings_tab: SettingsTab,
    pub ai_setup: Option<AiSetup>,
    pub catalog_sync: Option<CatalogSync>,
    pub offline: crate::offline::OfflineState,
    pub view: crate::canvas::View,
    pub page: PageState,
    pub ai_edit: Option<AiEdit>,
    /// The right-rail tab shown before a selection opened Edit, restored when it is cleared.
    pub rail_before_edit: Option<RailTab>,
    /// The tab the right panel showed last, which a drag in from the right edge reopens.
    pub last_rail: Option<RailTab>,
    pub replace_target: Option<ItemId>,
    /// Image items placed before their picture had loaded, to take its shape once it has.
    pending_fit: Vec<(ItemId, Fit)>,
    pub crop_mode: bool,
    /// The Text tool is armed: the next click on the canvas places a text box.
    pub placing_text: bool,
    /// The Draw tool is armed: drags on the canvas draw freehand lines.
    pub drawing: bool,
    /// The Draw tool's color and thickness.
    pub pen_color: [u8; 4],
    pub pen_width: f32,
    /// The page points of the stroke being drawn.
    pub stroke: Vec<Pos2>,
    /// The text box (or table) being typed into on the canvas.
    pub editing_text: Option<ItemId>,
    /// For a table in `editing_text`, the (row-major) cell being typed into.
    pub editing_cell: usize,
    /// Give the on-canvas text editor keyboard focus on its next frame.
    pub text_edit_focus: bool,
    pub toasts: Vec<Toast>,
    pub screen: Screen,
    ctx: egui::Context,
    pub store: Store,
    /// The open project's id; `None` for a new document not saved yet.
    pub project: Option<String>,
    /// The document as last written to the project (autosave compares against it).
    pub saved_doc: Option<Document>,
    pub last_autosave: f64,
    pub home: HomeState,
    thumb_tx: std::sync::mpsc::Sender<ThumbDone>,
    /// Actions from callbacks that finish later (file pickers in the browser).
    pub(crate) action_tx: std::sync::mpsc::Sender<Action>,
    action_rx: std::sync::mpsc::Receiver<Action>,
    thumb_rx: std::sync::mpsc::Receiver<ThumbDone>,
    /// Cache keys of thumbnails being rendered.
    pub thumbs_pending: HashSet<String>,
    /// Exporting a project from the home screen: its document (first slide is exported).
    pub export_doc: Option<Document>,
    /// The id of the project in `export_doc`, marked backed up once it is saved as a .biocartoon file.
    export_project: Option<String>,
    /// Emailing a project: subject and attachments, sent once the PNG export finishes.
    pub pending_email: Option<(String, Vec<PathBuf>)>,
    pub dirty: bool,
    pub actions: Vec<Action>,
    pub show_export: bool,
    pub export_job: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
    /// The export dialog's last file size, with the options it was worked out for.
    export_size: Option<(ExportOptions, Result<crate::export::SizeEstimate, String>)>,
    export_size_rx: Option<std::sync::mpsc::Receiver<(ExportOptions, Result<crate::export::SizeEstimate, String>)>>,
    pub variants: Vec<AssetRow>,
    pub variants_for: Option<String>,
    pub variants_req: u64,
    /// Asset ids whose credit has been asked of the catalog (see [`crate::credits::missing`]).
    pub credit_lookups: HashSet<String>,
    pub show_attributions: bool,
    /// The slide (0-based) the Attributions window is limited to, or the whole document.
    pub attributions_slide: Option<usize>,
    /// The credits of what was just exported, and the stem for saving them: the Attributions
    /// window shows these instead of the document's while set.
    pub attributions_export: Option<(Vec<crate::credits::Credit>, String)>,
    /// When the export whose credits the Attributions window shows finished, for the
    /// "Image saved successfully" banner above it.
    pub export_saved_at: Option<f64>,
    /// The credits of the export that is running, shown once it finishes.
    pub export_credits: Option<(Vec<crate::credits::Credit>, String)>,
    /// The image whose credit the Credit window shows (right-click > Show credit).
    pub credit_item: Option<ItemId>,
    /// The Attributions window lists only credits the licenses require.
    pub attributions_required_only: bool,
    pub drag: Option<crate::canvas::Drag>,
    /// The guide (index into `doc.guides`) picked on the canvas: Delete removes it.
    pub selected_guide: Option<usize>,
    pub app_dir: PathBuf,
    pub pending_edits: HashMap<ItemId, String>,
    pub upload_dialog: Option<UploadDialog>,
    pub contribute: ContributeState,
    pub suggest: SuggestState,
    pub upload_rate: RateLimit,
    pub templates: TemplateStore,
    pub template_rows: Vec<TemplateRow>,
    pub template_sync: Option<TemplateSync>,
    library_tx: std::sync::mpsc::Sender<LibraryMsg>,
    library_rx: std::sync::mpsc::Receiver<LibraryMsg>,
    /// Library uploads / template loads still running.
    pub library_busy: usize,
    /// The control socket for the CLI / MCP server (see `control.rs`).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) control: Option<crate::control::ControlServer>,
}

/// Grid spacings offered in the Page panel, besides a custom one.
pub const GRID_PRESETS: &[(&str, f32, Unit)] = &[
    ("1/8 in", 0.125, Unit::Inch),
    ("1/4 in", 0.25, Unit::Inch),
    ("1/2 in", 0.5, Unit::Inch),
    ("1 in", 1.0, Unit::Inch),
    ("0.5 cm", 0.5, Unit::Cm),
    ("1 cm", 1.0, Unit::Cm),
];

pub const PRESETS: &[(&str, f32, f32, Unit)] = &[
    ("Custom", 0.0, 0.0, Unit::Inch),
    ("Default 10 x 7 in", 10.0, 7.0, Unit::Inch),
    ("Letter portrait 8.5 x 11 in", 8.5, 11.0, Unit::Inch),
    ("Letter landscape 11 x 8.5 in", 11.0, 8.5, Unit::Inch),
    ("A4 portrait 21 x 29.7 cm", 21.0, 29.7, Unit::Cm),
    ("A4 landscape 29.7 x 21 cm", 29.7, 21.0, Unit::Cm),
    ("Slide 16:9 1920 x 1080 px", 1920.0, 1080.0, Unit::Px),
    ("Slide 4:3 1600 x 1200 px", 1600.0, 1200.0, Unit::Px),
    ("Square 10 x 10 in", 10.0, 10.0, Unit::Inch),
    ("Poster 36 x 48 in", 36.0, 48.0, Unit::Inch),
];

impl BioCartoon {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut fonts = egui::FontDefinitions::default();
        egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
        TextFont::install(&mut fonts);
        cc.egui_ctx.set_fonts(fonts);

        let mut settings: Settings = cc
            .storage
            .and_then(|s| s.get_string("settings"))
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        if settings.version < 3 {
            let d = Settings::default();
            settings.asset_base_url = d.asset_base_url;
            settings.catalog_path = d.catalog_path;
            settings.local_data_dir = d.local_data_dir;
        }
        // Snapping became off by default in version 4.
        if settings.version < 4 {
            settings.grid_snap = false;
        }
        settings.version = SETTINGS_VERSION;
        // In the browser, an absolute path inside crate::storage (user assets are stored by absolute path).
        let app_dir = if cfg!(target_arch = "wasm32") { PathBuf::from("/biocartoon") } else { dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("biocartoon") };
        let _ = std::fs::create_dir_all(app_dir.join("generated"));
        if settings.client_id.is_empty() {
            settings.client_id = crate::stars::new_client_id();
        }
        let local_dir = if settings.local_data_dir.is_empty() { None } else { Some(PathBuf::from(&settings.local_data_dir)) };
        let images = ImageCache::new(settings.asset_base_url.clone(), local_dir);
        if !cfg!(target_arch = "wasm32") {
            let offline_dir = if settings.offline_dir.is_empty() { app_dir.join(crate::offline::DIR_NAME) } else { PathBuf::from(&settings.offline_dir) };
            images.set_cache_dir(offline_dir.join("assets"));
        }
        let ai = AiClient::new(settings.ai_provider, &settings.ai_model, &settings.api_key, &settings.ai_base_url, &settings.ai_prompts);
        // Smoke-test runs (BIOCARTOON_SCREENSHOT) must not touch the real projects.
        let projects_dir = match std::env::var("BIOCARTOON_PROJECTS_DIR") {
            Ok(d) => PathBuf::from(d),
            Err(_) if std::env::var("BIOCARTOON_SCREENSHOT").is_ok() => std::env::temp_dir().join("biocartoon-smoke-projects"),
            Err(_) => app_dir.join("projects"),
        };
        let (thumb_tx, thumb_rx) = std::sync::mpsc::channel();
        let (library_tx, library_rx) = std::sync::mpsc::channel();
        let templates = TemplateStore::open(&app_dir.join("templates.sqlite"));
        let (action_tx, action_rx) = std::sync::mpsc::channel();
        let mut app = Self {
            doc: Document::default(),
            cur_slide: 0,
            selection: Vec::new(),
            history: Vec::new(),
            future: Vec::new(),
            clipboard: Vec::new(),
            paste_count: 0,
            ctrl_click: false,
            pasted: false,
            rail: Some(RailTab::Slides),
            icons: IconsState::default(),
            generate: GenerateState::default(),
            catalog: CatalogClient::new(PathBuf::new(), PathBuf::new()),
            stars: StarsClient::new(Some(cc.egui_ctx.clone())),
            stars_in_flight: HashSet::new(),
            stars_migrated: false,
            account: crate::account::Account::new(app_dir.join("account.json"), cc.egui_ctx.clone()),
            catalog_sync: None,
            offline: Default::default(),
            images,
            ai,
            settings,
            show_settings: false,
            applied_theme: None,
            show_about: false,
            spell: Default::default(),
            show_spell_window: false,
            settings_tab: SettingsTab::General,
            ai_setup: None,
            view: crate::canvas::View::default(),
            page: PageState::default(),
            ai_edit: None,
            rail_before_edit: None,
            last_rail: None,
            replace_target: None,
            pending_fit: Vec::new(),
            crop_mode: false,
            placing_text: false,
            drawing: false,
            pen_color: [40, 40, 50, 255],
            pen_width: 3.0,
            stroke: Vec::new(),
            editing_text: None,
            editing_cell: 0,
            text_edit_focus: false,
            toasts: Vec::new(),
            screen: Screen::Home,
            ctx: cc.egui_ctx.clone(),
            store: Store::new(projects_dir),
            project: None,
            saved_doc: None,
            last_autosave: 0.0,
            home: HomeState::default(),
            thumb_tx,
            thumb_rx,
            action_tx,
            action_rx,
            thumbs_pending: HashSet::new(),
            export_doc: None,
            export_project: None,
            pending_email: None,
            dirty: false,
            actions: Vec::new(),
            show_export: false,
            export_job: None,
            export_size: None,
            export_size_rx: None,
            variants: Vec::new(),
            variants_for: None,
            variants_req: 0,
            credit_lookups: HashSet::new(),
            show_attributions: false,
            attributions_slide: None,
            attributions_export: None,
            export_saved_at: None,
            export_credits: None,
            credit_item: None,
            attributions_required_only: false,
            drag: None,
            selected_guide: None,
            app_dir,
            pending_edits: HashMap::new(),
            upload_dialog: None,
            contribute: ContributeState::default(),
            suggest: SuggestState::default(),
            upload_rate: RateLimit::default(),
            template_rows: templates.list(),
            templates,
            template_sync: None,
            library_tx,
            library_rx,
            library_busy: 0,
            #[cfg(not(target_arch = "wasm32"))]
            control: None,
        };
        // Smoke-test runs must not take over the real editor's control socket.
        #[cfg(not(target_arch = "wasm32"))]
        if std::env::var("BIOCARTOON_NO_CONTROL").is_err() && std::env::var("BIOCARTOON_SCREENSHOT").is_err() {
            match crate::control::ControlServer::start(&app.app_dir, cc.egui_ctx.clone()) {
                Ok(c) => app.control = Some(c),
                Err(e) => log::warn!("{e}"),
            }
        }
        app.open_catalog();
        app.refresh_projects();
        app
    }

    /// Open the configured catalog: a local file, or the downloaded copy (refreshed in the background).
    fn open_catalog(&mut self) {
        let path = if self.settings.catalog_path.is_empty() {
            let dir = self.app_dir.join("catalog");
            self.catalog_sync = Some(CatalogSync::start(&self.settings.asset_base_url, dir.clone()));
            self.template_sync = Some(TemplateSync::start(&self.settings.asset_base_url, &self.templates, dir.clone()));
            cached_catalog(&dir).unwrap_or_default()
        } else {
            self.catalog_sync = None;
            PathBuf::from(&self.settings.catalog_path)
        };
        self.load_catalog(path);
    }

    fn load_catalog(&mut self, path: PathBuf) {
        self.catalog = CatalogClient::new(path, self.app_dir.join("usage.sqlite"));
        if !self.catalog.ok && !self.settings.catalog_path.is_empty() {
            self.catalog.status = format!("Catalog not found at {}", self.settings.catalog_path);
        }
        self.credit_lookups.clear();
        self.icons.categories.clear();
        self.icons.sources.clear();
        self.icons.facets = None;
        self.icons.facets_for = None;
        self.icons.last_sent_query = "\u{0}".into(); // re-run the current search
        if self.catalog.ok {
            self.catalog.send(Req::Sources);
            // For the library dialog's category suggestions.
            self.catalog.send(Req::Categories { source: "user".into() });
        }
    }

    fn poll_catalog_sync(&mut self, ctx: &egui::Context) {
        let Some(sync) = &mut self.catalog_sync else { return };
        match sync.poll() {
            None => ctx.request_repaint_after(std::time::Duration::from_millis(200)),
            Some(Ok(Some(path))) => {
                log::info!("catalog updated: {}", path.display());
                self.load_catalog(path);
                if std::mem::take(&mut self.offline.syncing) {
                    self.toast("Icon catalog updated");
                }
            }
            Some(Ok(None)) => {
                if std::mem::take(&mut self.offline.syncing) {
                    self.toast("Icon catalog is up to date");
                }
            }
            Some(Err(e)) => {
                log::warn!("catalog sync: {e}");
                if !self.catalog.ok {
                    self.catalog.status = format!("Couldn't download the icon catalog ({e}).");
                }
                if std::mem::take(&mut self.offline.syncing) {
                    self.toast_err(format!("Couldn't check the icon catalog ({e})"));
                }
            }
        }
    }

    pub(crate) fn ui_ctx(&self) -> egui::Context {
        self.ctx.clone()
    }

    pub fn open_settings(&mut self, tab: SettingsTab) {
        self.settings_tab = tab;
        self.show_settings = true;
    }

    /// True when AI tools can run without asking for a model first.
    pub fn ai_ready(&self) -> bool {
        let s = &self.settings;
        let connected = if s.ai_provider.self_hosted() { !s.ai_base_url.trim().is_empty() } else { !s.api_key.trim().is_empty() };
        connected || std::env::var("BIOCARTOON_AI").as_deref() == Ok("placeholder")
    }

    pub fn ai_status(&self) -> String {
        if self.ai_ready() {
            let s = &self.settings;
            let model = if s.ai_provider.self_hosted() { &s.ai_base_url } else { &s.ai_model };
            format!("Model: {} · {}", s.ai_provider.label(), model)
        } else {
            "You'll pick a model and enter an API key on first use.".into()
        }
    }

    fn open_ai_setup(&mut self, pending: Option<Action>) {
        self.ai_setup = Some(AiSetup {
            provider: self.settings.ai_provider,
            model: self.settings.ai_model.clone(),
            key: self.settings.api_key.clone(),
            base_url: self.settings.ai_base_url.clone(),
            pending,
        });
    }

    // ---------- helpers ----------

    pub fn slide(&self) -> &Slide {
        &self.doc.slides[self.cur_slide]
    }

    pub fn slide_mut(&mut self) -> &mut Slide {
        let i = self.cur_slide;
        &mut self.doc.slides[i]
    }

    pub fn item(&self, id: ItemId) -> Option<&Item> {
        self.slide().items.iter().find(|i| i.id == id)
    }

    pub fn item_mut(&mut self, id: ItemId) -> Option<&mut Item> {
        self.slide_mut().items.iter_mut().find(|i| i.id == id)
    }

    pub fn selected_items(&self) -> Vec<&Item> {
        self.slide().items.iter().filter(|i| self.selection.contains(&i.id)).collect()
    }

    pub fn selection_rect(&self) -> Option<Rect> {
        let items = self.selected_items();
        if items.is_empty() {
            return None;
        }
        let mut r = items[0].rect();
        for it in &items[1..] {
            r = r.union(it.rect());
        }
        Some(r)
    }

    pub fn single_selected(&self) -> Option<&Item> {
        if self.selection.len() == 1 {
            self.item(self.selection[0])
        } else {
            None
        }
    }

    pub fn toast(&mut self, text: impl Into<String>) {
        self.toasts.push(Toast { text: text.into(), until: 0.0, error: false });
    }

    pub fn toast_err(&mut self, text: impl Into<String>) {
        let t = text.into();
        log::warn!("{t}");
        self.toasts.push(Toast { text: t, until: 0.0, error: true });
    }

    /// Snapshot the document for undo. Call before mutating.
    pub fn commit(&mut self) {
        self.history.push((self.doc.clone(), self.cur_slide));
        if self.history.len() > 100 {
            self.history.remove(0);
        }
        self.future.clear();
        self.dirty = true;
    }

    pub fn undo(&mut self) {
        if let Some((d, s)) = self.history.pop() {
            self.future.push((self.doc.clone(), self.cur_slide));
            self.doc = d;
            self.cur_slide = s.min(self.doc.slides.len() - 1);
            self.prune_selection();
        }
    }

    pub fn redo(&mut self) {
        if let Some((d, s)) = self.future.pop() {
            self.history.push((self.doc.clone(), self.cur_slide));
            self.doc = d;
            self.cur_slide = s.min(self.doc.slides.len() - 1);
            self.prune_selection();
        }
    }

    fn prune_selection(&mut self) {
        let ids: Vec<ItemId> = self.slide().items.iter().map(|i| i.id).collect();
        self.selection.retain(|id| ids.contains(id));
        self.selected_guide = self.selected_guide.filter(|g| *g < self.doc.guides.len());
    }

    /// Expand selection to full groups.
    pub fn expand_groups(&mut self) {
        let groups: Vec<u64> = self.selected_items().iter().filter_map(|i| i.group).collect();
        if groups.is_empty() {
            return;
        }
        let extra: Vec<ItemId> = self
            .slide()
            .items
            .iter()
            .filter(|i| i.group.is_some_and(|g| groups.contains(&g)))
            .map(|i| i.id)
            .collect();
        for id in extra {
            if !self.selection.contains(&id) {
                self.selection.push(id);
            }
        }
    }

    pub fn select(&mut self, ids: Vec<ItemId>) {
        self.selected_guide = None;
        self.selection = ids;
        self.expand_groups();
        self.crop_mode = false;
        if !self.selection.is_empty() {
            self.open_edit_tab();
        } else {
            self.close_edit_tab();
        }
    }

    /// Show the selection's properties on the right, remembering the tab it replaces.
    pub(crate) fn open_edit_tab(&mut self) {
        if self.rail != Some(RailTab::Edit) {
            self.rail_before_edit = self.rail;
            self.rail = Some(RailTab::Edit);
        }
    }

    /// Back to the tab shown before the selection opened Edit.
    pub(crate) fn close_edit_tab(&mut self) {
        if self.rail == Some(RailTab::Edit) {
            self.rail = self.rail_before_edit.take();
        }
    }

    /// Start typing into text box `id` on the canvas (a table: its first cell).
    pub(crate) fn start_text_edit(&mut self, id: ItemId) {
        match self.item(id) {
            Some(i) if matches!(i.kind, ItemKind::Text { .. }) && !i.locked => {
                self.editing_text = Some(id);
                self.text_edit_focus = true;
            }
            Some(i) if matches!(i.kind, ItemKind::Table { .. }) => self.start_cell_edit(id, 0),
            _ => {}
        }
    }

    /// Start typing into cell `cell` of table `id` on the canvas.
    pub(crate) fn start_cell_edit(&mut self, id: ItemId, cell: usize) {
        let ok = self.item(id).is_some_and(|i| !i.locked && matches!(&i.kind, ItemKind::Table { rows, cols, cells, .. } if cell < (rows * cols).min(cells.len())));
        if ok {
            if self.editing_text != Some(id) {
                self.finish_text_edit();
            }
            self.editing_text = Some(id);
            self.editing_cell = cell;
            self.text_edit_focus = true;
        }
    }

    /// Stop typing on the canvas; a text box left empty is removed.
    pub(crate) fn finish_text_edit(&mut self) {
        let Some(id) = self.editing_text.take() else { return };
        self.ctx.memory_mut(|m| m.surrender_focus(egui::Id::new(("text_editor", id))));
        let empty = self.item(id).is_some_and(|i| matches!(&i.kind, ItemKind::Text { text, .. } if text.trim().is_empty()));
        if empty {
            self.slide_mut().items.retain(|i| i.id != id);
            self.selection.retain(|s| *s != id);
            self.dirty = true;
        }
    }

    pub(crate) fn insert_item(&mut self, mut item: Item) {
        self.commit();
        let page = self.doc.page_rect();
        let size = item.rect().size();
        let center = page.center();
        if item.is_line() {
            let s = item.size();
            item.pos = [center.x - s.x / 2.0, center.y - s.y / 2.0];
        } else {
            item.pos = [center.x - size.x / 2.0, center.y - size.y / 2.0];
        }
        if self.settings.grid_snap {
            let g = self.settings.grid_size;
            item.pos = [(item.pos[0] / g).round() * g, (item.pos[1] / g).round() * g];
        }
        // Avoid stacking exactly on top of the previous insert.
        let n = self.slide().items.iter().filter(|i| i.pos == item.pos).count() as f32;
        item.pos[0] += n * 12.0;
        item.pos[1] += n * 12.0;
        let id = item.id;
        self.slide_mut().items.push(item);
        // Select the new item but leave the sidebar where it was, so the user can keep inserting.
        self.selection = vec![id];
        self.crop_mode = false;
    }

    /// The longer side of a newly inserted picture: a quarter of the page's shorter side.
    fn insert_side(&self, fraction: f32) -> f32 {
        (self.doc.width.min(self.doc.height) * fraction).max(60.0)
    }

    /// A catalog asset's size on insertion, from its PNG's proportions; the SVG that is drawn
    /// may be proportioned differently, so the item is re-sized once it is loaded.
    fn asset_size(&self, row: &AssetRow) -> Vec2 {
        let (w, h) = (row.png_width.max(1) as f32, row.png_height.max(1) as f32);
        let s = self.insert_side(0.25) / w.max(h);
        Vec2::new(w * s, h * s)
    }

    /// A catalog asset as an image source, with its credit so the document can list it.
    fn asset_source(row: &AssetRow) -> ImageSource {
        ImageSource::Asset {
            id: row.id.clone(),
            key: row.png_key.clone(),
            svg: row.svg_key.clone(),
            title: row.title.clone(),
            source: row.source.clone(),
            credit: row.credit.clone(),
            license: row.license.clone(),
        }
    }

    pub fn insert_asset(&mut self, row: &AssetRow) {
        self.catalog.send(Req::RecordUse { source: row.source.clone(), title: row.title.clone() });
        if let Some(target) = self.replace_target.take() {
            self.commit();
            if let Some(it) = self.item_mut(target) {
                if let ItemKind::Image { source, crop, .. } = &mut it.kind {
                    *source = Self::asset_source(row);
                    *crop = [0.0, 0.0, 1.0, 1.0];
                    it.name = row.title.clone();
                    let (w, h) = (row.png_width.max(1) as f32, row.png_height.max(1) as f32);
                    let area = it.size[0] * it.size[1];
                    let nw = (area * w / h).sqrt();
                    it.size = [nw, nw * h / w];
                    self.pending_fit.push((target, Fit::Area(area)));
                }
            }
            self.toast(format!("Replaced with {}", row.title));
            self.open_edit_tab();
            return;
        }
        let id = self.doc.alloc_id();
        let size = self.asset_size(row);
        let kind = ItemKind::Image {
            source: Self::asset_source(row),
            crop: [0.0, 0.0, 1.0, 1.0],
            overlay: [255, 255, 255, 0],
        };
        let item = Item::new(id, row.title.clone(), kind, Pos2::ZERO, size);
        self.pending_fit.push((id, Fit::Side(self.insert_side(0.25))));
        self.insert_item(item);
    }

    pub fn insert_file(&mut self, path: &Path) {
        let p = path.display().to_string();
        let target = self.insert_side(0.3);
        // Size from the decoded image if it is already cached, else from the file's header; a
        // square to be re-sized on loading when neither is readable.
        let size = match self.images.file(&p) {
            ImgState::Ready { size, .. } => Some((size[0] as f32, size[1] as f32)),
            _ => crate::storage::read(path).ok().and_then(|b| crate::assets::image_size(&b)).filter(|(w, h)| *w > 0.0 && *h > 0.0),
        };
        let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Image".into());
        let id = self.doc.alloc_id();
        let kind = ItemKind::Image {
            source: ImageSource::file(p),
            crop: [0.0, 0.0, 1.0, 1.0],
            overlay: [255, 255, 255, 0],
        };
        let mut item = Item::new(id, name, kind, Pos2::ZERO, Vec2::splat(target));
        match size {
            Some((w, h)) => {
                let s = target / w.max(h);
                item.size = [w * s, h * s];
                item.fitted = true;
            }
            None => self.pending_fit.push((id, Fit::Side(target))),
        }
        self.insert_item(item);
    }

    /// Give image items the shape of their picture once it has loaded.
    ///
    /// A catalog asset is placed with its PNG's proportions, which need not be those of the SVG
    /// that is drawn (BioArt crops its PNGs to the drawing while its SVGs sit on a square
    /// artboard), so items on `pending_fit` are re-sized to the loaded picture. Image items of
    /// documents from before SVGs were trimmed to their drawing are moved onto the drawing
    /// ([`crate::doc::fit_to_drawing`]), which changes nothing visible; the saved copy gets the
    /// same change so that this does not count as an edit.
    fn fit_images(&mut self) {
        for (si, slide) in self.doc.slides.iter_mut().enumerate() {
            for it in &mut slide.items {
                let ItemKind::Image { source, .. } = &it.kind else { continue };
                let pending = self.pending_fit.iter().position(|(id, _)| *id == it.id);
                if it.fitted && pending.is_none() {
                    continue;
                }
                let key = source.cache_key();
                let size = match self.images.get(&key) {
                    Some(ImgState::Ready { size, .. }) => Vec2::new(size[0] as f32, size[1] as f32),
                    Some(ImgState::Failed(_)) => {
                        if let Some(i) = pending {
                            self.pending_fit.remove(i);
                        }
                        continue;
                    }
                    _ => continue,
                };
                let vector = self.images.vector(&key);
                if let Some(i) = pending {
                    let (_, fit) = self.pending_fit.remove(i);
                    let pic = vector.as_ref().map_or(size, |v| v.size());
                    let new = match fit {
                        Fit::Side(side) => pic * (side / pic.max_elem()),
                        Fit::Area(area) => {
                            let w = (area * pic.x / pic.y).sqrt();
                            Vec2::new(w, w * pic.y / pic.x)
                        }
                    };
                    let c = it.rect().center();
                    it.pos = [c.x - new.x / 2.0, c.y - new.y / 2.0];
                    it.size = [new.x, new.y];
                    it.fitted = true;
                } else {
                    let before = it.clone();
                    if let Some((r, crop)) = vector.and_then(|v| crate::doc::fit_to_drawing(it, v.artboard(), v.content())) {
                        it.pos = [r.min.x, r.min.y];
                        it.size = [r.width(), r.height()];
                        if let ItemKind::Image { crop: c, .. } = &mut it.kind {
                            *c = crop;
                        }
                    }
                    it.fitted = true;
                    let saved = self.saved_doc.as_mut().and_then(|d| d.slides.get_mut(si)).and_then(|s| s.items.iter_mut().find(|s| s.id == before.id));
                    if let Some(saved) = saved.filter(|s| **s == before) {
                        *saved = it.clone();
                    }
                }
            }
        }
        if !self.pending_fit.is_empty() {
            let ids: std::collections::HashSet<ItemId> = self.doc.slides.iter().flat_map(|s| s.items.iter().map(|i| i.id)).collect();
            self.pending_fit.retain(|(id, _)| ids.contains(id));
        }
    }

    /// Images dropped on the canvas: remembered in Uploads and placed on the page, fanned out
    /// around `at` when given, then all selected.
    fn drop_images(&mut self, files: &[PathBuf], at: Option<Pos2>) {
        let mut placed = Vec::new();
        for (i, f) in files.iter().enumerate() {
            let s = f.display().to_string();
            if !self.settings.uploads.contains(&s) {
                self.settings.uploads.push(s);
            }
            self.insert_file(f);
            let Some(&id) = self.selection.first() else { continue };
            placed.push(id);
            if let Some(p) = at {
                let off = i as f32 * 12.0;
                if let Some(item) = self.item_mut(id) {
                    let size = item.rect().size();
                    item.pos = [p.x - size.x / 2.0 + off, p.y - size.y / 2.0 + off];
                }
            }
        }
        self.selection = placed;
    }

    pub fn delete_selection(&mut self) {
        if let Some(g) = self.selected_guide.take() {
            if g < self.doc.guides.len() {
                self.commit();
                self.doc.guides.remove(g);
            }
            return;
        }
        if self.selection.is_empty() {
            return;
        }
        self.commit();
        let sel = self.selection.clone();
        self.slide_mut().items.retain(|i| !sel.contains(&i.id));
        self.selection.clear();
        self.close_edit_tab();
    }

    /// Copies the selection to the app's clipboard and, as a picture, to the system clipboard.
    pub fn copy_selection(&mut self) {
        self.clipboard = self.selected_items().into_iter().cloned().collect();
        self.paste_count = 0;
        if self.clipboard.is_empty() {
            return;
        }
        let mut cached = HashMap::new();
        for it in &self.clipboard {
            if let ItemKind::Image { source, .. } = &it.kind {
                let key = source.cache_key();
                if let Some(img) = self.images.cpu_image(&key) {
                    cached.insert(key, img);
                }
            }
        }
        let items: Vec<&Item> = self.clipboard.iter().collect();
        match crate::export::snapshot(&self.ctx, &items, &cached, 4096) {
            Ok(img) => {
                let (w, h) = img.dimensions();
                self.ctx.copy_image(egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], img.as_raw()));
            }
            Err(e) => log::warn!("copy: {e}"),
        }
    }

    pub fn paste(&mut self) {
        if self.clipboard.is_empty() {
            return;
        }
        self.commit();
        self.paste_count += 1;
        let off = 20.0 * self.paste_count as f32;
        let mut new_ids = Vec::new();
        let mut group_map: HashMap<u64, u64> = HashMap::new();
        let items = self.clipboard.clone();
        for mut it in items {
            it.id = self.doc.alloc_id();
            it.pos[0] += off;
            it.pos[1] += off;
            if let Some(g) = it.group {
                let ng = *group_map.entry(g).or_insert_with(|| self.doc.next_id * 1000 + g);
                it.group = Some(ng);
            }
            new_ids.push(it.id);
            self.slide_mut().items.push(it);
        }
        self.select(new_ids);
    }

    pub fn duplicate(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        let keep = self.clipboard.clone();
        let keep_count = self.paste_count;
        self.clipboard = self.selected_items().into_iter().cloned().collect();
        self.paste_count = 0;
        self.paste();
        self.clipboard = keep;
        self.paste_count = keep_count;
    }

    pub fn group_selection(&mut self) {
        if self.selection.len() < 2 {
            return;
        }
        self.commit();
        let g = self.doc.alloc_id() * 7919;
        let sel = self.selection.clone();
        for it in self.slide_mut().items.iter_mut() {
            if sel.contains(&it.id) {
                it.group = Some(g);
            }
        }
    }

    pub fn ungroup_selection(&mut self) {
        self.commit();
        let sel = self.selection.clone();
        for it in self.slide_mut().items.iter_mut() {
            if sel.contains(&it.id) {
                it.group = None;
            }
        }
    }

    pub fn is_grouped(&self) -> bool {
        self.selected_items().iter().any(|i| i.group.is_some())
    }

    pub fn align(&mut self, kind: AlignKind) {
        let Some(sel_rect) = self.selection_rect() else { return };
        let page = self.doc.page_rect();
        let target = if self.selection.len() > 1 { sel_rect } else { page };
        self.commit();
        let sel = self.selection.clone();
        let g = self.settings.grid_snap;
        let gs = self.settings.grid_size;
        let snap = |v: f32| if g { (v / gs).round() * gs } else { v };
        match kind {
            AlignKind::DistributeH | AlignKind::DistributeV => {
                let mut ids: Vec<ItemId> = sel.clone();
                let horizontal = kind == AlignKind::DistributeH;
                ids.sort_by(|a, b| {
                    let ra = self.item(*a).unwrap().rect();
                    let rb = self.item(*b).unwrap().rect();
                    if horizontal { ra.min.x.total_cmp(&rb.min.x) } else { ra.min.y.total_cmp(&rb.min.y) }
                });
                if ids.len() < 3 {
                    return;
                }
                let total: f32 = ids.iter().map(|id| {
                    let r = self.item(*id).unwrap().rect();
                    if horizontal { r.width() } else { r.height() }
                }).sum();
                let span = if horizontal { target.width() } else { target.height() };
                let gap = (span - total) / (ids.len() - 1) as f32;
                let mut cursor = if horizontal { target.min.x } else { target.min.y };
                for id in ids {
                    let it = self.item_mut(id).unwrap();
                    let mut r = it.rect();
                    if horizontal {
                        r = r.translate(Vec2::new(cursor - r.min.x, 0.0));
                        cursor += r.width() + gap;
                    } else {
                        r = r.translate(Vec2::new(0.0, cursor - r.min.y));
                        cursor += r.height() + gap;
                    }
                    it.set_rect(r);
                }
            }
            _ => {
                for id in sel {
                    let it = self.item_mut(id).unwrap();
                    let r = it.rect();
                    let d = match kind {
                        AlignKind::Left => Vec2::new(snap(target.min.x) - r.min.x, 0.0),
                        AlignKind::CenterH => Vec2::new(target.center().x - r.center().x, 0.0),
                        AlignKind::Right => Vec2::new(snap(target.max.x) - r.max.x, 0.0),
                        AlignKind::Top => Vec2::new(0.0, snap(target.min.y) - r.min.y),
                        AlignKind::Middle => Vec2::new(0.0, target.center().y - r.center().y),
                        AlignKind::Bottom => Vec2::new(0.0, snap(target.max.y) - r.max.y),
                        _ => Vec2::ZERO,
                    };
                    it.set_rect(r.translate(d));
                }
            }
        }
    }

    pub fn arrange(&mut self, kind: ArrangeKind) {
        if self.selection.is_empty() {
            return;
        }
        self.commit();
        let sel = self.selection.clone();
        let items = &mut self.slide_mut().items;
        match kind {
            ArrangeKind::Front => {
                let (mut a, b): (Vec<Item>, Vec<Item>) = items.drain(..).partition(|i| sel.contains(&i.id));
                *items = b;
                items.append(&mut a);
            }
            ArrangeKind::Back => {
                let (mut a, mut b): (Vec<Item>, Vec<Item>) = items.drain(..).partition(|i| sel.contains(&i.id));
                a.append(&mut b);
                *items = a;
            }
            ArrangeKind::Forward => {
                for i in (0..items.len().saturating_sub(1)).rev() {
                    if sel.contains(&items[i].id) && !sel.contains(&items[i + 1].id) {
                        items.swap(i, i + 1);
                    }
                }
            }
            ArrangeKind::Backward => {
                for i in 1..items.len() {
                    if sel.contains(&items[i].id) && !sel.contains(&items[i - 1].id) {
                        items.swap(i, i - 1);
                    }
                }
            }
        }
    }

    pub fn flip(&mut self, horizontal: bool) {
        if self.selection.is_empty() {
            return;
        }
        self.commit();
        let sel = self.selection.clone();
        let center = self.selection_rect().unwrap().center();
        let multi = sel.len() > 1;
        for it in self.slide_mut().items.iter_mut() {
            if !sel.contains(&it.id) {
                continue;
            }
            if horizontal {
                it.flip_h = !it.flip_h;
                it.rotation = -it.rotation;
                if it.is_line() {
                    it.pos[0] += it.size[0];
                    it.size[0] = -it.size[0];
                }
            } else {
                it.flip_v = !it.flip_v;
                it.rotation = -it.rotation;
                if it.is_line() {
                    it.pos[1] += it.size[1];
                    it.size[1] = -it.size[1];
                }
            }
            if multi {
                let r = it.rect();
                let nr = if horizontal {
                    Rect::from_min_size(Pos2::new(2.0 * center.x - r.max.x, r.min.y), r.size())
                } else {
                    Rect::from_min_size(Pos2::new(r.min.x, 2.0 * center.y - r.max.y), r.size())
                };
                it.set_rect(nr);
            }
        }
    }

    pub fn toggle_lock(&mut self) {
        self.commit();
        let sel = self.selection.clone();
        let any_unlocked = self.selected_items().iter().any(|i| !i.locked);
        for it in self.slide_mut().items.iter_mut() {
            if sel.contains(&it.id) {
                it.locked = any_unlocked;
            }
        }
    }

    // ---------- AI ----------

    pub fn run_ai(&mut self, op: AiOp) {
        let Some(it) = self.single_selected().cloned() else {
            self.toast_err("Select a single image first");
            return;
        };
        let ItemKind::Image { source, .. } = &it.kind else {
            self.toast_err("AI edits work on images");
            return;
        };
        let key = source.cache_key();
        let Some(img) = self.images.cpu_image(&key) else {
            self.toast_err("Image is still loading, try again in a moment");
            return;
        };
        self.toast(format!("{} ({})", op.label(), self.ai.backend.name()));
        self.ai.submit(AiJob::Edit { item: it.id, image: (*img).clone(), op });
    }

    fn handle_ai_results(&mut self, ctx: &egui::Context) {
        for r in self.ai.poll() {
            match r {
                AiResult::Generated { prompt, result } => {
                    self.generate.busy = false;
                    match result {
                        Ok(img) => {
                            let (w, h) = img.dimensions();
                            let ci = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], img.as_raw());
                            let tex = ctx.load_texture("generated", ci, egui::TextureOptions::LINEAR);
                            self.generate.status = format!("Generated with {}", self.ai.backend.name());
                            self.generate.result_path = self.keep_generated(ctx, &img, &prompt);
                            self.generate.result = Some((img, tex));
                        }
                        Err(e) => {
                            self.generate.status = e.clone();
                            self.toast_err(e);
                        }
                    }
                }
                AiResult::Edited { item, op, result } => match result {
                    Ok(img) => {
                        let path = self
                            .app_dir
                            .join("generated")
                            .join(format!("{}-{}.png", item, chrono::Utc::now().format("%Y%m%d%H%M%S")));
                        if let Err(e) = img.save(&path) {
                            self.toast_err(format!("save edited image: {e}"));
                            continue;
                        }
                        self.commit();
                        let p = path.display().to_string();
                        self.images.insert_image(ctx, &format!("file:{p}"), img);
                        if let Some(it) = self.item_mut(item) {
                            if let ItemKind::Image { source, .. } = &mut it.kind {
                                // The edit still owes the original's credit.
                                *source = source.derived(p);
                            }
                            it.name = format!("{} ({})", it.name, op.label());
                        }
                        self.toast(format!("Applied: {}", op.label()));
                        if let Some(ce) = &mut self.ai_edit {
                            if ce.item == item && !matches!(op, AiOp::Custom(_)) {
                                ce.messages.push((false, format!("Done: {}. Anything else?", op.label())));
                                ce.stage = AiChatStage::Start;
                            }
                        }
                    }
                    Err(e) => {
                        if let Some(ce) = &mut self.ai_edit {
                            if ce.item == item && !matches!(op, AiOp::Custom(_)) {
                                ce.messages.push((false, format!("That did not work: {e}")));
                                ce.stage = AiChatStage::Start;
                            }
                        }
                        self.toast_err(e);
                    }
                },
                AiResult::ChatReply { item, result } => {
                    if let Some(ce) = &mut self.ai_edit {
                        if ce.item == item {
                            ce.waiting = false;
                            match result {
                                Ok(t) => ce.messages.push((false, t)),
                                Err(e) => ce.messages.push((false, format!("Error: {e}"))),
                            }
                        }
                    }
                }
            }
        }
    }

    // ---------- catalog ----------

    /// Ask the catalog for credits the open documents lack (each id once per catalog).
    fn lookup_credits(&mut self) {
        if !self.catalog.ok {
            return;
        }
        let mut ids = crate::credits::missing(&self.doc);
        if let Some(d) = &self.export_doc {
            ids.extend(crate::credits::missing(d));
        }
        ids.retain(|id| self.credit_lookups.insert(id.clone()));
        if !ids.is_empty() {
            self.catalog.send(Req::Lookup { ids });
        }
    }

    fn handle_catalog(&mut self) {
        for r in self.catalog.poll() {
            match r {
                Resp::Assets { req_id, rows, more, total, colors } => {
                    if req_id == self.icons.results_req {
                        self.icons.loading = false;
                        self.icons.error = None;
                        self.icons.total = Some(total);
                        self.fetch_stars(&rows);
                        if self.icons.browse_offset > 0
                            && matches!(self.icons.mode, IconsMode::Search | IconsMode::Library)
                        {
                            self.icons.results.extend(rows);
                            self.icons.colors.extend(colors);
                        } else {
                            self.icons.results = rows;
                            self.icons.colors = colors;
                        }
                        self.icons.more = more;
                    }
                }
                Resp::Categories { source, cats, .. } => {
                    self.icons.categories.insert(source, cats);
                }
                Resp::Facets { req_id, facets } => {
                    if req_id == self.icons.facets_req {
                        self.icons.facets = Some(facets);
                    }
                }
                Resp::Sources { mut sources, .. } => {
                    // Alphabetical by badge abbreviation (BA, BI, ..., UG).
                    sources.sort_by_key(|s| theme::source_badge(&s.source).0);
                    self.icons.sources = sources;
                    self.catalog.ok = true;
                    if !self.stars_migrated {
                        self.stars_migrated = true;
                        self.catalog.send(Req::MyStars);
                        self.sync_stars();
                    }
                }
                Resp::MyStars { ids, .. } => self.adopt_stars(ids),
                Resp::Variants { req_id, rows } => {
                    if req_id == self.variants_req {
                        self.fetch_stars(&rows);
                        self.variants = rows;
                    }
                }
                Resp::Lookup { rows, .. } => {
                    crate::credits::fill(self.doc.slides.iter_mut(), &rows);
                    if let Some(d) = &mut self.export_doc {
                        crate::credits::fill(d.slides.iter_mut(), &rows);
                    }
                }
                Resp::Random { row, .. } => match row {
                    Some(row) => {
                        // A new icon, not a replacement for one waiting in the Edit tab.
                        self.replace_target = None;
                        self.insert_asset(&row);
                        self.toast(format!("Inserted {}", row.title));
                    }
                    None => self.toast_err("The icon library is empty".to_string()),
                },
                Resp::Inserted { result, .. } | Resp::Removed { result, .. } => match result {
                    Ok(()) => {
                        // Counts changed; the asset may also belong in (or leave) what's on screen.
                        self.icons.categories.remove("user");
                        self.catalog.send(Req::Categories { source: "user".into() });
                        self.catalog.send(Req::Sources);
                        self.icons.facets_for = None;
                        if matches!(self.icons.mode, IconsMode::Library | IconsMode::Favorites | IconsMode::Contribute) {
                            self.icons.browse_offset = 0;
                            self.request_browse();
                        }
                    }
                    Err(e) => self.toast_err(format!("library update failed: {e}")),
                },
                Resp::Done { .. } => {}
                Resp::Error { req_id, message } => {
                    if req_id == self.icons.results_req {
                        self.icons.loading = false;
                        // Or scrolling to the end would ask again every frame.
                        self.icons.more = false;
                    }
                    self.icons.error = Some(message.clone());
                    if self.catalog.ok {
                        self.toast_err(message);
                    }
                    self.catalog.ok = false;
                }
            }
        }
    }

    // ---------- stars ----------

    /// Ask the Worker for the current star counts of `rows` (they arrive via `poll_stars`).
    fn fetch_stars(&mut self, rows: &[AssetRow]) {
        if let Some(url) = self.submit_url() {
            let ids: Vec<String> = rows.iter().map(|r| r.id.clone()).filter(|i| !i.is_empty()).collect();
            self.stars.fetch(&url, &self.settings.client_id, ids);
        }
    }

    fn poll_stars(&mut self) {
        for m in self.stars.poll() {
            match m {
                StarsMsg::Counts(updates) => {
                    self.adopt_stars(updates.iter().filter(|u| u.mine).map(|u| u.id.clone()).collect());
                    self.apply_stars(updates);
                    // The Worker answers again: send what waited for it.
                    if self.stars_in_flight.is_empty() && !self.settings.pending_stars.is_empty() {
                        self.sync_stars();
                    }
                }
                StarsMsg::Toggled(Ok(u)) => {
                    self.stars_in_flight.remove(&u.id);
                    match self.settings.pending_stars.iter().position(|(p, _)| *p == u.id) {
                        // Changed again while this one was on its way: send the latest.
                        Some(i) if self.settings.pending_stars[i].1 != u.mine => self.sync_stars(),
                        Some(i) => {
                            self.settings.pending_stars.remove(i);
                        }
                        None => {}
                    }
                    let mine = self.settings.favorites.contains(&u.id);
                    self.apply_stars(vec![StarUpdate { mine, ..u }]);
                }
                StarsMsg::Toggled(Err((id, wanted, e))) => {
                    self.stars_in_flight.remove(&id);
                    if e.starts_with("Couldn't reach") || e.starts_with("The submission server answered") {
                        // Offline: it stays pending, and goes once the Worker answers again.
                    } else if e.starts_with("Limit reached") {
                        self.toast_err(format!("Your star is saved in Favorites and will be counted later. {e}"));
                    } else {
                        // Not on the server (one of the user's own uploads), or refused: the
                        // favorite stays, uncounted.
                        self.settings.pending_stars.retain(|(p, _)| *p != id);
                        if e != "No such asset" {
                            log::warn!("star {id} ({wanted}): {e}");
                        }
                    }
                }
            }
        }
    }

    /// Show the latest counts on the tiles and remember them in the usage file.
    fn apply_stars(&mut self, updates: Vec<StarUpdate>) {
        for r in self.icons.results.iter_mut().chain(self.variants.iter_mut()) {
            if let Some(u) = updates.iter().find(|u| u.id == r.id) {
                r.stars = u.n;
                r.starred = u.mine;
            }
        }
        self.catalog.send(Req::SetStars { updates });
    }

    /// Star the asset `id` (add it to Favorites), or take the star back. Favorites are kept in
    /// Settings, so this works offline; the Worker's count follows now, or once it can be reached.
    fn toggle_favorite(&mut self, id: String) {
        if id.is_empty() {
            return;
        }
        let on = !self.settings.favorites.contains(&id);
        if on {
            self.settings.favorites.insert(0, id.clone());
        } else {
            self.settings.favorites.retain(|f| *f != id);
        }
        // The count is shown right away; the Worker's answer corrects it.
        if let Some(r) = self.icons.results.iter().chain(&self.variants).find(|r| r.id == id) {
            let u = StarUpdate { id: id.clone(), n: (r.stars + if on { 1 } else { -1 }).max(0), mine: on };
            self.apply_stars(vec![u]);
        }
        if self.icons.mode == IconsMode::Favorites {
            if on {
                self.reload_results();
            } else {
                self.icons.results.retain(|r| r.id != id);
                self.icons.total = Some(self.icons.results.len() as i64);
            }
        }
        self.settings.pending_stars.retain(|(p, _)| *p != id);
        self.settings.pending_stars.push((id, on));
        self.sync_stars();
    }

    /// Send the star changes the Worker has not confirmed yet, one request per asset at a time.
    fn sync_stars(&mut self) {
        let Some(url) = self.submit_url() else { return };
        for (id, on) in self.settings.pending_stars.clone() {
            if self.stars_in_flight.insert(id.clone()) {
                self.stars.toggle(&url, &self.settings.client_id, id, on);
            }
        }
    }

    /// Stars this install gave (by the Worker or the usage file) become favorites, unless a
    /// change of the user's own is still on its way.
    fn adopt_stars(&mut self, ids: Vec<String>) {
        for id in ids {
            if !self.settings.favorites.contains(&id) && !self.settings.pending_stars.iter().any(|(p, _)| *p == id) {
                self.settings.favorites.push(id);
            }
        }
    }

    pub fn request_search(&mut self) {
        self.icons.browse_offset = 0;
        self.request_search_page();
        self.icons.last_sent_query = self.icons.query.clone();
        self.icons.last_sent_filter = self.icons.filter.clone();
    }

    /// Load (or, past offset 0, extend) the results of the current search.
    fn request_search_page(&mut self) {
        self.icons.loading = true;
        self.icons.results_req = self.catalog.send(Req::Search {
            query: self.icons.query.clone(),
            filter: self.icons.filter.catalog(),
            offset: self.icons.browse_offset,
            limit: RESULTS_PAGE,
            sort: self.settings.search_sort,
        });
    }

    /// The next page of the current search or browse (the list scrolled to its end).
    pub fn request_more(&mut self) {
        if !self.icons.more || self.icons.loading {
            return;
        }
        self.icons.browse_offset = self.icons.results.len();
        if self.icons.mode == IconsMode::Search {
            self.request_search_page();
        } else {
            self.request_browse();
        }
    }

    /// Load (or, past offset 0, extend) the results of the current browse.
    pub fn request_browse(&mut self) {
        let (offset, sort) = (self.icons.browse_offset, self.settings.icon_sort);
        let req = match self.icons.mode {
            IconsMode::Library => Req::Library { offset, limit: RESULTS_PAGE, sort },
            // All of the library at once, to pick from.
            IconsMode::Contribute => Req::Library { offset: 0, limit: 10_000, sort },
            IconsMode::Favorites if offset == 0 => Req::Favorites { ids: self.settings.favorites.clone() },
            _ => return,
        };
        self.icons.loading = true;
        self.icons.results_req = self.catalog.send(req);
    }

    /// Ask which of the filters' choices find icons under the current query and filter.
    pub fn request_facets(&mut self) {
        let query = self.icons.query.trim().to_string();
        self.icons.facets_req = self.catalog.send(Req::Facets { query: query.clone(), filter: self.icons.filter.catalog() });
        self.icons.facets_for = Some((query, self.icons.filter.clone()));
    }

    /// The Icons panel's Contribute artwork page, with the library's icons to pick from.
    pub fn open_contribute(&mut self) {
        self.icons.mode = IconsMode::Contribute;
        self.icons.results.clear();
        self.icons.total = None;
        self.icons.browse_offset = 0;
        self.request_browse();
    }

    /// Search one database: from the Databases list on the Icons panel's start.
    pub fn search_database(&mut self, source: &str) {
        self.icons.mode = IconsMode::Search;
        self.icons.query.clear();
        self.icons.filter = SearchFilter { sources: vec![source.to_string()], ..SearchFilter::default() };
        self.icons.show_filters = true;
        self.icons.results.clear();
        self.icons.total = None;
        self.request_search();
    }

    /// Re-run whatever produced the current results (after the sort order changed).
    pub fn reload_results(&mut self) {
        if self.icons.mode == IconsMode::Search {
            self.request_search();
        } else {
            self.icons.browse_offset = 0;
            self.icons.results.clear();
            self.request_browse();
        }
    }

    pub fn request_variants(&mut self, id: &str) {
        // Picking a preset swaps to an id already in the strip: keep it rather than reload.
        if self.variants_for.as_deref() == Some(id) || self.variants.iter().any(|v| v.id == id) {
            return;
        }
        self.variants_for = Some(id.into());
        self.variants.clear();
        self.variants_req = self.catalog.send(Req::Variants { id: id.into() });
    }

    // ---------- generate / upload ----------

    fn start_generate(&mut self, prompt: String) {
        if prompt.trim().is_empty() {
            return;
        }
        self.generate.busy = true;
        self.generate.status = format!("Generating with {}...", self.ai.backend.name());
        self.generate.prompt = prompt.clone();
        self.ai.submit(AiJob::Generate { prompt });
    }

    pub(crate) fn slugify(s: &str) -> String {
        let mut out = String::new();
        let mut last_dash = false;
        for c in s.to_lowercase().chars() {
            if c.is_alphanumeric() {
                out.push(c);
                last_dash = false;
            } else if !last_dash {
                out.push('-');
                last_dash = true;
            }
        }
        out.trim_matches('-').chars().take(60).collect()
    }

    /// Save a generated image to the history (and, with the setting on, the library); its path.
    fn keep_generated(&mut self, ctx: &egui::Context, img: &image::RgbaImage, prompt: &str) -> Option<String> {
        let now = chrono::Utc::now();
        let path = self.app_dir.join("generated").join(format!("gen-{}{:03}.png", now.format("%Y%m%d%H%M%S"), now.timestamp_subsec_millis()));
        let mut png = std::io::Cursor::new(Vec::new());
        if let Err(e) = img.write_to(&mut png, image::ImageFormat::Png).map_err(|e| e.to_string()).and_then(|_| crate::storage::write(&path, png.get_ref()).map_err(|e| e.to_string())) {
            self.toast_err(format!("save generated image: {e}"));
            return None;
        }
        let p = path.display().to_string();
        // Seed the cache so it shows (and is sized when placed) right away.
        self.images.insert_image(ctx, &format!("file:{p}"), img.clone());
        self.settings.generated.insert(0, GeneratedImage { path: p.clone(), prompt: prompt.into(), library_id: None });
        if self.settings.auto_library_generated {
            self.generated_to_library(&p, false);
        }
        Some(p)
    }

    /// The library id of the generated image at `path`, adding it to the library first if it
    /// is not there yet (`true` when it was just added).
    fn generated_library_id(&mut self, path: &str) -> Option<(String, bool)> {
        let i = self.settings.generated.iter().position(|g| g.path == path)?;
        if let Some(id) = &self.settings.generated[i].library_id {
            return Some((id.clone(), false));
        }
        let img = match read_image(Path::new(path)) {
            Ok(img) => img,
            Err(e) => {
                self.toast_err(e);
                return None;
            }
        };
        let prompt = self.settings.generated[i].prompt.clone();
        let mut name: String = prompt.trim().chars().take(60).collect();
        if name.is_empty() {
            name = "Generated image".into();
        }
        let author = self.settings.contributor_name.trim().to_string();
        let mut row = AssetRow {
            id: String::new(),
            source: "user".into(),
            source_name: "User-generated".into(),
            credit: format!("\"{name}\" generated with BioCartoon. Prompt: {prompt}"),
            title: name,
            variant: String::new(),
            category: String::new(),
            license: UPLOAD_LICENSES[0].0.into(),
            author,
            png_key: String::new(),
            png_local: None,
            png_width: 0,
            png_height: 0,
            svg_key: None,
            stars: 0,
            starred: false,
            taxa: Vec::new(),
        };
        if let Err(e) = self.save_user_png(img, &mut row) {
            self.toast_err(e);
            return None;
        }
        let id = row.id.clone();
        self.catalog.send(Req::InsertUser { row });
        self.settings.generated[i].library_id = Some(id.clone());
        Some((id, true))
    }

    /// Add a generated image to the library and, with `favorite`, to Favorites.
    fn generated_to_library(&mut self, path: &str, favorite: bool) {
        let Some((id, added)) = self.generated_library_id(path) else { return };
        let fav = favorite && !self.settings.favorites.contains(&id);
        if fav {
            self.toggle_favorite(id);
        }
        self.toast(match (added, fav) {
            (true, true) => "Added to your library and favorites",
            (true, false) => "Added to your library",
            (false, true) => "Added to your favorites",
            (false, false) if favorite => "Already in your favorites",
            (false, false) => "Already in your library",
        });
    }

    /// A library icon was replaced (`to`) or removed: generated images that pointed at `from` follow.
    fn relink_generated(&mut self, from: &str, to: Option<String>) {
        for g in self.settings.generated.iter_mut().filter(|g| g.library_id.as_deref() == Some(from)) {
            g.library_id = to.clone();
        }
    }

    /// Open Contribute artwork with a generated image picked (adding it to the library first).
    fn contribute_generated(&mut self, path: &str) {
        let Some((id, _)) = self.generated_library_id(path) else { return };
        self.open_contribute();
        self.contribute.library.insert(id);
    }

    /// The submission Worker, if one is configured.
    pub fn submit_url(&self) -> Option<String> {
        let s = self.settings.submit_url.trim();
        let url = if s.is_empty() { std::env::var("BIOCARTOON_SUBMIT_URL").unwrap_or_else(|_| DEFAULT_SUBMIT_URL.into()) } else { s.to_string() };
        Some(url.trim().trim_end_matches('/').to_string()).filter(|u| !u.is_empty())
    }

    /// Slide `i` of the open document, to save as a template.
    pub fn template_target(&self, i: usize) -> UploadTarget {
        let slide = self.doc.slides.get(i).cloned().unwrap_or_default();
        UploadTarget::Template(Box::new(TemplateSlide { slide, width: self.doc.width, height: self.doc.height, background: self.doc.background, saved: None }))
    }

    pub fn open_upload_dialog(&mut self, target: UploadTarget) {
        self.open_upload_queue(vec![target], UPLOAD_LICENSES[0].0, 1);
    }

    /// Open the library dialog for the first of `targets`, item `step` of the batch; the rest
    /// follow as each is submitted.
    fn open_upload_queue(&mut self, mut targets: Vec<UploadTarget>, license: &'static str, step: usize) {
        if targets.is_empty() {
            return;
        }
        let target = targets.remove(0);
        let (name, category, description) = match &target {
            UploadTarget::File(p) => (p.file_stem().map(|s| s.to_string_lossy().replace(['_', '-'], " ")).unwrap_or_default(), String::new(), String::new()),
            UploadTarget::Library(row) => (row.title.clone(), row.category.clone(), String::new()),
            UploadTarget::Template(t) => match &t.saved {
                Some(m) => (m.title.clone(), m.category.clone(), m.description.clone()),
                None => (t.slide.name.clone(), String::new(), String::new()),
            },
        };
        self.upload_dialog = Some(UploadDialog {
            target,
            name,
            category,
            description,
            license,
            share: self.submit_url().is_some(),
            credit: self.settings.contributor_name.clone(),
            step: (step, step + targets.len()),
            queue: targets,
            view_zoom: 1.0,
            view_pan: Vec2::ZERO,
        });
    }

    /// Everything picked under Contribute artwork, in the order it is listed there.
    fn contribute_selected(&mut self) {
        let c = std::mem::take(&mut self.contribute.icons);
        let mut targets: Vec<UploadTarget> = self.settings.uploads.iter().filter(|p| c.contains(*p)).map(|p| UploadTarget::File(p.into())).collect();
        let lib = std::mem::take(&mut self.contribute.library);
        targets.extend(self.icons.results.iter().filter(|r| r.png_local.is_some() && lib.contains(&r.id)).map(|r| UploadTarget::Library(Box::new(r.clone()))));
        let mut slides: Vec<usize> = std::mem::take(&mut self.contribute.slides).into_iter().filter(|&i| i < self.doc.slides.len()).collect();
        slides.sort_unstable();
        targets.extend(slides.into_iter().map(|i| self.template_target(i)));
        let mut picked: Vec<usize> = std::mem::take(&mut self.contribute.picked_templates).into_iter().collect();
        picked.sort_unstable();
        targets.extend(picked.into_iter().filter_map(|i| self.contribute.templates.get(i)).map(|t| UploadTarget::Template(Box::new(t.clone()))));
        let saved = std::mem::take(&mut self.contribute.library_templates);
        let rows: Vec<TemplateRow> = self.template_rows.iter().filter(|r| r.json_local.is_some() && saved.contains(&r.meta.id)).cloned().collect();
        for row in &rows {
            // A local file, so reading it here is quick.
            match crate::templates::load(row, "") {
                Ok(t) => targets.push(UploadTarget::Template(Box::new(TemplateSlide { slide: t.slide, width: t.width, height: t.height, background: t.background, saved: Some(row.meta.clone()) }))),
                Err(e) => self.toast_err(format!("\"{}\": {e}", row.meta.title)),
            }
        }
        if targets.is_empty() {
            return self.toast("Pick icons or templates to add first");
        }
        self.open_upload_queue(targets, UPLOAD_LICENSES[0].0, 1);
    }

    /// Offer the slides of .biocartoon files under Contribute artwork > Templates.
    fn add_template_files(&mut self, paths: &[PathBuf]) {
        let uploads = self.app_dir.join("uploads");
        let mut failed = Vec::new();
        for p in paths {
            match crate::storage::read(p).map_err(|e| e.to_string()).and_then(|b| crate::projects::from_file(&b, &uploads)) {
                Ok(doc) => {
                    for slide in doc.slides.into_iter().filter(|s| !s.items.is_empty()) {
                        self.contribute.templates.push(TemplateSlide { slide, width: doc.width, height: doc.height, background: doc.background, saved: None });
                    }
                }
                Err(e) => failed.push(format!("{}: {e}", p.display())),
            }
        }
        if !failed.is_empty() {
            self.toast_err(format!("Couldn't open {}", failed.join("; ")));
        }
    }

    /// Category suggestions for the library dialog: (name, is a subcategory).
    pub fn upload_categories(&self, template: bool) -> Vec<(String, bool)> {
        let mut out: Vec<(String, bool)> = Vec::new();
        let mut seen = HashSet::new();
        let mut own: Vec<String> = if template {
            self.template_rows.iter().map(|r| r.meta.category.trim().to_string()).collect()
        } else {
            self.icons.categories.get("user").map(|c| c.iter().map(|c| c.name.trim().to_string()).collect()).unwrap_or_default()
        };
        own.sort_by_key(|c| c.to_lowercase());
        for c in own.into_iter().filter(|c| !c.is_empty()) {
            if seen.insert(c.to_lowercase()) {
                out.push((c, false));
            }
        }
        for (cat, subs) in crate::taxonomy::TAXONOMY {
            if seen.insert(cat.to_lowercase()) {
                out.push((cat.to_string(), false));
            }
            for sub in *subs {
                if seen.insert(sub.to_lowercase()) {
                    out.push((sub.to_string(), true));
                }
            }
        }
        out
    }

    /// Unique id stem: readable slug plus a timestamp with sub-second digits.
    fn upload_slug(name: &str) -> String {
        let now = chrono::Utc::now();
        format!("{}-{}{:04x}", Self::slugify(name), now.format("%Y%m%d%H%M%S"), now.timestamp_subsec_micros() & 0xffff)
    }

    fn submit_upload(&mut self) {
        let Some(d) = self.upload_dialog.take() else { return };
        let name: String = d.name.trim().chars().take(80).collect();
        if name.is_empty() {
            self.toast_err("Give it a name first");
            self.upload_dialog = Some(d);
            return;
        }
        let category: String = d.category.trim().chars().take(60).collect();
        let description: String = d.description.trim().chars().take(500).collect();
        let credit: String = d.credit.trim().chars().take(60).collect();
        let share = match self.submit_url().filter(|_| d.share) {
            None => None,
            Some(url) => {
                if d.license == "CC-BY-4.0" && credit.is_empty() {
                    self.toast_err("CC BY needs a name to credit (a pseudonym is fine), or choose CC0");
                    self.upload_dialog = Some(d);
                    return;
                }
                let now = self.ctx.input(|i| i.time);
                if let Err(e) = self.upload_rate.try_take(now) {
                    self.toast_err(e);
                    self.upload_dialog = Some(d);
                    return;
                }
                self.settings.contributor_name = credit.clone();
                let meta = SubmitMeta {
                    title: name.clone(),
                    description: description.clone(),
                    category: category.clone(),
                    author: if credit.is_empty() { "anonymous".into() } else { credit.clone() },
                    license: d.license.into(),
                };
                Some((url, meta))
            }
        };
        // Local copies credit the contributor's chosen name, if they gave one.
        let author = match &share {
            Some((_, meta)) => meta.author.clone(),
            None => credit,
        };
        match &d.target {
            UploadTarget::Template(t) => self.save_template((**t).clone(), name, category, description, d.license, author, share),
            target => {
                let img = match target {
                    UploadTarget::File(p) => read_image(p),
                    UploadTarget::Library(row) => read_image(Path::new(row.png_local.as_deref().unwrap_or_default())),
                    UploadTarget::Template(_) => unreachable!(),
                };
                let replaces = match target {
                    UploadTarget::Library(row) => Some(row.id.clone()),
                    _ => None,
                };
                let credit_line = if author.is_empty() || author == "anonymous" { format!("\"{name}\", {}", d.license) } else { format!("\"{name}\" by {author}, {}", d.license) };
                let row = AssetRow {
                    id: String::new(),
                    source: "user".into(),
                    source_name: "User-generated".into(),
                    title: name,
                    variant: String::new(),
                    category,
                    license: d.license.into(),
                    credit: credit_line,
                    author,
                    png_key: String::new(),
                    png_local: None,
                    png_width: 0,
                    png_height: 0,
                    svg_key: None,
                    stars: 0,
                    starred: false,
                    taxa: Vec::new(),
                };
                match img {
                    Ok(img) => self.upload_asset(img, row, share, replaces),
                    Err(e) => self.toast_err(e),
                }
            }
        }
        // Next in the batch, keeping the license just chosen.
        self.open_upload_queue(d.queue, d.license, d.step.0 + 1);
    }

    /// Send the Submit a suggestion form to the submissions Worker.
    fn submit_suggestion(&mut self) {
        let title: String = self.suggest.title.trim().chars().take(80).collect();
        if title.is_empty() {
            return self.toast_err("Give the suggestion a title first");
        }
        let Some(url) = self.submit_url() else {
            return self.toast_err("No submission server is set up");
        };
        let now = self.ctx.input(|i| i.time);
        if let Err(e) = self.upload_rate.try_take(now) {
            return self.toast_err(e);
        }
        let name: String = self.suggest.name.trim().chars().take(60).collect();
        if !name.is_empty() {
            self.settings.contributor_name = name.clone();
        }
        let s = crate::submit::Suggestion {
            kind: if self.suggest.template { "template" } else { "icon" },
            title: title.clone(),
            description: self.suggest.description.trim().chars().take(1000).collect(),
            name,
        };
        self.suggest.busy = true;
        let (tx, ctx) = (self.library_tx.clone(), self.ctx.clone());
        self.library_busy += 1;
        crate::platform::spawn("suggest", move || {
            let _ = tx.send(LibraryMsg::SuggestionSent(title, crate::submit::suggest(&url, &s)));
            ctx.request_repaint();
        });
    }

    /// Save an image as a user asset in the local catalog and, with `share`, submit it for review.
    /// The local row is added once the submission answers, under the server's id when accepted,
    /// so the approved copy published later replaces it instead of appearing twice.
    /// Save an image to the library, and submit it if `share` says where. The library icon it
    /// `replaces`, if any, is removed once the new one is in.
    fn upload_asset(&mut self, img: image::RgbaImage, mut row: AssetRow, share: Option<(String, SubmitMeta)>, replaces: Option<String>) {
        let png = match self.save_user_png(img, &mut row) {
            Ok(png) => png,
            Err(e) => return self.toast_err(e),
        };
        let Some((url, meta)) = share else {
            self.toast(format!("Added \"{}\" to your library", row.title));
            if let Some(id) = &replaces {
                self.relink_generated(id, Some(row.id.clone()));
            }
            self.catalog.send(Req::InsertUser { row });
            if let Some(id) = replaces {
                self.catalog.send(Req::RemoveUser { id });
            }
            return;
        };
        let (tx, ctx) = (self.library_tx.clone(), self.ctx.clone());
        self.library_busy += 1;
        crate::platform::spawn("submit", move || {
            use crate::submit::Part;
            let result = serde_json::to_string(&meta).map_err(|e| e.to_string()).and_then(|m| {
                crate::submit::submit(&url, &[Part::Text("kind", "asset".into()), Part::Text("meta", m), Part::File("image", png, "image/png")])
            });
            let _ = tx.send(LibraryMsg::AssetSubmitted { row, result, replaces });
            ctx.request_repaint();
        });
    }

    /// Write an image to the user's library folder and fill in `row`'s id and file; the PNG.
    fn save_user_png(&self, img: image::RgbaImage, row: &mut AssetRow) -> Result<Vec<u8>, String> {
        let (png, w, h) = crate::templates::encode_png(img)?;
        let slug = Self::upload_slug(&row.title);
        let abs = self.app_dir.join("user_generated").join(format!("{slug}.png"));
        crate::storage::write(&abs, &png).map_err(|e| format!("save: {e}"))?;
        row.id = format!("user/{slug}");
        row.png_key = format!("user/{slug}.png");
        row.png_local = Some(abs.display().to_string());
        row.png_width = w;
        row.png_height = h;
        Ok(png)
    }

    /// Image loads for rendering `slide` off the UI thread: requests, and decoded copies we have.
    pub(crate) fn render_inputs(&self, slide: &Slide) -> (HashMap<String, crate::assets::Request>, HashMap<String, std::sync::Arc<image::RgbaImage>>) {
        let mut requests = HashMap::new();
        let mut cached = HashMap::new();
        for it in &slide.items {
            if let ItemKind::Image { source, .. } = &it.kind {
                let k = source.cache_key();
                if let Some(img) = self.images.cpu_image(&k) {
                    cached.insert(k.clone(), img);
                }
                if let Ok(req) = self.images.request_for(source) {
                    requests.insert(k, req);
                }
            }
        }
        (requests, cached)
    }

    fn save_template(&mut self, t: TemplateSlide, title: String, category: String, description: String, license: &str, author: String, submit: Option<(String, SubmitMeta)>) {
        let slug = Self::upload_slug(&title);
        let meta = TemplateMeta {
            id: slug.clone(),
            title,
            description,
            category,
            author,
            license: license.into(),
            created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            thumb_key: format!("templates/{slug}/thumb.png"),
        };
        let (page, bg) = (t.page_rect(), t.background);
        let replaces = t.saved.map(|m| m.id);
        let mut slide = t.slide;
        slide.name = meta.title.clone();
        let file = TemplateFile { meta: meta.clone(), width: t.width, height: t.height, background: t.background, slide };
        if let Err(e) = crate::templates::validate(&file) {
            return self.toast_err(e);
        }
        let (requests, cached) = self.render_inputs(&file.slide);
        let dir = self.app_dir.join("templates").join(&slug);
        let (tx, ctx) = (self.library_tx.clone(), self.ctx.clone());
        self.library_busy += 1;
        crate::platform::spawn("template-save", move || {
            let thumb = crate::export::thumbnail(&ctx, &file.slide, page, bg, &requests, &cached, 480);
            let res = crate::templates::save(crate::templates::SaveJob { meta, file, dir, thumb, submit });
            let _ = tx.send(LibraryMsg::TemplateSaved(res, replaces));
            ctx.request_repaint();
        });
    }

    pub(crate) fn load_saved_template(&mut self, row: TemplateRow) {
        let (tx, ctx, base) = (self.library_tx.clone(), self.ctx.clone(), self.settings.asset_base_url.clone());
        self.library_busy += 1;
        let own = row.json_local.is_some();
        // The browser can't block on a download, so a published template is fetched with ehttp.
        #[cfg(target_arch = "wasm32")]
        if !own && !base.is_empty() {
            let url = format!("{}/{}", base.trim_end_matches('/'), row.json_key);
            ehttp::fetch(ehttp::Request::get(&url), move |res| {
                let t = res.and_then(|r| if r.ok { Ok(r.bytes) } else { Err(format!("GET {url}: HTTP {}", r.status)) }).and_then(|b| crate::templates::parse(&b));
                let _ = tx.send(LibraryMsg::TemplateLoaded(t, own));
                ctx.request_repaint();
            });
            return;
        }
        crate::platform::spawn("template-load", move || {
            let _ = tx.send(LibraryMsg::TemplateLoaded(crate::templates::load(&row, &base), own));
            ctx.request_repaint();
        });
    }

    /// Add a template's slide after the current one, scaled to fit this document's page.
    fn insert_template(&mut self, t: TemplateFile, own: bool) {
        self.commit();
        let s = (self.doc.width / t.width).min(self.doc.height / t.height);
        let s = if s.is_finite() && s > 0.0 { s } else { 1.0 };
        let off = [(self.doc.width - t.width * s) / 2.0, (self.doc.height - t.height * s) / 2.0];
        let mut groups: HashMap<u64, u64> = HashMap::new();
        let mut slide = Slide { name: t.slide.name, items: Vec::new() };
        for mut it in t.slide.items {
            // Shared templates may only reference bucket objects, never files on this computer.
            if !own && matches!(&it.kind, ItemKind::Image { source: ImageSource::File { .. }, .. }) {
                continue;
            }
            it.id = self.doc.alloc_id();
            if let Some(g) = it.group {
                it.group = Some(*groups.entry(g).or_insert_with(|| self.doc.alloc_id()));
            }
            it.pos = [it.pos[0] * s + off[0], it.pos[1] * s + off[1]];
            it.size = [it.size[0] * s, it.size[1] * s];
            match &mut it.kind {
                ItemKind::Text { size, .. } => *size *= s,
                ItemKind::Shape { stroke_width, .. } => *stroke_width *= s,
                ItemKind::Line { width, .. } => *width *= s,
                ItemKind::Table { font_size, .. } | ItemKind::Code { font_size, .. } => *font_size *= s,
                // Templates published before credits were stored: credit the template's author.
                ItemKind::Image { source: ImageSource::Asset { source, title, credit, license, .. }, .. } if source == "template" && credit.is_empty() => {
                    *credit = crate::credits::template_credit(title, &t.meta.author, &t.meta.license, &t.meta.title);
                    *license = t.meta.license.clone();
                }
                ItemKind::Image { .. } => {}
            }
            slide.items.push(it);
        }
        self.doc.slides.insert(self.cur_slide + 1, slide);
        self.cur_slide += 1;
        self.selection.clear();
    }

    fn poll_library(&mut self, ctx: &egui::Context) {
        if let Some(sync) = &mut self.template_sync {
            if let Some(res) = sync.poll() {
                self.template_sync = None;
                match res {
                    Ok(Some((file, version))) => {
                        if let Err(e) = self.templates.merge_published(&file, &version) {
                            log::warn!("templates merge: {e}");
                        }
                        crate::templates::remove_download(&file);
                        self.template_rows = self.templates.list();
                    }
                    Ok(None) => {}
                    Err(e) => log::warn!("template sync: {e}"),
                }
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            }
        }
        while let Ok(msg) = self.library_rx.try_recv() {
            self.library_busy = self.library_busy.saturating_sub(1);
            match msg {
                LibraryMsg::AssetSubmitted { mut row, result, replaces } => {
                    let title = row.title.clone();
                    match result {
                        Ok(id) => {
                            row.id = format!("user/{id}");
                            row.png_key = format!("user/{id}.png");
                            self.toast(format!("Submitted \"{title}\" for review. It is in your library now and shared once approved."));
                        }
                        Err(e) => self.toast_err(format!("\"{title}\" is in your library but was not submitted: {e}")),
                    }
                    if let Some(id) = &replaces {
                        self.relink_generated(id, Some(row.id.clone()));
                    }
                    self.catalog.send(Req::InsertUser { row });
                    if let Some(id) = replaces {
                        self.catalog.send(Req::RemoveUser { id });
                    }
                }
                LibraryMsg::TemplateSaved(res, replaces) => match res {
                    Ok((row, submitted)) => {
                        let title = row.meta.title.clone();
                        if let Err(e) = self.templates.insert(&row) {
                            self.toast_err(e);
                        } else {
                            if let Some(id) = replaces.filter(|id| *id != row.meta.id) {
                                if let Err(e) = self.templates.remove_local(&id) {
                                    self.toast_err(e);
                                }
                            }
                            match submitted {
                                None => self.toast(format!("Saved template \"{title}\"")),
                                Some(Ok(())) => self.toast(format!("Submitted template \"{title}\" for review. It is saved here and shared once approved.")),
                                Some(Err(e)) => self.toast_err(format!("Template \"{title}\" is saved here but was not submitted: {e}")),
                            }
                        }
                        self.template_rows = self.templates.list();
                    }
                    Err(e) => self.toast_err(format!("Template not saved: {e}")),
                },
                LibraryMsg::TemplateLoaded(res, own) => match res {
                    Ok(t) => self.insert_template(t, own),
                    Err(e) => self.toast_err(e),
                },
                LibraryMsg::SuggestionSent(title, res) => {
                    self.suggest.busy = false;
                    match res {
                        Ok(_) => {
                            self.toast(format!("Thanks! Sent your suggestion \"{title}\" to the BioCartoon team."));
                            self.suggest.title.clear();
                            self.suggest.description.clear();
                        }
                        Err(e) => self.toast_err(format!("Suggestion not sent: {e}")),
                    }
                }
            }
        }
    }

    // ---------- file IO ----------

    /// Reset the editor to `doc` (no undo history, nothing selected).
    fn load_into_editor(&mut self, doc: Document, project: Option<String>) {
        self.saved_doc = Some(doc.clone());
        self.doc = doc;
        self.project = project;
        self.cur_slide = 0;
        self.selection.clear();
        self.selected_guide = None;
        self.history.clear();
        self.future.clear();
        self.ai_edit = None;
        self.page.last_edit = None;
        self.replace_target = None;
        self.pending_fit.clear();
        self.crop_mode = false;
        self.dirty = false;
        self.view.fit_requested = true;
        self.screen = Screen::Editor;
    }

    /// Start a blank document. It becomes a project on its first edit (or explicit save), so
    /// opening "+ New" and leaving straight away doesn't leave an empty project behind.
    pub(crate) fn new_doc(&mut self) {
        self.close_project();
        self.load_into_editor(Document::default(), None);
    }

    fn open_project(&mut self, id: &str) {
        if self.project.as_deref() == Some(id) && self.screen == Screen::Editor {
            return;
        }
        match self.store.load(id) {
            Ok(doc) => {
                self.close_project();
                self.store.touch_opened(id);
                self.load_into_editor(doc, Some(id.to_string()));
            }
            Err(e) => self.toast_err(format!("open: {e}")),
        }
    }

    /// Ask for a `.biocartoon` file to import (see [`Self::open_file`]).
    fn open(&mut self) {
        let tx = self.action_tx.clone();
        crate::platform::pick_files("biocartoon", &["biocartoon", "json", "zip"], true, move |mut paths| {
            let zip = paths.iter().any(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip")));
            let _ = tx.send(if paths.len() == 1 && !zip { Action::OpenFile(paths.remove(0)) } else { Action::ImportFiles(paths) });
        });
    }

    /// Import `.biocartoon` files, and those inside .zip files (a backup), as new projects.
    fn import_files(&mut self, paths: &[PathBuf]) {
        let (mut files, mut failed) = (Vec::new(), Vec::new());
        for p in paths {
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            match crate::storage::read(p) {
                Ok(bytes) if name.to_lowercase().ends_with(".zip") => match crate::projects::read_zip(&bytes) {
                    // Skip what macOS adds when it zips a folder.
                    Ok(inner) => files.extend(inner.into_iter().filter(|(n, _)| (n.ends_with(".biocartoon") || n.ends_with(".json")) && !n.contains("__MACOSX/"))),
                    Err(e) => failed.push(format!("{name}: {e}")),
                },
                Ok(bytes) => files.push((name, bytes)),
                Err(e) => failed.push(format!("{name}: {e}")),
            }
        }
        let uploads = self.app_dir.join("uploads");
        let mut ok = Vec::new();
        for (name, bytes) in files {
            match crate::projects::from_file(&bytes, &uploads).and_then(|d| self.store.create(&d).map(|m| (d, m))) {
                Ok(dm) => ok.push(dm),
                Err(e) => failed.push(format!("{name}: {e}")),
            }
        }
        for (d, m) in &ok {
            // It came from a file the user has, so it is already backed up.
            self.store.mark_backed_up(&m.id);
            self.request_thumb(&m.id, d);
        }
        self.refresh_projects();
        match (ok.len(), failed.len()) {
            (0, 0) => self.toast_err("No .biocartoon files found to import"),
            (n, 0) => self.toast(format!("Imported {n} project{}", if n == 1 { "" } else { "s" })),
            (n, _) => self.toast_err(format!("Imported {n}; couldn't import {}", failed.join("; "))),
        }
    }

    /// Download every project as a `.biocartoon` file, together in one .zip (stored: the files are
    /// mostly base64 images, which barely compress). Each project is then marked backed up.
    fn backup_all(&mut self) {
        let projects = self.store.list();
        if projects.is_empty() {
            return self.toast("There are no projects to back up yet");
        }
        let stamp = chrono::Local::now().format("%Y-%m-%d").to_string();
        let folder = format!("biocartoon-backup-{stamp}");
        let (mut zip, mut bytes, mut names) = (crate::offline::ZipWriter::default(), Vec::new(), HashSet::new());
        let (mut saved, mut failed) = (Vec::new(), Vec::new());
        for p in &projects {
            let file = match self.store.load(&p.id).and_then(|d| crate::projects::to_file(&d)) {
                Ok(f) => f,
                Err(e) => {
                    failed.push(format!("{}: {e}", p.name));
                    continue;
                }
            };
            let mut name = Self::slugify(&p.name);
            if !names.insert(name.clone()) {
                name = format!("{name}-{}", &p.id[..8.min(p.id.len())]);
                names.insert(name.clone());
            }
            match zip.entry(&format!("{folder}/{name}.biocartoon"), &file) {
                Ok(header) => {
                    bytes.extend(header);
                    bytes.extend(file);
                    saved.push(p.id.clone());
                }
                Err(e) => return self.toast_err(format!("backup: {e}")),
            }
        }
        if saved.is_empty() {
            return self.toast_err(format!("Couldn't back up {}", failed.join("; ")));
        }
        bytes.extend(zip.finish());
        let Some(path) = crate::platform::save_path("zip", "zip", &format!("{folder}.zip")) else { return };
        if let Err(e) = crate::platform::write_output(&path, &bytes) {
            return self.toast_err(format!("backup: {e}"));
        }
        for id in &saved {
            self.store.mark_backed_up(id);
        }
        self.refresh_projects();
        if failed.is_empty() {
            let n = saved.len();
            self.toast(format!("Backed up {n} project{}. To restore, open the .zip from the home screen.", if n == 1 { "" } else { "s" }));
        } else {
            self.toast_err(format!("Backup saved, but left out {}", failed.join("; ")));
        }
    }

    /// Import a `.biocartoon` file as a new project and open it.
    fn open_file(&mut self, p: &Path) {
        let doc = crate::storage::read(p).map_err(|e| e.to_string()).and_then(|b| crate::projects::from_file(&b, &self.app_dir.join("uploads")));
        match doc.and_then(|d| self.store.create(&d).map(|m| (d, m))) {
            Ok((d, m)) => {
                self.store.mark_backed_up(&m.id);
                self.close_project();
                self.load_into_editor(d.clone(), Some(m.id.clone()));
                self.request_thumb(&m.id, &d);
                self.refresh_projects();
            }
            Err(e) => self.toast_err(format!("open: {e}")),
        }
    }

    /// Write the open document to its project (creating the project for a new document). Returns
    /// false on error. A new document that was never edited is not saved.
    pub fn save_current(&mut self, snapshot: Snapshot, note: &str) -> bool {
        let id = match &self.project {
            Some(id) => id.clone(),
            None => {
                if self.doc == Document::default() {
                    return true;
                }
                match self.store.create(&self.doc) {
                    Ok(m) => {
                        self.project = Some(m.id.clone());
                        self.saved_doc = Some(self.doc.clone());
                        self.dirty = false;
                        self.request_thumb(&m.id, &self.doc.clone());
                        return true;
                    }
                    Err(e) => {
                        self.toast_err(format!("save: {e}"));
                        return false;
                    }
                }
            }
        };
        let changed = self.saved_doc.as_ref() != Some(&self.doc);
        // Unchanged documents may still need a version (autosaves only snapshot every 10 min).
        if !changed && snapshot == Snapshot::IfDue {
            return true;
        }
        match self.store.save(&id, &self.doc, snapshot, note) {
            Ok(()) => {
                self.saved_doc = Some(self.doc.clone());
                self.dirty = false;
                if changed {
                    self.request_thumb(&id, &self.doc.clone());
                }
                true
            }
            Err(e) => {
                self.toast_err(format!("save: {e}"));
                false
            }
        }
    }

    /// Save the open project (recording a version) before switching away from it.
    fn close_project(&mut self) {
        if self.screen == Screen::Editor {
            self.save_current(Snapshot::IfChanged, "Edited");
        }
    }

    fn go_home(&mut self) {
        self.close_project();
        self.project = None;
        self.saved_doc = None;
        self.doc = Document::default();
        self.history.clear();
        self.future.clear();
        self.selection.clear();
        self.selected_guide = None;
        self.screen = Screen::Home;
        self.refresh_projects();
    }

    /// Cmd+S saves the project (and records a version); "Save a copy" writes a standalone file.
    pub(crate) fn save(&mut self, save_as: bool) {
        if !save_as {
            if self.save_current(Snapshot::IfChanged, "Saved") {
                self.toast("Saved");
            }
            return;
        }
        if self.save_file(&self.doc.clone()) {
            if let Some(id) = self.project.clone() {
                self.store.mark_backed_up(&id);
            }
        }
    }

    /// Write `doc` as a standalone `.biocartoon` file (with its images) wherever the user picks.
    /// Returns whether the file was written.
    fn save_file(&mut self, doc: &Document) -> bool {
        let Some(path) = crate::platform::save_path("biocartoon", "biocartoon", &format!("{}.biocartoon", Self::slugify(&doc.title))) else {
            return false;
        };
        self.write_project_file(doc, &path)
    }

    fn write_project_file(&mut self, doc: &Document, path: &Path) -> bool {
        match crate::projects::to_file(doc).and_then(|b| crate::platform::write_output(path, &b).map_err(|e| e.to_string())) {
            Ok(()) => {
                self.toast(format!("Saved a copy to {}", path.display()));
                true
            }
            Err(e) => {
                self.toast_err(format!("save: {e}"));
                false
            }
        }
    }

    fn autosave(&mut self, now: f64) {
        if self.screen != Screen::Editor || now - self.last_autosave < 2.0 {
            return;
        }
        self.last_autosave = now;
        if self.saved_doc.as_ref() != Some(&self.doc) {
            self.dirty = true;
            self.save_current(Snapshot::IfDue, "Autosaved");
        }
    }

    // ---------- projects (home screen commands) ----------

    fn rename_project(&mut self, id: &str, name: &str) {
        if let Err(e) = self.store.rename(id, name) {
            self.toast_err(format!("rename: {e}"));
        }
        self.refresh_projects();
    }

    fn copy_project(&mut self, id: &str) {
        match self.store.duplicate(id) {
            Ok(m) => {
                if let Ok(doc) = self.store.load(&m.id) {
                    self.request_thumb(&m.id, &doc);
                }
                self.toast(format!("Created \"{}\"", m.name));
            }
            Err(e) => self.toast_err(format!("copy: {e}")),
        }
        self.refresh_projects();
    }

    fn delete_project(&mut self, id: &str) {
        let thumb = format!("file:{}", self.store.thumb_path(id).display());
        match self.store.delete(id) {
            Ok(()) => {
                self.account_forget(id);
                self.images.forget(&thumb);
                if self.project.as_deref() == Some(id) {
                    self.project = None;
                    self.saved_doc = None;
                }
            }
            Err(e) => self.toast_err(format!("delete: {e}")),
        }
        self.refresh_projects();
    }

    fn restore_version(&mut self, id: &str, time: i64) {
        match self.store.restore(id, time) {
            Ok(doc) => {
                if self.project.as_deref() == Some(id) && self.screen == Screen::Editor {
                    self.commit(); // Cmd+Z goes back to what was on screen
                    self.doc = doc.clone();
                    self.saved_doc = Some(doc.clone());
                    self.cur_slide = 0;
                    self.selection.clear();
                    self.selected_guide = None;
                    self.dirty = false;
                }
                self.request_thumb(id, &doc);
                self.toast(format!("Restored the version from {}", crate::projects::format_time(time)));
            }
            Err(e) => self.toast_err(format!("restore: {e}")),
        }
        self.refresh_projects();
    }

    fn export_project(&mut self, id: &str) {
        match self.store.load(id) {
            Ok(doc) => {
                self.export_doc = Some(doc);
                self.export_project = Some(id.to_string());
                self.show_export = true;
            }
            Err(e) => self.toast_err(format!("export: {e}")),
        }
    }

    /// Put the project file and a PNG of its first slide in a new Mail message.
    fn email_project(&mut self, ctx: &egui::Context, id: &str) {
        if cfg!(target_arch = "wasm32") {
            return self.toast("Email isn't available in the web version yet. Use Export instead.");
        }
        if self.export_job.is_some() {
            self.toast("An export is already running; try again in a moment");
            return;
        }
        let doc = match self.store.load(id) {
            Ok(d) => d,
            Err(e) => return self.toast_err(format!("email: {e}")),
        };
        let slug = Self::slugify(&doc.title);
        let dir = self.app_dir.join("outbox").join(format!("{slug}-{}", chrono::Local::now().format("%Y%m%d-%H%M%S")));
        let file = dir.join(format!("{slug}.biocartoon"));
        let written = std::fs::create_dir_all(&dir).map_err(|e| e.to_string()).and_then(|_| crate::projects::to_file(&doc)).and_then(|b| std::fs::write(&file, b).map_err(|e| e.to_string()));
        if let Err(e) = written {
            return self.toast_err(format!("email: {e}"));
        }
        let png = dir.join(format!("{slug}.png"));
        let opts = ExportOptions { format: ExportFormat::Png, dpi: 150, transparent: false, individual: false, crop: false, all_slides: false, pdf_pages: false, jpg_quality: 90 };
        self.pending_email = Some((doc.title.clone(), vec![png.clone(), file]));
        let job = self.export_job_for(&doc, &[0], png, opts);
        self.export_job = Some(crate::export::start(ctx.clone(), job));
        self.toast("Preparing email...");
    }

    // ---------- thumbnails ----------

    /// Re-render a project's tile thumbnail (written to its `thumb.png`).
    pub fn request_thumb(&mut self, id: &str, doc: &Document) {
        let path = self.store.thumb_path(id);
        let key = format!("file:{}", path.display());
        if self.thumbs_pending.contains(&key) {
            return;
        }
        self.start_thumb(key, Some(path), doc);
    }

    /// Render `doc`'s first slide on a worker thread; the result lands in the image cache under
    /// `key` (and in `save_to`, if given).
    pub fn start_thumb(&mut self, key: String, save_to: Option<PathBuf>, doc: &Document) {
        let Some(slide) = doc.slides.first().cloned() else { return };
        let (requests, cached) = self.render_inputs(&slide);
        let (page, bg) = (doc.page_rect(), doc.background);
        let tx = self.thumb_tx.clone();
        let ctx = self.ctx.clone();
        self.thumbs_pending.insert(key.clone());
        crate::platform::spawn("thumbnail", move || {
            let res = crate::export::thumbnail(&ctx, &slide, page, bg, &requests, &cached, 480);
            if let (Ok(img), Some(p)) = (&res, &save_to) {
                // Written atomically: the home screen may read thumb.png at any moment.
                let mut png = Vec::new();
                let written = img
                    .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
                    .map_err(|e| e.to_string())
                    .and_then(|_| crate::storage::write(p, &png).map_err(|e| e.to_string()));
                if let Err(e) = written {
                    log::warn!("thumbnail {}: {e}", p.display());
                }
            }
            let _ = tx.send((key, res));
            ctx.request_repaint();
        });
    }

    fn poll_thumbs(&mut self, ctx: &egui::Context) {
        while let Ok((key, res)) = self.thumb_rx.try_recv() {
            self.thumbs_pending.remove(&key);
            match res {
                Ok(img) => self.images.insert_image(ctx, &key, img),
                Err(e) => log::warn!("thumbnail {key}: {e}"),
            }
        }
    }

    /// Ask where to save, using the format chosen in the export dialog.
    fn pick_export_path(&mut self) {
        let fmt = self.settings.export.format;
        let (doc, slide) = self.export_source();
        // The project file always holds every slide.
        let name = if fmt == ExportFormat::Biocartoon || (self.settings.export.all_slides && doc.slides.len() > 1) {
            format!("{}.{}", Self::slugify(&doc.title), fmt.ext())
        } else {
            format!("{}-slide{}.{}", Self::slugify(&doc.title), slide + 1, fmt.ext())
        };
        let Some(p) = crate::platform::save_path(fmt.label(), fmt.ext(), &name) else {
            self.export_doc = None;
            return;
        };
        self.actions.push(Action::Export(p));
    }

    /// The document and slide the export dialog acts on: a project picked on the home screen, or
    /// the editor's current slide.
    fn export_source(&self) -> (&Document, usize) {
        match &self.export_doc {
            Some(d) => (d, 0),
            None => (&self.doc, self.cur_slide),
        }
    }

    fn start_export(&mut self, ctx: &egui::Context, path: PathBuf) {
        if self.export_job.is_some() {
            self.toast("An export is already running");
            return;
        }
        let (doc, slide, project) = match self.export_doc.take() {
            Some(d) => (d, 0, self.export_project.take()),
            None => (self.doc.clone(), self.cur_slide, self.project.clone()),
        };
        if self.settings.export.format == ExportFormat::Biocartoon {
            if self.write_project_file(&doc, &path) {
                if let Some(id) = project {
                    self.store.mark_backed_up(&id);
                    self.refresh_projects();
                }
            }
            return;
        }
        let slides: Vec<usize> = if self.settings.export.all_slides { (0..doc.slides.len()).collect() } else { vec![slide] };
        // Its credits pop up once it is done.
        let credits = if slides.len() > 1 { crate::credits::collect(&doc.slides, 1) } else { crate::credits::collect([&doc.slides[slide]], slide + 1) };
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| Self::slugify(&doc.title));
        self.export_credits = (!credits.is_empty()).then_some((credits, stem));
        let job = self.export_job_for(&doc, &slides, path, self.settings.export.clone());
        self.export_job = Some(crate::export::start(ctx.clone(), job));
        self.toast("Exporting...");
    }

    /// `slides` are 0-based indices into `doc.slides`.
    fn export_job_for(&self, doc: &Document, slides: &[usize], path: PathBuf, opts: ExportOptions) -> crate::export::ExportJob {
        let slides: Vec<(usize, Slide)> = slides.iter().map(|&i| i.min(doc.slides.len() - 1)).map(|i| (i + 1, doc.slides[i].clone())).collect();
        let mut requests = HashMap::new();
        let mut fallbacks = HashMap::new();
        for it in slides.iter().flat_map(|(_, s)| &s.items) {
            if let ItemKind::Image { source, .. } = &it.kind {
                let key = source.cache_key();
                if let Some(img) = self.images.cpu_image(&key) {
                    fallbacks.insert(key.clone(), img);
                }
                if let Ok(req) = self.images.request_for(source) {
                    requests.insert(key, req);
                }
            }
        }
        crate::export::ExportJob { path, opts, slides, page: doc.page_rect(), background: doc.background, requests, fallbacks, downloads: HashMap::new() }
    }

    fn poll_export(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.export_job else { return };
        match rx.try_recv() {
            Ok(res) => {
                self.export_job = None;
                let credits = self.export_credits.take();
                if let Some((subject, files)) = self.pending_email.take() {
                    // Without the PNG, still send the project file.
                    let files: Vec<PathBuf> = files.into_iter().filter(|f| f.exists()).collect();
                    if let Err(e) = res {
                        log::warn!("email preview: {e}");
                    }
                    open_mail(&subject, &files);
                    return;
                }
                match res {
                    Ok(msg) => {
                        self.toast(msg);
                        if credits.is_some() {
                            self.attributions_export = credits;
                            self.export_saved_at = Some(ctx.input(|i| i.time));
                            self.show_attributions = true;
                        }
                    }
                    Err(e) => self.toast_err(format!("export: {e}")),
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.export_job = None;
                self.pending_email = None;
                self.export_credits = None;
                self.toast_err("export failed");
            }
        }
    }

    fn export_window(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let mut go = false;
        let mut cancel = false;
        let (doc, slide) = self.export_source();
        let page = doc.page_rect();
        let count_images = |s: &Slide| s.items.iter().filter(|it| matches!(it.kind, ItemKind::Image { .. })).count();
        let slide_images = count_images(&doc.slides[slide]);
        let all_images: usize = doc.slides.iter().map(count_images).sum();
        let image_bounds = crate::export::images_bounds(&doc.slides[slide]);
        let n_slides = doc.slides.len();
        let multi = n_slides > 1;
        let all = self.settings.export.all_slides && multi;
        let n_images = if all { all_images } else { slide_images };
        let project_file = self.settings.export.format == ExportFormat::Biocartoon;
        let heading = match (&self.export_doc, all) {
            (Some(_), _) => format!("Export \"{}\"", doc.title),
            (None, _) if project_file => "Export project".into(),
            (None, true) => "Export slides".into(),
            (None, false) => "Export slide".into(),
        };
        // While the options change, the last size stays up, marked as updating.
        let size_note = match &self.export_size {
            Some((o, res)) => {
                let updating = if *o == self.settings.export { "" } else { " ..." };
                match res {
                    Ok(est) => {
                        let files = if est.files > 1 { format!(" in {} files", est.files) } else { String::new() };
                        format!("{}{}{files}{updating}", if est.approx { "about " } else { "" }, Self::file_size(est.bytes))
                    }
                    Err(_) => format!("unknown{updating}"),
                }
            }
            None => "working it out...".into(),
        };
        let size_row = |ui: &mut egui::Ui| {
            ui.label("File size");
            ui.label(egui::RichText::new(&size_note).color(theme::text_dim()));
            ui.end_row();
        };
        let opts = &mut self.settings.export;
        let modal = egui::Modal::new(egui::Id::new("export_modal")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.heading(heading);
            if multi && !all && !project_file && self.export_doc.is_some() {
                ui.label(egui::RichText::new("Exports the first slide. Open the project to export another slide alone.").color(theme::text_dim()));
            }
            ui.add_space(8.0);
            egui::Grid::new("export_grid").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
                ui.label("Format");
                ui.horizontal(|ui| {
                    for f in ExportFormat::ALL {
                        ui.selectable_value(&mut opts.format, f, f.label());
                    }
                });
                ui.end_row();

                // The project file has no rendering options, just a note where the resolution note goes.
                if project_file {
                    size_row(ui);
                    let slides = if multi { format!("all {n_slides} slides") } else { "the slide".into() };
                    let note = format!(".biocartoon format allows for sharing editable slides: {slides} and their images in one file, which anyone can open with Open file.");
                    ui.label("");
                    // Wrap within the dialog's width instead of stretching it.
                    ui.allocate_ui(egui::vec2(360.0, 0.0), |ui| {
                        ui.add(egui::Label::new(egui::RichText::new(note).color(theme::text_dim())).wrap());
                    });
                    ui.end_row();
                    return;
                }

                ui.label("Resolution (dpi)");
                ui.add_enabled_ui(opts.format.is_raster(), |ui| {
                    ui.horizontal(|ui| {
                        const PRESETS: [u32; 4] = [72, 150, 300, 600];
                        // The custom box stays empty while a preset is chosen, so the value never shows twice.
                        let id = ui.id().with("custom_dpi");
                        let mut custom: String = ui.data_mut(|d| d.get_temp(id)).unwrap_or_else(|| {
                            if PRESETS.contains(&opts.dpi) { String::new() } else { opts.dpi.to_string() }
                        });
                        for d in PRESETS {
                            if ui.selectable_value(&mut opts.dpi, d, d.to_string()).clicked() {
                                custom.clear();
                            }
                        }
                        let r = ui.add(egui::TextEdit::singleline(&mut custom).desired_width(44.0).hint_text("other"));
                        if r.changed() {
                            custom.retain(|c| c.is_ascii_digit());
                            if let Ok(v) = custom.parse::<u32>() {
                                opts.dpi = v.clamp(crate::export::MIN_DPI, crate::export::MAX_DPI);
                            }
                        }
                        if r.lost_focus() {
                            custom = if PRESETS.contains(&opts.dpi) { String::new() } else { opts.dpi.to_string() };
                        }
                        ui.data_mut(|d| d.insert_temp(id, custom));
                    });
                });
                ui.end_row();

                ui.label("");
                let note = if !opts.format.is_raster() {
                    "Vector output: scales to any size.".to_string()
                } else if opts.individual {
                    format!("Each image at {} dpi.", opts.dpi)
                } else if all && opts.crop {
                    format!("Each slide at {} dpi.", opts.dpi)
                } else {
                    let region = if opts.crop { image_bounds.unwrap_or(page) } else { page };
                    let [w, h] = crate::export::pixel_size(region, opts.dpi);
                    if all { format!("{w} x {h} px each") } else { format!("{w} x {h} px") }
                };
                ui.label(egui::RichText::new(note).color(theme::text_dim()));
                ui.end_row();

                if opts.format == ExportFormat::Jpg {
                    ui.label("Quality");
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Smaller").color(theme::text_dim()));
                        ui.add(egui::Slider::new(&mut opts.jpg_quality, 10..=100).suffix("%").trailing_fill(true))
                            .on_hover_text("JPEG compression: lower quality makes a smaller file, with more visible artifacts.");
                        ui.label(egui::RichText::new("Sharper").color(theme::text_dim()));
                    });
                    ui.end_row();
                }

                size_row(ui);
            });
            ui.add_space(6.0);
            if opts.format == ExportFormat::Png {
                ui.checkbox(&mut opts.transparent, "Transparent background");
            }
            if !project_file {
                ui.add_enabled_ui(n_images > 0 && !opts.individual, |ui| {
                    ui.checkbox(&mut opts.crop, "Crop to images")
                        .on_hover_text("Crop the sheet to the area covered by the images, instead of exporting the whole slide.");
                });
                ui.add_enabled_ui(n_images > 0, |ui| {
                    let counts = if multi { format!("{slide_images} on this slide, {all_images} on all slides") } else { format!("{slide_images} on this slide") };
                    ui.checkbox(&mut opts.individual, format!("Export images individually ({counts})"))
                        .on_hover_text("Save one file per image, cropped tightly to the image, instead of the whole slide. Files are named <name>-01-<image>, <name>-02-<image>, ...");
                });
                ui.add_enabled_ui(multi, |ui| {
                    ui.checkbox(&mut opts.all_slides, format!("Export all slides ({n_slides})"))
                        .on_hover_text("Save every slide, one file each, named <name>-slide1, <name>-slide2, ... (a PDF can instead hold them all)");
                });
                if all && opts.format == ExportFormat::Pdf && !opts.individual {
                    ui.indent("pdf_pages", |ui| {
                        ui.checkbox(&mut opts.pdf_pages, "One PDF, a page per slide")
                            .on_hover_text("Off: one PDF per slide, named <name>-slide1.pdf, <name>-slide2.pdf, ...");
                    });
                }
            }
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let can = project_file || !((opts.individual || opts.crop) && n_images == 0);
                if ui.add_enabled(can, egui::Button::new(egui::RichText::new("Export...").color(Color32::WHITE)).fill(theme::accent())).clicked() { go = true; }
                if ui.button("Cancel").clicked() { cancel = true; }
            });
        });
        if modal.should_close() {
            open = false;
        }
        if n_images == 0 {
            self.settings.export.individual = false;
            self.settings.export.crop = false;
        }
        self.show_export = open && !go && !cancel;
        if self.show_export {
            self.update_export_size(ctx);
        } else {
            self.export_size = None;
            self.export_size_rx = None;
        }
        if !self.show_export && !go {
            self.export_doc = None;
        }
        if go {
            self.pick_export_path();
        }
    }

    /// Keep the export dialog's file size up to date: once the last estimate is in, start another
    /// if the options have changed since.
    fn update_export_size(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.export_size_rx {
            match rx.try_recv() {
                Ok(done) => {
                    self.export_size = Some(done);
                    self.export_size_rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return ctx.request_repaint_after(std::time::Duration::from_millis(100)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.export_size_rx = None,
            }
        }
        let opts = self.settings.export.clone();
        if self.export_size.as_ref().is_some_and(|(o, _)| *o == opts) {
            return;
        }
        let (doc, slide) = self.export_source();
        let doc = doc.clone();
        let slides: Vec<usize> = if opts.all_slides { (0..doc.slides.len()).collect() } else { vec![slide] };
        // Never written: only its name matters.
        let job = (opts.format != ExportFormat::Biocartoon).then(|| self.export_job_for(&doc, &slides, PathBuf::from(format!("export.{}", opts.format.ext())), opts.clone()));
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx2 = ctx.clone();
        self.export_size_rx = Some(rx);
        crate::platform::spawn("export size", move || {
            let res = match job {
                Some(job) => crate::export::estimate(&ctx2, &job),
                None => crate::projects::to_file(&doc).map(|b| crate::export::SizeEstimate { bytes: b.len() as u64, files: 1, approx: false }),
            };
            let _ = tx.send((opts, res));
            ctx2.request_repaint();
        });
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }

    /// `bytes` as KB / MB / GB, with a decimal below 10.
    fn file_size(bytes: u64) -> String {
        let (v, unit) = match bytes {
            b if b >= 1_000_000_000 => (b as f64 / 1e9, "GB"),
            b if b >= 1_000_000 => (b as f64 / 1e6, "MB"),
            b => ((b as f64 / 1e3).max(1.0), "KB"),
        };
        if v < 10.0 { format!("{v:.1} {unit}") } else { format!("{v:.0} {unit}") }
    }

    // ---------- templates ----------

    pub fn template_names() -> &'static [(&'static str, &'static str)] {
        &[
            ("Blank", "Empty slide"),
            ("Title + figure", "Heading at the top, large figure area"),
            ("Two panels", "A and B panels side by side with labels"),
            ("Pathway", "Three steps connected by arrows"),
            ("Four-panel grid", "2 x 2 labelled panels"),
            ("Figure with legend", "Figure area with a legend table"),
        ]
    }

    pub(crate) fn use_template(&mut self, idx: usize) {
        self.commit();
        let w = self.doc.width;
        let h = self.doc.height;
        let mut slide = Slide { name: Self::template_names()[idx].0.to_string(), items: Vec::new() };
        let mut add = |doc: &mut Document, name: &str, kind: ItemKind, r: Rect| {
            let id = doc.alloc_id();
            slide.items.push(Item::new(id, name, kind, r.min, r.size()));
        };
        let text = |t: &str, size: f32, bold: bool, align: TextAlign| ItemKind::text(t, size, [30, 30, 30, 255], align, bold);
        let panel = || ItemKind::Shape { shape: ShapeKind::RoundedRectangle, fill: [245, 247, 250, 255], stroke: [200, 205, 215, 255], stroke_width: 1.5 };
        let m = w * 0.05;
        match idx {
            1 => {
                add(&mut self.doc, "Title", text("Figure title", 28.0, true, TextAlign::Left), Rect::from_min_size(Pos2::new(m, m), Vec2::new(w - 2.0 * m, 44.0)));
                add(&mut self.doc, "Figure area", panel(), Rect::from_min_size(Pos2::new(m, m + 60.0), Vec2::new(w - 2.0 * m, h - 2.0 * m - 60.0)));
            }
            2 => {
                let pw = (w - 3.0 * m) / 2.0;
                for (i, l) in ["A", "B"].iter().enumerate() {
                    let x = m + i as f32 * (pw + m);
                    add(&mut self.doc, &format!("Panel {l}"), panel(), Rect::from_min_size(Pos2::new(x, m + 36.0), Vec2::new(pw, h - 2.0 * m - 36.0)));
                    add(&mut self.doc, &format!("Label {l}"), text(l, 24.0, true, TextAlign::Left), Rect::from_min_size(Pos2::new(x, m), Vec2::new(40.0, 32.0)));
                }
            }
            3 => {
                let n = 3;
                let bw = (w - 2.0 * m - 2.0 * 80.0) / n as f32;
                for i in 0..n {
                    let x = m + i as f32 * (bw + 80.0);
                    let r = Rect::from_min_size(Pos2::new(x, h * 0.3), Vec2::new(bw, h * 0.4));
                    add(&mut self.doc, &format!("Step {}", i + 1), panel(), r);
                    add(&mut self.doc, &format!("Step {} label", i + 1), text(&format!("Step {}", i + 1), 18.0, true, TextAlign::Center), Rect::from_min_size(Pos2::new(x, h * 0.72), Vec2::new(bw, 30.0)));
                    if i + 1 < n {
                        add(&mut self.doc, "Arrow", ItemKind::Line { kind: LineKind::Arrow, color: [60, 60, 70, 255], width: 3.0, style: LineStyle::Solid, points: Vec::new() }, Rect::from_min_size(Pos2::new(x + bw + 10.0, h * 0.5), Vec2::new(60.0, 0.0)));
                    }
                }
            }
            4 => {
                let pw = (w - 3.0 * m) / 2.0;
                let ph_ = (h - 3.0 * m) / 2.0;
                for (i, l) in ["A", "B", "C", "D"].iter().enumerate() {
                    let x = m + (i % 2) as f32 * (pw + m);
                    let y = m + (i / 2) as f32 * (ph_ + m);
                    add(&mut self.doc, &format!("Panel {l}"), panel(), Rect::from_min_size(Pos2::new(x, y), Vec2::new(pw, ph_)));
                    add(&mut self.doc, &format!("Label {l}"), text(l, 22.0, true, TextAlign::Left), Rect::from_min_size(Pos2::new(x + 8.0, y + 6.0), Vec2::new(40.0, 30.0)));
                }
            }
            5 => {
                add(&mut self.doc, "Figure area", panel(), Rect::from_min_size(Pos2::new(m, m), Vec2::new(w * 0.62, h - 2.0 * m)));
                add(
                    &mut self.doc,
                    "Legend",
                    ItemKind::Table {
                        rows: 4,
                        cols: 2,
                        cells: vec!["Symbol".into(), "Meaning".into(), "".into(), "".into(), "".into(), "".into(), "".into(), "".into()],
                        header: true,
                        border: [190, 195, 205, 255],
                        fill: [255, 255, 255, 255],
                        header_fill: [230, 236, 245, 255],
                        text_color: [30, 30, 30, 255],
                        font_size: 14.0,
                    },
                    Rect::from_min_size(Pos2::new(m + w * 0.62 + m, m), Vec2::new(w - 3.0 * m - w * 0.62, 4.0 * 32.0)),
                );
            }
            _ => {}
        }
        self.doc.slides.insert(self.cur_slide + 1, slide);
        self.cur_slide += 1;
        self.selection.clear();
    }

    // ---------- actions ----------

    fn apply(&mut self, a: Action, ctx: &egui::Context) {
        match a {
            Action::InsertAsset(row) => self.insert_asset(&row),
            Action::InsertRandom => {
                let b = crate::projects::random_bytes();
                let seed = u64::from_le_bytes(b[..8].try_into().unwrap());
                self.catalog.send(Req::Random { seed });
            }
            Action::RemoveUserAsset(row) => {
                // Out of the results now; the reply refreshes counts and browse views.
                let before = self.icons.results.len();
                self.icons.results.retain(|r| r.id != row.id);
                if self.icons.results.len() < before {
                    self.icons.total = self.icons.total.map(|n| (n - 1).max(0));
                }
                self.catalog.send(Req::RemoveUser { id: row.id.clone() });
                self.relink_generated(&row.id, None);
                self.toast(format!("Removed \"{}\" from your library", row.title));
            }
            Action::ToggleFavorite(id) => self.toggle_favorite(id),
            Action::InsertFile(p) => self.insert_file(&p),
            Action::InsertText { text, size, bold } => {
                let id = self.doc.alloc_id();
                let kind = ItemKind::text(text.clone(), size, [30, 30, 30, 255], TextAlign::Left, bold);
                let w = (text.len() as f32 * size * 0.6).clamp(120.0, self.doc.width * 0.6);
                self.insert_item(Item::new(id, text, kind, Pos2::ZERO, Vec2::new(w, size * 1.6)));
            }
            Action::ArmText(on) => {
                self.finish_text_edit();
                self.placing_text = on;
                if on {
                    self.drawing = false;
                }
            }
            Action::PlaceText(p) => {
                self.placing_text = false;
                self.commit();
                let id = self.doc.alloc_id();
                let size = 18.0;
                let kind = ItemKind::text("", size, [30, 30, 30, 255], TextAlign::Left, false);
                let item = Item::new(id, "Text", kind, Pos2::new(p.x, p.y - size * 0.7), Vec2::new(280.0, size * 1.4));
                // Typing into the new box is part of the same undo step as placing it.
                self.pending_edits.insert(id, serde_json::to_string(&item).unwrap_or_default());
                self.slide_mut().items.push(item);
                self.select(vec![id]);
                self.start_text_edit(id);
            }
            Action::EditText(id) => self.start_text_edit(id),
            Action::EditCell(id, cell) => self.start_cell_edit(id, cell),
            Action::InsertShape(s) => {
                let id = self.doc.alloc_id();
                let kind = ItemKind::Shape { shape: s, fill: [120, 170, 230, 255], stroke: [40, 70, 120, 255], stroke_width: 2.0 };
                let d = self.doc.width.min(self.doc.height) * 0.2;
                self.insert_item(Item::new(id, s.label(), kind, Pos2::ZERO, Vec2::splat(d)));
            }
            Action::InsertLine(l) => {
                let id = self.doc.alloc_id();
                let kind = ItemKind::Line { kind: l, color: [40, 40, 50, 255], width: 3.0, style: LineStyle::Solid, points: Vec::new() };
                let d = self.doc.width * 0.2;
                let dy = if l.needs_height() { d * 0.5 } else { 0.0 };
                self.insert_item(Item::new(id, l.label(), kind, Pos2::ZERO, Vec2::new(d, dy)));
            }
            Action::ArmDraw(on) => {
                self.finish_text_edit();
                self.drawing = on;
                self.stroke.clear();
                if on {
                    self.placing_text = false;
                    self.crop_mode = false;
                }
            }
            Action::PlaceDrawing(pts) => {
                let r = Rect::from_points(&pts);
                // A click without a drag (or a tiny scribble) draws nothing.
                if pts.len() >= 2 && r.width().max(r.height()) >= 1.0 {
                    self.commit();
                    let id = self.doc.alloc_id();
                    let kind = ItemKind::Line { kind: LineKind::Freehand, color: self.pen_color, width: self.pen_width, style: LineStyle::Solid, points: Vec::new() };
                    let mut item = Item::new(id, LineKind::Freehand.label(), kind, Pos2::ZERO, Vec2::ZERO);
                    crate::canvas::set_freehand_path(&mut item, &pts);
                    // Not selected, so the next stroke can start right away.
                    self.slide_mut().items.push(item);
                }
            }
            Action::InsertTable(rows, cols) => {
                let id = self.doc.alloc_id();
                let kind = ItemKind::Table {
                    rows,
                    cols,
                    cells: vec![String::new(); rows * cols],
                    header: true,
                    border: [190, 195, 205, 255],
                    fill: [255, 255, 255, 255],
                    header_fill: [230, 236, 245, 255],
                    text_color: [30, 30, 30, 255],
                    font_size: 14.0,
                };
                self.insert_item(Item::new(id, "Table", kind, Pos2::ZERO, Vec2::new(cols as f32 * 110.0, rows as f32 * 32.0)));
            }
            Action::InsertCode { code, language, theme, line_numbers, window, rounded } => {
                let id = self.doc.alloc_id();
                let font_size = 14.0;
                let size = crate::code::fit_size(ctx, &code, font_size, line_numbers, window);
                let kind = ItemKind::Code { code, language, theme, font_size, line_numbers, window, rounded };
                self.insert_item(Item::new(id, "Code", kind, Pos2::ZERO, Vec2::new(size[0], size[1])));
            }
            Action::UseTemplate(i) => self.use_template(i),
            Action::Select(ids) => self.select(ids),
            Action::ToggleSelect(id) => {
                if let Some(i) = self.selection.iter().position(|x| *x == id) {
                    self.selection.remove(i);
                } else {
                    self.selection.push(id);
                }
                self.expand_groups();
            }
            Action::ClearSelection => self.select(Vec::new()),
            Action::Delete => self.delete_selection(),
            Action::AddGuide(vertical) => {
                self.commit();
                let pos = if vertical { self.doc.width } else { self.doc.height } / 2.0;
                self.doc.guides.push(crate::doc::Guide { vertical, pos });
                self.select(Vec::new());
                self.selected_guide = Some(self.doc.guides.len() - 1);
            }
            Action::ClearGuides => {
                if !self.doc.guides.is_empty() {
                    self.commit();
                    self.doc.guides.clear();
                }
                self.selected_guide = None;
            }
            Action::Duplicate => self.duplicate(),
            Action::Copy => self.copy_selection(),
            Action::Cut => {
                self.copy_selection();
                self.delete_selection();
            }
            Action::Paste => self.paste(),
            Action::Group => self.group_selection(),
            Action::Ungroup => self.ungroup_selection(),
            Action::ToggleLock => self.toggle_lock(),
            Action::Align(k) => self.align(k),
            Action::Arrange(k) => self.arrange(k),
            Action::FlipH => self.flip(true),
            Action::FlipV => self.flip(false),
            Action::SetCrop(on) => self.crop_mode = on && self.single_selected().is_some_and(|i| matches!(i.kind, ItemKind::Image { .. })),
            Action::Ai(op) => self.run_ai(op),
            Action::OpenAiEdit => {
                if let Some(it) = self.single_selected() {
                    if matches!(it.kind, ItemKind::Image { .. }) {
                        self.ai_edit = Some(AiEdit {
                            item: it.id,
                            messages: vec![(false, AI_EDIT_GREETING.into())],
                            input: String::new(),
                            waiting: false,
                            stage: AiChatStage::Start,
                        });
                    } else {
                        self.toast_err("Edit with AI works on images");
                    }
                }
            }
            Action::SendChat(msg) => {
                if let Some(ce) = &mut self.ai_edit {
                    ce.stage = AiChatStage::Start;
                    ce.messages.push((true, msg.clone()));
                    ce.waiting = true;
                    let item = ce.item;
                    let history = ce.messages.clone();
                    // Try the edit itself first; the chat reply explains what happened.
                    self.ai.submit(AiJob::Chat { item, history, message: msg.clone() });
                    if let Some(it) = self.item(item).cloned() {
                        if let ItemKind::Image { source, .. } = &it.kind {
                            if let Some(img) = self.images.cpu_image(&source.cache_key()) {
                                self.ai.submit(AiJob::Edit { item, image: (*img).clone(), op: AiOp::Custom(msg) });
                            }
                        }
                    }
                }
            }
            Action::StartReplace => {
                if let Some(it) = self.single_selected() {
                    self.replace_target = Some(it.id);
                    self.settings.show_icons = true;
                    if self.icons.mode == IconsMode::Home {
                        self.icons.mode = IconsMode::Search;
                    }
                    self.toast("Pick an icon to replace the selected one");
                }
            }
            Action::Undo => self.undo(),
            Action::Redo => self.redo(),
            Action::NewDoc => self.new_doc(),
            Action::Open => self.open(),
            Action::Save => self.save(false),
            Action::SaveAs => self.save(true),
            Action::GoHome => self.go_home(),
            Action::OpenProject(id) => self.open_project(&id),
            Action::RenameProject(id, name) => self.rename_project(&id, &name),
            Action::CopyProject(id) => self.copy_project(&id),
            Action::ExportProject(id) => self.export_project(&id),
            Action::SaveProjectFile(id) => match self.store.load(&id) {
                Ok(doc) => {
                    if self.save_file(&doc) {
                        self.store.mark_backed_up(&id);
                        self.refresh_projects();
                    }
                }
                Err(e) => self.toast_err(format!("save: {e}")),
            },
            Action::EmailProject(id) => self.email_project(ctx, &id),
            Action::DeleteProject(id) => self.delete_project(&id),
            Action::RestoreVersion(id, t) => self.restore_version(&id, t),
            Action::VersionHistory => {
                if self.save_current(Snapshot::IfChanged, "Edited") {
                    match self.project.clone() {
                        Some(id) => self.open_version_history(&id),
                        None => self.toast("Make a change first: new projects have no history yet"),
                    }
                }
            }
            Action::OpenExport => {
                self.export_doc = None;
                self.show_export = true;
            }
            Action::Export(p) => self.start_export(ctx, p),
            Action::AddSlide => {
                self.commit();
                self.doc.slides.insert(self.cur_slide + 1, Slide::default());
                self.cur_slide += 1;
                self.selection.clear();
            }
            Action::DuplicateSlide(i) => {
                self.commit();
                let mut s = self.doc.slides[i].clone();
                for it in &mut s.items {
                    it.id = self.doc.alloc_id();
                }
                self.doc.slides.insert(i + 1, s);
                self.cur_slide = i + 1;
                self.selection.clear();
            }
            Action::DeleteSlide(i) => {
                if self.doc.slides.len() > 1 {
                    self.commit();
                    self.doc.slides.remove(i);
                    self.cur_slide = self.cur_slide.min(self.doc.slides.len() - 1);
                    self.selection.clear();
                }
            }
            Action::MoveSlide(i, d) => {
                let j = i as isize + d;
                if j >= 0 && (j as usize) < self.doc.slides.len() {
                    self.commit();
                    self.doc.slides.swap(i, j as usize);
                    self.cur_slide = j as usize;
                }
            }
            Action::GoSlide(i) => {
                if i < self.doc.slides.len() {
                    self.cur_slide = i;
                    self.selection.clear();
                    self.close_edit_tab();
                }
            }
            Action::Generate(p) => self.start_generate(p),
            Action::GeneratedToLibrary { path, favorite } => self.generated_to_library(&path, favorite),
            Action::ContributeGenerated(path) => self.contribute_generated(&path),
            Action::OpenUploadDialog(target) => self.open_upload_dialog(target),
            Action::PickLibraryFile => {
                let tx = self.action_tx.clone();
                crate::platform::pick_files("Images", &crate::assets::IMAGE_EXTS, false, move |mut paths| {
                    let _ = tx.send(Action::LibraryFilePicked(paths.remove(0)));
                });
            }
            Action::LibraryFilePicked(f) => {
                let s = f.display().to_string();
                if !self.settings.uploads.contains(&s) {
                    self.settings.uploads.push(s);
                }
                self.open_upload_dialog(UploadTarget::File(f));
            }
            Action::SubmitUpload => self.submit_upload(),
            Action::ContributeSelected => self.contribute_selected(),
            Action::OpenSuggest(title) => {
                self.icons.mode = IconsMode::Suggest;
                if !title.is_empty() {
                    self.suggest.title = title;
                }
                if self.suggest.name.is_empty() {
                    self.suggest.name = self.settings.contributor_name.clone();
                }
            }
            Action::SubmitSuggestion => self.submit_suggestion(),
            Action::PickTemplateFiles => {
                let tx = self.action_tx.clone();
                crate::platform::pick_files("biocartoon", &["biocartoon", "json"], true, move |paths| {
                    let _ = tx.send(Action::AddTemplateFiles(paths));
                });
            }
            Action::AddTemplateFiles(paths) => self.add_template_files(&paths),
            Action::UseSavedTemplate(row) => self.load_saved_template(row),
            Action::RemoveTemplate(id) => {
                if let Err(e) = self.templates.remove_local(&id) {
                    self.toast_err(e);
                }
                self.template_rows = self.templates.list();
            }
            Action::PickUploads => {
                let tx = self.action_tx.clone();
                crate::platform::pick_files("Images", &crate::assets::IMAGE_EXTS, true, move |paths| {
                    let _ = tx.send(Action::AddUploads(paths));
                });
            }
            Action::AddUploads(files) => {
                for f in files {
                    let s = f.display().to_string();
                    if !self.settings.uploads.contains(&s) {
                        self.settings.uploads.push(s);
                    }
                }
            }
            Action::DropImages(files, at) => self.drop_images(&files, at),
            Action::OpenFile(p) => self.open_file(&p),
            Action::ImportFiles(paths) => self.import_files(&paths),
            Action::BackupAll => self.backup_all(),
            Action::RemoveUpload(i) => {
                if i < self.settings.uploads.len() {
                    self.settings.uploads.remove(i);
                }
            }
            Action::ZoomFit => self.view.fit_requested = true,
            Action::ZoomTo(z) => self.view.zoom = z.clamp(0.05, 8.0),
            Action::Toast(t) => self.toast(t),
            Action::SyncCatalog => self.open_catalog(),
            Action::SetRail(Some(RailTab::Text)) => self.apply(Action::ArmText(!self.placing_text), ctx),
            Action::SetRail(r) => {
                if r != Some(RailTab::Lines) {
                    self.drawing = false;
                    self.stroke.clear();
                }
                if self.rail == r {
                    self.rail = None;
                } else {
                    self.rail = r;
                }
                if r == Some(RailTab::Edit) {
                    self.rail_before_edit = None;
                }
            }
            Action::Commit => self.commit(),
        }
    }

    fn keyboard(&mut self, ctx: &egui::Context) {
        // Ignore shortcuts while typing into a text field.
        let typing = ctx.memory(|m| m.focused().is_some());
        if typing {
            return;
        }
        let mut acts = Vec::new();
        ctx.input_mut(|i| {
            use egui::{Key, Modifiers};
            let cmd = Modifiers::COMMAND;
            if i.consume_key(cmd | Modifiers::SHIFT, Key::Z) || i.consume_key(cmd, Key::Y) {
                acts.push(Action::Redo);
            }
            if i.consume_key(cmd, Key::Z) {
                acts.push(Action::Undo);
            }
            if i.consume_key(cmd, Key::S) {
                acts.push(Action::Save);
            }
            if i.consume_key(cmd, Key::O) {
                acts.push(Action::Open);
            }
            if i.consume_key(cmd, Key::D) {
                acts.push(Action::Duplicate);
            }
            if i.consume_key(cmd | Modifiers::SHIFT, Key::G) {
                acts.push(Action::Ungroup);
            }
            if i.consume_key(cmd, Key::G) {
                acts.push(Action::Group);
            }
            if i.consume_key(cmd, Key::A) {
                acts.push(Action::Select(self.slide().items.iter().map(|x| x.id).collect()));
            }
            if i.consume_key(cmd, Key::L) {
                acts.push(Action::ToggleLock);
            }
            if i.consume_key(cmd, Key::Num0) {
                acts.push(Action::ZoomFit);
            }
            if i.consume_key(cmd, Key::Equals) || i.consume_key(cmd, Key::Plus) {
                acts.push(Action::ZoomTo(self.view.zoom * 1.2));
            }
            if i.consume_key(cmd, Key::Minus) {
                acts.push(Action::ZoomTo(self.view.zoom / 1.2));
            }
            if i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace) {
                acts.push(Action::Delete);
            }
            if self.crop_mode && (i.consume_key(Modifiers::NONE, Key::Escape) || i.consume_key(Modifiers::NONE, Key::Enter)) {
                acts.push(Action::SetCrop(false));
            }
            if i.key_pressed(Key::Escape) {
                acts.push(if self.placing_text {
                    Action::ArmText(false)
                } else if self.drawing {
                    Action::ArmDraw(false)
                } else {
                    Action::ClearSelection
                });
            }
            if i.key_pressed(Key::Enter) {
                if let [id] = self.selection[..] {
                    acts.push(Action::EditText(id));
                }
            }
            let nudge = if i.modifiers.shift { 10.0 } else { 1.0 };
            let mut d = Vec2::ZERO;
            if i.key_pressed(Key::ArrowLeft) { d.x -= nudge; }
            if i.key_pressed(Key::ArrowRight) { d.x += nudge; }
            if i.key_pressed(Key::ArrowUp) { d.y -= nudge; }
            if i.key_pressed(Key::ArrowDown) { d.y += nudge; }
            if d != Vec2::ZERO && !self.selection.is_empty() {
                let sel = self.selection.clone();
                self.history.push((self.doc.clone(), self.cur_slide));
                self.dirty = true;
                for it in self.slide_mut().items.iter_mut() {
                    if sel.contains(&it.id) && !it.locked {
                        it.pos[0] += d.x;
                        it.pos[1] += d.y;
                    }
                }
            }
            for e in &i.events {
                match e {
                    egui::Event::Copy => acts.push(Action::Copy),
                    egui::Event::Cut => acts.push(Action::Cut),
                    egui::Event::Paste(_) => {
                        acts.push(Action::Paste);
                        self.pasted = true;
                    }
                    egui::Event::Key { key: Key::V, pressed: false, modifiers, .. } if modifiers.command => {
                        if !std::mem::take(&mut self.pasted) {
                            acts.push(Action::Paste);
                        }
                    }
                    _ => {}
                }
            }
        });
        self.actions.extend(acts);
    }

    // ---------- top-level UI ----------

    /// Applies the theme and text size when they (or, for "Match system", the system's theme) change.
    fn apply_theme(&mut self, ctx: &egui::Context) {
        let want = (self.settings.theme.is_dark(ctx), self.settings.text_size);
        if self.applied_theme != Some(want) {
            theme::apply(ctx, want.0, want.1.scale());
            self.applied_theme = Some(want);
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.apply_theme(&ctx);
        self.debug_hook(&ctx);
        #[cfg(not(target_arch = "wasm32"))]
        self.poll_control(&ctx);
        self.images.poll(&ctx);
        self.fit_images();
        self.handle_catalog();
        self.lookup_credits();
        self.poll_stars();
        self.poll_account();
        self.poll_catalog_sync(&ctx);
        self.handle_ai_results(&ctx);
        self.poll_export(&ctx);
        self.poll_offline(&ctx);
        self.poll_thumbs(&ctx);
        self.poll_library(&ctx);
        if self.screen == Screen::Home {
            self.home_ui(ui);
            if self.show_export {
                self.export_window(&ctx);
            }
            self.settings_windows(&ctx);
            self.toasts_ui(&ctx);
            self.run_actions(&ctx);
            return;
        }
        self.autosave(ctx.input(|i| i.time));
        if self.images.loads_in_flight > 0 || self.ai.busy > 0 || self.icons.loading {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        // Debounced search.
        if self.settings.show_icons
            && self.icons.mode == IconsMode::Search
            && (self.icons.query != self.icons.last_sent_query || self.icons.filter != self.icons.last_sent_filter)
        {
            self.request_search();
        }
        self.keyboard(&ctx);

        self.refresh_spelling(&ctx);
        self.top_bar(ui);
        self.bottom_bar(ui);
        if self.settings.show_icons {
            self.left_panel(ui);
        }
        self.rail_ui(ui);
        if self.rail.is_some() {
            self.right_panel(ui);
        }
        if let Some(r) = self.rail.filter(|&r| r != RailTab::Text) {
            self.last_rail = Some(r);
        }
        self.edge_swipes(ui);
        self.canvas_ui(ui);
        self.windows(&ctx);
        self.home_windows_in_editor(&ctx);
        self.toasts_ui(&ctx);
        self.run_actions(&ctx);
    }

    pub(crate) fn run_actions(&mut self, ctx: &egui::Context) {
        let ctx = ctx.clone();
        self.actions.extend(self.action_rx.try_iter());
        let acts = std::mem::take(&mut self.actions);
        for a in acts {
            if matches!(a, Action::Ai(_) | Action::OpenAiEdit | Action::Generate(_)) && !self.ai_ready() {
                self.open_ai_setup(Some(a));
                continue;
            }
            self.apply(a, &ctx);
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let frame = egui::Frame::new().fill(theme::bg()).inner_margin(egui::Margin::symmetric(10, 6));
        egui::Panel::top("topbar").exact_size(46.0).frame(frame).show_separator_line(false).show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                egui::Frame::new().fill(theme::panel()).corner_radius(8).inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
                    ui.menu_button(egui::RichText::new(ph::LIST).size(18.0), |ui| {
                        if ui.button("All projects").clicked() { self.actions.push(Action::GoHome); ui.close(); }
                        if ui.add_enabled(!self.settings.show_icons, egui::Button::new("Show icons menu"))
                            .on_hover_text("Open the Icons panel on the left")
                            .on_disabled_hover_text("Icon menu is already up")
                            .clicked()
                        {
                            self.settings.show_icons = true;
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("New").clicked() { self.actions.push(Action::NewDoc); ui.close(); }
                        if ui.button("Open file...").clicked() { self.actions.push(Action::Open); ui.close(); }
                        if ui.button("Save").clicked() { self.actions.push(Action::Save); ui.close(); }
                        if ui.button("Save as .biocartoon file...").on_hover_text("A copy of the whole project, still editable: anyone can open it with Open file").clicked() { self.actions.push(Action::SaveAs); ui.close(); }
                        if ui.button("Version history...").clicked() { self.actions.push(Action::VersionHistory); ui.close(); }
                        ui.separator();
                        // A button rather than ui.checkbox so the box sits on the right.
                        let view = self.settings.show_spelling;
                        let check = if view { ph::CHECK_SQUARE } else { ph::SQUARE };
                        if ui.add(egui::Button::new("View spelling and grammar").right_text(check)).on_hover_text("Underline mistakes in red (spelling) and blue (grammar) while you edit").clicked() {
                            self.settings.show_spelling = !view;
                        }
                        if ui.button("Check spelling and grammar...").clicked() { self.show_spell_window = true; ui.close(); }
                        ui.separator();
                        if ui.button("Export...").clicked() { self.actions.push(Action::OpenExport); ui.close(); }
                        if ui.button("Show credits...").on_hover_text("Credits for the library artwork in this document, ready for a figure legend").clicked() { self.show_attributions = true; self.attributions_slide = None; self.attributions_export = None; ui.close(); }
                        ui.separator();
                        if ui.button("Settings...").clicked() { self.open_settings(SettingsTab::General); ui.close(); }
                        if ui.button(if cfg!(target_arch = "wasm32") { "Bulk download..." } else { "Download..." }).clicked() { self.open_settings(SettingsTab::Offline); ui.close(); }
                        if ui.button("I'm feeling lucky").on_hover_text("Insert a random icon from the library").clicked() { self.actions.push(Action::InsertRandom); ui.close(); }
                        if ui.button("About BioCartoon").clicked() { self.show_about = true; ui.close(); }
                        // A browser tab is closed by the browser.
                        if !cfg!(target_arch = "wasm32") && ui.button("Quit").clicked() { ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close); }
                    });
                    if ui.add(crate::logo(ui.ctx(), 22.0).sense(egui::Sense::click()))
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text("All projects")
                        .clicked()
                    {
                        self.actions.push(Action::GoHome);
                    }
                    ui.separator();
                    let title = ui.add(egui::TextEdit::singleline(&mut self.doc.title).desired_width(140.0).frame(egui::Frame::NONE));
                    if title.changed() { self.dirty = true; }
                    ui.label(egui::RichText::new(if self.dirty { ph::CLOUD_ARROW_UP } else { ph::CHECK }).color(theme::text_dim()))
                        .on_hover_text(if self.dirty { "Saving..." } else { "All changes saved to this computer" });
                    ui.separator();
                    if ui.add_enabled(!self.history.is_empty(), egui::Button::new(egui::RichText::new(ph::ARROW_U_UP_LEFT).size(16.0)).frame(false)).on_hover_text("Undo (Cmd+Z)").clicked() {
                        self.actions.push(Action::Undo);
                    }
                    if ui.add_enabled(!self.future.is_empty(), egui::Button::new(egui::RichText::new(ph::ARROW_U_UP_RIGHT).size(16.0)).frame(false)).on_hover_text("Redo (Cmd+Shift+Z)").clicked() {
                        self.actions.push(Action::Redo);
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(egui::Button::new(egui::RichText::new("Export").color(Color32::WHITE)).fill(theme::accent())).clicked() {
                        self.actions.push(Action::OpenExport);
                    }
                });
            });
        });
    }

    fn bottom_bar(&mut self, ui: &mut egui::Ui) {
        let frame = egui::Frame::new().fill(theme::bg()).inner_margin(egui::Margin::symmetric(10, 4));
        let n = self.doc.slides.len();
        egui::Panel::bottom("bottombar").exact_size(36.0).frame(frame).show_separator_line(false).show(ui, |ui| {
            let full = ui.max_rect();
            let row = |layout: egui::Layout, rect: Rect| egui::UiBuilder::new().max_rect(rect).layout(layout);
            ui.scope_builder(row(egui::Layout::left_to_right(egui::Align::Center), full), |ui| {
                if ui.add(egui::Button::new(format!("{}  Icons", ph::DNA)).selected(self.settings.show_icons)).on_hover_text("Show or hide the Icons panel").clicked() {
                    self.settings.show_icons = !self.settings.show_icons;
                }
            });
            ui.scope_builder(row(egui::Layout::right_to_left(egui::Align::Center), full), |ui| {
                if ui.add(egui::Button::new(ph::CARET_RIGHT).frame(false)).clicked() && self.cur_slide + 1 < n { self.actions.push(Action::GoSlide(self.cur_slide + 1)); }
                ui.label(format!("{}/{}", self.cur_slide + 1, n));
                if ui.add(egui::Button::new(ph::CARET_LEFT).frame(false)).clicked() && self.cur_slide > 0 { self.actions.push(Action::GoSlide(self.cur_slide - 1)); }
                if ui.add(egui::Button::new(format!("{}  Slides", ph::PRESENTATION)).selected(self.rail == Some(RailTab::Slides))).clicked() { self.actions.push(Action::SetRail(Some(RailTab::Slides))); }
            });
            // Zoom controls, centred in the window using their width from the previous frame.
            let width_id = egui::Id::new("zoom_controls_width");
            let w = ui.ctx().data(|d| d.get_temp::<f32>(width_id)).unwrap_or(260.0);
            let center = Pos2::new(ui.ctx().content_rect().center().x, full.center().y);
            let zoom = ui.scope_builder(row(egui::Layout::left_to_right(egui::Align::Center), Rect::from_center_size(center, Vec2::new(w, full.height()))), |ui| {
                if ui.add(egui::Button::new(ph::MINUS).frame(false)).on_hover_text("Zoom out (Cmd+-)").clicked() { self.actions.push(Action::ZoomTo(self.view.zoom / 1.2)); }
                let mut z = self.view.zoom * 100.0;
                if ui.add(egui::Slider::new(&mut z, 10.0..=400.0).show_value(false).logarithmic(true)).changed() { self.view.zoom = z / 100.0; }
                if ui.add(egui::Button::new(ph::PLUS).frame(false)).on_hover_text("Zoom in (Cmd+=)").clicked() { self.actions.push(Action::ZoomTo(self.view.zoom * 1.2)); }
                ui.label(format!("{:.0}%", self.view.zoom * 100.0));
                if ui.add(egui::Button::new(ph::CORNERS_OUT).frame(false)).on_hover_text("Fit (Cmd+0)").clicked() { self.actions.push(Action::ZoomFit); }
            });
            let used = zoom.response.rect.width();
            if (used - w).abs() > 0.5 {
                ui.ctx().data_mut(|d| d.insert_temp(width_id, used));
                ui.ctx().request_repaint();
            }
        });
    }

    fn toasts_ui(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        for t in &mut self.toasts {
            if t.until == 0.0 {
                t.until = now + if t.error { 6.0 } else { 3.0 };
            }
        }
        self.toasts.retain(|t| t.until > now);
        if self.toasts.is_empty() {
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
        egui::Area::new(egui::Id::new("toasts")).anchor(egui::Align2::CENTER_BOTTOM, Vec2::new(0.0, -48.0)).order(egui::Order::Tooltip).interactable(false).show(ctx, |ui| {
            for t in &self.toasts {
                let fill = if t.error { Color32::from_rgb(0x7a, 0x2a, 0x2a) } else { theme::toast() };
                egui::Frame::new().fill(fill).corner_radius(8).inner_margin(egui::Margin::symmetric(12, 8)).stroke(egui::Stroke::new(1.0, theme::border())).show(ui, |ui| {
                    ui.label(&t.text);
                });
            }
        });
    }

    /// Settings and AI-setup dialogs (shown on both the home screen and in the editor).
    fn settings_windows(&mut self, ctx: &egui::Context) {
        // Settings
        if self.show_settings {
            let mut open = true;
            egui::Window::new("Settings").open(&mut open).collapsible(false).resizable(false).fixed_size([620.0, 0.0]).anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.settings_tab, SettingsTab::General, "General");
                    ui.selectable_value(&mut self.settings_tab, SettingsTab::Ai, "AI");
                    ui.selectable_value(&mut self.settings_tab, SettingsTab::Offline, crate::offline::TAB_LABEL);
                });
                ui.separator();
                // Edits apply (and are saved) right away, so there's no Save button.
                let d = &mut self.settings;
                let ai_before = (d.ai_provider, d.ai_model.clone(), d.api_key.clone(), d.ai_base_url.clone(), d.ai_prompts.clone());
                match self.settings_tab {
                    SettingsTab::General => {
                        egui::Grid::new("general_grid").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
                            ui.label("Appearance");
                            ui.horizontal(|ui| {
                                for m in theme::ThemeMode::ALL {
                                    ui.selectable_value(&mut d.theme, m, m.label());
                                }
                            });
                            ui.end_row();
                            ui.label("Text size");
                            egui::ComboBox::from_id_salt("text_size").selected_text(d.text_size.label()).width(160.0).show_ui(ui, |ui| {
                                for t in theme::TextSize::ALL {
                                    ui.selectable_value(&mut d.text_size, t, t.label());
                                }
                            });
                            ui.end_row();
                            ui.label("Icon columns");
                            ui.horizontal(|ui| {
                                for n in 1..=5 {
                                    ui.selectable_value(&mut d.icon_columns, n, n.to_string());
                                }
                            });
                            ui.end_row();
                            ui.label("On icons");
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut d.tile_database, "Database").on_hover_text("The database's badge, lower left");
                                ui.checkbox(&mut d.tile_colors, "Multicolor").on_hover_text("A color wheel on icons that come in several colors");
                                ui.checkbox(&mut d.tile_stars, "Stars").on_hover_text("The star and how many people starred the icon, upper right. Without it, your favorites still show a gold star.");
                            });
                            ui.end_row();
                        });
                    }
                    SettingsTab::Ai => {
                        ai_fields(ui, &mut d.ai_provider, &mut d.ai_model, &mut d.api_key, &mut d.ai_base_url);
                        ui.add_space(10.0);
                        ui.checkbox(&mut d.auto_library_generated, "Automatically add images to your library").on_hover_text("Every image generated with AI goes into Your library as soon as it arrives");
                        ui.add_space(10.0);
                        ui.separator();
                        let svg = d.ai_provider.draws_svg(&d.ai_model);
                        ai_prompt_fields(ui, &mut d.ai_prompts, svg);
                    }
                    SettingsTab::Offline => self.offline_tab(ui),
                }
                let d = &self.settings;
                if (d.ai_provider, &d.ai_model, &d.api_key, &d.ai_base_url, &d.ai_prompts) != (ai_before.0, &ai_before.1, &ai_before.2, &ai_before.3, &ai_before.4) {
                    self.ai.set_backend(d.ai_provider, &d.ai_model, &d.api_key, &d.ai_base_url, &d.ai_prompts);
                }
            });
            if !open { self.show_settings = false; }
        }

        // About
        if self.show_about {
            let mut open = true;
            egui::Window::new("About").open(&mut open).collapsible(false).resizable(false).fixed_size([500.0, 0.0]).anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add(crate::logo(ui.ctx(), 64.0));
                    ui.label(egui::RichText::new("BioCartoon").size(20.0).strong());
                    ui.label(egui::RichText::new(concat!("v", env!("CARGO_PKG_VERSION"))).color(theme::text_dim()));
                    ui.add_space(10.0);
                    linked_line(ui, "Developed by ", "Joseph Rich", "https://joseph-rich.com");
                    linked_line(ui, "Part of the ", "Pachter Lab", "https://pachterlab.github.io");
                    ui.add_space(10.0);
                    linked_line(ui, "Learn more at ", "www.biocartoon.com", "https://www.biocartoon.com");
                    linked_line(ui, "Web app: ", "app.biocartoon.com", "https://app.biocartoon.com");
                    ui.add_space(10.0);
                    linked_line(ui, "Free and open source under the ", "GNU AGPL v3.0", "https://www.gnu.org/licenses/agpl-3.0.html");
                    linked_line(ui, "Database and CLI Source Code: ", "github.com/josephrich98/biocartoon", "https://github.com/josephrich98/biocartoon");
                    linked_line(ui, "App source code: ", "github.com/josephrich98/biocartoon-app", crate::GITHUB_URL);
                });
            });
            if !open { self.show_about = false; }
        }

        // AI setup, shown the first time an AI tool is used
        if let Some(setup) = &mut self.ai_setup {
            let mut open = true;
            let mut connect = false;
            let mut cancel = false;
            egui::Window::new("Connect an AI model").open(&mut open).collapsible(false).resizable(false).fixed_size([460.0, 0.0]).anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
                ui.label(egui::RichText::new("AI tools run on a hosted model with your own API key, or on your own vLLM server. Keys are stored only on this computer.").color(theme::text_dim()));
                ui.add_space(8.0);
                ai_fields(ui, &mut setup.provider, &mut setup.model, &mut setup.key, &mut setup.base_url);
                ui.add_space(10.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can = if setup.provider.self_hosted() {
                        !setup.base_url.trim().is_empty()
                    } else {
                        !setup.key.trim().is_empty() && !setup.model.trim().is_empty()
                    };
                    if ui.add_enabled(can, egui::Button::new(egui::RichText::new("Connect").color(Color32::WHITE)).fill(theme::accent())).clicked() { connect = true; }
                    if ui.button("Cancel").clicked() { cancel = true; }
                });
            });
            if connect {
                let setup = self.ai_setup.take().unwrap();
                self.settings.ai_provider = setup.provider;
                self.settings.ai_model = setup.model.trim().to_string();
                self.settings.api_key = setup.key.trim().to_string();
                self.settings.ai_base_url = setup.base_url.trim().to_string();
                let s = &self.settings;
                self.ai.set_backend(s.ai_provider, &s.ai_model, &s.api_key, &s.ai_base_url, &s.ai_prompts);
                if let Some(a) = setup.pending {
                    self.actions.push(a);
                }
            } else if cancel || !open {
                self.ai_setup = None;
            }
        }

    }

    fn windows(&mut self, ctx: &egui::Context) {
        self.spell_window(ctx);
        self.attributions_window(ctx);
        self.credit_window(ctx);
        if self.show_export {
            self.export_window(ctx);
        }
        self.settings_windows(ctx);
        // Edit with AI: the image, then a chat that offers the quick edits as clickable prompts.
        if self.ai_edit.is_some() {
            let mut open = true;
            let mut send: Option<String> = None;
            let mut run: Option<AiOp> = None;
            let ce_item = self.ai_edit.as_ref().unwrap().item;
            let item = self.slide().items.iter().find(|i| i.id == ce_item).cloned();
            let ce = self.ai_edit.as_mut().unwrap();
            let center = ctx.content_rect().center();
            egui::Window::new(format!("{}  Edit with AI", ph::SPARKLE)).open(&mut open).collapsible(false).default_size([460.0, 600.0]).default_pos(center - Vec2::new(230.0, 300.0)).show(ctx, |ui| {
                if let Some(it) = &item {
                    if let ItemKind::Image { source, .. } = &it.kind {
                        let key = source.cache_key();
                        let (w, h) = (ui.available_width().min(420.0), 240.0);
                        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), egui::Sense::hover());
                        ui.painter().rect_filled(rect, 6, Color32::WHITE);
                        if let Some(ImgState::Ready { tex, size }) = self.images.get(&key) {
                            let s = (w / size[0] as f32).min(h / size[1] as f32) * 0.94;
                            let r = Rect::from_center_size(rect.center(), Vec2::new(size[0] as f32 * s, size[1] as f32 * s));
                            ui.painter().image(tex.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                        }
                        ui.label(egui::RichText::new(&it.name).small().color(theme::text_dim()));
                    }
                }
                ui.separator();
                egui::ScrollArea::vertical().max_height(220.0).stick_to_bottom(true).show(ui, |ui| {
                    for (from_user, text) in &ce.messages {
                        let fill = if *from_user { theme::accent_dim() } else { theme::panel() };
                        ui.with_layout(if *from_user { egui::Layout::right_to_left(egui::Align::Min) } else { egui::Layout::left_to_right(egui::Align::Min) }, |ui| {
                            egui::Frame::new().fill(fill).corner_radius(8).inner_margin(8).show(ui, |ui| { ui.set_max_width(360.0); ui.label(text); });
                        });
                    }
                    if ce.waiting {
                        ui.label(egui::RichText::new("Working...").italics().color(theme::text_dim()));
                    } else {
                        // Clickable prompts for the current step of the conversation.
                        let chip = |ui: &mut egui::Ui, icon: &str, label: &str| ui.add(egui::Button::new(format!("{icon}  {label}")).stroke(egui::Stroke::new(1.0, theme::accent())).corner_radius(12)).clicked();
                        ui.horizontal_wrapped(|ui| match ce.stage {
                            AiChatStage::Start => {
                                if chip(ui, ph::MAGIC_WAND, "Restyle icon") {
                                    ce.messages.push((true, "Restyle icon".into()));
                                    ce.messages.push((false, "In what style?".into()));
                                    ce.stage = AiChatStage::PickStyle;
                                }
                                if chip(ui, ph::ERASER, "Remove background") {
                                    ce.messages.push((true, "Remove background".into()));
                                    ce.messages.push((false, "Removing the background...".into()));
                                    run = Some(AiOp::RemoveBackground);
                                }
                                if chip(ui, ph::TEXT_T_SLASH, "Remove text") {
                                    ce.messages.push((true, "Remove text".into()));
                                    ce.messages.push((false, "Removing the text...".into()));
                                    run = Some(AiOp::RemoveText);
                                }
                            }
                            AiChatStage::PickStyle => {
                                for st in crate::ai::RestyleStyle::ALL {
                                    if chip(ui, ph::PAINT_BRUSH, st.label()) {
                                        ce.messages.push((true, st.label().into()));
                                        ce.messages.push((false, format!("Restyling as {}...", st.label().to_lowercase())));
                                        ce.stage = AiChatStage::Start;
                                        run = Some(AiOp::Restyle(st));
                                    }
                                }
                            }
                        });
                    }
                });
                ui.horizontal(|ui| {
                    let r = ui.add(egui::TextEdit::singleline(&mut ce.input).hint_text("Or describe the edit you want...").desired_width(ui.available_width() - 48.0));
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if (ui.add(egui::Button::new(egui::RichText::new(ph::PAPER_PLANE_RIGHT).color(Color32::WHITE)).fill(theme::accent())).clicked() || enter) && !ce.input.trim().is_empty() && !ce.waiting {
                        send = Some(ce.input.trim().to_string());
                        ce.input.clear();
                    }
                });
            });
            if let Some(op) = run {
                self.actions.push(Action::Select(vec![ce_item]));
                self.actions.push(Action::Ai(op));
            }
            if let Some(m) = send { self.actions.push(Action::SendChat(m)); }
            if !open { self.ai_edit = None; }
        }

        // "Add to library" / "Save as template" dialog: a large preview, and the details beside it.
        let can_submit = self.submit_url().is_some();
        let Some(mut d) = self.upload_dialog.take() else { return };
        let mut open = true;
        let mut submit = false;
        let mut cancel = false;
        let template = matches!(d.target, UploadTarget::Template(_));
        let mut title = if template { "Save slide as template" } else { "Add to icon library" }.to_string();
        if d.step.1 > 1 {
            title = format!("{title} ({} of {})", d.step.0, d.step.1);
        }
        let categories = self.upload_categories(template);
        let screen = ctx.content_rect();
        let size = Vec2::new((screen.width() * 0.92).min(1600.0), (screen.height() * 0.88).min(1100.0));
        // Side by side when there is room, otherwise the details go under the preview.
        let wide = size.x >= 760.0;
        const FORM_W: f32 = 320.0;
        egui::Window::new(title).id(egui::Id::new("upload_dialog")).open(&mut open).order(egui::Order::Foreground).collapsible(false).resizable(false).fixed_size(size).anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
            let form = |ui: &mut egui::Ui, d: &mut UploadDialog, submit: &mut bool, cancel: &mut bool| {
                let w = ui.available_width();
                let field = |ui: &mut egui::Ui, label: &str| {
                    ui.add_space(4.0);
                    ui.label(label);
                };
                field(ui, "Name");
                ui.add(egui::TextEdit::singleline(&mut d.name).char_limit(80).desired_width(w));
                field(ui, "Category");
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.add(egui::TextEdit::singleline(&mut d.category).char_limit(60).hint_text("pick one or type your own").desired_width(w - 32.0));
                    let pick = ui.button(ph::CARET_DOWN).on_hover_text("Current categories");
                    egui::Popup::menu(&pick).show(|ui| {
                        ui.set_min_width(w - 20.0);
                        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                            for (c, sub) in &categories {
                                let text = if *sub { format!("    {c}") } else { c.clone() };
                                if ui.selectable_label(d.category.eq_ignore_ascii_case(c), text).clicked() {
                                    d.category = c.clone();
                                    ui.close();
                                }
                            }
                        });
                    });
                });
                field(ui, "Description");
                ui.add(egui::TextEdit::multiline(&mut d.description).char_limit(500).hint_text("optional").desired_rows(3).desired_width(w));
                field(ui, "License");
                egui::ComboBox::from_id_salt("upload_license").width(w).selected_text(UPLOAD_LICENSES.iter().find(|l| l.0 == d.license).map(|l| l.1).unwrap_or(d.license)).show_ui(ui, |ui| {
                    for (id, label) in UPLOAD_LICENSES {
                        ui.selectable_value(&mut d.license, id, label);
                    }
                });
                field(ui, "Credit as");
                ui.add(egui::TextEdit::singleline(&mut d.credit).char_limit(60).hint_text("your name").desired_width(w));
                ui.add_space(10.0);
                let dim = |t: &str| egui::RichText::new(t).small().color(theme::text_dim());
                if can_submit {
                    ui.checkbox(&mut d.share, "Submit to the shared library");
                    if d.share {
                        ui.label(dim("It is reviewed before anyone else sees it. Once approved, the name, credit and details above are public."));
                        ui.label(dim("You keep the copyright. BioCartoon and its users only get the license you chose."));
                        ui.label(dim("To submit anonymously, choose the CC0 license and leave \"Credit as\" blank."));
                    } else {
                        ui.label(dim("Saved on this computer only."));
                    }
                } else {
                    ui.label(dim("Saved on this computer only (no submission server is set up)."));
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let sharing = can_submit && d.share;
                    let credited = d.license != "CC-BY-4.0" || !d.credit.trim().is_empty();
                    let can = !d.name.trim().is_empty() && (!sharing || credited);
                    let label = if sharing { "Submit" } else { "Save" };
                    let b = ui.add_enabled(can, egui::Button::new(egui::RichText::new(label).color(Color32::WHITE)).fill(theme::accent()));
                    let why = if d.name.trim().is_empty() { "Give it a name first" } else { "CC BY needs a name to credit, or choose CC0" };
                    if b.on_disabled_hover_text(why).clicked() { *submit = true; }
                    if ui.button(if d.queue.is_empty() { "Cancel" } else { "Cancel all" }).clicked() { *cancel = true; }
                });
            };
            if wide {
                let (w, h) = (ui.available_width(), size.y - 48.0);
                ui.horizontal_top(|ui| {
                    self.upload_preview(ui, &mut d, Vec2::new(w - FORM_W - 16.0, h));
                    ui.add_space(12.0);
                    ui.allocate_ui_with_layout(Vec2::new(FORM_W, h), egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.set_width(FORM_W);
                        egui::ScrollArea::vertical().id_salt("upload_form").show(ui, |ui| form(ui, &mut d, &mut submit, &mut cancel));
                    });
                });
            } else {
                egui::ScrollArea::vertical().id_salt("upload_form").show(ui, |ui| {
                    let w = ui.available_width();
                    self.upload_preview(ui, &mut d, Vec2::new(w, (size.y * 0.5).max(200.0)));
                    ui.add_space(6.0);
                    form(ui, &mut d, &mut submit, &mut cancel);
                });
            }
        });
        if open && !cancel {
            self.upload_dialog = Some(d);
        }
        if submit { self.actions.push(Action::SubmitUpload); }
    }

    /// The library dialog's view of what is being uploaded: drag to move, scroll or pinch to
    /// zoom, double-click to fit again.
    fn upload_preview(&mut self, ui: &mut egui::Ui, d: &mut UploadDialog, size: Vec2) {
        let (rect, r) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        // A checkerboard, so transparent parts show as such.
        painter.rect_filled(rect, 6.0, Color32::from_gray(250));
        let cell = 14.0;
        for (y, row) in (0..(rect.height() / cell).ceil() as usize).enumerate() {
            for x in (row % 2..(rect.width() / cell).ceil() as usize).step_by(2) {
                let c = Rect::from_min_size(rect.min + Vec2::new(x as f32 * cell, y as f32 * cell), Vec2::splat(cell));
                painter.rect_filled(c.intersect(rect), 0.0, Color32::from_gray(232));
            }
        }
        let content: Option<Vec2> = match &d.target {
            UploadTarget::File(_) | UploadTarget::Library(_) => match self.images.file(&d.target.image_path().unwrap_or_default()) {
                ImgState::Ready { size, .. } => Some(Vec2::new(size[0] as f32, size[1] as f32)),
                _ => None,
            },
            UploadTarget::Template(t) => Some(Vec2::new(t.width, t.height)),
        };
        if let Some(c) = content.filter(|c| c.x > 0.0 && c.y > 0.0) {
            // Zoom around the pointer, so what is under it stays put.
            if r.hovered() {
                let (pinch, scroll) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta.y));
                let factor = pinch * (scroll / 300.0).exp();
                if (factor - 1.0).abs() > 1e-4 {
                    let z = (d.view_zoom * factor).clamp(0.25, 32.0);
                    let f = z / d.view_zoom;
                    let p = r.hover_pos().unwrap_or(rect.center()) - rect.center();
                    d.view_pan = p - (p - d.view_pan) * f;
                    d.view_zoom = z;
                }
            }
            if r.dragged() {
                d.view_pan += r.drag_delta();
            }
            if r.double_clicked() {
                d.view_zoom = 1.0;
                d.view_pan = Vec2::ZERO;
            }
            let fit = (rect.width() / c.x).min(rect.height() / c.y) * 0.92;
            let scale = fit * d.view_zoom;
            let shown = Rect::from_center_size(rect.center() + d.view_pan, c * scale);
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            match &d.target {
                UploadTarget::File(_) | UploadTarget::Library(_) => {
                    if let ImgState::Ready { tex, .. } = self.images.file(&d.target.image_path().unwrap_or_default()) {
                        painter.image(tex.id(), shown, uv, Color32::WHITE);
                    }
                }
                UploadTarget::Template(t) => {
                    let xf = crate::canvas::Xform { origin: shown.min, zoom: scale };
                    crate::canvas::draw_slide(&painter, &xf, &t.slide, t.page_rect(), crate::doc::c32(t.background), &mut self.images);
                    painter.rect_stroke(shown, 0.0, egui::Stroke::new(1.0, Color32::from_gray(190)), egui::StrokeKind::Outside);
                }
            }
            if r.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            } else if r.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            }
        } else {
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, "Loading preview...", egui::FontId::proportional(14.0), Color32::from_gray(120));
        }
        painter.rect_stroke(rect, 6.0, egui::Stroke::new(1.0, theme::border()), egui::StrokeKind::Inside);
        // Controls float over the preview's bottom edge, so they never change the dialog's layout.
        let bar = Rect::from_min_max(Pos2::new(rect.min.x + 8.0, rect.max.y - 34.0), Pos2::new(rect.max.x - 8.0, rect.max.y - 8.0));
        let hint = painter.layout_no_wrap("Drag to move, scroll or pinch to zoom, double-click to fit".into(), egui::FontId::proportional(11.0), Color32::WHITE);
        let hint_rect = Rect::from_min_size(Pos2::new(bar.min.x, bar.center().y - hint.size().y / 2.0 - 4.0), hint.size() + Vec2::new(12.0, 8.0));
        if hint_rect.max.x < bar.max.x - 190.0 {
            painter.rect_filled(hint_rect, 4.0, Color32::from_black_alpha(150));
            painter.galley(hint_rect.min + Vec2::new(6.0, 4.0), hint, Color32::WHITE);
        }
        let mut x = bar.max.x;
        // Painted buttons: real widgets would move the layout cursor.
        let mut place = |ui: &mut egui::Ui, w: f32, label: &str| {
            x -= w;
            let b = Rect::from_min_size(Pos2::new(x, bar.min.y), Vec2::new(w, bar.height()));
            x -= 4.0;
            let r = ui.interact(b, ui.id().with(("preview_btn", label)), egui::Sense::click());
            painter.rect_filled(b, 4.0, Color32::from_black_alpha(if r.hovered() { 210 } else { 150 }));
            painter.text(b.center(), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(13.0), Color32::WHITE);
            r
        };
        if place(ui, 40.0, "Fit").clicked() {
            d.view_zoom = 1.0;
            d.view_pan = Vec2::ZERO;
        }
        for (icon, f) in [(ph::MAGNIFYING_GLASS_PLUS, 1.25), (ph::MAGNIFYING_GLASS_MINUS, 0.8)] {
            if place(ui, 30.0, icon).clicked() {
                let z = (d.view_zoom * f).clamp(0.25, 32.0);
                d.view_pan *= z / d.view_zoom;
                d.view_zoom = z;
            }
        }
        let pct = Rect::from_min_size(Pos2::new(x - 52.0, bar.min.y), Vec2::new(52.0, bar.height()));
        painter.rect_filled(pct, 4.0, Color32::from_black_alpha(150));
        painter.text(pct.center(), egui::Align2::CENTER_CENTER, format!("{:.0}%", d.view_zoom * 100.0), egui::FontId::proportional(11.0), Color32::WHITE);
    }
}

impl BioCartoon {
    /// Headless-ish smoke test: `BIOCARTOON_SCREENSHOT=out.png` captures the window after a
    /// couple of seconds and quits. `BIOCARTOON_SCRIPT` can seed the document first
    /// (comma-separated: demo, svgzoom, alllines, attributions, search=<q>, stars, select, aiedit, generate, page,
    /// settings, about, templates, savetemplate, export=<path>, light, dark, grid, snap, spell, viewspell, textedit, tableedit, placetext,
    /// fav=<id>, open=<project id>, lucky, rail=<text|lines|shapes|table|edit|slides|page|none|icons|noicons|uploads|favorites|library>).
    fn debug_hook(&mut self, ctx: &egui::Context) {
        let Ok(path) = std::env::var("BIOCARTOON_SCREENSHOT") else { return };
        let frame = ctx.cumulative_frame_nr();
        if frame == 2 {
            let script = std::env::var("BIOCARTOON_SCRIPT").unwrap_or_default();
            // Scripts drive the editor unless they ask for the home screen.
            if !script.split(',').any(|s| s.trim() == "home") {
                self.load_into_editor(Document::default(), None);
            }
            for step in script.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                match step {
                    "home" => self.screen = Screen::Home,
                    "lucky" => self.actions.push(Action::InsertRandom),
                    // A few projects in the (temporary) smoke-test store, two versions each.
                    "seedprojects" => {
                        for (i, name) in ["Medical Imaging Modalities", "Untitled", "T cell activation pathway with a long name that elides", "tmp"].iter().enumerate() {
                            let mut d = Document::default();
                            d.title = name.to_string();
                            if i % 2 == 0 {
                                d.slides.push(Slide::default());
                            }
                            let id = d.alloc_id();
                            let kind = ItemKind::Shape { shape: ShapeKind::ALL[i], fill: [120, 170, 230, 255], stroke: [40, 70, 120, 255], stroke_width: 2.0 };
                            d.slides[0].items.push(Item::new(id, "Shape", kind, Pos2::new(200.0 + 80.0 * i as f32, 150.0), Vec2::splat(300.0)));
                            if let Ok(m) = self.store.create(&d) {
                                d.slides[0].items[0].pos[0] += 200.0;
                                let _ = self.store.save(&m.id, &d, Snapshot::IfChanged, "Saved");
                                self.request_thumb(&m.id, &d);
                            }
                        }
                        self.refresh_projects();
                    }
                    s if s.starts_with("view=") => {
                        self.settings.home_view = match &s["view=".len()..] { "list" => HomeView::List, "small" => HomeView::SmallTiles, _ => HomeView::LargeTiles };
                    }
                    "history" => {
                        if let Some(p) = self.home.projects.first().cloned() {
                            self.open_version_history(&p.id);
                        }
                    }
                    // A project from the store (`BIOCARTOON_PROJECTS_DIR`), as it looks when opened.
                    s if s.starts_with("open=") => self.open_project(&s["open=".len()..]),
                    "demo" => {
                        self.show_settings = false;
                        self.actions.push(Action::UseTemplate(3));
                        self.actions.push(Action::InsertShape(ShapeKind::Ellipse));
                        self.actions.push(Action::InsertText { text: "Hello BioCartoon".into(), size: 28.0, bold: true });
                        let icon = self.app_dir.join("generated").join("demo-icon.png");
                        if image::load_from_memory(crate::ICON_PNG).ok().and_then(|i| i.save(&icon).ok()).is_some() {
                            self.actions.push(Action::InsertFile(icon));
                        }
                    }
                    // The logo SVG placed as an upload and zoomed far in: checks that SVGs draw sharp.
                    "svgzoom" => {
                        self.show_settings = false;
                        let svg = self.app_dir.join("generated").join("demo-logo.svg");
                        if crate::storage::write(&svg, include_bytes!("../assets/logo.svg")).is_ok() {
                            self.actions.push(Action::InsertFile(svg));
                        }
                        self.view.fit_requested = false;
                        self.actions.push(Action::ZoomTo(5.0));
                    }
                    // Text with spelling and grammar mistakes, and the Spelling and grammar window.
                    "spell" => {
                        self.show_settings = false;
                        let text = "The the intron is spliced out by the spliceosme. a mRNA is then exported , and transalted by ribosomes.";
                        self.actions.push(Action::InsertText { text: text.into(), size: 28.0, bold: false });
                        self.show_spell_window = true;
                    }
                    // The same text with "View spelling and grammar" on instead of the window.
                    "viewspell" => {
                        self.show_settings = false;
                        let text = "The the intron is spliced out by the spliceosme. a mRNA is then exported , and transalted by ribosomes.";
                        self.actions.push(Action::InsertText { text: text.into(), size: 28.0, bold: false });
                        self.settings.show_spelling = true;
                    }
                    // A styled list being typed into on the canvas, with its Text settings open.
                    "textedit" => {
                        self.show_settings = false;
                        let id = self.doc.alloc_id();
                        let mut kind = ItemKind::text("Signalling steps\n• Ligand binds\n• Receptor dimerizes\n    • Kinase cascade", 24.0, [30, 30, 30, 255], TextAlign::Left, false);
                        if let ItemKind::Text { underline, line_spacing, .. } = &mut kind {
                            *underline = true;
                            *line_spacing = 1.5;
                        }
                        self.slide_mut().items.push(Item::new(id, "Text", kind, Pos2::new(160.0, 140.0), Vec2::new(420.0, 200.0)));
                        self.select(vec![id]);
                        self.start_text_edit(id);
                    }
                    // A table with a cell being typed into on the canvas, with its Edit panel open.
                    "tableedit" => {
                        self.show_settings = false;
                        let id = self.doc.alloc_id();
                        let cells = ["Gene", "Fold", "p", "TP53", "2.4", "0.01", "MYC", "", "0.2"].map(String::from).to_vec();
                        let kind = ItemKind::Table { rows: 3, cols: 3, cells, header: true, border: [100, 100, 100, 255], fill: [255, 255, 255, 255], header_fill: [220, 230, 250, 255], text_color: [0, 0, 0, 255], font_size: 20.0 };
                        self.slide_mut().items.push(Item::new(id, "Table", kind, Pos2::new(200.0, 200.0), Vec2::new(450.0, 150.0)));
                        self.select(vec![id]);
                        self.start_cell_edit(id, 4);
                    }
                    "placetext" => {
                        self.show_settings = false;
                        self.actions.push(Action::PlaceText(Pos2::new(500.0, 500.0)));
                    }
                    "select" => {
                        self.show_settings = false;
                        if let Some(it) = self.slide().items.last() {
                            self.actions.push(Action::Select(vec![it.id]));
                        }
                    }
                    // Every line kind, five per row, to check how they draw.
                    "alllines" => {
                        let (w, h) = (self.doc.width, self.doc.height);
                        let (cw, ch) = (w / 5.0, h / 4.0);
                        for (i, k) in LineKind::ALL.iter().enumerate() {
                            let (col, row) = ((i % 5) as f32, (i / 5) as f32);
                            let (lw, lh) = (cw * 0.6, if k.needs_height() { ch * 0.5 } else { 0.0 });
                            let pos = Pos2::new(col * cw + (cw - lw) / 2.0, row * ch + (ch - lh) / 2.0);
                            let id = self.doc.alloc_id();
                            let style = LineStyle::ALL[i % 3];
                            let kind = ItemKind::Line { kind: *k, color: [40, 40, 50, 255], width: 3.0, style, points: Vec::new() };
                            self.slide_mut().items.push(Item::new(id, k.label(), kind, pos, Vec2::new(lw, lh)));
                        }
                    }
                    // The "Edit with AI" chat on the last image, without an AI model connected
                    // (a few frames later, once the steps before it have inserted their items).
                    "aiedit" => {
                        self.show_settings = false;
                        ctx.data_mut(|d| d.insert_temp(egui::Id::new("smoke_aiedit"), true));
                    }
                    "generate" => {
                        self.show_settings = false;
                        self.settings.show_icons = true;
                        self.icons.mode = IconsMode::Generate;
                        self.actions.push(Action::Generate("a flat-style macrophage engulfing a bacterium".into()));
                    }
                    // export=<path>: format from the extension, other options from exportdpi= / exportquality= /
                    // exporttransparent / exportindividual / exportcrop steps given before it.
                    s if s.starts_with("export=") => {
                        self.show_settings = false;
                        let p = PathBuf::from(&s["export=".len()..]);
                        if let Some(f) = p.extension().and_then(|e| ExportFormat::from_ext(&e.to_string_lossy())) {
                            self.settings.export.format = f;
                        }
                        self.actions.push(Action::Export(p));
                    }
                    s if s.starts_with("exportdpi=") => self.settings.export.dpi = s["exportdpi=".len()..].parse().unwrap_or(300),
                    s if s.starts_with("exportquality=") => self.settings.export.jpg_quality = s["exportquality=".len()..].parse().unwrap_or(90),
                    // fav=<asset id>: mark it as a favorite.
                    s if s.starts_with("fav=") => {
                        let id = s["fav=".len()..].to_string();
                        if !self.settings.favorites.contains(&id) {
                            self.settings.favorites.push(id);
                        }
                    }
                    "exporttransparent" => self.settings.export.transparent = true,
                    "exportindividual" => self.settings.export.individual = true,
                    "exportcrop" => self.settings.export.crop = true,
                    "exportall" => self.settings.export.all_slides = true,
                    s if s.starts_with("exportformat=") => {
                        if let Some(f) = ExportFormat::from_ext(&s["exportformat=".len()..]) {
                            self.settings.export.format = f;
                        }
                    }
                    "exportdialog" => { self.show_settings = false; self.show_export = true; }
                    // Export the slide to a path, to see the credits pop up afterwards.
                    s if s.starts_with("export=") => { self.show_attributions = false; self.start_export(ctx, PathBuf::from(&s[7..])); }
                    // Library images saved without their credits (an older document), one of them
                    // AI-edited, and the Attributions window that looks the credits up.
                    "attributions" => {
                        self.show_settings = false;
                        for (i, (id, title)) in [("scidraw/9e80cff8-4f88-47cf-b1ed-a8840ac13809", "mouse"), ("servier/abdominal-aorta", "Abdominal aorta"), ("bioart/369_638402", "Mouse Brain")].into_iter().enumerate() {
                            let mut source = ImageSource::Asset { id: id.into(), key: format!("{id}.png"), svg: None, title: title.into(), source: id.split('/').next().unwrap_or_default().into(), credit: String::new(), license: String::new() };
                            if i == 1 {
                                source = source.derived(self.app_dir.join("generated").join("missing.png").display().to_string());
                            }
                            let item = Item::new(self.doc.alloc_id(), title, ItemKind::Image { source, crop: [0.0, 0.0, 1.0, 1.0], overlay: [255, 255, 255, 0] }, Pos2::new(80.0 + 280.0 * i as f32, 200.0), Vec2::splat(240.0));
                            self.doc.slides[self.cur_slide].items.push(item);
                        }
                        self.show_attributions = true;
                    }
                    // The Attributions window as an export leaves it, with the "Image saved" banner.
                    "exportsaved" => {
                        self.attributions_export = Some((crate::credits::collect(&self.doc.slides, 1), Self::slugify(&self.doc.title)));
                        self.export_saved_at = Some(0.0);
                        self.show_attributions = true;
                    }
                    "offline" => self.open_settings(SettingsTab::Offline),
                    "canvasedit" | "page" => { self.show_settings = false; self.rail = Some(RailTab::Page); }
                    "light" => self.settings.theme = theme::ThemeMode::Light,
                    "dark" => self.settings.theme = theme::ThemeMode::Dark,
                    "grid" => self.settings.show_grid = true,
                    "snap" => self.settings.grid_snap = true,
                    "settings" => self.open_settings(SettingsTab::General),
                    "aisettings" => self.open_settings(SettingsTab::Ai),
                    "about" => self.show_about = true,
                    "templates" => {
                        self.settings.show_icons = true;
                        self.search_database(theme::BIOCARTOON_SOURCE);
                    }
                    "savetemplate" => self.open_upload_dialog(self.template_target(self.cur_slide)),
                    "aisetup" => self.open_ai_setup(None),
                    "nosettings" => self.show_settings = false,
                    s if s.starts_with("search=") => {
                        self.show_settings = false;
                        self.settings.show_icons = true;
                        self.icons.mode = IconsMode::Search;
                        self.icons.query = s["search=".len()..].to_string();
                    }
                    "filters" => {
                        self.settings.show_icons = true;
                        self.icons.mode = IconsMode::Search;
                        self.icons.show_filters = true;
                    }
                    s if s.starts_with("localdir=") => {
                        self.settings.local_data_dir = s["localdir=".len()..].to_string();
                        self.images.configure(self.settings.asset_base_url.clone(), Some(PathBuf::from(&self.settings.local_data_dir)));
                    }
                    s if s.starts_with("rail=") => {
                        self.show_settings = false;
                        match &s["rail=".len()..] {
                            "icons" => self.settings.show_icons = true,
                            "noicons" => self.settings.show_icons = false,
                            "uploads" => { self.settings.show_icons = true; self.icons.mode = IconsMode::Uploads; }
                            "contribute" => { self.settings.show_icons = true; self.open_contribute(); }
                            "suggest" => { self.settings.show_icons = true; self.icons.mode = IconsMode::Suggest; }
                            "favorites" => { self.settings.show_icons = true; self.icons.mode = IconsMode::Favorites; self.request_browse(); }
                            "library" => { self.settings.show_icons = true; self.icons.mode = IconsMode::Library; self.request_browse(); }
                            t => {
                                self.rail = match t {
                                    "text" => { self.placing_text = true; self.rail }, "lines" => Some(RailTab::Lines), "shapes" => Some(RailTab::Shapes),
                                    "table" => Some(RailTab::Table), "code" => Some(RailTab::Code), "edit" => Some(RailTab::Edit), "page" => Some(RailTab::Page), "none" => None, _ => Some(RailTab::Slides),
                                };
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        // `stars`: once results are in, give the first tiles made-up star counts (two of them
        // ours) to look at; the tiles only, nothing is written to the usage file or the Worker.
        let smoke_stars = egui::Id::new("smoke_stars");
        if !self.icons.results.is_empty()
            && !ctx.data(|d| d.get_temp::<bool>(smoke_stars)).unwrap_or(false)
            && std::env::var("BIOCARTOON_SCRIPT").unwrap_or_default().split(',').any(|s| s.trim() == "stars")
        {
            ctx.data_mut(|d| d.insert_temp(smoke_stars, true));
            let fake = [(3, false), (12, true), (1234, false), (56_000, true), (2_300_000, false), (999_999, false)];
            for (r, (n, mine)) in self.icons.results.iter_mut().zip(fake) {
                r.stars = n;
                r.starred = mine;
            }
        }
        if frame == 8 && ctx.data(|d| d.get_temp::<bool>(egui::Id::new("smoke_aiedit"))).unwrap_or(false) {
            if let Some(it) = self.slide().items.iter().rev().find(|i| matches!(i.kind, ItemKind::Image { .. })) {
                let id = it.id;
                self.actions.push(Action::Select(vec![id]));
                self.ai_edit = Some(AiEdit { item: id, messages: vec![(false, AI_EDIT_GREETING.into())], input: String::new(), waiting: false, stage: AiChatStage::Start });
            }
        }
        let shot_secs: f64 = std::env::var("BIOCARTOON_SHOT_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(3.0);
        let now = ctx.input(|i| i.time);
        let fired = ctx.data(|d| d.get_temp::<bool>(egui::Id::new("shot_fired"))).unwrap_or(false);
        if now > shot_secs && !fired {
            ctx.data_mut(|d| d.insert_temp(egui::Id::new("shot_fired"), true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(path.clone())));
        }
        let shot = ctx.input(|i| i.events.iter().find_map(|e| match e {
            egui::Event::Screenshot { image, user_data, .. } if user_data.data.as_ref().and_then(|d| d.downcast_ref::<String>()).is_some() => Some(image.clone()),
            _ => None,
        }));
        if let Some(img) = shot {
            let [w, h] = img.size;
            if let Err(e) = image::save_buffer(&path, img.as_raw(), w as u32, h as u32, image::ColorType::Rgba8) {
                log::error!("screenshot: {e}");
            } else {
                log::info!("wrote screenshot {path}");
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint();
    }
}

/// The AI settings tab's editors for the prompt text sent with each request. The system
/// prompts are left out when the selected model is an image model, which never sees them.
fn ai_prompt_fields(ui: &mut egui::Ui, prompts: &mut AiPrompts, svg: bool) {
    let defaults = AiPrompts::default();
    let dim = |t: &str| egui::RichText::new(t).color(theme::text_dim());
    // A titled text box with a Reset button that's live once it differs from the default.
    let field = |ui: &mut egui::Ui, title: &str, about: &str, text: &mut String, default: &str, rows: usize| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(title).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(text.as_str() != default, egui::Button::new("Reset")).clicked() {
                    *text = default.into();
                }
            });
        });
        ui.label(dim(about));
        ui.add(egui::TextEdit::multiline(text).desired_rows(rows).desired_width(f32::INFINITY));
        ui.add_space(8.0);
    };

    egui::ScrollArea::vertical().min_scrolled_height(440.0).max_height(440.0).show(ui, |ui| {
        ui.label(egui::RichText::new("Generate with AI").size(15.0).strong());
        ui.add_space(4.0);
        field(ui, "Image prompt", "Wraps what you type on the Generate page, for every provider. {prompt} marks where your text goes; without it, your text is added at the end.", &mut prompts.generate, &defaults.generate, 2);
        if svg {
            field(ui, "Illustrator instructions", "System prompt for the model, used when generating and editing images.", &mut prompts.illustrator, &defaults.illustrator, 3);
            ui.label(dim("Always added after it, so the app can read the reply:"));
            ui.label(egui::RichText::new(crate::ai::SVG_RULES).small().color(theme::text_dim()));
        }
        ui.add_space(12.0);

        ui.label(egui::RichText::new("Edit with AI").size(15.0).strong());
        ui.add_space(4.0);
        field(ui, "Edit prompt", "Wraps the change you ask for, for every provider. {prompt} marks where your request goes.", &mut prompts.edit, &defaults.edit, 2);
        if svg {
            field(ui, "Assistant instructions", "System prompt for the dialog's text replies. It doesn't change the image.", &mut prompts.chat, &defaults.chat, 4);
        } else {
            ui.label(dim("Image models take no system prompt, so the illustrator and assistant instructions are hidden. They appear when you pick a model that draws SVG (Anthropic, an OpenAI chat model, or vLLM)."));
        }
    });
}

/// Provider / model / API key fields shared by the AI setup dialog and the AI settings tab.
fn ai_fields(ui: &mut egui::Ui, provider: &mut BackendKind, model: &mut String, key: &mut String, base_url: &mut String) {
    egui::Grid::new(ui.id().with("ai_fields")).num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
        ui.label("Provider");
        let before = *provider;
        egui::ComboBox::from_id_salt("ai_provider").selected_text(provider.label()).width(220.0).show_ui(ui, |ui| {
            for k in BackendKind::ALL {
                ui.selectable_value(provider, k, k.label());
            }
        });
        if *provider != before {
            *model = provider.models().first().copied().unwrap_or_default().into();
        }
        ui.end_row();

        // vLLM serves the model it was started with and needs no key.
        if provider.self_hosted() {
            ui.label("Server URL");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(base_url).hint_text("http://localhost:8000").desired_width(220.0));
                ui.hyperlink_to("Docs", provider.key_url());
            });
            ui.end_row();
            // The web app's CSP (csp.py) only reaches localhost, and the server must allow its origin.
            if cfg!(target_arch = "wasm32") {
                ui.label("");
                ui.label(
                    egui::RichText::new("Start vLLM with --allowed-origins '[\"https://app.biocartoon.com\"]',\nthen enter http://localhost:8000.")
                        .small()
                        .color(theme::text_dim()),
                );
                ui.end_row();
            }
            return;
        }

        ui.label("Model");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(model).desired_width(220.0));
            ui.menu_button(ph::CARET_DOWN, |ui| {
                for m in provider.models() {
                    if ui.button(*m).clicked() {
                        *model = (*m).into();
                        ui.close();
                    }
                }
            });
        });
        ui.end_row();

        ui.label("API key");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(key).password(true).hint_text("Paste your key").desired_width(220.0));
            ui.hyperlink_to("Get a key", provider.key_url());
        });
        ui.end_row();
    });
}

/// One centered line of plain text followed by a link.
fn linked_line(ui: &mut egui::Ui, text: &str, link: &str, url: &str) {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let w = ui.fonts_mut(|f| f.layout_no_wrap(format!("{text}{link}"), font, Color32::WHITE).size().x);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(((ui.available_width() - w) / 2.0).max(0.0));
        ui.label(text);
        ui.hyperlink_to(link, url);
    });
}

impl eframe::App for BioCartoon {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }

    /// On macOS, Ctrl+click is a right click: the press and its release become secondary-button events.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if ctx.os() != egui::os::OperatingSystem::Mac {
            return;
        }
        for e in raw_input.events.iter_mut() {
            if let egui::Event::PointerButton { button, pressed, modifiers, .. } = e {
                if *button != egui::PointerButton::Primary {
                    continue;
                }
                if *pressed {
                    self.ctrl_click = modifiers.ctrl && !modifiers.mac_cmd;
                }
                if self.ctrl_click {
                    *button = egui::PointerButton::Secondary;
                    modifiers.ctrl = false;
                    if !*pressed {
                        self.ctrl_click = false;
                    }
                }
            }
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if std::env::var("BIOCARTOON_SCREENSHOT").is_ok() {
            return; // smoke-test runs must not persist their scripted settings
        }
        if let Ok(s) = serde_json::to_string(&self.settings) {
            storage.set_string("settings", s);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.screen == Screen::Editor {
            self.save_current(Snapshot::IfChanged, "Edited");
        }
    }
}

/// Open a new mail message with `files` attached (Mail on macOS). Falls back to a mailto: link
/// and showing the files, since mailto: can't carry attachments.
fn open_mail(subject: &str, files: &[PathBuf]) {
    #[cfg(target_os = "macos")]
    {
        let ok = std::process::Command::new("open").arg("-a").arg("Mail").args(files).status().map(|s| s.success()).unwrap_or(false);
        if ok {
            return;
        }
    }
    let enc = |s: &str| s.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect::<String>();
    let url = format!("mailto:?subject={}&body={}", enc(subject), enc("Attach the files from the folder that just opened."));
    let folder = files.first().and_then(|f| f.parent()).map(Path::to_path_buf);
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(target_os = "windows")]
    let opener = "explorer";
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let opener = "xdg-open";
    let _ = std::process::Command::new(opener).arg(&url).status();
    if let Some(d) = folder {
        let _ = std::process::Command::new(opener).arg(d).status();
    }
}
