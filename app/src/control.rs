//! A control socket for driving the running editor from outside (desktop only): the command line
//! (`biocartoon app ...`) and the MCP server (`biocartoon mcp`) talk to the app
//! through it, so an assistant can place assets, move them and look at the slide.
//!
//! The app listens on a random localhost port and writes `<app dir>/control.json`
//! (`{"port", "token", "pid"}`, readable only by the user). A connection sends one JSON object
//! per line, `{"token": "...", "cmd": "...", "args": {...}}`, and gets one back:
//! `{"ok": true, "result": ...}` or `{"ok": false, "error": "..."}`. Commands run on the UI
//! thread between frames (see [`BioCartoon::poll_control`]); rendering runs on the connection's
//! thread so the UI never waits on image downloads. `BIOCARTOON_NO_CONTROL=1` disables it.

use crate::app::{Action, AlignKind, ArrangeKind, BioCartoon, Screen};
use crate::catalog::AssetRow;
use crate::doc::{CodeLang, CodeTheme, ImageSource, Item, ItemKind, LineKind, LineStyle, ShapeKind, TextAlign, TextFont, TextVAlign, Unit, DPI};
use egui::{Pos2, Rect, Vec2};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

/// Longest a command may wait for the UI thread (the app may be minimised or busy exporting).
const UI_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_LINE: u64 = 16 << 20;

#[derive(Serialize, Deserialize)]
struct Info {
    port: u16,
    token: String,
    pid: u32,
}

/// Work a command hands back after it ran on the UI thread: a result, or a job to finish on the
/// connection thread (rendering).
pub enum Outcome {
    Now(Result<Value, String>),
    Later(Box<dyn FnOnce() -> Result<Value, String> + Send>),
}

struct Pending {
    cmd: String,
    args: Value,
    reply: Sender<Outcome>,
}

pub struct ControlServer {
    rx: Receiver<Pending>,
    info_path: PathBuf,
    pub port: u16,
}

pub fn info_path(app_dir: &Path) -> PathBuf {
    app_dir.join("control.json")
}

fn random_token() -> String {
    let s = RandomState::new();
    (0..4).map(|i| { let mut h = s.build_hasher(); h.write_u64(i); format!("{:016x}", h.finish()) }).collect()
}

impl ControlServer {
    pub fn start(app_dir: &Path, ctx: egui::Context) -> Result<Self, String> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("control socket: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let token = random_token();
        let info_path = info_path(app_dir);
        let info = serde_json::to_vec(&Info { port, token: token.clone(), pid: std::process::id() }).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(app_dir).map_err(|e| e.to_string())?;
        std::fs::write(&info_path, info).map_err(|e| format!("{}: {e}", info_path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&info_path, std::fs::Permissions::from_mode(0o600));
        }
        let (tx, rx) = channel::<Pending>();
        std::thread::Builder::new()
            .name("control".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let (tx, ctx, token) = (tx.clone(), ctx.clone(), token.clone());
                    let _ = std::thread::Builder::new().name("control-conn".into()).spawn(move || serve_conn(stream, &token, &tx, &ctx));
                }
            })
            .map_err(|e| e.to_string())?;
        log::info!("control socket on 127.0.0.1:{port} ({})", info_path.display());
        Ok(Self { rx, info_path, port })
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        // Only ours: a newer instance may have written its own file since.
        let mine = std::fs::read(&self.info_path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Info>(&b).ok())
            .is_some_and(|i| i.pid == std::process::id());
        if mine {
            let _ = std::fs::remove_file(&self.info_path);
        }
    }
}

