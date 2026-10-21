//! Spelling and grammar checking of the document's text boxes and table cells.
//!
//! Spelling uses a Hunspell dictionary ([`spellbook`]): the SCOWL general English (US) word list,
//! plus a medical dictionary (en-med-glut) and a molecular and cell biology list
//! (`assets/dict/biology.txt`), so words like "intron", "plasmid" or "transcriptome" are not
//! flagged. Words with digits, acronyms and mixed-case names (BRCA1, mRNA, SpCas9) are skipped.
//! Grammar is a handful of rules: repeated words, "a" vs "an", a space before punctuation, a
//! lowercase sentence start, "could of" and "more then".

use crate::app::{Action, BioCartoon};
use crate::doc::{Document, ItemId, ItemKind};
use crate::theme;
use egui::{Color32, Vec2};
use egui_phosphor::regular as ph;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::ops::Range;
use std::sync::OnceLock;

const AFF: &str = include_str!("../assets/dict/en_US.aff");
const DIC_EN: &[u8] = include_bytes!("../assets/dict/en_US.dic.gz");
const DIC_MED: &[u8] = include_bytes!("../assets/dict/en_med.dic.gz");
const DIC_BIO: &str = include_str!("../assets/dict/biology.txt");

/// What is shown under the issue list.
pub const DICTIONARY_LABEL: &str = "English (US), with medical and biology terms";

fn gunzip(bytes: &[u8]) -> String {
    let mut s = String::new();
    let _ = flate2::read::GzDecoder::new(bytes).read_to_string(&mut s);
    s
}

