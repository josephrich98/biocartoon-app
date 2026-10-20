//! Code blocks (in the spirit of carbon.now.sh): a small, dependency-free syntax highlighter with
//! language auto-detection, colour themes, and the block geometry shared by the canvas and export.

use serde::{Deserialize, Serialize};

/// Spaces a tab expands to, everywhere code is shown.
const TAB: &str = "    ";

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum CodeLang {
    #[default]
    Auto,
    Plain,
    Python,
    R,
    Julia,
    Matlab,
    Rust,
    C,
    Cpp,
    Java,
    JavaScript,
    TypeScript,
    Go,
    Bash,
    Sql,
    Json,
    Yaml,
}

impl CodeLang {
    pub const ALL: [CodeLang; 17] = [
        CodeLang::Auto,
        CodeLang::Plain,
        CodeLang::Python,
        CodeLang::R,
        CodeLang::Julia,
        CodeLang::Matlab,
        CodeLang::Rust,
        CodeLang::C,
        CodeLang::Cpp,
        CodeLang::Java,
        CodeLang::JavaScript,
        CodeLang::TypeScript,
        CodeLang::Go,
        CodeLang::Bash,
        CodeLang::Sql,
        CodeLang::Json,
        CodeLang::Yaml,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CodeLang::Auto => "Auto-detect",
            CodeLang::Plain => "Plain text",
            CodeLang::Python => "Python",
            CodeLang::R => "R",
            CodeLang::Julia => "Julia",
            CodeLang::Matlab => "MATLAB",
            CodeLang::Rust => "Rust",
            CodeLang::C => "C",
            CodeLang::Cpp => "C++",
            CodeLang::Java => "Java",
            CodeLang::JavaScript => "JavaScript",
            CodeLang::TypeScript => "TypeScript",
            CodeLang::Go => "Go",
            CodeLang::Bash => "Bash",
            CodeLang::Sql => "SQL",
            CodeLang::Json => "JSON",
            CodeLang::Yaml => "YAML",
        }
    }

    /// The language to highlight `code` as: this one, or the detected one for [`CodeLang::Auto`].
    pub fn resolve(self, code: &str) -> CodeLang {
        if self == CodeLang::Auto { detect(code) } else { self }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum CodeTheme {
    #[default]
    Dracula,
    OneDark,
    Monokai,
    Nord,
    GitHubLight,
    SolarizedLight,
}

/// A theme's colours, as rgb.
pub struct Palette {
    pub bg: [u8; 3],
    pub fg: [u8; 3],
    pub comment: [u8; 3],
    pub keyword: [u8; 3],
    pub string: [u8; 3],
    pub number: [u8; 3],
    pub function: [u8; 3],
    pub ty: [u8; 3],
    pub operator: [u8; 3],
    pub property: [u8; 3],
    pub line_number: [u8; 3],
}

const fn rgb(c: u32) -> [u8; 3] {
    [(c >> 16) as u8, (c >> 8) as u8, c as u8]
}

impl CodeTheme {
    pub const ALL: [CodeTheme; 6] = [CodeTheme::Dracula, CodeTheme::OneDark, CodeTheme::Monokai, CodeTheme::Nord, CodeTheme::GitHubLight, CodeTheme::SolarizedLight];

    pub fn label(self) -> &'static str {
        match self {
            CodeTheme::Dracula => "Dracula",
            CodeTheme::OneDark => "One Dark",
            CodeTheme::Monokai => "Monokai",
            CodeTheme::Nord => "Nord",
            CodeTheme::GitHubLight => "GitHub Light",
            CodeTheme::SolarizedLight => "Solarized Light",
        }
    }

    pub fn palette(self) -> Palette {
        let p = |c: [u32; 11]| Palette {
            bg: rgb(c[0]),
            fg: rgb(c[1]),
            comment: rgb(c[2]),
            keyword: rgb(c[3]),
            string: rgb(c[4]),
            number: rgb(c[5]),
            function: rgb(c[6]),
            ty: rgb(c[7]),
            operator: rgb(c[8]),
            property: rgb(c[9]),
            line_number: rgb(c[10]),
        };
        match self {
            CodeTheme::Dracula => p([0x282a36, 0xf8f8f2, 0x6272a4, 0xff79c6, 0xf1fa8c, 0xbd93f9, 0x50fa7b, 0x8be9fd, 0xff79c6, 0x8be9fd, 0x6272a4]),
            CodeTheme::OneDark => p([0x282c34, 0xabb2bf, 0x5c6370, 0xc678dd, 0x98c379, 0xd19a66, 0x61afef, 0xe5c07b, 0x56b6c2, 0xe06c75, 0x4b5263]),
            CodeTheme::Monokai => p([0x272822, 0xf8f8f2, 0x75715e, 0xf92672, 0xe6db74, 0xae81ff, 0xa6e22e, 0x66d9ef, 0xf92672, 0xfd971f, 0x75715e]),
            CodeTheme::Nord => p([0x2e3440, 0xd8dee9, 0x616e88, 0x81a1c1, 0xa3be8c, 0xb48ead, 0x88c0d0, 0x8fbcbb, 0x81a1c1, 0x8fbcbb, 0x4c566a]),
            CodeTheme::GitHubLight => p([0xffffff, 0x24292f, 0x6e7781, 0xcf222e, 0x0a3069, 0x0550ae, 0x8250df, 0x953800, 0xcf222e, 0x0550ae, 0x8c959f]),
            CodeTheme::SolarizedLight => p([0xfdf6e3, 0x657b83, 0x93a1a1, 0x859900, 0x2aa198, 0xd33682, 0x268bd2, 0xb58900, 0x859900, 0x268bd2, 0x93a1a1]),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tok {
    Plain,
    Comment,
    Keyword,
    String,
    Number,
    Function,
    Type,
    Operator,
    Property,
}

impl Palette {
    pub fn color(&self, t: Tok) -> [u8; 3] {
        match t {
            Tok::Plain => self.fg,
            Tok::Comment => self.comment,
            Tok::Keyword => self.keyword,
            Tok::String => self.string,
            Tok::Number => self.number,
            Tok::Function => self.function,
            Tok::Type => self.ty,
            Tok::Operator => self.operator,
            Tok::Property => self.property,
        }
    }
}

// ---------- languages ----------

struct Spec {
    line_comments: &'static [&'static str],
    block_comment: Option<(&'static str, &'static str)>,
    quotes: &'static str,
    /// `"""` / `'''` strings.
    triple: bool,
    /// `#include`-style lines.
    preproc: bool,
    /// Identifiers may start with `$` (Bash, JavaScript variables).
    dollar: bool,
    /// Identifiers may contain `.` (R).
    dotted: bool,
    /// Keywords match regardless of case (SQL).
    nocase: bool,
    /// Capitalised identifiers are types.
    caps_types: bool,
    /// A name or string followed by `:` is a key (JSON, YAML).
    keys: bool,
    keywords: &'static [&'static str],
    types: &'static [&'static str],
}

