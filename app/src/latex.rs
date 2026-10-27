//! The Edit panel's LaTeX button: a text box's LaTeX math written out as plain text with real
//! superscripts and subscripts (`P_i \rightarrow ATP`, `H_2O`, `Ca^{2+}`, `\Delta G^\circ`), so it
//! stays an ordinary text box that edits, spell-checks and exports like any other. Greek letters,
//! operators, arrows and relations become their Unicode characters; `\frac{a}{b}` is written a/b
//! and `\sqrt{x}` √x, since there is no two-dimensional layout. A command it doesn't know is kept
//! as typed.

use crate::doc::{pack_runs, RunStyle, Script, TextRun};

/// `src` read as LaTeX math: the text, and the runs that style it.
pub fn to_text(src: &str) -> (String, Vec<TextRun>) {
    let mut r = Reader { s: src.chars().collect(), i: 0, out: Vec::new() };
    r.seq(RunStyle::default(), false, false);
    (r.out.iter().map(|c| c.0).collect(), pack_runs(&r.out.iter().map(|c| c.1).collect::<Vec<_>>()))
}

struct Reader {
    s: Vec<char>,
    i: usize,
    out: Vec<(char, RunStyle)>,
}

impl Reader {
    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    fn push(&mut self, t: &str, style: RunStyle) {
        self.out.extend(t.chars().map(|c| (c, style)));
    }

    /// Everything up to the `}` closing this group (when `in_group`) or the end. In `text`
    /// (`\text{...}`) characters are taken as they are.
    fn seq(&mut self, style: RunStyle, in_group: bool, text: bool) {
        while let Some(c) = self.peek() {
            if c == '}' && in_group {
                self.i += 1;
                return;
            }
            self.atom(style, text);
        }
    }

    /// One thing: a character, a `{group}`, a command, or a superscript or subscript.
    fn atom(&mut self, style: RunStyle, text: bool) {
        let Some(c) = self.peek() else { return };
        self.i += 1;
        match c {
            '{' => self.seq(style, true, text),
            '}' => {}
            '\\' => self.command(style, text),
            '$' if !text => {}
            '~' => self.push("\u{a0}", style),
            '^' | '_' if !text => {
                // `^\circ` is a degree sign; nested scripts keep the outer one.
                if c == '^' && self.s[self.i..].starts_with(&['\\', 'c', 'i', 'r', 'c']) && !self.s.get(self.i + 5).is_some_and(|c| c.is_ascii_alphabetic()) {
                    self.i += 5;
                    return self.push("°", style);
                }
                let script = if c == '^' { Script::Super } else { Script::Sub };
                self.arg(RunStyle { script: style.script.or(Some(script)), ..style }, text);
            }
            '-' if !text => self.push("−", style),
            '\'' if !text => self.push("′", style),
            c => self.out.push((c, style)),
        }
    }

    /// A command's argument: a `{group}`, or else the next single atom (spaces before it skipped).
    fn arg(&mut self, style: RunStyle, text: bool) {
        while self.peek() == Some(' ') {
            self.i += 1;
        }
        if self.peek().is_some() {
            self.atom(style, text);
        }
    }

    /// An argument's text and runs on their own, to be wrapped (`\frac`, `\sqrt`).
    fn arg_apart(&mut self, style: RunStyle, text: bool) -> Vec<(char, RunStyle)> {
        let before = std::mem::take(&mut self.out);
        self.arg(style, text);
        std::mem::replace(&mut self.out, before)
    }

    /// `part` written out, in parentheses when it is more than one term (`a+b`, not `d[A]`).
    fn push_grouped(&mut self, part: Vec<(char, RunStyle)>, style: RunStyle) {
        let simple = part.len() <= 1 || part.iter().all(|c| c.0.is_alphanumeric() || "()[].′".contains(c.0));
        if !simple {
            self.out.push(('(', style));
        }
        self.out.extend(part);
        if !simple {
            self.out.push((')', style));
        }
    }