/// The dictionary, loaded on first use (a fraction of a second).
pub fn dictionary() -> &'static spellbook::Dictionary {
    static DICT: OnceLock<spellbook::Dictionary> = OnceLock::new();
    DICT.get_or_init(|| {
        let mut dict = spellbook::Dictionary::new(AFF, &gunzip(DIC_EN)).expect("bundled en_US dictionary");
        let med = gunzip(DIC_MED);
        let words = med.lines().chain(DIC_BIO.lines()).map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#'));
        for w in words {
            // The medical list uses en_US's affix flags; a word whose flags don't parse goes in bare.
            if dict.add(w).is_err() {
                let _ = dict.add(w.split('/').next().unwrap_or(w));
            }
        }
        dict
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IssueKind {
    Spelling,
    Grammar,
}

/// Where a checked string lives: an item on a slide, and for tables the cell index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Place {
    pub slide: usize,
    pub item: ItemId,
    pub cell: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct Issue {
    pub place: Place,
    pub kind: IssueKind,
    /// Byte range of the problem in the string.
    pub range: Range<usize>,
    /// The flagged text (checked again before a fix is applied).
    pub found: String,
    pub message: String,
    /// Fixed replacements from grammar rules (spelling suggestions come from [`SpellState::suggestions`]).
    pub fixes: Vec<String>,
}

impl Issue {
    /// What "Ignore" remembers: the word for spelling, the place and text for grammar.
    pub fn ignore_key(&self) -> String {
        match self.kind {
            IssueKind::Spelling => self.found.clone(),
            IssueKind::Grammar => format!("{:?}|{}|{}", self.place, self.message, self.found),
        }
    }
}

/// The open Spelling and grammar window.
#[derive(Default)]
pub struct SpellState {
    pub issues: Vec<Issue>,
    /// Fingerprint of the texts and settings the issues were found for.
    checked: Option<u64>,
    /// Ignored for this session (see [`Issue::ignore_key`]).
    pub ignored: HashSet<String>,
    /// Spelling suggestions by word, filled in a few per frame (suggesting is slow).
    pub suggestions: HashMap<String, Vec<String>>,
}

impl SpellState {
    /// Re-checks the document if its text (or the ignore lists) changed since the last check;
    /// true if it did.
    pub fn refresh(&mut self, doc: &Document, custom: &[String], grammar: bool) -> bool {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for (place, text) in texts(doc) {
            place.hash(&mut h);
            text.hash(&mut h);
        }
        custom.hash(&mut h);
        grammar.hash(&mut h);
        let mut ig: Vec<&String> = self.ignored.iter().collect();
        ig.sort();
        ig.hash(&mut h);
        let fp = h.finish();
        if self.checked == Some(fp) {
            return false;
        }
        self.checked = Some(fp);
        let custom: HashSet<String> = custom.iter().map(|w| w.to_lowercase()).collect();
        self.issues = texts(doc)
            .into_iter()
            .flat_map(|(place, text)| check(place, text, &custom, grammar))
            .filter(|i| !self.ignored.contains(&i.ignore_key()))
            .collect();
        true
    }

    /// The issues in one string (an item's text, or one table cell).
    pub fn at(&self, item: ItemId, cell: Option<usize>) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(move |i| i.place.item == item && i.place.cell == cell)
    }

    /// Computes suggestions for up to `budget` flagged words; true if some are still missing.
    pub fn fill_suggestions(&mut self, budget: usize) -> bool {
        let missing: Vec<String> = self
            .issues
            .iter()
            .filter(|i| i.kind == IssueKind::Spelling && !self.suggestions.contains_key(&i.found))
            .map(|i| i.found.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        for w in missing.iter().take(budget) {
            let mut out = Vec::new();
            dictionary().suggest(&normalize(w), &mut out);
            out.truncate(5);
            self.suggestions.insert(w.clone(), out);
        }
        missing.len() > budget
    }
}

/// Every non-empty text in the document, with where it lives.
pub fn texts(doc: &Document) -> Vec<(Place, &str)> {
    let mut out = Vec::new();
    for (s, slide) in doc.slides.iter().enumerate() {
        for it in &slide.items {
            match &it.kind {
                ItemKind::Text { text, .. } => out.push((Place { slide: s, item: it.id, cell: None }, text.as_str())),
                ItemKind::Table { cells, .. } => {
                    for (c, t) in cells.iter().enumerate() {
                        out.push((Place { slide: s, item: it.id, cell: Some(c) }, t.as_str()));
                    }
                }
                _ => {}
            }
        }
    }
    out.retain(|(_, t)| !t.trim().is_empty());
    out
}

/// The string at `place`, for applying a fix.
pub fn text_mut(doc: &mut Document, place: Place) -> Option<&mut String> {
    let it = doc.slides.get_mut(place.slide)?.items.iter_mut().find(|i| i.id == place.item)?;
    match (&mut it.kind, place.cell) {
        (ItemKind::Text { text, .. }, None) => Some(text),
        (ItemKind::Table { cells, .. }, Some(c)) => cells.get_mut(c),
        _ => None,
    }
}

/// A word (letters, digits and inner apostrophes) and its byte range.
struct Tok<'a> {
    start: usize,
    end: usize,
    text: &'a str,
}

fn is_apostrophe(c: char) -> bool {
    c == '\'' || c == '\u{2019}'
}

fn tokens(s: &str) -> Vec<Tok<'_>> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    for (k, &(i, c)) in chars.iter().enumerate() {
        let inner_apostrophe = is_apostrophe(c)
            && start.is_some()
            && chars.get(k + 1).is_some_and(|&(_, n)| n.is_alphabetic());
        if c.is_alphanumeric() || inner_apostrophe {
            start.get_or_insert(i);
        } else if let Some(st) = start.take() {
            out.push(Tok { start: st, end: i, text: &s[st..i] });
        }
    }
    if let Some(st) = start {
        out.push(Tok { start: st, end: s.len(), text: &s[st..] });
    }
    out
}

fn normalize(w: &str) -> String {
    w.replace('\u{2019}', "'")
}

/// Words the speller leaves alone: numbers and words with digits, single letters, acronyms,
/// mixed-case names (mRNA, SpCas9) and words in other scripts (Greek letters).
fn skip_word(w: &str) -> bool {
    let letters = w.chars().filter(|c| c.is_alphabetic()).count();
    letters < 2
        || w.chars().any(|c| c.is_numeric())
        || w.chars().any(|c| c.is_alphabetic() && c as u32 > 0x24F)
        || w.chars().skip(1).any(|c| c.is_uppercase())
}

