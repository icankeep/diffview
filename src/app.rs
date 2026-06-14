//! Application state and key handling.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use anyhow::Result;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;

use crate::git;
use crate::highlight::{Highlighter, StyledLine};
use crate::model::FileEntry;
use crate::tree::{self, Tree, TreeRow};

#[derive(Debug, Clone)]
pub enum Source {
    Git { base: String },
    Directory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Tree,
    Diff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Diff,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchScope {
    Tree,
    Diff,
}

pub struct App {
    pub root: PathBuf,
    pub source: Source,
    pub files: Vec<FileEntry>,
    pub tree: Tree,
    pub tree_rows: Vec<TreeRow>,
    pub cursor: usize,
    pub tree_scroll: usize,
    pub selected: Option<usize>,
    pub focus: Focus,
    pub view: View,
    /// Vertical offset into the diff rows / full-file lines.
    pub scroll: usize,
    /// Horizontal character offset into rendered code text.
    pub hscroll: usize,
    /// Inner heights recorded during the last draw, for paging and clamping.
    pub diff_height: u16,
    pub tree_height: u16,
    pub tree_width: u16,
    pub tree_area: Rect,
    pub tree_inner: Rect,
    pub diff_area: Rect,
    pub resizing_tree: bool,
    pub hl: Highlighter,
    /// Cache key: (file index, is_old_side).
    pub hl_cache: HashMap<(usize, bool), Vec<StyledLine>>,
    /// Changed line numbers per file for full-file view.
    pub full_marks: HashMap<usize, HashSet<usize>>,
    /// Pending numeric prefix for commands like 20j / 120g.
    pub count_prefix: Option<usize>,
    pub search_active: bool,
    pub search_query: String,
    pub search_scope: SearchScope,
    pub help: bool,
    pub quit: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchStatus {
    pub scope: SearchScope,
    pub current: usize,
    pub total: usize,
}

impl App {
    pub fn new(root: PathBuf, source: Source) -> Result<Self> {
        let files = collect_files(&root, &source)?;
        let tree = tree::build(&files);
        let tree_rows = tree.flatten();
        let cursor = tree_rows.iter().position(|r| r.file.is_some()).unwrap_or(0);
        let selected = tree_rows.get(cursor).and_then(|r| r.file);
        Ok(Self {
            root,
            source,
            files,
            tree,
            tree_rows,
            cursor,
            tree_scroll: 0,
            selected,
            focus: Focus::Tree,
            view: View::Diff,
            scroll: 0,
            hscroll: 0,
            diff_height: 24,
            tree_height: 24,
            tree_width: 32,
            tree_area: Rect::default(),
            tree_inner: Rect::default(),
            diff_area: Rect::default(),
            resizing_tree: false,
            hl: Highlighter::new(),
            hl_cache: HashMap::new(),
            full_marks: HashMap::new(),
            count_prefix: None,
            search_active: false,
            search_query: String::new(),
            search_scope: SearchScope::Tree,
            help: false,
            quit: false,
        })
    }

