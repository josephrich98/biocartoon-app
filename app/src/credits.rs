//! Attributions: who made the artwork a document uses, and the text that credits them in a figure
//! legend. Each catalog image carries its credit line in the document (see
//! [`ImageSource::Asset`]); documents saved before that are filled in from the catalog.

use crate::app::BioCartoon;
use crate::doc::{Document, ImageSource, Item, ItemKind, Slide};
use crate::theme;
use biocartoon_catalog::catalog::AssetRow;
use egui::Vec2;
use egui_phosphor::regular as ph;

/// One asset the document uses, however many times.
#[derive(Clone, Debug, PartialEq)]
pub struct Credit {
    pub id: String,
    pub title: String,
    /// The ready-to-print attribution line; empty while it is being looked up.
    pub credit: String,
    pub license: String,
    /// Its license requires the credit (CC BY, CC BY-SA, MIT, BSD...), rather than just welcoming it.
    pub required: bool,
    /// Some uses are edited copies (an AI edit), which still owe the original's credit.
    pub derived: bool,
    /// 1-based numbers of the slides it appears on.
    pub slides: Vec<usize>,
}

/// Public domain and CC0 ask for nothing; anything else (or an unknown license) wants credit.
pub fn license_needs_credit(license: &str) -> bool {
    !matches!(license, "PD" | "CC0-1.0")
}

/// The credit line for one of a template's own images; keep in step with `creditLine` in the
/// biocartoon repository's worker/src/validate.ts, which sets it on published templates.
pub fn template_credit(title: &str, author: &str, license: &str, template: &str) -> String {
    let by = if author.is_empty() || author == "anonymous" { String::new() } else { format!(" by {author}") };
    let lic = match license {
        "CC0-1.0" => "CC0".to_string(),
        "CC-BY-4.0" => "CC-BY-4.0 (https://creativecommons.org/licenses/by/4.0/)".to_string(),
        l => l.to_string(),
    };
    format!("\"{title}\"{by}, template \"{template}\", {lic}")
}

/// The credit line of a template itself, for its hover text: the one its metadata carries, else
/// `"Title" by Author, BioCartoon, CC0` (a community template names the library's contribution).
pub fn template_row_credit(meta: &crate::templates::TemplateMeta) -> String {
    if !meta.credit.is_empty() {
        return meta.credit.clone();
    }
    let by = if meta.author.is_empty() || meta.author == "anonymous" { String::new() } else { format!(" by {}", meta.author) };
    let from = if meta.official || meta.author == theme::BIOCARTOON_AUTHOR { "BioCartoon" } else { "BioCartoon community contribution" };
    let lic = match meta.license.as_str() {
        "CC0-1.0" => "CC0",
        "" => "license unknown",
        l => l,
    };
    format!("\"{}\"{by}, {from}, {lic}", meta.title)
}

/// The asset an image item is, or was made from.
fn origin(it: &Item) -> Option<(&ImageSource, bool)> {
    let ItemKind::Image { source, .. } = &it.kind else { return None };
    let o = source.origin()?;
    matches!(o, ImageSource::Asset { id, .. } if !id.is_empty()).then_some((o, !matches!(source, ImageSource::Asset { .. })))
}

/// The assets on `slides` (numbered from `first`), in order of first appearance.
pub fn collect<'a>(slides: impl IntoIterator<Item = &'a Slide>, first: usize) -> Vec<Credit> {
    let mut out: Vec<Credit> = Vec::new();
    for (n, slide) in slides.into_iter().enumerate() {
        for it in &slide.items {
            let Some((ImageSource::Asset { id, title, credit, license, .. }, derived)) = origin(it) else { continue };
            let n = n + first;
            match out.iter_mut().find(|c| &c.id == id) {
                Some(c) => {
                    c.derived |= derived;
                    if c.slides.last() != Some(&n) {
                        c.slides.push(n);
                    }
                }
                None => out.push(Credit {
                    id: id.clone(),
                    title: title.clone(),
                    credit: credit.clone(),
                    license: license.clone(),
                    // Not known until the credit is looked up: assume it is needed.
                    required: license.is_empty() || license_needs_credit(license),
                    derived,
                    slides: vec![n],
                }),
            }
        }
    }
    out
}

/// The credits of a set of items, one per line (embedded in exported files, or saved beside them).
pub fn for_items(items: &[&Item], only_required: bool) -> String {
    let slide = Slide { name: String::new(), items: items.iter().map(|it| (*it).clone()).collect(), comments: Vec::new(), background: None };
    list(&collect([&slide], 1), only_required)
}