const BASE: Spec = Spec {
    line_comments: &[],
    block_comment: None,
    quotes: "\"'",
    triple: false,
    preproc: false,
    dollar: false,
    dotted: false,
    nocase: false,
    caps_types: false,
    keys: false,
    keywords: &[],
    types: &[],
};

const C_TYPES: &[&str] = &["int", "char", "float", "double", "long", "short", "unsigned", "signed", "void", "bool", "size_t", "uint8_t", "uint16_t", "uint32_t", "uint64_t", "int8_t", "int16_t", "int32_t", "int64_t", "FILE"];
const C_KW: &[&str] = &["auto", "break", "case", "const", "continue", "default", "do", "else", "enum", "extern", "for", "goto", "if", "inline", "register", "return", "sizeof", "static", "struct", "switch", "typedef", "union", "volatile", "while", "NULL", "true", "false"];
const CPP_KW: &[&str] = &[
    "auto", "break", "case", "catch", "class", "const", "constexpr", "continue", "default", "delete", "do", "else", "enum", "explicit", "extern", "for", "friend", "goto", "if", "inline", "mutable", "namespace", "new", "noexcept", "nullptr", "operator", "override", "private", "protected", "public", "return", "sizeof", "static", "static_cast", "struct", "switch", "template", "this", "throw", "try", "typedef", "typename", "union", "using", "virtual", "volatile", "while", "true", "false",
];
const JS_KW: &[&str] = &[
    "async", "await", "break", "case", "catch", "class", "const", "continue", "default", "delete", "do", "else", "export", "extends", "finally", "for", "from", "function", "if", "import", "in", "instanceof", "let", "new", "of", "return", "static", "super", "switch", "this", "throw", "try", "typeof", "var", "void", "while", "yield", "true", "false", "null", "undefined",
];
const TS_KW: &[&str] = &[
    "abstract", "as", "async", "await", "break", "case", "catch", "class", "const", "continue", "declare", "default", "delete", "do", "else", "enum", "export", "extends", "finally", "for", "from", "function", "if", "implements", "import", "in", "instanceof", "interface", "keyof", "let", "namespace", "new", "of", "private", "protected", "public", "readonly", "return", "static", "super", "switch", "this", "throw", "try", "type", "typeof", "var", "void", "while", "yield", "true", "false", "null", "undefined",
];