    pub fn reload(&mut self) {
        let keep = self.selected.map(|i| self.files[i].path.clone());
        let Ok(files) = collect_files(&self.root, &self.source) else {
            return;
        };
        self.files = files;
        self.tree = tree::build(&self.files);
        self.tree_rows = self.tree.flatten();
        self.hl_cache.clear();
        self.full_marks.clear();
        self.selected = keep
            .and_then(|p| self.files.iter().position(|f| f.path == p))
            .or(if self.files.is_empty() { None } else { Some(0) });
        self.cursor = match self.selected {
            Some(fi) => self
                .tree_rows
                .iter()
                .position(|r| r.file == Some(fi))
                .unwrap_or(0),
            None => 0,
        };
        self.scroll = 0;
        self.hscroll = 0;
        self.tree_scroll = 0;
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        use KeyCode::*;
        if self.help {
            match key.code {
                Char('?') | Esc | Char('q') => self.help = false,
                _ => {}
            }
            return;
        }
        if self.search_active {
            self.on_search_key(key);
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                Char('c') => self.quit = true,
                Char('d') => self.page(1),
                Char('u') => self.page(-1),
                KeyCode::Left => self.horizontal_scroll(-24),
                KeyCode::Right => self.horizontal_scroll(24),
                _ => return,
            }
            return;
        }
        match key.code {
            Char(c) if c.is_ascii_digit() => self.push_count_digit(c),
            Char('q') | Esc => {
                self.count_prefix = None;
                self.quit = true;
            }
            Char('/') => self.start_search(),
            Char('?') => {
                self.count_prefix = None;
                self.help = true;
            }
            Tab | BackTab => {
                self.count_prefix = None;
                self.focus = match self.focus {
                    Focus::Tree => Focus::Diff,
                    Focus::Diff => Focus::Tree,
                }
            }
            Left if key.modifiers.contains(KeyModifiers::SHIFT) => self.horizontal_scroll(-4),
            Right if key.modifiers.contains(KeyModifiers::SHIFT) => self.horizontal_scroll(4),
            Left if key.modifiers.contains(KeyModifiers::CONTROL) => self.horizontal_scroll(-24),
            Right if key.modifiers.contains(KeyModifiers::CONTROL) => self.horizontal_scroll(24),
            Char('H') => self.horizontal_scroll(-24),
            Char('L') => self.horizontal_scroll(24),
            Char('h') | Left => {
                self.count_prefix = None;
                self.focus = Focus::Tree;
            }
            Char('l') | Right => {
                self.count_prefix = None;
                self.focus = Focus::Diff;
            }
            Char('j') | Down => self.step_count(1),
            Char('k') | Up => self.step_count(-1),
            PageDown => self.page_count(1),
            PageUp => self.page_count(-1),
            Char(']') => self.jump_folder(1, false),
            Char('[') => self.jump_folder(-1, false),
            Char('}') => self.jump_folder(1, true),
            Char('{') => self.jump_folder(-1, true),
            Char('g') => self.to_line_or_edge(),
            Home => {
                self.count_prefix = None;
                self.to_edge(false);
            }
            Char('G') | End => {
                self.count_prefix = None;
                self.to_edge(true);
            }
            Char('n') if !self.search_query.is_empty() => self.jump_search(1),
            Char('N') if !self.search_query.is_empty() => self.jump_search(-1),
            Char('n') => self.jump_hunk_count(1),
            Char('p') | Char('N') => self.jump_hunk_count(-1),
            Char('v') => {
                self.count_prefix = None;
                self.hscroll = 0;
                self.toggle_view();
            }
            Char('r') => {
                self.count_prefix = None;
                self.reload();
            }
            Enter => {
                self.count_prefix = None;
                self.on_enter();
            }
            _ => self.count_prefix = None,
        }
    }

    fn on_search_key(&mut self, key: KeyEvent) {
        use KeyCode::*;
        match key.code {
            Esc => self.search_active = false,
            Enter => self.search_active = false,
            Down if key.modifiers.is_empty() => self.jump_search(1),
            Up if key.modifiers.is_empty() => self.jump_search(-1),
            Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => self.jump_search(1),
            Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => self.jump_search(-1),
            Backspace => {
                self.search_query.pop();
                self.select_first_search_match();
            }
            Char(c) => {
                self.search_query.push(c);
                self.select_first_search_match();
            }
            _ => {}
        }
    }

    fn start_search(&mut self) {
        self.count_prefix = None;
        self.search_active = true;
        self.search_query.clear();
        self.search_scope = match self.focus {
            Focus::Tree => SearchScope::Tree,
            Focus::Diff => SearchScope::Diff,
        };
    }

    fn push_count_digit(&mut self, c: char) {
        let Some(digit) = c.to_digit(10).map(|d| d as usize) else {
            return;
        };
        if self.count_prefix.is_none() && digit == 0 {
            return;
        }
        self.count_prefix = Some(
            self.count_prefix
                .unwrap_or(0)
                .saturating_mul(10)
                .saturating_add(digit),
        );
    }

    fn take_count(&mut self) -> usize {
        self.count_prefix.take().unwrap_or(1)
    }

    fn step_count(&mut self, dir: isize) {
        let count = self.take_count() as isize;
        self.step(dir * count);
    }