const PREFIXES: &[&str] = &[
    "pre", "post", "non", "anti", "co", "re", "sub", "multi", "trans", "cis", "bi", "tri", "intra", "inter", "extra", "auto", "semi", "de",
    "un", "mono", "poly", "hetero", "homo", "hyper", "hypo", "micro", "macro", "nano", "mega", "meta", "para", "pan", "pro", "neo", "mid",
    "mis", "over", "under", "ultra", "endo", "exo", "epi", "peri", "pseudo", "quasi", "self", "single", "double", "wild", "dox", "ortho",
];

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric()
}

/// The issues in one string.
pub fn check(place: Place, s: &str, custom: &HashSet<String>, grammar: bool) -> Vec<Issue> {
    let dict = dictionary();
    let toks = tokens(s);
    let issue = |kind, range: Range<usize>, message: &str, fixes: Vec<String>| Issue {
        place,
        kind,
        found: s[range.clone()].to_string(),
        range,
        message: message.into(),
        fixes,
    };
    let mut out = Vec::new();
    for t in &toks {
        let w = normalize(t.text);
        if skip_word(&w) || custom.contains(&w.to_lowercase()) {
            continue;
        }
        // Prefixes of hyphenated words: "pre-mRNA", "co-culture".
        if s[t.end..].starts_with('-') && PREFIXES.contains(&w.to_lowercase().as_str()) {
            continue;
        }
        // Possessives and plurals of names ("Golgi's") are checked without the "'s".
        let stem = w.strip_suffix("'s").unwrap_or(&w);
        if !dict.check(&w) && !dict.check(stem) && !custom.contains(&stem.to_lowercase()) {
            out.push(issue(IssueKind::Spelling, t.start..t.end, "Not in the dictionary", Vec::new()));
        }
    }
    if !grammar {
        return out;
    }
    let between = |a: &Tok, b: &Tok| &s[a.end..b.start];
    for (k, t) in toks.iter().enumerate() {
        let lower = t.text.to_lowercase();
        let next = toks.get(k + 1);
        let gap_is_space = |n: &Tok| {
            let g = between(t, n);
            !g.is_empty() && g.chars().all(|c| c == ' ' || c == '\t')
        };
        // Repeated word: "the the".
        if let Some(n) = next {
            if gap_is_space(n) && n.text.to_lowercase() == lower && !lower.chars().any(|c| c.is_numeric()) && !matches!(lower.as_str(), "that" | "had") {
                out.push(issue(IssueKind::Grammar, t.start..n.end, "Repeated word", vec![t.text.to_string()]));
            }
        }
        // "a" vs "an".
        if let (Some(n), "a" | "an") = (next, lower.as_str()) {
            if gap_is_space(n) {
                if let Some(want_an) = wants_an(n.text) {
                    if want_an != (lower == "an") {
                        let fix = match (t.text, want_an) {
                            ("A", true) => "An",
                            ("AN" | "An", false) => "A",
                            (_, true) => "an",
                            (_, false) => "a",
                        };
                        out.push(issue(IssueKind::Grammar, t.start..t.end, &format!("Use \"{}\" before \"{}\"", fix.to_lowercase(), n.text), vec![fix.to_string()]));
                    }
                }
            }
        }
        // "could of", "more then".
        if let Some(n) = next {
            let nl = n.text.to_lowercase();
            if gap_is_space(n) {
                let fix = match (lower.as_str(), nl.as_str()) {
                    ("could" | "should" | "would" | "must" | "might", "of") => Some("have"),
                    ("more" | "less" | "rather" | "greater" | "higher" | "lower" | "better" | "worse" | "larger" | "smaller" | "faster" | "slower" | "other", "then") => Some("than"),
                    _ => None,
                };
                if let Some(f) = fix {
                    let f = if n.text.starts_with(char::is_uppercase) { capitalize(f) } else { f.to_string() };
                    out.push(issue(IssueKind::Grammar, n.start..n.end, &format!("Did you mean \"{} {}\"?", t.text, f), vec![f]));
                }
            }
        }
        // A lone lowercase "i".
        if t.text == "i" && !s[t.end..].starts_with('.') && !s[..t.start].ends_with('.') {
            out.push(issue(IssueKind::Grammar, t.start..t.end, "Capitalize \"I\"", vec!["I".into()]));
        }
        // A sentence starting with a lowercase letter: ". the" (not after "e.g.", "et al." etc.).
        if let Some(n) = next {
            let gap = between(t, n);
            let ends_sentence = matches!(gap.trim_end_matches([' ', '"', '\u{201D}', ')']).chars().last(), Some('.' | '?' | '!'))
                && gap.trim_start().starts_with(['.', '?', '!'])
                && gap.contains(' ')
                && !gap.contains("..")
                && !gap.contains('\n');
            let abbreviation = t.text.chars().count() == 1
                || matches!(lower.as_str(), "al" | "eg" | "ie" | "etc" | "vs" | "fig" | "figs" | "approx" | "cf" | "ca" | "resp" | "no" | "nos" | "ref" | "refs" | "eq" | "eqs" | "spp" | "sp" | "var" | "dr" | "mr" | "ms" | "mrs" | "st" | "min" | "max" | "conc" | "vol")
                || s[..t.start].ends_with('.');
            let first = n.text.chars().next().unwrap_or(' ');
            let lowercase_word = first.is_lowercase() && !n.text.chars().skip(1).any(char::is_uppercase);
            if ends_sentence && !abbreviation && lowercase_word && (n.text == "a" || !skip_word(n.text)) {
                out.push(issue(IssueKind::Grammar, n.start..n.end, "Start the sentence with a capital letter", vec![capitalize(n.text)]));
            }
        }
    }
    // A space before punctuation: "cells , which".
    let b = s.as_bytes();
    for (i, c) in s.char_indices() {
        if matches!(c, ',' | ';' | ':' | '!' | '?' | '.') && i >= 2 && b[i - 1] == b' ' {
            let before = s[..i - 1].trim_end_matches(' ');
            let after = s[i + 1..].chars().next();
            let word_before = before.chars().last().is_some_and(is_word_char);
            let ends_here = after.is_none_or(|a| a.is_whitespace());
            if word_before && ends_here {
                let start = before.len();
                out.push(issue(IssueKind::Grammar, start..i + 1, "Remove the space before the punctuation", vec![c.to_string()]));
            }
        }
    }
    out.sort_by_key(|i| i.range.start);
    out
}

