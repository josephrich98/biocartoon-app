//! A hidden snake game on the page: Ctrl+Option+Cmd+T starts one (or a new one), the arrow keys
//! steer, and Escape puts it away. The snake and the dot are drawn in whichever of black or white
//! stands out against the page; the page edge, the snake itself and anything on the slide end
//! the game. Images obstruct only their visible artwork. Nothing runs while no
//! game is open (`BioCartoon::snake` is `None`).

use crate::app::BioCartoon;
use crate::assets::ImageCache;
use crate::doc::{Item, ItemKind, Slide, DPI};
use crate::theme;
use egui::{Align2, Color32, Context, FontId, Painter, Pos2, Rect, RichText, Vec2};
use std::collections::VecDeque;

/// Page points per cell: an eighth of an inch.
const CELL: f32 = DPI / 8.0;
/// Seconds per step.
const STEP: f64 = 0.09;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dir {
    Up,
    Down,
    Left,
    Right,
}

impl Dir {
    fn delta(self) -> (i32, i32) {
        match self {
            Dir::Up => (0, -1),
            Dir::Down => (0, 1),
            Dir::Left => (-1, 0),
            Dir::Right => (1, 0),
        }
    }

    fn opposite(self) -> Dir {
        match self {
            Dir::Up => Dir::Down,
            Dir::Down => Dir::Up,
            Dir::Left => Dir::Right,
            Dir::Right => Dir::Left,
        }
    }
}

pub struct Snake {
    ctx: Context,
    /// Page points per cell, and the grid's size in cells.
    cell: f32,
    cols: i32,
    rows: i32,
    /// Head first.
    body: VecDeque<(i32, i32)>,
    dir: Dir,
    /// Turns pressed but not yet taken, so two quick presses between steps both count.
    turns: VecDeque<Dir>,
    food: (i32, i32),
    score: u32,
    /// When the first arrow key was pressed (the snake waits until then), and the next step's time.
    started: Option<f64>,
    next_step: f64,
    /// When the game ended, and the cell the snake ran into.
    over: Option<f64>,
    won: bool,
    crash: Option<(i32, i32)>,
    /// Which cells (row-major) something on the slide covers, as of the last step.
    walls: Vec<bool>,
    rng: u64,
}

impl Snake {
    pub fn new(slide: &Slide, page: Rect, now: f64, images: &mut ImageCache, ctx: &Context) -> Self {
        let cell = CELL;
        let (cols, rows) = (((page.width() / cell) as i32).max(4), ((page.height() / cell) as i32).max(4));
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
        let mut s = Self { ctx: ctx.clone(), cell, cols, rows, body: VecDeque::new(), dir: Dir::Right, turns: VecDeque::new(), food: (0, 0), score: 0, started: None, next_step: now, over: None, won: false, crash: None, walls: Vec::new(), rng: seed | 1 };
        s.find_walls(slide, images);
        // Start at the middle if three cells there are free, otherwise somewhere that is.
        let free3 = |s: &Snake, x: i32, y: i32| (0..3).all(|i| x - i >= 0 && !s.blocked((x - i, y)));
        let mut head = (cols / 2, rows / 2);
        if !free3(&s, head.0, head.1) {
            for _ in 0..400 {
                let (x, y) = (s.rand(cols), s.rand(rows));
                if free3(&s, x, y) {
                    head = (x, y);
                    break;
                }
            }
        }
        s.body = (0..3).map(|i| (head.0 - i, head.1)).collect();
        if !s.place_food() {
            s.over = Some(now);
            s.won = true;
        }
        s
    }

    fn turn(&mut self, d: Dir, now: f64) {
        if self.over.is_some() {
            return;
        }
        if self.started.is_none() {
            self.started = Some(now);
            self.next_step = now + STEP;
        }
        let last = self.turns.back().copied().unwrap_or(self.dir);
        if d != last && d != last.opposite() && self.turns.len() < 3 {
            self.turns.push_back(d);
        }
    }

    /// Advances the snake to `now`; returns how long until the next step (`None` once it's over
    /// or before it has started, when nothing on the page changes by itself).
    pub fn update(&mut self, slide: &Slide, now: f64, images: &mut ImageCache) -> Option<f64> {
        if self.started.is_none() {
            // Objects moved before the first arrow key still count.
            self.find_walls(slide, images);
            return None;
        }
        while self.over.is_none() && now >= self.next_step {
            self.step(slide, self.next_step, images);
            self.next_step += STEP;
        }
        if self.over.is_some() {
            return None;
        }
        Some(self.next_step - now)
    }