/// The credits that have a line (optionally just those the licenses require), alphabetical by
/// that line, ignoring case and the opening quote around the title.
pub fn sorted(credits: &[Credit], only_required: bool) -> Vec<&Credit> {
    let mut out: Vec<&Credit> = credits.iter().filter(|c| (!only_required || c.required) && !c.credit.is_empty()).collect();
    let key = |c: &Credit| c.credit.trim_start_matches(['"', '\u{201c}', '\'']).to_lowercase();
    out.sort_by(|a, b| key(a).cmp(&key(b)).then_with(|| a.credit.cmp(&b.credit)));
    out
}

/// The credit lines, alphabetical, one per line, optionally just those the licenses require.
pub fn list(credits: &[Credit], only_required: bool) -> String {
    sorted(credits, only_required).iter().map(|c| c.credit.as_str()).collect::<Vec<_>>().join("\n")
}

/// Ids of catalog assets in `doc` that don't carry their credit yet (documents saved before
/// credits were stored), or that were placed from their PNG before the catalog had their SVG.
/// Template images are left out: their credit comes with the template.
pub fn missing(doc: &Document) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for it in doc.slides.iter().flat_map(|s| &s.items) {
        let no_svg = matches!(&it.kind, ItemKind::Image { source: ImageSource::Asset { svg: None, .. }, .. });
        if let Some((ImageSource::Asset { id, source, credit, .. }, _)) = origin(it) {
            if (credit.is_empty() || no_svg) && source != "template" && !ids.contains(id) {
                ids.push(id.clone());
            }
        }
    }
    ids
}

/// Copy credit lines from catalog rows into every image of `slides` that uses them, and give
/// images placed from a PNG the SVG the catalog now has, so they export as vectors. A PNG
/// shows the SVG's whole artboard, so the image is unfitted again and the editor (and export)
/// moves it onto the drawing once the SVG loads; BioArt crops its PNGs to the drawing already.
pub fn fill<'a>(slides: impl IntoIterator<Item = &'a mut Slide>, rows: &[AssetRow]) {
    for it in slides.into_iter().flat_map(|s| s.items.iter_mut()) {
        let ItemKind::Image { source, .. } = &mut it.kind else { continue };
        if let ImageSource::Asset { id, svg: svg @ None, source: db, .. } = source {
            if let Some(key) = rows.iter().find(|r| &r.id == id).and_then(|r| r.svg_key.clone()).filter(|_| db != "template") {
                *svg = Some(key);
                it.fitted = db == "bioart";
            }
        }
        if let Some(ImageSource::Asset { id, credit, license, .. }) = source.origin_mut() {
            if let Some(r) = rows.iter().find(|r| &r.id == id) {
                *credit = r.credit.clone();
                *license = r.license.clone();
            }
        }
    }
}

impl BioCartoon {
    /// The title and credit line a library image (or an edit of one) owes; the credit is looked up
    /// in the loaded catalog rows when the document doesn't carry it yet.
    pub fn item_credit(&self, it: &Item) -> Option<(String, String)> {
        let (ImageSource::Asset { id, title, credit, .. }, _) = origin(it)? else { return None };
        let credit = Some(credit.clone())
            .filter(|c| !c.is_empty())
            .or_else(|| self.icons.results.iter().chain(self.variants.iter()).find(|r| &r.id == id).map(|r| r.credit.clone()))
            .unwrap_or_else(|| format!("Asset {id}"));
        Some((title.clone(), credit))
    }

    /// Whether generative AI made a library asset, and how (its uploader's or our record's words;
    /// empty when not said), from its catalog row once loaded.
    pub fn asset_ai(&self, id: &str) -> Option<(Option<bool>, String)> {
        self.asset_row(id).map(|r| (r.generative_ai_used, r.generative_ai_details.clone()))
    }

    /// Whether generative AI made a library asset (`Some(None)`: the catalog doesn't say), from a
    /// loaded catalog row or a credit lookup; `None` until one of them has it.
    pub fn asset_ai_flag(&self, id: &str) -> Option<Option<bool>> {
        self.asset_row(id).map(|r| r.generative_ai_used).or_else(|| self.asset_ai_flags.get(id).copied())
    }