fn serve_conn(stream: TcpStream, token: &str, tx: &Sender<Pending>, ctx: &egui::Context) {
    let mut out = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let reader = BufReader::new(stream.take(MAX_LINE));
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Err(e) => Err(format!("bad request: {e}")),
            Ok(req) if req.get("token").and_then(Value::as_str) != Some(token) => Err("bad token".into()),
            Ok(req) => {
                let cmd = req.get("cmd").and_then(Value::as_str).unwrap_or_default().to_string();
                let args = req.get("args").cloned().unwrap_or(Value::Object(Map::new()));
                if cmd == "ping" {
                    Ok(json!({"pong": true, "pid": std::process::id()}))
                } else {
                    let (rtx, rrx) = channel();
                    if tx.send(Pending { cmd, args, reply: rtx }).is_err() {
                        Err("the editor is shutting down".to_string())
                    } else {
                        ctx.request_repaint();
                        match rrx.recv_timeout(UI_TIMEOUT) {
                            Ok(Outcome::Now(r)) => r,
                            Ok(Outcome::Later(job)) => job(),
                            Err(_) => Err("the editor did not respond in time".into()),
                        }
                    }
                }
            }
        };
        let msg = match reply {
            Ok(v) => json!({"ok": true, "result": v}),
            Err(e) => json!({"ok": false, "error": e}),
        };
        if out.write_all(format!("{msg}\n").as_bytes()).and_then(|_| out.flush()).is_err() {
            break;
        }
    }
}

// ---------- client ----------

/// A connection to the running editor.
pub struct Client {
    stream: BufReader<TcpStream>,
    token: String,
}

impl Client {
    /// Connect to the editor whose `control.json` is in `app_dir`.
    pub fn connect(app_dir: &Path) -> Result<Self, String> {
        let p = info_path(app_dir);
        let not_running = || "BioCartoon is not running (start the editor first)".to_string();
        let info: Info = std::fs::read(&p).ok().and_then(|b| serde_json::from_slice(&b).ok()).ok_or_else(not_running)?;
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], info.port));
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).map_err(|_| not_running())?;
        let _ = stream.set_read_timeout(Some(UI_TIMEOUT + Duration::from_secs(60)));
        Ok(Self { stream: BufReader::new(stream), token: info.token })
    }

    pub fn call(&mut self, cmd: &str, args: Value) -> Result<Value, String> {
        let req = json!({"token": self.token, "cmd": cmd, "args": args});
        let w = self.stream.get_mut();
        w.write_all(format!("{req}\n").as_bytes()).and_then(|_| w.flush()).map_err(|e| format!("control: {e}"))?;
        let mut line = String::new();
        self.stream.read_line(&mut line).map_err(|e| format!("control: {e}"))?;
        if line.is_empty() {
            return Err("the editor closed the connection".into());
        }
        let v: Value = serde_json::from_str(&line).map_err(|e| format!("control: bad reply: {e}"))?;
        if v.get("ok").and_then(Value::as_bool) == Some(true) {
            Ok(v.get("result").cloned().unwrap_or(Value::Null))
        } else {
            Err(v.get("error").and_then(Value::as_str).unwrap_or("unknown error").to_string())
        }
    }
}

/// One-shot: connect, run `cmd`, disconnect.
pub fn send(app_dir: &Path, cmd: &str, args: Value) -> Result<Value, String> {
    Client::connect(app_dir)?.call(cmd, args)
}

// ---------- commands (UI thread) ----------

fn f(args: &Value, k: &str) -> Option<f32> {
    args.get(k).and_then(Value::as_f64).map(|v| v as f32)
}
fn s<'a>(args: &'a Value, k: &str) -> Option<&'a str> {
    args.get(k).and_then(Value::as_str)
}
fn b(args: &Value, k: &str) -> Option<bool> {
    args.get(k).and_then(Value::as_bool)
}
fn u(args: &Value, k: &str) -> Option<u64> {
    args.get(k).and_then(Value::as_u64)
}
fn ids(args: &Value) -> Vec<u64> {
    let mut v: Vec<u64> = args.get("ids").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
    if let Some(id) = u(args, "id") {
        v.push(id);
    }
    v
}
fn color(args: &Value, k: &str) -> Option<[u8; 4]> {
    let a = args.get(k)?.as_array()?;
    let c = |i: usize, d: u8| a.get(i).and_then(Value::as_u64).map(|v| v.min(255) as u8).unwrap_or(d);
    Some([c(0, 0), c(1, 0), c(2, 0), c(3, 255)])
}
/// Enum names are accepted in any case, with or without underscores ("rounded_rectangle").
fn key(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()
}
fn parse_enum<T: Copy + std::fmt::Debug>(all: &[T], name: &str, what: &str) -> Result<T, String> {
    let k = key(name);
    all.iter().copied().find(|v| key(&format!("{v:?}")) == k).ok_or_else(|| {
        format!("unknown {what} {name:?}; one of {}", all.iter().map(|v| format!("{v:?}")).collect::<Vec<_>>().join(", "))
    })
}
fn name_of<T: std::fmt::Debug>(v: T) -> String {
    format!("{v:?}")
}