    fn step(&mut self, slide: &Slide, at: f64, images: &mut ImageCache) {
        self.find_walls(slide, images);
        if let Some(d) = self.turns.pop_front() {
            self.dir = d;
        }
        let (dx, dy) = self.dir.delta();
        let head = (self.body[0].0 + dx, self.body[0].1 + dy);
        let eats = head == self.food;
        // The tail moves out of the way this step unless the snake is growing.
        let body_hit = self.body.iter().take(self.body.len() - usize::from(!eats)).any(|&c| c == head);
        if body_hit || self.blocked(head) {
            self.over = Some(at);
            self.crash = Some(head);
            return;
        }
        self.body.push_front(head);
        if eats {
            self.score += 1;
            if !self.place_food() {
                self.over = Some(at);
                self.won = true;
            }
        } else {
            self.body.pop_back();
        }
    }

    fn rand(&mut self, n: i32) -> i32 {
        // xorshift64
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng % n.max(1) as u64) as i32
    }

    /// Place a dot in the open region connected to the head. The body and slide art
    /// both separate regions, even when there are empty cells elsewhere on the page.
    fn place_food(&mut self) -> bool {
        let index = |(x, y): (i32, i32)| (y * self.cols + x) as usize;
        let mut seen = vec![false; (self.cols * self.rows) as usize];
        let mut occupied = vec![false; seen.len()];
        for &c in &self.body {
            occupied[index(c)] = true;
        }
        let mut queue = VecDeque::from([self.body[0]]);
        seen[index(self.body[0])] = true;
        let mut reachable = Vec::new();
        while let Some((x, y)) = queue.pop_front() {
            for (dx, dy) in [Dir::Up, Dir::Down, Dir::Left, Dir::Right].map(Dir::delta) {
                let c = (x + dx, y + dy);
                if self.blocked(c) {
                    continue;
                }
                let i = index(c);
                if seen[i] || occupied[i] {
                    continue;
                }
                seen[i] = true;
                reachable.push(c);
                queue.push_back(c);
            }
        }
        if reachable.is_empty() {
            return false;
        }
        // A dead end is safe only when eating its dot ends the game. Otherwise
        // the cell needs a way in and a different way out. Count the head as a
        // possible entrance, but not the rest of the occupied body.
        if reachable.len() > 1 {
            reachable.retain(|&(x, y)| {
                [Dir::Up, Dir::Down, Dir::Left, Dir::Right]
                    .into_iter()
                    .map(Dir::delta)
                    .filter(|&(dx, dy)| {
                        let c = (x + dx, y + dy);
                        !self.blocked(c) && (c == self.body[0] || !occupied[index(c)])
                    })
                    .count() >= 2
            });
        }
        if reachable.is_empty() {
            return false;
        }
        self.food = reachable[self.rand(reachable.len() as i32) as usize];
        true
    }

    fn cell_rect(&self, (x, y): (i32, i32)) -> Rect {
        Rect::from_min_size(Pos2::new(x as f32 * self.cell, y as f32 * self.cell), Vec2::splat(self.cell))
    }

    /// Off the grid, or on something on the slide.
    fn blocked(&self, c: (i32, i32)) -> bool {
        c.0 < 0 || c.1 < 0 || c.0 >= self.cols || c.1 >= self.rows || self.walls[(c.1 * self.cols + c.0) as usize]
    }

    fn find_walls(&mut self, slide: &Slide, images: &mut ImageCache) {
        // Resolve each image once per step, using the same layer rendering (including the
        // last ready version during an edit) that draw_item uses. CPU pixels already contain
        // the SVG's transparency and are kept by the image cache; no extra rendering needed.
        let pixels: Vec<_> = slide.items.iter().map(|it| {
            let ItemKind::Image { source, layers, .. } = &it.kind else { return None };
            let key = source.cache_key();
            let key = images.restyled(&key, layers).map(|(key, _)| key).unwrap_or(key);
            images.cpu_image(&key)
        }).collect();
        let text_rows: Vec<_> = slide.items.iter().map(|it| text_rows(&self.ctx, it)).collect();
        let inset = self.cell * 0.25;
        let cells = (0..self.rows).flat_map(|y| (0..self.cols).map(move |x| (x, y)));
        self.walls = cells.map(|c| {
            let r = self.cell_rect(c).shrink(inset);
            slide.items.iter().zip(&pixels).zip(&text_rows).any(|((it, img), rows)| {
                if let Some(rows) = rows {
                    rows.iter().any(|&row| rotated_rect_hits(row, it.rect().center(), it.rotation, r))
                } else {
                    item_hits(it, r, img.as_deref())
                }
            })
        }).collect();
    }

    pub fn draw(&self, painter: &Painter, xf: &crate::canvas::Xform, page_s: Rect, bg: Color32, now: f64) {
        let fg = contrast(bg);
        let z = xf.zoom;
        for &c in &self.body {
            painter.rect_filled(xf.rect(self.cell_rect(c)).shrink(0.5), 0.0, fg);
        }
        if !self.won {
            painter.rect_filled(xf.rect(self.cell_rect(self.food)).shrink(self.cell * z * 0.15), 0.0, fg);
        }

        let secs = match (self.started, self.over) {
            (Some(s), Some(o)) => o - s,
            (Some(s), None) => now - s,
            _ => 0.0,
        }
        .max(0.0) as u64;
        let font = FontId::monospace((self.cell * z * 1.4).max(11.0));
        let pad = self.cell * z;
        let p = painter.with_clip_rect(page_s);
        p.text(page_s.right_top() + Vec2::new(-pad, pad), Align2::RIGHT_TOP, format!("{}   {}:{:02}", self.score, secs / 60, secs % 60), font, fg);
        if let Some(c) = self.crash {
            painter.rect_filled(xf.rect(self.cell_rect(c)), 0.0, Color32::from_rgb(230, 50, 50));
        }
    }
}

