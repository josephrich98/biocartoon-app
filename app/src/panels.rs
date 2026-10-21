//! The Icons panel on the left (search, AI generation, categories, databases including
//! BioCartoon's own templates, user-generated assets, uploads), and the rail on the right with
//! its panel (Text / Lines / Shapes / Table / Edit / Slides / Page).

use crate::app::{Action, AlignKind, ArrangeKind, BioCartoon, GeneratedImage, IconsMode, Include, RailTab, SettingsTab, UploadTarget, GRID_PRESETS, PRESETS};
use crate::templates::TemplateRow;
use crate::assets::ImgState;
use crate::canvas::{draw_slide, tool_button, Xform};
use crate::catalog::{AssetRow, IconSort};
use crate::doc::{c32, c4, CodeLang, CodeTheme, ImageSource, Item, ItemKind, LineFamily, LineKind, LineStyle, ShapeKind, TextAlign, TextFont, TextVAlign, Unit};
use crate::stars::format_stars;
use crate::theme;
use egui::{Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use egui_phosphor::regular as ph;

const PANEL_W: f32 = 340.0;
/// How narrow and wide the side panels can be dragged.
const PANEL_MIN_W: f32 = 260.0;
const PANEL_MAX_W: f32 = 720.0;
const TILE: f32 = 96.0;

/// The Slides panel starts narrower, and can be dragged narrower, than the others.
const SLIDES_W: f32 = 186.0;
const SLIDES_MIN_W: f32 = 150.0;

impl BioCartoon {
    /// The rail at the right edge: one button per [`RailTab`].
    pub fn rail_ui(&mut self, ui: &mut egui::Ui) {
        let frame = egui::Frame::new().fill(theme::bg()).inner_margin(egui::Margin::symmetric(4, 8));
        egui::Panel::right("rail").exact_size(60.0).frame(frame).show_separator_line(false).show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                for t in RailTab::ALL {
                    if t == RailTab::Text || t == RailTab::Slides {
                        ui.add_space(12.0);
                    }
                    let on = if t == RailTab::Text { self.placing_text } else { self.rail == Some(t) };
                    let mut r = tool_button(ui, t.icon(), t.label(), on);
                    if t == RailTab::Text {
                        r = r.on_hover_text("Click, then click on the canvas to add a text box");
                    }
                    if r.clicked() {
                        self.actions.push(Action::SetRail(Some(t)));
                    }
                    ui.add_space(4.0);
                }
            });
        });
    }

    /// The Icons panel, on the left.
    pub fn left_panel(&mut self, ui: &mut egui::Ui) {
        let frame = egui::Frame::new().fill(theme::bg()).inner_margin(egui::Margin { left: 8, right: 0, top: 36, bottom: 8 });
        // Resizable by dragging its right edge; egui remembers the width.
        egui::Panel::left("leftpanel").resizable(true).default_size(PANEL_W + 8.0).size_range(PANEL_MIN_W..=PANEL_MAX_W).frame(frame).show_separator_line(false).show(ui, |ui| {
            egui::Frame::new().fill(theme::panel()).corner_radius(10).inner_margin(egui::Margin::same(12)).stroke(Stroke::new(1.0, theme::border())).show(ui, |ui| {
                ui.set_min_height(ui.available_height());
                ui.set_width(ui.available_width());
                let (title, back) = match self.icons.mode {
                    IconsMode::Home => ("Icons".to_string(), false),
                    IconsMode::Search => ("Search".into(), true),
                    IconsMode::Generate => ("Generate with AI".into(), true),
                    IconsMode::Uploads => ("Uploads".into(), true),
                    IconsMode::Contribute => ("Contribute artwork".into(), true),
                    IconsMode::Suggest => ("Submit a suggestion".into(), true),
                    IconsMode::Favorites => ("Favorites".into(), true),
                    IconsMode::Library => ("Your library".into(), true),
                };
                ui.horizontal(|ui| {
                    if back && ui.add(egui::Button::new(ph::ARROW_LEFT).frame(false)).clicked() {
                        self.icons_back();
                    }
                    ui.label(egui::RichText::new(title).strong().size(15.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(egui::Button::new(ph::X).frame(false)).on_hover_text("Hide the Icons panel").clicked() {
                            self.settings.show_icons = false;
                        }
                    });
                });
                ui.add_space(6.0);
                if self.replace_target.is_some() {
                    egui::Frame::new().fill(theme::accent_dim()).corner_radius(6).inner_margin(6).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(format!("{}  Pick an icon to replace the selection", ph::SWAP));
                            if ui.small_button("Cancel").clicked() {
                                self.replace_target = None;
                            }
                        });
                    });
                }
                egui::ScrollArea::vertical().auto_shrink([false, false]).max_width(ui.available_width()).show(ui, |ui| self.icons_panel(ui));
            });
        });
    }

    /// The panel next to the rail, showing the open [`RailTab`].
    pub fn right_panel(&mut self, ui: &mut egui::Ui) {
        let Some(tab) = self.rail else { return };
        let frame = egui::Frame::new().fill(theme::bg()).inner_margin(egui::Margin { left: 0, right: 4, top: 36, bottom: 8 });
        // Resizable by dragging its left edge. The narrow Slides panel has its own id, so egui
        // remembers its width separately from the other tabs'.
        let panel = if tab == RailTab::Slides {
            egui::Panel::right("rightpanel_slides").resizable(true).default_size(SLIDES_W).size_range(SLIDES_MIN_W..=PANEL_MAX_W)
        } else {
            egui::Panel::right("rightpanel").resizable(true).default_size(PANEL_W + 20.0).size_range(PANEL_MIN_W..=PANEL_MAX_W)
        };
        panel.frame(frame).show_separator_line(false).show(ui, |ui| {
            let margin = if tab == RailTab::Slides { 10 } else { 12 };
            egui::Frame::new().fill(theme::panel()).corner_radius(10).inner_margin(egui::Margin::same(margin)).stroke(Stroke::new(1.0, theme::border())).show(ui, |ui| {
                ui.set_min_height(ui.available_height());
                // No wider than the panel (the frame's margins include its stroke): anything wider would
                // push it out to the left, under the canvas.
                ui.set_width(ui.available_width());
                let title = match tab {
                    RailTab::Edit if self.selection.len() > 1 => format!("{} items", self.selection.len()),
                    t => t.label().to_string(),
                };
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(title).strong().size(15.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(egui::Button::new(ph::X).frame(false)).clicked() {
                            self.actions.push(Action::SetRail(None));
                        }
                    });
                });
                ui.add_space(6.0);
                // max_width: a row too wide for the panel is clipped instead of widening it.
                egui::ScrollArea::vertical().auto_shrink([false, false]).max_width(ui.available_width()).show(ui, |ui| match tab {
                    // Text has no panel: its rail button arms placing a text box on the canvas.
                    RailTab::Text => {}
                    RailTab::Lines => self.lines_panel(ui),
                    RailTab::Shapes => self.shapes_panel(ui),
                    RailTab::Table => self.table_panel(ui),
                    RailTab::Code => self.code_panel(ui),
                    RailTab::Edit => self.edit_panel(ui),
                    RailTab::Slides => self.slides_content(ui),
                    RailTab::Page => self.page_panel(ui),
                });
            });
        });
    }

    /// A database's display name: BioCartoon's own, or one from the catalog.
    fn source_name(&self, source: &str) -> String {
        if source == theme::BIOCARTOON_SOURCE {
            return "BioCartoon".into();
        }
        self.icons.sources.iter().find(|s| s.source == source).map(|s| s.name.clone()).unwrap_or_else(|| source.to_string())
    }

    /// Every database, BioCartoon's own included, as (source, abbreviation, name), in order of
    /// abbreviation.
    fn databases(&self) -> Vec<(String, &'static str, String)> {
        // BioCartoon is listed even before it has icons: its templates are in it.
        let mut dbs: Vec<_> = std::iter::once(theme::BIOCARTOON_SOURCE.to_string())
            .chain(self.icons.sources.iter().map(|s| s.source.clone()).filter(|s| s != theme::BIOCARTOON_SOURCE))
            .map(|s| (theme::source_badge(&s).0, self.source_name(&s), s))
            .map(|(abbr, name, s)| (s, abbr, name))
            .collect();
        // BioCartoon's own first, then the rest by abbreviation.
        dbs.sort_by_key(|d| (d.0 != theme::BIOCARTOON_SOURCE, d.1));
        dbs
    }

    /// The saved templates a browse or search shows next to the icons, for the current menu.
    fn matching_templates(&self) -> Vec<TemplateRow> {
        if !self.icons.include.templates() {
            return Vec::new();
        }
        let rows = self.template_rows.iter();
        match self.icons.mode {
            IconsMode::Search => {
                let f = &self.icons.filter;
                // Templates belong to BioCartoon: another database's filter leaves them out. They
                // are slides, not image files, so a file type leaves them out too.
                if (!f.sources.is_empty() && !f.sources.iter().any(|s| s == theme::BIOCARTOON_SOURCE)) || !f.formats.is_empty() {
                    return Vec::new();
                }
                let eq = |a: &str, b: &str| a.trim().eq_ignore_ascii_case(b.trim());
                let q = self.icons.query.trim().to_lowercase();
                let authors = f.authors();
                // A template's category is free text: it matches a picked category or subcategory by name.
                let places: Vec<&str> = f.cats.iter().map(String::as_str).chain(f.subs.iter().map(|(_, s)| s.as_str())).collect();
                rows.filter(|r| q.is_empty() || [&r.meta.title, &r.meta.description, &r.meta.category].iter().any(|t| t.to_lowercase().contains(&q)))
                    .filter(|r| authors.is_empty() || authors.iter().any(|a| r.meta.author.to_lowercase().contains(a)))
                    .filter(|r| f.licenses.is_empty() || f.licenses.iter().any(|l| eq(l, &r.meta.license)))
                    .filter(|r| places.is_empty() || places.iter().any(|p| eq(p, &r.meta.category)))
                    // Their categories are BioCartoon's own database categories.
                    .filter(|r| f.db_cats.is_empty() || f.db_cats.iter().any(|(s, c)| s == theme::BIOCARTOON_SOURCE && eq(c, &r.meta.category)))
                    .cloned()
                    .collect()
            }
            // The user's own: templates saved on this computer.
            IconsMode::Library => rows.filter(|r| r.json_local.is_some()).cloned().collect(),
            _ => Vec::new(),
        }
    }

    /// Back to the Icons panel's start.
    fn icons_back(&mut self) {
        let i = &mut self.icons;
        i.results.clear();
        i.total = None;
        i.browse_offset = 0;
        i.mode = IconsMode::Home;
    }

    /// The filters under the search box: database, author, category, subcategory, license and file type.
    /// The search re-runs once one changes (see `last_sent_filter`).
    fn filters_panel(&mut self, ui: &mut egui::Ui) {
        if self.icons.facets_for.as_ref().is_none_or(|(q, f)| q != self.icons.query.trim() || *f != self.icons.filter) {
            self.request_facets();
        }
        // Each dropdown lists the choices that find icons under the query and the other filters, with
        // how many; the picked ones stay listed so they can be taken out. Until the counts arrive, all of them.
        let facets = self.icons.facets.clone();
        let label = |name: &str, n: Option<i64>| match n {
            Some(n) => format!("{name} ({})", thousands(n)),
            None => name.to_string(),
        };
        let f = &self.icons.filter;
        let in_facet = |list: &[(String, i64)], v: &str| facets.as_ref().map(|_| list.iter().find(|(k, _)| k == v).map_or(0, |(_, n)| *n));
        let dbs: Vec<(String, String)> = self
            .databases()
            .into_iter()
            .filter_map(|(s, _, name)| {
                let name = if s == "user" { "Community and user-generated".to_string() } else { name };
                let n = in_facet(facets.as_ref().map_or(&[][..], |fc| &fc.sources), &s);
                // BioCartoon's templates are in it even when none of its icons are.
                let keep = n != Some(0) || f.sources.contains(&s) || (s == theme::BIOCARTOON_SOURCE && !self.template_rows.is_empty());
                keep.then(|| (s, label(&name, n)))
            })
            .collect();
        let mut licenses: Vec<String> = match &facets {
            Some(fc) => fc.licenses.iter().map(|(l, _)| l.clone()).collect(),
            None => self.icons.sources.iter().flat_map(|s| s.licenses.split(',')).map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect(),
        };
        licenses.extend(f.licenses.iter().cloned());
        licenses.sort();
        licenses.dedup();
        let licenses: Vec<(String, String)> =
            licenses.into_iter().map(|l| (l.clone(), label(&l, in_facet(facets.as_ref().map_or(&[][..], |fc| &fc.licenses), &l)))).collect();
        let formats: Vec<(String, String)> = [crate::catalog::FORMAT_SVG, crate::catalog::FORMAT_PNG]
            .iter()
            .map(|x| (x.to_string(), label(x, in_facet(facets.as_ref().map_or(&[][..], |fc| &fc.formats), x))))
            .collect();
        let count = |cat: &str, sub: &str| facets.as_ref().map(|fc| fc.taxa.iter().find(|t| t.cat == cat && t.sub == sub).map_or(0, |t| t.count));
        let cats: Vec<(String, String)> = crate::taxonomy::TAXONOMY
            .iter()
            .filter(|(c, _)| count(c, "") != Some(0) || f.cats.iter().any(|p| p == c))
            .map(|(c, _)| (c.to_string(), label(c, count(c, ""))))
            .collect();
        // The subcategories of the picked categories (of all of them when none is picked).
        let mut subs: Vec<((String, String), String)> = Vec::new();
        for (cat, list) in crate::taxonomy::TAXONOMY.iter().filter(|(c, _)| f.cats.is_empty() || f.cats.iter().any(|p| p == c)) {
            for sub in *list {
                let key = (cat.to_string(), sub.to_string());
                let n = count(cat, sub);
                if n != Some(0) || f.subs.contains(&key) {
                    let name = if f.cats.len() == 1 { sub.to_string() } else { format!("{cat}: {sub}") };
                    subs.push((key, label(&name, n)));
                }
            }
        }
        // Each database's own categories, "BA: Viruses", in the order of the databases. BioCartoon's
        // include its templates' categories.
        let mut own: Vec<(String, String, Option<i64>)> = facets.as_ref().map_or(Vec::new(), |fc| fc.db_cats.iter().map(|(s, c, n)| (s.clone(), c.clone(), Some(*n))).collect());
        if f.sources.is_empty() || f.sources.iter().any(|s| s == theme::BIOCARTOON_SOURCE) {
            for c in self.template_rows.iter().map(|r| r.meta.category.trim()).filter(|c| !c.is_empty()) {
                match own.iter_mut().find(|(s, k, _)| s == theme::BIOCARTOON_SOURCE && k.eq_ignore_ascii_case(c)) {
                    Some((_, _, n)) => *n = Some(n.unwrap_or(0) + 1),
                    None => own.push((theme::BIOCARTOON_SOURCE.into(), c.to_string(), Some(1))),
                }
            }
        }
        for (s, c) in &f.db_cats {
            if !own.iter().any(|(os, oc, _)| os == s && oc == c) {
                own.push((s.clone(), c.clone(), facets.as_ref().map(|_| 0)));
            }
        }
        let order: Vec<String> = self.databases().into_iter().map(|(s, ..)| s).collect();
        own.sort_by_key(|(s, c, _)| (order.iter().position(|o| o == s).unwrap_or(usize::MAX), c.to_lowercase()));
        let db_cats: Vec<((String, String), String)> = own
            .into_iter()
            .map(|(s, c, n)| {
                let l = label(&format!("{}: {c}", theme::source_badge(&s).0), n);
                ((s, c), l)
            })
            .collect();
        let f = &mut self.icons.filter;
        egui::Frame::new().fill(theme::panel_raised()).corner_radius(8).inner_margin(8).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let w = ui.available_width() - 84.0;
            egui::Grid::new("search_filters").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                ui.label("Database");
                if multi_select(ui, "filter_db", w, &mut f.sources, &dbs) && !f.sources.is_empty() {
                    // Another database's own categories would find nothing.
                    let sources = f.sources.clone();
                    f.db_cats.retain(|(s, _)| sources.contains(s));
                }
                ui.end_row();
                ui.label("Author");
                ui.add(egui::TextEdit::singleline(&mut f.authors).hint_text("Any (commas for several)").desired_width(w));
                ui.end_row();
                ui.label("Category");
                let before = f.cats.clone();
                if multi_select(ui, "filter_cat", w, &mut f.cats, &cats) {
                    // Taking a category out takes its subcategories with it.
                    f.subs.retain(|(c, _)| !before.contains(c) || f.cats.contains(c));
                }
                ui.end_row();
                ui.label("Subcategory");
                multi_select(ui, "filter_sub", w, &mut f.subs, &subs);
                ui.end_row();
                ui.label("Database category");
                multi_select(ui, "filter_dbcat", w, &mut f.db_cats, &db_cats);
                ui.end_row();
                ui.label("License");
                multi_select(ui, "filter_license", w, &mut f.licenses, &licenses);
                ui.end_row();
                ui.label("File type").on_hover_text("SVG: the source publishes a vector version. PNG: an image only");
                multi_select(ui, "filter_format", w, &mut f.formats, &formats);
                ui.end_row();
            });
            if f.active() > 0 {
                ui.add_space(2.0);
                if ui.small_button(format!("{}  Clear filters", ph::FUNNEL_X)).clicked() {
                    *f = Default::default();
                }
            }
        });
        ui.add_space(4.0);
    }

    /// The set filters in a line: "Bioicons, OpenMoji · by Ann · Cells · CC0-1.0".
    fn filter_summary(&self) -> String {
        let f = &self.icons.filter;
        let mut parts: Vec<String> = Vec::new();
        if !f.sources.is_empty() {
            parts.push(f.sources.iter().map(|s| self.source_name(s)).collect::<Vec<_>>().join(", "));
        }
        let authors = f.authors();
        if !authors.is_empty() {
            parts.push(format!("by {}", f.authors.split(',').map(str::trim).filter(|a| !a.is_empty()).collect::<Vec<_>>().join(", ")));
        }
        let places: Vec<&str> = f.cats.iter().filter(|c| !f.subs.iter().any(|(sc, _)| sc == *c)).map(String::as_str).chain(f.subs.iter().map(|(_, s)| s.as_str())).collect();
        if !places.is_empty() {
            parts.push(places.join(", "));
        }
        if !f.db_cats.is_empty() {
            parts.push(f.db_cats.iter().map(|(s, c)| format!("{}: {c}", theme::source_badge(s).0)).collect::<Vec<_>>().join(", "));
        }
        if !f.licenses.is_empty() {
            parts.push(f.licenses.join(", "));
        }
        if !f.formats.is_empty() {
            parts.push(f.formats.join(", "));
        }
        parts.join("  ·  ")
    }

    /// Columns and tile size of the Icons panel's grids: `Settings::icon_columns` tiles filling the width.
    fn icon_grid(&self, ui: &egui::Ui) -> (usize, f32) {
        let cols = self.settings.icon_columns.clamp(1, 5);
        (cols, ((ui.available_width() - 6.0 * (cols - 1) as f32) / cols as f32).floor().max(32.0))
    }

    fn search_box(&mut self, ui: &mut egui::Ui) {
        let r = egui::Frame::new().fill(theme::panel_raised()).corner_radius(8).stroke(Stroke::new(1.0, theme::border())).inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
            // As wide as the filters and results beneath it.
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(ph::MAGNIFYING_GLASS).color(theme::text_dim()));
                // Right to left, so the funnel sits at the right edge and the query takes the rest.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // The funnel opens the filters, and shows how many are set.
                    let n = self.icons.filter.active();
                    let (text, color) = match n {
                        0 => (ph::FUNNEL.to_string(), if self.icons.show_filters { theme::text() } else { theme::text_dim() }),
                        n => (format!("{} {n}", ph::FUNNEL), theme::accent()),
                    };
                    let tip = if self.icons.show_filters { "Hide the filters" } else { "Filter by database, author, category or license" };
                    if ui.add(egui::Button::new(egui::RichText::new(text).color(color)).frame(false)).on_hover_text(tip).clicked() {
                        self.icons.show_filters = !self.icons.show_filters;
                    }
                    ui.add(egui::TextEdit::singleline(&mut self.icons.query).frame(egui::Frame::NONE).desired_width(ui.available_width()))
                })
                .inner
            })
            .inner
        });
        let r = r.inner;
        if r.gained_focus() || (r.changed() && self.icons.mode == IconsMode::Home) {
            self.icons.mode = IconsMode::Search;
        }
        if r.changed() && self.icons.mode != IconsMode::Search {
            self.icons.mode = IconsMode::Search;
        }
    }

    fn icons_panel(&mut self, ui: &mut egui::Ui) {
        if !self.catalog.ok && self.icons.sources.is_empty() {
            egui::Frame::new().fill(theme::panel_raised()).corner_radius(6).inner_margin(8).show(ui, |ui| {
                ui.set_width(ui.available_width());
                match &self.catalog_sync {
                    Some(sync) if sync.running => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            let mb = sync.bytes.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6;
                            ui.label(format!("Downloading the icon catalog... {mb:.1} MB"));
                        });
                    }
                    _ => {
                        ui.label(&self.catalog.status);
                        ui.horizontal(|ui| {
                            if ui.button("Retry").clicked() {
                                self.actions.push(Action::SyncCatalog);
                            }
                            if ui.button("Settings").clicked() {
                                self.open_settings(SettingsTab::Ai);
                            }
                        });
                    }
                }
            });
        }
        match self.icons.mode {
            IconsMode::Home => {
                self.home_row_buttons(ui, &[
                    (ph::MAGNIFYING_GLASS, "Search", IconsMode::Search),
                    (ph::SPARKLE, "Generate with AI", IconsMode::Generate),
                ]);
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                self.home_row_buttons(ui, &[
                    (ph::STAR, "Favorites", IconsMode::Favorites),
                    (ph::BOOKS, "Your library", IconsMode::Library),
                    (ph::UPLOAD_SIMPLE, "Uploads", IconsMode::Uploads),
                ]);
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);
                if row_button(ui, ph::CLOUD_ARROW_UP, "Contribute artwork").on_hover_text("Share your icons and templates with everyone").clicked() {
                    self.open_contribute();
                }
                if row_button(ui, ph::CHAT_CIRCLE_TEXT, "Submit a suggestion").on_hover_text("Ask the BioCartoon team for an icon or template").clicked() {
                    self.actions.push(Action::OpenSuggest(String::new()));
                }
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);
                let sources = self.icons.sources.clone();
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Databases").strong());
                    if !sources.is_empty() {
                        let icons: i64 = sources.iter().map(|s| s.icons).sum();
                        // The upstream collections plus BioCartoon's own (uploads are not one).
                        let dbs = sources.iter().filter(|s| s.source != "user" && s.source != theme::BIOCARTOON_SOURCE).count() + 1;
                        ui.label(egui::RichText::new(format!("{} icons in {dbs} databases", thousands(icons))).small().color(theme::text_dim()));
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    for (source, abbr, name) in self.databases() {
                        let col = theme::source_badge(&source).1;
                        let hover = match sources.iter().find(|s| s.source == source) {
                            Some(s) if source == theme::BIOCARTOON_SOURCE => format!("{name}: {} icons, {} templates", thousands(s.icons), self.template_rows.len()),
                            _ if source == theme::BIOCARTOON_SOURCE => format!("{name}: {} templates", self.template_rows.len()),
                            Some(s) => format!("{name}: {} icons ({} images), {}", thousands(s.icons), thousands(s.count), s.licenses),
                            None => name.clone(),
                        };
                        let b = egui::Button::new(egui::RichText::new(format!("({abbr}) {name}")).size(11.0)).stroke(Stroke::new(1.0, col));
                        if ui.add(b).on_hover_text(hover).clicked() {
                            self.search_database(&source);
                        }
                    }
                });
            }
            IconsMode::Search => {
                self.search_box(ui);
                ui.add_space(4.0);
                if self.icons.show_filters {
                    self.filters_panel(ui);
                } else if self.icons.filter.active() > 0 {
                    // What the closed filters keep, as a reminder; clicking opens them.
                    let summary = self.filter_summary();
                    let l = egui::Label::new(egui::RichText::new(format!("{}  {summary}", ph::FUNNEL)).small().color(theme::text_dim())).sense(Sense::click());
                    if ui.add(l).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        self.icons.show_filters = true;
                    }
                }
                self.results_grid(ui);
            }
            IconsMode::Generate => self.generate_panel(ui),
            IconsMode::Uploads => self.uploads_panel(ui),
            IconsMode::Contribute => self.contribute_panel(ui),
            IconsMode::Suggest => self.suggest_panel(ui),
            IconsMode::Favorites => {
                if self.settings.favorites.is_empty() {
                    ui.label(egui::RichText::new("No favorites yet. Right-click an icon and pick Favorite.").color(theme::text_dim()));
                } else {
                    self.results_grid(ui);
                }
            }
            IconsMode::Library => {
                if self.icons.results.is_empty() && !self.icons.loading && self.matching_templates().is_empty() {
                    ui.label(egui::RichText::new("Nothing here yet. Images you add to your library (generated with AI, or from Uploads) and slides you save as templates appear here.").color(theme::text_dim()));
                }
                #[cfg(target_arch = "wasm32")]
                ui.label(egui::RichText::new("In the browser, images added to your library are kept until the page is reloaded.").small().color(theme::text_dim()));
                self.results_grid(ui);
            }
        }
    }

    /// Full-width buttons that open one of the Icons panel's modes, from its start.
    fn home_row_buttons(&mut self, ui: &mut egui::Ui, rows: &[(&str, &str, IconsMode)]) {
        for (icon, label, mode) in rows {
            if row_button(ui, icon, label).clicked() {
                self.icons.mode = mode.clone();
                self.icons.results.clear();
                self.icons.total = None;
                self.icons.browse_offset = 0;
                match mode {
                    IconsMode::Search => self.request_search(),
                    IconsMode::Library | IconsMode::Favorites => self.request_browse(),
                    _ => {}
                }
            }
        }
    }

    fn results_grid(&mut self, ui: &mut egui::Ui) {
        let templates = self.matching_templates();
        if let Some(e) = &self.icons.error {
            ui.label(egui::RichText::new(e).color(Color32::from_rgb(230, 120, 120)));
        }
        if self.icons.loading && self.icons.results.is_empty() {
            ui.label(egui::RichText::new("Searching...").color(theme::text_dim()));
        }
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Sort").small().color(theme::text_dim()));
            let (sort, options): (&mut IconSort, &[IconSort]) = if self.icons.mode == IconsMode::Search {
                (&mut self.settings.search_sort, &IconSort::SEARCH)
            } else {
                (&mut self.settings.icon_sort, &IconSort::ALL)
            };
            let before = *sort;
            egui::ComboBox::from_id_salt("icon_sort").selected_text(before.label()).show_ui(ui, |ui| {
                for &s in options {
                    ui.selectable_value(sort, s, s.label());
                }
            });
            if *sort != before {
                self.reload_results();
            }
            ui.label(egui::RichText::new("Include").small().color(theme::text_dim()));
            egui::ComboBox::from_id_salt("icon_include").selected_text(self.icons.include.label()).show_ui(ui, |ui| {
                for inc in Include::ALL {
                    ui.selectable_value(&mut self.icons.include, inc, inc.label());
                }
            });
        });
        ui.horizontal(|ui| {
            if let Some(n) = self.icons.total.filter(|_| self.icons.include.icons() && !(self.icons.loading && self.icons.results.is_empty())) {
                let text = format!("{} {}", thousands(n), if n == 1 { "icon" } else { "icons" });
                ui.label(egui::RichText::new(text).small().color(theme::text_dim()));
            }
            if !templates.is_empty() {
                ui.label(egui::RichText::new(format!("{} {}", thousands(templates.len() as i64), if templates.len() == 1 { "template" } else { "templates" })).small().color(theme::text_dim()));
            }
        });
        let icons = self.icons.include.icons();
        if !icons && templates.is_empty() {
            ui.label(egui::RichText::new("No templates here").color(theme::text_dim()));
            return;
        }
        let rows = if icons { self.icons.results.clone() } else { Vec::new() };
        if rows.is_empty() && templates.is_empty() && !self.icons.loading && self.icons.mode == IconsMode::Search && !self.icons.query.trim().is_empty() {
            ui.label(egui::RichText::new("No results").color(theme::text_dim()));
        }
        let sort = if self.icons.mode == IconsMode::Search { self.settings.search_sort } else { self.settings.icon_sort };
        let more = icons && self.icons.more;
        let total = self.icons.total.filter(|_| icons).map_or(rows.len(), |n| n.max(0) as usize);
        let tiles = mix_templates(&rows, &templates, sort, more, total);
        let mut picked: Option<AssetRow> = None;
        let (per_row, size) = self.icon_grid(ui);
        // Reveal thumbnails in order: hold back each one until the visible tiles before it are in.
        let mut hold = false;
        egui::Grid::new("results").spacing([6.0, 6.0]).show(ui, |ui| {
            for (i, tile) in tiles.into_iter().enumerate() {
                let row = match tile {
                    Tile::Template(t) => {
                        self.template_tile(ui, size, t);
                        if (i + 1) % per_row == 0 {
                            ui.end_row();
                        }
                        continue;
                    }
                    Tile::Icon(row) => row,
                };
                let colors = self.icons.colors.get(&row.id).copied().unwrap_or(1);
                let marks = TileMarks {
                    database: self.settings.tile_database,
                    colors: if self.settings.tile_colors { colors } else { 1 },
                    stars: self.settings.tile_stars,
                    fav: self.settings.favorites.contains(&row.id),
                };
                let (r, act) = asset_tile(ui, size, row, row.png_local.as_deref(), &mut self.images, hold, colors, marks);
                match act {
                    Some(TileAct::Remove) => self.actions.push(Action::RemoveUserAsset(row.clone())),
                    Some(TileAct::Favorite) => self.actions.push(Action::ToggleFavorite(row.id.clone())),
                    None if r.clicked() => picked = Some(row.clone()),
                    None => {}
                }
                if ui.is_rect_visible(r.rect) && matches!(self.images.get(&format!("asset:{}", row.png_key)), Some(ImgState::Loading)) {
                    hold = true;
                }
                if (i + 1) % per_row == 0 {
                    ui.end_row();
                }
            }
        });
        if let Some(r) = picked {
            self.actions.push(Action::InsertAsset(r));
        }
        // Load the next page once the end of the list is within a few rows of the bottom.
        let (end, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
        if self.icons.more && end.top() < ui.clip_rect().bottom() + 3.0 * size {
            self.request_more();
        }
        if self.icons.loading && !self.icons.results.is_empty() {
            ui.label(egui::RichText::new("Loading...").small().color(theme::text_dim()));
        }
        if self.icons.mode == IconsMode::Search && !self.icons.more && !self.icons.loading {
            self.not_found_footer(ui);
        }
        // A plain outbound link to run the same search on another icon library's site.
        // let q = self.icons.query.trim();
        // if self.icons.mode == IconsMode::Search && !q.is_empty() {
        //     ui.add_space(6.0);
        //     if ui.button(format!("{}  Search other", ph::ARROW_SQUARE_OUT)).on_hover_text("Opens another icon library in a new tab").clicked() {
        //         ui.ctx().open_url(egui::OpenUrl::new_tab(format!("https://example.com/search?q={}", form_encode(q))));
        //     }
        // }
    }

    /// Under the last search result: where to go when the search came up short.
    fn not_found_footer(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        let dim = |t: &str| egui::RichText::new(t).small().color(theme::text_dim());
        let link = |t: &str| egui::Label::new(egui::RichText::new(t).small().italics().underline().color(theme::text_dim())).sense(Sense::click());
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.label(dim("Didn't find what you were looking for? "));
            if ui.add(link("Upload an image")).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.icons.mode = IconsMode::Uploads;
                self.actions.push(Action::PickUploads);
            }
            ui.label(dim(", "));
            if ui.add(link("Generate an image with AI")).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.icons.mode = IconsMode::Generate;
            }
            ui.label(dim(", or "));
            if ui.add(link("Submit a suggestion")).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.actions.push(Action::OpenSuggest(self.icons.query.trim().to_string()));
            }
            ui.label(dim("."));
        });
    }

    /// Submit a suggestion: ask the BioCartoon team for an icon or template.
    fn suggest_panel(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Submit a suggestion for an icon or template to the BioCartoon team.").color(theme::text_dim()));
        ui.add_space(8.0);
        let w = ui.available_width();
        ui.label(egui::RichText::new("Item type").strong());
        egui::ComboBox::from_id_salt("suggest_kind").width(w).selected_text(if self.suggest.template { "Template" } else { "Icon" }).show_ui(ui, |ui| {
            ui.selectable_value(&mut self.suggest.template, false, "Icon");
            ui.selectable_value(&mut self.suggest.template, true, "Template");
        });
        ui.add_space(6.0);
        ui.label(egui::RichText::new("Title").strong());
        ui.add(egui::TextEdit::singleline(&mut self.suggest.title).hint_text("Required").char_limit(80).desired_width(w));
        ui.add_space(6.0);
        ui.label(egui::RichText::new("Description").strong());
        ui.add(egui::TextEdit::multiline(&mut self.suggest.description).hint_text("Optional").char_limit(1000).desired_rows(4).desired_width(w));
        ui.add_space(6.0);
        ui.label(egui::RichText::new("Name").strong());
        ui.add(egui::TextEdit::singleline(&mut self.suggest.name).hint_text("Optional").char_limit(60).desired_width(w));
        ui.add_space(12.0);
        let can = !self.suggest.title.trim().is_empty() && !self.suggest.busy;
        let label = format!("{}  Submit suggestion", ph::PAPER_PLANE_TILT);
        let b = ui.add_enabled(can, egui::Button::new(egui::RichText::new(label).color(Color32::WHITE).strong()).fill(theme::accent()).min_size(Vec2::new(w, 38.0)));
        if b.on_disabled_hover_text("Give it a title first").clicked() {
            self.actions.push(Action::SubmitSuggestion);
        }
        if self.suggest.busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(egui::RichText::new("Sending...").small().color(theme::text_dim()));
            });
        }
    }

    fn generate_panel(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Describe the icon or illustration you need. The result can be placed on the sheet, kept in your library, or contributed.").color(theme::text_dim()));
        ui.add_space(4.0);
        let r = ui.add(egui::TextEdit::multiline(&mut self.generate.prompt).hint_text("e.g. a flat-style T cell attacking a tumour cell, transparent background").desired_rows(3).desired_width(f32::INFINITY));
        let enter = r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command);
        ui.horizontal(|ui| {
            let can = !self.generate.prompt.trim().is_empty() && !self.generate.busy;
            if ui.add_enabled(can, egui::Button::new(egui::RichText::new(format!("{}  Generate", ph::SPARKLE)).color(Color32::WHITE)).fill(theme::accent())).clicked() || (enter && can) {
                self.actions.push(Action::Generate(self.generate.prompt.clone()));
            }
            ui.label(egui::RichText::new(self.ai_status()).small().color(theme::text_dim()));
        });
        if !self.generate.status.is_empty() {
            ui.label(egui::RichText::new(&self.generate.status).small().color(theme::text_dim()));
        }
        if let Some((img, tex)) = &self.generate.result {
            let w = ui.available_width();
            let (rect, _) = ui.allocate_exact_size(Vec2::new(w, w * 0.8), Sense::hover());
            ui.painter().rect_filled(rect, 8.0, Color32::WHITE);
            let (iw, ih) = (img.width() as f32, img.height() as f32);
            let s = (rect.width() / iw).min(rect.height() / ih) * 0.92;
            let r = Rect::from_center_size(rect.center(), Vec2::new(iw * s, ih * s));
            ui.painter().image(tex.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            // Saved to the history as it arrived; without a file there is nothing to add.
            if let Some(path) = self.generate.result_path.clone() {
                ui.horizontal_wrapped(|ui| {
                    if ui.button(format!("{}  Add to sheet", ph::PLUS)).clicked() {
                        self.actions.push(Action::InsertFile(path.clone().into()));
                    }
                    if ui.button(format!("{}  Add to your library", ph::BOOKS)).on_hover_text("Keep it in Your library, on this computer").clicked() {
                        self.actions.push(Action::GeneratedToLibrary { path: path.clone(), favorite: false });
                    }
                    if ui.button(format!("{}  Contribute", ph::CLOUD_ARROW_UP)).on_hover_text("Share it with everyone through Contribute artwork").clicked() {
                        self.actions.push(Action::ContributeGenerated(path));
                    }
                });
            }
        }
        self.generated_history(ui);
    }

    /// Earlier generated images as small tiles: click to place, right-click for the library.
    fn generated_history(&mut self, ui: &mut egui::Ui) {
        let shown = self.generate.result.as_ref().and(self.generate.result_path.as_deref());
        let history: Vec<GeneratedImage> = self.settings.generated.iter().filter(|g| Some(g.path.as_str()) != shown).cloned().collect();
        if history.is_empty() {
            return;
        }
        ui.add_space(12.0);
        ui.label(egui::RichText::new("Generated earlier").strong());
        ui.add_space(2.0);
        let size = 56.0;
        let per_row = ((ui.available_width() + 6.0) / (size + 6.0)).floor().max(1.0) as usize;
        egui::Grid::new("generated_history").spacing([6.0, 6.0]).show(ui, |ui| {
            for (i, g) in history.iter().enumerate() {
                let state = self.images.file(&g.path);
                let r = tile(ui, size, "", state, None);
                if r.clicked() {
                    self.actions.push(Action::InsertFile(g.path.clone().into()));
                }
                let r = r.on_hover_text(format!("{}\n\nClick to place; right-click for more", g.prompt));
                r.context_menu(|ui| {
                    if ui.button(format!("{}  Add to your library", ph::BOOKS)).clicked() {
                        self.actions.push(Action::GeneratedToLibrary { path: g.path.clone(), favorite: false });
                        ui.close();
                    }
                    if ui.button(format!("{}  Favorite", ph::STAR)).clicked() {
                        self.actions.push(Action::GeneratedToLibrary { path: g.path.clone(), favorite: true });
                        ui.close();
                    }
                    if ui.button(format!("{}  Contribute", ph::CLOUD_ARROW_UP)).clicked() {
                        self.actions.push(Action::ContributeGenerated(g.path.clone()));
                        ui.close();
                    }
                });
                if (i + 1) % per_row == 0 {
                    ui.end_row();
                }
            }
        });
    }

    /// One template's thumbnail in the results grid; clicking it adds its slide.
    fn template_tile(&mut self, ui: &mut egui::Ui, size: f32, row: &TemplateRow) {
        let state = self.images.asset(&row.meta.thumb_key, row.thumb_local.as_deref());
        let (abbr, col, _) = theme::source_badge(theme::BIOCARTOON_SOURCE);
        let r = tile(ui, size, &row.meta.title, state, self.settings.tile_database.then_some((abbr, col, "Template")));
        let info = |ui: &mut egui::Ui| {
            ui.label(egui::RichText::new(&row.meta.title).strong());
            if !row.meta.description.is_empty() {
                ui.label(&row.meta.description);
            }
            if !row.meta.category.is_empty() {
                ui.label(format!("Database category: {}", row.meta.category));
            }
            ui.label(egui::RichText::new(format!("Template  ·  {}  ·  {}", row.meta.author, row.meta.license)).small().color(theme::text_dim()));
        };
        // A long press opens the context menu when there is one, so the info goes there.
        let r = if row.json_local.is_some() { r.on_hover_ui(info) } else { info_ui(r, info) };
        if r.clicked() {
            self.actions.push(Action::UseSavedTemplate(row.clone()));
        }
        if row.json_local.is_some() {
            r.context_menu(|ui| {
                info(ui);
                ui.separator();
                if ui.button("Remove from this computer").clicked() {
                    self.actions.push(Action::RemoveTemplate(row.meta.id.clone()));
                    ui.close();
                }
            });
        }
    }

    /// Thumbnail of a built-in slide format, as shown in the new-slide dropdown.
    fn format_thumb(ui: &mut egui::Ui, i: usize) {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(72.0, 50.0), Sense::hover());
        ui.painter().rect_filled(rect, 3.0, Color32::WHITE);
        let p = ui.painter();
        let g = Color32::from_gray(200);
        let r = rect.shrink(5.0);
        match i {
            1 => { p.rect_filled(Rect::from_min_size(r.min, Vec2::new(r.width(), 6.0)), 1.0, g); p.rect_filled(Rect::from_min_size(r.min + Vec2::new(0.0, 10.0), Vec2::new(r.width(), r.height() - 10.0)), 1.0, Color32::from_gray(235)); }
            2 => { for k in 0..2 { p.rect_filled(Rect::from_min_size(r.min + Vec2::new(k as f32 * (r.width() / 2.0 + 2.0), 0.0), Vec2::new(r.width() / 2.0 - 2.0, r.height())), 1.0, Color32::from_gray(235)); } }
            3 => { for k in 0..3 { p.rect_filled(Rect::from_min_size(r.min + Vec2::new(k as f32 * (r.width() / 3.0 + 1.0), 10.0), Vec2::new(r.width() / 3.0 - 6.0, 20.0)), 1.0, Color32::from_gray(225)); } }
            4 => { for k in 0..4 { p.rect_filled(Rect::from_min_size(r.min + Vec2::new((k % 2) as f32 * (r.width() / 2.0 + 2.0), (k / 2) as f32 * (r.height() / 2.0 + 2.0)), Vec2::new(r.width() / 2.0 - 2.0, r.height() / 2.0 - 2.0)), 1.0, Color32::from_gray(235)); } }
            5 => { p.rect_filled(Rect::from_min_size(r.min, Vec2::new(r.width() * 0.6, r.height())), 1.0, Color32::from_gray(235)); for k in 0..3 { p.rect_filled(Rect::from_min_size(r.min + Vec2::new(r.width() * 0.65, k as f32 * 8.0), Vec2::new(r.width() * 0.35, 6.0)), 0.0, g); } }
            _ => {}
        }
    }

    /// Dropdown listing the built-in slide formats; picking one adds a slide in that format.
    fn slide_formats_menu(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(260.0);
        for (i, (name, desc)) in BioCartoon::template_names().iter().enumerate() {
            let r = egui::Frame::new().fill(theme::panel_raised()).corner_radius(8).inner_margin(8).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    Self::format_thumb(ui, i);
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(*name).strong());
                        ui.label(egui::RichText::new(*desc).small().color(theme::text_dim()));
                    });
                });
            });
            let resp = ui.interact(r.response.rect, ui.id().with(("slide_format", i)), Sense::click());
            if resp.hovered() {
                ui.painter().rect_stroke(r.response.rect, 8.0, Stroke::new(1.0, theme::accent()), StrokeKind::Inside);
            }
            if resp.clicked() {
                self.actions.push(Action::UseTemplate(i));
                ui.close();
            }
        }
    }

    /// Contribute artwork: pick uploads and slides, then send them to the library dialog.
    fn contribute_panel(&mut self, ui: &mut egui::Ui) {
        let full = Vec2::new(ui.available_width(), 32.0);
        ui.label(egui::RichText::new("Icons").strong().size(14.0));
        ui.add_space(2.0);
        if ui.add(egui::Button::new(egui::RichText::new(format!("{}  Upload images", ph::UPLOAD_SIMPLE)).color(Color32::WHITE)).fill(theme::accent()).min_size(full)).clicked() {
            self.actions.push(Action::PickUploads);
        }
        ui.add_space(6.0);
        ui.label(egui::RichText::new("Uploads").strong());
        let uploads = self.settings.uploads.clone();
        if uploads.is_empty() {
            ui.label(egui::RichText::new("No uploads yet.").color(theme::text_dim()));
        }
        let (per_row, size) = self.icon_grid(ui);
        egui::Grid::new("contribute_icons").spacing([6.0, 6.0]).show(ui, |ui| {
            for (i, p) in uploads.iter().enumerate() {
                let name = std::path::Path::new(p).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                let state = self.images.file(p);
                let r = tile(ui, size, &name, state, None);
                let on = self.contribute.icons.contains(p);
                check_corner(ui, r.rect, on, true);
                if r.on_hover_text(&name).clicked() && !self.contribute.icons.remove(p) {
                    self.contribute.icons.insert(p.clone());
                }
                if (i + 1) % per_row == 0 {
                    ui.end_row();
                }
            }
        });
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Your library").strong());
        let library: Vec<AssetRow> = self.icons.results.iter().filter(|r| r.png_local.is_some()).cloned().collect();
        if library.is_empty() {
            let text = if self.icons.loading { "Loading..." } else { "No icons in your library yet." };
            ui.label(egui::RichText::new(text).color(theme::text_dim()));
        }
        egui::Grid::new("contribute_library").spacing([6.0, 6.0]).show(ui, |ui| {
            for (i, row) in library.iter().enumerate() {
                let state = self.images.file(row.png_local.as_deref().unwrap_or_default());
                let r = tile(ui, size, &row.title, state, None);
                let on = self.contribute.library.contains(&row.id);
                check_corner(ui, r.rect, on, true);
                if r.on_hover_text(&row.title).clicked() && !self.contribute.library.remove(&row.id) {
                    self.contribute.library.insert(row.id.clone());
                }
                if (i + 1) % per_row == 0 {
                    ui.end_row();
                }
            }
        });

        ui.add_space(14.0);
        ui.label(egui::RichText::new("Templates").strong().size(14.0));
        ui.add_space(2.0);
        if ui.add(egui::Button::new(egui::RichText::new(format!("{}  Upload templates", ph::UPLOAD_SIMPLE)).color(Color32::WHITE)).fill(theme::accent()).min_size(full))
            .on_hover_text("Add the slides of .biocartoon files to pick from")
            .clicked()
        {
            self.actions.push(Action::PickTemplateFiles);
        }
        ui.add_space(6.0);
        let cell_w = (ui.available_width() - 6.0) / 2.0;
        let page = self.doc.page_rect();
        let bg = c32(self.doc.background);
        // This document's slides, then the uploaded ones: (slide, page, background, uploaded index).
        let mut entries: Vec<(crate::doc::Slide, Rect, Color32, Option<usize>)> = self.doc.slides.iter().map(|s| (s.clone(), page, bg, None)).collect();
        entries.extend(self.contribute.templates.iter().enumerate().map(|(i, t)| (t.slide.clone(), t.page_rect(), c32(t.background), Some(i))));
        let mut col = 0;
        let mut row_start = ui.cursor().min;
        let mut row_h: f32 = 0.0;
        for (k, (slide, page, bg, uploaded)) in entries.iter().enumerate() {
            let slide_no = k.min(self.doc.slides.len());
            let h = cell_w * page.height() / page.width().max(1.0);
            let min = row_start + Vec2::new(col as f32 * (cell_w + 6.0), 0.0);
            let rect = Rect::from_min_size(min, Vec2::new(cell_w, h));
            let label_rect = Rect::from_min_size(rect.left_bottom() + Vec2::new(0.0, 2.0), Vec2::new(cell_w, 14.0));
            let r = ui.interact(rect, ui.id().with(("contribute_slide", k)), Sense::click());
            let painter = ui.painter_at(rect);
            let inner = rect.shrink(3.0);
            let xf = Xform { origin: inner.min, zoom: inner.width() / page.width().max(1.0) };
            draw_slide(&painter, &xf, slide, *page, *bg, &mut self.images);
            let usable = !slide.items.is_empty();
            let on = match uploaded {
                Some(i) => self.contribute.picked_templates.contains(i),
                None => self.contribute.slides.contains(&slide_no),
            };
            let hot = r.hovered() && usable;
            ui.painter().rect_stroke(rect, 4.0, Stroke::new(if on || hot { 2.0 } else { 1.0 }, if on || hot { theme::accent() } else { theme::border() }), StrokeKind::Inside);
            check_corner(ui, rect, on, usable);
            let label = match uploaded {
                Some(_) => format!("Uploaded  {}", slide.name),
                None => format!("{}  {}", slide_no + 1, slide.name),
            };
            ui.painter().with_clip_rect(label_rect).text(label_rect.left_top(), egui::Align2::LEFT_TOP, label, FontId::proportional(10.5), theme::text_dim());
            let r = if usable { r.on_hover_text(&slide.name) } else { r.on_hover_text("This slide is empty") };
            if r.clicked() && usable {
                let set = match uploaded {
                    Some(i) => (&mut self.contribute.picked_templates, *i),
                    None => (&mut self.contribute.slides, slide_no),
                };
                if !set.0.remove(&set.1) {
                    set.0.insert(set.1);
                }
            }
            if let Some(i) = uploaded {
                r.context_menu(|ui| {
                    if ui.button("Remove").clicked() {
                        self.contribute.templates.remove(*i);
                        self.contribute.picked_templates.clear();
                        ui.close();
                    }
                });
            }
            row_h = row_h.max(h + 18.0);
            col += 1;
            if col == 2 {
                col = 0;
                row_start.y += row_h + 6.0;
                row_h = 0.0;
            }
        }
        let used = row_start.y + row_h - ui.cursor().min.y;
        ui.allocate_space(Vec2::new(ui.available_width(), used.max(0.0)));
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Your library").strong());
        let saved: Vec<TemplateRow> = self.template_rows.iter().filter(|r| r.json_local.is_some()).cloned().collect();
        if saved.is_empty() {
            ui.label(egui::RichText::new("No templates in your library yet.").color(theme::text_dim()));
        }
        let (per_row, size) = self.icon_grid(ui);
        egui::Grid::new("contribute_templates").spacing([6.0, 6.0]).show(ui, |ui| {
            for (i, row) in saved.iter().enumerate() {
                let state = self.images.asset(&row.meta.thumb_key, row.thumb_local.as_deref());
                let r = tile(ui, size, &row.meta.title, state, None);
                let on = self.contribute.library_templates.contains(&row.meta.id);
                check_corner(ui, r.rect, on, true);
                if r.on_hover_text(&row.meta.title).clicked() && !self.contribute.library_templates.remove(&row.meta.id) {
                    self.contribute.library_templates.insert(row.meta.id.clone());
                }
                if (i + 1) % per_row == 0 {
                    ui.end_row();
                }
            }
        });

        ui.add_space(16.0);
        let c = &self.contribute;
        let n = c.icons.len() + c.library.len() + c.slides.len() + c.picked_templates.len() + c.library_templates.len();
        let label = if n == 0 { format!("{}  Add to BioCartoon library", ph::CLOUD_ARROW_UP) } else { format!("{}  Add to BioCartoon library ({n})", ph::CLOUD_ARROW_UP) };
        let b = ui.add_enabled(n > 0, egui::Button::new(egui::RichText::new(label).color(Color32::WHITE).strong()).fill(theme::accent()).min_size(Vec2::new(ui.available_width(), 38.0)));
        if b.on_disabled_hover_text("Pick icons or templates above first").clicked() {
            self.actions.push(Action::ContributeSelected);
        }
        ui.label(egui::RichText::new("Each one is reviewed before anyone else sees it.").small().color(theme::text_dim()));
    }

    fn uploads_panel(&mut self, ui: &mut egui::Ui) {
        if ui.add(egui::Button::new(egui::RichText::new(format!("{}  Upload images", ph::UPLOAD_SIMPLE)).color(Color32::WHITE)).fill(theme::accent()).min_size(Vec2::new(ui.available_width(), 32.0))).clicked() {
            self.actions.push(Action::PickUploads);
        }
        ui.label(egui::RichText::new("PNG, JPEG, WebP, GIF, BMP. Click a thumbnail to place it on the sheet.").small().color(theme::text_dim()));
        ui.add_space(6.0);
        let uploads = self.settings.uploads.clone();
        if uploads.is_empty() {
            ui.label(egui::RichText::new("No uploads yet.").color(theme::text_dim()));
        }
        let (per_row, size) = self.icon_grid(ui);
        let mut remove: Option<usize> = None;
        egui::Grid::new("uploads").spacing([6.0, 6.0]).show(ui, |ui| {
            for (i, p) in uploads.iter().enumerate() {
                let name = std::path::Path::new(p).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                let state = self.images.file(p);
                let r = tile(ui, size, &name, state, Some(("UP", theme::source_badge("user").1, "Upload")));
                if r.clicked() {
                    self.actions.push(Action::InsertFile(p.into()));
                }
                let r = r.on_hover_text("Click to place; right-click to add to the icon library");
                r.context_menu(|ui| {
                    if ui.button(format!("{}  Add to icon library...", ph::CLOUD_ARROW_UP)).clicked() {
                        self.actions.push(Action::OpenUploadDialog(UploadTarget::File(p.into())));
                        ui.close();
                    }
                    if ui.button("Remove from uploads").clicked() {
                        remove = Some(i);
                        ui.close();
                    }
                });
                if (i + 1) % per_row == 0 {
                    ui.end_row();
                }
            }
        });
        if let Some(i) = remove {
            self.actions.push(Action::RemoveUpload(i));
        }
    }

    fn lines_panel(&mut self, ui: &mut egui::Ui) {
        let per_row = 3;
        ui.label(egui::RichText::new("Draw").strong());
        ui.horizontal(|ui| {
            let label = format!("{}  {}", ph::SCRIBBLE, if self.drawing { "Drawing..." } else { "Draw" });
            if ui.add(egui::Button::new(label).selected(self.drawing)).on_hover_text("Drag on the canvas to draw a line by hand; Esc to stop").clicked() {
                self.actions.push(Action::ArmDraw(!self.drawing));
            }
            let mut col = c32(self.pen_color);
            if ui.color_edit_button_srgba(&mut col).on_hover_text("Pen color").changed() {
                self.pen_color = c4(col);
            }
            ui.add(egui::DragValue::new(&mut self.pen_width).range(0.5..=40.0).speed(0.5).suffix(" px")).on_hover_text("Pen thickness");
        });
        ui.add_space(6.0);
        for family in LineFamily::ALL {
            ui.label(egui::RichText::new(family.label()).strong());
            egui::Grid::new(("lines", family.label())).spacing([6.0, 6.0]).show(ui, |ui| {
                let kinds = LineKind::ALL.iter().filter(|k| k.family() == family);
                for (i, k) in kinds.enumerate() {
                    let (rect, r) = ui.allocate_exact_size(Vec2::new(TILE, 68.0), Sense::click());
                    ui.painter().rect_filled(rect, 8.0, if r.hovered() { theme::hover() } else { theme::panel_raised() });
                    let (pos, size) = match family {
                        LineFamily::Straight => (Pos2::new(14.0, 26.0), Vec2::new(TILE - 28.0, 0.0)),
                        LineFamily::Bent | LineFamily::Spline | LineFamily::Freehand => (Pos2::new(14.0, 12.0), Vec2::new(TILE - 28.0, 28.0)),
                        LineFamily::Circular => (Pos2::new(TILE / 2.0 - 19.0, 7.0), Vec2::new(38.0, 38.0)),
                    };
                    let it = crate::doc::Item::new(0, "", ItemKind::Line { kind: *k, color: c4(theme::text()), width: 2.0, style: LineStyle::Solid, points: Vec::new() }, pos, size);
                    let xf = Xform { origin: rect.min, zoom: 1.0 };
                    crate::canvas::draw_item(&ui.painter_at(rect), &xf, &it, &mut self.images, None);
                    ui.painter().text(Pos2::new(rect.center().x, rect.max.y - 10.0), egui::Align2::CENTER_CENTER, k.short_label(), FontId::proportional(10.0), theme::text_dim());
                    if r.on_hover_text(k.label()).clicked() {
                        self.actions.push(Action::InsertLine(*k));
                    }
                    if (i + 1) % per_row == 0 {
                        ui.end_row();
                    }
                }
            });
            ui.add_space(6.0);
        }
        ui.label(egui::RichText::new("Color, thickness and solid / dashed / dotted are set in Edit once a line is placed.").small().color(theme::text_dim()));
    }

    fn shapes_panel(&mut self, ui: &mut egui::Ui) {
        let per_row = 3;
        egui::Grid::new("shapes").spacing([6.0, 6.0]).show(ui, |ui| {
            for (i, k) in ShapeKind::ALL.iter().enumerate() {
                let (rect, r) = ui.allocate_exact_size(Vec2::new(TILE, 80.0), Sense::click());
                ui.painter().rect_filled(rect, 8.0, if r.hovered() { theme::hover() } else { theme::panel_raised() });
                let it = crate::doc::Item::new(0, "", ItemKind::Shape { shape: *k, fill: [120, 170, 230, 255], stroke: [200, 220, 255, 255], stroke_width: 1.5 }, Pos2::new(24.0, 8.0), Vec2::new(TILE - 48.0, 48.0));
                let xf = Xform { origin: rect.min, zoom: 1.0 };
                crate::canvas::draw_item(&ui.painter_at(rect), &xf, &it, &mut self.images, None);
                ui.painter().text(Pos2::new(rect.center().x, rect.max.y - 10.0), egui::Align2::CENTER_CENTER, k.label(), FontId::proportional(10.0), theme::text_dim());
                if r.clicked() {
                    self.actions.push(Action::InsertShape(*k));
                }
                if (i + 1) % per_row == 0 {
                    ui.end_row();
                }
            }
        });
    }

    fn table_panel(&mut self, ui: &mut egui::Ui) {
        let id = ui.id().with("table_dims");
        let (mut rows, mut cols) = ui.ctx().data(|d| d.get_temp::<(usize, usize)>(id)).unwrap_or((3, 3));
        ui.horizontal(|ui| {
            ui.label("Rows");
            ui.add(egui::DragValue::new(&mut rows).range(1..=30));
            ui.label("Columns");
            ui.add(egui::DragValue::new(&mut cols).range(1..=12));
        });
        ui.ctx().data_mut(|d| d.insert_temp(id, (rows, cols)));
        // Hover grid picker
        let cell = 22.0;
        let (rect, r) = ui.allocate_exact_size(Vec2::new(cell * 8.0, cell * 6.0), Sense::click());
        let hover = r.hover_pos().map(|p| (((p.y - rect.min.y) / cell).floor() as usize + 1, ((p.x - rect.min.x) / cell).floor() as usize + 1));
        for y in 0..6 {
            for x in 0..8 {
                let c = Rect::from_min_size(rect.min + Vec2::new(x as f32 * cell, y as f32 * cell), Vec2::splat(cell - 2.0));
                let on = hover.is_some_and(|(hr, hc)| y < hr && x < hc);
                ui.painter().rect_filled(c, 2.0, if on { theme::accent() } else { theme::panel_raised() });
            }
        }
        if let Some((hr, hc)) = hover {
            ui.label(format!("{hr} x {hc} table"));
            if r.clicked() {
                self.actions.push(Action::InsertTable(hr, hc));
            }
        } else {
            ui.label(egui::RichText::new("Hover to pick a size, or use the fields above.").small().color(theme::text_dim()));
        }
        if ui.add(egui::Button::new(egui::RichText::new(format!("{}  Insert {rows} x {cols} table", ph::TABLE)).color(Color32::WHITE)).fill(theme::accent())).clicked() {
            self.actions.push(Action::InsertTable(rows, cols));
        }
    }

    fn code_panel(&mut self, ui: &mut egui::Ui) {
        let id = ui.id().with("code_draft");
        let (mut code, mut language, mut theme, mut line_numbers, mut window, mut rounded) = ui.ctx().data(|d| d.get_temp::<(String, CodeLang, CodeTheme, bool, bool, bool)>(id)).unwrap_or_else(|| {
            ("import numpy as np\n\ndef normalize(counts):\n    # counts per million\n    return counts / counts.sum() * 1e6\n".into(), CodeLang::Auto, CodeTheme::Dracula, true, false, false)
        });
        code_options(ui, &code, &mut language, &mut theme);
        ui.horizontal(|ui| {
            ui.checkbox(&mut line_numbers, "Line numbers");
            ui.checkbox(&mut window, "Window");
            ui.checkbox(&mut rounded, "Rounded");
        });
        ui.add_space(4.0);
        code_editor(ui, &mut code, language, theme, 10);
        ui.add_space(6.0);
        if ui.add(egui::Button::new(egui::RichText::new(format!("{}  Insert code block", ph::CODE)).color(Color32::WHITE)).fill(theme::accent())).clicked() {
            self.actions.push(Action::InsertCode { code: code.clone(), language, theme, line_numbers, window, rounded });
        }
        ui.ctx().data_mut(|d| d.insert_temp(id, (code, language, theme, line_numbers, window, rounded)));
    }

    // ---------- Edit (properties) ----------

    /// The Edit panel's Align (to the sheet, or to each other when several are selected) and
    /// Arrange rows.
    fn align_arrange_ui(&mut self, ui: &mut egui::Ui) {
        let w = ui.available_width();
        ui.label(egui::RichText::new("Align").strong());
        segmented(ui, w, 6, |ui, cell| {
            for (k, icon, tip) in [
                (AlignKind::Left, ph::ALIGN_LEFT, "Align left"),
                (AlignKind::CenterH, ph::ALIGN_CENTER_HORIZONTAL, "Align center"),
                (AlignKind::Right, ph::ALIGN_RIGHT, "Align right"),
                (AlignKind::Top, ph::ALIGN_TOP, "Align top"),
                (AlignKind::Middle, ph::ALIGN_CENTER_VERTICAL, "Align middle"),
                (AlignKind::Bottom, ph::ALIGN_BOTTOM, "Align bottom"),
            ] {
                if seg_toggle(ui, cell, icon, false, tip).clicked() {
                    self.actions.push(Action::Align(k));
                }
            }
        });
        seg_captions(ui, w, &["Left", "Center", "Right", "Top", "Middle", "Bottom"]);
        ui.label(egui::RichText::new("Arrange").strong());
        segmented(ui, w, 4, |ui, cell| {
            for (k, icon, tip) in [
                (ArrangeKind::Front, ph::ARROW_LINE_UP, "Bring to front"),
                (ArrangeKind::Forward, ph::ARROW_UP, "Bring forward"),
                (ArrangeKind::Backward, ph::ARROW_DOWN, "Send backward"),
                (ArrangeKind::Back, ph::ARROW_LINE_DOWN, "Send to back"),
            ] {
                if seg_toggle(ui, cell, icon, false, tip).clicked() {
                    self.actions.push(Action::Arrange(k));
                }
            }
        });
        seg_captions(ui, w, &["Bring to front", "Bring forward", "Send backward", "Send to back"]);
    }

    fn edit_panel(&mut self, ui: &mut egui::Ui) {
        if self.selection.is_empty() {
            ui.label(egui::RichText::new("Select something on the canvas to edit it.").small().color(theme::text_dim()));
        }
        if self.selection.is_empty() {
            return;
        }
        let multi = self.selection.len() > 1;
        if multi {
            ui.label(format!("{} items selected", self.selection.len()));
            ui.horizontal(|ui| {
                if ui.button("Group").clicked() { self.actions.push(Action::Group); }
                if ui.button("Ungroup").clicked() { self.actions.push(Action::Ungroup); }
                if ui.button("Delete").clicked() { self.actions.push(Action::Delete); }
            });
            ui.separator();
            self.align_arrange_ui(ui);
            return;
        }
        let id = self.selection[0];
        let Some(it) = self.item(id).cloned() else { return };
        let mut edited = it.clone();
        let mut changed = false;

        // Transform
        ui.label(egui::RichText::new("Transform").strong());
        egui::Grid::new("xform").num_columns(4).spacing([6.0, 4.0]).show(ui, |ui| {
            ui.label("X");
            changed |= ui.add(egui::DragValue::new(&mut edited.pos[0]).speed(1.0).max_decimals(1)).changed();
            ui.label("Y");
            changed |= ui.add(egui::DragValue::new(&mut edited.pos[1]).speed(1.0).max_decimals(1)).changed();
            ui.end_row();
            ui.label("W");
            changed |= ui.add(egui::DragValue::new(&mut edited.size[0]).speed(1.0).max_decimals(1)).changed();
            ui.label("H");
            changed |= ui.add(egui::DragValue::new(&mut edited.size[1]).speed(1.0).max_decimals(1)).changed();
            ui.end_row();
            ui.label("Rotate");
            let mut deg = edited.rotation.to_degrees();
            if ui.add(egui::DragValue::new(&mut deg).speed(1.0).suffix("°").max_decimals(1)).changed() {
                edited.rotation = deg.to_radians();
                changed = true;
            }
            ui.label("Opacity");
            let mut op = edited.opacity * 100.0;
            if ui.add(egui::DragValue::new(&mut op).speed(1.0).range(0.0..=100.0).suffix("%")).changed() {
                edited.opacity = op / 100.0;
                changed = true;
            }
            ui.end_row();
        });
        ui.horizontal_wrapped(|ui| {
            changed |= ui.checkbox(&mut edited.locked, format!("{} Lock", ph::LOCK)).changed();
            changed |= ui.checkbox(&mut edited.flip_h, format!("{} Flip horizontal", ph::FLIP_HORIZONTAL)).changed();
            changed |= ui.checkbox(&mut edited.flip_v, format!("{} Flip vertical", ph::FLIP_VERTICAL)).changed();
        });
        ui.separator();
        self.align_arrange_ui(ui);
        ui.separator();

        match &mut edited.kind {
            ItemKind::Image { source, overlay, .. } => {
                // Presets: other colour variants of the same asset.
                if let ImageSource::Asset { id: aid, .. } = source {
                    self.request_variants(aid);
                    let variants = self.variants.clone();
                    if variants.len() > 1 {
                        ui.label(egui::RichText::new("Style presets").strong());
                        let mut pick: Option<AssetRow> = None;
                        ui.horizontal_wrapped(|ui| {
                            for v in &variants {
                                let state = self.images.asset(&v.png_key, v.png_local.as_deref());
                                let (rect, r) = ui.allocate_exact_size(Vec2::splat(44.0), Sense::click());
                                let on = &v.id == aid;
                                ui.painter().rect(rect, 6.0, Color32::WHITE, Stroke::new(if on { 2.0 } else { 1.0 }, if on { theme::accent() } else { theme::border() }), StrokeKind::Inside);
                                if let ImgState::Ready { tex, size } = state {
                                    let s = (36.0 / size[0] as f32).min(36.0 / size[1] as f32);
                                    let ir = Rect::from_center_size(rect.center(), Vec2::new(size[0] as f32 * s, size[1] as f32 * s));
                                    ui.painter().image(tex.id(), ir, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                                }
                                let r = r.on_hover_text(if v.variant.is_empty() { "default" } else { v.variant.as_str() });
                                if r.clicked() && !on {
                                    pick = Some(v.clone());
                                }
                            }
                        });
                        if let Some(v) = pick {
                            self.replace_target = Some(id);
                            self.actions.push(Action::InsertAsset(v));
                        }
                    }
                }
                ui.label(egui::RichText::new("Style").strong());
                ui.horizontal(|ui| {
                    ui.label("Color overlay");
                    let mut col = Color32::from_rgb(overlay[0], overlay[1], overlay[2]);
                    if ui.color_edit_button_srgba(&mut col).changed() {
                        overlay[0] = col.r();
                        overlay[1] = col.g();
                        overlay[2] = col.b();
                        if overlay[3] == 0 { overlay[3] = 255; }
                        changed = true;
                    }
                    let mut strength = overlay[3] as f32 / 255.0 * 100.0;
                    // Fit the slider between the colour button and Reset (its value box and Reset take ~130px).
                    ui.spacing_mut().slider_width = (ui.available_width() - 130.0).max(40.0);
                    if ui.add(egui::Slider::new(&mut strength, 0.0..=100.0).suffix("%")).changed() {
                        overlay[3] = (strength / 100.0 * 255.0) as u8;
                        changed = true;
                    }
                    if ui.small_button("Reset").clicked() {
                        *overlay = [255, 255, 255, 0];
                        changed = true;
                    }
                });
                ui.separator();
                let b = egui::Button::new(egui::RichText::new(format!("{}  Edit with AI", ph::SPARKLE)).color(Color32::WHITE)).fill(theme::accent()).min_size(Vec2::new(ui.available_width(), 36.0));
                if ui.add(b).on_hover_text("Restyle, remove the background or text, or describe any other change").clicked() {
                    self.actions.push(Action::OpenAiEdit);
                }
                // The credit this image owes, also when it is an edited copy of a library asset.
                if let Some(ImageSource::Asset { id: aid, credit, .. }) = source.origin() {
                    ui.separator();
                    let dim = |t: String| egui::RichText::new(t).small().color(theme::text_dim());
                    if matches!(source, ImageSource::File { .. }) {
                        ui.label(dim("Edited from a library asset. Credit the original:".into()));
                    }
                    let credit = Some(credit.clone()).filter(|c| !c.is_empty()).or_else(|| self.icons.results.iter().chain(self.variants.iter()).find(|r| &r.id == aid).map(|r| r.credit.clone()));
                    match credit {
                        Some(c) => ui.add(egui::Label::new(dim(c)).selectable(true)),
                        None => ui.label(dim(format!("Asset {aid}"))),
                    };
                    let list = crate::credits::list(&crate::credits::collect([self.slide()], self.cur_slide + 1), false);
                    if ui.add_enabled(!list.is_empty(), egui::Button::new(format!("{}  Copy slide credits", ph::COPY)).small()).on_hover_text(list.as_str()).clicked() {
                        ui.ctx().copy_text(list);
                        self.toast("Copied the credits");
                    }
                }
            }
            ItemKind::Text { text, size, color, align, bold, italic, underline, font, line_spacing, valign } => {
                changed |= text_style_ui(ui, text, size, color, align, bold, italic, underline, font, line_spacing, valign);
            }
            ItemKind::Shape { shape, fill, stroke, stroke_width } => {
                ui.label(egui::RichText::new("Shape").strong());
                egui::ComboBox::from_id_salt("shape_kind").selected_text(shape.label()).show_ui(ui, |ui| {
                    for k in ShapeKind::ALL { if ui.selectable_value(shape, k, k.label()).changed() { changed = true; } }
                });
                ui.horizontal(|ui| {
                    ui.label("Fill");
                    let mut col = c32(*fill);
                    if ui.color_edit_button_srgba(&mut col).changed() { *fill = c4(col); changed = true; }
                    ui.label("Stroke");
                    let mut col = c32(*stroke);
                    if ui.color_edit_button_srgba(&mut col).changed() { *stroke = c4(col); changed = true; }
                    changed |= ui.add(egui::DragValue::new(stroke_width).range(0.0..=40.0).speed(0.5)).changed();
                });
            }
            ItemKind::Line { kind, color, width, style, .. } => {
                let was = *kind;
                if was == LineKind::Freehand {
                    // A drawing keeps its path; it can't become (or come back from) another kind.
                    ui.label(egui::RichText::new("Drawing").strong());
                } else {
                    ui.label(egui::RichText::new("Line").strong());
                    egui::ComboBox::from_id_salt("line_kind").selected_text(kind.label()).width(200.0).show_ui(ui, |ui| {
                        for family in LineFamily::ALL {
                            ui.label(egui::RichText::new(family.label()).small().color(theme::text_dim()));
                            for k in LineKind::ALL.iter().filter(|k| k.family() == family) {
                                if ui.selectable_value(kind, *k, k.short_label()).changed() { changed = true; }
                            }
                        }
                    });
                }
                if *kind != was && kind.needs_height() && edited.size[1].abs() < 1.0 {
                    // A flat line has no box for a bend, curve or arc to show in: give it one.
                    edited.size[1] = edited.size[0].abs() * 0.5;
                }
                egui::Grid::new("line_props").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                    ui.label("Color");
                    let mut col = c32(*color);
                    if ui.color_edit_button_srgba(&mut col).changed() { *color = c4(col); changed = true; }
                    ui.end_row();
                    ui.label("Thickness");
                    changed |= ui.add(egui::DragValue::new(width).range(0.5..=40.0).speed(0.5)).changed();
                    ui.end_row();
                    ui.label("Style");
                    ui.horizontal(|ui| {
                        for st in LineStyle::ALL {
                            if ui.selectable_label(*style == st, st.label()).clicked() && *style != st { *style = st; changed = true; }
                        }
                    });
                    ui.end_row();
                });
            }
            ItemKind::Table { rows, cols, cells, header, border, fill, header_fill, text_color, font_size } => {
                ui.label(egui::RichText::new("Table").strong());
                ui.horizontal(|ui| {
                    let (r0, c0) = (*rows, *cols);
                    ui.label("Rows");
                    let rr = ui.add(egui::DragValue::new(rows).range(1..=50)).changed();
                    ui.label("Cols");
                    let rc = ui.add(egui::DragValue::new(cols).range(1..=20)).changed();
                    if rr || rc {
                        let mut nc = vec![String::new(); *rows * *cols];
                        for r in 0..r0.min(*rows) {
                            for c in 0..c0.min(*cols) {
                                nc[r * *cols + c] = cells[r * c0 + c].clone();
                            }
                        }
                        *cells = nc;
                        changed = true;
                    }
                    changed |= ui.checkbox(header, "Header row").changed();
                });
                // Two rows: as one, the colours ran past the panel's edge and were cut off.
                ui.horizontal(|ui| {
                    ui.label("Font");
                    changed |= ui.add(egui::DragValue::new(font_size).range(4.0..=100.0)).changed();
                    ui.label("Text");
                    let mut col = c32(*text_color);
                    if ui.color_edit_button_srgba(&mut col).changed() { *text_color = c4(col); changed = true; }
                });
                ui.horizontal(|ui| {
                    ui.label("Fill");
                    let mut col = c32(*fill);
                    if ui.color_edit_button_srgba(&mut col).changed() { *fill = c4(col); changed = true; }
                    ui.label("Header");
                    let mut col = c32(*header_fill);
                    if ui.color_edit_button_srgba(&mut col).changed() { *header_fill = c4(col); changed = true; }
                    ui.label("Border");
                    let mut col = c32(*border);
                    if ui.color_edit_button_srgba(&mut col).changed() { *border = c4(col); changed = true; }
                });
                ui.label(egui::RichText::new("Double-click a cell on the canvas to type in it; Tab moves to the next cell.").small().color(theme::text_dim()));
            }
            ItemKind::Code { code, language, theme, font_size, line_numbers, window, rounded } => {
                changed |= code_options(ui, code, language, theme);
                ui.horizontal(|ui| {
                    ui.label("Font");
                    changed |= ui.add(egui::DragValue::new(font_size).range(4.0..=72.0)).changed();
                    changed |= ui.checkbox(line_numbers, "Line numbers").changed();
                    changed |= ui.checkbox(window, "Window").changed();
                    changed |= ui.checkbox(rounded, "Rounded").changed();
                });
                changed |= code_editor(ui, code, *language, *theme, 14).changed();
                if ui.button(format!("{}  Fit to code", ph::ARROWS_OUT_SIMPLE)).on_hover_text("Resize the block to show every line").clicked() {
                    let s = crate::code::fit_size(ui.ctx(), code, *font_size, *line_numbers, *window);
                    edited.size = s;
                    changed = true;
                }
            }
        }

        if changed && edited != it {
            self.update_item(&it, edited);
        }
    }

    /// Replace `before` with `edited`, one undo step per burst of edits: only snapshot when the item
    /// was unchanged since the last commit.
    pub(crate) fn update_item(&mut self, before: &Item, edited: Item) {
        let id = before.id;
        let key = self.pending_edits.get(&id).cloned();
        let snapshot = serde_json::to_string(before).unwrap_or_default();
        if key.as_deref() != Some(&snapshot) {
            self.commit();
        }
        self.pending_edits.insert(id, serde_json::to_string(&edited).unwrap_or_default());
        if let Some(slot) = self.item_mut(id) {
            *slot = edited;
        }
    }

    // ---------- Page ----------

    /// The page's size and colour (applied as they are edited), and the grid.
    fn page_panel(&mut self, ui: &mut egui::Ui) {
        let before = (self.doc.width, self.doc.height, self.doc.unit, self.doc.background);
        let unit = self.doc.unit;
        let (mut w, mut h) = (unit.from_px(self.doc.width), unit.from_px(self.doc.height));
        let near = |a: f32, b: f32| (a - b).abs() < 0.005;
        let preset = PRESETS.iter().skip(1).position(|(_, pw, ph_, u)| *u == unit && near(*pw, w) && near(*ph_, h)).map_or(0, |i| i + 1);
        let mut fit = false;

        ui.label(egui::RichText::new("Size").strong());
        egui::ComboBox::from_id_salt("page_preset").selected_text(PRESETS[preset].0).width(ui.available_width()).show_ui(ui, |ui| {
            for (i, (name, pw, ph_, u)) in PRESETS.iter().enumerate().skip(1) {
                if ui.selectable_label(preset == i, *name).clicked() {
                    self.doc.width = u.to_px(*pw);
                    self.doc.height = u.to_px(*ph_);
                    self.doc.unit = *u;
                    fit = true;
                }
            }
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let ar = w / h.max(0.001);
            ui.label("W");
            let rw = ui.add(egui::DragValue::new(&mut w).speed(0.05).range(0.1..=10000.0).max_decimals(2));
            if ui.add(egui::Button::new(if self.page.lock_aspect { ph::LOCK } else { ph::LOCK_OPEN }).frame(false).selected(self.page.lock_aspect)).on_hover_text("Lock aspect ratio").clicked() {
                self.page.lock_aspect = !self.page.lock_aspect;
            }
            ui.label("H");
            let rh = ui.add(egui::DragValue::new(&mut h).speed(0.05).range(0.1..=10000.0).max_decimals(2));
            if self.page.lock_aspect {
                if rw.changed() { h = w / ar; }
                if rh.changed() { w = h * ar; }
            }
            if rw.changed() || rh.changed() {
                self.doc.width = unit.to_px(w).max(48.0);
                self.doc.height = unit.to_px(h).max(48.0);
                fit |= !rw.dragged() && !rh.dragged();
            }
            egui::ComboBox::from_id_salt("page_unit").selected_text(unit.label()).width(56.0).show_ui(ui, |ui| {
                for u in Unit::ALL { ui.selectable_value(&mut self.doc.unit, u, u.label()); }
            });
        });
        ui.add_space(10.0);
        ui.label(egui::RichText::new("Color").strong());
        ui.horizontal(|ui| {
            let mut col = c32(self.doc.background);
            if ui.color_edit_button_srgba(&mut col).changed() {
                self.doc.background = col.to_srgba_unmultiplied();
            }
            ui.label("Page color");
        });

        let after = (self.doc.width, self.doc.height, self.doc.unit, self.doc.background);
        if after != before {
            // One undo step per burst of edits, like the Edit panel: snapshot the state before them.
            if self.page.last_edit != Some(before) {
                (self.doc.width, self.doc.height, self.doc.unit, self.doc.background) = before;
                self.commit();
                (self.doc.width, self.doc.height, self.doc.unit, self.doc.background) = after;
            }
            self.dirty = true;
            self.page.last_edit = Some(after);
            if fit { self.view.fit_requested = true; }
        }

        ui.add_space(14.0);
        ui.label(egui::RichText::new("Grid").strong());
        if action_row(ui, ph::LINE_VERTICAL, "Add vertical guide", ph::PLUS).on_hover_text("Drag it into place; Delete removes it").clicked() {
            self.actions.push(Action::AddGuide(true));
        }
        if action_row(ui, ph::MINUS, "Add horizontal guide", ph::PLUS).on_hover_text("Drag it into place; Delete removes it").clicked() {
            self.actions.push(Action::AddGuide(false));
        }
        if toggle_row(ui, ph::MAGNET, "Snap to guides", self.settings.guide_snap).on_hover_text("Hold Alt (Option) while dragging to place freely").clicked() {
            self.settings.guide_snap = !self.settings.guide_snap;
        }
        let has_guides = !self.doc.guides.is_empty();
        if ui.add_enabled_ui(has_guides, |ui| action_row(ui, ph::TRASH, "Clear all guides", "")).inner.clicked() && has_guides {
            self.actions.push(Action::ClearGuides);
        }
        if toggle_row(ui, ph::GRID_FOUR, "Show gridlines", self.settings.show_grid).clicked() {
            self.settings.show_grid = !self.settings.show_grid;
        }
        if toggle_row(ui, ph::MAGNET, "Snap to grid", self.settings.grid_snap).on_hover_text("Hold Alt (Option) while dragging to place freely").clicked() {
            self.settings.grid_snap = !self.settings.grid_snap;
        }
        ui.add_space(4.0);
        let g = self.settings.grid_size;
        let current = GRID_PRESETS.iter().position(|(_, v, u)| (u.to_px(*v) - g).abs() < 0.01).filter(|_| !self.settings.grid_custom);
        ui.horizontal(|ui| {
            ui.label("Spacing");
            egui::ComboBox::from_id_salt("grid_spacing").selected_text(current.map_or("Custom", |i| GRID_PRESETS[i].0)).width(110.0).show_ui(ui, |ui| {
                for (i, (l, v, u)) in GRID_PRESETS.iter().enumerate() {
                    if ui.selectable_label(current == Some(i), *l).clicked() {
                        self.settings.grid_size = u.to_px(*v);
                        self.settings.grid_custom = false;
                    }
                }
                if ui.selectable_label(current.is_none(), "Custom").clicked() {
                    self.settings.grid_custom = true;
                }
            });
        });
        if current.is_none() {
            ui.horizontal(|ui| {
                let u = self.settings.grid_unit;
                let mut v = u.from_px(self.settings.grid_size);
                if ui.add(egui::DragValue::new(&mut v).speed(0.01).range(0.01..=10000.0).max_decimals(3)).changed() {
                    self.settings.grid_size = u.to_px(v).max(2.0);
                }
                egui::ComboBox::from_id_salt("grid_unit").selected_text(u.label()).width(56.0).show_ui(ui, |ui| {
                    for u in Unit::ALL { ui.selectable_value(&mut self.settings.grid_unit, u, u.label()); }
                });
            });
        }
    }

    // ---------- Slides ----------

    /// The slide list, inside the right panel's Slides tab.
    fn slides_content(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let add = ui.add(egui::Button::new(ph::PLUS).frame(false)).on_hover_text("Add slide");
            if add.clicked() {
                self.actions.push(Action::AddSlide);
            }
            // Small dropdown arrow just off the bottom-right corner of the add button.
            let caret_rect = Rect::from_min_size(add.rect.right_bottom() - Vec2::new(3.0, 11.0), Vec2::new(12.0, 12.0));
            let caret = ui.interact(caret_rect, add.id.with("formats"), Sense::click()).on_hover_text("Add slide with a format");
            ui.painter().text(caret_rect.center(), egui::Align2::CENTER_CENTER, ph::CARET_DOWN, FontId::proportional(10.0), if caret.hovered() { theme::accent() } else { theme::text_dim() });
            egui::Popup::menu(&caret).show(|ui| self.slide_formats_menu(ui));
        });
        ui.separator();
        let n = self.doc.slides.len();
        let page = self.doc.page_rect();
        let bg = c32(self.doc.background);
        let cur = self.cur_slide;
        for i in 0..n {
            let name = self.doc.slides[i].name.clone();
            ui.label(egui::RichText::new(format!("{}  {}", i + 1, name)).small());
            let w = ui.available_width();
            let h = w * page.height() / page.width();
            let (rect, r) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
            let inner = rect.shrink(3.0);
            let zoom = inner.width() / page.width();
            let xf = Xform { origin: inner.min, zoom };
            let painter = ui.painter_at(rect);
            {
                let slide = self.doc.slides[i].clone();
                draw_slide(&painter, &xf, &slide, page, bg, &mut self.images);
            }
            painter.rect_stroke(rect, 4.0, Stroke::new(if i == cur { 2.0 } else { 1.0 }, if i == cur { theme::accent() } else { theme::border() }), StrokeKind::Inside);
            if r.clicked() {
                self.actions.push(Action::GoSlide(i));
            }
            r.context_menu(|ui| {
                if ui.button("Duplicate").clicked() { self.actions.push(Action::DuplicateSlide(i)); ui.close(); }
                if ui.add_enabled(i > 0, egui::Button::new("Move up")).clicked() { self.actions.push(Action::MoveSlide(i, -1)); ui.close(); }
                if ui.add_enabled(i + 1 < n, egui::Button::new("Move down")).clicked() { self.actions.push(Action::MoveSlide(i, 1)); ui.close(); }
                if ui.add_enabled(n > 1, egui::Button::new("Delete")).clicked() { self.actions.push(Action::DeleteSlide(i)); ui.close(); }
                let empty = self.doc.slides[i].items.is_empty();
                let add = ui.add_enabled(!empty, egui::Button::new(format!("{}  Add to BioCartoon library...", ph::CLOUD_ARROW_UP)));
                if add.on_disabled_hover_text("Add something to the slide first").clicked() {
                    self.actions.push(Action::OpenUploadDialog(self.template_target(i)));
                    ui.close();
                }
                ui.separator();
                ui.label("Rename");
                let mut nm = self.doc.slides[i].name.clone();
                if ui.text_edit_singleline(&mut nm).changed() {
                    self.doc.slides[i].name = nm;
                    self.dirty = true;
                }
            });
            ui.add_space(4.0);
        }
    }
}