fn capitalize(w: &str) -> String {
    let mut c = w.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// Whether `word` takes "an" (true) or "a" (false); `None` when unsure (numbers, "u" and "h"
/// words, which depend on how they are said).
fn wants_an(word: &str) -> Option<bool> {
    if word.chars().any(|c| c.is_numeric()) {
        return None;
    }
    let first = word.chars().next()?;
    let letters = word.chars().filter(|c| c.is_alphabetic()).count();
    // Acronyms are read letter by letter: "an RNA", "a DNA", "an mRNA", "an sgRNA".
    let acronym = letters <= 6 && word.chars().all(|c| !c.is_alphabetic() || c.is_uppercase());
    let prefixed = first.is_lowercase() && word.chars().nth(1).is_some_and(char::is_uppercase);
    if letters == 1 || acronym || prefixed {
        return Some("AEFHILMNORSX".contains(first.to_ascii_uppercase()));
    }
    let w = word.to_lowercase();
    let starts = |ps: &[&str]| ps.iter().any(|p| w.starts_with(p));
    if starts(&["hour", "honest", "honor", "honour", "heir"]) {
        return Some(true);
    }
    if starts(&["uni", "use", "usu", "uti", "ute", "ure", "uri", "uro", "ura", "ubiq", "eu", "ewe", "one", "once", "uk"]) {
        return Some(false);
    }
    match w.chars().next()? {
        'u' => starts(&["un", "um", "up", "ult", "ug", "ush"]).then_some(true),
        'h' => None,
        'a' | 'e' | 'i' | 'o' => Some(true),
        c if c.is_ascii_alphabetic() => Some(false),
        _ => None,
    }
}

/// What the window asked for this frame.
enum Cmd {
    Fix(usize, String),
    Ignore(usize),
    AddWord(String),
    Show(Place),
}

impl BioCartoon {
    /// Squiggles are drawn while "View spelling and grammar" is on, or the window is open.
    pub fn spelling_visible(&self) -> bool {
        self.settings.show_spelling || self.show_spell_window
    }

    /// Re-checks the document when squiggles or the window need it (cheap when nothing changed).
    pub fn refresh_spelling(&mut self, ctx: &egui::Context) {
        if self.spelling_visible() && self.spell.refresh(&self.doc, &self.settings.custom_words, self.settings.check_grammar) {
            ctx.request_repaint();
        }
    }

    /// The Spelling and grammar window (main menu > Check spelling and grammar).
    pub fn spell_window(&mut self, ctx: &egui::Context) {
        if !self.show_spell_window {
            return;
        }
        let st = &mut self.spell;
        st.refresh(&self.doc, &self.settings.custom_words, self.settings.check_grammar);
        if st.fill_suggestions(3) {
            ctx.request_repaint();
        }
        let mut open = true;
        let mut cmd: Option<Cmd> = None;
        let mut grammar = self.settings.check_grammar;
        let doc = &self.doc;
        let center = ctx.content_rect().center();
        egui::Window::new(format!("{}  Spelling and grammar", ph::TEXT_AA))
            .open(&mut open)
            .collapsible(false)
            .default_size([420.0, 520.0])
            .default_pos(center - Vec2::new(210.0, 260.0))
            .show(ctx, |ui| {
                let n = st.issues.len();
                ui.horizontal(|ui| {
                    if n == 0 {
                        ui.label(egui::RichText::new(format!("{}  No problems found", ph::CHECK_CIRCLE)).color(theme::text()));
                    } else {
                        ui.label(format!("{n} {} in the text boxes and tables", if n == 1 { "problem" } else { "problems" }));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.checkbox(&mut grammar, "Grammar");
                    });
                });
                ui.separator();
                egui::ScrollArea::vertical().max_height(400.0).auto_shrink([false, true]).show(ui, |ui| {
                    for (k, is) in st.issues.iter().enumerate() {
                        egui::Frame::new().fill(theme::panel_raised()).corner_radius(6).inner_margin(8).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            let item = doc.slides.get(is.place.slide).and_then(|s| s.items.iter().find(|i| i.id == is.place.item));
                            let mut where_ = format!("Slide {}", is.place.slide + 1);
                            if let Some(it) = item {
                                let name: String = it.name.chars().take(28).collect();
                                where_ += &format!(" · {name}{}", if name.len() < it.name.len() { "..." } else { "" });
                                if let (Some(c), ItemKind::Table { cols, .. }) = (is.place.cell, &it.kind) {
                                    where_ += &format!(", row {} column {}", c / cols.max(&1) + 1, c % cols.max(&1) + 1);
                                }
                            }
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(where_).small().color(theme::text_dim()));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.add(egui::Button::new(egui::RichText::new(ph::CROSSHAIR).small()).frame(false)).on_hover_text("Show on the slide").clicked() {
                                        cmd = Some(Cmd::Show(is.place));
                                    }
                                });
                            });
                            if let Some(text) = texts(doc).into_iter().find(|(p, _)| *p == is.place).map(|(_, t)| t) {
                                ui.label(context_job(ui, text, is));
                            }
                            let icon = if is.kind == IssueKind::Spelling { ph::TEXT_AA } else { ph::LIGHTBULB };
                            ui.label(egui::RichText::new(format!("{icon}  {}", is.message)).small().color(squiggle_color(is.kind)));
                            ui.horizontal_wrapped(|ui| {
                                let fixes = match is.kind {
                                    IssueKind::Spelling => st.suggestions.get(&is.found).cloned(),
                                    IssueKind::Grammar => Some(is.fixes.clone()),
                                };
                                match fixes {
                                    None => {
                                        ui.label(egui::RichText::new("Finding suggestions...").small().italics().color(theme::text_dim()));
                                    }
                                    Some(f) if f.is_empty() && is.kind == IssueKind::Spelling => {
                                        ui.label(egui::RichText::new("No suggestions").small().color(theme::text_dim()));
                                    }
                                    Some(f) => {
                                        for fix in f {
                                            let b = egui::Button::new(egui::RichText::new(&fix).strong()).stroke(egui::Stroke::new(1.0, theme::accent())).corner_radius(10);
                                            if ui.add(b).on_hover_text("Replace").clicked() {
                                                cmd = Some(Cmd::Fix(k, fix));
                                            }
                                        }
                                    }
                                }
                                if ui.button("Ignore").clicked() {
                                    cmd = Some(Cmd::Ignore(k));
                                }
                                if is.kind == IssueKind::Spelling && ui.button("Add to dictionary").clicked() {
                                    cmd = Some(Cmd::AddWord(is.found.clone()));
                                }
                            });
                        });
                        ui.add_space(4.0);
                    }
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("Dictionary: {DICTIONARY_LABEL}")).small().color(theme::text_dim()));
                    let added = self.settings.custom_words.len();
                    if added > 0 {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let r = ui.add(egui::Button::new(egui::RichText::new(format!("{added} added")).small()).frame(false));
                            if r.on_hover_text(format!("{}\nClick to remove the words you added", self.settings.custom_words.join(", "))).clicked() {
                                self.settings.custom_words.clear();
                            }
                        });
                    }
                });
            });
        self.settings.check_grammar = grammar;
        match cmd {
            Some(Cmd::Fix(k, fix)) => {
                if let Some(is) = st.issues.get(k).cloned() {
                    let current = text_mut(&mut self.doc, is.place).and_then(|t| t.get(is.range.clone()).map(str::to_string));
                    if current.as_deref() == Some(is.found.as_str()) {
                        self.commit();
                        if let Some(t) = text_mut(&mut self.doc, is.place) {
                            t.replace_range(is.range.clone(), &fix);
                        }
                    }
                }
            }
            Some(Cmd::Ignore(k)) => {
                if let Some(is) = st.issues.get(k) {
                    st.ignored.insert(is.ignore_key());
                }
            }
            Some(Cmd::AddWord(w)) => {
                let w = normalize(&w);
                if !self.settings.custom_words.contains(&w) {
                    self.settings.custom_words.push(w);
                }
            }
            Some(Cmd::Show(place)) => {
                self.actions.push(Action::GoSlide(place.slide));
                self.actions.push(Action::Select(vec![place.item]));
            }
            None => {}
        }
        if !open {
            self.show_spell_window = false;
        }
    }
}