fn spec(lang: CodeLang) -> Spec {
    match lang {
        CodeLang::Python => Spec {
            line_comments: &["#"],
            triple: true,
            caps_types: true,
            keywords: &[
                "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while", "with", "yield", "True", "False", "None", "self",
            ],
            types: &["int", "float", "str", "bool", "list", "dict", "set", "tuple", "bytes", "object", "len", "range", "print", "open", "super", "type", "isinstance", "enumerate", "zip", "map", "filter", "sorted"],
            ..BASE
        },
        CodeLang::R => Spec {
            line_comments: &["#"],
            dotted: true,
            keywords: &["if", "else", "for", "while", "repeat", "function", "return", "next", "break", "in", "TRUE", "FALSE", "NULL", "NA", "NaN", "Inf", "T", "F", "library", "require"],
            types: &["c", "list", "data.frame", "matrix", "vector", "factor", "numeric", "character", "logical", "integer"],
            ..BASE
        },
        CodeLang::Julia => Spec {
            line_comments: &["#"],
            block_comment: Some(("#=", "=#")),
            triple: true,
            caps_types: true,
            keywords: &[
                "abstract", "baremodule", "begin", "break", "catch", "const", "continue", "do", "else", "elseif", "end", "export", "finally", "for", "function", "global", "if", "import", "in", "let", "local", "macro", "module", "mutable", "quote", "return", "struct", "try", "using", "where", "while", "true", "false", "nothing",
            ],
            ..BASE
        },
        CodeLang::Matlab => Spec {
            line_comments: &["%"],
            block_comment: Some(("%{", "%}")),
            keywords: &["break", "case", "catch", "classdef", "continue", "else", "elseif", "end", "for", "function", "global", "if", "otherwise", "parfor", "persistent", "return", "switch", "try", "while", "true", "false"],
            ..BASE
        },
        CodeLang::Rust => Spec {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            quotes: "\"",
            caps_types: true,
            keywords: &[
                "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where", "while",
            ],
            types: &["i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize", "f32", "f64", "bool", "char", "str"],
            ..BASE
        },
        CodeLang::C => Spec { line_comments: &["//"], block_comment: Some(("/*", "*/")), preproc: true, keywords: C_KW, types: C_TYPES, ..BASE },
        CodeLang::Cpp => Spec { line_comments: &["//"], block_comment: Some(("/*", "*/")), preproc: true, caps_types: true, keywords: CPP_KW, types: C_TYPES, ..BASE },
        CodeLang::Java => Spec {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            caps_types: true,
            keywords: &[
                "abstract", "assert", "break", "case", "catch", "class", "const", "continue", "default", "do", "else", "enum", "extends", "final", "finally", "for", "if", "implements", "import", "instanceof", "interface", "native", "new", "package", "private", "protected", "public", "return", "static", "super", "switch", "synchronized", "this", "throw", "throws", "try", "var", "void", "volatile", "while", "true", "false", "null",
            ],
            types: &["int", "long", "short", "byte", "char", "float", "double", "boolean"],
            ..BASE
        },
        CodeLang::JavaScript => Spec { line_comments: &["//"], block_comment: Some(("/*", "*/")), quotes: "\"'`", dollar: true, caps_types: true, keywords: JS_KW, ..BASE },
        CodeLang::TypeScript => Spec {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            quotes: "\"'`",
            dollar: true,
            caps_types: true,
            keywords: TS_KW,
            types: &["string", "number", "boolean", "any", "unknown", "never", "object", "void"],
            ..BASE
        },
        CodeLang::Go => Spec {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            quotes: "\"'`",
            caps_types: true,
            keywords: &["break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough", "for", "func", "go", "goto", "if", "import", "interface", "map", "package", "range", "return", "select", "struct", "switch", "type", "var", "true", "false", "nil"],
            types: &["int", "int8", "int16", "int32", "int64", "uint", "uint8", "uint16", "uint32", "uint64", "float32", "float64", "string", "bool", "byte", "rune", "error", "make", "len", "append", "new"],
            ..BASE
        },
        CodeLang::Bash => Spec {
            line_comments: &["#"],
            dollar: true,
            keywords: &["if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac", "in", "function", "return", "exit", "export", "local", "readonly", "source", "set", "unset", "shift"],
            types: &["echo", "printf", "cd", "ls", "grep", "sed", "awk", "cat", "mkdir", "rm", "cp", "mv", "test", "read", "eval", "exec"],
            ..BASE
        },
        CodeLang::Sql => Spec {
            line_comments: &["--"],
            block_comment: Some(("/*", "*/")),
            nocase: true,
            keywords: &[
                "select", "from", "where", "and", "or", "not", "insert", "into", "values", "update", "set", "delete", "create", "table", "drop", "alter", "add", "index", "view", "join", "inner", "left", "right", "outer", "full", "on", "as", "group", "by", "order", "having", "limit", "offset", "distinct", "union", "all", "case", "when", "then", "else", "end", "is", "null", "in", "like", "between", "exists", "primary", "key", "foreign", "references", "asc", "desc", "with",
            ],
            types: &["int", "integer", "bigint", "text", "varchar", "char", "real", "float", "double", "boolean", "date", "timestamp", "count", "sum", "avg", "min", "max", "coalesce"],
            ..BASE
        },
        CodeLang::Json => Spec { quotes: "\"", keys: true, keywords: &["true", "false", "null"], ..BASE },
        CodeLang::Yaml => Spec { line_comments: &["#"], keys: true, keywords: &["true", "false", "null", "yes", "no", "on", "off", "~"], ..BASE },
        CodeLang::Plain | CodeLang::Auto => Spec { quotes: "", ..BASE },
    }
}