pub fn item_json(it: &Item) -> Value {
    let r = it.rect();
    let mut v = json!({
        "id": it.id, "name": it.name, "type": match it.kind { ItemKind::Image{..} => "image", ItemKind::Text{..} => "text", ItemKind::Shape{..} => "shape", ItemKind::Line{..} => "line", ItemKind::Table{..} => "table", ItemKind::Code{..} => "code" },
        "x": r.min.x, "y": r.min.y, "width": r.width(), "height": r.height(),
        "rotation": it.rotation.to_degrees(), "flip_h": it.flip_h, "flip_v": it.flip_v,
        "locked": it.locked, "opacity": it.opacity, "group": it.group,
    });
    let o = v.as_object_mut().unwrap();
    match &it.kind {
        ItemKind::Image { source, crop, overlay } => {
            match source {
                ImageSource::Asset { id, key, svg, title, source, credit, license } => {
                    o.insert("asset_id".into(), json!(id));
                    o.insert("asset_title".into(), json!(title));
                    o.insert("asset_source".into(), json!(source));
                    o.insert("png_key".into(), json!(key));
                    if let Some(svg) = svg {
                        o.insert("svg_key".into(), json!(svg));
                    }
                    o.insert("credit".into(), json!(credit));
                    o.insert("license".into(), json!(license));
                }
                ImageSource::File { path, from } => {
                    o.insert("file".into(), json!(path));
                    if let Some(ImageSource::Asset { id, credit, .. }) = from.as_deref() {
                        o.insert("derived_from".into(), json!(id));
                        o.insert("credit".into(), json!(credit));
                    }
                }
            }
            o.insert("crop".into(), json!(crop));
            o.insert("overlay".into(), json!(overlay));
        }
        ItemKind::Text { text, size, color, align, bold, italic, underline, font, line_spacing, valign } => {
            o.insert("text".into(), json!(text));
            o.insert("font_size".into(), json!(size));
            o.insert("color".into(), json!(color));
            o.insert("align".into(), json!(name_of(align)));
            o.insert("valign".into(), json!(name_of(valign)));
            o.insert("bold".into(), json!(bold));
            o.insert("italic".into(), json!(italic));
            o.insert("underline".into(), json!(underline));
            o.insert("font".into(), json!(name_of(font)));
            o.insert("line_spacing".into(), json!(line_spacing));
        }
        ItemKind::Shape { shape, fill, stroke, stroke_width } => {
            o.insert("shape".into(), json!(name_of(shape)));
            o.insert("fill".into(), json!(fill));
            o.insert("stroke".into(), json!(stroke));
            o.insert("stroke_width".into(), json!(stroke_width));
        }
        ItemKind::Line { kind, color, width, style, .. } => {
            o.insert("line_kind".into(), json!(name_of(kind)));
            o.insert("line_style".into(), json!(name_of(style)));
            o.insert("color".into(), json!(color));
            o.insert("line_width".into(), json!(width));
            o.insert("x1".into(), json!(it.pos[0]));
            o.insert("y1".into(), json!(it.pos[1]));
            o.insert("x2".into(), json!(it.pos[0] + it.size[0]));
            o.insert("y2".into(), json!(it.pos[1] + it.size[1]));
        }
        ItemKind::Table { rows, cols, cells, header, font_size, .. } => {
            o.insert("rows".into(), json!(rows));
            o.insert("cols".into(), json!(cols));
            o.insert("cells".into(), json!(cells));
            o.insert("header".into(), json!(header));
            o.insert("font_size".into(), json!(font_size));
        }
        ItemKind::Code { code, language, theme, font_size, line_numbers, window, rounded } => {
            o.insert("code".into(), json!(code));
            o.insert("language".into(), json!(name_of(language)));
            if *language == CodeLang::Auto {
                o.insert("detected_language".into(), json!(name_of(language.resolve(code))));
            }
            o.insert("theme".into(), json!(name_of(theme)));
            o.insert("font_size".into(), json!(font_size));
            o.insert("line_numbers".into(), json!(line_numbers));
            o.insert("window".into(), json!(window));
            o.insert("rounded".into(), json!(rounded));
        }
    }
    v
}