    fn page_count(&mut self, dir: isize) {
        let count = self.take_count() as isize;
        self.page(dir * count);
    }

    fn jump_hunk_count(&mut self, dir: isize) {
        let count = self.take_count() as isize;
        self.jump_hunk(dir * count);
    }

    fn step(&mut self, delta: isize) {
        match self.focus {
            Focus::Tree => self.move_cursor(delta),
            Focus::Diff => self.scroll_by(delta),
        }
    }

    fn page(&mut self, dir: isize) {
        match self.focus {
            Focus::Tree => {
                let step = (self.tree_height.max(2) - 1) as isize;
                self.move_cursor(dir * step);
            }
            Focus::Diff => {
                let step = (self.diff_height.max(2) - 1) as isize;
                self.scroll_by(dir * step);
            }
        }
    }

    fn to_edge(&mut self, bottom: bool) {
        match self.focus {
            Focus::Tree => {
                let last = self.tree_rows.len().saturating_sub(1);
                self.set_cursor(if bottom { last } else { 0 });
            }
            Focus::Diff => {
                // Overshoot is clamped against the pane height during draw.
                self.scroll = if bottom { self.content_len() } else { 0 };
            }
        }
    }

    fn to_line_or_edge(&mut self) {
        match self.count_prefix.take() {
            Some(line) => {
                self.focus = Focus::Diff;
                self.scroll = line
                    .saturating_sub(1)
                    .min(self.content_len().saturating_sub(1));
            }
            None => self.to_edge(false),
        }
    }

    fn move_cursor(&mut self, delta: isize) {
        if self.tree_rows.is_empty() {
            return;
        }
        let last = self.tree_rows.len() as isize - 1;
        let c = (self.cursor as isize + delta).clamp(0, last) as usize;
        self.set_cursor(c);
    }

    fn set_cursor(&mut self, c: usize) {
        if self.tree_rows.is_empty() {
            return;
        }
        self.cursor = c.min(self.tree_rows.len() - 1);
        if let Some(fi) = self.tree_rows[self.cursor].file {
            if self.selected != Some(fi) {
                self.selected = Some(fi);
                self.scroll = 0;
                self.hscroll = 0;
            }
        }
    }

    fn set_cursor_to_row(&mut self, c: usize) {
        self.set_cursor(c);
        if self.tree_height > 0 {
            let h = self.tree_height as usize;
            if self.cursor < self.tree_scroll {
                self.tree_scroll = self.cursor;
            } else if self.cursor >= self.tree_scroll + h {
                self.tree_scroll = self.cursor + 1 - h;
            }
        }
    }

    fn scroll_by(&mut self, delta: isize) {
        let max = self.content_len().saturating_sub(1) as isize;
        self.scroll = (self.scroll as isize + delta).clamp(0, max.max(0)) as usize;
    }

    pub fn content_len(&self) -> usize {
        match (self.selected, self.view) {
            (Some(fi), View::Diff) => self.files[fi].rows.len(),
            (Some(fi), View::Full) => self.files[fi].full_lines().len(),
            _ => 0,
        }
    }

    fn anchors_for(&self, fi: usize) -> Vec<usize> {
        match self.view {
            View::Diff => self.files[fi].anchors.clone(),
            View::Full => self.files[fi].full_anchors(),
        }
    }

    /// Index of the hunk currently at or above the top of the viewport.
    pub fn current_hunk(&self) -> Option<usize> {
        let fi = self.selected?;
        let anchors = self.anchors_for(fi);
        if anchors.is_empty() {
            return None;
        }
        let n = anchors.partition_point(|&a| a <= self.scroll);
        Some(n.saturating_sub(1))
    }

    fn jump_hunk(&mut self, dir: isize) {
        self.focus = Focus::Diff;
        for _ in 0..dir.unsigned_abs().max(1) {
            let moved = if dir > 0 {
                self.jump_hunk_once(1)
            } else {
                self.jump_hunk_once(-1)
            };
            if !moved {
                return;
            }
        }
    }