/// Guess the language of `code` from telltale snippets; [`CodeLang::Plain`] when nothing stands out.
pub fn detect(code: &str) -> CodeLang {
    let t = code.trim_start();
    if t.is_empty() {
        return CodeLang::Plain;
    }
    if t.starts_with("#!") {
        let first = t.lines().next().unwrap_or("");
        for (k, l) in [("python", CodeLang::Python), ("Rscript", CodeLang::R), ("julia", CodeLang::Julia), ("node", CodeLang::JavaScript), ("sh", CodeLang::Bash)] {
            if first.contains(k) {
                return l;
            }
        }
    }
    if (t.starts_with('{') || t.starts_with('[')) && serde_json::from_str::<serde_json::Value>(t).is_ok() {
        return CodeLang::Json;
    }
    let lower = code.to_lowercase();
    let count = |hay: &str, pats: &[&str]| pats.iter().map(|p| hay.matches(p).count().min(4)).sum::<usize>() as i32;
    let mut scores: Vec<(CodeLang, i32)> = vec![
        (CodeLang::Python, count(code, &["def ", "import ", "self.", "elif ", "print(", "None", "True", "False", "__init__", "from ", "lambda ", "\"\"\""]) + 2 * count(code, &["):\n", "]:\n"])),
        (CodeLang::R, 3 * count(code, &["<-", "%>%", "library(", "data.frame", "ggplot(", "TRUE", "FALSE", "NULL"]) + count(code, &["function(", "c(", "|>", "$"])),
        (CodeLang::Julia, count(code, &["function ", "end\n", "using ", "println(", "::", "module ", ".=", "nothing"]) + 2 * count(code, &["end\n", "using "]).min(1)),
        (CodeLang::Matlab, count(code, &["end\n", "disp(", "zeros(", "ones(", "fprintf(", ".*", "./", "% ", "end;"]) + 2 * count(code, &["clc", "clear all", "figure;"])),
        (CodeLang::Rust, 2 * count(code, &["fn ", "let mut ", "impl ", "pub fn", "&self", "println!", "use std", "-> ", "::new(", "Vec<", "Option<", "Some(", "Ok("]) + count(code, &["::", "match "])),
        (CodeLang::C, 2 * count(code, &["#include <stdio.h>", "#include <stdlib.h>", "printf(", "malloc(", "int main(", "struct "]) + count(code, &["#include", "->", ";\n"])),
        (CodeLang::Cpp, 3 * count(code, &["std::", "cout", "#include <iostream>", "template<", "template <", "namespace ", "nullptr"]) + count(code, &["#include", "class ", "::"])),
        (CodeLang::Java, 3 * count(code, &["public class", "System.out", "public static void", "import java", "@Override", "private final"]) + count(code, &["public ", "private ", "new ", ";\n"])),
        (CodeLang::JavaScript, 2 * count(code, &["const ", "console.log", "=> ", "===", "require(", "document.", "function(", "module.exports"]) + count(code, &["let ", "function ", "export ", ";\n"])),
        (CodeLang::TypeScript, 2 * count(code, &["const ", "console.log", "=> ", "==="]) + 3 * count(code, &[": string", ": number", ": boolean", "interface ", "export type ", "<T>", "as const"])),
        (CodeLang::Go, 3 * count(code, &["func ", "package ", ":= ", "fmt.", "import (", "go func", "chan "]) + count(code, &["err != nil", "return "])),
        (CodeLang::Bash, 2 * count(code, &["echo ", "fi\n", "done\n", "then\n", "$(", "${", "export ", "sudo ", "apt ", "pip install", "cd ", "conda "]) + count(code, &["$", " | ", "&&", " -"])),
        (CodeLang::Sql, 3 * count(&lower, &["select ", " from ", " where ", "insert into", "create table", "group by", "order by", "join "])),
        (CodeLang::Yaml, 2 * code.lines().filter(|l| { let l = l.trim(); l.starts_with("- ") || (l.contains(": ") && !l.contains('(') && !l.contains(';') && !l.contains('=')) || l.ends_with(':') && !l.contains(' ') }).count() as i32 - 3 * count(code, &["{", "(", ";"])),
    ];
    // Ties go to the earlier, more common languages.
    scores.sort_by_key(|(_, s)| std::cmp::Reverse(*s));
    let (lang, best) = scores[0];
    if best >= 2 { lang } else { CodeLang::Plain }
}

