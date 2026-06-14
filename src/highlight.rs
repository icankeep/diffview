//! Whole-file syntax highlighting via syntect, converted to ratatui styles.

use std::path::Path;

use ratatui::style::{Color, Style};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;

/// One source line as styled (fg-colored) text fragments.
pub type StyledLine = Vec<(Style, String)>;

const MAX_HIGHLIGHT_LINES: usize = 20_000;

pub struct Highlighter {
    syntaxes: SyntaxSet,
    theme: Theme,
}

impl Highlighter {
    pub fn new() -> Self {
        let syntaxes = SyntaxSet::load_defaults_newlines();
        let mut themes = ThemeSet::load_defaults();
        let theme = themes
            .themes
            .remove("base16-ocean.dark")
            .unwrap_or_default();
        Self { syntaxes, theme }
    }

    /// Highlight a whole file. Falls back to plain text for unknown syntaxes
    /// and very large files. Only foreground colors are produced; diff
    /// backgrounds are layered on top by the renderer.
    pub fn highlight(&self, path: &Path, lines: &[String]) -> Vec<StyledLine> {
        let syntax = path
            .extension()
            .and_then(|e| e.to_str())
            .and_then(|e| self.syntaxes.find_syntax_by_extension(e))
            .or_else(|| {
                path.file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|n| self.syntaxes.find_syntax_by_extension(n))
            })
            .or_else(|| {
                lines
                    .first()
                    .and_then(|l| self.syntaxes.find_syntax_by_first_line(l))
            })
            .filter(|_| lines.len() <= MAX_HIGHLIGHT_LINES);
        let Some(syntax) = syntax else {
            return lines
                .iter()
                .map(|l| vec![(Style::new(), l.clone())])
                .collect();
        };
        let mut hl = HighlightLines::new(syntax, &self.theme);
        lines
            .iter()
            .map(|l| {
                let with_nl = format!("{l}\n");
                match hl.highlight_line(&with_nl, &self.syntaxes) {
                    Ok(spans) => spans
                        .into_iter()
                        .filter_map(|(st, txt)| {
                            let txt = txt.trim_end_matches('\n');
                            (!txt.is_empty()).then(|| (convert(st), txt.to_string()))
                        })
                        .collect(),
                    Err(_) => vec![(Style::new(), l.clone())],
                }
            })
            .collect()
    }
}

fn convert(s: syntect::highlighting::Style) -> Style {
    let f = s.foreground;
    if f.a == 0 {
        Style::new()
    } else {
        Style::new().fg(Color::Rgb(f.r, f.g, f.b))
    }
}