    /// Ask the catalog whether AI made those of `ids` it hasn't told us about yet (each once), for
    /// the note beside their credits.
    pub fn look_up_ai(&mut self, ids: impl IntoIterator<Item = String>) {
        if !self.catalog.ok {
            return;
        }
        let ids: Vec<String> = ids.into_iter().filter(|id| self.asset_ai_flag(id).is_none() && self.credit_lookups.insert(id.clone())).collect();
        if !ids.is_empty() {
            self.catalog.send(crate::catalog::Req::Lookup { ids });
        }
    }

    /// The gray note beside one asset's credit (see [`ai_note`]).
    pub fn asset_ai_note(&self, id: &str) -> Option<&'static str> {
        ai_note(self.asset_ai_flag(id))
    }

    /// A library asset's catalog row, once loaded.
    pub fn asset_row(&self, id: &str) -> Option<&crate::catalog::AssetRow> {
        self.icons.results.iter().chain(self.variants.iter()).find(|r| r.id == id)
    }

    /// The catalog id of the asset an image item is, or was made from.
    pub fn item_asset_id(&self, it: &Item) -> Option<String> {
        let (ImageSource::Asset { id, .. }, _) = origin(it)? else { return None };
        Some(id.clone())
    }

    /// Right-click > Show credit: one image's credit, with a button to copy it.
    pub fn credit_window(&mut self, ctx: &egui::Context) {
        let Some(id) = self.credit_item else { return };
        let Some((title, credit)) = self.item(id).and_then(|it| self.item_credit(it)) else {
            self.credit_item = None;
            return;
        };
        let asset = self.item(id).and_then(|it| self.item_asset_id(it));
        self.look_up_ai(asset.clone());
        let note = asset.and_then(|a| self.asset_ai_note(&a));
        let mut open = true;
        let mut copied = false;
        egui::Window::new(format!("{}  Credit", ph::QUOTES))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(360.0)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(egui::RichText::new(&title).strong());
                credit_line(ui, &credit, note);
                ui.add_space(6.0);
                if ui.button(format!("{}  Copy credit", ph::COPY)).clicked() {
                    ui.ctx().copy_text(credit.clone());
                    copied = true;
                }
            });
        if copied {
            self.toast("Copied the credit");
        }
        if !open {
            self.credit_item = None;
        }
    }

    /// Credits for one slide or the whole document, with copy/save actions for a figure legend.
    pub fn attributions_window(&mut self, ctx: &egui::Context) {
        if !self.show_attributions {
            return;
        }
        let stem = BioCartoon::slugify(&self.doc.title);
        let export_scope = self.attributions_export.as_ref().map(|(_, _, all)| *all);
        let mut all_slides = export_scope.unwrap_or(self.attributions_all_slides);
        let slide = self.attributions_slide.filter(|&i| i < self.doc.slides.len()).unwrap_or(self.cur_slide);
        let (credits, title, stem) = match (&self.attributions_export, all_slides) {
            // Just after an export: the credits of what was exported.
            (Some((credits, stem, _)), _) => {
                // Fill in credits that were still being looked up when the export started.
                let live = collect(&self.doc.slides, 1);
                let credits = credits
                    .iter()
                    .map(|c| match live.iter().find(|l| l.id == c.id && c.credit.is_empty()) {
                        Some(l) => Credit { credit: l.credit.clone(), required: l.required, ..c.clone() },
                        None => c.clone(),
                    })
                    .collect();
                (credits, format!("{}  Credits for your export", ph::QUOTES), stem.clone())
            }
            (None, false) => (collect([&self.doc.slides[slide]], slide + 1), format!("{}  Attributions: slide {}", ph::QUOTES, slide + 1), format!("{stem}-slide{}", slide + 1)),
            (None, true) => (collect(&self.doc.slides, 1), format!("{}  Attributions", ph::QUOTES), stem),
        };
        self.look_up_ai(credits.iter().map(|c| c.id.clone()));
        let ai: std::collections::HashMap<String, Option<bool>> = credits.iter().filter_map(|c| Some((c.id.clone(), self.asset_ai_flag(&c.id)?))).collect();
        let mut open = true;
        let mut action = None;
        let window = egui::Window::new(title)
            .id(egui::Id::new("attributions_window"))
            .open(&mut open)
            .collapsible(false)
            .default_size([480.0, 460.0])
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                action = attributions_ui(ui, &credits, &ai, &mut self.attributions_required_only, &mut all_slides, export_scope.is_some());
            });
        if export_scope.is_none() {
            self.attributions_all_slides = all_slides;
        }
        // For a few seconds after an export, a green banner just above the window says it worked.
        let now = ctx.input(|i| i.time);
        if let (Some(at), Some(w), Some(_)) = (self.export_saved_at, &window, &self.attributions_export) {
            if now < at + SAVED_BANNER_SECS {
                let rect = w.response.rect;
                egui::Area::new(egui::Id::new("export_saved_banner"))
                    .fixed_pos(rect.left_top() - Vec2::new(0.0, 40.0))
                    .order(egui::Order::Foreground)
                    .interactable(false)
                    .show(ctx, |ui| {
                        egui::Frame::new().fill(theme::success()).corner_radius(8).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
                            ui.set_width(rect.width() - 24.0);
                            ui.label(egui::RichText::new(format!("{}  Image saved successfully", ph::CHECK_CIRCLE)).color(egui::Color32::WHITE).strong());
                        });
                    });
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(at + SAVED_BANNER_SECS - now));
            } else {
                self.export_saved_at = None;
            }
        }
        if let Some(a) = action {
            self.credits_action(ctx, a, &credits, &stem);
        }
        self.show_attributions = open;
        if !open {
            self.attributions_export = None;
            self.export_saved_at = None;
        }
    }

    /// Copy or save the credit list (one per line), as a button of the Attributions popup asked.
    fn credits_action(&mut self, ctx: &egui::Context, action: CreditsAction, credits: &[Credit], stem: &str) {
        match action {
            CreditsAction::Copy { required } => {
                ctx.copy_text(shared_list(credits, required));
                self.toast(if required { "Copied the required credits" } else { "Copied all credits" });
            }
            CreditsAction::Save { required } => {
                let name = format!("{stem}-{}credits.txt", if required { "required-" } else { "" });
                let Some(path) = crate::platform::save_path("Text", "txt", &name) else { return };
                match crate::platform::write_output(&path, format!("{}\n", shared_list(credits, required)).as_bytes()) {
                    Ok(()) => self.toast(if required { "Saved the required credits" } else { "Saved all credits" }),
                    Err(e) => self.toast_err(&format!("{}: {e}", path.display())),
                }
            }
        }
    }
}