/// One coloured run of text on a line.
pub type Span = (String, Tok);

/// `code` as a block shows it: tabs expanded, no trailing blank lines.
fn display(code: &str) -> String {
    code.replace("\r\n", "\n").replace('\t', TAB).trim_end_matches(['\n', ' ']).to_string()
}

/// Split `code` into lines of coloured runs, as a block shows it (see [`display`]).
pub fn highlight(code: &str, lang: CodeLang) -> Vec<Vec<Span>> {
    highlight_exact(&display(code), lang)
}

/// [`highlight`] without touching the text, so the runs join back to exactly `code` (for editors).
pub fn highlight_exact(code: &str, lang: CodeLang) -> Vec<Vec<Span>> {
    let sp = spec(lang);
    let cs: Vec<char> = code.chars().collect();
    let n = cs.len();
    let mut lines: Vec<Vec<Span>> = vec![Vec::new()];
    let mut push = |s: &[char], t: Tok| {
        for (k, part) in s.split(|c| *c == '\n').enumerate() {
            if k > 0 {
                lines.push(Vec::new());
            }
            if part.is_empty() {
                continue;
            }
            let line = lines.last_mut().unwrap();
            match line.last_mut() {
                Some((text, lt)) if *lt == t => text.extend(part),
                _ => line.push((part.iter().collect(), t)),
            }
        }
    };
    let at = |i: usize, pat: &str| pat.chars().enumerate().all(|(k, c)| cs.get(i + k) == Some(&c));
    // The next non-space character on the line after `i`.
    let next_char = |mut i: usize| {
        while i < n && (cs[i] == ' ') {
            i += 1;
        }
        cs.get(i).copied()
    };
    let is_key = |end: usize| sp.keys && next_char(end) == Some(':') && cs.get(end + 1) != Some(&':');
    let line_start = |i: usize| cs[..i].iter().rev().take_while(|c| **c != '\n').all(|c| *c == ' ');
    let mut i = 0;
    if lang == CodeLang::Plain {
        push(&cs, Tok::Plain);
        return lines;
    }
    while i < n {
        let c = cs[i];
        let start = i;
        if let Some((open, close)) = sp.block_comment {
            if at(i, open) && (lang != CodeLang::Matlab || line_start(i)) {
                i += open.chars().count();
                while i < n && !at(i, close) {
                    i += 1;
                }
                i = (i + close.chars().count()).min(n);
                push(&cs[start..i], Tok::Comment);
                continue;
            }
        }
        let comment = sp.line_comments.iter().any(|m| at(i, m)) && !(lang == CodeLang::Bash && i > 0 && (cs[i - 1] == '$' || cs[i - 1].is_alphanumeric()));
        if comment || (sp.preproc && c == '#' && line_start(i)) {
            while i < n && cs[i] != '\n' {
                i += 1;
            }
            push(&cs[start..i], if comment { Tok::Comment } else { Tok::Keyword });
            continue;
        }
        if sp.triple && (at(i, "\"\"\"") || at(i, "'''")) {
            let q: String = cs[i..i + 3].iter().collect();
            i += 3;
            while i < n && !at(i, &q) {
                i += if cs[i] == '\\' { 2 } else { 1 };
            }
            i = (i + 3).min(n);
            push(&cs[start..i], Tok::String);
            continue;
        }
        if sp.quotes.contains(c) {
            // Rust lifetimes ('a) are not character literals.
            if lang == CodeLang::Rust || (c == '\'' && lang == CodeLang::Matlab && i > 0 && (cs[i - 1].is_alphanumeric() || cs[i - 1] == ')' || cs[i - 1] == ']')) {
                // fallthrough to the operator branch below
            } else {
                i += 1;
                while i < n && cs[i] != c && (cs[i] != '\n' || c == '`') {
                    i += if cs[i] == '\\' { 2 } else { 1 };
                }
                i = (i + 1).min(n);
                push(&cs[start..i], if is_key(i) { Tok::Property } else { Tok::String });
                continue;
            }
        }
        if lang == CodeLang::Rust && c == '\'' {
            let char_lit = cs.get(i + 1) == Some(&'\\') || cs.get(i + 2) == Some(&'\'');
            if char_lit {
                i += 1;
                while i < n && cs[i] != '\'' && cs[i] != '\n' {
                    i += if cs[i] == '\\' { 2 } else { 1 };
                }
                i = (i + 1).min(n);
                push(&cs[start..i], Tok::String);
            } else {
                i += 1;
                while i < n && (cs[i].is_alphanumeric() || cs[i] == '_') {
                    i += 1;
                }
                push(&cs[start..i], Tok::Type);
            }
            continue;
        }
        if lang == CodeLang::Rust && c == '"' {
            i += 1;
            while i < n && cs[i] != '"' {
                i += if cs[i] == '\\' { 2 } else { 1 };
            }
            i = (i + 1).min(n);
            push(&cs[start..i], Tok::String);
            continue;
        }
        let prev_word = i > 0 && (cs[i - 1].is_alphanumeric() || cs[i - 1] == '_');
        if !prev_word && (c.is_ascii_digit() || (c == '.' && cs.get(i + 1).is_some_and(|d| d.is_ascii_digit()))) {
            while i < n && (cs[i].is_ascii_alphanumeric() || cs[i] == '.' || cs[i] == '_' || ((cs[i] == '-' || cs[i] == '+') && matches!(cs[i - 1], 'e' | 'E'))) {
                if cs[i] == '.' && cs.get(i + 1) == Some(&'.') {
                    break;
                }
                i += 1;
            }
            push(&cs[start..i], Tok::Number);
            continue;
        }
        let ident_start = c.is_alphabetic() || c == '_' || (sp.dollar && c == '$') || (c == '@' && lang != CodeLang::R && lang != CodeLang::Json) || (lang == CodeLang::Julia && c == '@');
        if ident_start {
            i += 1;
            if c == '$' && i < n && cs[i] == '{' {
                while i < n && cs[i] != '}' && cs[i] != '\n' {
                    i += 1;
                }
                i = (i + 1).min(n);
                push(&cs[start..i], Tok::Type);
                continue;
            }
            while i < n && (cs[i].is_alphanumeric() || cs[i] == '_' || (sp.dotted && cs[i] == '.') || (lang == CodeLang::Yaml && cs[i] == '-')) {
                i += 1;
            }
            let word: String = cs[start..i].iter().collect();
            let lookup = if sp.nocase { word.to_lowercase() } else { word.clone() };
            let t = if c == '$' {
                Tok::Type
            } else if c == '@' {
                Tok::Function
            } else if is_key(i) && lang == CodeLang::Yaml {
                Tok::Property
            } else if sp.keywords.contains(&lookup.as_str()) {
                Tok::Keyword
            } else if sp.types.contains(&lookup.as_str()) {
                Tok::Type
            } else if lang == CodeLang::Rust && cs.get(i) == Some(&'!') {
                i += 1;
                Tok::Function
            } else if next_char(i) == Some('(') {
                Tok::Function
            } else if sp.caps_types && c.is_uppercase() {
                Tok::Type
            } else {
                Tok::Plain
            };
            push(&cs[start..i], t);
            continue;
        }
        if c == '\n' || c == ' ' {
            i += 1;
            while i < n && cs[i] == ' ' {
                i += 1;
            }
            push(&cs[start..i], Tok::Plain);
            continue;
        }
        i += 1;
        let t = if "+-*/%=<>!&|^~?:".contains(c) { Tok::Operator } else { Tok::Plain };
        push(&cs[start..i], t);
    }
    lines
}