    fn jump_hunk_once(&mut self, dir: isize) -> bool {
        let Some(fi) = self.selected else {
            return false;
        };
        let cur = self.scroll;
        let anchors = self.anchors_for(fi);
        if dir > 0 {
            if let Some(&a) = anchors.iter().find(|&&a| a > cur) {
                self.scroll = a;
                return true;
            }
            for nfi in fi + 1..self.files.len() {
                if let Some(&a0) = self.anchors_for(nfi).first() {
                    self.select_file(nfi);
                    self.scroll = a0;
                    return true;
                }
            }
        } else {
            if let Some(&a) = anchors.iter().rev().find(|&&a| a < cur) {
                self.scroll = a;
                return true;
            }
            for nfi in (0..fi).rev() {
                if let Some(&al) = self.anchors_for(nfi).last() {
                    self.select_file(nfi);
                    self.scroll = al;
                    return true;
                }
            }
        }
        false
    }

    pub fn mouse_scroll(&mut self, delta: isize) {
        let step = 3;
        match self.focus {
            Focus::Tree => self.move_cursor(delta * step),
            Focus::Diff => self.scroll_by(delta * step),
        }
    }

    pub fn mouse_horizontal_scroll(&mut self, delta: isize) {
        self.horizontal_scroll(delta * 6);
    }

    fn horizontal_scroll(&mut self, delta: isize) {
        self.count_prefix = None;
        self.focus = Focus::Diff;
        self.hscroll = (self.hscroll as isize + delta).max(0) as usize;
    }

    fn select_file(&mut self, fi: usize) {
        self.selected = Some(fi);
        self.scroll = 0;
        self.hscroll = 0;
        if let Some(rc) = self.tree_rows.iter().position(|r| r.file == Some(fi)) {
            self.cursor = rc;
        }
    }

    fn toggle_tree_row(&mut self, row_idx: usize) {
        let Some(row) = self.tree_rows.get(row_idx) else {
            return;
        };
        if row.file.is_some() {
            self.set_cursor_to_row(row_idx);
            return;
        }
        self.tree.toggle(&row.node_path.clone());
        self.tree_rows = self.tree.flatten();
        self.cursor = self.cursor.min(self.tree_rows.len().saturating_sub(1));
    }

    pub fn on_mouse(&mut self, event: MouseEvent) {
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => self.mouse_down(event.column, event.row),
            MouseEventKind::Drag(MouseButton::Left) => self.mouse_drag(event.column),
            MouseEventKind::Up(MouseButton::Left) => self.resizing_tree = false,
            MouseEventKind::ScrollDown => self.mouse_scroll_at(event.column, event.row, 1),
            MouseEventKind::ScrollUp => self.mouse_scroll_at(event.column, event.row, -1),
            MouseEventKind::ScrollRight => self.mouse_horizontal_scroll(1),
            MouseEventKind::ScrollLeft => self.mouse_horizontal_scroll(-1),
            _ => {}
        }
    }

    fn mouse_down(&mut self, col: u16, row: u16) {
        if self.is_on_split(col) {
            self.resizing_tree = true;
            return;
        }
        if contains(self.tree_area, col, row) {
            self.focus = Focus::Tree;
            if let Some(row_idx) = self.tree_row_at(row) {
                self.toggle_tree_row(row_idx);
            }
        } else if contains(self.diff_area, col, row) {
            self.focus = Focus::Diff;
        }
    }

    fn mouse_drag(&mut self, col: u16) {
        if self.resizing_tree {
            self.tree_width = col.clamp(18, 80);
        }
    }

    fn mouse_scroll_at(&mut self, col: u16, row: u16, delta: isize) {
        if contains(self.tree_area, col, row) {
            self.focus = Focus::Tree;
        } else if contains(self.diff_area, col, row) {
            self.focus = Focus::Diff;
        }
        self.mouse_scroll(delta);
    }

    fn is_on_split(&self, col: u16) -> bool {
        let split = self.tree_area.x.saturating_add(self.tree_area.width);
        col.abs_diff(split) <= 1
    }