pub fn squiggle_color(kind: IssueKind) -> Color32 {
    match kind {
        IssueKind::Spelling => Color32::from_rgb(0xe0, 0x40, 0x40),
        IssueKind::Grammar => Color32::from_rgb(0x3a, 0x7b, 0xe0),
    }
}

/// Wavy underlines under `issues` in `galley` (laid out from `text`), drawn with `map` taking
/// galley coordinates to the screen (for placing and rotating). `scale` is the zoom.
pub fn paint_squiggles<'a>(painter: &egui::Painter, galley: &egui::Galley, text: &str, issues: impl Iterator<Item = &'a Issue>, scale: f32, map: impl Fn(egui::Pos2) -> egui::Pos2) {
    let amp = (1.3 * scale).clamp(0.8, 3.0);
    let period = amp * 3.0;
    for is in issues {
        // A stale issue (the text changed this frame) is skipped until the next check.
        if text.get(is.range.clone()) != Some(is.found.as_str()) {
            continue;
        }
        let (c0, c1) = (text[..is.range.start].chars().count(), text[..is.range.end].chars().count());
        let mut k = 0;
        for placed in &galley.rows {
            let row = &placed.row;
            let (mut x0, mut x1) = (f32::INFINITY, f32::NEG_INFINITY);
            for g in &row.glyphs {
                if k >= c0 && k < c1 && !g.chr.is_whitespace() {
                    x0 = x0.min(g.pos.x);
                    x1 = x1.max(g.pos.x + g.advance_width);
                }
                k += 1;
            }
            if placed.ends_with_newline {
                k += 1;
            }
            if x0 >= x1 {
                continue;
            }
            let y = placed.pos.y + row.size.y - amp;
            let n = (((x1 - x0) / period * 2.0).ceil() as usize).max(2);
            let pts = (0..=n)
                .map(|i| {
                    let x = x0 + (x1 - x0) * i as f32 / n as f32;
                    let dy = if i % 2 == 0 { -amp } else { amp } * 0.5;
                    map(egui::pos2(placed.pos.x + x, y + dy))
                })
                .collect();
            painter.add(egui::Shape::line(pts, egui::Stroke::new((1.0 * scale).clamp(1.0, 2.0), squiggle_color(is.kind))));
        }
    }
}