// ---------- geometry ----------

/// Where things go in a code block, in document units (font size `fs`), for a monospace
/// character `char_w` wide and rows `line_h` tall.
pub struct BlockLayout {
    /// Height of the title bar with the window dots (0 without it).
    pub header: f32,
    pub pad_x: f32,
    pub pad_y: f32,
    /// Width of the line-number column plus its gap (0 without numbers).
    pub gutter: f32,
    pub line_h: f32,
    pub radius: f32,
}

impl BlockLayout {
    pub fn new(fs: f32, lines: usize, line_numbers: bool, window: bool, char_w: f32, line_h: f32) -> Self {
        let digits = lines.max(1).to_string().len() as f32;
        Self {
            header: if window { fs * 2.4 } else { 0.0 },
            pad_x: fs * 1.6,
            pad_y: if window { fs * 0.4 } else { fs * 1.2 } + fs * 0.4,
            gutter: if line_numbers { digits * char_w + fs * 1.4 } else { 0.0 },
            line_h,
            radius: fs * 0.6,
        }
    }

    /// The window dots: centre offsets from the block's top-left corner, radius, and rgb.
    pub fn dots(&self, fs: f32) -> [([f32; 2], f32, [u8; 3]); 3] {
        let r = fs * 0.42;
        let y = self.header * 0.6;
        let x0 = self.pad_x * 0.8 + r;
        let step = 2.0 * r + fs * 0.55;
        [([x0, y], r, rgb(0xff5f56)), ([x0 + step, y], r, rgb(0xffbd2e)), ([x0 + 2.0 * step, y], r, rgb(0x27c93f))]
    }