// ---------- widgets ----------

/// A full-width on/off row: highlighted while on, with its icon crossed out while off.
fn toggle_row(ui: &mut egui::Ui, icon: &str, label: &str, on: bool) -> egui::Response {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), font.size + 18.0), Sense::click());
    let p = ui.painter();
    let fill = if on { theme::accent_dim() } else if resp.hovered() { theme::hover() } else { theme::panel_raised() };
    p.rect_filled(rect, 6.0, fill);
    let (fg, dim) = if on { (theme::on_accent_dim(), theme::on_accent_dim()) } else { (theme::text(), theme::text_dim()) };
    let c = Pos2::new(rect.min.x + 18.0, rect.center().y);
    p.text(c, egui::Align2::CENTER_CENTER, icon, FontId::proportional(16.0), dim);
    if !on {
        let (r, stroke) = (10.0, Stroke::new(1.5, dim));
        p.circle_stroke(c, r, stroke);
        let d = r * std::f32::consts::FRAC_1_SQRT_2;
        p.line_segment([c + Vec2::new(-d, -d), c + Vec2::new(d, d)], stroke);
    }
    p.text(Pos2::new(rect.min.x + 38.0, rect.center().y), egui::Align2::LEFT_CENTER, label, font.clone(), fg);
    p.text(Pos2::new(rect.max.x - 10.0, rect.center().y), egui::Align2::RIGHT_CENTER, if on { "On" } else { "Off" }, font, dim);
    resp
}

