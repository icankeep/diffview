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
use crate::model::{FileEntry, LineKind, Row};
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
    /// Row/line index of the change block we last jumped to (n/p), in the
    /// current view's coordinate space. Drives the on-screen marker; retained
    /// through manual scrolling and cleared by file switches or non-change jumps.
    pub active_change: Option<usize>,
    /// Horizontal character offset per pane: [0] = old/left, [1] = new/right.
    /// Single-pane views (added file, full view) use the new-side slot.
    pub hscroll: [usize; 2],
    /// Diff side targeted by keyboard horizontal scroll: 0 = old, 1 = new.
    pub diff_side: usize,
    /// Inner heights recorded during the last draw, for paging and clamping.
    pub diff_height: u16,
    pub tree_height: u16,
    pub tree_width: u16,
    /// When true the file-tree pane is fully hidden and the diff fills the row.
    pub tree_collapsed: bool,
    /// Clickable collapse/expand icon, recorded during the last draw.
    pub tree_toggle_icon: Rect,
    pub tree_area: Rect,
    pub tree_inner: Rect,
    pub diff_area: Rect,
    /// Percentage of the diff region given to the old (left) pane.
    pub diff_split_pct: u16,
    /// Column of the old/new divider, recorded during the last draw.
    pub diff_split_x: u16,
    pub resizing_tree: bool,
    pub resizing_diff: bool,
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
            active_change: None,
            hscroll: [0, 0],
            diff_side: 1,
            diff_height: 24,
            tree_height: 24,
            tree_width: 32,
            tree_collapsed: false,
            tree_toggle_icon: Rect::default(),
            tree_area: Rect::default(),
            tree_inner: Rect::default(),
            diff_area: Rect::default(),
            diff_split_pct: 50,
            diff_split_x: 0,
            resizing_tree: false,
            resizing_diff: false,
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
        self.active_change = None;
        self.hscroll = [0, 0];
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
                Char('b') => self.toggle_tree_collapsed(),
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
                if self.tree_collapsed {
                    self.tree_collapsed = false;
                    self.focus = Focus::Tree;
                } else {
                    self.focus = match self.focus {
                        Focus::Tree => Focus::Diff,
                        Focus::Diff => Focus::Tree,
                    }
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
                self.tree_collapsed = false;
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
            Char('n') => self.jump_change_count(1),
            Char('p') | Char('N') => self.jump_change_count(-1),
            Char('v') => {
                self.count_prefix = None;
                self.hscroll = [0, 0];
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

    fn jump_change_count(&mut self, dir: isize) {
        let count = self.take_count() as isize;
        self.jump_change(dir * count);
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
                self.active_change = None;
                self.hscroll = [0, 0];
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
        // The jump marker persists through manual scrolling: it stays the
        // "current change" so n/p remain reliable and the highlight is stable.
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
            View::Diff => self.files[fi].change_anchors.clone(),
            View::Full => self.files[fi].full_change_anchors(),
        }
    }

    /// Index of the active change block: the one we last jumped to if it still
    /// stands, otherwise the block at or above the top of the viewport.
    pub fn current_change(&self) -> Option<usize> {
        let fi = self.selected?;
        let anchors = self.anchors_for(fi);
        if anchors.is_empty() {
            return None;
        }
        if let Some(r) = self.active_change {
            if let Some(i) = anchors.iter().position(|&a| a == r) {
                return Some(i);
            }
        }
        let n = anchors.partition_point(|&a| a <= self.scroll);
        Some(n.saturating_sub(1))
    }

    /// Row/line range `[start, end)` of the active change block in the current
    /// view, for rendering the jump marker. `None` when nothing is targeted.
    pub fn active_block_range(&self) -> Option<(usize, usize)> {
        let fi = self.selected?;
        let start = self.active_change?;
        let f = self.files.get(fi)?;
        match self.view {
            View::Diff => {
                let rows = &f.rows;
                let mut end = start;
                while end < rows.len() && is_change_row(&rows[end]) {
                    end += 1;
                }
                Some((start, end.max(start + 1)))
            }
            View::Full => {
                let total = f.full_lines().len();
                let owned;
                let changed = match self.full_marks.get(&fi) {
                    Some(m) => m,
                    None => {
                        owned = f.changed_full_lines();
                        &owned
                    }
                };
                let mut end = start;
                while end < total && changed.contains(&(end + 1)) {
                    end += 1;
                }
                Some((start, end.max(start + 1)))
            }
        }
    }

    /// Number of change blocks in the selected file for the current view.
    pub fn change_count(&self) -> usize {
        self.selected
            .map(|fi| self.anchors_for(fi).len())
            .unwrap_or(0)
    }

    fn jump_change(&mut self, dir: isize) {
        self.focus = Focus::Diff;
        for _ in 0..dir.unsigned_abs().max(1) {
            let moved = if dir > 0 {
                self.jump_change_once(1)
            } else {
                self.jump_change_once(-1)
            };
            if !moved {
                return;
            }
        }
    }

    /// Largest scroll offset the draw pass will not clamp away, given the last
    /// recorded pane height. Used to keep a centered block from overscrolling.
    fn max_scroll_for(&self, fi: usize) -> usize {
        let len = match self.view {
            View::Diff => self.files[fi].rows.len(),
            View::Full => self.files[fi].full_lines().len(),
        };
        len.saturating_sub(self.diff_height.max(1) as usize)
    }

    /// Scroll offset that vertically centers `row` in the viewport, clamped so
    /// the draw pass renders it where we expect.
    fn center_scroll(&self, fi: usize, row: usize) -> usize {
        let h = self.diff_height.max(1) as usize;
        row.saturating_sub(h / 2).min(self.max_scroll_for(fi))
    }

    /// Mark `row` as the active change block and center the viewport on it.
    fn focus_change(&mut self, fi: usize, row: usize) {
        self.active_change = Some(row);
        self.scroll = self.center_scroll(fi, row);
    }

    fn jump_change_once(&mut self, dir: isize) -> bool {
        let Some(fi) = self.selected else {
            return false;
        };
        let anchors = self.anchors_for(fi);
        // Where we are now: the marked block if it still holds, otherwise infer
        // from the current scroll position so n/p resume from the viewport.
        let cur_idx = self
            .active_change
            .and_then(|r| anchors.iter().position(|&a| a == r));
        if dir > 0 {
            let next = match cur_idx {
                Some(i) => anchors.get(i + 1).copied(),
                None => anchors.iter().copied().find(|&a| a >= self.scroll),
            };
            if let Some(a) = next {
                self.focus_change(fi, a);
                return true;
            }
            let file_order = self.tree.file_order();
            let Some(order_idx) = file_order.iter().position(|&candidate| candidate == fi) else {
                return false;
            };
            for &nfi in file_order
                .iter()
                .skip(order_idx + 1)
                .chain(file_order.iter().take(order_idx))
            {
                if let Some(&a0) = self.anchors_for(nfi).first() {
                    self.select_file(nfi);
                    self.focus_change(nfi, a0);
                    return true;
                }
            }
        } else {
            let prev = match cur_idx {
                Some(i) => i.checked_sub(1).and_then(|j| anchors.get(j).copied()),
                None => anchors.iter().rev().copied().find(|&a| a < self.scroll),
            };
            if let Some(a) = prev {
                self.focus_change(fi, a);
                return true;
            }
            let file_order = self.tree.file_order();
            let Some(order_idx) = file_order.iter().position(|&candidate| candidate == fi) else {
                return false;
            };
            for &nfi in file_order[..order_idx]
                .iter()
                .rev()
                .chain(file_order[order_idx + 1..].iter().rev())
            {
                if let Some(&al) = self.anchors_for(nfi).last() {
                    self.select_file(nfi);
                    self.focus_change(nfi, al);
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

    pub fn mouse_horizontal_scroll(&mut self, col: u16, delta: isize) {
        // Scroll the pane under the cursor, independently of the other pane.
        if let Some(side) = self.side_at_col(col) {
            self.diff_side = side;
        }
        self.horizontal_scroll(delta * 6);
    }

    /// Which diff pane a column falls in: 0 = old/left, 1 = new/right.
    /// `None` when only one pane is visible (no old/new divider).
    fn side_at_col(&self, col: u16) -> Option<usize> {
        (self.diff_split_x > 0).then(|| usize::from(col >= self.diff_split_x))
    }

    fn horizontal_scroll(&mut self, delta: isize) {
        self.count_prefix = None;
        self.focus = Focus::Diff;
        let cur = self.hscroll[self.diff_side] as isize;
        self.hscroll[self.diff_side] = (cur + delta).max(0) as usize;
    }

    fn select_file(&mut self, fi: usize) {
        self.selected = Some(fi);
        self.scroll = 0;
        self.active_change = None;
        self.hscroll = [0, 0];
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
            MouseEventKind::Up(MouseButton::Left) => {
                self.resizing_tree = false;
                self.resizing_diff = false;
            }
            MouseEventKind::ScrollDown => self.mouse_scroll_at(event.column, event.row, 1),
            MouseEventKind::ScrollUp => self.mouse_scroll_at(event.column, event.row, -1),
            MouseEventKind::ScrollRight => self.mouse_horizontal_scroll(event.column, 1),
            MouseEventKind::ScrollLeft => self.mouse_horizontal_scroll(event.column, -1),
            _ => {}
        }
    }

    fn mouse_down(&mut self, col: u16, row: u16) {
        if contains(self.tree_toggle_icon, col, row) {
            self.toggle_tree_collapsed();
            return;
        }
        if self.is_on_split(col, row) {
            self.resizing_tree = true;
            return;
        }
        if self.is_on_diff_split(col, row) {
            self.resizing_diff = true;
            return;
        }
        if contains(self.tree_area, col, row) {
            self.focus = Focus::Tree;
            if let Some(row_idx) = self.tree_row_at(row) {
                self.toggle_tree_row(row_idx);
            }
        } else if contains(self.diff_area, col, row) {
            self.focus = Focus::Diff;
            if let Some(side) = self.side_at_col(col) {
                self.diff_side = side;
            }
        }
    }

    fn mouse_drag(&mut self, col: u16) {
        if self.resizing_tree {
            self.tree_width = col.clamp(18, 80);
        } else if self.resizing_diff {
            self.set_diff_split(col);
        }
    }

    fn set_diff_split(&mut self, col: u16) {
        let area = self.diff_area;
        if area.width == 0 {
            return;
        }
        let rel = col.saturating_sub(area.x).min(area.width);
        // Round to match the percent-to-column conversion used when rendering,
        // so the divider tracks the cursor instead of lagging by a column.
        let pct = ((rel as u32 * 100 + area.width as u32 / 2) / area.width as u32) as u16;
        self.diff_split_pct = pct.clamp(10, 90);
    }

    fn mouse_scroll_at(&mut self, col: u16, row: u16, delta: isize) {
        if contains(self.tree_area, col, row) {
            self.focus = Focus::Tree;
        } else if contains(self.diff_area, col, row) {
            self.focus = Focus::Diff;
        }
        self.mouse_scroll(delta);
    }

    fn toggle_tree_collapsed(&mut self) {
        self.count_prefix = None;
        // Cancel any in-flight drag so it cannot act on the changed geometry.
        self.resizing_tree = false;
        self.resizing_diff = false;
        self.tree_collapsed = !self.tree_collapsed;
        self.focus = if self.tree_collapsed {
            Focus::Diff
        } else {
            Focus::Tree
        };
    }

    fn is_on_split(&self, col: u16, row: u16) -> bool {
        if self.tree_collapsed || !within_rows(self.tree_area, row) {
            return false;
        }
        let split = self.tree_area.x.saturating_add(self.tree_area.width);
        col.abs_diff(split) <= 1
    }

    fn is_on_diff_split(&self, col: u16, row: u16) -> bool {
        // The divider occupies the two adjacent border columns just left of the
        // new pane; do not grab the new pane's first content column.
        self.diff_split_x > 0
            && within_rows(self.diff_area, row)
            && (col == self.diff_split_x || col + 1 == self.diff_split_x)
    }

    fn tree_row_at(&self, row: u16) -> Option<usize> {
        if !contains(self.tree_inner, self.tree_inner.x, row) || row < self.tree_inner.y {
            return None;
        }
        let visible = usize::from(row - self.tree_inner.y);
        let idx = self.tree_scroll + visible;
        (idx < self.tree_rows.len()).then_some(idx)
    }

    /// Switch diff/full view, keeping the viewport on the same change block.
    /// A jump marker carries over (re-centered); a plain viewport does not gain
    /// one, so toggling after a manual scroll doesn't resurrect a marker.
    fn toggle_view(&mut self) {
        let had_marker = self.active_change.is_some();
        let cur_change = self.current_change();
        self.view = match self.view {
            View::Diff => View::Full,
            View::Full => View::Diff,
        };
        let target = self
            .selected
            .zip(cur_change)
            .and_then(|(fi, k)| self.anchors_for(fi).get(k).copied().map(|a| (fi, a)));
        match target {
            Some((fi, a)) if had_marker => self.focus_change(fi, a),
            Some((fi, a)) => {
                self.active_change = None;
                self.scroll = a.min(self.max_scroll_for(fi));
            }
            None => {
                self.active_change = None;
                self.scroll = 0;
            }
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
                    self.active_change = None;
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
                    self.active_change = None;
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

/// Whether a diff row carries a change (an add/del on either side), as opposed
/// to a pure context row or a hunk header.
fn is_change_row(row: &Row) -> bool {
    matches!(
        row,
        Row::Line { old, new }
            if !(old.kind == LineKind::Context && new.kind == LineKind::Context)
    )
}

fn within_rows(area: Rect, row: u16) -> bool {
    row >= area.y && row < area.y.saturating_add(area.height)
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
        app.diff_side = 1;

        press_chars(&mut app, "L");

        assert_eq!(app.focus, Focus::Diff);
        assert_eq!(app.hscroll[1], 24);

        press_chars(&mut app, "H");

        assert_eq!(app.hscroll[1], 0);
    }

    #[test]
    fn mouse_horizontal_scroll_moves_sideways() {
        let mut app = test_app();
        app.diff_side = 1;

        app.mouse_horizontal_scroll(0, 2);

        assert_eq!(app.focus, Focus::Diff);
        assert_eq!(app.hscroll[1], 12);

        app.mouse_horizontal_scroll(0, -4);

        assert_eq!(app.hscroll[1], 0);
    }

    #[test]
    fn diff_panes_scroll_horizontally_independently() {
        let mut app = test_app();
        // Two panes visible, divider at column 40.
        app.diff_split_x = 40;

        // Scroll over the old (left) pane.
        app.mouse_horizontal_scroll(10, 3);
        assert_eq!(app.diff_side, 0);
        assert_eq!(app.hscroll[0], 18);
        assert_eq!(app.hscroll[1], 0);

        // Scroll over the new (right) pane: left pane stays put.
        app.mouse_horizontal_scroll(60, 1);
        assert_eq!(app.diff_side, 1);
        assert_eq!(app.hscroll[0], 18);
        assert_eq!(app.hscroll[1], 6);
    }

    #[test]
    fn n_stops_at_each_change_block_within_a_hunk() {
        let mut app = blocks_app();
        app.focus = Focus::Diff;

        // rows: [header, +, ctx, ctx, +, ctx, ctx] -> blocks at rows 1 and 4.
        assert_eq!(app.files[0].change_anchors, vec![1, 4]);

        press_chars(&mut app, "n");
        assert_eq!(app.active_change, Some(1));
        assert_eq!(app.current_change(), Some(0));

        press_chars(&mut app, "n");
        assert_eq!(app.active_change, Some(4));
        assert_eq!(app.current_change(), Some(1));

        // No further block in this single-file fixture.
        press_chars(&mut app, "n");
        assert_eq!(app.active_change, Some(4));

        press_chars(&mut app, "p");
        assert_eq!(app.active_change, Some(1));

        press_chars(&mut app, "N");
        assert_eq!(app.active_change, Some(1));
    }

    #[test]
    fn jumping_centers_the_target_block_in_the_viewport() {
        let mut app = blocks_app();
        app.focus = Focus::Diff;

        // Viewport height 2, so a half-height of 1 is subtracted to center.
        // Block at row 4 -> scroll 3 (within max scroll 5).
        press_chars(&mut app, "nn");
        assert_eq!(app.active_change, Some(4));
        assert_eq!(app.scroll, 3);
    }

    #[test]
    fn the_active_marker_spans_the_whole_change_block() {
        let mut app = blocks_app();
        app.focus = Focus::Diff;

        // First block is a single added row at index 1.
        press_chars(&mut app, "n");
        assert_eq!(app.active_block_range(), Some((1, 2)));

        // The marker persists through manual scrolling so the highlight is
        // stable and n/p stay anchored to the current change.
        press_chars(&mut app, "j");
        assert_eq!(app.active_change, Some(1));
        assert_eq!(app.active_block_range(), Some((1, 2)));
    }

    #[test]
    fn n_crosses_out_of_a_file_that_fits_the_viewport() {
        // Two short modified files that each fit entirely on screen (max scroll 0).
        // Navigation must not get stuck re-selecting the first block.
        let mut app = app_from_files(vec![fits_viewport_file("a.rs"), fits_viewport_file("b.rs")]);
        app.focus = Focus::Diff;
        app.diff_height = 24; // both files are far shorter than the viewport

        // First stop is file a's own block; advancing again crosses into file b
        // (it must not get stuck re-selecting the first block).
        press_chars(&mut app, "n");
        assert_eq!(app.selected, Some(0));
        press_chars(&mut app, "n");
        assert_eq!(app.selected, Some(1));

        // Reverse crosses back into file a.
        press_chars(&mut app, "p");
        assert_eq!(app.selected, Some(0));
    }

    #[test]
    fn n_marks_a_change_at_line_one_in_full_view_before_crossing_files() {
        let mut app = app_from_files(vec![fits_viewport_file("a.rs"), fits_viewport_file("b.rs")]);
        app.focus = Focus::Diff;
        app.view = View::Full;
        app.diff_height = 24;

        // Full-file anchors are line indices, so a change on line one is at 0.
        // The first n must select that block instead of skipping to file b.
        assert_eq!(app.files[0].full_change_anchors(), vec![0]);
        press_chars(&mut app, "n");
        assert_eq!(app.selected, Some(0));
        assert_eq!(app.active_change, Some(0));

        press_chars(&mut app, "n");
        assert_eq!(app.selected, Some(1));
        assert_eq!(app.active_change, Some(0));
    }

    #[test]
    fn stray_wheel_event_does_not_strand_change_navigation() {
        // A viewport-fitting file pins scroll at 0, so navigation cannot rely on
        // scroll to know which block is current. A wheel event (terminals emit
        // these around keypresses) must not wipe the marker and trap us in file a.
        let mut app = app_from_files(vec![fits_viewport_file("a.rs"), fits_viewport_file("b.rs")]);
        app.focus = Focus::Diff;
        app.diff_height = 24;

        press_chars(&mut app, "n");
        assert_eq!(app.selected, Some(0));
        assert_eq!(app.active_change, Some(1));

        // Wheel up: scroll is already clamped at 0, so it does not move, but the
        // marker must survive.
        app.mouse_scroll(-1);
        assert_eq!(app.active_change, Some(1));

        // n still advances across the file boundary.
        press_chars(&mut app, "n");
        assert_eq!(app.selected, Some(1));
    }

    #[test]
    fn n_crosses_into_the_next_file() {
        let mut app = test_app();
        app.focus = Focus::Diff;

        // Each fixture file is one addition block anchored at row 1.
        press_chars(&mut app, "n");
        assert_eq!(app.selected, Some(0));
        assert_eq!(app.active_change, Some(1));

        press_chars(&mut app, "n");
        assert_eq!(app.selected, Some(1));
        assert_eq!(app.active_change, Some(1));
    }

    #[test]
    fn n_wraps_from_the_last_file_to_the_first_file() {
        let mut app = test_app();
        app.focus = Focus::Diff;
        let last = app.files.len() - 1;
        app.select_file(last);

        press_chars(&mut app, "n");
        assert_eq!(app.selected, Some(last));

        press_chars(&mut app, "n");
        assert_eq!(app.selected, Some(0));
        assert_eq!(app.active_change, Some(1));
    }

    #[test]
    fn p_wraps_from_the_first_file_to_the_last_file() {
        let mut app = test_app();
        app.focus = Focus::Diff;
        press_chars(&mut app, "n");
        assert_eq!(app.selected, Some(0));

        press_chars(&mut app, "p");
        assert_eq!(app.selected, Some(app.files.len() - 1));
        assert_eq!(app.active_change, Some(1));
    }

    #[test]
    fn change_navigation_follows_tree_file_order() {
        // Flat path order is AGENTS, app, bin, ui. The tree sorts directories
        // before files, so its leaf order is bin, app, ui, AGENTS.
        let mut app = app_from_files(vec![
            fits_viewport_file("AGENTS.md"),
            fits_viewport_file("src/app.rs"),
            fits_viewport_file("src/bin/diffview-tui.rs"),
            fits_viewport_file("src/ui.rs"),
        ]);
        app.focus = Focus::Diff;
        app.select_file(2);

        // Mark bin, then walk forward in tree order and wrap.
        press_chars(&mut app, "n");
        for expected in [1, 3, 0, 2] {
            press_chars(&mut app, "n");
            assert_eq!(app.selected, Some(expected));
        }

        // Reverse follows the same order back to the tree's last file.
        press_chars(&mut app, "p");
        assert_eq!(app.selected, Some(0));
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

    #[test]
    fn mouse_drag_on_diff_split_resizes_diff_panes() {
        let mut app = test_app();
        // Diff region spans columns 30..100; divider sits at column 65.
        app.diff_area = Rect::new(30, 0, 70, 10);
        app.diff_split_x = 65;

        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 65,
            row: 3,
            modifiers: KeyModifiers::empty(),
        });
        assert!(app.resizing_diff);

        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 44,
            row: 3,
            modifiers: KeyModifiers::empty(),
        });

        // 44 is 14 columns into a 70-wide region -> 20%.
        assert_eq!(app.diff_split_pct, 20);

        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 44,
            row: 3,
            modifiers: KeyModifiers::empty(),
        });
        assert!(!app.resizing_diff);
    }

    #[test]
    fn ctrl_b_toggles_tree_collapse() {
        let mut app = test_app();

        app.on_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));

        assert!(app.tree_collapsed);
        assert_eq!(app.focus, Focus::Diff);

        app.on_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));

        assert!(!app.tree_collapsed);
        assert_eq!(app.focus, Focus::Tree);
    }

    #[test]
    fn tab_reveals_collapsed_tree() {
        let mut app = test_app();
        app.tree_collapsed = true;
        app.focus = Focus::Diff;

        app.on_key(KeyEvent::from(KeyCode::Tab));

        assert!(!app.tree_collapsed);
        assert_eq!(app.focus, Focus::Tree);
    }

    #[test]
    fn clicking_toggle_icon_collapses_and_expands_tree() {
        let mut app = test_app();
        app.tree_toggle_icon = Rect::new(26, 0, 3, 1);

        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 27,
            row: 0,
            modifiers: KeyModifiers::empty(),
        });

        assert!(app.tree_collapsed);
        assert_eq!(app.focus, Focus::Diff);

        // While collapsed the icon moves to the diff region's corner.
        app.tree_toggle_icon = Rect::new(0, 0, 3, 1);
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: 0,
            modifiers: KeyModifiers::empty(),
        });

        assert!(!app.tree_collapsed);
        assert_eq!(app.focus, Focus::Tree);
    }

    #[test]
    fn click_below_diff_area_does_not_start_resize() {
        let mut app = test_app();
        app.diff_area = Rect::new(30, 0, 70, 10);
        app.diff_split_x = 65;

        // Row 10 is the status bar, outside the diff area's rows.
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 65,
            row: 10,
            modifiers: KeyModifiers::empty(),
        });

        assert!(!app.resizing_diff);
    }

    #[test]
    fn collapsed_tree_ignores_split_drag() {
        let mut app = test_app();
        app.tree_collapsed = true;
        app.tree_area = Rect::new(0, 0, 0, 10);

        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 3,
            modifiers: KeyModifiers::empty(),
        });

        assert!(!app.resizing_tree);
    }

    /// A one-line modified file (`old` -> `new`) whose single change block fits
    /// any realistic viewport, pinning max scroll at 0.
    fn fits_viewport_file(path: &str) -> FileEntry {
        let mut e = FileEntry {
            path: PathBuf::from(path),
            status: FileStatus::Modified,
            binary: false,
            hunks: vec![Hunk {
                old_start: 1,
                old_count: 1,
                new_start: 1,
                new_count: 1,
                kinds: vec!['-', '+'],
            }],
            old_lines: vec!["old".to_string()],
            new_lines: vec!["new".to_string()],
            rows: Vec::new(),
            anchors: Vec::new(),
            change_anchors: Vec::new(),
            additions: 0,
            deletions: 0,
        };
        e.finalize();
        e
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

    /// Single modified file whose one hunk holds two context-separated change
    /// blocks (anchors at rows 1 and 4), with trailing context so the content is
    /// taller than the test viewport and both blocks are top-reachable.
    fn blocks_app() -> App {
        let to_lines = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let mut entry = FileEntry {
            path: PathBuf::from("src/alpha.rs"),
            status: FileStatus::Modified,
            binary: false,
            hunks: vec![Hunk {
                old_start: 1,
                old_count: 4,
                new_start: 1,
                new_count: 6,
                kinds: vec!['+', ' ', ' ', '+', ' ', ' '],
            }],
            old_lines: to_lines(&["ctx 1", "ctx 2", "ctx 3", "ctx 4"]),
            new_lines: to_lines(&["add a", "ctx 1", "ctx 2", "add b", "ctx 3", "ctx 4"]),
            rows: Vec::new(),
            anchors: Vec::new(),
            change_anchors: Vec::new(),
            additions: 0,
            deletions: 0,
        };
        entry.finalize();
        let mut app = app_from_files(vec![entry]);
        // Viewport of 2 rows over 7 rows -> max scroll 5, so anchors 1 and 4 are
        // both reachable as top-aligned positions.
        app.diff_height = 2;
        app
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
                change_anchors: Vec::new(),
                additions: 0,
                deletions: 0,
            };
            entry.finalize();
            files.push(entry);
        }
        app_from_files(files)
    }

    /// Build an `App` around pre-built files without touching git or the disk.
    fn app_from_files(files: Vec<FileEntry>) -> App {
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
            active_change: None,
            hscroll: [0, 0],
            diff_side: 1,
            diff_height: 24,
            tree_height: 24,
            tree_width: 32,
            tree_collapsed: false,
            tree_toggle_icon: Rect::default(),
            tree_area: Rect::default(),
            tree_inner: Rect::default(),
            diff_area: Rect::default(),
            diff_split_pct: 50,
            diff_split_x: 0,
            resizing_tree: false,
            resizing_diff: false,
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
