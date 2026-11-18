//! Incremental syntax highlighting on top of syntect.
//!
//! Each line caches its highlighted spans plus the parser state at its end.
//! On edit we only re-highlight from the first changed line until the parser
//! state converges with the old cache again, so typing stays O(changed lines).

use eframe::egui::{
    Color32, FontId,
    text::{LayoutJob, TextFormat},
};
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::str::FromStr;
use syntect::highlighting::{
    Color, FontStyle, HighlightState, Highlighter, RangedHighlightIterator, ScopeSelectors,
    StyleModifier, Theme, ThemeItem, ThemeSettings,
};
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};

const MAX_LINE_LEN: usize = 5000;

pub struct Highlighting {
    pub ss: SyntaxSet,
    dark: Theme,
    light: Theme,
}

impl Highlighting {
    pub fn new() -> Self {
        Self {
            ss: SyntaxSet::load_defaults_newlines(),
            dark: make_theme(true),
            light: make_theme(false),
        }
    }

    pub fn theme(&self, dark: bool) -> &Theme {
        if dark { &self.dark } else { &self.light }
    }

    /// Pick a syntax name for a file.
    pub fn syntax_for(&self, path: &Path, first_line: &str) -> Option<String> {
        let fname = path
            .file_name()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let ext = path
            .extension()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let by_name = match fname.as_str() {
            "dockerfile" | ".bashrc" | ".zshrc" | ".profile" => Some("sh"),
            "makefile" | "gnumakefile" => Some("make"),
            ".env" | ".editorconfig" | ".gitconfig" => Some("properties"),
            _ if fname.ends_with(".blade.php") => Some("php"),
            _ => None,
        };
        let mapped = by_name.unwrap_or(match ext.as_str() {
            "ts" | "tsx" | "mts" | "cts" | "jsx" | "mjs" | "cjs" => "js",
            "vue" | "svelte" | "twig" | "htm" | "xhtml" | "astro" => "html",
            "scss" | "less" | "sass" | "pcss" => "css",
            "kt" | "kts" | "dart" | "groovy" | "gradle" => "java",
            "phtml" | "inc" => "php",
            "yml" => "yaml",
            "jsonc" | "json5" | "lock" => "json",
            "h" | "hpp" | "hh" | "cc" | "cxx" => "cpp",
            "toml" | "ini" | "cfg" | "conf" | "env" => "properties",
            "zsh" | "bash" | "fish" => "sh",
            "svg" | "csproj" | "xaml" | "plist" | "vcxproj" | "props" => "xml",
            "markdown" | "mdx" => "md",
            other => other,
        });
        let s = self
            .ss
            .find_syntax_by_extension(mapped)
            .or_else(|| self.ss.find_syntax_by_first_line(first_line))?;
        if s.name == "Plain Text" {
            None
        } else {
            Some(s.name.clone())
        }
    }
}

#[derive(Clone)]
struct Span {
    start: u32,
    end: u32,
    color: Color32,
    italic: bool,
}

#[derive(Clone)]
struct HLine {
    hash: u64,
    end_parse: ParseState,
    end_hl: HighlightState,
    spans: Vec<Span>,
}

#[derive(Default)]
pub struct LineCache {
    syntax: String,
    dark: bool,
    lines: Vec<HLine>,
}