/// A row like [`toggle_row`] for a one-shot action, with `right` where the On/Off would be.
fn action_row(ui: &mut egui::Ui, icon: &str, label: &str, right: &str) -> egui::Response {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), font.size + 18.0), Sense::click());
    let p = ui.painter();
    let enabled = ui.is_enabled();
    p.rect_filled(rect, 6.0, if resp.hovered() && enabled { theme::hover() } else { theme::panel_raised() });
    let (fg, dim) = if enabled { (theme::text(), theme::text_dim()) } else { (theme::text_dim(), theme::text_dim().gamma_multiply(0.5)) };
    p.text(Pos2::new(rect.min.x + 18.0, rect.center().y), egui::Align2::CENTER_CENTER, icon, FontId::proportional(16.0), dim);
    p.text(Pos2::new(rect.min.x + 38.0, rect.center().y), egui::Align2::LEFT_CENTER, label, font, fg);
    p.text(Pos2::new(rect.max.x - 10.0, rect.center().y), egui::Align2::RIGHT_CENTER, right, FontId::proportional(14.0), dim);
    resp
}

/// A dropdown of checkboxes, any number of which can be on ("Any" with none). It stays open
/// while they are ticked. Returns true when `chosen` changed.
fn multi_select<T: Clone + PartialEq>(ui: &mut egui::Ui, id: &str, width: f32, chosen: &mut Vec<T>, options: &[(T, String)]) -> bool {
    let text = match chosen.as_slice() {
        [] => "Any".to_string(),
        [one] => options.iter().find(|(v, _)| v == one).map_or_else(|| "1 selected".into(), |(_, l)| l.clone()),
        many => format!("{} selected", many.len()),
    };
    let mut changed = false;
    egui::ComboBox::from_id_salt(id)
        .width(width)
        .selected_text(text)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, |ui| {
            for (v, label) in options {
                let mut on = chosen.contains(v);
                if ui.checkbox(&mut on, label).changed() {
                    if on {
                        chosen.push(v.clone());
                    } else {
                        chosen.retain(|c| c != v);
                    }
                    changed = true;
                }
            }
        });
    changed
}