/// A word for a final score.
fn praise(score: u32) -> &'static str {
    match score {
        0..=4 => "Thanks for playing!",
        5..=14 => "Good job!",
        15..=29 => "Great job!",
        30..=49 => "Amazing!",
        50..=99 => "Perfect!",
        _ => "Unbelievable!",
    }
}

enum Choice {
    PlayAgain,
    Exit,
}

impl BioCartoon {
    /// Ctrl+Option+Cmd+T starts a game (a new one if one is open); while one is, the arrow keys
    /// steer it instead of nudging the selection, Escape closes it, and Enter on the Game Over
    /// popup plays again.
    pub fn snake_keys(&mut self, ctx: &egui::Context) {
        use egui::{Event, Key, Modifiers};
        let now = ctx.input(|i| i.time);
        // Option changes the typed character on a Mac, so match the physical key too.
        let start = ctx.input_mut(|i| {
            let n = i.events.len();
            i.events.retain(|e| {
                !matches!(e, Event::Key { key, physical_key, pressed: true, modifiers, .. }
                    if (*key == Key::T || *physical_key == Some(Key::T)) && modifiers.ctrl && modifiers.alt && modifiers.command)
            });
            i.events.len() != n
        });
        if start {
            self.snake = Some(Snake::new(&self.doc.slides[self.cur_slide], self.doc.page_rect(), now, &mut self.images, ctx));
        }
        let Some(snake) = &mut self.snake else { return };
        if snake.over.is_some() && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
            self.snake = Some(Snake::new(&self.doc.slides[self.cur_slide], self.doc.page_rect(), now, &mut self.images, ctx));
            return;
        }
        let close = ctx.input_mut(|i| {
            if i.consume_key(Modifiers::NONE, Key::Escape) {
                return true;
            }
            for (key, dir) in [(Key::ArrowUp, Dir::Up), (Key::ArrowDown, Dir::Down), (Key::ArrowLeft, Dir::Left), (Key::ArrowRight, Dir::Right)] {
                if i.consume_key(Modifiers::NONE, key) {
                    snake.turn(dir, now);
                }
            }
            false
        });
        if close {
            self.snake = None;
        }
    }

    /// Steps and draws the open game on the page, and the popup once it's over.
    pub fn snake_ui(&mut self, ui: &egui::Ui, painter: &Painter, xf: &crate::canvas::Xform, page_s: Rect, bg: Color32) {
        let Some(snake) = &mut self.snake else { return };
        let now = ui.input(|i| i.time);
        if let Some(wait) = snake.update(&self.doc.slides[self.cur_slide], now, &mut self.images) {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(wait));
        }
        snake.draw(painter, xf, page_s, bg, now);
        if snake.over.is_none() {
            return;
        }
        let score = snake.score;
        let mut choice = None;
        let modal = egui::Modal::new(egui::Id::new("snake_over_modal")).show(ui.ctx(), |ui| {
            ui.set_width(260.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(if snake.won { "You win" } else { "Game Over" }).size(26.0).strong());
                ui.add_space(4.0);
                ui.label(RichText::new(format!("Score: {score}")).size(16.0));
                ui.add_space(2.0);
                ui.label(if snake.won { "Congratulations!" } else { praise(score) });
                ui.add_space(12.0);
                let w = ui.available_width();
                let wide = |text: RichText| egui::Button::new(text).min_size(Vec2::new(w, 30.0)).corner_radius(6);
                if ui.add(wide(RichText::new("Play again").color(Color32::WHITE)).fill(theme::accent())).clicked() {
                    choice = Some(Choice::PlayAgain);
                }
                if ui.add(wide("Exit".into()).fill(theme::hover())).clicked() {
                    choice = Some(Choice::Exit);
                }
            });
        });
        match choice.or(modal.should_close().then_some(Choice::Exit)) {
            Some(Choice::PlayAgain) => self.snake = Some(Snake::new(&self.doc.slides[self.cur_slide], self.doc.page_rect(), now, &mut self.images, ui.ctx())),
            Some(Choice::Exit) => self.snake = None,
            None => {}
        }
    }
}