fn line_hash(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

fn split_lines(text: &str) -> impl Iterator<Item = &str> {
    text.split_inclusive('\n')
}

fn c32(c: Color) -> Color32 {
    Color32::from_rgb(c.r, c.g, c.b)
}

impl LineCache {
    pub fn update(&mut self, h: &Highlighting, syntax: &SyntaxReference, dark: bool, text: &str) {
        let highlighter = Highlighter::new(h.theme(dark));
        if self.syntax != syntax.name || self.dark != dark {
            self.lines.clear();
            self.syntax = syntax.name.clone();
            self.dark = dark;
        }
        let new_lines: Vec<&str> = split_lines(text).collect();
        let hashes: Vec<u64> = new_lines.iter().map(|l| line_hash(l)).collect();
        let n = new_lines.len();
        let mut old: Vec<Option<HLine>> = std::mem::take(&mut self.lines).into_iter().map(Some).collect();
        let m = old.len();

        // common prefix / suffix by line hash
        let mut p = 0;
        while p < n && p < m && old[p].as_ref().unwrap().hash == hashes[p] {
            p += 1;
        }
        let mut s = 0;
        while s < n - p && s < m - p && old[m - 1 - s].as_ref().unwrap().hash == hashes[n - 1 - s] {
            s += 1;
        }

        let init_parse = ParseState::new(syntax);
        let init_hl = HighlightState::new(&highlighter, ScopeStack::new());

        let mut out: Vec<HLine> = Vec::with_capacity(n);
        for slot in old.iter_mut().take(p) {
            out.push(slot.take().unwrap());
        }
        let (mut ps, mut hs) = match out.last() {
            Some(l) => (l.end_parse.clone(), l.end_hl.clone()),
            None => (init_parse.clone(), init_hl.clone()),
        };

        let mut i = p;
        while i < n {
            if i >= n - s {
                // inside the unchanged suffix: old line k corresponds to new line i
                let k = i + m - n;
                let converged = if k == 0 {
                    ps == init_parse && hs == init_hl
                } else if k - 1 < p {
                    let l = &out[k - 1];
                    l.end_parse == ps && l.end_hl == hs
                } else {
                    match &old[k - 1] {
                        Some(l) => l.end_parse == ps && l.end_hl == hs,
                        None => false,
                    }
                };
                if converged {
                    for slot in old.iter_mut().skip(k) {
                        if let Some(l) = slot.take() {
                            out.push(l);
                        }
                    }
                    break;
                }
            }
            let line = new_lines[i];
            let spans = highlight_line(&mut ps, &mut hs, line, &highlighter, &h.ss);
            out.push(HLine {
                hash: hashes[i],
                end_parse: ps.clone(),
                end_hl: hs.clone(),
                spans,
            });
            i += 1;
        }
        self.lines = out;
    }

    pub fn layout_job(&self, text: &str, font: FontId, default: Color32) -> LayoutJob {
        let mut job = LayoutJob::default();
        job.wrap.max_width = f32::INFINITY;
        let plain = TextFormat::simple(font.clone(), default);
        for (idx, line) in split_lines(text).enumerate() {
            let spans: &[Span] = self.lines.get(idx).map(|l| &l.spans[..]).unwrap_or(&[]);
            let mut pos = 0usize;
            for sp in spans {
                let (a, b) = (sp.start as usize, sp.end as usize);
                if a < pos || b > line.len() || !line.is_char_boundary(a) || !line.is_char_boundary(b) {
                    continue;
                }
                if a > pos {
                    job.append(&line[pos..a], 0.0, plain.clone());
                }
                let mut f = TextFormat::simple(font.clone(), sp.color);
                f.italics = sp.italic;
                job.append(&line[a..b], 0.0, f);
                pos = b;
            }
            if pos < line.len() {
                job.append(&line[pos..], 0.0, plain.clone());
            }
        }
        job
    }
}

fn highlight_line(
    ps: &mut ParseState,
    hs: &mut HighlightState,
    line: &str,
    highlighter: &Highlighter,
    ss: &SyntaxSet,
) -> Vec<Span> {
    if line.len() > MAX_LINE_LEN {
        return vec![];
    }
    let owned;
    let l: &str = if line.ends_with('\n') {
        line
    } else {
        owned = format!("{line}\n");
        &owned
    };
    let ops = match ps.parse_line(l, ss) {
        Ok(o) => o,
        Err(_) => return vec![],
    };
    let mut spans: Vec<Span> = Vec::new();
    for (style, _, range) in RangedHighlightIterator::new(hs, &ops, l, highlighter) {
        let end = range.end.min(line.len());
        if range.start >= end {
            continue;
        }
        let color = c32(style.foreground);
        let italic = style.font_style.contains(FontStyle::ITALIC);
        if let Some(last) = spans.last_mut() {
            if last.color == color && last.italic == italic && last.end as usize == range.start {
                last.end = end as u32;
                continue;
            }
        }
        spans.push(Span {
            start: range.start as u32,
            end: end as u32,
            color,
            italic,
        });
    }
    spans
}

/// Plain (unhighlighted) layout.
pub fn plain_job(text: &str, font: FontId, color: Color32) -> LayoutJob {
    let mut job = LayoutJob::simple(text.to_owned(), font, color, f32::INFINITY);
    job.wrap.max_width = f32::INFINITY;
    job
}

/// Unified diff layout with colored line backgrounds.
pub fn diff_job(text: &str, font: FontId, pal: &crate::theme::Palette) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    for line in split_lines(text) {
        let mut f = TextFormat::simple(font.clone(), pal.text);
        if line.starts_with("+++") || line.starts_with("---") || line.starts_with("diff ") || line.starts_with("index ") {
            f.color = pal.text_weak;
        } else if line.starts_with('+') {
            f.background = pal.diff_add_bg;
        } else if line.starts_with('-') {
            f.background = pal.diff_del_bg;
        } else if line.starts_with("@@") {
            f.color = pal.hunk;
        }
        job.append(line, 0.0, f);
    }
    job
}