fn row_button(ui: &mut egui::Ui, icon: &str, label: &str) -> egui::Response {
    let r = egui::Frame::new().fill(theme::panel_raised()).corner_radius(8).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            egui::Frame::new().fill(theme::accent_dim()).corner_radius(6).inner_margin(4).show(ui, |ui| {
                ui.label(egui::RichText::new(icon).size(16.0).color(Color32::WHITE));
            });
            ui.label(label);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(ph::CARET_RIGHT).color(theme::text_dim()));
            });
        });
    });
    let resp = ui.interact(r.response.rect, ui.id().with(label), Sense::click());
    if resp.hovered() {
        ui.painter().rect_stroke(r.response.rect, 8.0, Stroke::new(1.0, theme::accent()), StrokeKind::Inside);
    }
    resp
}

/// `s` as an HTML form query value: "mitochondrial gene" -> "mitochondrial+gene".
#[allow(dead_code)]
fn form_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `n` with thousands separators: 13,076.
fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// A thumbnail tile with optional database badge in the lower-left corner.
fn tile(ui: &mut egui::Ui, size: f32, title: &str, state: &ImgState, badge: Option<(&str, Color32, &str)>) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let p = ui.painter();
    p.rect(rect, 8.0, Color32::WHITE, Stroke::new(if r.hovered() { 2.0 } else { 1.0 }, if r.hovered() { theme::accent() } else { theme::border() }), StrokeKind::Inside);
    match state {
        ImgState::Ready { tex, size } => {
            let inner = rect.shrink(8.0);
            let s = (inner.width() / size[0] as f32).min(inner.height() / size[1] as f32);
            let ir = Rect::from_center_size(inner.center(), Vec2::new(size[0] as f32 * s, size[1] as f32 * s));
            p.image(tex.id(), ir, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }
        ImgState::Loading => {
            p.text(rect.center(), egui::Align2::CENTER_CENTER, "...", FontId::proportional(14.0), Color32::from_gray(160));
        }
        ImgState::Failed(_) => {
            let galley = p.layout(title.to_string(), FontId::proportional(10.0), Color32::from_gray(110), size - 16.0);
            p.galley(Pos2::new(rect.min.x + 8.0, rect.center().y - galley.size().y / 2.0), galley, Color32::from_gray(110));
        }
    }
    if let Some((abbr, col, name)) = badge {
        let b = Rect::from_min_size(rect.left_bottom() + Vec2::new(5.0, -21.0), Vec2::new(22.0, 16.0));
        p.rect_filled(b, 4.0, col);
        p.text(b.center(), egui::Align2::CENTER_CENTER, abbr, FontId::proportional(9.0), Color32::WHITE);
        let _ = name;
    }
    r
}

