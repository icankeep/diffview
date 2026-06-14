//! ratatui rendering: tree pane, side-by-side diff panes, full view, status bar.

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;

use crate::app::{App, Focus, SearchScope, Source, View};
use crate::highlight::StyledLine;
use crate::model::{Cell, FileStatus, LineKind, Row};

const DEL_BG: Color = Color::Rgb(58, 22, 22);
const DEL_EMPH_BG: Color = Color::Rgb(112, 36, 36);
const ADD_BG: Color = Color::Rgb(18, 52, 26);
const ADD_EMPH_BG: Color = Color::Rgb(28, 94, 44);
const FILLER_BG: Color = Color::Rgb(24, 24, 28);
const SEARCH_BG: Color = Color::Rgb(115, 90, 25);
const LINENO_FG: Color = Color::Rgb(108, 112, 122);
const HEADER_FG: Color = Color::Rgb(100, 160, 190);
const CURSOR_BG: Color = Color::Rgb(50, 52, 70);
const DIR_FG: Color = Color::Rgb(140, 150, 175);
const DIM_FG: Color = Color::Rgb(120, 120, 130);
const STATUS_BG: Color = Color::Rgb(34, 36, 44);
const ACCENT: Color = Color::Rgb(110, 150, 230);

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [main, status] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());
    let tree_w = app
        .tree_width
        .clamp(18, frame.area().width.saturating_sub(20));
    let [tree_a, old_a, new_a] = Layout::horizontal([
        Constraint::Length(tree_w),
        Constraint::Fill(1),
        Constraint::Fill(1),
    ])
    .areas(main);
    app.tree_area = tree_a;
    app.diff_area = old_a.union(new_a);

    draw_tree(frame, app, tree_a);
    match app.view {
        View::Diff => {
            let added = app
                .selected
                .and_then(|fi| app.files.get(fi))
                .is_some_and(|f| f.status == FileStatus::Added);
            if added {
                draw_added_diff(frame, app, old_a.union(new_a));
            } else {
                draw_diff(frame, app, old_a, new_a);
            }
        }
        View::Full => draw_full(frame, app, old_a.union(new_a)),
    }
    draw_status(frame, app, status);
    if app.help {
        draw_help(frame, frame.area());
    }
}