/// The visible mesh of each laid-out line, in page points. The editor uses the same
/// layout job and origin, so wrapping, alignment, and vertical alignment match.
fn text_rows(ctx: &Context, it: &Item) -> Option<Vec<Rect>> {
    let ItemKind::Text { text, .. } = &it.kind else { return None };
    let frame = it.rect();
    let job = crate::canvas::text_job(ctx, text, &it.kind, 1.0, frame.width(), it.opacity);
    let galley = ctx.fonts_mut(|fonts| fonts.layout_job(job));
    let origin = crate::canvas::text_origin(&galley, frame, &it.kind);
    Some(galley.rows.iter().filter_map(|row| {
        let bounds = row.visuals.mesh_bounds;
        bounds.is_positive().then(|| bounds.translate(origin.to_vec2() + row.pos.to_vec2()))
    }).collect())
}

/// A rotated line rectangle against an axis-aligned snake cell.
fn rotated_rect_hits(row: Rect, center: Pos2, angle: f32, cell: Rect) -> bool {
    let rot = egui::emath::Rot2::from_angle(angle);
    let corners = rect_corners(row).map(|p| center + rot * (p - center));
    let cell_corners = rect_corners(cell);
    let axes = [Vec2::X, Vec2::Y, corners[1] - corners[0], corners[3] - corners[0]];
    axes.into_iter().all(|axis| {
        let projection = |points: &[Pos2; 4]| {
            let values = points.map(|p| p.to_vec2().dot(axis));
            (values.into_iter().fold(f32::INFINITY, f32::min), values.into_iter().fold(f32::NEG_INFINITY, f32::max))
        };
        let (a0, a1) = projection(&corners);
        let (b0, b1) = projection(&cell_corners);
        a0 < b1 && b0 < a1
    })
}

/// Whether `it` covers any of `r` (page points): images by their rendered alpha, lines by
/// their path, shapes by their outline, other objects (and unloaded image placeholders) by
/// their bounding box.
fn item_hits(it: &Item, r: Rect, pixels: Option<&image::RgbaImage>) -> bool {
    if it.is_line() {
        let pts = crate::canvas::line_points(it);
        let reach = r.width() / 2.0;
        return pts.windows(2).any(|w| dist_to_segment(r.center(), w[0], w[1]) <= reach);
    }
    let b = Rect::from_points(&crate::canvas::corners(it));
    if !b.intersects(r) {
        return false;
    }
    if let ItemKind::Shape { shape, stroke, stroke_width, .. } = &it.kind {
        return shape_hits(it, *shape, if stroke[3] > 0 { *stroke_width } else { 0.0 }, r);
    }
    if let ItemKind::Image { crop, .. } = &it.kind {
        if it.opacity <= 0.0 {
            return false;
        }
        if let Some(img) = pixels {
            return image_hits(it, *crop, img, r);
        }
    }
    true
}

/// Test every visible texel touched by the cell, rather than sampling a few points (which
/// could skip a thin outline). Inverse-transform the cell to find the small pixel range to
/// inspect; the two rectangles' axes reject empty corners of rotated images precisely.
fn image_hits(it: &Item, crop: [f32; 4], img: &image::RgbaImage, cell: Rect) -> bool {
    let frame = it.rect();
    if frame.width() <= 0.0 || frame.height() <= 0.0 || img.width() == 0 || img.height() == 0 {
        return false;
    }
    let rot = egui::emath::Rot2::from_angle(it.rotation);
    let center = frame.center();
    let to_local = |p: Pos2| center + rot.inverse() * (p - center);
    let local_cell = Rect::from_points(&rect_corners(cell).map(to_local));
    let overlap = local_cell.intersect(frame);
    if !overlap.is_positive() {
        return false;
    }
    let (mut uv0, mut uv1) = (Pos2::new(crop[0], crop[1]), Pos2::new(crop[2], crop[3]));
    if it.flip_h {
        std::mem::swap(&mut uv0.x, &mut uv1.x);
    }
    if it.flip_v {
        std::mem::swap(&mut uv0.y, &mut uv1.y);
    }
    let dimensions = Vec2::new(img.width() as f32, img.height() as f32);
    let uv_size = uv1 - uv0;
    if uv_size.x == 0.0 || uv_size.y == 0.0 {
        return false;
    }
    let to_pixel = |p: Pos2| (uv0.to_vec2() + (p - frame.min) / frame.size() * uv_size) * dimensions;
    let pixel_range = Rect::from_two_pos(to_pixel(overlap.min).to_pos2(), to_pixel(overlap.max).to_pos2());
    let x0 = (pixel_range.min.x.floor().max(0.0) as u32).min(img.width());
    let y0 = (pixel_range.min.y.floor().max(0.0) as u32).min(img.height());
    let x1 = (pixel_range.max.x.ceil().max(0.0) as u32).min(img.width());
    let y1 = (pixel_range.max.y.ceil().max(0.0) as u32).min(img.height());
    let to_page = |p: Vec2| frame.min + (p / dimensions - uv0.to_vec2()) / uv_size * frame.size();
    for y in y0..y1 {
        for x in x0..x1 {
            // Ignore barely visible antialias fringes, but keep white artwork: colour is
            // unrelated to transparency, even when it happens to match the page.
            if img.get_pixel(x, y)[3] as f32 * it.opacity.clamp(0.0, 1.0) < 16.0 {
                continue;
            }
            let pixel = Rect::from_two_pos(to_page(Vec2::new(x as f32, y as f32)), to_page(Vec2::new(x as f32 + 1.0, y as f32 + 1.0))).intersect(frame);
            if !pixel.is_positive() || !pixel.intersects(local_cell) {
                continue;
            }
            let page_bounds = Rect::from_points(&rect_corners(pixel).map(|p| center + rot * (p - center)));
            if page_bounds.intersects(cell) {
                return true;
            }
        }
    }
    false
}