/// Geometry and appearance fields shared by every `insert_*` and `update_item`.
fn patch_item(it: &mut Item, args: &Value) -> Result<(), String> {
    let mut r = it.rect();
    let mut moved = false;
    if let Some(x) = f(args, "x") { r = Rect::from_min_size(Pos2::new(x, r.min.y), r.size()); moved = true; }
    if let Some(y) = f(args, "y") { r = Rect::from_min_size(Pos2::new(r.min.x, y), r.size()); moved = true; }
    if let Some(w) = f(args, "width") { r = Rect::from_min_size(r.min, Vec2::new(w.max(1.0), r.height())); moved = true; }
    if let Some(h) = f(args, "height") { r = Rect::from_min_size(r.min, Vec2::new(r.width(), h.max(1.0))); moved = true; }
    if moved {
        it.set_rect(r);
    }
    if it.is_line() {
        if let (Some(x1), Some(y1)) = (f(args, "x1"), f(args, "y1")) {
            let end = Pos2::new(it.pos[0] + it.size[0], it.pos[1] + it.size[1]);
            it.pos = [x1, y1];
            it.size = [end.x - x1, end.y - y1];
        }
        if let (Some(x2), Some(y2)) = (f(args, "x2"), f(args, "y2")) {
            it.size = [x2 - it.pos[0], y2 - it.pos[1]];
        }
    }
    if let Some(d) = f(args, "rotation") { it.rotation = d.to_radians(); }
    if let Some(v) = b(args, "flip_h") { it.flip_h = v; }
    if let Some(v) = b(args, "flip_v") { it.flip_v = v; }
    if let Some(v) = b(args, "locked") { it.locked = v; }
    if let Some(v) = f(args, "opacity") { it.opacity = v.clamp(0.0, 1.0); }
    if let Some(v) = s(args, "name") { it.name = v.to_string(); }
    match &mut it.kind {
        ItemKind::Image { crop, overlay, .. } => {
            if let Some(c) = args.get("crop").and_then(Value::as_array) {
                if c.len() == 4 {
                    let g = |i: usize| c[i].as_f64().unwrap_or(0.0) as f32;
                    *crop = [g(0).clamp(0.0, 1.0), g(1).clamp(0.0, 1.0), g(2).clamp(0.0, 1.0), g(3).clamp(0.0, 1.0)];
                }
            }
            if let Some(c) = color(args, "overlay") { *overlay = c; }
        }
        ItemKind::Text { text, size, color: c, align, bold, italic, underline, font, line_spacing, valign } => {
            if let Some(v) = s(args, "text") { *text = v.to_string(); }
            if let Some(v) = f(args, "font_size") { *size = v.max(1.0); }
            if let Some(v) = color(args, "color") { *c = v; }
            if let Some(v) = b(args, "bold") { *bold = v; }
            if let Some(v) = s(args, "align") { *align = parse_enum(&TextAlign::ALL, v, "align")?; }
            if let Some(v) = s(args, "valign") { *valign = parse_enum(&TextVAlign::ALL, v, "valign")?; }
            if let Some(v) = b(args, "italic") { *italic = v; }
            if let Some(v) = b(args, "underline") { *underline = v; }
            if let Some(v) = s(args, "font") { *font = parse_enum(&TextFont::ALL, v, "font")?; }
            if let Some(v) = f(args, "line_spacing") { *line_spacing = v.clamp(0.5, 4.0); }
        }
        ItemKind::Shape { shape, fill, stroke, stroke_width } => {
            if let Some(v) = s(args, "shape") { *shape = parse_enum(&ShapeKind::ALL, v, "shape")?; }
            if let Some(v) = color(args, "fill") { *fill = v; }
            if let Some(v) = color(args, "stroke") { *stroke = v; }
            if let Some(v) = f(args, "stroke_width") { *stroke_width = v.max(0.0); }
        }
        ItemKind::Line { kind, color: c, width, style, .. } => {
            if let Some(v) = s(args, "line_kind") { *kind = parse_enum(&LineKind::ALL, v, "line kind")?; }
            if let Some(v) = s(args, "line_style") { *style = parse_enum(&LineStyle::ALL, v, "line style")?; }
            if let Some(v) = color(args, "color") { *c = v; }
            if let Some(v) = f(args, "line_width") { *width = v.max(0.5); }
        }
        ItemKind::Table { font_size, cells, header, .. } => {
            if let Some(v) = f(args, "font_size") { *font_size = v.max(1.0); }
            if let Some(v) = b(args, "header") { *header = v; }
            if let Some(c) = args.get("cells").and_then(Value::as_array) {
                for (i, v) in c.iter().enumerate().take(cells.len()) {
                    if let Some(t) = v.as_str() { cells[i] = t.to_string(); }
                }
            }
        }
        ItemKind::Code { code, language, theme, font_size, line_numbers, window, rounded } => {
            if let Some(v) = s(args, "code") { *code = v.to_string(); }
            if let Some(v) = s(args, "language") { *language = parse_enum(&CodeLang::ALL, v, "language")?; }
            if let Some(v) = s(args, "theme") { *theme = parse_enum(&CodeTheme::ALL, v, "theme")?; }
            if let Some(v) = f(args, "font_size") { *font_size = v.max(1.0); }
            if let Some(v) = b(args, "line_numbers") { *line_numbers = v; }
            if let Some(v) = b(args, "window") { *window = v; }
            if let Some(v) = b(args, "rounded") { *rounded = v; }
        }
    }
    Ok(())
}