fn draw_added_diff(frame: &mut Frame, app: &mut App, area: Rect) {
    let border = border_style(app.focus == Focus::Diff);
    let Some(fi) = app.selected else {
        let block = Block::bordered().border_style(border).title(" diff ");
        frame.render_widget(
            Paragraph::new("select a file")
                .style(Style::new().fg(DIM_FG))
                .block(block),
            area,
        );
        return;
    };
    app.ensure_hl(fi);
    let f = &app.files[fi];
    let block = Block::bordered()
        .border_style(border)
        .title(format!(" {} · added ", f.path.display()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.diff_height = inner.height;

    if f.binary {
        frame.render_widget(
            Paragraph::new("(binary or oversized file)").style(Style::new().fg(DIM_FG)),
            inner,
        );
        return;
    }

    let h = inner.height as usize;
    app.scroll = app.scroll.min(f.rows.len().saturating_sub(h.max(1)));
    let styled = app.hl_cache.get(&(fi, false));
    let nw = digits(f.new_lines.len());
    let width = inner.width as usize;

    let mut out: Vec<Line> = Vec::with_capacity(h);
    for row in f.rows.iter().skip(app.scroll).take(h) {
        match row {
            Row::HunkHeader(i) => {
                let hk = &f.hunks[*i];
                out.push(Line::styled(
                    format!("@@ +{},{} @@", hk.new_start, hk.new_count),
                    Style::new().fg(HEADER_FG).add_modifier(Modifier::ITALIC),
                ));
            }
            Row::Line { new, .. } => {
                let line = render_cell(new, &f.new_lines, styled, nw, width, app.hscroll);
                out.push(apply_search_highlight(line, app));
            }
        }
    }
    frame.render_widget(Paragraph::new(out), inner);
}

fn border_style(focused: bool) -> Style {
    if focused {
        Style::new().fg(ACCENT)
    } else {
        Style::new().fg(Color::Rgb(70, 72, 82))
    }
}

fn draw_tree(frame: &mut Frame, app: &mut App, area: Rect) {
    let adds: usize = app.files.iter().map(|f| f.additions).sum();
    let dels: usize = app.files.iter().map(|f| f.deletions).sum();
    let block = Block::bordered()
        .border_style(border_style(app.focus == Focus::Tree))
        .title(format!(
            " Changes · {} files +{adds} -{dels} ",
            app.files.len()
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.tree_height = inner.height;
    app.tree_inner = inner;

    let h = inner.height as usize;
    if app.cursor < app.tree_scroll {
        app.tree_scroll = app.cursor;
    } else if h > 0 && app.cursor >= app.tree_scroll + h {
        app.tree_scroll = app.cursor + 1 - h;
    }

    if app.tree_rows.is_empty() {
        frame.render_widget(
            Paragraph::new("no changes").style(Style::new().fg(DIM_FG)),
            inner,
        );
        return;
    }

    let mut lines: Vec<Line> = Vec::with_capacity(h);
    for (i, row) in app
        .tree_rows
        .iter()
        .enumerate()
        .skip(app.tree_scroll)
        .take(h)
    {
        let mut spans: Vec<Span> = vec![Span::raw("  ".repeat(row.depth))];
        match row.file {
            None => {
                spans.push(Span::styled(
                    if row.expanded { "▾ " } else { "▸ " },
                    Style::new().fg(DIM_FG),
                ));
                spans.push(Span::styled(
                    format!("{}/", row.label),
                    Style::new().fg(DIR_FG).add_modifier(Modifier::BOLD),
                ));
            }
            Some(fi) => {
                let f = &app.files[fi];
                let color = match f.status {
                    FileStatus::Added => Color::Rgb(95, 200, 120),
                    FileStatus::Modified => Color::Rgb(220, 180, 85),
                    FileStatus::Deleted => Color::Rgb(225, 105, 105),
                };
                spans.push(Span::styled(
                    format!("{} ", f.status.letter()),
                    Style::new().fg(color),
                ));
                let mut st = Style::new();
                if app.selected == Some(fi) {
                    st = st.add_modifier(Modifier::BOLD);
                }
                spans.push(Span::styled(row.label.clone(), st));
            }
        }
        let mut line = Line::from(spans);
        if i == app.cursor {
            let pad = (inner.width as usize).saturating_sub(line.width());
            if pad > 0 {
                line.push_span(Span::raw(" ".repeat(pad)));
            }
            line = line.style(Style::new().bg(CURSOR_BG));
        }
        lines.push(line);
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_diff(frame: &mut Frame, app: &mut App, old_a: Rect, new_a: Rect) {
    let border = border_style(app.focus == Focus::Diff);
    let Some(fi) = app.selected else {
        let block = Block::bordered().border_style(border).title(" diff ");
        frame.render_widget(
            Paragraph::new("select a file")
                .style(Style::new().fg(DIM_FG))
                .block(block),
            old_a.union(new_a),
        );
        return;
    };
    app.ensure_hl(fi);
    let f = &app.files[fi];

    let old_title = match &app.source {
        Source::Git { base } => format!(" {} @ {base} ", f.path.display()),
        Source::Directory => format!(" {} @ empty ", f.path.display()),
    };
    let new_title = match &app.source {
        Source::Git { .. } => " worktree ",
        Source::Directory => " directory ",
    };
    let old_block = Block::bordered().border_style(border).title(old_title);
    let new_block = Block::bordered().border_style(border).title(new_title);
    let old_inner = old_block.inner(old_a);
    let new_inner = new_block.inner(new_a);
    frame.render_widget(old_block, old_a);
    frame.render_widget(new_block, new_a);
    app.diff_height = old_inner.height;

    if f.binary {
        frame.render_widget(
            Paragraph::new("(binary or oversized file)").style(Style::new().fg(DIM_FG)),
            new_inner,
        );
        return;
    }

    let h = old_inner.height as usize;
    app.scroll = app.scroll.min(f.rows.len().saturating_sub(h.max(1)));
    let scroll = app.scroll;

    let old_styled = app.hl_cache.get(&(fi, true));
    let new_styled = app.hl_cache.get(&(fi, false));
    let old_w = old_inner.width as usize;
    let new_w = new_inner.width as usize;
    let old_nw = digits(f.old_lines.len());
    let new_nw = digits(f.new_lines.len());

    let mut ol: Vec<Line> = Vec::with_capacity(h);
    let mut nl: Vec<Line> = Vec::with_capacity(h);
    for row in f.rows.iter().skip(scroll).take(h) {
        match row {
            Row::HunkHeader(i) => {
                let hk = &f.hunks[*i];
                let text = format!(
                    "@@ -{},{} +{},{} @@",
                    hk.old_start, hk.old_count, hk.new_start, hk.new_count
                );
                let style = Style::new().fg(HEADER_FG).add_modifier(Modifier::ITALIC);
                ol.push(Line::styled(text.clone(), style));
                nl.push(Line::styled(text, style));
            }
            Row::Line { old, new } => {
                let old_line =
                    render_cell(old, &f.old_lines, old_styled, old_nw, old_w, app.hscroll);
                let new_line =
                    render_cell(new, &f.new_lines, new_styled, new_nw, new_w, app.hscroll);
                ol.push(apply_search_highlight(old_line, app));
                nl.push(apply_search_highlight(new_line, app));
            }
        }
    }
    frame.render_widget(Paragraph::new(ol), old_inner);
    frame.render_widget(Paragraph::new(nl), new_inner);
}

fn render_cell(
    cell: &Cell,
    src: &[String],
    styled: Option<&Vec<StyledLine>>,
    num_w: usize,
    width: usize,
    hscroll: usize,
) -> Line<'static> {
    if cell.kind == LineKind::Filler {
        return Line::styled(" ".repeat(width), Style::new().bg(FILLER_BG));
    }
    let (row_bg, emph_bg) = match cell.kind {
        LineKind::Del => (Some(DEL_BG), Some(DEL_EMPH_BG)),
        LineKind::Add => (Some(ADD_BG), Some(ADD_EMPH_BG)),
        _ => (None, None),
    };
    let ln = cell.line_no.unwrap_or(0);
    let mut spans: Vec<Span<'static>> = vec![Span::styled(
        format!("{ln:>num_w$} "),
        Style::new().fg(LINENO_FG),
    )];
    let content: StyledLine = styled
        .and_then(|s| s.get(ln.saturating_sub(1)))
        .cloned()
        .unwrap_or_else(|| {
            vec![(
                Style::new(),
                src.get(ln.saturating_sub(1)).cloned().unwrap_or_default(),
            )]
        });
    match emph_bg {
        Some(e) if !cell.inline.is_empty() => spans.extend(apply_inline(content, &cell.inline, e)),
        _ => spans.extend(content.into_iter().map(|(st, tx)| Span::styled(tx, st))),
    }
    let prefix_len = num_w + 1;
    if hscroll > 0 {
        spans = scroll_code_spans(spans, prefix_len, hscroll);
    }
    let mut line = Line::from(spans);
    if let Some(bg) = row_bg {
        let pad = width.saturating_sub(line.width());
        if pad > 0 {
            line.push_span(Span::raw(" ".repeat(pad)));
        }
        line = line.style(Style::new().bg(bg));
    }
    line
}

fn scroll_code_spans(
    spans: Vec<Span<'static>>,
    prefix_len: usize,
    hscroll: usize,
) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut code_pos = 0usize;
    for (idx, span) in spans.into_iter().enumerate() {
        if idx == 0 {
            out.push(span);
            continue;
        }
        let style = span.style;
        let text = span.content.into_owned();
        let len = text.chars().count();
        let end = code_pos + len;
        if end <= hscroll {
            code_pos = end;
            continue;
        }
        let skip = hscroll.saturating_sub(code_pos);
        let clipped: String = text.chars().skip(skip).collect();
        if !clipped.is_empty() {
            out.push(Span::styled(clipped, style));
        }
        code_pos = end;
    }
    if out.len() == 1 {
        out.push(Span::raw(""));
    }
    let visible_width: usize = out.iter().map(Span::width).sum();
    if visible_width < prefix_len {
        out.push(Span::raw(" ".repeat(prefix_len - visible_width)));
    }
    out
}

/// Split styled fragments at the changed byte ranges and give the changed
/// pieces an emphasized background.
fn apply_inline(spans: StyledLine, ranges: &[(usize, usize)], emph: Color) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    for (st, tx) in spans {
        let (start, end) = (pos, pos + tx.len());
        let mut cuts: Vec<usize> = vec![start, end];
        for &(s, e) in ranges {
            if s > start && s < end {
                cuts.push(s);
            }
            if e > start && e < end {
                cuts.push(e);
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        for w in cuts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let Some(piece) = tx.get(a - start..b - start) else {
                continue;
            };
            if piece.is_empty() {
                continue;
            }
            let in_range = ranges.iter().any(|&(s, e)| a >= s && b <= e);
            let style = if in_range { st.bg(emph) } else { st };
            out.push(Span::styled(piece.to_string(), style));
        }
        pos = end;
    }
    out
}

fn draw_full(frame: &mut Frame, app: &mut App, area: Rect) {
    let border = border_style(app.focus == Focus::Diff);
    let Some(fi) = app.selected else {
        let block = Block::bordered().border_style(border).title(" file ");
        frame.render_widget(
            Paragraph::new("select a file")
                .style(Style::new().fg(DIM_FG))
                .block(block),
            area,
        );
        return;
    };
    app.ensure_full(fi);
    let f = &app.files[fi];
    let side = if f.status == FileStatus::Deleted {
        " (deleted, showing base)"
    } else {
        ""
    };
    let block = Block::bordered()
        .border_style(border)
        .title(format!(" {} · full{side} ", f.path.display()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.diff_height = inner.height;

    if f.binary {
        frame.render_widget(
            Paragraph::new("(binary or oversized file)").style(Style::new().fg(DIM_FG)),
            inner,
        );
        return;
    }

    let lines_src = f.full_lines();
    let h = inner.height as usize;
    app.scroll = app.scroll.min(lines_src.len().saturating_sub(h.max(1)));
    let scroll = app.scroll;

    let styled = app.hl_cache.get(&(fi, f.status == FileStatus::Deleted));
    let marks = app.full_marks.get(&fi);
    let nw = digits(lines_src.len());
    let width = inner.width as usize;

    let mut out: Vec<Line> = Vec::with_capacity(h);
    for (idx, raw) in lines_src.iter().enumerate().skip(scroll).take(h) {
        let ln = idx + 1;
        let mut spans: Vec<Span<'static>> = vec![Span::styled(
            format!("{ln:>nw$} "),
            Style::new().fg(LINENO_FG),
        )];
        match styled.and_then(|s| s.get(idx)) {
            Some(frags) => spans.extend(frags.iter().map(|(st, tx)| Span::styled(tx.clone(), *st))),
            None => spans.push(Span::raw(raw.clone())),
        }
        if app.hscroll > 0 {
            spans = scroll_code_spans(spans, nw + 1, app.hscroll);
        }
        let mut line = Line::from(spans);
        if marks.is_some_and(|m| m.contains(&ln)) {
            let pad = width.saturating_sub(line.width());
            if pad > 0 {
                line.push_span(Span::raw(" ".repeat(pad)));
            }
            line = line.style(Style::new().bg(ADD_BG));
        }
        out.push(apply_search_highlight(line, app));
    }
    frame.render_widget(Paragraph::new(out), inner);
}

fn apply_search_highlight(line: Line<'static>, app: &App) -> Line<'static> {
    if app.search_scope != SearchScope::Diff || app.search_query.is_empty() {
        return line;
    }
    highlight_line_text(line, &app.search_query)
}

fn highlight_line_text(line: Line<'static>, query: &str) -> Line<'static> {
    let needle = query.to_ascii_lowercase();
    if needle.is_empty() {
        return line;
    }
    let mut out = Vec::new();
    for span in line.spans {
        let style = span.style;
        let text = span.content.into_owned();
        let lower = text.to_ascii_lowercase();
        let mut start = 0usize;
        while let Some(rel) = lower[start..].find(&needle) {
            let hit_start = start + rel;
            let hit_end = hit_start + needle.len();
            if !text.is_char_boundary(hit_start) || !text.is_char_boundary(hit_end) {
                break;
            }
            if hit_start > start {
                out.push(Span::styled(text[start..hit_start].to_string(), style));
            }
            out.push(Span::styled(
                text[hit_start..hit_end].to_string(),
                style.bg(SEARCH_BG).fg(Color::White),
            ));
            start = hit_end;
        }
        if start < text.len() {
            out.push(Span::styled(text[start..].to_string(), style));
        }
    }
    Line::from(out).style(line.style)
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    frame.render_widget(Block::new().style(Style::new().bg(STATUS_BG)), area);
    let keys = if app.search_active || !app.search_query.is_empty() {
        let scope = match app.search_scope {
            SearchScope::Tree => "tree",
            SearchScope::Diff => "file",
        };
        let suffix = app
            .search_status()
            .map(|s| format!(" {}/{}", s.current, s.total))
            .unwrap_or_default();
        if app.search_active {
            format!("/{scope}:{}{} ", app.search_query, suffix)
        } else {
            format!(
                "search {scope}:{}{} · n/N next/prev ",
                app.search_query, suffix
            )
        }
    } else {
        String::from(
            " q quit · ? help · / search · [] folders · drag split · H/L sideways · v view · r refresh",
        )
    };
    frame.render_widget(
        Paragraph::new(keys).style(Style::new().fg(DIM_FG).bg(STATUS_BG)),
        area,
    );
    let right = match app.selected {
        Some(fi) => {
            let f = &app.files[fi];
            let hunk = match (app.current_hunk(), f.hunks.len()) {
                (Some(k), total) if total > 0 => format!(" · hunk {}/{total}", k + 1),
                _ => String::new(),
            };
            format!(
                "{} +{} -{}{hunk} ",
                f.path.display(),
                f.additions,
                f.deletions
            )
        }
        None => String::from("no changes "),
    };
    frame.render_widget(
        Paragraph::new(right)
            .alignment(Alignment::Right)
            .style(Style::new().fg(DIM_FG).bg(STATUS_BG)),
        area,
    );
}

fn digits(n: usize) -> usize {
    n.max(1).to_string().len().max(3)
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let w = area.width.saturating_sub(8).clamp(58, 92);
    let h = 23.min(area.height.saturating_sub(4));
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let rect = Rect::new(x, y, w, h);
    let block = Block::bordered()
        .title(" help · ? / Esc close ")
        .border_style(Style::new().fg(ACCENT));
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);

    let width = inner.width as usize;
    let lines = help_lines(width, inner.height as usize);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn help_lines(width: usize, max_lines: usize) -> Vec<Line<'static>> {
    let groups: &[(&str, &[(&str, &str)])] = &[
        (
            "Navigation",
            &[
                ("Tab", "switch focus between file tree and diff"),
                ("j/k, arrows", "move one line; numeric prefix jumps N lines"),
                ("PgUp/PgDn", "page current pane"),
                ("Ctrl+u/d", "page current pane up/down"),
                ("g / G", "jump to top or bottom"),
                ("[] / {}", "jump folders; braces jump top-level folders"),
            ],
        ),
        (
            "Search",
            &[
                ("/", "search the currently focused pane"),
                ("Enter / Esc", "accept or leave search input"),
                ("n / N", "next or previous result after search"),
                ("Down/Up", "next or previous result while typing search"),
            ],
        ),
        (
            "View",
            &[
                ("v", "toggle diff and full-file view"),
                ("H / L", "scroll code horizontally"),
                ("Shift/Ctrl arrows", "fine or large horizontal code scroll"),
            ],
        ),
        (
            "Mouse",
            &[
                ("click tree", "select file or toggle folder"),
                ("click diff", "focus diff pane"),
                ("drag split", "resize the file tree"),
                ("wheel", "scroll the pane under the pointer"),
            ],
        ),
        (
            "System",
            &[
                ("r", "refresh changes"),
                ("? / Esc", "close this help"),
                ("q", "quit"),
            ],
        ),
    ];
    let key_w = 18usize.min(width.saturating_sub(12)).max(8);
    let mut lines = Vec::new();
    for (title, items) in groups {
        if !lines.is_empty() {
            lines.push(Line::raw(""));
        }
        lines.push(Line::from(vec![Span::styled(
            one_line_prefix(title, width),
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        )]));
        for (keys, desc) in *items {
            let desc_w = width.saturating_sub(key_w + 3);
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    pad_right(&one_line_prefix(keys, key_w), key_w),
                    Style::new()
                        .fg(Color::Rgb(235, 205, 120))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    one_line_prefix(desc, desc_w),
                    Style::new().fg(Color::Rgb(205, 210, 220)),
                ),
            ]));
        }
    }
    lines.truncate(max_lines);
    lines
}

fn pad_right(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len >= width {
        text.to_string()
    } else {
        format!("{text}{}", " ".repeat(width - len))
    }
}

fn one_line_prefix(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    for ch in text.chars().take(width) {
        if ch == '\n' || ch == '\r' {
            break;
        }
        out.push(ch);
    }
    out
}