    fn tree_row_at(&self, row: u16) -> Option<usize> {
        if !contains(self.tree_inner, self.tree_inner.x, row) || row < self.tree_inner.y {
            return None;
        }
        let visible = usize::from(row - self.tree_inner.y);
        let idx = self.tree_scroll + visible;
        (idx < self.tree_rows.len()).then_some(idx)
    }

    /// Switch diff/full view, keeping the viewport on the same hunk.
    fn toggle_view(&mut self) {
        let cur_hunk = self.current_hunk();
        self.view = match self.view {
            View::Diff => View::Full,
            View::Full => View::Diff,
        };
        match (self.selected, cur_hunk) {
            (Some(fi), Some(k)) => {
                let anchors = self.anchors_for(fi);
                self.scroll = anchors.get(k).copied().unwrap_or(0);
            }
            _ => self.scroll = 0,
        }
    }

    fn on_enter(&mut self) {
        if self.focus != Focus::Tree {
            return;
        }
        let Some(row) = self.tree_rows.get(self.cursor) else {
            return;
        };
        if row.file.is_some() {
            self.focus = Focus::Diff;
        } else {
            // Toggling only adds/removes rows below the cursor, so the
            // cursor index stays on the same node.
            self.tree.toggle(&row.node_path.clone());
            self.tree_rows = self.tree.flatten();
            self.cursor = self.cursor.min(self.tree_rows.len().saturating_sub(1));
        }
    }

    fn jump_folder(&mut self, dir: isize, top_level: bool) {
        let count = self.take_count();
        for _ in 0..count {
            let Some(next) = self.next_folder_row(dir, top_level) else {
                return;
            };
            self.set_cursor_to_row(next);
        }
    }

    fn next_folder_row(&self, dir: isize, top_level: bool) -> Option<usize> {
        let is_folder = |row: &TreeRow| row.file.is_none() && (!top_level || row.depth == 0);
        if dir > 0 {
            self.tree_rows
                .iter()
                .enumerate()
                .skip(self.cursor + 1)
                .find_map(|(idx, row)| is_folder(row).then_some(idx))
        } else {
            self.tree_rows
                .iter()
                .enumerate()
                .take(self.cursor)
                .rev()
                .find_map(|(idx, row)| is_folder(row).then_some(idx))
        }
    }

    fn select_first_search_match(&mut self) {
        if self.search_query.is_empty() {
            return;
        }
        match self.search_scope {
            SearchScope::Tree => {
                if let Some(idx) = self.find_tree_search_match(self.cursor, 1, true) {
                    self.set_cursor_to_row(idx);
                }
            }
            SearchScope::Diff => {
                if let Some(pos) = self.find_diff_search_match(self.scroll, 1, true) {
                    self.focus = Focus::Diff;
                    self.scroll = pos;
                }
            }
        }
    }

    fn jump_search(&mut self, dir: isize) {
        let count = self.take_count();
        for _ in 0..count {
            match self.search_scope {
                SearchScope::Tree => {
                    let start = self.cursor;
                    let Some(idx) = self.find_tree_search_match(start, dir, false) else {
                        return;
                    };
                    self.set_cursor_to_row(idx);
                }
                SearchScope::Diff => {
                    let start = self.scroll;
                    let Some(pos) = self.find_diff_search_match(start, dir, false) else {
                        return;
                    };
                    self.focus = Focus::Diff;
                    self.scroll = pos;
                }
            }
        }
    }

    fn find_tree_search_match(
        &self,
        start: usize,
        dir: isize,
        include_start: bool,
    ) -> Option<usize> {
        if self.search_query.is_empty() || self.tree_rows.is_empty() {
            return None;
        }
        let total = self.tree_rows.len();
        let mut idx = if include_start {
            start
        } else if dir > 0 {
            (start + 1) % total
        } else {
            start.checked_sub(1).unwrap_or(total - 1)
        };
        for _ in 0..total {
            if self.row_matches_search(idx) {
                return Some(idx);
            }
            idx = if dir > 0 {
                (idx + 1) % total
            } else {
                idx.checked_sub(1).unwrap_or(total - 1)
            };
        }
        None
    }