/// A shape's drawn outline (rounded corners, ellipses, stars...) against a snake cell. The
/// cell is carried into the shape's unrotated, unflipped frame, where the outline is built.
fn shape_hits(it: &Item, shape: crate::doc::ShapeKind, stroke_width: f32, cell: Rect) -> bool {
    let frame = it.rect();
    let (_, outline) = crate::canvas::shape_polygons(shape, frame);
    let center = frame.center();
    let rot = egui::emath::Rot2::from_angle(it.rotation).inverse();
    let quad = rect_corners(cell).map(|p| {
        let mut q = center + rot * (p - center);
        if it.flip_h {
            q.x = 2.0 * center.x - q.x;
        }
        if it.flip_v {
            q.y = 2.0 * center.y - q.y;
        }
        q
    });
    let edges = |poly: &[Pos2]| (0..poly.len()).map(|i| (poly[i], poly[(i + 1) % poly.len()])).collect::<Vec<_>>();
    let (shape_edges, cell_edges) = (edges(&outline), edges(&quad));
    quad.iter().any(|&p| inside(&outline, p))
        || outline.iter().any(|&p| inside(&quad, p))
        || shape_edges.iter().any(|&(a, b)| cell_edges.iter().any(|&(c, d)| segments_cross(a, b, c, d)))
        // The stroke straddles the outline, so its outer half reaches a little beyond it.
        || (stroke_width > 0.0 && shape_edges.iter().any(|&(a, b)| quad.iter().any(|&p| dist_to_segment(p, a, b) <= stroke_width / 2.0)))
}

/// Even-odd point-in-polygon test.
fn inside(poly: &[Pos2], p: Pos2) -> bool {
    let mut odd = false;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            odd = !odd;
        }
    }
    odd
}

fn segments_cross(a: Pos2, b: Pos2, c: Pos2, d: Pos2) -> bool {
    let side = |p: Pos2, q: Pos2, r: Pos2| (q - p).x * (r - p).y - (q - p).y * (r - p).x;
    let (d1, d2) = (side(c, d, a), side(c, d, b));
    let (d3, d4) = (side(a, b, c), side(a, b, d));
    d1 * d2 < 0.0 && d3 * d4 < 0.0
}

fn rect_corners(r: Rect) -> [Pos2; 4] {
    [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()]
}

fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = if ab.length_sq() > 0.0 { ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
    (a + ab * t).distance(p)
}

/// What shows behind the page's contents: its color, or the canvas through a see-through page.
fn backdrop(bg: Color32) -> Color32 {
    if bg.a() < 128 { crate::theme::canvas_bg() } else { bg.to_opaque() }
}