impl BioCartoon {
    /// Run the commands that arrived since the last frame (called once per frame).
    pub fn poll_control(&mut self, ctx: &egui::Context) {
        let pending: Vec<Pending> = match &self.control {
            Some(c) => c.rx.try_iter().collect(),
            None => return,
        };
        for p in pending {
            let out = self.control_exec(&p.cmd, &p.args, ctx);
            let _ = p.reply.send(out);
        }
    }

    fn page_json(&self) -> Value {
        let u = self.doc.unit;
        json!({
            "width": self.doc.width, "height": self.doc.height, "unit": name_of(u),
            "width_in_unit": u.from_px(self.doc.width), "height_in_unit": u.from_px(self.doc.height),
            "background": self.doc.background_of(self.slide()), "dpi": DPI,
        })
    }

    fn state_json(&self) -> Value {
        if self.screen == Screen::Home {
            return json!({"screen": "home", "note": "no document is open; insert commands open a new one", "projects": self.home.projects.iter().map(|p| json!({"id": p.id, "name": p.name})).collect::<Vec<_>>()});
        }
        json!({
            "screen": "editor",
            "project": self.project,
            "title": self.doc.title,
            "page": self.page_json(),
            "slide": {"index": self.cur_slide, "count": self.doc.slides.len(), "name": self.slide().name},
            "items": self.slide().items.iter().map(item_json).collect::<Vec<_>>(),
            "selection": self.selection,
            "unsaved_changes": self.dirty,
        })
    }

    /// Insert commands need an open document; on the home screen start a new one.
    fn ensure_editor(&mut self) {
        if self.screen != Screen::Editor {
            self.new_doc();
        }
    }

    fn require_item(&self, id: u64) -> Result<&Item, String> {
        self.item(id).ok_or_else(|| format!("no item {id} on this slide"))
    }

    fn control_exec(&mut self, cmd: &str, args: &Value, ctx: &egui::Context) -> Outcome {
        let r = self.control_cmd(cmd, args, ctx);
        if matches!(r, Ok(Outcome::Now(Ok(_)))) {
            ctx.request_repaint();
        }
        match r {
            Ok(o) => o,
            Err(e) => Outcome::Now(Err(e)),
        }
    }

