//! A hidden snake game on the page: Ctrl+Option+Cmd+T starts one (or a new one), the arrow keys
//! steer, and Escape puts it away. The snake and the dot are drawn in whichever of black or white
//! stands out against the page; the page edge, the snake itself and anything on the slide end
//! the game, and the cells that count as obstacles are shaded faintly. Nothing runs while no
//! game is open (`BioCartoon::snake` is `None`).

use crate::app::BioCartoon;
use crate::doc::{Item, Slide, DPI};
use crate::theme;
use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, RichText, Vec2};
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
    crash: Option<(i32, i32)>,
    /// Which cells (row-major) something on the slide covers, as of the last step.
    walls: Vec<bool>,
    rng: u64,
}

impl Snake {
    pub fn new(slide: &Slide, page: Rect, now: f64) -> Self {
        let cell = CELL;
        let (cols, rows) = (((page.width() / cell) as i32).max(4), ((page.height() / cell) as i32).max(4));
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
        let mut s = Self { cell, cols, rows, body: VecDeque::new(), dir: Dir::Right, turns: VecDeque::new(), food: (0, 0), score: 0, started: None, next_step: now, over: None, crash: None, walls: Vec::new(), rng: seed | 1 };
        s.find_walls(slide);
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
        s.place_food();
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
    pub fn update(&mut self, slide: &Slide, now: f64) -> Option<f64> {
        if self.started.is_none() {
            // Objects moved before the first arrow key still count.
            self.find_walls(slide);
            return None;
        }
        while self.over.is_none() && now >= self.next_step {
            self.step(slide, self.next_step);
            self.next_step += STEP;
        }
        if self.over.is_some() {
            return None;
        }
        Some(self.next_step - now)
    }

    fn step(&mut self, slide: &Slide, at: f64) {
        self.find_walls(slide);
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
            self.place_food();
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

    fn place_food(&mut self) {
        let ok = |s: &Snake, c: (i32, i32)| !s.body.contains(&c) && !s.blocked(c);
        for _ in 0..500 {
            let c = (self.rand(self.cols), self.rand(self.rows));
            if ok(self, c) {
                self.food = c;
                return;
            }
        }
        // A crowded page: take the first free cell, if any.
        if let Some(c) = (0..self.rows).flat_map(|y| (0..self.cols).map(move |x| (x, y))).find(|&c| ok(self, c)) {
            self.food = c;
        }
    }

    fn cell_rect(&self, (x, y): (i32, i32)) -> Rect {
        Rect::from_min_size(Pos2::new(x as f32 * self.cell, y as f32 * self.cell), Vec2::splat(self.cell))
    }

    /// Off the grid, or on something on the slide.
    fn blocked(&self, c: (i32, i32)) -> bool {
        c.0 < 0 || c.1 < 0 || c.0 >= self.cols || c.1 >= self.rows || self.walls[(c.1 * self.cols + c.0) as usize]
    }

    fn find_walls(&mut self, slide: &Slide) {
        let inset = self.cell * 0.15;
        let cells = (0..self.rows).flat_map(|y| (0..self.cols).map(move |x| (x, y)));
        self.walls = cells.map(|c| {
            let r = self.cell_rect(c).shrink(inset);
            slide.items.iter().any(|it| item_hits(it, r))
        }).collect();
    }

    pub fn draw(&self, painter: &Painter, xf: &crate::canvas::Xform, page_s: Rect, bg: Color32, now: f64) {
        let fg = contrast(bg);
        let z = xf.zoom;
        // What counts as an obstacle, faintly, so it's clear where an icon's box ends.
        let wall = fg.gamma_multiply(0.12);
        for (i, _) in self.walls.iter().enumerate().filter(|(_, w)| **w) {
            let c = (i as i32 % self.cols, i as i32 / self.cols);
            painter.rect_filled(xf.rect(self.cell_rect(c)).shrink(0.5), 0.0, wall);
        }
        for &c in &self.body {
            painter.rect_filled(xf.rect(self.cell_rect(c)).shrink(0.5), 0.0, fg);
        }
        painter.rect_filled(xf.rect(self.cell_rect(self.food)).shrink(self.cell * z * 0.15), 0.0, fg);

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
            self.snake = Some(Snake::new(self.slide(), self.doc.page_rect(), now));
        }
        let Some(snake) = &mut self.snake else { return };
        if snake.over.is_some() && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
            self.snake = Some(Snake::new(self.slide(), self.doc.page_rect(), now));
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
        if let Some(wait) = snake.update(&self.doc.slides[self.cur_slide], now) {
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
                ui.label(RichText::new("Game Over").size(26.0).strong());
                ui.add_space(4.0);
                ui.label(RichText::new(format!("Score: {score}")).size(16.0));
                ui.add_space(2.0);
                ui.label(praise(score));
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
            Some(Choice::PlayAgain) => self.snake = Some(Snake::new(self.slide(), self.doc.page_rect(), now)),
            Some(Choice::Exit) => self.snake = None,
            None => {}
        }
    }
}

/// Whether `it` covers any of `r` (page points): lines by their path, everything else by its
/// bounding box (rotated items by the box around their corners).
fn item_hits(it: &Item, r: Rect) -> bool {
    if it.is_line() {
        let pts = crate::canvas::line_points(it);
        let reach = r.width() / 2.0;
        return pts.windows(2).any(|w| dist_to_segment(r.center(), w[0], w[1]) <= reach);
    }
    let b = Rect::from_points(&crate::canvas::corners(it));
    b.intersects(r)
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
    use crate::doc::{ItemKind, ShapeKind};

    /// An 8 x 4 inch page: a 64 x 32 grid, the snake at (32, 16) facing right.
    fn game(slide: &Slide) -> Snake {
        Snake::new(slide, Rect::from_min_size(Pos2::ZERO, Vec2::new(8.0 * DPI, 4.0 * DPI)), 0.0)
    }

    fn run(s: &mut Snake, slide: &Slide, steps: u32) {
        let now = s.next_step + STEP * (steps as f64 - 1.0) + 1e-6;
        s.update(slide, now);
    }

    #[test]
    fn waits_for_an_arrow_then_moves() {
        let slide = Slide::default();
        let mut s = game(&slide);
        assert_eq!(s.body[0], (32, 16));
        assert_eq!(s.update(&slide, 10.0), None);
        assert_eq!(s.body[0], (32, 16));
        s.turn(Dir::Down, 0.0);
        run(&mut s, &slide, 2);
        assert_eq!(s.body[0], (32, 18));
        assert_eq!(s.body.len(), 3);
    }

    #[test]
    fn eating_grows_by_one() {
        let slide = Slide::default();
        let mut s = game(&slide);
        s.food = (34, 16);
        s.turn(Dir::Right, 0.0);
        run(&mut s, &slide, 2);
        assert_eq!((s.score, s.body.len()), (1, 4));
        assert_ne!(s.food, (34, 16));
        assert!(!s.body.contains(&s.food));
    }

    #[test]
    fn the_edge_ends_it() {
        let slide = Slide::default();
        let mut s = game(&slide);
        s.food = (0, 0);
        s.turn(Dir::Right, 0.0);
        run(&mut s, &slide, 31);
        assert!(s.over.is_none());
        run(&mut s, &slide, 1);
        assert!(s.over.is_some());
    }

    #[test]
    fn its_own_body_ends_it() {
        let slide = Slide::default();
        let mut s = game(&slide);
        s.food = (0, 0);
        s.body = (0..6).map(|i| (32 - i, 16)).collect();
        for d in [Dir::Down, Dir::Left, Dir::Up] {
            s.turn(d, 0.0);
            run(&mut s, &slide, 1);
        }
        assert!(s.over.is_some());
    }

    #[test]
    fn objects_end_it_and_the_dot_avoids_them() {
        let mut slide = Slide::default();
        let shape = ItemKind::Shape { shape: ShapeKind::Rectangle, fill: [0; 4], stroke: [0; 4], stroke_width: 1.0 };
        slide.items.push(Item::new(1, "box", shape, Pos2::new(480.0, 100.0), Vec2::new(100.0, 100.0)));
        let mut s = game(&slide);
        s.food = (0, 0);
        for _ in 0..200 {
            s.place_food();
            assert!(!s.blocked(s.food));
        }
        s.food = (0, 0);
        s.turn(Dir::Right, 0.0);
        run(&mut s, &slide, 7);
        assert!(s.over.is_none());
        run(&mut s, &slide, 1);
        assert!(s.over.is_some());
        assert_eq!(s.crash, Some((40, 16)));
    }
}
