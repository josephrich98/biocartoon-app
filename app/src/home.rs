//! Home screen: every saved project as tiles or a list, with search, sorting and a per-project
//! menu (rename, copy, version history, export, email, add to community, delete).

use crate::app::{Action, BioCartoon};
use crate::assets::ImgState;
use crate::projects::{format_date, format_time, ProjectMeta, Version};
use crate::theme;
use egui::{Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke, StrokeKind, Vec2};
use egui_phosphor::regular as ph;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum SortKey {
    LastOpened,
    LastModified,
    Name,
    Created,
}

impl SortKey {
    pub const ALL: [SortKey; 4] = [SortKey::LastOpened, SortKey::LastModified, SortKey::Name, SortKey::Created];
    pub fn label(self) -> &'static str {
        match self {
            SortKey::LastOpened => "Last opened",
            SortKey::LastModified => "Last modified",
            SortKey::Name => "Name",
            SortKey::Created => "Date created",
        }
    }
    /// The natural direction when this key is picked: A to Z for names, newest first for dates.
    pub fn default_ascending(self) -> bool {
        self == SortKey::Name
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum HomeView {
    LargeTiles,
    SmallTiles,
    List,
}

impl HomeView {
    pub const ALL: [HomeView; 3] = [HomeView::LargeTiles, HomeView::SmallTiles, HomeView::List];
    fn icon(self) -> &'static str {
        match self {
            HomeView::LargeTiles => ph::SQUARES_FOUR,
            HomeView::SmallTiles => ph::GRID_NINE,
            HomeView::List => ph::LIST,
        }
    }
    fn label(self) -> &'static str {
        match self {
            HomeView::LargeTiles => "Large tiles",
            HomeView::SmallTiles => "Small tiles",
            HomeView::List => "List",
        }
    }
}

/// Per-project commands from the "..." menu.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProjectCmd {
    Rename,
    Copy,
    History,
    Export,
    SaveFile,
    Email,
    Community,
    Delete,
}

impl ProjectCmd {
    const MENU: [ProjectCmd; 8] = [
        ProjectCmd::Rename,
        ProjectCmd::Copy,
        ProjectCmd::History,
        ProjectCmd::Export,
        ProjectCmd::SaveFile,
        ProjectCmd::Email,
        ProjectCmd::Community,
        ProjectCmd::Delete,
    ];
    fn label(self) -> &'static str {
        match self {
            ProjectCmd::Rename => "Rename",
            ProjectCmd::Copy => "Make a copy",
            ProjectCmd::History => "Version history",
            ProjectCmd::Export => "Export",
            ProjectCmd::SaveFile => "Save as .biocartoon file",
            ProjectCmd::Email => "Email",
            ProjectCmd::Community => "Add to community",
            ProjectCmd::Delete => "Delete",
        }
    }
    fn icon(self) -> &'static str {
        match self {
            ProjectCmd::Rename => ph::PENCIL_SIMPLE,
            ProjectCmd::Copy => ph::COPY,
            ProjectCmd::History => ph::CLOCK_COUNTER_CLOCKWISE,
            ProjectCmd::Export => ph::DOWNLOAD_SIMPLE,
            ProjectCmd::SaveFile => ph::FILE_ARROW_DOWN,
            ProjectCmd::Email => ph::ENVELOPE_SIMPLE,
            ProjectCmd::Community => ph::GLOBE,
            ProjectCmd::Delete => ph::TRASH,
        }
    }
}

pub struct VersionHistory {
    pub id: String,
    pub name: String,
    pub versions: Vec<Version>,
    pub selected: Option<i64>,
}

#[derive(Default)]
pub struct HomeState {
    pub query: String,
    pub projects: Vec<ProjectMeta>,
    pub renaming: Option<(String, String)>,
    pub confirm_delete: Option<ProjectMeta>,
    pub history: Option<VersionHistory>,
    /// Project ids whose missing thumbnail was already requested this session.
    pub thumbs_tried: std::collections::HashSet<String>,
    /// The account menu's popup, so the backup reminder can open it.
    pub account_popup: Option<egui::Id>,
    pub confirm_account_delete: bool,
}

const GAP: f32 = 24.0;
const INFO_H: f32 = 78.0;

impl BioCartoon {
    pub fn refresh_projects(&mut self) {
        self.home.projects = self.store.list();
    }

    /// Projects matching the search, in the chosen order.
    fn visible_projects(&self) -> Vec<ProjectMeta> {
        let q = self.home.query.trim().to_lowercase();
        let mut v: Vec<ProjectMeta> = self.home.projects.iter().filter(|p| q.is_empty() || p.name.to_lowercase().contains(&q)).cloned().collect();
        let key = self.settings.home_sort;
        v.sort_by(|a, b| {
            let o = match key {
                SortKey::LastOpened => a.opened.cmp(&b.opened),
                SortKey::LastModified => a.modified.cmp(&b.modified),
                SortKey::Created => a.created.cmp(&b.created),
                SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            };
            o.then_with(|| a.id.cmp(&b.id))
        });
        if !self.settings.home_ascending {
            v.reverse();
        }
        v
    }