/// A selection checkbox in the lower-right corner of a tile; faded when it can't be picked.
fn check_corner(ui: &egui::Ui, rect: Rect, on: bool, enabled: bool) {
    let b = Rect::from_min_size(rect.right_bottom() - Vec2::splat(23.0), Vec2::splat(18.0));
    let p = ui.painter();
    if on {
        p.rect_filled(b, 4.0, theme::accent());
        p.text(b.center(), egui::Align2::CENTER_CENTER, ph::CHECK, FontId::proportional(12.0), Color32::WHITE);
    } else {
        let alpha = if enabled { 230 } else { 90 };
        p.rect(b, 4.0, Color32::from_white_alpha(alpha), Stroke::new(1.5, Color32::from_gray(140).gamma_multiply(alpha as f32 / 255.0)), StrokeKind::Inside);
    }
}

/// The star in a tile's upper-right corner, with everyone's count next to it ("☆ 12"): empty,
/// or filled gold once the user starred it (one of their favorites). At 0 stars it shows only
/// while the tile is hovered, so the button can be found. `count` false leaves out the number
/// and shows the star only when filled. Returns the clickable area.
fn star_corner(ui: &egui::Ui, rect: Rect, stars: i64, starred: bool, count: bool, pointer: Option<Pos2>) -> Rect {
    let p = ui.painter();
    // The user's own star counts even before the Worker has answered.
    let stars = stars.max(starred as i64);
    let text = if count && stars > 0 { format_stars(stars) } else { String::new() };
    let galley = p.layout_no_wrap(text, FontId::proportional(10.0), Color32::WHITE);
    let (radius, gap) = (6.0, if galley.size().x > 0.0 { 3.0 } else { 0.0 });
    let size = Vec2::new(4.0 + 2.0 * radius + gap + galley.size().x + 5.0, 18.0);
    let b = Rect::from_min_size(rect.right_top() + Vec2::new(-5.0 - size.x, 5.0), size);
    let hot = pointer.is_some_and(|q| b.contains(q));
    let shown = starred || (count && (stars > 0 || pointer.is_some_and(|q| rect.contains(q))));
    if shown {
        p.rect_filled(b, 4.0, Color32::from_black_alpha(if hot { 200 } else { 140 }));
        if hot {
            p.rect_stroke(b, 4.0, Stroke::new(1.0, Color32::WHITE), StrokeKind::Inside);
        }
        star_shape(p, Pos2::new(b.min.x + 4.0 + radius, b.center().y), radius, starred);
        p.galley(Pos2::new(b.max.x - 5.0 - galley.size().x, b.center().y - galley.size().y / 2.0), galley, Color32::WHITE);
    }
    b
}