/// The credit list displayed and handed over by copy or save: all credits end by naming the app; the
/// required ones stay just what the licenses ask for.
fn shared_list(credits: &[Credit], only_required: bool) -> String {
    let lines = list(credits, only_required);
    if only_required { lines } else { format!("{lines}\n{MADE_IN}") }
}

/// A credit, selectable to copy, then "· " and its AI note in small gray (not part of the credit).
pub fn credit_line(ui: &mut egui::Ui, credit: impl Into<egui::WidgetText>, note: Option<&str>) {
    ui.horizontal_wrapped(|ui| {
        ui.add(egui::Label::new(credit).wrap().selectable(true));
        if let Some(note) = note {
            ui.add(egui::Label::new(egui::RichText::new(format!("· {note}")).small().color(theme::text_dim())).selectable(false));
        }
    });
}

/// The line that ends the full credit list.
const MADE_IN: &str = "Made in BioCartoon.";

/// The gray note beside a credit in the Attributions popup: `None` while the catalog hasn't said,
/// or when the art was made without AI.
fn ai_note(used: Option<Option<bool>>) -> Option<&'static str> {
    match used? {
        Some(false) => None,
        u => Some(crate::panels::ai_label(u)),
    }
}

/// How long the "Image saved successfully" banner stays above the Attributions popup.
const SAVED_BANNER_SECS: f64 = 4.0;

/// A button of the Attributions popup: copy or save the credits (`required`: only those the
/// licenses require).
enum CreditsAction {
    Copy { required: bool },
    Save { required: bool },
}