    fn date_label(&self, p: &ProjectMeta) -> String {
        match self.settings.home_sort {
            SortKey::LastModified => format!("Edited {}", format_date(p.modified)),
            SortKey::Created => format!("Created {}", format_date(p.created)),
            SortKey::LastOpened | SortKey::Name => format!("Opened {}", format_date(p.opened)),
        }
    }

    pub fn home_ui(&mut self, ui: &mut egui::Ui) {
        self.home_top_bar(ui);
        let frame = egui::Frame::new().fill(theme::bg()).inner_margin(egui::Margin { left: 16, right: 16, top: 8, bottom: 16 });
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            let projects = self.visible_projects();
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                self.backup_banner(ui);
                match self.settings.home_view {
                    HomeView::List => self.project_list(ui, &projects),
                    v => self.project_grid(ui, &projects, if v == HomeView::LargeTiles { 300.0 } else { 190.0 }),
                }
                if projects.is_empty() && !self.home.query.trim().is_empty() {
                    ui.add_space(24.0);
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new(format!("No projects match \"{}\"", self.home.query.trim())).color(theme::text_dim()));
                    });
                }
            });
        });
        self.home_windows(ui.ctx());
    }

    fn home_top_bar(&mut self, ui: &mut egui::Ui) {
        let frame = egui::Frame::new().fill(theme::bg()).inner_margin(egui::Margin { left: 16, right: 16, top: 14, bottom: 8 });
        let web = cfg!(target_arch = "wasm32");
        let right_w = 470.0 + if web { 130.0 } else { 0.0 };
        // Narrow (a phone): two rows, the logo, search and account on top, the sort, view and other
        // buttons below. In one row the search would shrink to nothing and the rest run off screen.
        let narrow = ui.available_width() < 32.0 + 150.0 + right_w + 200.0;
        let panel = egui::Panel::top("home_top").frame(frame).show_separator_line(false);
        // Narrow, the panel takes the height of its rows, which wrap onto a third on a phone.
        let panel = if narrow { panel } else { panel.exact_size(66.0) };
        panel.show(ui, |ui| {
            if narrow {
                ui.horizontal(|ui| {
                    ui.add(crate::logo(ui.ctx(), 30.0));
                    ui.add_space(4.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if web {
                            self.account_button(ui);
                            ui.add_space(8.0);
                        }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            let w = ui.available_width() - ui.spacing().item_spacing.x;
                            self.home_search(ui, w);
                        });
                    });
                });
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.y = 8.0;
                    self.home_sort(ui);
                    ui.add_space(8.0);
                    self.home_view_toggle(ui);
                    ui.add_space(8.0);
                    self.home_icon_buttons(ui);
                });
                return;
            }
            ui.horizontal_centered(|ui| {
                ui.add(crate::logo(ui.ctx(), 30.0));
                ui.label(egui::RichText::new("BioCartoon").size(16.0).strong());
                ui.add_space(12.0);
                let search_w = (ui.available_width() - right_w).max(160.0);
                self.home_search(ui, search_w);
                ui.add_space(8.0);
                self.home_sort(ui);
                ui.add_space(8.0);
                self.home_view_toggle(ui);
                ui.add_space(8.0);
                self.home_icon_buttons(ui);
                if web {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| self.account_button(ui));
                }
            });
        });
    }

    fn home_search(&mut self, ui: &mut egui::Ui, width: f32) {
        egui::Frame::new().fill(theme::panel()).stroke(Stroke::new(1.0, theme::border())).corner_radius(8).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
            ui.set_width(width - 22.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(ph::MAGNIFYING_GLASS).size(16.0).color(theme::text_dim()));
                ui.add(egui::TextEdit::singleline(&mut self.home.query).hint_text("Search all projects").frame(egui::Frame::NONE).desired_width(f32::INFINITY));
            });
        });
    }

    /// Sort key + direction.
    fn home_sort(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new().fill(theme::panel()).stroke(Stroke::new(1.0, theme::border())).corner_radius(8).inner_margin(egui::Margin::symmetric(6, 3)).show(ui, |ui| {
            ui.horizontal(|ui| {
                let cur = self.settings.home_sort;
                ui.menu_button(format!("{}  {}", cur.label(), ph::CARET_DOWN), |ui| {
                    for k in SortKey::ALL {
                        if ui.selectable_label(k == cur, k.label()).clicked() {
                            if k != cur {
                                self.settings.home_sort = k;
                                self.settings.home_ascending = k.default_ascending();
                            }
                            ui.close();
                        }
                    }
                });
                ui.separator();
                let asc = self.settings.home_ascending;
                let tip = match (self.settings.home_sort, asc) {
                    (SortKey::Name, true) => "A to Z",
                    (SortKey::Name, false) => "Z to A",
                    (_, true) => "Oldest first",
                    (_, false) => "Newest first",
                };
                if ui.add(egui::Button::new(egui::RichText::new(if asc { ph::ARROW_UP } else { ph::ARROW_DOWN }).size(16.0)).frame(false)).on_hover_text(tip).clicked() {
                    self.settings.home_ascending = !asc;
                }
            });
        });
    }

    fn home_view_toggle(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new().fill(theme::panel()).stroke(Stroke::new(1.0, theme::border())).corner_radius(8).inner_margin(egui::Margin::symmetric(4, 3)).show(ui, |ui| {
            ui.horizontal(|ui| {
                for v in HomeView::ALL {
                    let sel = self.settings.home_view == v;
                    let text = egui::RichText::new(v.icon()).size(18.0).color(if sel { theme::accent() } else { theme::text() });
                    if ui.add(egui::Button::new(text).frame(false).selected(sel)).on_hover_text(v.label()).clicked() {
                        self.settings.home_view = v;
                    }
                }
            });
        });
    }

    /// Open, Settings and About.
    fn home_icon_buttons(&mut self, ui: &mut egui::Ui) {
        for b in 0..3 {
            let (icon, tip) = match b {
                0 => (ph::FOLDER_OPEN, "Open .biocartoon files, or a backup .zip, as new projects"),
                1 => (ph::GEAR, "Settings"),
                _ => (ph::INFO, "About BioCartoon"),
            };
            if ui.add(egui::Button::new(egui::RichText::new(icon).size(18.0)).frame(false)).on_hover_text(tip).clicked() {
                match b {
                    0 => self.actions.push(Action::Open),
                    1 => self.open_settings(crate::app::SettingsTab::General),
                    _ => self.show_about = true,
                }
            }
        }
    }

    /// Web: a reminder to download a backup while any project has changed since the last one.
    /// The browser holds the only copy, and it goes with the site's data.
    fn backup_banner(&mut self, ui: &mut egui::Ui) {
        if !cfg!(target_arch = "wasm32") {
            return;
        }
        let now = crate::projects::now_ms();
        let changed = self.home.projects.iter().filter(|p| p.modified > p.backed_up).count();
        // Signed in, the online copy takes the place of downloaded backups.
        if changed == 0 || now < self.settings.backup_snoozed_until || self.account.signed_in() {
            return;
        }
        let what = if changed == 1 { "1 project has".to_string() } else { format!("{changed} projects have") };
        let risk = if crate::storage::persisted() == Some(true) {
            "Projects live only in this browser, and are lost if its site data is cleared."
        } else if crate::storage::is_safari() {
            "Projects live only in this browser, which may clear them (Safari does after 7 days without a visit)."
        } else {
            "Projects live only in this browser, which may clear them if the device runs low on space."
        };
        let message = format!("{what} changed since your last backup. {risk}");
        // Narrow (a phone): the message on top, the buttons wrapping below it. Side by side, the
        // buttons would leave the message no room and it would wrap a few letters to a line.
        let narrow = ui.available_width() < 640.0;
        let margin = if narrow { egui::Margin::symmetric(10, 10) } else { egui::Margin::symmetric(14, 10) };
        egui::Frame::new().fill(theme::panel()).stroke(Stroke::new(1.0, theme::accent())).corner_radius(8).inner_margin(margin).show(ui, |ui| {
            ui.set_width(ui.available_width());
            if narrow {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(ph::WARNING_CIRCLE).size(18.0).color(theme::accent()));
                    ui.add(egui::Label::new(&message).wrap());
                });
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    self.backup_banner_buttons(ui, now, false);
                });
                return;
            }
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(ph::WARNING_CIRCLE).size(18.0).color(theme::accent()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.backup_banner_buttons(ui, now, true);
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true), |ui| {
                        ui.label(message);
                    });
                });
            });
        });
        ui.add_space(12.0);
    }

    /// The backup banner's buttons, in reading order ("Download a backup" first) or, for a
    /// right-to-left row, reversed so that it still reads that way.
    fn backup_banner_buttons(&mut self, ui: &mut egui::Ui, now: i64, rtl: bool) {
        let order: [u8; 3] = if rtl { [2, 1, 0] } else { [0, 1, 2] };
        for b in order {
            match b {
                0 => {
                    if ui.add(egui::Button::new(egui::RichText::new(format!("{}  Download a backup", ph::DOWNLOAD_SIMPLE)).color(Color32::WHITE)).fill(theme::accent()))
                        .on_hover_text("Every project as a .biocartoon file, in one .zip. Open the .zip here to restore them.")
                        .clicked()
                    {
                        self.actions.push(Action::BackupAll);
                    }
                }
                1 => {
                    if ui.button(format!("{}  Keep them online", ph::CLOUD)).on_hover_text("Optional: sign in, and a copy of every project is kept on the BioCartoon server").clicked() {
                        if let Some(id) = self.home.account_popup {
                            egui::Popup::open_id(ui.ctx(), id);
                        }
                    }
                }
                _ => {
                    if ui.button("Remind me in a week").clicked() {
                        self.settings.backup_snoozed_until = now + 7 * 24 * 3600 * 1000;
                    }
                }
            }
        }
    }

    /// Web: the account button in the top right corner ("Sign in", or the sync status and name once
    /// signed in) and its menu (sign in, sync status, sign out, delete).
    fn account_button(&mut self, ui: &mut egui::Ui) {
        let local = self.home.projects.len();
        let pending = self.account.pending(&self.home.projects);
        let (icon, color, tip) = match &self.account.saved {
            None => (ph::USER_CIRCLE, theme::text(), "Account (optional): keep your projects online".to_string()),
            Some(_) if self.account.error.is_some() => (ph::CLOUD_WARNING, Color32::from_rgb(0xe0, 0x8a, 0x1e), "Account: there was a problem saving online".to_string()),
            Some(_) if pending > 0 => (ph::CLOUD_ARROW_UP, theme::text(), format!("Account: saving {pending} project{} online", if pending == 1 { "" } else { "s" })),
            Some(_) => (ph::CLOUD_CHECK, theme::accent(), "Account: every project is saved online".to_string()),
        };
        let btn = match &self.account.saved {
            None => {
                let text = egui::RichText::new(format!("{}  Sign in", ph::USER_CIRCLE)).color(theme::accent());
                ui.add(egui::Button::new(text).fill(theme::panel()).stroke(Stroke::new(1.0, theme::accent())).corner_radius(8)).on_hover_text(tip)
            }
            Some(s) => {
                let who: String = s.name.split_whitespace().next().unwrap_or("Account").chars().take(14).collect();
                let text = egui::RichText::new(format!("{icon}  {who}")).color(color);
                ui.add(egui::Button::new(text).fill(theme::panel()).stroke(Stroke::new(1.0, theme::border())).corner_radius(8)).on_hover_text(tip)
            }
        };
        self.home.account_popup = Some(egui::Popup::default_response_id(&btn));
        let mut cmd: Option<&str> = None;
        egui::Popup::menu(&btn).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            ui.set_width(320.0);
            let dim = |t: &str| egui::RichText::new(t).color(theme::text_dim());
            match &self.account.saved {
                None => {
                    ui.label(egui::RichText::new("Keep your projects online").strong().size(15.0));
                    ui.add_space(4.0);
                    ui.label(dim("Optional. Sign in, and a copy of every project is kept on the BioCartoon server, so it is safe even if this browser clears its data, and comes back wherever you sign in."));
                    ui.add_space(8.0);
                    if ui.add(egui::Button::new(format!("{}  Sign in with Google", ph::GOOGLE_LOGO)).min_size(Vec2::new(0.0, 30.0))).clicked() {
                        cmd = Some("google");
                    }
                    if ui.add(egui::Button::new(format!("{}  Sign in with GitHub", ph::GITHUB_LOGO)).min_size(Vec2::new(0.0, 30.0))).clicked() {
                        cmd = Some("github");
                    }
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(dim("Only your name, email and projects are stored. You can delete them any time.").size(12.0));
                        ui.hyperlink_to(egui::RichText::new("Privacy").size(12.0), "https://biocartoon.com/privacy");
                    });
                }
                Some(s) => {
                    let who = if s.name.is_empty() { s.email.clone() } else { s.name.clone() };
                    ui.label(egui::RichText::new(if who.is_empty() { "Signed in".to_string() } else { who }).strong().size(15.0));
                    let via = match s.provider.as_str() {
                        "google" => "Google",
                        "github" => "GitHub",
                        _ => "",
                    };
                    if !via.is_empty() {
                        ui.label(dim(&if s.email.is_empty() || s.name.is_empty() { format!("Signed in with {via}") } else { format!("{} · {via}", s.email) }));
                    }
                    ui.add_space(6.0);
                    let status = match (&self.account.error, &self.account.cloud) {
                        (Some(e), _) => format!("{}  {e}", ph::CLOUD_WARNING),
                        (None, None) => format!("{}  Checking your online copies...", ph::CLOUD),
                        (None, Some(_)) if pending > 0 => format!("{}  Saving {pending} change{} online...", ph::CLOUD_ARROW_UP, if pending == 1 { "" } else { "s" }),
                        (None, Some(_)) => format!("{}  All {local} project{} saved online", ph::CLOUD_CHECK, if local == 1 { "" } else { "s" }),
                    };
                    ui.label(status);
                    if self.account.quota > 0 {
                        let mb = |b: i64| b as f64 / (1024.0 * 1024.0);
                        ui.label(dim(&format!("{:.1} MB of {:.0} MB used", mb(self.account.used), mb(self.account.quota))).size(12.0));
                    }
                    ui.add_space(4.0);
                    ui.separator();
                    if ui.add(egui::Button::new(format!("{}  Sign out", ph::SIGN_OUT)).frame(false).min_size(Vec2::new(0.0, 28.0))).clicked() {
                        cmd = Some("sign_out");
                    }
                    let del = egui::RichText::new(format!("{}  Delete account...", ph::TRASH)).color(Color32::from_rgb(0xf0, 0x6a, 0x6a));
                    if ui.add(egui::Button::new(del).frame(false).min_size(Vec2::new(0.0, 28.0))).clicked() {
                        cmd = Some("delete");
                    }
                }
            }
            if cmd.is_some() {
                ui.close();
            }
        });
        match cmd {
            Some("sign_out") => self.account_sign_out(),
            Some("delete") => self.home.confirm_account_delete = true,
            Some(p) => self.account_sign_in(p),
            None => {}
        }
    }

    fn project_grid(&mut self, ui: &mut egui::Ui, projects: &[ProjectMeta], min_w: f32) {
        let avail = ui.available_width();
        let cols = (((avail + GAP) / (min_w + GAP)).floor() as usize).max(1);
        let tile_w = (avail - GAP * (cols - 1) as f32) / cols as f32;
        let thumb_h = (tile_w * 0.62).round();
        let tile = Vec2::new(tile_w, thumb_h + INFO_H);
        let n = projects.len() + 1; // "+ New" first
        let rows = n.div_ceil(cols);
        let origin = ui.cursor().min;
        let (_, painter_rect) = ui.allocate_space(Vec2::new(avail, rows as f32 * tile.y + (rows.saturating_sub(1)) as f32 * GAP));
        let _ = painter_rect;
        for i in 0..n {
            let (r, c) = (i / cols, i % cols);
            let rect = Rect::from_min_size(origin + Vec2::new(c as f32 * (tile_w + GAP), r as f32 * (tile.y + GAP)), tile);
            if !ui.is_rect_visible(rect) {
                continue;
            }
            if i == 0 {
                self.new_tile(ui, rect, thumb_h);
            } else {
                self.project_tile(ui, rect, thumb_h, &projects[i - 1]);
            }
        }
    }

    fn new_tile(&mut self, ui: &mut egui::Ui, rect: Rect, thumb_h: f32) {
        let resp = ui.interact(rect, ui.id().with("new_tile"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        let p = ui.painter();
        let hovered = resp.hovered();
        p.rect(rect, CornerRadius::same(8), if hovered { theme::panel_raised() } else { theme::panel() }, Stroke::new(1.5, if hovered { theme::accent() } else { theme::border() }), StrokeKind::Inside);
        let c = Rect::from_min_size(rect.min, Vec2::new(rect.width(), thumb_h)).center();
        p.text(c, Align2::CENTER_CENTER, ph::PLUS, FontId::proportional(44.0), theme::accent());
        let name_pos = egui::pos2(rect.min.x + 18.0, rect.min.y + thumb_h + 22.0);
        p.text(name_pos, Align2::LEFT_CENTER, "New", FontId::proportional(18.0), theme::text());
        p.text(name_pos + Vec2::new(0.0, 26.0), Align2::LEFT_CENTER, "Start a blank project", FontId::proportional(14.0), theme::text_dim());
        if resp.clicked() {
            self.actions.push(Action::NewDoc);
        }
    }

    /// Thumbnail texture for a project, requesting a render if it has none yet.
    fn project_thumb(&mut self, id: &str) -> Option<(egui::TextureHandle, [usize; 2])> {
        let path = self.store.thumb_path(id);
        if crate::storage::exists(&path) {
            if let ImgState::Ready { tex, size } = self.images.file(&path.display().to_string()) {
                return Some((tex.clone(), *size));
            }
        } else if self.home.thumbs_tried.insert(id.to_string()) {
            if let Ok(doc) = self.store.load(id) {
                self.request_thumb(id, &doc);
            }
        }
        None
    }

    fn project_tile(&mut self, ui: &mut egui::Ui, rect: Rect, thumb_h: f32, p: &ProjectMeta) {
        let id = ui.id().with(("tile", &p.id));
        let resp = ui.interact(rect, id, Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        let hovered = resp.hovered();
        let thumb_rect = Rect::from_min_size(rect.min, Vec2::new(rect.width(), thumb_h));
        let thumb = self.project_thumb(&p.id);
        let painter = ui.painter();
        painter.rect(rect, CornerRadius::same(8), theme::panel(), Stroke::NONE, StrokeKind::Inside);
        painter.rect_filled(thumb_rect, CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 }, Color32::WHITE);
        if let Some((tex, size)) = thumb {
            let inner = thumb_rect.shrink(12.0);
            let s = (inner.width() / size[0] as f32).min(inner.height() / size[1] as f32);
            let r = Rect::from_center_size(inner.center(), Vec2::new(size[0] as f32, size[1] as f32) * s);
            painter.image(tex.id(), r, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
        }
        badge(painter, thumb_rect.min + Vec2::splat(12.0), p.slides);
        painter.rect_stroke(rect, CornerRadius::same(8), Stroke::new(if hovered { 1.5 } else { 1.0 }, if hovered { theme::accent() } else { theme::border() }), StrokeKind::Inside);

        let left = rect.min.x + 18.0;
        let name_y = rect.min.y + thumb_h + 22.0;
        let max_w = rect.width() - 36.0;
        let name = elide(ui, &p.name, FontId::proportional(18.0), max_w);
        painter.text(egui::pos2(left, name_y), Align2::LEFT_CENTER, name, FontId::proportional(18.0), theme::text());
        painter.text(egui::pos2(left, name_y + 26.0), Align2::LEFT_CENTER, self.date_label(p), FontId::proportional(14.0), theme::text_dim());

        let dots = Rect::from_center_size(egui::pos2(rect.max.x - 28.0, name_y + 26.0), Vec2::new(32.0, 24.0));
        self.project_menu(ui, dots, p, &resp);
        if resp.clicked() {
            self.actions.push(Action::OpenProject(p.id.clone()));
        }
        if resp.hovered() {
            resp.on_hover_text(format!("Opened {}\nEdited {}\nCreated {}", format_time(p.opened), format_time(p.modified), format_time(p.created)));
        }
    }

    /// The "..." button (and the same menu on right-click of the whole tile / row).
    fn project_menu(&mut self, ui: &mut egui::Ui, dots: Rect, p: &ProjectMeta, row: &egui::Response) {
        let btn = ui.put(dots, egui::Button::new(egui::RichText::new(ph::DOTS_THREE).size(22.0)).frame(false)).on_hover_text("More");
        let mut cmd = None;
        let mut items = |ui: &mut egui::Ui| {
            ui.set_min_width(210.0);
            for c in ProjectCmd::MENU {
                if c == ProjectCmd::Delete {
                    ui.separator();
                }
                let text = egui::RichText::new(format!("{}   {}", c.icon(), c.label())).size(15.0);
                let text = if c == ProjectCmd::Delete { text.color(Color32::from_rgb(0xf0, 0x6a, 0x6a)) } else { text };
                if ui.add(egui::Button::new(text).frame(false).min_size(Vec2::new(0.0, 30.0))).clicked() {
                    cmd = Some(c);
                    ui.close();
                }
            }
        };
        egui::Popup::menu(&btn).show(&mut items);
        egui::Popup::context_menu(row).show(&mut items);
        if let Some(c) = cmd {
            self.project_cmd(c, p);
        }
    }

    pub fn project_cmd(&mut self, c: ProjectCmd, p: &ProjectMeta) {
        match c {
            ProjectCmd::Rename => self.home.renaming = Some((p.id.clone(), p.name.clone())),
            ProjectCmd::Copy => self.actions.push(Action::CopyProject(p.id.clone())),
            ProjectCmd::History => self.open_version_history(&p.id),
            ProjectCmd::Export => self.actions.push(Action::ExportProject(p.id.clone())),
            ProjectCmd::SaveFile => self.actions.push(Action::SaveProjectFile(p.id.clone())),
            ProjectCmd::Email => self.actions.push(Action::EmailProject(p.id.clone())),
            ProjectCmd::Community => self.toast("Sharing to the community is not available yet"),
            ProjectCmd::Delete => self.home.confirm_delete = Some(p.clone()),
        }
    }

    fn project_list(&mut self, ui: &mut egui::Ui, projects: &[ProjectMeta]) {
        const ROW_H: f32 = 64.0;
        let w = ui.available_width();
        // Date columns: all three, or on a narrow screen just the one the list is sorted by.
        let dates: &[usize] = if w >= 560.0 {
            &[1, 2, 3]
        } else {
            match self.settings.home_sort {
                SortKey::LastModified => &[2],
                SortKey::Created => &[3],
                SortKey::LastOpened | SortKey::Name => &[1],
            }
        };
        let cols: Vec<f32> = if dates.len() == 3 { vec![w * 0.40, w * 0.18, w * 0.18, w * 0.18] } else { vec![w - 96.0 - 160.0, 110.0] };
        let x0 = ui.cursor().min.x;
        let x = |i: usize| x0 + 96.0 + cols[..i].iter().sum::<f32>();
        // Header
        let (hdr, _) = ui.allocate_exact_size(Vec2::new(w, 28.0), Sense::hover());
        let headers = ["Name", "Last opened", "Last modified", "Created"];
        for (i, &k) in std::iter::once(&0).chain(dates).enumerate() {
            let active = matches!((k, self.settings.home_sort), (0, SortKey::Name) | (1, SortKey::LastOpened) | (2, SortKey::LastModified) | (3, SortKey::Created));
            ui.painter().text(egui::pos2(x(i), hdr.center().y), Align2::LEFT_CENTER, headers[k], FontId::proportional(13.0), if active { theme::text() } else { theme::text_dim() });
        }
        ui.painter().hline(hdr.x_range(), hdr.max.y, Stroke::new(1.0, theme::border()));

        // "+ New" row
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, ROW_H), Sense::click());
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(6), theme::panel());
        }
        let thumb = Rect::from_min_size(rect.min + Vec2::new(8.0, 10.0), Vec2::new(72.0, ROW_H - 20.0));
        ui.painter().rect(thumb, CornerRadius::same(4), theme::panel_raised(), Stroke::new(1.0, theme::border()), StrokeKind::Inside);
        ui.painter().text(thumb.center(), Align2::CENTER_CENTER, ph::PLUS, FontId::proportional(24.0), theme::accent());
        ui.painter().text(egui::pos2(x(0), rect.center().y), Align2::LEFT_CENTER, "New", FontId::proportional(16.0), theme::text());
        if resp.clicked() {
            self.actions.push(Action::NewDoc);
        }

        for p in projects {
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, ROW_H), Sense::click());
            if !ui.is_rect_visible(rect) {
                continue;
            }
            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            if resp.hovered() {
                ui.painter().rect_filled(rect, CornerRadius::same(6), theme::panel());
            }
            ui.painter().hline(rect.x_range(), rect.min.y, Stroke::new(1.0, theme::panel_raised()));
            let thumb = Rect::from_min_size(rect.min + Vec2::new(8.0, 10.0), Vec2::new(72.0, ROW_H - 20.0));
            let tex = self.project_thumb(&p.id);
            let painter = ui.painter();
            painter.rect_filled(thumb, CornerRadius::same(4), Color32::WHITE);
            if let Some((tex, size)) = tex {
                let inner = thumb.shrink(3.0);
                let s = (inner.width() / size[0] as f32).min(inner.height() / size[1] as f32);
                let r = Rect::from_center_size(inner.center(), Vec2::new(size[0] as f32, size[1] as f32) * s);
                painter.image(tex.id(), r, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
            }
            let y = rect.center().y;
            let name = elide(ui, &p.name, FontId::proportional(16.0), cols[0] - 16.0);
            let painter = ui.painter();
            painter.text(egui::pos2(x(0), y), Align2::LEFT_CENTER, name, FontId::proportional(16.0), theme::text());
            let times = [p.opened, p.modified, p.created];
            for (i, &k) in dates.iter().enumerate() {
                painter.text(egui::pos2(x(i + 1), y), Align2::LEFT_CENTER, format_date(times[k - 1]), FontId::proportional(14.0), theme::text_dim());
            }
            let dots = Rect::from_center_size(egui::pos2(rect.max.x - 28.0, y), Vec2::new(32.0, 24.0));
            self.project_menu(ui, dots, p, &resp);
            if resp.clicked() {
                self.actions.push(Action::OpenProject(p.id.clone()));
            }
        }
    }

    fn home_windows(&mut self, ctx: &egui::Context) {
        // Rename
        if let Some((id, mut name)) = self.home.renaming.take() {
            let mut done = false;
            let mut cancel = false;
            let modal = egui::Modal::new(egui::Id::new("rename_modal")).show(ctx, |ui| {
                ui.set_width(360.0);
                ui.heading("Rename project");
                ui.add_space(8.0);
                let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY));
                if !r.has_focus() && !r.lost_focus() {
                    r.request_focus();
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    done = true;
                }
                ui.add_space(10.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(egui::Button::new(egui::RichText::new("Rename").color(Color32::WHITE)).fill(theme::accent())).clicked() {
                        done = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
            if done && !name.trim().is_empty() {
                self.actions.push(Action::RenameProject(id, name.trim().to_string()));
            } else if !(cancel || modal.should_close() || done) {
                self.home.renaming = Some((id, name));
            }
        }

        // Delete
        if let Some(p) = self.home.confirm_delete.take() {
            let mut go = false;
            let mut cancel = false;
            let modal = egui::Modal::new(egui::Id::new("delete_modal")).show(ctx, |ui| {
                ui.set_width(380.0);
                ui.heading("Delete project?");
                ui.add_space(6.0);
                ui.label(format!("\"{}\" and its version history will be permanently deleted.", p.name));
                ui.add_space(10.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(egui::Button::new(egui::RichText::new("Delete").color(Color32::WHITE)).fill(Color32::from_rgb(0xc0, 0x39, 0x2b))).clicked() {
                        go = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
            if go {
                self.actions.push(Action::DeleteProject(p.id.clone()));
            } else if !(cancel || modal.should_close()) {
                self.home.confirm_delete = Some(p);
            }
        }

        // Delete account
        if self.home.confirm_account_delete {
            let mut go = false;
            let mut cancel = false;
            let modal = egui::Modal::new(egui::Id::new("delete_account_modal")).show(ctx, |ui| {
                ui.set_width(400.0);
                ui.heading("Delete your account?");
                ui.add_space(6.0);
                ui.label("The online copy of every project is deleted from the BioCartoon server, along with your name and email. Projects in this browser stay here.");
                ui.add_space(10.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(egui::Button::new(egui::RichText::new("Delete account").color(Color32::WHITE)).fill(Color32::from_rgb(0xc0, 0x39, 0x2b))).clicked() {
                        go = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
            if go {
                self.account_delete();
            }
            if go || cancel || modal.should_close() {
                self.home.confirm_account_delete = false;
            }
        }

        self.version_history_window(ctx);
    }

    /// Dialogs opened from the editor's menu.
    pub fn home_windows_in_editor(&mut self, ctx: &egui::Context) {
        self.version_history_window(ctx);
    }

    pub fn open_version_history(&mut self, id: &str) {
        // Record the open document first so the list includes the latest edits.
        if self.project.as_deref() == Some(id) {
            self.save_current(crate::projects::Snapshot::IfChanged, "Edited");
        }
        let Ok(meta) = self.store.meta(id) else { return };
        let versions = self.store.versions(id);
        let selected = versions.first().map(|v| v.time);
        self.home.history = Some(VersionHistory { id: id.to_string(), name: meta.name, versions, selected });
        if let Some(t) = selected {
            self.request_version_preview(id, t);
        }
    }

    fn version_history_window(&mut self, ctx: &egui::Context) {
        let Some(h) = self.home.history.take() else { return };
        let mut close = false;
        let mut pick = None;
        let mut restore = None;
        let preview = h.selected.and_then(|t| match self.images.get(&version_key(&h.id, t)) {
            Some(ImgState::Ready { tex, size }) => Some((tex.clone(), *size)),
            _ => None,
        });
        let modal = egui::Modal::new(egui::Id::new("history_modal")).show(ctx, |ui| {
            ui.set_width(760.0);
            ui.horizontal(|ui| {
                ui.heading(format!("Version history: {}", h.name));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(egui::Button::new(egui::RichText::new(ph::X).size(16.0)).frame(false)).clicked() {
                        close = true;
                    }
                });
            });
            ui.label(egui::RichText::new("A version is kept every time you save, close a project, or keep editing for 10 minutes.").color(theme::text_dim()));
            ui.add_space(8.0);
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(300.0);
                    egui::ScrollArea::vertical().max_height(400.0).auto_shrink([false, false]).show(ui, |ui| {
                        for (i, v) in h.versions.iter().enumerate() {
                            let sel = h.selected == Some(v.time);
                            let title = if i == 0 { format!("{}  (current)", format_time(v.time)) } else { format_time(v.time) };
                            let detail = format!("{} · {} slide{} · {} item{}", if v.note.is_empty() { "Saved" } else { &v.note }, v.slides, if v.slides == 1 { "" } else { "s" }, v.items, if v.items == 1 { "" } else { "s" });
                            let r = ui.add(egui::Button::selectable(sel, egui::RichText::new(format!("{title}\n{detail}"))).min_size(Vec2::new(290.0, 44.0)));
                            if r.clicked() && !sel {
                                pick = Some(v.time);
                            }
                        }
                        if h.versions.is_empty() {
                            ui.label("No versions yet.");
                        }
                    });
                });
                ui.vertical(|ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(430.0, 300.0), Sense::hover());
                    ui.painter().rect(rect, CornerRadius::same(6), Color32::WHITE, Stroke::new(1.0, theme::border()), StrokeKind::Outside);
                    match &preview {
                        Some((tex, size)) => {
                            let inner = rect.shrink(10.0);
                            let s = (inner.width() / size[0] as f32).min(inner.height() / size[1] as f32);
                            let r = Rect::from_center_size(inner.center(), Vec2::new(size[0] as f32, size[1] as f32) * s);
                            ui.painter().image(tex.id(), r, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                        }
                        None if h.selected.is_some() => {
                            ui.painter().text(rect.center(), Align2::CENTER_CENTER, "Rendering preview...", FontId::proportional(14.0), Color32::GRAY);
                        }
                        None => {}
                    }
                    ui.label(egui::RichText::new("Preview of the first slide").color(theme::text_dim()).size(12.0));
                    ui.add_space(6.0);
                    let is_current = h.selected.is_some() && h.selected == h.versions.first().map(|v| v.time);
                    ui.horizontal(|ui| {
                        if ui.add_enabled(h.selected.is_some() && !is_current, egui::Button::new(egui::RichText::new(format!("{}  Restore this version", ph::ARROW_COUNTER_CLOCKWISE)).color(Color32::WHITE)).fill(theme::accent()))
                            .on_hover_text("The current version stays in the history, so a restore can be undone.")
                            .clicked()
                        {
                            restore = h.selected;
                        }
                    });
                });
            });
        });
        if let Some(t) = restore {
            self.actions.push(Action::RestoreVersion(h.id.clone(), t));
            return;
        }
        if close || modal.should_close() {
            return;
        }
        let mut h = h;
        if let Some(t) = pick {
            h.selected = Some(t);
            self.request_version_preview(&h.id, t);
        }
        self.home.history = Some(h);
    }

    fn request_version_preview(&mut self, id: &str, time: i64) {
        let key = version_key(id, time);
        if self.images.get(&key).is_some() || self.thumbs_pending.contains(&key) {
            return;
        }
        match self.store.load_version(id, time) {
            Ok(doc) => self.start_thumb(key, None, &doc),
            Err(e) => self.toast_err(format!("version: {e}")),
        }
    }
}

pub fn version_key(id: &str, time: i64) -> String {
    format!("version:{id}:{time}")
}

/// Slide-count badge in the tile's top-left corner: one image, or a stack for multi-slide decks.
fn badge(p: &egui::Painter, at: egui::Pos2, slides: usize) {
    let r = Rect::from_min_size(at, Vec2::splat(30.0));
    let (icon, fill) = if slides > 1 { (ph::IMAGES, Color32::from_rgb(0xb7, 0x7a, 0x1e)) } else { (ph::IMAGE, Color32::from_rgb(0x2b, 0x5c, 0x9e)) };
    p.rect_filled(r, CornerRadius::same(6), fill);
    p.text(r.center(), Align2::CENTER_CENTER, icon, FontId::proportional(18.0), Color32::WHITE);
}

/// Truncate `s` with an ellipsis to fit `max_w`.
fn elide(ui: &egui::Ui, s: &str, font: FontId, max_w: f32) -> String {
    let width = |t: &str| ui.fonts_mut(|f| f.layout_no_wrap(t.to_string(), font.clone(), Color32::WHITE).size().x);
    if width(s) <= max_w {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        let t: String = chars[..mid].iter().collect::<String>() + "...";
        if width(&t) <= max_w { lo = mid } else { hi = mid - 1 }
    }
    chars[..lo].iter().collect::<String>().trim_end().to_string() + "..."
}