/// The flagged text with up to ~40 characters around it, the problem underlined.
fn context_job(ui: &egui::Ui, text: &str, is: &Issue) -> egui::text::LayoutJob {
    let floor = |mut i: usize| { while !text.is_char_boundary(i) { i -= 1; } i };
    let ceil = |mut i: usize| { while !text.is_char_boundary(i) { i += 1; } i };
    let start = floor(is.range.start.saturating_sub(40));
    let end = ceil((is.range.end + 40).min(text.len()));
    let font = egui::TextStyle::Body.resolve(ui.style());
    let plain = egui::TextFormat { font_id: font.clone(), color: theme::text_dim(), ..Default::default() };
    let marked = egui::TextFormat {
        font_id: font,
        color: theme::text(),
        underline: egui::Stroke::new(1.5, squiggle_color(is.kind)),
        ..Default::default()
    };
    let flat = |s: &str| s.replace(['\n', '\t'], " ");
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = ui.available_width();
    job.append(&format!("{}{}", if start > 0 { "..." } else { "" }, flat(&text[start..is.range.start])), 0.0, plain.clone());
    job.append(&flat(&text[is.range.clone()]), 0.0, marked);
    job.append(&format!("{}{}", flat(&text[is.range.end..end]), if end < text.len() { "..." } else { "" }), 0.0, plain);
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(s: &str) -> Vec<(IssueKind, String)> {
        let place = Place { slide: 0, item: 0, cell: None };
        check(place, s, &HashSet::new(), true).into_iter().map(|i| (i.kind, i.found)).collect()
    }

    #[test]
    fn biology_words_pass() {
        let r = run("The intron is spliced out of the pre-mRNA by the spliceosome.");
        assert!(r.is_empty(), "{r:?}");
        assert!(run("Plasmids, ribosomes and the transcriptome of CRISPR-edited zebrafish").is_empty());
        assert!(run("BRCA1 and SpCas9 in β-catenin signaling, in vivo and in vitro").is_empty(), "{:?}", run("BRCA1 and SpCas9 in β-catenin signaling, in vivo and in vitro"));
    }

    #[test]
    fn finds_typos_and_grammar() {
        let r = run("The the mitochondira is a organelle , i think.");
        assert!(r.contains(&(IssueKind::Spelling, "mitochondira".into())), "{r:?}");
        assert!(r.contains(&(IssueKind::Grammar, "The the".into())), "{r:?}");
        assert!(r.contains(&(IssueKind::Grammar, "a".into())), "{r:?}");
        assert!(r.contains(&(IssueKind::Grammar, "i".into())), "{r:?}");
        assert!(r.iter().any(|(k, f)| *k == IssueKind::Grammar && f.ends_with(',')), "{r:?}");
        assert!(run("Cells divide. then they grow").contains(&(IssueKind::Grammar, "then".into())));
        assert!(run("e.g. cells, et al. showed").is_empty(), "{:?}", run("e.g. cells, et al. showed"));
        assert!(run("an RNA and a DNA, a unit and an hour, an mRNA").is_empty());
        let r = run("It is spliced. a mRNA is made");
        assert!(r.contains(&(IssueKind::Grammar, "a".into())) && r.iter().filter(|(k, _)| *k == IssueKind::Grammar).count() == 2, "{r:?}");
    }

    #[test]
    fn suggests() {
        let mut out = Vec::new();
        dictionary().suggest("mitochondira", &mut out);
        assert!(out.iter().any(|s| s == "mitochondria"), "{out:?}");
    }
}