    fn find_diff_search_match(
        &self,
        start: usize,
        dir: isize,
        include_start: bool,
    ) -> Option<usize> {
        let Some(fi) = self.selected else {
            return None;
        };
        let positions = self.diff_search_positions(fi);
        if positions.is_empty() {
            return None;
        }
        let query = self.search_query.to_ascii_lowercase();
        let matches: Vec<usize> = positions
            .into_iter()
            .filter(|(_, text)| text.to_ascii_lowercase().contains(&query))
            .map(|(pos, _)| pos)
            .collect();
        if matches.is_empty() {
            return None;
        }
        if include_start {
            if let Some(pos) = matches.iter().copied().find(|&pos| pos >= start) {
                return Some(pos);
            }
            return matches.first().copied();
        }
        if dir > 0 {
            matches
                .iter()
                .copied()
                .find(|&pos| pos > start)
                .or_else(|| matches.first().copied())
        } else {
            matches
                .iter()
                .rev()
                .copied()
                .find(|&pos| pos < start)
                .or_else(|| matches.last().copied())
        }
    }

    fn diff_search_positions(&self, fi: usize) -> Vec<(usize, String)> {
        let Some(f) = self.files.get(fi) else {
            return Vec::new();
        };
        match self.view {
            View::Full => f
                .full_lines()
                .iter()
                .enumerate()
                .map(|(idx, line)| (idx, line.clone()))
                .collect(),
            View::Diff => f
                .rows
                .iter()
                .enumerate()
                .filter_map(|(idx, row)| match row {
                    crate::model::Row::HunkHeader(_) => None,
                    crate::model::Row::Line { old, new } => {
                        let mut text = String::new();
                        if let Some(ln) = old.line_no {
                            if let Some(line) = f.old_lines.get(ln.saturating_sub(1)) {
                                text.push_str(line);
                            }
                        }
                        if let Some(ln) = new.line_no {
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            if let Some(line) = f.new_lines.get(ln.saturating_sub(1)) {
                                text.push_str(line);
                            }
                        }
                        (!text.is_empty()).then_some((idx, text))
                    }
                })
                .collect(),
        }
    }

    pub fn search_status(&self) -> Option<SearchStatus> {
        if self.search_query.is_empty() {
            return None;
        }
        match self.search_scope {
            SearchScope::Tree => {
                let matches: Vec<usize> = (0..self.tree_rows.len())
                    .filter(|&idx| self.row_matches_search(idx))
                    .collect();
                let total = matches.len();
                let current = matches
                    .iter()
                    .position(|&idx| idx == self.cursor)
                    .map(|idx| idx + 1)
                    .unwrap_or(0);
                Some(SearchStatus {
                    scope: SearchScope::Tree,
                    current,
                    total,
                })
            }
            SearchScope::Diff => {
                let Some(fi) = self.selected else {
                    return Some(SearchStatus {
                        scope: SearchScope::Diff,
                        current: 0,
                        total: 0,
                    });
                };
                let query = self.search_query.to_ascii_lowercase();
                let matches: Vec<usize> = self
                    .diff_search_positions(fi)
                    .into_iter()
                    .filter(|(_, text)| text.to_ascii_lowercase().contains(&query))
                    .map(|(pos, _)| pos)
                    .collect();
                let total = matches.len();
                let current = matches
                    .iter()
                    .position(|&pos| pos == self.scroll)
                    .map(|idx| idx + 1)
                    .unwrap_or(0);
                Some(SearchStatus {
                    scope: SearchScope::Diff,
                    current,
                    total,
                })
            }
        }
    }

    fn row_matches_search(&self, idx: usize) -> bool {
        let query = self.search_query.to_ascii_lowercase();
        let Some(row) = self.tree_rows.get(idx) else {
            return false;
        };
        if row.label.to_ascii_lowercase().contains(&query) {
            return true;
        }
        row.file.and_then(|fi| self.files.get(fi)).is_some_and(|f| {
            f.path
                .to_string_lossy()
                .to_ascii_lowercase()
                .contains(&query)
        })
    }