/// `r.on_hover_ui(info)`, and the same info in a popup on a long press, since touch screens
/// (the web app on a phone) have no hover. The popup closes on the next tap.
fn info_ui(r: egui::Response, info: impl Fn(&mut egui::Ui)) -> egui::Response {
    egui::Popup::from_response(&r)
        .id(r.id.with("hold_info"))
        .open_memory(r.long_touched().then_some(egui::SetOpenCommand::Bool(true)))
        .width(260.0)
        .show(|ui| info(ui));
    r.on_hover_ui(info)
}

/// One tile of the Icons panel's results: icons and templates share one grid.
enum Tile<'a> {
    Icon(&'a AssetRow),
    Template(&'a TemplateRow),
}

/// The loaded icons with the matching templates mixed in. A to Z and Z to A place each template
/// by its title; the other orders have nothing to rank a template by, so they spread the
/// templates through the icons, at most a few tiles apart. A template whose place is past the
/// icons loaded so far waits for the page that reaches it.
fn mix_templates<'a>(icons: &'a [AssetRow], templates: &'a [TemplateRow], sort: IconSort, more: bool, total: usize) -> Vec<Tile<'a>> {
    let key = |t: &str| t.to_lowercase();
    let mut temps: Vec<&TemplateRow> = templates.iter().collect();
    let mut at: Vec<usize> = match sort {
        IconSort::Alphabetical | IconSort::ReverseAlphabetical => {
            let rev = sort == IconSort::ReverseAlphabetical;
            temps.sort_by_cached_key(|t| key(&t.meta.title));
            if rev {
                temps.reverse();
            }
            temps.iter().map(|t| {
                let k = key(&t.meta.title);
                icons.iter().position(|a| if rev { key(&a.title) < k } else { key(&a.title) > k }).unwrap_or(usize::MAX)
            }).collect()
        }
        _ => {
            let step = (total / (temps.len() + 1)).clamp(1, 6);
            (1..=temps.len()).map(|j| j * step).collect()
        }
    };
    // Past the last loaded icon: shown at the end once there are no more pages.
    for p in &mut at {
        if *p >= icons.len() {
            *p = if more { usize::MAX } else { icons.len() };
        }
    }
    let mut out = Vec::with_capacity(icons.len() + temps.len());
    let mut next = temps.iter().zip(at).peekable();
    for (i, a) in icons.iter().enumerate() {
        while let Some((t, _)) = next.next_if(|(_, p)| *p == i) {
            out.push(Tile::Template(t));
        }
        out.push(Tile::Icon(a));
    }
    out.extend(next.filter(|(_, p)| *p == icons.len()).map(|(t, _)| Tile::Template(t)));
    out
}

/// The gold of a favorite's star.
const GOLD: Color32 = Color32::from_rgb(0xf5, 0xb3, 0x01);

/// What a click on an asset tile asked for, besides placing the asset.
enum TileAct {
    /// Star / unstar it, i.e. Favorite / Unfavorite (the star count, or the right-click menu).
    Favorite,
    /// Remove from library, on one of the user's own uploads.
    Remove,
}

/// What an asset tile shows besides its image (the Settings > General choices, and whether it is
/// one of the user's favorites).
struct TileMarks {
    /// The database's badge, lower left.
    database: bool,
    /// How many colours to mark with a colour wheel, lower right (1: none).
    colors: usize,
    /// The star and everyone's count, upper right.
    stars: bool,
    /// The user starred it (one of their favorites): the star is filled gold.
    fav: bool,
}

/// A catalog asset's tile. `hold` draws it as still loading even if its image is ready. `colors`
/// is how many colours it comes in (the colours are picked in Edit once it is placed); `marks`
/// what to show on it. Clicking the star stars it, which makes it a favorite. Right-click
/// (or a long press) opens its info with Favorite, Copy name and Copy credit.
fn asset_tile(ui: &mut egui::Ui, size: f32, row: &AssetRow, local: Option<&str>, images: &mut crate::assets::ImageCache, hold: bool, colors: usize, marks: TileMarks) -> (egui::Response, Option<TileAct>) {
    let fav = marks.fav;
    let state = images.asset(&row.png_key, local);
    let loading = ImgState::Loading;
    let state = if hold { &loading } else { state };
    let (abbr, col, name) = theme::source_badge(theme::asset_source(&row.source, &row.author));
    let r = tile(ui, size, &row.title, state, marks.database.then_some((abbr, col, name)));
    if marks.colors > 1 {
        color_wheel(ui.painter(), r.rect.right_bottom() - Vec2::new(13.0, 13.0), 7.0);
    }
    let star_rect = star_corner(ui, r.rect, row.stars, fav, marks.stars, r.hover_pos());
    let star_clickable = marks.stars || fav;
    let mut act = (star_clickable && r.clicked() && r.interact_pointer_pos().is_some_and(|q| star_rect.contains(q))).then_some(TileAct::Favorite);
    let info = |ui: &mut egui::Ui| {
        ui.label(egui::RichText::new(&row.title).strong());
        if !row.variant.is_empty() {
            ui.label(format!("Variant: {}", row.variant));
        }
        if colors > 1 {
            ui.label(format!("Comes in {colors} colors: place it, then pick one in Edit."));
        }
        if !row.taxa.is_empty() {
            let taxa: Vec<String> = row.taxa.iter().map(|(c, s)| if s.is_empty() { c.to_string() } else { format!("{c} › {s}") }).collect();
            ui.label(format!("Category: {}", taxa.join(", ")));
        }
        if !row.category.is_empty() {
            ui.label(format!("Database category: {}", row.category));
        }
        let format = if row.svg_key.is_some() { crate::catalog::FORMAT_SVG } else { crate::catalog::FORMAT_PNG };
        let mut last = format!("{name}  ·  {}  ·  {format}", row.license);
        if row.png_width > 0 && row.png_height > 0 {
            last += &format!("  ·  {} × {} px", row.png_width, row.png_height);
        }
        ui.label(last);
        ui.label(egui::RichText::new(&row.credit).small().color(theme::text_dim()));
    };
    // A long press opens the context menu too, so the info goes there for touch screens.
    let r = r.on_hover_ui(info);
    r.context_menu(|ui| {
        info(ui);
        ui.separator();
        if ui.button(format!("{}  {}", ph::STAR, if fav { "Unfavorite" } else { "Favorite" })).clicked() {
            act = Some(TileAct::Favorite);
            ui.close();
        }
        if ui.button(format!("{}  Copy name", ph::COPY)).clicked() {
            ui.ctx().copy_text(row.title.clone());
            ui.close();
        }
        if ui.button(format!("{}  Copy credit", ph::QUOTES)).clicked() {
            ui.ctx().copy_text(row.credit.clone());
            ui.close();
        }
        // The user's own uploads (published ones have no local copy) can be taken out again.
        if row.source == "user" && row.png_local.is_some() {
            ui.separator();
            if ui.button(format!("{}  Remove from library", ph::TRASH)).clicked() {
                act = Some(TileAct::Remove);
                ui.close();
            }
        }
    });
    if let ImgState::Failed(e) = images.get(&format!("asset:{}", row.png_key)).unwrap_or(&ImgState::Loading) {
        let e = e.clone();
        return (r.on_hover_text(e), act);
    }
    (r, act)
}

/// A five-pointed star centred on `c`: filled gold (a favorite), or an empty white outline.
fn star_shape(p: &egui::Painter, c: Pos2, radius: f32, filled: bool) {
    let pts: Vec<Pos2> = (0..10)
        .map(|k| {
            let a = -std::f32::consts::FRAC_PI_2 + k as f32 * std::f32::consts::PI / 5.0;
            c + Vec2::angled(a) * if k % 2 == 0 { radius } else { radius * 0.45 }
        })
        .collect();
    if !filled {
        p.add(egui::Shape::closed_line(pts, Stroke::new(1.2, Color32::WHITE)));
        return;
    }
    // A star is concave but every point sees the centre, so a fan from it fills it.
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(c, GOLD);
    for q in &pts {
        mesh.colored_vertex(*q, GOLD);
    }
    for k in 0..10u32 {
        mesh.add_triangle(0, 1 + k, 1 + (k + 1) % 10);
    }
    p.add(mesh);
}

/// A small colour wheel: the mark of an icon that comes in several colours.
fn color_wheel(p: &egui::Painter, c: Pos2, radius: f32) {
    let tau = std::f32::consts::TAU;
    for (k, rgb) in [[226, 52, 52], [245, 200, 40], [52, 178, 90], [36, 128, 230]].into_iter().enumerate() {
        let from = k as f32 * 0.25 - 0.25;
        let pts = std::iter::once(c).chain((0..=8).map(|j| c + Vec2::angled((from + 0.25 * j as f32 / 8.0) * tau) * radius)).collect();
        p.add(egui::Shape::convex_polygon(pts, Color32::from_rgb(rgb[0], rgb[1], rgb[2]), Stroke::NONE));
    }
    p.circle_stroke(c, radius, Stroke::new(1.5, Color32::WHITE));
}

/// Language and theme pickers for a code block; auto-detect shows what it found.
fn code_options(ui: &mut egui::Ui, code: &str, language: &mut CodeLang, theme: &mut CodeTheme) -> bool {
    let mut changed = false;
    egui::Grid::new(ui.id().with("code_opts")).num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label("Language");
        let shown = if *language == CodeLang::Auto { format!("Auto ({})", language.resolve(code).label()) } else { language.label().to_string() };
        egui::ComboBox::from_id_salt("code_lang").selected_text(shown).width(170.0).show_ui(ui, |ui| {
            for l in CodeLang::ALL {
                changed |= ui.selectable_value(language, l, l.label()).changed();
            }
        });
        ui.end_row();
        ui.label("Theme");
        egui::ComboBox::from_id_salt("code_theme").selected_text(theme.label()).width(170.0).show_ui(ui, |ui| {
            for t in CodeTheme::ALL {
                changed |= ui.selectable_value(theme, t, t.label()).changed();
            }
        });
        ui.end_row();
    });
    changed
}