/// The body of the Attributions popup. Returns what a copy or save button asked for, if one was clicked.
/// `ai`: whether generative AI made each asset, where the catalog has said.
fn attributions_ui(ui: &mut egui::Ui, credits: &[Credit], ai: &std::collections::HashMap<String, Option<bool>>, only: &mut bool, all_slides: &mut bool, export: bool) -> Option<CreditsAction> {
    let mut action = None;
    let pending = credits.iter().filter(|c| c.credit.is_empty()).count();
    let dim = |t: &str| egui::RichText::new(t).small().color(theme::text_dim());
    if credits.is_empty() {
        ui.label("There is no library artwork here, so there is no one to credit.");
        ui.label(dim("Images you uploaded or generated yourself are listed here once they are in the library."));
    }
    let required = credits.iter().filter(|c| c.required).count();
    ui.label(format!(
        "{} {} from the library; {required} {} credit.",
        credits.len(),
        if credits.len() == 1 { "piece of artwork" } else { "pieces of artwork" },
        if required == 1 { "requires" } else { "require" }
    ));
    if pending > 0 {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(dim(&format!("Looking up {pending} credit{} in the catalog...", if pending == 1 { "" } else { "s" })));
        });
    }
    ui.add_space(4.0);
    let has_required = !list(credits, true).is_empty();
    let has_any = !list(credits, false).is_empty();
    ui.horizontal(|ui| {
        if ui.add_enabled(has_any, egui::Button::new(format!("{}  Copy all credits", ph::COPY))).clicked() {
            action = Some(CreditsAction::Copy { required: false });
        }
        if ui.add_enabled(has_required, egui::Button::new(format!("{}  Copy required credits", ph::COPY))).clicked() {
            action = Some(CreditsAction::Copy { required: true });
        }
    });
    ui.horizontal(|ui| {
        let save_all = egui::RichText::new(format!("{}  Save all credits", ph::FLOPPY_DISK)).color(egui::Color32::WHITE);
        // Just after an export, Enter saves them all. (Not when opened from the menu: that window stays
        // open beside the canvas, where Enter edits the selected text.)
        if ui.add_enabled(has_any, egui::Button::new(save_all).fill(theme::accent())).clicked() || (export && has_any && theme::enter_pressed(ui)) {
            action = Some(CreditsAction::Save { required: false });
        }
        if ui.add_enabled(has_required, egui::Button::new(format!("{}  Save required credits", ph::FLOPPY_DISK))).clicked() {
            action = Some(CreditsAction::Save { required: true });
        }
    });
    ui.label(dim(
        "AI-generated labels for third party assets and user-generated assets may contain errors.",
    ));
    ui.separator();
    // A filter for the list below, set apart from it so it doesn't read as a credit.
    ui.checkbox(only, "Only show credits for licenses requiring credit")
        .on_hover_text("Public-domain and CC0 artwork needs no credit, but artists still appreciate it.");
    ui.add_enabled(!export, egui::Checkbox::new(all_slides, "Show all slides"))
        .on_hover_text("Include credits for artwork on every slide.")
        .on_disabled_hover_text("Fixed to the slides selected for this export.");
    ui.separator();
    // The lines `shared_list` copies and saves, in its order, each with a small gray note of AI use
    // for the author (not copied: it isn't part of the credit). Art made without AI gets no note.
    egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
        for c in sorted(credits, *only) {
            credit_line(ui, &c.credit, ai_note(ai.get(&c.id).copied()));
        }
        if !*only && has_any {
            ui.add(egui::Label::new(MADE_IN).selectable(true));
        }
    });
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Pos2, Vec2};

    fn asset(id: &str, credit: &str, license: &str) -> ImageSource {
        ImageSource::Asset { id: id.into(), key: format!("{id}.png"), svg: None, title: id.into(), source: "servier".into(), credit: credit.into(), license: license.into() }
    }

    fn image(n: u64, source: ImageSource) -> Item {
        Item::new(n, "img", ItemKind::Image { source, crop: [0.0, 0.0, 1.0, 1.0], overlay: [0; 4], layers: Vec::new() }, Pos2::ZERO, Vec2::splat(10.0))
    }

    #[test]
    fn collects_dedupes_and_follows_edits() {
        let cell = asset("servier/cell", "\"Cell\", Servier Medical Art, CC-BY-4.0", "CC-BY-4.0");
        let tube = asset("bioart/tube", "\"Tube\", NIAID NIH BioArt Source, public domain", "PD");
        let s1 = Slide { name: String::new(), items: vec![image(1, cell.clone()), image(2, tube.clone()), image(3, cell.clone())], comments: Vec::new(), background: None };
        let s2 = Slide { name: String::new(), items: vec![image(4, cell.derived("/tmp/edit.png".into())), image(5, ImageSource::file("/tmp/own.png".into()))], comments: Vec::new(), background: None };
        let c = collect([&s1, &s2], 1);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].slides, vec![1, 2]);
        assert!(c[0].derived && c[0].required);
        assert!(!c[1].required);
        assert_eq!(list(&c, true), "\"Cell\", Servier Medical Art, CC-BY-4.0");
        assert!(list(&c, false).ends_with("CC-BY-4.0\n\"Tube\", NIAID NIH BioArt Source, public domain"));
        // Alphabetical whatever the order on the slides, ignoring case and the opening quote.
        let ant = asset("servier/ant", "ant drawing, CC0", "CC0-1.0");
        let s3 = Slide { name: String::new(), items: vec![image(6, tube.clone()), image(7, ant), image(8, cell.clone())], comments: Vec::new(), background: None };
        let c = collect([&s3], 1);
        assert_eq!(c[0].id, "bioart/tube");
        assert_eq!(list(&c, false), "ant drawing, CC0\n\"Cell\", Servier Medical Art, CC-BY-4.0\n\"Tube\", NIAID NIH BioArt Source, public domain");
        assert_eq!(shared_list(&c, true), "\"Cell\", Servier Medical Art, CC-BY-4.0");
        assert!(shared_list(&c, false).ends_with("public domain\nMade in BioCartoon."));
        // An edit of an edit still points at the catalog asset.
        let twice = ImageSource::file("/tmp/a.png".into()).derived("/tmp/b.png".into());
        assert!(twice.origin().is_none());
        let twice = cell.derived("/tmp/a.png".into());
        let twice = twice.derived("/tmp/b.png".into());
        assert_eq!(twice.origin(), Some(&cell));
    }

    #[test]
    fn ai_note_skips_art_made_without_ai() {
        assert_eq!(ai_note(Some(Some(true))), Some("AI-generated"));
        assert_eq!(ai_note(Some(None)), Some("AI use not specified"));
        assert_eq!(ai_note(Some(Some(false))), None);
        assert_eq!(ai_note(None), None);
    }

    #[test]
    fn fills_missing_credits() {
        let mut doc = Document::default();
        doc.slides[0].items.push(image(1, asset("servier/cell", "", "")));
        doc.slides[0].items.push(image(2, asset("servier/cell", "", "").derived("/tmp/e.png".into())));
        assert_eq!(missing(&doc), vec!["servier/cell".to_string()]);
        let row = AssetRow {
            id: "servier/cell".into(),
            source: "servier".into(),
            source_name: "Servier Medical Art".into(),
            title: "Cell".into(),
            variant: String::new(),
            category: String::new(),
            description: String::new(),
            tags: Vec::new(),
            license: "CC-BY-4.0".into(),
            credit: "\"Cell\", Servier".into(),
            author: String::new(),
            png_key: String::new(),
            png_local: None,
            png_width: 1,
            png_height: 1,
            svg_key: Some("servier/cell.svg".into()),
            stars: 0,
            starred: false,
            taxa: Vec::new(),
            layers: Vec::new(),
            generative_ai_used: None,
            generative_ai_details: String::new(),
            file_bytes: 0,
            thumb_key: None,
        };
        fill(doc.slides.iter_mut(), &[row]);
        assert!(missing(&doc).is_empty());
        // The image placed from the PNG gets the SVG, to be fitted to its drawing once it loads.
        let it = &doc.slides[0].items[0];
        assert!(matches!(&it.kind, ItemKind::Image { source: ImageSource::Asset { svg: Some(k), .. }, .. } if k == "servier/cell.svg"));
        assert!(!it.fitted);
        let c = collect(&doc.slides, 1);
        assert_eq!(c[0].credit, "\"Cell\", Servier");
        assert!(c[0].derived);
    }

    #[test]
    fn template_credit_matches_the_worker() {
        assert_eq!(template_credit("pic", "ann", "CC0-1.0", "Pathway"), "\"pic\" by ann, template \"Pathway\", CC0");
    }

    #[test]
    fn template_row_credit_prefers_the_stored_line() {
        use crate::templates::TemplateMeta;
        let m = TemplateMeta { title: "Eukaryotic cell".into(), author: "BioCartoon".into(), license: "CC0-1.0".into(), credit: "\"Eukaryotic cell\" by Joseph Rich, BioCartoon, CC0".into(), ..Default::default() };
        assert_eq!(template_row_credit(&m), "\"Eukaryotic cell\" by Joseph Rich, BioCartoon, CC0");
        let m = TemplateMeta { title: "Pathway".into(), author: "ann".into(), license: "CC-BY-4.0".into(), ..Default::default() };
        assert_eq!(template_row_credit(&m), "\"Pathway\" by ann, BioCartoon community contribution, CC-BY-4.0");
        let m = TemplateMeta { title: "Cell".into(), author: "BioCartoon".into(), license: "CC0-1.0".into(), ..Default::default() };
        assert_eq!(template_row_credit(&m), "\"Cell\" by BioCartoon, BioCartoon, CC0");
    }
}