// ---------------------------------------------------------------------------
// Themes (JetBrains-like)

fn col(c: u32) -> Color {
    Color {
        r: (c >> 16) as u8,
        g: (c >> 8) as u8,
        b: c as u8,
        a: 0xFF,
    }
}

fn make_theme(dark: bool) -> Theme {
    let rules: &[(&str, u32, u32, bool)] = &[
        // (scope selector, dark color, light color, italic)
        ("comment", 0x7A7E85, 0x8C8C8C, true),
        ("comment.block.documentation", 0x5F826B, 0x8C8C8C, true),
        ("string, punctuation.definition.string", 0x6AAB73, 0x067D17, false),
        ("constant.character.escape", 0xCF8E6D, 0x0037A6, false),
        ("constant.numeric", 0x2AACB8, 0x1750EB, false),
        ("constant.language", 0xCF8E6D, 0x0033B3, false),
        ("constant.other, variable.other.constant, support.constant", 0xC77DBB, 0x871094, false),
        ("keyword, storage, storage.type, storage.modifier", 0xCF8E6D, 0x0033B3, false),
        ("keyword.operator", 0xBCBEC4, 0x080808, false),
        ("keyword.operator.word, keyword.operator.new", 0xCF8E6D, 0x0033B3, false),
        ("entity.name.function, support.function, variable.function, meta.function-call.identifier", 0x56A8F5, 0x00627A, false),
        ("variable.other.php, variable.other.member, variable.other.property, variable.language, variable.other.readwrite.instance", 0xC77DBB, 0x871094, false),
        ("entity.name.tag", 0xD5B778, 0x0033B3, false),
        ("punctuation.definition.tag", 0xD5B778, 0x0033B3, false),
        ("entity.other.attribute-name", 0xBABABA, 0x174AD4, false),
        ("meta.annotation, storage.type.annotation, entity.name.function.decorator, meta.attribute", 0xB3AE60, 0x9E880D, false),
        ("meta.mapping.key string, support.type.property-name", 0xC77DBB, 0x871094, false),
        ("entity.name.section, markup.heading", 0x56A8F5, 0x0033B3, false),
        ("markup.raw, markup.inline.raw", 0x6AAB73, 0x067D17, false),
        ("markup.underline.link, string.other.link", 0x548AF7, 0x2E55A3, false),
        ("markup.italic", 0xBCBEC4, 0x080808, true),
        ("entity.name.type, entity.name.class, support.class, support.type, entity.other.inherited-class", 0x16BAAC, 0x008080, false),
        ("constant.other.color, constant.other.rgb-value", 0x2AACB8, 0x1750EB, false),
        ("invalid", 0xF75464, 0xF50000, false),
    ];
    let scopes = rules
        .iter()
        .filter_map(|(sel, d, l, it)| {
            Some(ThemeItem {
                scope: ScopeSelectors::from_str(sel).ok()?,
                style: StyleModifier {
                    foreground: Some(col(if dark { *d } else { *l })),
                    background: None,
                    font_style: Some(if *it { FontStyle::ITALIC } else { FontStyle::empty() }),
                },
            })
        })
        .collect();
    Theme {
        name: Some(if dark { "VibeStorm Dark" } else { "VibeStorm Light" }.into()),
        author: None,
        settings: ThemeSettings {
            foreground: Some(col(if dark { 0xBCBEC4 } else { 0x080808 })),
            background: Some(col(if dark { 0x1E1F22 } else { 0xFFFFFF })),
            ..Default::default()
        },
        scopes,
    }
}