/// A multiline editor that highlights `code` as it is typed, in the block's theme.
fn code_editor(ui: &mut egui::Ui, code: &mut String, language: CodeLang, theme: CodeTheme, rows: usize) -> egui::Response {
    let pal = theme.palette();
    let col = |c: [u8; 3]| Color32::from_rgb(c[0], c[1], c[2]);
    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap: f32| {
        let text = buf.as_str();
        let font = egui::TextStyle::Monospace.resolve(ui.style());
        let mut job = egui::text::LayoutJob::default();
        for (i, spans) in crate::code::highlight_exact(text, language.resolve(text)).iter().enumerate() {
            if i > 0 {
                job.append("\n", 0.0, egui::TextFormat::simple(font.clone(), col(pal.fg)));
            }
            for (t, k) in spans {
                job.append(t, 0.0, egui::TextFormat::simple(font.clone(), col(pal.color(*k))));
            }
        }
        job.wrap.max_width = wrap;
        ui.fonts_mut(|f| f.layout_job(job))
    };
    egui::Frame::new().fill(col(pal.bg)).corner_radius(6).inner_margin(6).show(ui, |ui| {
        egui::ScrollArea::both().id_salt("code_scroll").max_height(rows as f32 * 16.0).show(ui, |ui| {
            ui.add(egui::TextEdit::multiline(code).code_editor().frame(egui::Frame::NONE).desired_rows(rows).desired_width(f32::INFINITY).text_color(col(pal.fg)).layouter(&mut layouter))
        }).inner
    }).inner
}