    fn control_cmd(&mut self, cmd: &str, args: &Value, ctx: &egui::Context) -> Result<Outcome, String> {
        let now = |v: Value| Ok(Outcome::Now(Ok(v)));
        match cmd {
            "state" => now(self.state_json()),
            "get_item" => {
                let id = u(args, "id").ok_or("id required")?;
                now(item_json(self.require_item(id)?))
            }
            "slides" => now(json!(self.doc.slides.iter().enumerate().map(|(i, s)| json!({"index": i, "name": s.name, "items": s.items.len(), "current": i == self.cur_slide})).collect::<Vec<_>>())),
            "go_slide" => {
                let i = u(args, "index").ok_or("index required")? as usize;
                if i >= self.doc.slides.len() {
                    return Err(format!("slide index {i} out of range (0..{})", self.doc.slides.len()));
                }
                self.ensure_editor();
                self.actions.push(Action::GoSlide(i));
                self.run_actions(ctx);
                now(json!({"slide": i}))
            }
            "add_slide" => {
                self.ensure_editor();
                self.actions.push(Action::AddSlide);
                self.run_actions(ctx);
                now(json!({"slide": self.cur_slide, "count": self.doc.slides.len()}))
            }
            "new_document" => {
                self.new_doc();
                now(self.state_json())
            }
            "set_page" => {
                self.ensure_editor();
                self.commit();
                if let Some(un) = s(args, "unit") {
                    self.doc.unit = parse_enum(&Unit::ALL, un, "unit")?;
                }
                let unit = self.doc.unit;
                if let Some(w) = f(args, "width") { self.doc.width = unit.to_px(w).max(1.0); }
                if let Some(h) = f(args, "height") { self.doc.height = unit.to_px(h).max(1.0); }
                if let Some(c) = color(args, "background") { self.slide_mut().background = Some(c); }
                self.view.fit_requested = true;
                now(self.page_json())
            }
            "insert_asset" => {
                let a = args.get("asset").ok_or("asset required (a catalog row)")?;
                let g = |k: &str| s(a, k).unwrap_or_default().to_string();
                let row = AssetRow {
                    id: g("id"),
                    source: g("source"),
                    source_name: g("source_name"),
                    title: g("title"),
                    variant: g("variant"),
                    category: g("category"),
                    description: g("description"),
                    license: g("license"),
                    credit: g("credit"),
                    author: g("author"),
                    png_key: g("png_key"),
                    png_local: s(a, "png_local").map(str::to_string),
                    png_width: u(a, "width").unwrap_or(1) as u32,
                    png_height: u(a, "height").unwrap_or(1) as u32,
                    svg_key: s(a, "svg_key").map(str::to_string),
                    stars: 0,
                    starred: false,
                    taxa: Vec::new(),
                };
                if row.id.is_empty() || row.png_key.is_empty() {
                    return Err("asset needs at least id and png_key".into());
                }
                self.ensure_editor();
                if let Some(target) = u(args, "replace_item") {
                    self.require_item(target)?;
                    if !matches!(self.require_item(target)?.kind, ItemKind::Image { .. }) {
                        return Err(format!("item {target} is not an image"));
                    }
                    self.replace_target = Some(target);
                    self.insert_asset(&row);
                    self.commit_patch(target, args)?;
                    return now(item_json(self.require_item(target)?));
                }
                self.insert_asset(&row);
                let id = *self.selection.first().ok_or("insert failed")?;
                self.patch_in_place(id, args)?;
                now(item_json(self.require_item(id)?))
            }
            "insert_text" => {
                let text = s(args, "text").ok_or("text required")?.to_string();
                let size = f(args, "font_size").unwrap_or(24.0).max(1.0);
                let lines = text.lines().count().max(1) as f32;
                let longest = text.lines().map(|l| l.chars().count()).max().unwrap_or(1) as f32;
                let kind = ItemKind::text(text, size, color(args, "color").unwrap_or([30, 30, 30, 255]), TextAlign::Center, false);
                let item = Item::new(0, "Text", kind, Pos2::ZERO, Vec2::new((longest * size * 0.6).max(size), lines * size * 1.4));
                self.insert_new(item, args)
            }
            "insert_code" => {
                let code = s(args, "code").ok_or("code required")?.to_string();
                let language = match s(args, "language") { Some(v) => parse_enum(&CodeLang::ALL, v, "language")?, None => CodeLang::Auto };
                let theme = match s(args, "theme") { Some(v) => parse_enum(&CodeTheme::ALL, v, "theme")?, None => CodeTheme::Dracula };
                let line_numbers = b(args, "line_numbers").unwrap_or(true);
                let window = b(args, "window").unwrap_or(false);
                let rounded = b(args, "rounded").unwrap_or(false);
                let size = crate::code::fit_size(ctx, &code, 14.0, line_numbers, window);
                let kind = ItemKind::Code { code, language, theme, font_size: 14.0, line_numbers, window, rounded };
                let item = Item::new(0, "Code", kind, Pos2::ZERO, Vec2::new(size[0], size[1]));
                self.insert_new(item, args)
            }
            "insert_shape" => {
                let shape = parse_enum(&ShapeKind::ALL, s(args, "shape").unwrap_or("rectangle"), "shape")?;
                let kind = ItemKind::Shape { shape, fill: [120, 170, 230, 255], stroke: [40, 70, 120, 255], stroke_width: 2.0 };
                let d = self.doc.width.min(self.doc.height) * 0.25;
                let item = Item::new(0, shape.label(), kind, Pos2::ZERO, Vec2::splat(d));
                self.insert_new(item, args)
            }
            "insert_line" => {
                let lk = parse_enum(&LineKind::ALL, s(args, "line_kind").unwrap_or("arrow"), "line kind")?;
                let style = match s(args, "line_style") { Some(v) => parse_enum(&LineStyle::ALL, v, "line style")?, None => LineStyle::Solid };
                let kind = ItemKind::Line { kind: lk, color: [40, 40, 40, 255], width: 3.0, style, points: Vec::new() };
                let d = self.doc.width.min(self.doc.height) * 0.25;
                let item = Item::new(0, lk.label(), kind, Pos2::ZERO, Vec2::new(d, if lk.needs_height() { d * 0.5 } else { 0.0 }));
                self.insert_new(item, args)
            }
            "insert_template" => {
                let id = s(args, "id").ok_or("id required")?;
                self.ensure_editor();
                if let Some(n) = id.strip_prefix("builtin:") {
                    let n: usize = n.parse().map_err(|_| "bad builtin template id")?;
                    if n >= Self::template_names().len() {
                        return Err("no such built-in template".into());
                    }
                    self.use_template(n);
                    return now(json!({"status": "inserted", "slide": self.cur_slide, "items": self.slide().items.len()}));
                }
                let row = self.template_rows.iter().find(|r| r.meta.id == id).cloned().ok_or_else(|| format!("no template {id}"))?;
                self.load_saved_template(row);
                now(json!({"status": "loading", "note": "the template's slide is added after the current one once its file is loaded; call state to see it"}))
            }
            "update_item" => {
                let id = u(args, "id").ok_or("id required")?;
                self.require_item(id)?;
                self.commit_patch(id, args)?;
                now(item_json(self.require_item(id)?))
            }
            "move_items" => {
                let ids = ids(args);
                if ids.is_empty() {
                    return Err("ids required".into());
                }
                for id in &ids {
                    self.require_item(*id)?;
                }
                self.commit();
                let (x, y, dx, dy) = (f(args, "x"), f(args, "y"), f(args, "dx").unwrap_or(0.0), f(args, "dy").unwrap_or(0.0));
                // Absolute x/y place the group's bounding box; dx/dy shift everything.
                let bounds = ids.iter().filter_map(|id| self.item(*id)).map(Item::rect).reduce(|a, b| a.union(b)).unwrap();
                let shift = Vec2::new(x.map_or(0.0, |x| x - bounds.min.x) + dx, y.map_or(0.0, |y| y - bounds.min.y) + dy);
                for id in &ids {
                    if let Some(it) = self.item_mut(*id) {
                        it.pos = [it.pos[0] + shift.x, it.pos[1] + shift.y];
                    }
                }
                now(json!(ids.iter().filter_map(|id| self.item(*id)).map(item_json).collect::<Vec<_>>()))
            }
            "delete_items" => {
                let ids = ids(args);
                self.select(ids.clone());
                let n = self.selection.len();
                self.delete_selection();
                now(json!({"deleted": n}))
            }
            "select" => {
                self.select(ids(args));
                now(json!({"selection": self.selection}))
            }
            "duplicate" => {
                self.select(ids(args));
                self.actions.push(Action::Duplicate);
                self.run_actions(ctx);
                now(json!(self.selected_items().into_iter().map(item_json).collect::<Vec<_>>()))
            }
            "align" => {
                let k = parse_enum(&[AlignKind::Left, AlignKind::CenterH, AlignKind::Right, AlignKind::Top, AlignKind::Middle, AlignKind::Bottom, AlignKind::DistributeH, AlignKind::DistributeV], s(args, "kind").ok_or("kind required")?, "align kind")?;
                let ids = ids(args);
                if !ids.is_empty() {
                    self.select(ids);
                }
                self.align(k);
                now(json!(self.selected_items().into_iter().map(item_json).collect::<Vec<_>>()))
            }
            "arrange" => {
                let k = parse_enum(&[ArrangeKind::Front, ArrangeKind::Forward, ArrangeKind::Backward, ArrangeKind::Back], s(args, "kind").ok_or("kind required")?, "arrange kind")?;
                let ids = ids(args);
                if !ids.is_empty() {
                    self.select(ids);
                }
                self.arrange(k);
                now(json!({"order": self.slide().items.iter().map(|i| i.id).collect::<Vec<_>>()}))
            }
            "undo" => {
                self.undo();
                now(self.state_json())
            }
            "redo" => {
                self.redo();
                now(self.state_json())
            }
            "save" => {
                self.ensure_editor();
                self.save(false);
                now(json!({"project": self.project}))
            }
            "render" => {
                if self.screen != Screen::Editor {
                    return Err("no document is open".into());
                }
                let idx = u(args, "slide").map(|i| i as usize).unwrap_or(self.cur_slide);
                let slide = self.doc.slides.get(idx).cloned().ok_or_else(|| format!("slide index {idx} out of range"))?;
                let max_side = u(args, "max_side").unwrap_or(1024).clamp(64, 4096) as u32;
                let path = s(args, "path").map(PathBuf::from);
                let (requests, cached) = self.render_inputs(&slide);
                let (page, bg, ctx) = (self.doc.page_rect(), self.doc.background_of(&slide), ctx.clone());
                Ok(Outcome::Later(Box::new(move || {
                    let img = crate::export::thumbnail(&ctx, &slide, page, bg, &requests, &cached, max_side)?;
                    let (w, h) = img.dimensions();
                    let mut png = Vec::new();
                    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(|e| e.to_string())?;
                    if let Some(p) = path {
                        std::fs::write(&p, &png).map_err(|e| format!("{}: {e}", p.display()))?;
                        return Ok(json!({"path": p, "width": w, "height": h}));
                    }
                    use base64::Engine;
                    Ok(json!({"png_base64": base64::engine::general_purpose::STANDARD.encode(&png), "width": w, "height": h}))
                })))
            }
            other => Err(format!("unknown command {other:?}")),
        }
    }

    /// Insert a freshly built item (centred, like the sidebar does), then apply any geometry
    /// or appearance given in `args`.
    fn insert_new(&mut self, mut item: Item, args: &Value) -> Result<Outcome, String> {
        self.ensure_editor();
        item.id = self.doc.alloc_id();
        let id = item.id;
        self.insert_item(item);
        self.patch_in_place(id, args)?;
        Ok(Outcome::Now(Ok(item_json(self.require_item(id)?))))
    }

    /// Apply `args` to an item that was just inserted (its undo snapshot was taken already).
    fn patch_in_place(&mut self, id: u64, args: &Value) -> Result<(), String> {
        let it = self.item_mut(id).ok_or("item vanished")?;
        patch_item(it, args)
    }

    /// Apply `args` to an existing item as one undoable edit.
    fn commit_patch(&mut self, id: u64, args: &Value) -> Result<(), String> {
        let mut edited = self.require_item(id)?.clone();
        patch_item(&mut edited, args)?;
        if edited != *self.require_item(id)? {
            self.commit();
            *self.item_mut(id).ok_or("item vanished")? = edited;
        }
        Ok(())
    }
}