/// Black or white, whichever stands out against `bg`.
fn contrast(bg: Color32) -> Color32 {
    let bg = backdrop(bg);
    let lum = 0.299 * bg.r() as f32 + 0.587 * bg.g() as f32 + 0.114 * bg.b() as f32;
    if lum > 140.0 { Color32::BLACK } else { Color32::WHITE }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{ShapeKind, TextAlign};

    /// An 8 x 4 inch page: a 64 x 32 grid, the snake at (32, 16) facing right.
    fn game(slide: &Slide, images: &mut ImageCache) -> Snake {
        let ctx = egui::Context::default();
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        Snake::new(slide, Rect::from_min_size(Pos2::ZERO, Vec2::new(8.0 * DPI, 4.0 * DPI)), 0.0, images, &ctx)
    }

    fn run(s: &mut Snake, slide: &Slide, steps: u32, images: &mut ImageCache) {
        let now = s.next_step + STEP * (steps as f64 - 1.0) + 1e-6;
        s.update(slide, now, images);
    }

    #[test]
    fn waits_for_an_arrow_then_moves() {
        let slide = Slide::default();
        let mut images = ImageCache::new(String::new(), None);
        let mut s = game(&slide, &mut images);
        assert_eq!(s.body[0], (32, 16));
        assert_eq!(s.update(&slide, 10.0, &mut images), None);
        assert_eq!(s.body[0], (32, 16));
        s.turn(Dir::Down, 0.0);
        run(&mut s, &slide, 2, &mut images);
        assert_eq!(s.body[0], (32, 18));
        assert_eq!(s.body.len(), 3);
    }

    #[test]
    fn eating_grows_by_one() {
        let slide = Slide::default();
        let mut images = ImageCache::new(String::new(), None);
        let mut s = game(&slide, &mut images);
        s.food = (34, 16);
        s.turn(Dir::Right, 0.0);
        run(&mut s, &slide, 2, &mut images);
        assert_eq!((s.score, s.body.len()), (1, 4));
        assert_ne!(s.food, (34, 16));
        assert!(!s.body.contains(&s.food));
    }

    #[test]
    fn dots_stay_in_the_heads_enclosed_region() {
        let slide = Slide::default();
        let mut images = ImageCache::new(String::new(), None);
        let mut s = game(&slide, &mut images);
        // A wall divides the page, leaving plenty of empty cells beyond it.
        for y in 0..s.rows {
            s.walls[(y * s.cols + 35) as usize] = true;
        }
        for _ in 0..200 {
            assert!(s.place_food());
            assert!(s.food.0 < 35);
            assert!(!s.body.contains(&s.food));
        }
    }

    #[test]
    fn dots_avoid_dead_ends_until_the_final_cell() {
        let slide = Slide::default();
        let mut images = ImageCache::new(String::new(), None);
        let mut s = game(&slide, &mut images);
        s.body = VecDeque::from([(2, 2), (1, 2), (0, 2)]);
        s.walls.fill(true);
        for (x, y) in [(2, 2), (3, 2), (4, 2), (3, 1), (3, 0)] {
            s.walls[(y * s.cols + x) as usize] = false;
        }
        for _ in 0..100 {
            assert!(s.place_food());
            assert!([(3, 2), (3, 1)].contains(&s.food), "dot landed in a dead end: {:?}", s.food);
        }

        s.walls.fill(true);
        s.walls[(2 * s.cols + 2) as usize] = false;
        s.walls[(2 * s.cols + 3) as usize] = false;
        assert!(s.place_food());
        assert_eq!(s.food, (3, 2), "the last reachable cell can be a dead end");
    }

    #[test]
    fn filling_an_enclosed_region_wins_even_with_space_elsewhere() {
        let slide = Slide::default();
        let mut images = ImageCache::new(String::new(), None);
        let mut s = game(&slide, &mut images);
        s.body = VecDeque::from([(32, 16), (31, 16), (31, 15), (32, 15), (33, 15), (33, 16), (33, 17), (32, 17), (31, 17)]);
        assert!(!s.place_food(), "the snake encloses its head, despite open space outside");

        // The final bite fills a pocket while the rest of the page stays empty.
        s.body = VecDeque::from([(32, 16), (31, 16), (31, 15), (32, 15), (33, 15), (34, 15), (34, 16), (34, 17), (33, 17), (32, 17), (31, 17)]);
        s.food = (33, 16);
        s.dir = Dir::Right;
        s.step(&slide, 1.0, &mut images);
        assert_eq!(s.score, 1);
        assert_eq!(s.over, Some(1.0));
        assert!(s.won);
        assert!(s.crash.is_none());
        assert!(!s.blocked((40, 16)), "there are empty cells outside the filled pocket");
    }

    #[test]
    fn the_edge_ends_it() {
        let slide = Slide::default();
        let mut images = ImageCache::new(String::new(), None);
        let mut s = game(&slide, &mut images);
        s.food = (0, 0);
        s.turn(Dir::Right, 0.0);
        run(&mut s, &slide, 31, &mut images);
        assert!(s.over.is_none());
        run(&mut s, &slide, 1, &mut images);
        assert!(s.over.is_some());
    }

    #[test]
    fn its_own_body_ends_it() {
        let slide = Slide::default();
        let mut images = ImageCache::new(String::new(), None);
        let mut s = game(&slide, &mut images);
        s.food = (0, 0);
        s.body = (0..6).map(|i| (32 - i, 16)).collect();
        for d in [Dir::Down, Dir::Left, Dir::Up] {
            s.turn(d, 0.0);
            run(&mut s, &slide, 1, &mut images);
        }
        assert!(s.over.is_some());
    }

    #[test]
    fn objects_end_it_and_the_dot_avoids_them() {
        let mut slide = Slide::default();
        let shape = ItemKind::Shape { shape: ShapeKind::Rectangle, fill: [0; 4], stroke: [0; 4], stroke_width: 1.0 };
        slide.items.push(Item::new(1, "box", shape, Pos2::new(480.0, 100.0), Vec2::new(100.0, 100.0)));
        let mut images = ImageCache::new(String::new(), None);
        let mut s = game(&slide, &mut images);
        s.food = (0, 0);
        for _ in 0..200 {
            s.place_food();
            assert!(!s.blocked(s.food));
        }
        s.food = (0, 0);
        s.turn(Dir::Right, 0.0);
        run(&mut s, &slide, 7, &mut images);
        assert!(s.over.is_none());
        run(&mut s, &slide, 1, &mut images);
        assert!(s.over.is_some());
        assert_eq!(s.crash, Some((40, 16)));
    }

    #[test]
    fn rounded_corners_are_open_but_the_edges_are_walls() {
        let mut slide = Slide::default();
        let shape = ItemKind::Shape { shape: ShapeKind::RoundedRectangle, fill: [0, 0, 0, 255], stroke: [0; 4], stroke_width: 0.0 };
        // Cells 10..40 by 2..32 (the page's full height); the corner radius is 0.18 * 360
        // points, about 5 cells.
        slide.items.push(Item::new(1, "box", shape, Pos2::new(120.0, 24.0), Vec2::new(360.0, 360.0)));
        let mut images = ImageCache::new(String::new(), None);
        let s = game(&slide, &mut images);
        assert!(!s.blocked((10, 2)), "the rounded-off corner is open");
        assert!(!s.blocked((39, 31)), "every rounded-off corner is open");
        assert!(s.blocked((10, 17)), "the straight edge is a wall");
        assert!(s.blocked((12, 4)), "inside the curve is a wall");
        assert!(s.blocked((25, 17)), "the middle is a wall");
    }

    #[test]
    fn text_walls_follow_the_letters_instead_of_the_frame() {
        let mut slide = Slide::default();
        slide.items.push(Item::new(1, "text", ItemKind::text("Hi", 24.0, [0, 0, 0, 255], TextAlign::Left, false), Pos2::new(120.0, 96.0), Vec2::new(240.0, 96.0)));
        let mut images = ImageCache::new(String::new(), None);
        let mut s = game(&slide, &mut images);
        assert!(s.blocked((10, 8)), "letters are walls");
        assert!(!s.blocked((25, 8)), "empty width of the text frame is open");
        assert!(!s.blocked((10, 14)), "empty height of the text frame is open");

        if let ItemKind::Text { align, .. } = &mut slide.items[0].kind {
            *align = TextAlign::Right;
        }
        s.find_walls(&slide, &mut images);
        assert!(!s.blocked((10, 8)), "the old position is open after alignment changes");
        assert!(s.blocked((29, 8)), "right-aligned letters are walls");
    }

    fn icon(pos: Pos2, size: Vec2) -> Item {
        Item::new(1, "tube", ItemKind::Image {
            source: crate::doc::ImageSource::file("tube.svg".into()),
            crop: [0.0, 0.0, 1.0, 1.0], overlay: [0; 4], layers: Vec::new(),
        }, pos, size)
    }

    // A white tube silhouette with transparent space beneath its wide cap. Its drawing
    // fills the artboard's bounds, just as a trimmed catalog SVG does.
    const TUBE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="120"><path d="M0 0H120V24H84V96L60 120L36 96V24H0Z" fill="white"/></svg>"#;

    #[test]
    fn svg_background_is_passable_but_white_artwork_ends_the_game() {
        let ctx = egui::Context::default();
        let mut images = ImageCache::new(String::new(), None);
        images.insert_svg(&ctx, "file:tube.svg", TUBE).unwrap();
        let slide = Slide { items: vec![icon(Pos2::new(480.0, 96.0), Vec2::splat(120.0))], ..Slide::default() };
        let mut s = game(&slide, &mut images);
        assert!(!s.blocked((40, 16)), "transparent space inside the asset rectangle");
        assert!(s.blocked((44, 16)), "white tube artwork is still solid");
        for _ in 0..200 {
            s.place_food();
            assert!(!s.blocked(s.food));
        }
        // Eating in the transparent margin is allowed; moving into the tube is fatal.
        s.food = (41, 16);
        s.turn(Dir::Right, 0.0);
        run(&mut s, &slide, 9, &mut images);
        assert_eq!(s.score, 1);
        s.food = (0, 0);
        run(&mut s, &slide, 1, &mut images);
        assert_eq!(s.body[0], (42, 16));
        assert!(s.over.is_none());
        run(&mut s, &slide, 1, &mut images);
        assert_eq!(s.crash, Some((43, 16)));
    }

    #[test]
    fn pixel_boundaries_follow_flips_crop_and_rotation() {
        let mut it = icon(Pos2::new(100.0, 100.0), Vec2::splat(80.0));
        let mut img = image::RgbaImage::new(8, 8);
        img.put_pixel(1, 2, image::Rgba([255; 4]));
        let hit = |it: &Item, x, y| item_hits(it, Rect::from_center_size(Pos2::new(x, y), Vec2::splat(2.0)), Some(&img));
        assert!(hit(&it, 115.0, 125.0));
        assert!(!hit(&it, 165.0, 125.0));
        it.flip_h = true;
        assert!(!hit(&it, 115.0, 125.0));
        assert!(hit(&it, 165.0, 125.0));
        it.flip_v = true;
        assert!(hit(&it, 165.0, 155.0));
        assert!(!hit(&it, 165.0, 125.0));
        it.flip_h = false;
        it.flip_v = false;
        it.rotation = std::f32::consts::FRAC_PI_2;
        assert!(hit(&it, 155.0, 115.0));
        assert!(!hit(&it, 115.0, 125.0));
        it.rotation = 0.0;
        if let ItemKind::Image { crop, .. } = &mut it.kind {
            *crop = [0.0, 0.0, 0.5, 0.5];
        }
        assert!(hit(&it, 130.0, 150.0));
        assert!(!hit(&it, 115.0, 125.0));
        if let ItemKind::Image { crop, .. } = &mut it.kind {
            *crop = [0.5, 0.5, 1.0, 1.0];
        }
        assert!(!hit(&it, 130.0, 150.0), "cropped-out art is not an obstacle");
    }

    #[test]
    fn rotation_does_not_fill_empty_corners_and_thin_art_is_not_skipped() {
        let mut it = icon(Pos2::new(100.0, 100.0), Vec2::splat(80.0));
        it.rotation = std::f32::consts::FRAC_PI_4;
        let solid = image::RgbaImage::from_pixel(8, 8, image::Rgba([255; 4]));
        let outside = Rect::from_center_size(Pos2::new(90.0, 90.0), Vec2::splat(2.0));
        assert!(Rect::from_points(&crate::canvas::corners(&it)).intersects(outside));
        assert!(!item_hits(&it, outside, Some(&solid)));
        assert!(item_hits(&it, Rect::from_center_size(Pos2::new(140.0, 90.0), Vec2::splat(2.0)), Some(&solid)));

        let it = icon(Pos2::ZERO, Vec2::splat(80.0));
        let mut thin = image::RgbaImage::new(80, 80);
        thin.put_pixel(2, 2, image::Rgba([0, 0, 0, 255]));
        let cell = Rect::from_min_max(Pos2::new(1.8, 1.8), Pos2::new(10.2, 10.2));
        assert!(item_hits(&it, cell, Some(&thin)), "one-pixel artwork away from the cell center");
        thin.put_pixel(2, 2, image::Rgba([0, 0, 0, 1]));
        assert!(!item_hits(&it, cell, Some(&thin)), "nearly invisible antialias fringe");
    }

    #[test]
    fn walls_refresh_when_images_load_change_or_hide_layers() {
        let ctx = egui::Context::default();
        let mut images = ImageCache::new(String::new(), None);
        let mut slide = Slide { items: vec![icon(Pos2::new(480.0, 96.0), Vec2::splat(120.0))], ..Slide::default() };
        let mut s = game(&slide, &mut images);
        assert!(s.blocked((40, 16)), "unloaded placeholder");
        images.insert_svg(&ctx, "file:tube.svg", TUBE).unwrap();
        s.update(&slide, 0.0, &mut images);
        assert!(!s.blocked((40, 16)));
        assert!(s.blocked((44, 16)));

        let hidden = vec![crate::doc::LayerStyle { colors: vec![[255; 3]], color: None, opacity: 0.0 }];
        if let ItemKind::Image { layers, .. } = &mut slide.items[0].kind {
            *layers = hidden.clone();
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while images.restyled("file:tube.svg", &hidden).is_none() {
            assert!(std::time::Instant::now() < deadline, "restyled SVG did not load");
            std::thread::sleep(std::time::Duration::from_millis(10));
            images.poll(&ctx);
        }
        s.update(&slide, 0.0, &mut images);
        assert!(!s.blocked((44, 16)), "hidden layers leave no obstacle");

        // Replacing the image under the same cache key must also refresh its collision art.
        images.insert_image(&ctx, "file:tube.svg", image::RgbaImage::from_pixel(8, 8, image::Rgba([255; 4])));
        s.update(&slide, 0.0, &mut images);
        assert!(s.blocked((40, 16)));
        slide.items[0].opacity = 0.0;
        s.update(&slide, 0.0, &mut images);
        assert!(!s.blocked((40, 16)), "invisible image");
    }
}