/// The Edit panel's Text section: font, size, colour, line spacing, alignment, lists and
/// decoration, one labelled row each.
#[allow(clippy::too_many_arguments)]
fn text_style_ui(
    ui: &mut egui::Ui,
    text: &mut String,
    size: &mut f32,
    color: &mut [u8; 4],
    align: &mut TextAlign,
    bold: &mut bool,
    italic: &mut bool,
    underline: &mut bool,
    font: &mut TextFont,
    line_spacing: &mut f32,
    valign: &mut TextVAlign,
) -> bool {
    let mut changed = false;
    ui.label(egui::RichText::new("Text").strong());
    let w = (ui.available_width() * 0.58).max(160.0);
    egui::Grid::new("text_style").num_columns(2).spacing([8.0, 8.0]).show(ui, |ui| {
        ui.label("Font");
        egui::ComboBox::from_id_salt("text_font").selected_text(font.label()).width(w - 8.0).show_ui(ui, |ui| {
            for f in TextFont::ALL {
                changed |= ui.selectable_value(font, f, f.label()).changed();
            }
        });
        ui.end_row();

        ui.label("Size");
        segmented(ui, w, 3, |ui, cell| {
            if ui.add_sized(cell, egui::Button::new(ph::MINUS).frame(false)).on_hover_text("Smaller").clicked() {
                *size = (*size - 1.0).max(4.0);
                changed = true;
            }
            changed |= ui.add_sized(cell, egui::DragValue::new(size).range(4.0..=400.0).speed(0.25).max_decimals(1)).changed();
            if ui.add_sized(cell, egui::Button::new(ph::PLUS).frame(false)).on_hover_text("Larger").clicked() {
                *size = (*size + 1.0).min(400.0);
                changed = true;
            }
        });
        ui.end_row();

        ui.label("Color");
        ui.horizontal(|ui| {
            let mut col = c32(*color);
            if ui.color_edit_button_srgba(&mut col).changed() {
                *color = c4(col);
                changed = true;
            }
            // Keep what is being typed while the field has focus; show the colour otherwise.
            let id = ui.make_persistent_id("text_hex");
            let current = format!("#{:02X}{:02X}{:02X}", color[0], color[1], color[2]);
            let mut hex = ui.data(|d| d.get_temp::<String>(id)).unwrap_or(current);
            let r = ui.add(egui::TextEdit::singleline(&mut hex).id(id.with("edit")).desired_width(ui.available_width().min(w - 48.0)).char_limit(7));
            if r.changed() {
                if let Some(c) = parse_hex(&hex) {
                    *color = [c[0], c[1], c[2], color[3]];
                    changed = true;
                }
            }
            let focused = r.has_focus();
            ui.data_mut(|d| {
                if focused {
                    d.insert_temp(id, hex);
                } else {
                    d.remove::<String>(id);
                }
            });
        });
        ui.end_row();

        ui.label("Line spacing");
        let label = |v: f32| if v.fract() == 0.0 { format!("{v:.1}") } else { format!("{v}") };
        egui::ComboBox::from_id_salt("text_spacing").selected_text(label(*line_spacing)).width(w - 8.0).show_ui(ui, |ui| {
            for v in [1.0, 1.15, 1.5, 2.0] {
                changed |= ui.selectable_value(line_spacing, v, label(v)).changed();
            }
        });
        ui.end_row();

        ui.label("Horizontal align");
        segmented(ui, w, 4, |ui, cell| {
            for (a, icon, tip) in [(TextAlign::Left, ph::TEXT_ALIGN_LEFT, "Left"), (TextAlign::Center, ph::TEXT_ALIGN_CENTER, "Center"), (TextAlign::Right, ph::TEXT_ALIGN_RIGHT, "Right"), (TextAlign::Justify, ph::TEXT_ALIGN_JUSTIFY, "Justify")] {
                if seg_toggle(ui, cell, icon, *align == a, tip).clicked() && *align != a {
                    *align = a;
                    changed = true;
                }
            }
        });
        ui.end_row();

        ui.label("Vertical align");
        segmented(ui, w, 4, |ui, cell| {
            for (a, icon, tip) in [(TextVAlign::Top, ph::ARROW_LINE_UP, "Top"), (TextVAlign::Middle, ph::ARROWS_IN_LINE_VERTICAL, "Middle"), (TextVAlign::Bottom, ph::ARROW_LINE_DOWN, "Bottom")] {
                if seg_toggle(ui, cell, icon, *valign == a, tip).clicked() && *valign != a {
                    *valign = a;
                    changed = true;
                }
            }
        });
        ui.end_row();

        ui.label("List style");
        segmented(ui, w, 4, |ui, cell| {
            let list = list_style(text);
            for (numbered, icon, tip) in [(false, ph::LIST_BULLETS, "Bulleted list"), (true, ph::LIST_NUMBERS, "Numbered list")] {
                if seg_toggle(ui, cell, icon, list == Some(numbered), tip).clicked() {
                    set_list(text, if list == Some(numbered) { None } else { Some(numbered) });
                    changed = true;
                }
            }
            let indented = text.split('\n').any(|l| l.starts_with(' '));
            if ui.add_enabled_ui(indented, |ui| seg_toggle(ui, cell, ph::TEXT_OUTDENT, false, "Decrease indent")).inner.clicked() {
                indent(text, false);
                changed = true;
            }
            if seg_toggle(ui, cell, ph::TEXT_INDENT, false, "Increase indent").clicked() {
                indent(text, true);
                changed = true;
            }
        });
        ui.end_row();

        ui.label("Decoration");
        segmented(ui, w, 3, |ui, cell| {
            for (v, icon, tip) in [(bold, ph::TEXT_B, "Bold"), (italic, ph::TEXT_ITALIC, "Italic"), (underline, ph::TEXT_UNDERLINE, "Underline")] {
                if seg_toggle(ui, cell, icon, *v, tip).clicked() {
                    *v = !*v;
                    changed = true;
                }
            }
        });
        ui.end_row();
    });
    changed
}

/// A bordered row of `n` equal cells, `width` wide, like a segmented control. `add` gets each
/// cell's size.
fn segmented(ui: &mut egui::Ui, width: f32, n: usize, add: impl FnOnce(&mut egui::Ui, Vec2)) {
    egui::Frame::new().stroke(Stroke::new(1.0, theme::border())).corner_radius(6).inner_margin(2).show(ui, |ui| {
        let inner = width - 6.0;
        ui.set_width(inner);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            add(ui, Vec2::new(inner / n as f32, 26.0));
        });
    });
}

/// Small labels under the cells of a [`segmented`] row, one per cell.
fn seg_captions(ui: &mut egui::Ui, width: f32, labels: &[&str]) {
    let cell = (width - 6.0) / labels.len() as f32;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(3.0);
        for l in labels {
            let text = egui::RichText::new(*l).size(10.0).color(theme::text_dim());
            ui.allocate_ui_with_layout(Vec2::new(cell, 0.0), egui::Layout::top_down(egui::Align::Center), |ui| {
                ui.set_width(cell);
                ui.add(egui::Label::new(text).halign(egui::Align::Center).wrap());
            });
        }
    });
}

/// An icon cell in a [`segmented`] row, highlighted when `on`.
fn seg_toggle(ui: &mut egui::Ui, cell: Vec2, icon: &str, on: bool, tip: &str) -> egui::Response {
    let rt = egui::RichText::new(icon).size(17.0).color(if on { theme::accent() } else { theme::text() });
    ui.add_sized(cell, egui::Button::new(rt).frame(on).selected(on)).on_hover_text(tip)
}

/// `#RRGGBB` (the `#` optional) as rgb.
fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 || !h.is_ascii() {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

const BULLET: &str = "• ";
const INDENT: &str = "    ";

/// A line split into its indent, its list mark (`Some(false)` a bullet, `Some(true)` a number)
/// and the rest.
fn split_mark(line: &str) -> (&str, Option<bool>, &str) {
    let body = line.trim_start_matches(' ');
    let lead = &line[..line.len() - body.len()];
    if let Some(rest) = body.strip_prefix(BULLET) {
        return (lead, Some(false), rest);
    }
    let digits = body.bytes().take_while(u8::is_ascii_digit).count();
    if let Some(rest) = body[digits..].strip_prefix(". ").filter(|_| digits > 0) {
        return (lead, Some(true), rest);
    }
    (lead, None, body)
}

/// The list every non-blank line of `text` is part of: bullets (`false`) or numbers (`true`).
fn list_style(text: &str) -> Option<bool> {
    let mut marks = text.split('\n').filter(|l| !l.trim().is_empty()).map(|l| split_mark(l).1);
    let first = marks.next()??;
    marks.all(|m| m == Some(first)).then_some(first)
}

/// Make every non-blank line of `text` a bullet (`Some(false)`), a numbered item (`Some(true)`),
/// or plain again (`None`).
fn set_list(text: &mut String, list: Option<bool>) {
    if text.trim().is_empty() {
        *text = match list {
            Some(false) => BULLET.into(),
            Some(true) => "1. ".into(),
            None => String::new(),
        };
        return;
    }
    let mut n = 0;
    *text = text
        .split('\n')
        .map(|l| {
            if l.trim().is_empty() {
                return l.to_string();
            }
            let (lead, _, rest) = split_mark(l);
            match list {
                None => format!("{lead}{rest}"),
                Some(false) => format!("{lead}{BULLET}{rest}"),
                Some(true) => {
                    n += 1;
                    format!("{lead}{n}. {rest}")
                }
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
}

/// Indent every non-blank line of `text` one step, or take one step off.
fn indent(text: &mut String, more: bool) {
    *text = text
        .split('\n')
        .map(|l| {
            if more {
                if l.trim().is_empty() { l.to_string() } else { format!("{INDENT}{l}") }
            } else {
                let k = l.len() - l.trim_start_matches(' ').len();
                l[k.min(INDENT.len())..].to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_and_indents() {
        let mut t = "Gene\n\nProtein".to_string();
        set_list(&mut t, Some(true));
        assert_eq!(t, "1. Gene\n\n2. Protein");
        assert_eq!(list_style(&t), Some(true));
        set_list(&mut t, Some(false));
        assert_eq!(t, "• Gene\n\n• Protein");
        indent(&mut t, true);
        assert_eq!(t, "    • Gene\n\n    • Protein");
        assert_eq!(list_style(&t), Some(false));
        indent(&mut t, false);
        set_list(&mut t, None);
        assert_eq!(t, "Gene\n\nProtein");
        assert_eq!(parse_hex("#ff8000"), Some([255, 128, 0]));
        assert_eq!(parse_hex("#ff80"), None);
    }
}