    fn command(&mut self, style: RunStyle, text: bool) {
        let start = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
            self.i += 1;
        }
        if self.i == start {
            // A symbol: `\{`, `\%`, a space or a line break.
            let Some(c) = self.peek() else { return self.push("\\", style) };
            self.i += 1;
            return match c {
                ',' | ':' | ';' | ' ' => self.push(" ", style),
                '!' => {}
                '\\' => self.push("\n", style),
                '|' => self.push("‖", style),
                c => self.out.push((c, style)),
            };
        }
        let name: String = self.s[start..self.i].iter().collect();
        match name.as_str() {
            "text" | "textrm" | "mathrm" | "textnormal" | "operatorname" | "mbox" => self.arg(style, true),
            "mathbf" | "textbf" | "boldsymbol" => self.arg(RunStyle { bold: Some(true), ..style }, text),
            "mathit" | "textit" | "emph" => self.arg(RunStyle { italic: Some(true), ..style }, text),
            "mathsf" | "mathtt" | "texttt" | "mathcal" | "mathbb" | "displaystyle" | "textstyle" | "limits" | "nolimits" => {
                if !matches!(name.as_str(), "displaystyle" | "textstyle" | "limits" | "nolimits") {
                    self.arg(style, text);
                }
            }
            "frac" | "dfrac" | "tfrac" => {
                let (a, b) = (self.arg_apart(style, text), self.arg_apart(style, text));
                self.push_grouped(a, style);
                self.out.push(('/', style));
                self.push_grouped(b, style);
            }
            "sqrt" => {
                if self.peek() == Some('[') {
                    self.i += 1;
                    let end = self.s[self.i..].iter().position(|c| *c == ']').map_or(self.s.len(), |p| self.i + p);
                    let n: String = self.s[self.i..end].iter().collect();
                    self.i = (end + 1).min(self.s.len());
                    self.push(&n, RunStyle { script: style.script.or(Some(Script::Super)), ..style });
                }
                self.out.push(('√', style));
                let x = self.arg_apart(style, text);
                self.push_grouped(x, style);
            }
            "left" | "right" | "big" | "Big" | "bigg" | "Bigg" | "bigl" | "bigr" | "Bigl" | "Bigr" => {
                // The delimiter after it is written as it is; `\left.` is none.
                if self.peek() == Some('.') {
                    self.i += 1;
                }
            }
            "hat" | "bar" | "overline" | "vec" | "dot" | "ddot" | "tilde" => {
                let mark = match name.as_str() {
                    "hat" => '\u{302}',
                    "bar" | "overline" => '\u{305}',
                    "vec" => '\u{20d7}',
                    "dot" => '\u{307}',
                    "ddot" => '\u{308}',
                    _ => '\u{303}',
                };
                self.arg(style, text);
                self.out.push((mark, style));
            }
            _ => match symbol(&name) {
                Some(s) => {
                    self.push(s, style);
                    // A space between a letter and the next (`\Delta G`) only ended the command's name.
                    let letter = |c: Option<&char>| c.is_some_and(|c| c.is_alphabetic());
                    if s.chars().count() == 1 && s.chars().all(char::is_alphabetic) && self.peek() == Some(' ') && letter(self.s.get(self.i + 1)) {
                        self.i += 1;
                    }
                }
                None => self.push(&format!("\\{name}"), style),
            },
        }
    }
}

