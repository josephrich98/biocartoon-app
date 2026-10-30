//! The Icons panel on the left (search, AI generation, categories, databases including
//! BioCartoon's own templates, user-generated assets, uploads), and the rail on the right with
//! its panel (Text / Lines / Shapes / Table / Edit / Slides / Page).

use crate::app::{Action, AlignKind, ArrangeKind, BioCartoon, GeneratedImage, IconsMode, Include, RailTab, SearchFilter, SettingsTab, UploadTarget, GRID_PRESETS, PRESETS};
use crate::templates::TemplateRow;
use crate::assets::ImgState;
use crate::canvas::{draw_slide, tool_button, Xform};
use crate::catalog::{AssetRow, Facets, IconSort, LIBRARY_SOURCE};
use biocartoon_catalog::syntax::{self, contains, Expr, Field};
use crate::doc::{c32, c4, CodeLang, CodeTheme, ImageSource, Item, ItemKind, LayerStyle, LineFamily, LineKind, LineStyle, ShapeKind, TextAlign, TextFlag, TextFont, TextVAlign, Unit};
use crate::stars::format_stars;
use crate::theme;
use egui::{Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use egui_phosphor::regular as ph;

const PANEL_W: f32 = 340.0;
/// How narrow and wide the side panels can be dragged.
const PANEL_MIN_W: f32 = 260.0;
const PANEL_MAX_W: f32 = 720.0;
const TILE: f32 = 96.0;
/// Most layers an SVG offers from its own colours (when the catalog names none), the most used.
const MAX_COLOR_LAYERS: usize = 8;
/// Most layers an upload keeps, so the Layers editor stays short for everyone who uses it.
pub const MAX_LAYERS: usize = 20;

/// The Slides panel starts narrower, and can be dragged narrower, than the others.
const SLIDES_W: f32 = 186.0;
const SLIDES_MIN_W: f32 = 150.0;

impl BioCartoon {
    /// The rail at the right edge: one button per [`RailTab`].
    pub fn rail_ui(&mut self, ui: &mut egui::Ui) {
        let frame = egui::Frame::new().fill(theme::bg()).inner_margin(egui::Margin::symmetric(4, 8));
        egui::Panel::right("rail").exact_size(60.0 * theme::text_scale()).frame(frame).show_separator_line(false).show(ui, |ui| {
            // Scrolls when the window is too short for every button (a phone, or large text).
            egui::ScrollArea::vertical().auto_shrink([false, false]).scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                for t in RailTab::ALL {
                    if matches!(t, RailTab::Text | RailTab::Comments | RailTab::Slides) {
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
            }));
        });
    }

    /// With the Icons panel hidden, a drag in from the window's left edge shows it again; with the
    /// right panel closed, a drag in from the right edge reopens the tab it showed last. Call after
    /// the panels, before the canvas, so the canvas doesn't take the drag.
    pub fn edge_swipes(&mut self, ui: &mut egui::Ui) {
        const STRIP: f32 = 6.0;
        const DISTANCE: f32 = 30.0;
        let ctx = ui.ctx().clone();
        let area = ui.available_rect_before_wrap();
        let screen = ctx.content_rect();
        let strip = |side: &str, rect: Rect, inward: f32| -> bool {
            let mut opened = false;
            // Sized exactly and left where it is: a free-sized Area kept on screen grows across the
            // window on a phone and takes every tap meant for the panels under it.
            let area = egui::Area::new(egui::Id::new(("edge_swipe", side))).fixed_pos(rect.min).constrain(false).order(egui::Order::Foreground);
            area.show(&ctx, |ui| {
                let (_, r) = ui.allocate_exact_size(rect.size(), Sense::drag());
                if r.hovered() || r.dragged() {
                    ui.ctx().set_cursor_icon(if inward > 0.0 { egui::CursorIcon::ResizeEast } else { egui::CursorIcon::ResizeWest });
                }
                if r.dragged() {
                    let moved = ui.input(|i| i.pointer.press_origin().zip(i.pointer.latest_pos()).map(|(a, b)| (b.x - a.x) * inward));
                    opened = moved.is_some_and(|d| d > DISTANCE);
                }
            });
            opened
        };
        let rows = area.y_range();
        if !self.settings.show_icons && strip("left", Rect::from_x_y_ranges(screen.left()..=screen.left() + STRIP, rows), 1.0) {
            self.settings.show_icons = true;
        }
        if self.rail.is_none() && strip("right", Rect::from_x_y_ranges(screen.right() - STRIP..=screen.right(), rows), -1.0) {
            self.actions.push(Action::SetRail(Some(self.last_rail.unwrap_or(RailTab::Edit))));
        }
    }

    /// The Icons panel, on the left.
    pub fn left_panel(&mut self, ui: &mut egui::Ui) {
        let frame = egui::Frame::new().fill(theme::bg()).inner_margin(egui::Margin { left: 8, right: 0, top: 36, bottom: 8 });
        // Resizable by dragging its right edge; egui remembers the width.
        // How far the results are scrolled, and how tall their view is, for the back-to-top button.
        let (mut scrolled, mut view_h) = (0.0, f32::INFINITY);
        let panel = egui::Panel::left("leftpanel").resizable(true).default_size(PANEL_W + 8.0).size_range(PANEL_MIN_W..=PANEL_MAX_W).frame(frame).show_separator_line(false).show(ui, |ui| {
            egui::Frame::new().fill(theme::panel()).corner_radius(10).inner_margin(egui::Margin::same(12)).stroke(Stroke::new(1.0, theme::border())).show(ui, |ui| {
                ui.set_min_height(ui.available_height());
                ui.set_width(ui.available_width());
                let (title, back) = match self.icons.mode {
                    IconsMode::Home => ("Artwork".to_string(), false),
                    IconsMode::Search => ("Search artwork".into(), true),
                    IconsMode::Generate => ("Generate with AI".into(), true),
                    IconsMode::Uploads => ("Uploads".into(), true),
                    IconsMode::Contribute if self.contribute.share => ("Add to BioCartoon library".into(), true),
                    IconsMode::Contribute => ("Add to your library".into(), true),
                    IconsMode::Suggest => ("Submit an idea".into(), true),
                    IconsMode::Favorites => ("Favorites".into(), true),
                    IconsMode::Library => ("Your library".into(), true),
                };
                ui.horizontal(|ui| {
                    if back && ui.add(egui::Button::new(ph::ARROW_LEFT).frame(false)).clicked() {
                        self.icons_back();
                    }
                    // Tapping the title goes back to the top, as tapping the status bar does on iOS.
                    if ui.add(egui::Label::new(egui::RichText::new(title).strong().size(15.0 * theme::text_scale())).sense(Sense::click())).clicked() {
                        self.icons.scroll_to = Some(egui::Align::Min);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(egui::Button::new(ph::X).frame(false)).on_hover_text("Hide the Artwork panel").clicked() {
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
                // The search box stays put above the results, so a new search is always at hand.
                if self.icons.mode == IconsMode::Search {
                    self.search_box(ui);
                    ui.add_space(4.0);
                }
                let mine = match self.icons.mode {
                    IconsMode::Favorites if !self.settings.favorites.is_empty() => Some("Search favorites"),
                    IconsMode::Library => Some("Search your library"),
                    _ => None,
                };
                if let Some(hint) = mine {
                    self.mine_search_box(ui, hint);
                    ui.add_space(4.0);
                }
                let mut scroll = egui::ScrollArea::vertical().auto_shrink([false, false]).max_width(ui.available_width());
                let to = self.icons.scroll_to.take();
                if to == Some(egui::Align::Min) {
                    scroll = scroll.vertical_scroll_offset(0.0);
                }
                let out = scroll.show(ui, |ui| {
                    self.icons_panel(ui);
                    if to == Some(egui::Align::Max) {
                        ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                    }
                });
                (scrolled, view_h) = (out.state.offset.y, out.inner_rect.height());
            })
            .response
            .rect
        });
        self.back_to_top(ui.ctx(), panel.inner, scrolled > view_h);
    }

    /// A circled up arrow that appears once the Icons panel has scrolled down a screen, and goes
    /// back to the top. It sits just right of the panel, so it covers no icons, or in the panel's
    /// bottom right corner when the panel fills the screen (on phones).
    fn back_to_top(&mut self, ctx: &egui::Context, panel: Rect, show: bool) {
        let t = ctx.animate_bool(egui::Id::new("icons_back_to_top"), show);
        if t == 0.0 {
            return;
        }
        const D: f32 = 30.0;
        let y = panel.bottom() - 10.0 - D;
        let x = if panel.right() + 8.0 + D + 8.0 <= ctx.content_rect().right() { panel.right() + 8.0 } else { panel.right() - 18.0 - D };
        egui::Area::new(egui::Id::new("icons_back_to_top")).fixed_pos(Pos2::new(x, y)).order(egui::Order::Foreground).show(ctx, |ui| {
            let (rect, r) = ui.allocate_exact_size(Vec2::splat(D), Sense::click());
            let r = r.on_hover_text("Back to the top (⌘↑)");
            let stroke = if r.hovered() { theme::accent() } else { theme::border() };
            let p = ui.painter();
            p.circle(rect.center(), D / 2.0, theme::panel_raised().gamma_multiply(t), Stroke::new(1.0, stroke.gamma_multiply(t)));
            p.text(rect.center(), egui::Align2::CENTER_CENTER, ph::ARROW_UP, FontId::proportional(16.0), theme::text().gamma_multiply(t));
            if r.clicked() {
                self.icons.scroll_to = Some(egui::Align::Min);
            }
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
                    ui.label(egui::RichText::new(title).strong().size(15.0 * theme::text_scale()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(egui::Button::new(ph::X).frame(false)).clicked() {
                            self.actions.push(Action::SetRail(None));
                        }
                    });
                });
                ui.add_space(6.0);
                // The body gets exactly the room left and is clipped to it. A vertical ScrollArea
                // grows to fit a row wider than it, and the panel keeps whatever width it ends up
                // with, so a row sized from the panel's width could push it a little wider every
                // frame: the panel kept widening on its own after a resize.
                let body = ui.available_rect_before_wrap();
                ui.advance_cursor_after_rect(body);
                let mut ui = ui.new_child(egui::UiBuilder::new().id_salt("panel_body").max_rect(body));
                // A little slack for focus outlines at the edges.
                ui.shrink_clip_rect(body.expand(4.0));
                egui::ScrollArea::vertical().auto_shrink([false, false]).max_width(body.width()).show(&mut ui, |ui| match tab {
                    // Text has no panel: its rail button arms placing a text box on the canvas.
                    RailTab::Text => {}
                    RailTab::Lines => self.lines_panel(ui),
                    RailTab::Shapes => self.shapes_panel(ui),
                    RailTab::Table => self.table_panel(ui),
                    RailTab::Code => self.code_panel(ui),
                    RailTab::Comments => self.comments_panel(ui),
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
        if source == LIBRARY_SOURCE {
            return "Your library".into();
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
        self.templates_matching(false)
    }

    /// `matching_templates`, but with `any_source` whichever databases are picked (for the
    /// database filter's count, which like the icons' counts leaves its own pick out).
    fn templates_matching(&self, any_source: bool) -> Vec<TemplateRow> {
        if !self.icons.include.templates() {
            return Vec::new();
        }
        let rows = self.template_rows.iter();
        match self.icons.mode {
            // Favorites: the starred ones, under Favorites' box and filters.
            IconsMode::Search | IconsMode::Favorites => {
                let fav = self.icons.mode == IconsMode::Favorites;
                let f = if fav { &self.icons.mine_filter } else { &self.icons.filter };
                // Templates are slides, not image files, so a file type leaves them out.
                if !f.formats.is_empty() {
                    return Vec::new();
                }
                let eq = |a: &str, b: &str| a.trim().eq_ignore_ascii_case(b.trim());
                let q = if fav { self.icons.mine_query.trim() } else { self.icons.query.trim() };
                let authors = f.authors();
                let tags = f.tags();
                // A template's category is free text: it matches a picked category or subcategory by name.
                let places: Vec<&str> = f.cats.iter().map(String::as_str).chain(f.subs.iter().map(|(_, s)| s.as_str())).collect();
                // Each is in BioCartoon or User-generated, and the saved ones in Your library too:
                // another database's filter leaves it out.
                rows.filter(|r| (!fav && !f.favorites) || self.settings.favorites.contains(&r.meta.id))
                    .filter(|r| !f.downloads || self.offline.template_ids.contains(&r.meta.id))
                    .filter(|r| any_source || f.sources.is_empty() || f.sources.iter().any(|s| s == r.source() || (s == LIBRARY_SOURCE && r.json_local.is_some())))
                    .filter(|r| template_match(r, q))
                    .filter(|r| authors.is_empty() || authors.iter().any(|a| r.meta.author.to_lowercase().contains(a)))
                    .filter(|r| tags.is_empty() || tags.iter().any(|t| r.meta.tags.iter().any(|x| eq(x, t))))
                    .filter(|r| f.licenses.is_empty() || f.licenses.iter().any(|l| eq(l, &r.meta.license)))
                    .filter(|r| places.is_empty() || places.iter().any(|p| eq(p, &r.meta.category)))
                    // Their categories are their database's own categories.
                    .filter(|r| f.db_cats.is_empty() || f.db_cats.iter().any(|(s, c)| s == r.source() && eq(c, &r.meta.category)))
                    .cloned()
                    .collect()
            }
            // The user's own: templates saved on this computer, under the library's search box.
            IconsMode::Library => {
                let f = &self.icons.mine_filter;
                // Slides, not image files: a file type leaves them out.
                if !f.formats.is_empty() {
                    return Vec::new();
                }
                let eq = |a: &str, b: &str| a.trim().eq_ignore_ascii_case(b.trim());
                let places: Vec<&str> = f.cats.iter().map(String::as_str).chain(f.subs.iter().map(|(_, s)| s.as_str())).collect();
                let q = self.icons.mine_query.trim();
                rows.filter(|r| r.json_local.is_some())
                    .filter(|r| places.is_empty() || places.iter().any(|p| eq(p, &r.meta.category)))
                    .filter(|r| template_match(r, q))
                    .cloned()
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    /// Back to the Icons panel's start.
    fn icons_back(&mut self) {
        let i = &mut self.icons;
        i.results.clear();
        i.total = None;
        i.browse_offset = 0;
        i.mine_query.clear();
        i.mine_filter = SearchFilter::default();
        i.mine_show_filters = false;
        i.mode = IconsMode::Home;
    }

    /// The filters under the search box: database, author, category, subcategory, license, file type and tags.
    /// The search re-runs once one changes (see `last_sent_filter`). With `mine`, Favorites' or
    /// Your library's filters instead, over what is loaded (Your library's just category,
    /// subcategory and file type: its icons are all the user's own).
    fn filters_panel(&mut self, ui: &mut egui::Ui, mine: bool) {
        // Each dropdown lists the choices that find icons under the query and the other filters, with
        // how many; the picked ones stay listed so they can be taken out. Until the counts arrive, all of them.
        let facets = if mine {
            Some(local_facets(&self.icons.results, self.icons.mine_query.trim(), &self.icons.mine_filter))
        } else {
            if self.icons.facets_for.as_ref().is_none_or(|(q, f)| q != self.icons.query.trim() || *f != self.icons.filter) {
                self.request_facets();
            }
            self.icons.facets.clone()
        };
        let all = !(mine && self.icons.mode == IconsMode::Library);
        let label = |name: &str, n: Option<i64>| match n {
            Some(n) => format!("{name} ({})", thousands(n)),
            None => name.to_string(),
        };
        let f = if mine { &self.icons.mine_filter } else { &self.icons.filter };
        let in_facet = |list: &[(String, i64)], v: &str| facets.as_ref().map(|_| list.iter().find(|(k, _)| k == v).map_or(0, |(_, n)| *n));
        // The templates under the query and the other filters (Favorites' counts are of its icons).
        let templates = if mine { Vec::new() } else { self.templates_matching(true) };
        // Your library last: its icons and templates are also in the database they are listed under.
        let library = (LIBRARY_SOURCE.to_string(), "", "Your library".to_string());
        let dbs: Vec<(String, String)> = self
            .databases()
            .into_iter()
            .chain(std::iter::once(library))
            .filter_map(|(s, _, name)| {
                let name = if s == "user" { "Community and user-generated".to_string() } else { name };
                let n = in_facet(facets.as_ref().map_or(&[][..], |fc| &fc.sources), &s);
                let in_db = |r: &TemplateRow| if s == LIBRARY_SOURCE { r.json_local.is_some() } else { r.source() == s };
                let t = templates.iter().filter(|r| in_db(r)).count() as i64;
                // A database's templates are in it even when none of its icons are.
                let keep = n != Some(0) || f.sources.contains(&s) || (!mine && self.template_rows.iter().any(in_db));
                // "BioCartoon (1 icon, 6 templates)": templates only when there are some.
                let parts: Vec<String> =
                    n.map(|n| counted(n, "icon")).into_iter().chain((t > 0).then(|| counted(t, "template"))).collect();
                let l = if parts.is_empty() { name } else { format!("{name} ({})", parts.join(", ")) };
                keep.then(|| (s, l))
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
        // and User-generated's include their templates' categories.
        let mut own: Vec<(String, String, Option<i64>)> = facets.as_ref().map_or(Vec::new(), |fc| fc.db_cats.iter().map(|(s, c, n)| (s.clone(), c.clone(), Some(*n))).collect());
        // (Favorites' are of its icons.)
        if !mine {
            let picked = |src: &str| f.sources.is_empty() || f.sources.iter().any(|s| s == src);
            for (src, c) in self.template_rows.iter().map(|r| (r.source(), r.meta.category.trim())).filter(|(src, c)| !c.is_empty() && picked(src)) {
                match own.iter_mut().find(|(s, k, _)| s == src && k.eq_ignore_ascii_case(c)) {
                    Some((_, _, n)) => *n = Some(n.unwrap_or(0) + 1),
                    None => own.push((src.into(), c.to_string(), Some(1))),
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
        let f = if mine { &mut self.icons.mine_filter } else { &mut self.icons.filter };
        egui::Frame::new().fill(theme::panel_raised()).corner_radius(8).inner_margin(8).show(ui, |ui| {
            ui.set_width(ui.available_width());
            // The second column takes what the labels leave; any wider and the grid would push the
            // panel out (it grows to fit), a little more each frame.
            let font = egui::TextStyle::Body.resolve(ui.style());
            let labels: &[&str] = if all { &["Database", "Author", "Category", "Subcategory", "Database category", "License", "File type", "Tags"] } else { &["Category", "Subcategory", "File type"] };
            let label_w = labels.iter().map(|l| ui.painter().layout_no_wrap(l.to_string(), font.clone(), Color32::WHITE).size().x).fold(0.0, f32::max);
            let w = (ui.available_width() - label_w - 8.0).max(60.0);
            egui::Grid::new("search_filters").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                if all {
                    ui.label("Database");
                    if multi_select(ui, "filter_db", w, &mut f.sources, &dbs) && !f.sources.is_empty() && !f.sources.iter().any(|s| s == LIBRARY_SOURCE) {
                        // Another database's own categories would find nothing (Your library's
                        // icons are in any of them).
                        let sources = f.sources.clone();
                        f.db_cats.retain(|(s, _)| sources.contains(s));
                    }
                    ui.end_row();
                    ui.label("Author");
                    ui.add(egui::TextEdit::singleline(&mut f.authors).hint_text("Any (commas for several)").desired_width(w));
                    ui.end_row();
                }
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
                if all {
                    ui.label("Database category");
                    multi_select(ui, "filter_dbcat", w, &mut f.db_cats, &db_cats);
                    ui.end_row();
                    ui.label("License");
                    multi_select(ui, "filter_license", w, &mut f.licenses, &licenses);
                    ui.end_row();
                }
                ui.label("File type").on_hover_text("SVG: the source publishes a vector version. PNG: an image only");
                multi_select(ui, "filter_format", w, &mut f.formats, &formats);
                ui.end_row();
                if all {
                    ui.label("Tags").on_hover_text("Icons and templates with one of these as a whole tag, any case");
                    ui.add(egui::TextEdit::singleline(&mut f.tags).hint_text("Any (commas for several)").desired_width(w));
                    ui.end_row();
                }
                // Favorites' own filters are all favorites already.
                if !mine {
                    // Outlined even when off so they read as buttons; a narrow panel wraps the second.
                    let toggle = |ui: &mut egui::Ui, on: &mut bool, text: String, tip: &str| {
                        let b = egui::Button::new(text).selected(*on).stroke(Stroke::new(1.0, theme::border())).corner_radius(6);
                        if ui.add(b).on_hover_text(tip).clicked() {
                            *on = !*on;
                        }
                    };
                    let fav = format!("{}  Favorites", ph::STAR);
                    let dl = format!("{}  Downloads", ph::DOWNLOAD_SIMPLE);
                    // horizontal_wrapped leaves a gap above the buttons once they wrap, so measure and
                    // pick a row or a stack.
                    let pad = ui.spacing().button_padding.x * 2.0;
                    // The browser keeps no offline folder, so nothing in it is downloaded.
                    let web = cfg!(target_arch = "wasm32");
                    let shown: &[&String] = if web { &[&fav] } else { &[&fav, &dl] };
                    let sizes: Vec<egui::Vec2> = shown.iter().map(|t| ui.painter().layout_no_wrap(t.to_string(), font.clone(), Color32::WHITE).size()).collect();
                    let both = sizes.iter().map(|s| s.x + pad).sum::<f32>() + ui.spacing().item_spacing.x;
                    if both <= w {
                        ui.label("Show only");
                    } else {
                        // The grid centers a cell in its row; keep the label level with the first button.
                        let button_h = (sizes[0].y + ui.spacing().button_padding.y * 2.0).max(ui.spacing().interact_size.y);
                        ui.vertical(|ui| {
                            ui.set_min_height(button_h * sizes.len() as f32 + 4.0);
                            ui.allocate_ui_with_layout(egui::vec2(label_w, button_h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                ui.label("Show only");
                            });
                        });
                    }
                    let buttons = |ui: &mut egui::Ui| {
                        toggle(ui, &mut f.favorites, fav, "Only icons and templates you starred");
                        if !web {
                            toggle(ui, &mut f.downloads, dl, "Only icons and templates downloaded for offline use (Settings > Offline, or right-click > Download)");
                        }
                    };
                    if both <= w {
                        ui.horizontal(buttons);
                    } else {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 4.0;
                            buttons(ui);
                        });
                    }
                    ui.end_row();
                }
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

    /// The set filters (the search's, or with `mine` Favorites' or Your library's) in a line:
    /// "Bioicons, OpenMoji · by Ann · Cells · CC0-1.0".
    fn filter_summary(&self, mine: bool) -> String {
        let f = if mine { &self.icons.mine_filter } else { &self.icons.filter };
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
        let tags = f.tags();
        if !tags.is_empty() {
            parts.push(format!("tagged {}", tags.join(", ")));
        }
        if f.favorites {
            parts.push("Favorites".into());
        }
        if f.downloads {
            parts.push("Downloads".into());
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
                    let tip = if self.icons.show_filters { "Hide the filters" } else { "Filter by database, author, category, license or tags" };
                    if ui.add(egui::Button::new(egui::RichText::new(text).color(color)).frame(false)).on_hover_text(tip).clicked() {
                        self.icons.show_filters = !self.icons.show_filters;
                    }
                    search_help(ui);
                    ui.add(egui::TextEdit::singleline(&mut self.icons.query).frame(egui::Frame::NONE).desired_width(ui.available_width()))
                })
                .inner
            })
            .inner
        });
        let r = r.inner;
        if r.changed() {
            self.icons.typed_at = ui.input(|i| i.time);
        }
        // Enter searches without waiting for the pause.
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            self.icons.typed_at = f64::NEG_INFINITY;
        }
        if r.gained_focus() || (r.changed() && self.icons.mode == IconsMode::Home) {
            self.icons.mode = IconsMode::Search;
        }
        if r.changed() && self.icons.mode != IconsMode::Search {
            self.icons.mode = IconsMode::Search;
        }
    }

    /// The box above Favorites and Your library that narrows them to what has the typed words in its
    /// title, description, category or tags.
    fn mine_search_box(&mut self, ui: &mut egui::Ui, hint: &str) {
        let r = egui::Frame::new().fill(theme::panel_raised()).corner_radius(8).stroke(Stroke::new(1.0, theme::border())).inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(ph::MAGNIFYING_GLASS).color(theme::text_dim()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let n = self.icons.mine_filter.active();
                    let (text, color) = match n {
                        0 => (ph::FUNNEL.to_string(), if self.icons.mine_show_filters { theme::text() } else { theme::text_dim() }),
                        n => (format!("{} {n}", ph::FUNNEL), theme::accent()),
                    };
                    let tip = match (self.icons.mine_show_filters, &self.icons.mode) {
                        (true, _) => "Hide the filters",
                        (false, IconsMode::Library) => "Filter by category or file type",
                        (false, _) => "Filter by database, author, category, license or tags",
                    };
                    if ui.add(egui::Button::new(egui::RichText::new(text).color(color)).frame(false)).on_hover_text(tip).clicked() {
                        self.icons.mine_show_filters = !self.icons.mine_show_filters;
                    }
                    search_help(ui);
                    if !self.icons.mine_query.is_empty() && ui.add(egui::Button::new(egui::RichText::new(ph::X).color(theme::text_dim())).frame(false)).on_hover_text("Clear").clicked() {
                        self.icons.mine_query.clear();
                        self.icons.typed_at = f64::NEG_INFINITY;
                    }
                    ui.add(egui::TextEdit::singleline(&mut self.icons.mine_query).hint_text(hint).frame(egui::Frame::NONE).desired_width(ui.available_width()))
                })
                .inner
            })
            .inner
        });
        let r = r.inner;
        if r.changed() {
            self.icons.typed_at = ui.input(|i| i.time);
        }
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            self.icons.typed_at = f64::NEG_INFINITY;
        }
    }

    /// Favorites' or Your library's filters when open, else what they keep, as the search's are.
    fn mine_filters(&mut self, ui: &mut egui::Ui) {
        if self.icons.mine_show_filters {
            self.filters_panel(ui, true);
        } else if self.icons.mine_filter.active() > 0 {
            let summary = self.filter_summary(true);
            let l = egui::Label::new(egui::RichText::new(format!("{}  {summary}", ph::FUNNEL)).small().color(theme::text_dim())).sense(Sense::click());
            if ui.add(l).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.icons.mine_show_filters = true;
            }
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
        } else if let Some(sync) = self.catalog_sync.as_ref().filter(|s| s.downloading()) {
            // Searches run on the cached copy until then, so newly published icons are missing.
            ui.horizontal(|ui| {
                ui.spinner();
                let mb = sync.bytes.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6;
                let text = if mb > 0.0 { format!("Updating the icon catalog... {mb:.1} MB") } else { "Updating the icon catalog...".to_string() };
                ui.label(egui::RichText::new(text).small().color(theme::text_dim()))
                    .on_hover_text("New icons will appear in searches once the download finishes");
            });
            ui.add_space(4.0);
        }
        match self.icons.mode {
            IconsMode::Home => {
                self.home_row_buttons(ui, &[
                    (ph::MAGNIFYING_GLASS, "Search artwork", IconsMode::Search),
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
                if row_button(ui, ph::BOOKS, "Add to your library").on_hover_text("Keep your images and slides in Your library").clicked() {
                    self.library_notice();
                    self.open_contribute(false);
                }
                if row_button(ui, ph::CLOUD_ARROW_UP, "Add to BioCartoon library").on_hover_text("Share your icons and templates with everyone").clicked() {
                    self.open_contribute(true);
                }
                if row_button_with(ui, paint_thought_bubble, "Submit an idea").on_hover_text("Ask the BioCartoon team for an icon, a template or anything else").clicked() {
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
                        let tn = self.template_rows.iter().filter(|r| r.source() == source).count();
                        let hover = match sources.iter().find(|s| s.source == source) {
                            Some(s) if tn > 0 || source == theme::BIOCARTOON_SOURCE => format!("{name}: {} icons, {tn} templates", thousands(s.icons)),
                            _ if tn > 0 || source == theme::BIOCARTOON_SOURCE => format!("{name}: {tn} templates"),
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
                // The search box is above the scroll area, in `left_panel`.
                if self.icons.show_filters {
                    self.filters_panel(ui, false);
                } else if self.icons.filter.active() > 0 {
                    // What the closed filters keep, as a reminder; clicking opens them.
                    let summary = self.filter_summary(false);
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
                    ui.label(egui::RichText::new("No favorites yet. Click the star on an icon or template, or right-click it and pick Favorite.").color(theme::text_dim()));
                } else {
                    self.mine_filters(ui);
                    self.results_grid(ui);
                }
            }
            IconsMode::Library => {
                self.mine_filters(ui);
                if self.icons.results.is_empty() && !self.icons.loading && self.matching_templates().is_empty() {
                    ui.label(egui::RichText::new("Nothing here yet. Images you add to your library (generated with AI, or from Uploads) and slides you save as templates appear here.").color(theme::text_dim()));
                }
                #[cfg(target_arch = "wasm32")]
                ui.label(
                    egui::RichText::new(if self.account.signed_in() {
                        "Your library is kept in this browser, with a copy online in your account."
                    } else {
                        "Your library is kept only in this browser. Sign in on the home screen to keep a copy online."
                    })
                    .small()
                    .color(theme::text_dim()),
                );
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
                self.icons.mine_query.clear();
                self.icons.mine_filter = SearchFilter::default();
                self.icons.mine_show_filters = false;
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
        // Favorites and Your library are loaded whole, so their box and filters narrow them here.
        let mine = matches!(self.icons.mode, IconsMode::Favorites | IconsMode::Library)
            && (!self.icons.mine_query.trim().is_empty() || self.icons.mine_filter.active() > 0);
        let shown: Vec<AssetRow> = if mine {
            let q = self.icons.mine_query.trim();
            self.icons.results.iter().filter(|r| local_match(r, q, &self.icons.mine_filter, None)).cloned().collect()
        } else {
            self.icons.results.clone()
        };
        let shown_total = if mine { Some(shown.len() as i64) } else { self.icons.total };
        if let Some(e) = &self.icons.error {
            ui.label(egui::RichText::new(e).color(Color32::from_rgb(230, 120, 120)));
        }
        if self.icons.loading && self.icons.results.is_empty() {
            ui.label(egui::RichText::new("Searching...").color(theme::text_dim()));
        }
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Sort").small().color(theme::text_dim()));
            let (sort, options): (&mut IconSort, &[IconSort]) = if self.icons.mode == IconsMode::Search {
                // Relevance only means something once a query is typed.
                let options: &[IconSort] = if self.icons.query.trim().is_empty() { &IconSort::SEARCH[1..] } else { &IconSort::SEARCH };
                (&mut self.settings.search_sort, options)
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
                self.icons.sort_picked = self.icons.mode == IconsMode::Search;
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
            if let Some(n) = shown_total.filter(|_| self.icons.include.icons() && !(self.icons.loading && self.icons.results.is_empty())) {
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
        let rows = if icons { shown } else { Vec::new() };
        if rows.is_empty() && templates.is_empty() && !self.icons.loading && self.icons.mode == IconsMode::Search && !self.icons.query.trim().is_empty() {
            ui.label(egui::RichText::new("No results").color(theme::text_dim()));
        }
        if rows.is_empty() && templates.is_empty() && !self.icons.loading && mine {
            ui.label(egui::RichText::new("Nothing matches").color(theme::text_dim()));
        }
        let sort = if self.icons.mode == IconsMode::Search { self.settings.search_sort } else { self.settings.icon_sort };
        let more = icons && self.icons.more;
        let total = shown_total.filter(|_| icons).map_or(rows.len(), |n| n.max(0) as usize);
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
                let (fingerprints, submitted) = (&mut self.fingerprints, &self.settings.submitted);
                let mut unsent = || !crate::app::row_submitted(fingerprints, submitted, row);
                let (r, act) = asset_tile(ui, size, row, row.png_local.as_deref(), &mut self.images, hold, colors, marks, &mut unsent);
                match act {
                    Some(TileAct::Remove) => self.actions.push(Action::RemoveUserAsset(row.clone())),
                    Some(TileAct::RequestRemoval) => self.actions.push(Action::OpenRemovalRequest(row.clone())),
                    Some(TileAct::Share) => self.actions.push(Action::OpenUploadDialog(UploadTarget::Library(Box::new(row.clone())), true)),
                    Some(TileAct::Favorite) => self.actions.push(Action::ToggleFavorite(row.id.clone())),
                    Some(TileAct::Download) => self.actions.push(Action::DownloadAsset(Box::new(row.clone()))),
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
            if ui.add(link("Submit an idea")).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.actions.push(Action::OpenSuggest(self.icons.query.trim().to_string()));
            }
            ui.label(dim("."));
        });
    }

    /// Submit an idea: ask the BioCartoon team for an icon, a template or anything else.
    fn suggest_panel(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Submit an idea for an icon or template (or anything else you would like to see) to the BioCartoon team.").color(theme::text_dim()));
        ui.add_space(8.0);
        let w = ui.available_width();
        ui.label(egui::RichText::new("Idea type").strong());
        let kinds = crate::submit::IDEA_KINDS;
        egui::ComboBox::from_id_salt("suggest_kind").width(w).selected_text(kinds.get(self.suggest.kind).unwrap_or(&kinds[0]).1).show_ui(ui, |ui| {
            for (i, (_, label)) in kinds.iter().enumerate() {
                ui.selectable_value(&mut self.suggest.kind, i, *label);
            }
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
        ui.add_space(6.0);
        ui.label(egui::RichText::new("Email").strong());
        ui.add(egui::TextEdit::singleline(&mut self.suggest.email).hint_text("Optional, never published").char_limit(254).desired_width(w))
            .on_hover_text("Only the BioCartoon team sees it, in case they want to follow up on your idea");
        ui.add_space(12.0);
        let can = !self.suggest.title.trim().is_empty() && !self.suggest.busy;
        let label = format!("{}  Submit idea", ph::PAPER_PLANE_TILT);
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
                    if ui.button(format!("{}  Add to your library", ph::BOOKS)).on_hover_text("Keep it in Your library").clicked() {
                        self.actions.push(Action::GeneratedToLibrary { path: path.clone(), favorite: false });
                    }
                    let sent = crate::app::file_submitted(&mut self.fingerprints, &self.settings.submitted, &path);
                    let b = ui.add_enabled(!sent, egui::Button::new(format!("{}  Add to BioCartoon library", ph::CLOUD_ARROW_UP)));
                    if b.on_hover_text("Share it with everyone").on_disabled_hover_text(crate::app::ALREADY_SUBMITTED).clicked() {
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
                    let sent = crate::app::file_submitted(&mut self.fingerprints, &self.settings.submitted, &g.path);
                    let b = ui.add_enabled(!sent, egui::Button::new(format!("{}  Add to BioCartoon library", ph::CLOUD_ARROW_UP)));
                    if b.on_disabled_hover_text(crate::app::ALREADY_SUBMITTED).clicked() {
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

    /// One template's thumbnail in the results grid; clicking it adds its slide, and clicking its
    /// star stars it (makes it a favorite), as on an icon.
    fn template_tile(&mut self, ui: &mut egui::Ui, size: f32, row: &TemplateRow) {
        let state = self.images.asset(&row.meta.thumb_key, row.thumb_local.as_deref());
        let (abbr, col, _) = theme::source_badge(row.source());
        let r = tile(ui, size, &row.meta.title, state, self.settings.tile_database.then_some((abbr, col, "Template")));
        let fav = self.settings.favorites.contains(&row.meta.id);
        let stars = self.template_stars.get(&row.meta.id).copied().unwrap_or(0);
        let star_rect = star_corner(ui, r.rect, stars, fav, self.settings.tile_stars, r.hover_pos());
        let on_star = (self.settings.tile_stars || fav) && r.interact_pointer_pos().is_some_and(|q| star_rect.contains(q));
        // Laid out like an icon's: "Eukaryotic cell  ·  Template", the description and category,
        // "BioCartoon  ·  CC0-1.0  ·  AI unknown", the ID, and the credit line in grey.
        let info = |ui: &mut egui::Ui| {
            ui.label(egui::RichText::new(format!("{}  ·  Template", row.meta.title)).strong());
            if !row.meta.description.is_empty() {
                ui.label(&row.meta.description);
            }
            if !row.meta.category.is_empty() {
                ui.label(format!("Database category: {}", row.meta.category));
            }
            let line = [row.meta.author.as_str(), row.meta.license.as_str(), ai_label(row.meta.generative_ai_used)];
            ui.label(line.iter().filter(|t| !t.is_empty()).copied().collect::<Vec<_>>().join("  ·  "));
            ui.label(format!("Template ID: {}", row.meta.id));
            ui.label(egui::RichText::new(crate::credits::template_row_credit(&row.meta)).small().color(theme::text_dim()));
        };
        // A long press opens the context menu too, so the info goes there for touch screens.
        let r = r.on_hover_ui(info);
        if r.clicked() {
            self.actions.push(if on_star { Action::ToggleFavorite(row.meta.id.clone()) } else { Action::UseSavedTemplate(row.clone()) });
        }
        r.context_menu(|ui| {
            info(ui);
            ui.separator();
            if ui.button(format!("{}  {}", ph::STAR, if fav { "Unfavorite" } else { "Favorite" })).clicked() {
                self.actions.push(Action::ToggleFavorite(row.meta.id.clone()));
                ui.close();
            }
            if !cfg!(target_arch = "wasm32") && ui.button(format!("{}  Download", ph::DOWNLOAD_SIMPLE)).on_hover_text("Keep it, and the icons it uses, for offline use").clicked() {
                self.actions.push(Action::DownloadTemplate(Box::new(row.clone())));
                ui.close();
            }
            if row.json_local.is_some() && ui.button("Remove from this computer").clicked() {
                self.actions.push(Action::RemoveTemplate(row.meta.id.clone()));
                ui.close();
            }
        });
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
                // What went to the BioCartoon library already can't go again.
                let sent = self.contribute.share && crate::app::file_submitted(&mut self.fingerprints, &self.settings.submitted, p);
                if sent {
                    dim_tile(ui, r.rect);
                    r.on_hover_text(crate::app::ALREADY_SUBMITTED);
                } else {
                    let on = self.contribute.icons.contains(p);
                    check_corner(ui, r.rect, on, true);
                    if r.on_hover_text(&name).clicked() && !self.contribute.icons.remove(p) {
                        self.contribute.icons.insert(p.clone());
                    }
                }
                if (i + 1) % per_row == 0 {
                    ui.end_row();
                }
            }
        });
        // Your library's own icons and templates are already there: only offered for sharing.
        let share = self.contribute.share;
        if share {
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
                    if crate::app::row_submitted(&mut self.fingerprints, &self.settings.submitted, row) {
                        dim_tile(ui, r.rect);
                        r.on_hover_text(crate::app::ALREADY_SUBMITTED);
                    } else {
                        let on = self.contribute.library.contains(&row.id);
                        check_corner(ui, r.rect, on, true);
                        if r.on_hover_text(&row.title).clicked() && !self.contribute.library.remove(&row.id) {
                            self.contribute.library.insert(row.id.clone());
                        }
                    }
                    if (i + 1) % per_row == 0 {
                        ui.end_row();
                    }
                }
            });
        }

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
        // This document's slides, then the uploaded ones: (slide, page, background, uploaded index).
        let mut entries: Vec<(crate::doc::Slide, Rect, Color32, Option<usize>)> = self.doc.slides.iter().map(|s| (s.clone(), page, c32(self.doc.background_of(s)), None)).collect();
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
        if share {
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

        }

        ui.add_space(16.0);
        let c = &self.contribute;
        let n = c.icons.len() + c.library.len() + c.slides.len() + c.picked_templates.len() + c.library_templates.len();
        let (icon, what) = if share { (ph::CLOUD_ARROW_UP, "Add to BioCartoon library") } else { (ph::BOOKS, "Add to your library") };
        let label = if n == 0 { format!("{icon}  {what}") } else { format!("{icon}  {what} ({n})") };
        let b = ui.add_enabled(n > 0, egui::Button::new(egui::RichText::new(label).color(Color32::WHITE).strong()).fill(theme::accent()).min_size(Vec2::new(ui.available_width(), 38.0)));
        if b.on_disabled_hover_text("Pick icons or templates above first").clicked() {
            self.actions.push(Action::ContributeSelected);
        }
        let note = if share { "Each one is reviewed before anyone else sees it." } else { self.library_where() };
        ui.label(egui::RichText::new(note).small().color(theme::text_dim()));
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
                let r = r.on_hover_text("Click to place; right-click to add to a library");
                r.context_menu(|ui| {
                    if ui.button(format!("{}  Add to your library...", ph::BOOKS)).clicked() {
                        self.actions.push(Action::OpenUploadDialog(UploadTarget::File(p.into()), false));
                        ui.close();
                    }
                    let sent = crate::app::file_submitted(&mut self.fingerprints, &self.settings.submitted, p);
                    let b = ui.add_enabled(!sent, egui::Button::new(format!("{}  Add to BioCartoon library...", ph::CLOUD_ARROW_UP)));
                    if b.on_disabled_hover_text(crate::app::ALREADY_SUBMITTED).clicked() {
                        self.actions.push(Action::OpenUploadDialog(UploadTarget::File(p.into()), true));
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
            let mut col = self.pen_color;
            if crate::color::edit_button(ui, &mut col).on_hover_text("Pen color").changed() {
                self.pen_color = col;
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

    /// What an image is, above Transform: its name, database and license, then its file type
    /// and size. The user's own images say just User-generated until they are given a license.
    fn image_info(&mut self, ui: &mut egui::Ui, source: &ImageSource) {
        let user = "User-generated".to_string();
        let first = match source {
            ImageSource::Asset { source: db, license, .. } if db == "user" && license.is_empty() => user,
            ImageSource::Asset { title, source: db, license, .. } => {
                let db = if db == "user" { "User-generated" } else { theme::source_badge(db).2 };
                [title.as_str(), db, license.as_str()].iter().filter(|t| !t.is_empty()).copied().collect::<Vec<_>>().join("  ·  ")
            }
            ImageSource::File { .. } => user,
        };
        let key = source.cache_key();
        let vector = self.images.vector(&key);
        let kind = match (&vector, source) {
            (Some(_), _) => "SVG".to_string(),
            (None, ImageSource::Asset { .. }) => "PNG".to_string(),
            (None, ImageSource::File { path, .. }) => match std::path::Path::new(path).extension().map(|e| e.to_string_lossy().to_lowercase()).as_deref() {
                Some("jpg" | "jpeg") => "JPEG".to_string(),
                Some(e) => e.to_uppercase(),
                None => "Image".to_string(),
            },
        };
        let file = self.image_file(ui.ctx(), source, &key);
        let dims = match &vector {
            Some(v) => Some((v.size().x, v.size().y)),
            None => file.and_then(|f| f.0),
        };
        let mut second = kind;
        if let Some((w, h)) = dims {
            second += &format!("  ·  {} × {} px", w.round(), h.round());
        }
        if let Some((_, bytes)) = file {
            second += &format!("  ·  {}", crate::offline::human_bytes(bytes as u64));
        }
        ui.label(egui::RichText::new(first).strong());
        ui.label(egui::RichText::new(second).small().color(theme::text_dim()));
        if let ImageSource::Asset { id, .. } = source {
            ui.label(format!("Asset ID: {id}"));
        }
    }

    /// An image file's size in pixels (from its header, as the canvas holds a scaled-down copy)
    /// and in bytes, read once; `None` while it is only on the network.
    #[allow(clippy::type_complexity)]
    fn image_file(&self, ctx: &egui::Context, source: &ImageSource, key: &str) -> Option<(Option<(f32, f32)>, usize)> {
        let id = egui::Id::new(("image_file", key));
        if let Some(info) = ctx.data(|d| d.get_temp::<(Option<(f32, f32)>, usize)>(id)) {
            return Some(info);
        }
        fn path(r: &crate::assets::Request) -> Option<&std::path::Path> {
            match r {
                crate::assets::Request::Path(p) => Some(p),
                crate::assets::Request::Either(a, b) => path(a).or_else(|| path(b)),
                crate::assets::Request::Url(_) => None,
            }
        }
        let req = self.images.request_for(source).ok()?;
        let bytes = crate::storage::read(path(&req)?).ok()?;
        let info = (crate::assets::image_size(&bytes), bytes.len());
        ctx.data_mut(|d| d.insert_temp(id, info));
        Some(info)
    }

    /// The layers an image offers to restyle, each a name and the colours in it: the catalog's
    /// named ones for a library asset (once its row has come), else the SVG's own colours, the
    /// [`MAX_COLOR_LAYERS`] most used. None for a picture that is not an SVG or has one colour.
    fn image_layers(&self, source: &ImageSource) -> Vec<(String, Vec<[u8; 3]>)> {
        let Some(vector) = self.images.vector(&source.cache_key()) else { return Vec::new() };
        if let ImageSource::Asset { id, .. } = source {
            let Some(row) = self.variants.iter().find(|r| &r.id == id) else { return Vec::new() };
            let named = named_layers(&row.layers);
            if !named.is_empty() {
                return named;
            }
        }
        color_layers(vector.colors())
    }

    /// Layers: a colour and an opacity for each of [`image_layers`](Self::image_layers), kept in
    /// `layers` while they differ from the drawing's own. Whether anything changed.
    fn layers_ui(&self, ui: &mut egui::Ui, source: &ImageSource, layers: &mut Vec<LayerStyle>) -> bool {
        let offered = self.image_layers(source);
        if offered.len() < 2 {
            return false;
        }
        let mut changed = false;
        // Collapsible, open by default; the Reset stays in the header row.
        let state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), ui.make_persistent_id("image_layers_open"), true);
        state
            .show_header(ui, |ui| {
                ui.label(egui::RichText::new("Layers").strong()).on_hover_text("Recolor or fade one part of the drawing at a time");
                if layers.iter().any(|l| !l.is_plain()) {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Reset").on_hover_text("Every layer back to the drawing's own colors").clicked() {
                            layers.clear();
                            changed = true;
                        }
                    });
                }
            })
            .body_unindented(|ui| {
                let mut rows: Vec<EditLayer> = offered
                    .into_iter()
                    .map(|(name, colors)| {
                        let style = layers.iter().find(|l| same_colors(&l.colors, &colors)).cloned().unwrap_or(LayerStyle { colors, color: None, opacity: 1.0 });
                        EditLayer { name, style, picked: false, removed: false }
                    })
                    .collect();
                if layer_grid(ui, "image_layers", &mut rows, None) {
                    changed = true;
                    *layers = rows.into_iter().map(|r| r.style).filter(|l| !l.is_plain()).collect();
                }
            });
        changed
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

        if let ItemKind::Image { source, .. } = &it.kind {
            self.image_info(ui, source);
            ui.add_space(6.0);
        }

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
            ItemKind::Image { source, overlay, layers, .. } => {
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
                    // As tall as the % box from the start, so the row centres on it rather than
                    // the box hanging below the label.
                    ui.set_min_height(ui.text_style_height(&egui::TextStyle::Button) + 2.0 * ui.spacing().button_padding.y);
                    ui.label("Color overlay");
                    let mut rgb = [overlay[0], overlay[1], overlay[2]];
                    if crate::color::edit_button_rgb(ui, &mut rgb).changed() {
                        overlay[..3].copy_from_slice(&rgb);
                        if overlay[3] == 0 { overlay[3] = 255; }
                        changed = true;
                    }
                    let mut strength = overlay[3] as f32 / 255.0 * 100.0;
                    if ui.add(egui::DragValue::new(&mut strength).speed(1.0).range(0.0..=100.0).max_decimals(0).suffix("%")).on_hover_text("Overlay strength").changed() {
                        overlay[3] = (strength / 100.0 * 255.0).round() as u8;
                        changed = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Reset").on_hover_text("Remove the color overlay").clicked() {
                            *overlay = [255, 255, 255, 0];
                            changed = true;
                        }
                    });
                });
                ui.add_space(4.0);
                changed |= self.layers_ui(ui, source, layers);
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
                    let credit = credit.unwrap_or_else(|| format!("Asset {aid}"));
                    ui.add(egui::Label::new(dim(credit.clone())).selectable(true));
                    if ui.add(egui::Button::new(format!("{}  Copy credit", ph::COPY)).small()).on_hover_text(credit.as_str()).clicked() {
                        ui.ctx().copy_text(credit);
                        self.toast("Copied the credit");
                    }
                }
            }
            ItemKind::Text { .. } => {
                let sel = self.text_selection(it.id);
                let styled = text_style_ui(ui, &mut edited.kind, sel.clone());
                // Back to typing, with the letters just styled still selected.
                self.text_edit_focus |= styled && sel.is_some();
                changed |= styled;
            }
            ItemKind::Shape { shape, fill, stroke, stroke_width } => {
                ui.label(egui::RichText::new("Shape").strong());
                egui::ComboBox::from_id_salt("shape_kind").selected_text(shape.label()).show_ui(ui, |ui| {
                    for k in ShapeKind::ALL { if ui.selectable_value(shape, k, k.label()).changed() { changed = true; } }
                });
                ui.horizontal(|ui| {
                    ui.label("Fill");
                    let mut col = *fill;
                    if crate::color::edit_button(ui, &mut col).changed() { *fill = col; changed = true; }
                    ui.label("Stroke");
                    let mut col = *stroke;
                    if crate::color::edit_button(ui, &mut col).changed() { *stroke = col; changed = true; }
                    changed |= ui.add(egui::DragValue::new(stroke_width).range(0.0..=40.0).speed(0.5)).changed();
                });
                egui::Grid::new("shape_presets").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                    for (label, c) in [("Fill presets", fill), ("Stroke presets", stroke)] {
                        ui.label(label);
                        if let Some(p) = color_presets(ui, *c, true) { *c = p; changed = true; }
                        ui.end_row();
                    }
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
                    let mut col = *color;
                    if crate::color::edit_button(ui, &mut col).changed() { *color = col; changed = true; }
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
                    let mut col = *text_color;
                    if crate::color::edit_button(ui, &mut col).changed() { *text_color = col; changed = true; }
                });
                ui.horizontal(|ui| {
                    ui.label("Fill");
                    let mut col = *fill;
                    if crate::color::edit_button(ui, &mut col).changed() { *fill = col; changed = true; }
                    ui.label("Header");
                    let mut col = *header_fill;
                    if crate::color::edit_button(ui, &mut col).changed() { *header_fill = col; changed = true; }
                    ui.label("Border");
                    let mut col = *border;
                    if crate::color::edit_button(ui, &mut col).changed() { *border = col; changed = true; }
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
        let before = (self.doc.width, self.doc.height, self.doc.unit, self.slide().background);
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
        let mut apply_all = false;
        ui.horizontal(|ui| {
            let mut col = self.doc.background_of(self.slide());
            if crate::color::edit_button(ui, &mut col).changed() {
                self.slide_mut().background = Some(col);
            }
            ui.label("Page color");
            let first = self.doc.background_of(&self.doc.slides[0]);
            let differ = self.doc.slides.iter().any(|s| self.doc.background_of(s) != first);
            apply_all = ui.add_enabled(differ, egui::Button::new("Apply to all")).on_hover_text("Give every slide this page color").clicked();
        });
        if toggle_row(ui, ph::RULER, "Show rulers", self.settings.show_rulers).clicked() {
            self.settings.show_rulers = !self.settings.show_rulers;
        }

        let after = (self.doc.width, self.doc.height, self.doc.unit, self.slide().background);
        if after != before {
            // One undo step per burst of edits, like the Edit panel: snapshot the state before them.
            if self.page.last_edit != Some(before) {
                (self.doc.width, self.doc.height, self.doc.unit, self.slide_mut().background) = before;
                self.commit();
                (self.doc.width, self.doc.height, self.doc.unit, self.slide_mut().background) = after;
            }
            self.dirty = true;
            self.page.last_edit = Some(after);
            if fit { self.view.fit_requested = true; }
        }
        if apply_all {
            self.commit();
            self.doc.background = self.doc.background_of(self.slide());
            for s in &mut self.doc.slides {
                s.background = None;
            }
            self.dirty = true;
            self.page.last_edit = None;
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
        if toggle_row(ui, ph::MAGNET, "Snap to grid", self.settings.grid_snap).on_hover_text("Edges and centers near a gridline jump onto it; hold Alt (Option) while dragging to place freely").clicked() {
            self.settings.grid_snap = !self.settings.grid_snap;
        }
        if toggle_row(ui, ph::DOTS_NINE, "Grid increments only", self.settings.grid_steps).on_hover_text("Positions and sizes always land on the grid; hold Alt (Option) while dragging to place freely").clicked() {
            self.settings.grid_steps = !self.settings.grid_steps;
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

    // ---------- Comments ----------

    /// The button that arms the comment tool, then this slide's comments; a click on one shows it.
    fn comments_panel(&mut self, ui: &mut egui::Ui) {
        let (label, tip) = if self.placing_comment {
            (format!("{}  Click on the page…", ph::CURSOR_CLICK), "Click where the comment goes, or click here again to cancel")
        } else {
            (format!("{}  Add a comment", ph::PLUS), "Then click on the page where it goes")
        };
        let b = egui::Button::new(egui::RichText::new(label).color(Color32::WHITE).strong()).fill(theme::accent()).min_size(Vec2::new(ui.available_width(), 38.0));
        if ui.add(b).on_hover_text(tip).clicked() {
            self.actions.push(Action::ArmComment(!self.placing_comment));
        }
        ui.add_space(6.0);
        let shown = self.settings.show_comments;
        if toggle_row(ui, ph::EYE, "Show comments on the page", shown).clicked() {
            self.settings.show_comments = !shown;
            if shown {
                self.open_comment = None;
            }
        }
        ui.add_space(10.0);
        let comments = &self.slide().comments;
        if comments.is_empty() {
            ui.label(egui::RichText::new("No comments on this slide yet.").color(theme::text_dim()));
            return;
        }
        let mut show = None;
        for (n, c) in comments.iter().enumerate() {
            let open = self.open_comment == Some(c.id);
            let fill = if open { theme::accent_dim() } else { theme::panel_raised() };
            let r = egui::Frame::new().fill(fill).corner_radius(6).inner_margin(8).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("{}", n + 1)).strong().color(theme::accent()));
                    let who = if c.author.is_empty() { "Comment" } else { c.author.as_str() };
                    ui.label(egui::RichText::new(who).strong());
                });
                ui.add(egui::Label::new(c.text.as_str()).wrap().selectable(false));
            });
            let r = ui.interact(r.response.rect, egui::Id::new(("comment_row", c.id)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Show it on the page");
            if r.clicked() {
                show = Some(c.id);
            }
            ui.add_space(4.0);
        }
        if let Some(id) = show {
            self.actions.push(Action::ShowComment(id));
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
                let bg = c32(self.doc.background_of(&slide));
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
                for (share, label) in [(false, format!("{}  Add to your library...", ph::BOOKS)), (true, format!("{}  Add to BioCartoon library...", ph::CLOUD_ARROW_UP))] {
                    let add = ui.add_enabled(!empty, egui::Button::new(label));
                    if add.on_disabled_hover_text("Add something to the slide first").clicked() {
                        self.actions.push(Action::OpenUploadDialog(self.template_target(i), share));
                        ui.close();
                    }
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
    // The button stays `width` wide, truncating a long choice, so picking one never widens the
    // panel; the open list can still be wider and hang over it.
    ui.allocate_ui(Vec2::new(width, ui.spacing().interact_size.y), |ui| {
        egui::ComboBox::from_id_salt(id)
            .width(width)
            .truncate()
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
    });
    changed
}

fn row_button(ui: &mut egui::Ui, icon: &str, label: &str) -> egui::Response {
    row_button_with(ui, |ui| { ui.label(egui::RichText::new(icon).size(16.0).color(Color32::WHITE)); }, label)
}

/// A [`row_button`] whose icon is drawn by `icon` (for icons the Phosphor font lacks).
fn row_button_with(ui: &mut egui::Ui, icon: impl FnOnce(&mut egui::Ui), label: &str) -> egui::Response {
    let r = egui::Frame::new().fill(theme::panel_raised()).corner_radius(8).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            egui::Frame::new().fill(theme::accent_dim()).corner_radius(6).inner_margin(4).show(ui, icon);
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

/// A thought bubble, in the space and style of a 16 px Phosphor icon (Phosphor has none).
fn paint_thought_bubble(ui: &mut egui::Ui) {
    // A transparent glyph reserves exactly the room the other rows' icons take.
    let r = ui.label(egui::RichText::new(ph::SQUARE).size(16.0).color(Color32::TRANSPARENT)).rect;
    let s = r.height().min(r.width()) / 16.0;
    let at = |x: f32, y: f32| r.center() + Vec2::new(x - 8.0, y - 8.0) * s;
    let stroke = Stroke::new(1.4 * s, Color32::WHITE);
    let p = ui.painter();
    // A cloud of three overlapping puffs, its outline drawn as arcs so the overlaps don't show.
    let puffs = [(at(6.6, 6.6), 3.6 * s), (at(10.6, 5.4), 3.9 * s), (at(11.4, 9.4), 3.0 * s), (at(7.4, 9.8), 2.8 * s)];
    for (i, &(c, rad)) in puffs.iter().enumerate() {
        let mut arc: Vec<Pos2> = Vec::new();
        for k in 0..=48 {
            let a = k as f32 / 48.0 * std::f32::consts::TAU;
            let q = c + Vec2::angled(a) * rad;
            if puffs.iter().enumerate().all(|(j, &(c2, r2))| j == i || q.distance(c2) >= r2 - 0.01) {
                arc.push(q);
            } else if arc.len() > 1 {
                p.add(egui::Shape::line(std::mem::take(&mut arc), stroke));
            } else {
                arc.clear();
            }
        }
        if arc.len() > 1 {
            p.add(egui::Shape::line(arc, stroke));
        }
    }
    // The trail of little bubbles down to the thinker.
    p.circle_stroke(at(3.8, 12.6), 1.3 * s, stroke);
    p.circle_filled(at(1.9, 14.9), 0.9 * s, Color32::WHITE);
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

/// "1 icon", "2,789 icons".
fn counted(n: i64, what: &str) -> String {
    format!("{} {what}{}", thousands(n), if n == 1 { "" } else { "s" })
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
/// Greys out a tile that can't be picked.
fn dim_tile(ui: &egui::Ui, rect: Rect) {
    let bg = if theme::is_dark() { Color32::from_black_alpha(150) } else { Color32::from_white_alpha(170) };
    ui.painter().rect_filled(rect, 6.0, bg);
}

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
    /// Save its original file (SVG, else PNG).
    Download,
    /// Remove from your library, on one of the user's own uploads.
    Remove,
    /// Request removal from BioCartoon library, on a published (or submitted) community icon.
    RequestRemoval,
    /// Add to BioCartoon library, on one of the user's own uploads.
    Share,
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
/// (or a long press) opens its info with Favorite, Download, Copy name and Copy credit.
/// `unsent` says whether one of the user's own icons can still go to the BioCartoon library
/// (asked only when its menu is open).
#[allow(clippy::too_many_arguments)]
/// Whether generative AI made an asset, for its hover text (`None`: nobody said).
fn ai_label(used: Option<bool>) -> &'static str {
    match used {
        Some(true) => "AI",
        Some(false) => "No AI",
        None => "AI unknown",
    }
}

fn asset_tile(ui: &mut egui::Ui, size: f32, row: &AssetRow, local: Option<&str>, images: &mut crate::assets::ImageCache, hold: bool, colors: usize, marks: TileMarks, unsent: &mut dyn FnMut() -> bool) -> (egui::Response, Option<TileAct>) {
    let fav = marks.fav;
    let state = images.asset(&row.png_key, local);
    let loading = ImgState::Loading;
    let state = if hold { &loading } else { state };
    let (abbr, col, name) = theme::source_badge(theme::asset_source(row));
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
        ui.label([name, row.license.as_str(), ai_label(row.generative_ai_used)].iter().filter(|t| !t.is_empty()).copied().collect::<Vec<_>>().join("  ·  "));
        let mut file = (if row.svg_key.is_some() { crate::catalog::FORMAT_SVG } else { crate::catalog::FORMAT_PNG }).to_string();
        if row.png_width > 0 && row.png_height > 0 {
            file += &format!("  ·  {} × {} px", row.png_width, row.png_height);
        }
        if row.file_bytes > 0 {
            file += &format!("  ·  {}", crate::offline::human_bytes(row.file_bytes));
        }
        ui.label(file);
        ui.label(format!("Asset ID: {}", row.id));
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
        if ui.button(format!("{}  Download", ph::DOWNLOAD_SIMPLE)).clicked() {
            act = Some(TileAct::Download);
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
        // Anyone can ask for a published community icon, or one they submitted, to come out of
        // the BioCartoon library; a reviewer decides.
        if row.source == "user" {
            ui.separator();
            let sent = if row.png_local.is_some() {
                let unsent = unsent();
                let b = ui.add_enabled(unsent, egui::Button::new(format!("{}  Add to BioCartoon library...", ph::CLOUD_ARROW_UP)));
                if b.on_disabled_hover_text(crate::app::ALREADY_SUBMITTED).clicked() {
                    act = Some(TileAct::Share);
                    ui.close();
                }
                if ui.button(format!("{}  Remove from your library", ph::TRASH)).clicked() {
                    act = Some(TileAct::Remove);
                    ui.close();
                }
                !unsent
            } else {
                true
            };
            if sent && ui.button(format!("{}  Request removal from BioCartoon library...", ph::FLAG)).clicked() {
                act = Some(TileAct::RequestRemoval);
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
/// decoration, one labelled row each. Colour and decoration go to the characters `sel` (char
/// indices) when letters are selected in the box, and to the whole box otherwise.
fn text_style_ui(ui: &mut egui::Ui, kind: &mut ItemKind, sel: Option<std::ops::Range<usize>>) -> bool {
    let mut color = crate::doc::color_at(kind, sel.clone());
    let highlight = crate::doc::highlight_at(kind, sel.clone());
    const FLAGS: [TextFlag; 6] = [TextFlag::Bold, TextFlag::Italic, TextFlag::Underline, TextFlag::Strike, TextFlag::Superscript, TextFlag::Subscript];
    let flags = FLAGS.map(|f| crate::doc::flag_all(kind, sel.clone(), f));
    let (mut new_color, mut new_highlight, mut toggled, mut from_latex) = (None, None, None, false);
    let ItemKind::Text { text, size, align, font, line_spacing, valign, fill, .. } = kind else { return false };
    let color = &mut color;
    let mut changed = false;
    ui.label(egui::RichText::new("Text").strong());
    if sel.is_some() {
        ui.label(egui::RichText::new("Color, highlight and decoration go to the selected letters.").small().color(theme::text_dim()));
    }
    // Each font's name is written in that font.
    let px = egui::TextStyle::Body.resolve(ui.style()).size;
    let in_font = |f: TextFont| egui::RichText::new(f.label()).font(f.font_id(px));
    let text_w = |ui: &egui::Ui, t: egui::WidgetText| t.into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Body).size().x;
    // The fields fill the column beside the labels, and are never narrower than the font
    // dropdown needs for its longest name, so every row lines up with it.
    let labels = ["Font", "Size", "Text color", "Presets", "Textbox color", "Highlight", "Line spacing", "Horizontal align", "Vertical align", "List style", "Decoration", "Equation"];
    let label_w = labels.iter().map(|l| text_w(ui, (*l).into())).fold(0.0, f32::max);
    let sp = ui.spacing();
    let combo_extra = sp.icon_spacing + sp.icon_width + 2.0 * sp.button_padding.x;
    let font_w = TextFont::ALL.iter().map(|f| text_w(ui, in_font(*f).into())).fold(0.0, f32::max) + combo_extra;
    let w = (ui.available_width() - label_w - 8.0).max(font_w).max(160.0);
    egui::Grid::new("text_style").num_columns(2).spacing([8.0, 8.0]).show(ui, |ui| {
        ui.label("Font");
        egui::ComboBox::from_id_salt("text_font").selected_text(in_font(*font)).width(w).show_ui(ui, |ui| {
            for f in TextFont::ALL {
                changed |= ui.selectable_value(font, f, in_font(f)).changed();
            }
        });
        ui.end_row();

        ui.label("Size");
        segmented(ui, w, 3, |ui, cell| {
            if ui.add_sized(cell, egui::Button::new(ph::MINUS).frame(false)).on_hover_text("Smaller").clicked() {
                *size = (*size - 1.0).max(4.0);
                changed = true;
            }
            changed |= ui.add_sized(cell, egui::DragValue::new(size).range(4.0..=crate::doc::MAX_FONT_SIZE).speed(0.25).max_decimals(1)).changed();
            if ui.add_sized(cell, egui::Button::new(ph::PLUS).frame(false)).on_hover_text("Larger").clicked() {
                *size = (*size + 1.0).min(crate::doc::MAX_FONT_SIZE);
                changed = true;
            }
        });
        ui.end_row();

        ui.label("Text color");
        new_color = text_color_field(ui, "text_hex", *color, w, 0.0, |_| {});
        ui.end_row();

        ui.label("Presets");
        new_color = color_presets(ui, *color, false).or(new_color);
        ui.end_row();

        // The whole box's colour, behind the text; clear by default, and picking a colour from
        // there makes it opaque.
        ui.label("Textbox color");
        let picked = text_color_field(ui, "text_fill_hex", *fill, w, 0.0, |_| {}).map(|c| if c[3] == 0 { [c[0], c[1], c[2], 255] } else { c });
        if let Some(c) = picked.filter(|c| c != fill) {
            *fill = c;
            changed = true;
        }
        ui.end_row();

        ui.label("");
        if let Some(c) = color_presets(ui, *fill, true).filter(|c| c != fill) {
            *fill = c;
            changed = true;
        }
        ui.end_row();

        // No highlight shows as clear, and picking a colour from there makes it opaque.
        ui.label("Highlight");
        let shown = highlight.unwrap_or([255, 255, 255, 0]);
        let mut cleared = false;
        let picked = text_color_field(ui, "text_highlight_hex", shown, w, 40.0, |ui| {
            cleared = ui.add_enabled(highlight.is_some(), egui::Button::new(ph::PROHIBIT)).on_hover_text("No highlight").clicked();
        });
        let picked = picked.map(|c| if c[3] == 0 { [c[0], c[1], c[2], 255] } else { c });
        new_highlight = if cleared { Some(None) } else { picked.map(Some) };
        ui.end_row();

        ui.label("");
        new_highlight = color_presets(ui, shown, false).map(Some).or(new_highlight);
        ui.end_row();

        ui.label("Line spacing");
        let label = |v: f32| if v.fract() == 0.0 { format!("{v:.1}") } else { format!("{v}") };
        egui::ComboBox::from_id_salt("text_spacing").selected_text(label(*line_spacing)).width(w).show_ui(ui, |ui| {
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
        segmented(ui, w, 4, |ui, cell| {
            let tips = [(ph::TEXT_B, "Bold (⌘/Ctrl+B)"), (ph::TEXT_ITALIC, "Italic (⌘/Ctrl+I)"), (ph::TEXT_UNDERLINE, "Underline (⌘/Ctrl+U)"), (ph::TEXT_STRIKETHROUGH, "Strikethrough")];
            for (i, (icon, tip)) in tips.into_iter().enumerate() {
                if seg_toggle(ui, cell, icon, flags[i], tip).clicked() {
                    toggled = Some(i);
                }
            }
        });
        ui.end_row();

        // Below the other four, cells the same width.
        ui.label("");
        segmented(ui, w, 4, |ui, cell| {
            for (i, icon, tip) in [(4, ph::TEXT_SUPERSCRIPT, "Superscript"), (5, ph::TEXT_SUBSCRIPT, "Subscript")] {
                if seg_toggle(ui, cell, icon, flags[i], tip).clicked() {
                    toggled = Some(i);
                }
            }
        });
        ui.end_row();

        ui.label("Equation");
        let tip = "Read the box's text as LaTeX math and write it out: x^2, H_2O, \\alpha, \\rightarrow, \\frac{a}{b}, \\sqrt{x} \
                   become x², H₂O, α, →, a/b, √x, with real superscripts and subscripts";
        from_latex = ui.add_sized([w.min(120.0), ui.spacing().interact_size.y], egui::Button::new("LaTeX")).on_hover_text(tip).clicked();
        ui.end_row();
    });
    if let Some(c) = new_color.filter(|c| *c != *color) {
        crate::doc::set_color(kind, sel.clone(), c);
        changed = true;
    }
    if let Some(h) = new_highlight.filter(|h| *h != highlight) {
        crate::doc::set_highlight(kind, sel.clone(), h);
        changed = true;
    }
    if let Some(i) = toggled {
        crate::doc::set_flag(kind, sel, FLAGS[i], !flags[i]);
        changed = true;
    }
    if from_latex {
        if let ItemKind::Text { text, runs, .. } = kind {
            let (t, r) = crate::latex::to_text(text);
            if t != *text || r != *runs {
                (*text, *runs) = (t, r);
                changed = true;
            }
        }
    }
    changed
}

/// A colour button and its hex field, `w` wide, with `extra` (`extra_w` wide) after them; the
/// colour picked or typed. `id` keeps what is being typed while the field has focus.
fn text_color_field(ui: &mut egui::Ui, id: &str, color: [u8; 4], w: f32, extra_w: f32, extra: impl FnOnce(&mut egui::Ui)) -> Option<[u8; 4]> {
    let mut out = None;
    ui.horizontal(|ui| {
        let mut col = color;
        if crate::color::edit_button(ui, &mut col).changed() {
            out = Some(col);
        }
        // Keep what is being typed while the field has focus; show the colour otherwise.
        let id = ui.make_persistent_id(id);
        let current = format!("#{:02X}{:02X}{:02X}", color[0], color[1], color[2]);
        let mut hex = ui.data(|d| d.get_temp::<String>(id)).unwrap_or(current);
        let r = ui.add(egui::TextEdit::singleline(&mut hex).id(id.with("edit")).desired_width(ui.available_width().min(w - 48.0 - extra_w)).char_limit(7));
        if r.changed() {
            if let Some(c) = parse_hex(&hex) {
                out = Some([c[0], c[1], c[2], color[3]]);
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
        extra(ui);
    });
    out
}

/// The common colours as a row of swatches, the one matching `current` ringed (none when it is
/// clear, unless `transparent` adds a last swatch for clear); the one clicked.
pub(crate) fn color_presets(ui: &mut egui::Ui, current: [u8; 4], transparent: bool) -> Option<[u8; 4]> {
    const PRESETS: [(&str, [u8; 3]); 11] = [
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
    let mut picked = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(4.0);
        for (name, [r, g, b]) in PRESETS {
            let (rect, resp) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
            let on = current[3] > 0 && current[..3] == [r, g, b];
            let ring = if on { Stroke::new(2.0, theme::accent()) } else if resp.hovered() { Stroke::new(1.5, theme::text()) } else { Stroke::new(1.0, theme::border()) };
            ui.painter().rect(rect, 4.0, Color32::from_rgb(r, g, b), ring, StrokeKind::Inside);
            if resp.on_hover_text(name).clicked() {
                // Keep the colour's transparency.
                picked = Some([r, g, b, if current[3] == 0 { 255 } else { current[3] }]);
            }
        }
        if transparent {
            // White crossed out in red, as clear is shown elsewhere.
            let (rect, resp) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
            let on = current[3] == 0;
            let ring = if on { Stroke::new(2.0, theme::accent()) } else if resp.hovered() { Stroke::new(1.5, theme::text()) } else { Stroke::new(1.0, theme::border()) };
            ui.painter().rect_filled(rect, 4.0, Color32::WHITE);
            let d = rect.shrink(3.0);
            ui.painter().line_segment([d.left_bottom(), d.right_top()], Stroke::new(1.5, Color32::from_rgb(220, 38, 38)));
            ui.painter().rect_stroke(rect, 4.0, ring, StrokeKind::Inside);
            if resp.on_hover_text("Transparent").clicked() {
                picked = Some([current[0], current[1], current[2], 0]);
            }
        }
    });
    picked
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

/// One of the filters, for counting a filter's choices under all the others (see [`local_facets`]).
#[derive(Clone, Copy, PartialEq)]
enum Dim {
    Source,
    Taxa,
    DbCat,
    License,
    Format,
}

/// Whether a loaded asset passes Favorites' or Your library's box (`q`, lowercase: in its title,
/// description, category or tags) and filters, as the catalog's search would, leaving out `skip`.
/// The question mark in a search box, and the popup it opens on a click (so it works on a phone
/// too) that explains the advanced syntax (see [`syntax`]).
fn search_help(ui: &mut egui::Ui) {
    let btn = ui.add(egui::Button::new(egui::RichText::new(ph::QUESTION).color(theme::text_dim())).frame(false)).on_hover_text("Search syntax");
    egui::Popup::menu(&btn).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.set_width(300.0);
        // Left-aligned: a menu justifies wrapped text, which spaces the field list out.
        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
            let dim = |t: &str| egui::RichText::new(t).color(theme::text_dim());
            let code = |ui: &mut egui::Ui, q: &str, what: &str| {
                ui.label(egui::RichText::new(q).monospace());
                ui.label(dim(what));
                ui.add_space(4.0);
            };
            ui.label(egui::RichText::new("Search syntax").strong());
            ui.add_space(4.0);
            code(ui, "(mouse OR rat) AND file_type:svg", "Mouse or rat icons with a vector version");
            code(ui, "author:\"Jane Doe\"", "Quotes keep words together");
            code(ui, "neuron NOT database:servier", "Neurons from any database but Servier");
            ui.add_space(4.0);
            ui.label(dim("Fields: database, author, category, subcategory, database_category, license, file_type, tags, id. \
    Combine with AND, OR, NOT (in capitals) and parentheses; words side by side must all match."));
        });
    });
}

/// Whether a record passes the search box's `q` (as typed): a plain query as a part of its
/// `text` or `tags`, an advanced one (see [`syntax`]) term by term, `field` matching a field term.
fn query_match(q: &str, text: &[&str], tags: &[String], field: impl Fn(Field, &str) -> bool) -> bool {
    match syntax::parse(q) {
        None => {
            let q = q.to_lowercase();
            q.is_empty() || text.iter().any(|t| t.to_lowercase().contains(&q)) || tags.iter().any(|t| t.to_lowercase().contains(&q))
        }
        Some(e) => {
            let all = text.iter().copied().chain(tags.iter().map(String::as_str)).collect::<Vec<_>>().join(" ");
            e.eval(&|t| match t {
                Expr::Field(f, v) => field(*f, v),
                t => syntax::text_matches(&all, t),
            })
        }
    }
}

/// [`query_match`] for a saved template. Its category is free text, so it answers `category`,
/// `subcategory` and `database_category` alike; slides, it has no file type.
fn template_match(r: &TemplateRow, q: &str) -> bool {
    let m = &r.meta;
    query_match(q, &[&m.title, &m.description, &m.category], &m.tags, |f, v| match f {
        Field::Database => contains(r.source(), v) || (r.source() == "user" && contains("user-generated", v)),
        Field::Author => contains(&m.author, v),
        Field::Category | Field::Subcategory | Field::DatabaseCategory => contains(&m.category, v),
        Field::License => contains(&m.license, v),
        Field::FileType => false,
        Field::Tags => m.tags.iter().any(|t| contains(t, v)),
        Field::Id => m.id.eq_ignore_ascii_case(v),
    })
}

fn local_match(row: &AssetRow, q: &str, f: &SearchFilter, skip: Option<Dim>) -> bool {
    let eq = |a: &str, b: &str| a.trim().eq_ignore_ascii_case(b.trim());
    let shown = theme::asset_source(row);
    let text = query_match(q, &[&row.title, &row.description, &row.category], &row.tags, |f, v| match f {
        Field::Database => contains(shown, v) || contains(&row.source_name, v),
        Field::Author => contains(&row.author, v),
        Field::Category => row.taxa.iter().any(|(c, _)| contains(c, v)),
        Field::Subcategory => row.taxa.iter().any(|(_, s)| contains(s, v)),
        Field::DatabaseCategory => contains(&row.category, v),
        Field::License => contains(&row.license, v),
        Field::FileType => syntax::file_type_matches(v, row.svg_key.is_some()),
        Field::Tags => row.tags.iter().any(|t| contains(t, v)),
        Field::Id => row.id.eq_ignore_ascii_case(v),
    });
    let authors = f.authors();
    let tags = f.tags();
    let taxa = f.catalog(&[]).taxa;
    text && (skip == Some(Dim::Source) || f.sources.is_empty() || f.sources.iter().any(|s| s == shown || (s == LIBRARY_SOURCE && own_asset(row))))
        && (authors.is_empty() || authors.iter().any(|a| row.author.to_lowercase().contains(a)))
        && (skip == Some(Dim::Taxa) || taxa.is_empty() || taxa.iter().any(|(c, s)| row.taxa.iter().any(|(rc, rs)| rc == c && (s.is_empty() || rs == s))))
        && (skip == Some(Dim::DbCat) || f.db_cats.is_empty() || f.db_cats.iter().any(|(s, c)| s == shown && row.category.split(", ").any(|rc| eq(rc, c))))
        && (skip == Some(Dim::License) || f.licenses.is_empty() || f.licenses.iter().any(|l| l == &row.license))
        && (skip == Some(Dim::Format) || f.formats.is_empty() || f.formats.iter().any(|x| x == asset_format(row)))
        && (tags.is_empty() || tags.iter().any(|t| row.tags.iter().any(|x| eq(x, t))))
}

/// One of the user's own icons, in Your library (as the catalog's `LIBRARY_SOURCE` keeps them).
fn own_asset(row: &AssetRow) -> bool {
    row.source == "user" && row.png_local.is_some()
}

/// `catalog::FORMAT_SVG` when the asset has an SVG, else `catalog::FORMAT_PNG`.
fn asset_format(row: &AssetRow) -> &'static str {
    if row.svg_key.is_some() { crate::catalog::FORMAT_SVG } else { crate::catalog::FORMAT_PNG }
}

/// The filters' choices among loaded assets (Favorites, Your library), each counted under the
/// box and the other filters, as the catalog's facets are.
fn local_facets(rows: &[AssetRow], q: &str, f: &SearchFilter) -> Facets {
    fn tally(counts: &mut Vec<(String, i64)>, v: &str) {
        match counts.iter_mut().find(|(k, _)| k == v) {
            Some((_, n)) => *n += 1,
            None => counts.push((v.to_string(), 1)),
        }
    }
    let under = |dim: Dim| rows.iter().filter(move |r| local_match(r, q, f, Some(dim)));
    let mut fc = Facets { sources: Vec::new(), licenses: Vec::new(), taxa: Vec::new(), db_cats: Vec::new(), formats: Vec::new() };
    for r in under(Dim::Source) {
        tally(&mut fc.sources, theme::asset_source(r));
        if own_asset(r) {
            tally(&mut fc.sources, LIBRARY_SOURCE);
        }
    }
    for r in under(Dim::License).filter(|r| !r.license.is_empty()) {
        tally(&mut fc.licenses, &r.license);
    }
    for r in under(Dim::Format) {
        tally(&mut fc.formats, asset_format(r));
    }
    let mut taxa: Vec<(String, i64)> = Vec::new();
    for r in under(Dim::Taxa) {
        // Once per category, however many of its subcategories the asset is in.
        let mut cats: Vec<&str> = r.taxa.iter().map(|(c, _)| *c).collect();
        cats.sort();
        cats.dedup();
        for c in cats {
            tally(&mut taxa, &format!("{c}\u{0}"));
        }
        for (c, s) in &r.taxa {
            tally(&mut taxa, &format!("{c}\u{0}{s}"));
        }
    }
    fc.taxa = taxa
        .into_iter()
        .map(|(k, count)| {
            let (cat, sub) = k.split_once('\u{0}').unwrap_or((&k, ""));
            crate::catalog::TaxonCount { cat: cat.to_string(), sub: sub.to_string(), count }
        })
        .collect();
    for r in under(Dim::DbCat) {
        let shown = theme::asset_source(r);
        for c in r.category.split(", ").map(str::trim).filter(|c| !c.is_empty()) {
            match fc.db_cats.iter_mut().find(|(s, k, _)| s == shown && k == c) {
                Some((_, _, n)) => *n += 1,
                None => fc.db_cats.push((shown.to_string(), c.to_string(), 1)),
            }
        }
    }
    fc
}


/// One layer as [`layer_grid`] shows it: its name, and its colours restyled (or not).
pub struct EditLayer {
    pub name: String,
    pub style: LayerStyle,
    /// Ticked to be merged (only in the upload dialog).
    pub picked: bool,
    /// Crossed out in the upload dialog: not kept as a layer, so its colour stays as drawn.
    pub removed: bool,
}

impl EditLayer {
    pub fn new(name: String, colors: Vec<[u8; 3]>) -> Self {
        Self { name, style: LayerStyle { colors, color: None, opacity: 1.0 }, picked: false, removed: false }
    }
}

/// How many of the layers are kept (not crossed out).
pub fn kept(rows: &[EditLayer]) -> usize {
    rows.iter().filter(|r| !r.removed).count()
}

/// The catalog's named layers with their colours read, leaving out any with none.
pub fn named_layers(layers: &[crate::catalog::AssetLayer]) -> Vec<(String, Vec<[u8; 3]>)> {
    let hex = |c: &str| -> Option<[u8; 3]> {
        let h = c.strip_prefix('#').filter(|h| h.len() == 6)?;
        let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
        Some([b(0)?, b(2)?, b(4)?])
    };
    layers.iter().map(|l| (l.name.clone(), l.colors.iter().filter_map(|c| hex(c)).collect::<Vec<_>>())).filter(|(_, c)| !c.is_empty()).collect()
}

/// A drawing's own colours as layers, the [`MAX_COLOR_LAYERS`] most used, named Layer 1, 2...
/// None when it has fewer than two.
pub fn color_layers(colors: &[[u8; 3]]) -> Vec<(String, Vec<[u8; 3]>)> {
    if colors.len() < 2 {
        return Vec::new();
    }
    colors.iter().take(MAX_COLOR_LAYERS).enumerate().map(|(i, c)| (format!("Layer {}", i + 1), vec![*c])).collect()
}

/// The layers an upload starts with: `named` (the catalog's, when it names two or more) or none,
/// then every colour of the drawing they leave out as a layer of its own, Layer n. Not capped at
/// [`MAX_COLOR_LAYERS`] like the layers a drawing nobody named offers: the contributor merges
/// them into the parts they belong to, and no colour is left unrestylable.
pub fn upload_layers(named: Vec<(String, Vec<[u8; 3]>)>, colors: &[[u8; 3]]) -> Vec<(String, Vec<[u8; 3]>)> {
    let mut layers = if named.len() >= 2 { named } else { Vec::new() };
    for c in colors {
        if !layers.iter().any(|(_, l)| l.contains(c)) {
            layers.push((format!("Layer {}", layers.len() + 1), vec![*c]));
        }
    }
    if layers.len() < 2 { Vec::new() } else { layers }
}

/// Layers whose colours look at least this alike (CIE76 ΔE; about 2.3 is just noticeable) are
/// offered to [`merge_similar`].
const SIMILAR_DELTA_E: f32 = 10.0;

/// A colour in CIELAB (D65).
fn lab(c: [u8; 3]) -> [f32; 3] {
    let lin = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    let (r, g, b) = (lin(c[0]), lin(c[1]), lin(c[2]));
    let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
    let f = |t: f32| if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// How different two colours look (CIE76 ΔE).
fn delta_e(a: [u8; 3], b: [u8; 3]) -> f32 {
    let (a, b) = (lab(a), lab(b));
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Whether two layers hold the same colours, in any order.
pub fn same_colors(a: &[[u8; 3]], b: &[[u8; 3]]) -> bool {
    a.len() == b.len() && a.iter().all(|c| b.contains(c))
}

/// Whether a layer still has the name it was given (Layer n), not one the contributor typed.
fn numbered(name: &str) -> bool {
    name.strip_prefix("Layer ").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Name the kept layers still called Layer n after their place among them, once merging,
/// splitting or crossing out moved them.
fn renumber(rows: &mut [EditLayer]) {
    for (i, r) in rows.iter_mut().filter(|r| !r.removed).enumerate() {
        if numbered(&r.name) {
            r.name = format!("Layer {}", i + 1);
        }
    }
}

/// Merge the picked layers into the first of them, which keeps its name and look and takes all
/// their colours.
pub fn merge_picked(rows: &mut Vec<EditLayer>) {
    let picked: Vec<usize> = (0..rows.len()).filter(|&i| rows[i].picked && !rows[i].removed).collect();
    if let [first, rest @ ..] = picked.as_slice() {
        for &i in rest.iter().rev() {
            let r = rows.remove(i);
            rows[*first].style.colors.extend(r.style.colors);
        }
    }
    rows.iter_mut().for_each(|r| r.picked = false);
    renumber(rows);
}

/// Pairs of layers that look alike (by their most used colours), each the later into the earlier.
fn similar(rows: &[EditLayer]) -> Vec<(usize, usize)> {
    let mut into: Vec<(usize, usize)> = Vec::new();
    for i in (1..rows.len()).filter(|&i| !rows[i].removed) {
        // Into an earlier kept layer that is not itself merged away.
        let target = (0..i)
            .filter(|&j| !rows[j].removed && !into.iter().any(|&(k, _)| k == j))
            .find(|&j| delta_e(rows[j].style.colors[0], rows[i].style.colors[0]) < SIMILAR_DELTA_E);
        if let Some(j) = target {
            into.push((i, j));
        }
    }
    into
}

/// Merge every layer into an earlier one that looks alike. Whether any was.
pub fn merge_similar(rows: &mut Vec<EditLayer>) -> bool {
    let into = similar(rows);
    for &(i, j) in into.iter().rev() {
        let r = rows.remove(i);
        rows[j].style.colors.extend(r.style.colors);
    }
    renumber(rows);
    !into.is_empty()
}

/// Split a layer into one per colour, after it; the first keeps its name and look.
pub fn split_layer(rows: &mut Vec<EditLayer>, i: usize) {
    let rest = rows[i].style.colors.split_off(1);
    let name = rows[i].name.clone();
    for (k, c) in rest.into_iter().enumerate() {
        let name = if numbered(&name) { "Layer 0".to_string() } else { format!("{name} {}", k + 2) };
        rows.insert(i + 1 + k, EditLayer::new(name, vec![c]));
    }
    renumber(rows);
}

/// Split every layer of several colours into one per colour.
pub fn unmerge_all(rows: &mut Vec<EditLayer>) {
    for i in (0..rows.len()).rev() {
        if rows[i].style.colors.len() > 1 {
            split_layer(rows, i);
        }
    }
}

/// Cross a layer out (or bring it back): one crossed out keeps its own colour and is not kept.
pub fn remove_layer(rows: &mut [EditLayer], i: usize, removed: bool) {
    let r = &mut rows[i];
    r.removed = removed;
    r.picked = false;
    r.style.color = None;
    r.style.opacity = 1.0;
    renumber(rows);
}

/// Whether the layers are as the upload dialog started them.
fn as_started(rows: &[EditLayer], start: &[(String, Vec<[u8; 3]>)]) -> bool {
    rows.len() == start.len() && rows.iter().zip(start).all(|(r, (name, colors))| !r.removed && r.style.is_plain() && &r.name == name && &r.style.colors == colors)
}

/// Most stripes a layer of several colours shows on its colour box.
const MAX_STRIPES: usize = 8;

/// One row per layer, as the Layers editor and the library dialog show them: `Name:`, a colour
/// box (striped with each of its colours while a layer of several is not recoloured), the
/// opacity and an eye, and a button back to its own colour once it is restyled. Whether a style
/// changed (not a name).
///
/// `upload` (the layers the upload dialog started with) makes it the dialog's editor: the names
/// are text boxes, a box to tick layers to merge starts each row, and one at its end splits a
/// layer of several colours, or else crosses it out (or brings it back); above, the buttons to
/// merge, and below them, to unmerge all and to start over.
pub fn layer_grid(ui: &mut egui::Ui, id_salt: &str, rows: &mut Vec<EditLayer>, upload: Option<&[(String, Vec<[u8; 3]>)]>) -> bool {
    let edit = upload.is_some();
    let mut changed = false;
    if let Some(start) = upload {
        let picked = rows.iter().filter(|r| r.picked && !r.removed).count();
        let alike = similar(rows).len();
        ui.horizontal(|ui| {
            if ui.add_enabled(picked >= 2, egui::Button::new(format!("{} Merge", ph::ARROWS_MERGE)).small())
                .on_hover_text("Merge the ticked layers into one, so recoloring it recolors all of them")
                .on_disabled_hover_text("Tick two or more layers to merge them")
                .clicked()
            {
                merge_picked(rows);
                changed = true;
            }
            if alike > 0
                && ui.add(egui::Button::new(format!("Merge similar colors ({alike})")).small())
                    .on_hover_text("Merge each layer into an earlier one of nearly the same color")
                    .clicked()
            {
                merge_similar(rows);
                changed = true;
            }
        });
        ui.horizontal(|ui| {
            let merged = rows.iter().any(|r| r.style.colors.len() > 1);
            if ui.add_enabled(merged, egui::Button::new(format!("{} Unmerge all", ph::ARROWS_SPLIT)).small())
                .on_hover_text("Split every merged layer into one layer per color")
                .clicked()
            {
                unmerge_all(rows);
                changed = true;
            }
            if ui.add_enabled(!as_started(rows, start), egui::Button::new(format!("{} Reset all", ph::ARROW_COUNTER_CLOCKWISE)).small())
                .on_hover_text("Every layer back as it was: unmerged, none crossed out, the names and colors it started with")
                .clicked()
            {
                *rows = start.iter().map(|(name, colors)| EditLayer::new(name.clone(), colors.clone())).collect();
                changed = true;
            }
        });
    }
    // What a row's last button asks for: split it, or cross it out (true) or bring it back.
    let (mut split, mut remove) = (None, None);
    egui::Grid::new(id_salt).num_columns(if edit { 7 } else { 5 }).spacing([6.0, 4.0]).show(ui, |ui| {
        for (i, row) in rows.iter_mut().enumerate() {
            let style = &mut row.style;
            let before = style.clone();
            let live = !row.removed;
            if edit {
                ui.add_enabled(live, egui::Checkbox::new(&mut row.picked, ""))
                    .on_hover_text("Tick two or more to merge them into one layer")
                    .on_disabled_hover_text("Crossed out");
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    if live {
                        ui.add(egui::TextEdit::singleline(&mut row.name).char_limit(40).desired_width(110.0));
                    } else {
                        let size = egui::vec2(110.0, ui.spacing().interact_size.y);
                        ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.set_min_size(size);
                            ui.add_space(4.0);
                            ui.label(egui::RichText::new(&row.name).strikethrough().color(theme::text_dim()));
                        });
                    }
                    ui.label(":");
                });
            } else {
                ui.label(format!("{}:", row.name));
            }
            ui.add_enabled_ui(live, |ui| {
                let mut rgb = style.color.unwrap_or(style.colors[0]);
                let several = style.colors.len() > 1;
                let hover = if several {
                    let hex: Vec<String> = style.colors.iter().map(|c| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])).collect();
                    format!("Color (one for all {} of this layer's colors: {})", style.colors.len(), hex.join(", "))
                } else {
                    "Color".to_string()
                };
                let swatch = ui.color_edit_button_srgb(&mut rgb);
                if several && style.color.is_none() && ui.is_rect_visible(swatch.rect) {
                    // Inside the button's 1 px border, as egui fills it.
                    let inner = swatch.rect.shrink(1.0);
                    let shown = &style.colors[..style.colors.len().min(MAX_STRIPES)];
                    let w = inner.width() / shown.len() as f32;
                    for (k, c) in shown.iter().enumerate() {
                        let x = inner.left() + w * k as f32;
                        let r = egui::Rect::from_min_max(egui::pos2(x, inner.top()), egui::pos2(x + w, inner.bottom()));
                        ui.painter().rect_filled(r, 0.0, Color32::from_rgb(c[0], c[1], c[2]));
                    }
                }
                if swatch.on_hover_text(hover).on_disabled_hover_text("Crossed out: its color stays as drawn").changed() {
                    style.color = Some(rgb);
                }
            });
            ui.add_enabled_ui(live, |ui| {
                let mut op = style.opacity * 100.0;
                if ui.add(egui::DragValue::new(&mut op).speed(1.0).range(0.0..=100.0).suffix("%")).on_hover_text("Opacity").changed() {
                    style.opacity = op / 100.0;
                }
            });
            ui.add_enabled_ui(live, |ui| {
                let hidden = style.opacity <= 0.0;
                if ui.small_button(if hidden { ph::EYE_SLASH } else { ph::EYE }).on_hover_text(if hidden { "Show" } else { "Hide" }).clicked() {
                    style.opacity = if hidden { 1.0 } else { 0.0 };
                }
            });
            if before.is_plain() {
                ui.label("");
            } else if ui.small_button(ph::ARROW_COUNTER_CLOCKWISE).on_hover_text("Its own color").clicked() {
                style.color = None;
                style.opacity = 1.0;
            }
            if edit {
                if !live {
                    if ui.small_button(ph::ARROW_U_UP_LEFT).on_hover_text("Bring it back as a layer").clicked() {
                        remove = Some((i, false));
                    }
                } else if style.colors.len() > 1 {
                    if ui.small_button(ph::ARROWS_SPLIT).on_hover_text("Split into one layer per color (a merged layer is split before it can be crossed out)").clicked() {
                        split = Some(i);
                    }
                } else if ui.small_button(ph::X).on_hover_text("Cross it out: not a layer, so its color stays as drawn").clicked() {
                    remove = Some((i, true));
                }
            }
            ui.end_row();
            changed |= *style != before;
        }
    });
    if let Some(i) = split {
        split_layer(rows, i);
        changed = true;
    }
    if let Some((i, removed)) = remove {
        remove_layer(rows, i, removed);
        changed = true;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fav(id: &str, title: &str, source: &str, svg: bool, taxa: Vec<crate::taxonomy::Taxon>, tags: &[&str]) -> AssetRow {
        AssetRow {
            id: id.into(),
            source: source.into(),
            source_name: String::new(),
            title: title.into(),
            variant: String::new(),
            category: "Viruses, Cells".into(),
            description: String::new(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            license: "CC0-1.0".into(),
            credit: String::new(),
            author: "Ann Smith".into(),
            png_key: String::new(),
            png_local: None,
            png_width: 1,
            png_height: 1,
            svg_key: svg.then(|| "x.svg".into()),
            stars: 0,
            starred: false,
            taxa,
            layers: Vec::new(),
            generative_ai_used: None,
            file_bytes: 0,
        }
    }

    #[test]
    fn filters_loaded_favorites() {
        let rows = vec![
            fav("a", "Neuron", "bioicons", true, vec![("Cells", "Cell types")], &["brain"]),
            fav("b", "Mitochondrion", "servier", false, vec![("Cells", "Organelles and cell components")], &[]),
        ];
        let ids = |q: &str, f: &SearchFilter| rows.iter().filter(|r| local_match(r, q, f, None)).map(|r| r.id.as_str()).collect::<Vec<_>>();
        let none = SearchFilter::default();
        assert_eq!(ids("", &none), ["a", "b"]);
        assert_eq!(ids("mito", &none), ["b"]);
        assert_eq!(ids("brain", &none), ["a"]);
        assert_eq!(ids("", &SearchFilter { formats: vec![crate::catalog::FORMAT_PNG.into()], ..none.clone() }), ["b"]);
        assert_eq!(ids("", &SearchFilter { cats: vec!["Cells".into()], subs: vec![("Cells".into(), "Cell types".into())], ..none.clone() }), ["a"]);
        assert_eq!(ids("", &SearchFilter { sources: vec!["servier".into()], ..none.clone() }), ["b"]);
        assert_eq!(ids("", &SearchFilter { db_cats: vec![("bioicons".into(), "cells".into())], ..none.clone() }), ["a"]);
        assert_eq!(ids("", &SearchFilter { authors: "smith".into(), tags: "Brain".into(), ..none.clone() }), ["a"]);
        // Each choice is counted under the other filters, not its own.
        let fc = local_facets(&rows, "", &SearchFilter { sources: vec!["servier".into()], ..none.clone() });
        assert_eq!(fc.sources, [("bioicons".to_string(), 1), ("servier".to_string(), 1)]);
        assert_eq!(fc.formats, [("PNG".to_string(), 1)]);
        assert!(fc.taxa.iter().any(|t| t.cat == "Cells" && t.sub.is_empty() && t.count == 1));
    }

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

    fn rows(colors: &[[u8; 3]]) -> Vec<EditLayer> {
        color_layers(colors).into_iter().map(|(n, c)| EditLayer::new(n, c)).collect()
    }

    fn names(rows: &[EditLayer]) -> Vec<&str> {
        rows.iter().map(|r| r.name.as_str()).collect()
    }

    #[test]
    fn uploads_leave_no_color_out() {
        // Every colour its own layer, past the eight a drawing nobody named offers.
        let colors: Vec<[u8; 3]> = (0..10u8).map(|i| [i * 25, 0, 255 - i * 25]).collect();
        let layers = upload_layers(Vec::new(), &colors);
        assert_eq!(layers.len(), 10);
        assert_eq!(layers[9], ("Layer 10".to_string(), vec![[225, 0, 30]]));
        // A catalog's named layers are kept, and a colour they missed added after them.
        let named = vec![("Ink".to_string(), vec![[0, 0, 0]]), ("Paper".to_string(), vec![[255, 255, 255], [240, 240, 240]])];
        let layers = upload_layers(named, &[[0, 0, 0], [255, 255, 255], [240, 240, 240], [9, 9, 200]]);
        assert_eq!(layers.iter().map(|l| l.0.as_str()).collect::<Vec<_>>(), ["Ink", "Paper", "Layer 3"]);
        // One colour: no layers.
        assert!(upload_layers(Vec::new(), &[[1, 2, 3]]).is_empty());
    }

    #[test]
    fn layers_merge_and_split() {
        let mut r = rows(&[[0, 136, 255], [255, 0, 0], [255, 102, 0], [27, 31, 46]]);
        r[1].name = "Blood".into();
        r[1].picked = true;
        r[3].picked = true;
        merge_picked(&mut r);
        assert_eq!(names(&r), ["Layer 1", "Blood", "Layer 3"]);
        assert_eq!(r[1].style.colors, vec![[255, 0, 0], [27, 31, 46]]);
        assert!(r.iter().all(|r| !r.picked));
        split_layer(&mut r, 1);
        assert_eq!(names(&r), ["Layer 1", "Blood", "Blood 2", "Layer 4"]);
        assert_eq!(r[2].style.colors, vec![[27, 31, 46]]);
        // A numbered layer's parts are numbered too.
        r[0].style.colors.push([1, 2, 3]);
        split_layer(&mut r, 0);
        assert_eq!(names(&r), ["Layer 1", "Layer 2", "Blood", "Blood 2", "Layer 5"]);
    }

    #[test]
    fn similar_colors_merge() {
        // The near-blacks and the near-whites, not the light grey-blue or the bright colours.
        let mut r = rows(&[[0, 136, 255], [255, 0, 0], [255, 102, 0], [27, 31, 46], [10, 18, 38], [255, 255, 255], [241, 243, 248], [203, 213, 225]]);
        assert!(merge_similar(&mut r));
        assert_eq!(r.len(), 6);
        assert_eq!(r[3].style.colors, vec![[27, 31, 46], [10, 18, 38]]);
        assert_eq!(r[4].style.colors, vec![[255, 255, 255], [241, 243, 248]]);
        assert_eq!(names(&r), ["Layer 1", "Layer 2", "Layer 3", "Layer 4", "Layer 5", "Layer 6"]);
        assert!(!merge_similar(&mut r));
        assert!(same_colors(&[[1, 1, 1], [2, 2, 2]], &[[2, 2, 2], [1, 1, 1]]));
        assert!(!same_colors(&[[1, 1, 1]], &[[1, 1, 1], [2, 2, 2]]));
    }

    #[test]
    fn layers_cross_out_and_unmerge() {
        let mut r = rows(&[[0, 136, 255], [255, 0, 0], [255, 102, 0], [27, 31, 46]]);
        r[1].style.color = Some([1, 2, 3]);
        remove_layer(&mut r, 1, true);
        // Its colour as drawn, and the numbered layers after it one lower.
        assert!(r[1].style.is_plain());
        assert_eq!(names(&r), ["Layer 1", "Layer 2", "Layer 2", "Layer 3"]);
        assert_eq!(kept(&r), 3);
        // Crossed out: not merged, nor merged into.
        r[1].picked = true;
        r[2].picked = true;
        merge_picked(&mut r);
        assert_eq!(r.len(), 4);
        remove_layer(&mut r, 1, false);
        assert_eq!(names(&r), ["Layer 1", "Layer 2", "Layer 3", "Layer 4"]);
        // Unmerge all splits every merged layer back.
        r[0].picked = true;
        r[3].picked = true;
        merge_picked(&mut r);
        r[1].picked = true;
        r[2].picked = true;
        merge_picked(&mut r);
        assert_eq!(r.len(), 2);
        unmerge_all(&mut r);
        assert_eq!(r.iter().map(|r| r.style.colors.len()).collect::<Vec<_>>(), [1, 1, 1, 1]);
        assert_eq!(names(&r), ["Layer 1", "Layer 2", "Layer 3", "Layer 4"]);
    }

    #[test]
    fn crossed_out_layers_are_not_similar() {
        let mut r = rows(&[[255, 255, 255], [241, 243, 248], [0, 0, 0]]);
        remove_layer(&mut r, 1, true);
        assert!(!merge_similar(&mut r));
    }
}