    /// Top-left of the first character of code, from the block's corner.
    pub fn text_origin(&self) -> [f32; 2] {
        [self.pad_x + self.gutter, self.header + self.pad_y]
    }

    /// The size that shows every line of `code` without clipping.
    pub fn fit(&self, code: &str, char_w: f32) -> [f32; 2] {
        let code = display(code);
        let lines = code.split('\n').count().max(1) as f32;
        let cols = code.split('\n').map(|l| l.chars().count()).max().unwrap_or(0).max(8) as f32;
        let o = self.text_origin();
        [o[0] + cols * char_w + self.pad_x, o[1] + lines * self.line_h + self.pad_y]
    }
}

/// The size a block needs to show all of `code` at `font_size`, measured with egui's monospace font.
pub fn fit_size(ctx: &egui::Context, code: &str, font_size: f32, line_numbers: bool, window: bool) -> [f32; 2] {
    let font = egui::FontId::monospace(font_size.max(1.0));
    let (char_w, line_h) = ctx.fonts_mut(|f| (f.glyph_width(&font, '0'), f.row_height(&font)));
    BlockLayout::new(font_size, line_count(code), line_numbers, window, char_w, line_h).fit(code, char_w)
}

/// Number of lines `code` shows.
pub fn line_count(code: &str) -> usize {
    display(code).split('\n').count().max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects() {
        assert_eq!(detect("import numpy as np\n\ndef f(x):\n    return np.sum(x)\n"), CodeLang::Python);
        assert_eq!(detect("library(ggplot2)\ndf <- data.frame(x = 1:10)\n"), CodeLang::R);
        assert_eq!(detect("fn main() {\n    let mut v = Vec::new();\n    println!(\"{}\", v.len());\n}\n"), CodeLang::Rust);
        assert_eq!(detect("SELECT name, count(*) FROM genes WHERE chr = 'X' GROUP BY name;"), CodeLang::Sql);
        assert_eq!(detect("{\"a\": [1, 2, 3]}"), CodeLang::Json);
        assert_eq!(detect("#!/bin/bash\necho hi\n"), CodeLang::Bash);
        assert_eq!(detect("const x = (a) => a + 1;\nconsole.log(x(2));\n"), CodeLang::JavaScript);
        assert_eq!(detect("package main\n\nimport \"fmt\"\n\nfunc main() {\n\tfmt.Println(\"hi\")\n}\n"), CodeLang::Go);
        assert_eq!(detect("hello there"), CodeLang::Plain);
    }

    #[test]
    fn highlights() {
        let lines = highlight("def f(x):  # add\n    return \"a\" + 1", CodeLang::Python);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0][0], ("def".into(), Tok::Keyword));
        assert!(lines[0].contains(&("f".into(), Tok::Function)));
        assert!(lines[0].contains(&("# add".into(), Tok::Comment)));
        assert!(lines[1].contains(&("\"a\"".into(), Tok::String)));
        assert!(lines[1].contains(&("1".into(), Tok::Number)));
        let text: String = lines.iter().map(|l| l.iter().map(|s| s.0.as_str()).collect::<String>()).collect::<Vec<_>>().join("\n");
        assert_eq!(text, "def f(x):  # add\n    return \"a\" + 1");
        let json = highlight("{\"k\": \"v\"}", CodeLang::Json);
        assert!(json[0].contains(&("\"k\"".into(), Tok::Property)));
        assert!(json[0].contains(&("\"v\"".into(), Tok::String)));
    }
}