/// The character (or word, for functions like `\sin`) a LaTeX command stands for.
fn symbol(name: &str) -> Option<&'static str> {
    Some(match name {
        "alpha" => "α", "beta" => "β", "gamma" => "γ", "delta" => "δ", "epsilon" => "ϵ", "varepsilon" => "ε",
        "zeta" => "ζ", "eta" => "η", "theta" => "θ", "vartheta" => "ϑ", "iota" => "ι", "kappa" => "κ",
        "lambda" => "λ", "mu" => "μ", "nu" => "ν", "xi" => "ξ", "omicron" => "ο", "pi" => "π", "varpi" => "ϖ",
        "rho" => "ρ", "varrho" => "ϱ", "sigma" => "σ", "varsigma" => "ς", "tau" => "τ", "upsilon" => "υ",
        "phi" => "ϕ", "varphi" => "φ", "chi" => "χ", "psi" => "ψ", "omega" => "ω",
        "Gamma" => "Γ", "Delta" => "Δ", "Theta" => "Θ", "Lambda" => "Λ", "Xi" => "Ξ", "Pi" => "Π",
        "Sigma" => "Σ", "Upsilon" => "Υ", "Phi" => "Φ", "Psi" => "Ψ", "Omega" => "Ω",
        "pm" => "±", "mp" => "∓", "times" => "×", "div" => "÷", "cdot" => "·", "ast" => "∗", "star" => "⋆",
        "circ" => "∘", "bullet" => "•", "degree" => "°", "prime" => "′",
        "le" | "leq" => "≤", "ge" | "geq" => "≥", "ne" | "neq" => "≠", "approx" => "≈", "equiv" => "≡",
        "sim" => "∼", "simeq" => "≃", "cong" => "≅", "propto" => "∝", "ll" => "≪", "gg" => "≫",
        "infty" => "∞", "partial" => "∂", "nabla" => "∇", "sum" => "∑", "prod" => "∏", "int" => "∫",
        "iint" => "∬", "oint" => "∮", "in" => "∈", "notin" => "∉", "ni" => "∋", "subset" => "⊂",
        "subseteq" => "⊆", "supset" => "⊃", "supseteq" => "⊇", "cup" => "∪", "cap" => "∩",
        "emptyset" | "varnothing" => "∅", "forall" => "∀", "exists" => "∃", "neg" | "lnot" => "¬",
        "land" | "wedge" => "∧", "lor" | "vee" => "∨", "oplus" => "⊕", "otimes" => "⊗",
        "to" | "rightarrow" => "→", "gets" | "leftarrow" => "←", "leftrightarrow" => "↔",
        "Rightarrow" | "implies" => "⇒", "Leftarrow" => "⇐", "Leftrightarrow" | "iff" => "⇔",
        "longrightarrow" => "⟶", "longleftarrow" => "⟵", "longleftrightarrow" => "⟷",
        "rightleftharpoons" => "⇌", "leftrightharpoons" => "⇋", "rightleftarrows" => "⇄",
        "uparrow" => "↑", "downarrow" => "↓", "updownarrow" => "↕", "mapsto" => "↦",
        "ldots" | "dots" | "dotsc" => "…", "cdots" | "dotsb" => "⋯", "vdots" => "⋮", "ddots" => "⋱",
        "hbar" => "ℏ", "ell" => "ℓ", "Re" => "ℜ", "Im" => "ℑ", "aleph" => "ℵ", "angle" => "∠",
        "perp" => "⊥", "parallel" => "∥", "mid" => "∣", "langle" => "⟨", "rangle" => "⟩",
        "lfloor" => "⌊", "rfloor" => "⌋", "lceil" => "⌈", "rceil" => "⌉", "lbrace" => "{", "rbrace" => "}",
        "AA" | "angstrom" => "Å", "checkmark" => "✓", "dagger" => "†", "ddagger" => "‡",
        "quad" => "\u{2003}", "qquad" => "\u{2003}\u{2003}",
        "sin" => "sin", "cos" => "cos", "tan" => "tan", "cot" => "cot", "sec" => "sec", "csc" => "csc",
        "arcsin" => "arcsin", "arccos" => "arccos", "arctan" => "arctan", "sinh" => "sinh", "cosh" => "cosh",
        "tanh" => "tanh", "log" => "log", "ln" => "ln", "lg" => "lg", "exp" => "exp", "lim" => "lim",
        "max" => "max", "min" => "min", "sup" => "sup", "inf" => "inf", "det" => "det", "deg" => "deg",
        "arg" => "arg", "dim" => "dim", "ker" => "ker", "gcd" => "gcd", "Pr" => "Pr",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::expand_runs;

    /// The text, with superscripts written ^(..) and subscripts _(..) to compare easily.
    fn show(src: &str) -> String {
        let (t, runs) = to_text(src);
        let styles = expand_runs(&runs, t.chars().count());
        let mut out = String::new();
        let mut open: Option<Script> = None;
        for (c, s) in t.chars().zip(styles) {
            if s.script != open {
                if open.is_some() {
                    out.push(')');
                }
                match s.script {
                    Some(Script::Super) => out.push_str("^("),
                    Some(Script::Sub) => out.push_str("_("),
                    None => {}
                }
                open = s.script;
            }
            out.push(c);
        }
        if open.is_some() {
            out.push(')');
        }
        out
    }

    #[test]
    fn scripts_symbols_and_text() {
        assert_eq!(show("ADP + P_i \\rightarrow ATP"), "ADP + P_(i) → ATP");
        assert_eq!(show("$H_2O$"), "H_(2)O");
        assert_eq!(show("Ca^{2+}"), "Ca^(2+)");
        assert_eq!(show("Cl^-"), "Cl^(−)");
        assert_eq!(show("\\Delta G^\\circ = -RT\\ln K"), "ΔG° = −RTln K");
        assert_eq!(show("e^{x_1}"), "e^(x1)");
        assert_eq!(show("\\text{scRNA-seq}"), "scRNA-seq");
        assert_eq!(show("\\frac{a+b}{2}"), "(a+b)/2");
        assert_eq!(show("\\frac{d[A]}{dt}"), "d[A]/dt");
        assert_eq!(show("\\sqrt[3]{x}"), "^(3)√x");
        assert_eq!(show("\\alpha\\beta + \\unknown"), "αβ + \\unknown");
        assert_eq!(show("50\\%"), "50%");
    }

    #[test]
    fn bold_and_italic_runs() {
        let (t, runs) = to_text("\\mathbf{v} = \\mathit{x}");
        assert_eq!(t, "v = x");
        let styles = expand_runs(&runs, 5);
        assert_eq!(styles[0].bold, Some(true));
        assert_eq!(styles[4].italic, Some(true));
        assert_eq!(styles[2], RunStyle::default());
    }
}