    /// Ensure both sides of a file are syntax-highlighted in the cache.
    pub fn ensure_hl(&mut self, fi: usize) {
        for old in [true, false] {
            if self.hl_cache.contains_key(&(fi, old)) {
                continue;
            }
            let f = &self.files[fi];
            let lines = if old { &f.old_lines } else { &f.new_lines };
            let styled = self.hl.highlight(&f.path, lines);
            self.hl_cache.insert((fi, old), styled);
        }
    }

    /// Ensure highlight + changed-line marks needed by the full-file view.
    pub fn ensure_full(&mut self, fi: usize) {
        self.ensure_hl(fi);
        if !self.full_marks.contains_key(&fi) {
            let marks = self.files[fi].changed_full_lines();
            self.full_marks.insert(fi, marks);
        }
    }
}

fn contains(area: Rect, col: u16, row: u16) -> bool {
    col >= area.x
        && row >= area.y
        && col < area.x.saturating_add(area.width)
        && row < area.y.saturating_add(area.height)
}

fn collect_files(root: &PathBuf, source: &Source) -> Result<Vec<FileEntry>> {
    match source {
        Source::Git { base } => git::collect(root, base),
        Source::Directory => git::collect_directory(root),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FileEntry, FileStatus, Hunk};

    #[test]
    fn numeric_prefix_moves_diff_by_n_lines() {
        let mut app = test_app();
        app.focus = Focus::Diff;

        press_chars(&mut app, "12j");

        assert_eq!(app.scroll, 12);

        press_chars(&mut app, "5k");

        assert_eq!(app.scroll, 7);
    }

    #[test]
    fn numeric_prefix_moves_tree_by_n_rows() {
        let mut app = test_app();
        app.focus = Focus::Tree;

        press_chars(&mut app, "2j");

        assert_eq!(app.cursor, 2);
    }

    #[test]
    fn numeric_prefix_g_jumps_to_line() {
        let mut app = test_app();
        app.focus = Focus::Diff;

        press_chars(&mut app, "21g");

        assert_eq!(app.scroll, 20);
    }

    #[test]
    fn horizontal_keys_scroll_sideways() {
        let mut app = test_app();

        press_chars(&mut app, "L");

        assert_eq!(app.focus, Focus::Diff);
        assert_eq!(app.hscroll, 24);

        press_chars(&mut app, "H");

        assert_eq!(app.hscroll, 0);
    }

    #[test]
    fn mouse_horizontal_scroll_moves_sideways() {
        let mut app = test_app();

        app.mouse_horizontal_scroll(2);

        assert_eq!(app.focus, Focus::Diff);
        assert_eq!(app.hscroll, 12);

        app.mouse_horizontal_scroll(-4);

        assert_eq!(app.hscroll, 0);
    }

    #[test]
    fn folder_shortcuts_jump_between_visible_folders() {
        let mut app = test_app();

        press_chars(&mut app, "]");

        assert_eq!(app.tree_rows[app.cursor].label, "tests");

        press_chars(&mut app, "[");

        assert_eq!(app.tree_rows[app.cursor].label, "src");
    }

    #[test]
    fn search_jumps_to_matching_tree_rows() {
        let mut app = test_app();

        press_chars(&mut app, "/alp");

        assert!(app.search_active);
        assert_eq!(app.search_query, "alp");
        assert_eq!(app.tree_rows[app.cursor].label, "alpha.rs");

        press_chars(&mut app, "\n");

        assert!(!app.search_active);
    }

    #[test]
    fn search_scope_follows_diff_focus_and_searches_file_content() {
        let mut app = test_app();
        app.focus = Focus::Diff;

        press_chars(&mut app, "/line 20");

        assert!(app.search_active);
        assert_eq!(app.search_scope, SearchScope::Diff);
        assert_eq!(app.scroll, 20);

        press_chars(&mut app, "\n");
        press_chars(&mut app, "N");

        assert_eq!(app.scroll, 20);
    }

    #[test]
    fn search_mode_arrow_keys_jump_between_results() {
        let mut app = test_app();
        app.focus = Focus::Diff;

        press_chars(&mut app, "/needle");

        assert_eq!(app.scroll, 20);

        app.on_key(KeyEvent::from(KeyCode::Down));

        assert_eq!(app.scroll, 30);

        app.on_key(KeyEvent::from(KeyCode::Up));

        assert_eq!(app.scroll, 20);
    }

    #[test]
    fn normal_mode_n_keys_jump_between_search_results() {
        let mut app = test_app();
        app.focus = Focus::Diff;

        press_chars(&mut app, "/needle\nn");

        assert_eq!(app.scroll, 30);

        press_chars(&mut app, "N");

        assert_eq!(app.scroll, 20);
    }

    #[test]
    fn search_status_reports_current_match() {
        let mut app = test_app();
        app.focus = Focus::Diff;

        press_chars(&mut app, "/needle\nn");

        assert_eq!(
            app.search_status(),
            Some(SearchStatus {
                scope: SearchScope::Diff,
                current: 2,
                total: 2,
            })
        );
    }

    #[test]
    fn question_mark_toggles_help_overlay() {
        let mut app = test_app();

        press_chars(&mut app, "?");

        assert!(app.help);

        press_chars(&mut app, "?");

        assert!(!app.help);
    }

    #[test]
    fn mouse_click_selects_tree_file_and_diff_focus() {
        let mut app = test_app();
        app.tree_area = Rect::new(0, 0, 30, 10);
        app.tree_inner = Rect::new(1, 1, 28, 8);
        app.diff_area = Rect::new(30, 0, 70, 10);

        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 2,
            row: 2,
            modifiers: KeyModifiers::empty(),
        });

        assert_eq!(app.focus, Focus::Tree);
        assert_eq!(app.tree_rows[app.cursor].label, "alpha.rs");

        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 40,
            row: 2,
            modifiers: KeyModifiers::empty(),
        });

        assert_eq!(app.focus, Focus::Diff);
    }

    #[test]
    fn mouse_drag_on_split_resizes_tree() {
        let mut app = test_app();
        app.tree_area = Rect::new(0, 0, 30, 10);

        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 30,
            row: 3,
            modifiers: KeyModifiers::empty(),
        });
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 45,
            row: 3,
            modifiers: KeyModifiers::empty(),
        });

        assert_eq!(app.tree_width, 45);
    }

    fn press_chars(app: &mut App, chars: &str) {
        for c in chars.chars() {
            let code = match c {
                '\n' => KeyCode::Enter,
                _ => KeyCode::Char(c),
            };
            app.on_key(KeyEvent::from(code));
        }
    }

    fn test_app() -> App {
        let paths = ["src/alpha.rs", "src/beta.rs", "tests/gamma.rs"];
        let mut files = Vec::new();
        for path in paths {
            let lines: Vec<String> = (1..=50)
                .map(|n| {
                    if n == 20 || n == 30 {
                        format!("line {n} needle")
                    } else {
                        format!("line {n}")
                    }
                })
                .collect();
            let mut entry = FileEntry {
                path: PathBuf::from(path),
                status: FileStatus::Added,
                binary: false,
                hunks: vec![Hunk {
                    old_start: 0,
                    old_count: 0,
                    new_start: 1,
                    new_count: lines.len(),
                    kinds: vec!['+'; lines.len()],
                }],
                old_lines: Vec::new(),
                new_lines: lines,
                rows: Vec::new(),
                anchors: Vec::new(),
                additions: 0,
                deletions: 0,
            };
            entry.finalize();
            files.push(entry);
        }
        let tree = tree::build(&files);
        let tree_rows = tree.flatten();
        App {
            root: PathBuf::from("."),
            source: Source::Directory,
            files,
            tree,
            tree_rows,
            cursor: 0,
            tree_scroll: 0,
            selected: Some(0),
            focus: Focus::Tree,
            view: View::Diff,
            scroll: 0,
            hscroll: 0,
            diff_height: 24,
            tree_height: 24,
            tree_width: 32,
            tree_area: Rect::default(),
            tree_inner: Rect::default(),
            diff_area: Rect::default(),
            resizing_tree: false,
            hl: Highlighter::new(),
            hl_cache: HashMap::new(),
            full_marks: HashMap::new(),
            count_prefix: None,
            search_active: false,
            search_query: String::new(),
            search_scope: SearchScope::Tree,
            help: false,
            quit: false,
        }
    }
}
