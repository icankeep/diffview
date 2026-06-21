//! File-change model and the side-by-side row alignment algorithm.

use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
}

impl FileStatus {
    pub fn letter(self) -> char {
        match self {
            Self::Added => 'A',
            Self::Modified => 'M',
            Self::Deleted => 'D',
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Del,
    Add,
    Filler,
}

/// One side (old or new) of a side-by-side row.
#[derive(Debug, Clone)]
pub struct Cell {
    /// 1-based line number into the old/new file text. None for fillers.
    pub line_no: Option<usize>,
    pub kind: LineKind,
    /// Byte ranges within the line that differ from the paired line.
    pub inline: Vec<(usize, usize)>,
}

impl Cell {
    fn filler() -> Self {
        Cell {
            line_no: None,
            kind: LineKind::Filler,
            inline: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Row {
    /// Separator row; index into `FileEntry::hunks`.
    HunkHeader(usize),
    Line {
        old: Cell,
        new: Cell,
    },
}

#[derive(Debug, Clone)]
pub struct Hunk {
    pub old_start: usize,
    pub old_count: usize,
    pub new_start: usize,
    pub new_count: usize,
    /// One of ' ', '-', '+' per body line, in diff order.
    pub kinds: Vec<char>,
}

#[derive(Debug)]
pub struct FileEntry {
    /// Repo-relative path (new path; old path for deletions).
    pub path: PathBuf,
    pub status: FileStatus,
    pub binary: bool,
    pub hunks: Vec<Hunk>,
    pub old_lines: Vec<String>,
    pub new_lines: Vec<String>,
    pub rows: Vec<Row>,
    /// Row index of each hunk header.
    pub anchors: Vec<usize>,
    /// Row index of the first row of each change block, for n/p navigation.
    pub change_anchors: Vec<usize>,
    pub additions: usize,
    pub deletions: usize,
}

impl FileEntry {
    /// Compute rows/anchors/stats once contents and hunks are loaded.
    pub fn finalize(&mut self) {
        self.additions = self
            .hunks
            .iter()
            .flat_map(|h| &h.kinds)
            .filter(|&&k| k == '+')
            .count();
        self.deletions = self
            .hunks
            .iter()
            .flat_map(|h| &h.kinds)
            .filter(|&&k| k == '-')
            .count();
        let (rows, anchors) = build_rows(&self.hunks, &self.old_lines, &self.new_lines);
        self.change_anchors = change_block_anchors(&rows);
        self.rows = rows;
        self.anchors = anchors;
    }

    /// Lines shown in full-file view: the new side, or the old side for deletions.
    pub fn full_lines(&self) -> &[String] {
        if self.status == FileStatus::Deleted {
            &self.old_lines
        } else {
            &self.new_lines
        }
    }

    /// 0-based line indices of hunk starts within `full_lines()`.
    pub fn full_anchors(&self) -> Vec<usize> {
        self.hunks
            .iter()
            .map(|h| {
                let start = if self.status == FileStatus::Deleted {
                    h.old_start
                } else {
                    h.new_start
                };
                start.saturating_sub(1)
            })
            .collect()
    }

    /// 0-based line indices of each change block's start within `full_lines()`.
    pub fn full_change_anchors(&self) -> Vec<usize> {
        // Deletions show the base text with nothing marked changed, so fall back
        // to hunk starts to keep navigation stops available.
        if self.status == FileStatus::Deleted {
            return self.full_anchors();
        }
        let marks = self.changed_full_lines();
        let mut anchors = Vec::new();
        let mut in_block = false;
        for idx in 0..self.full_lines().len() {
            if marks.contains(&(idx + 1)) {
                if !in_block {
                    anchors.push(idx);
                    in_block = true;
                }
            } else {
                in_block = false;
            }
        }
        anchors
    }

    /// 1-based new-file line numbers that were added or changed.
    pub fn changed_full_lines(&self) -> HashSet<usize> {
        let mut set = HashSet::new();
        if self.status == FileStatus::Deleted {
            return set;
        }
        for h in &self.hunks {
            let mut new_ln = h.new_start;
            for &k in &h.kinds {
                match k {
                    ' ' => new_ln += 1,
                    '+' => {
                        set.insert(new_ln);
                        new_ln += 1;
                    }
                    _ => {}
                }
            }
        }
        set
    }
}

/// Align hunk lines into side-by-side rows.
///
/// Within a hunk, context lines pair 1:1. A run of consecutive `-`/`+` lines
/// pairs the i-th deletion with the i-th addition (with intraline word diff);
/// the shorter side is padded with filler cells.
pub fn build_rows(
    hunks: &[Hunk],
    old_lines: &[String],
    new_lines: &[String],
) -> (Vec<Row>, Vec<usize>) {
    let mut rows = Vec::new();
    let mut anchors = Vec::new();
    for (i, h) in hunks.iter().enumerate() {
        anchors.push(rows.len());
        rows.push(Row::HunkHeader(i));
        let mut old_ln = h.old_start;
        let mut new_ln = h.new_start;
        let mut dels: Vec<usize> = Vec::new();
        let mut adds: Vec<usize> = Vec::new();
        for &k in &h.kinds {
            match k {
                '-' => {
                    dels.push(old_ln);
                    old_ln += 1;
                }
                '+' => {
                    adds.push(new_ln);
                    new_ln += 1;
                }
                _ => {
                    flush_run(&mut rows, &mut dels, &mut adds, old_lines, new_lines);
                    rows.push(Row::Line {
                        old: Cell {
                            line_no: Some(old_ln),
                            kind: LineKind::Context,
                            inline: Vec::new(),
                        },
                        new: Cell {
                            line_no: Some(new_ln),
                            kind: LineKind::Context,
                            inline: Vec::new(),
                        },
                    });
                    old_ln += 1;
                    new_ln += 1;
                }
            }
        }
        flush_run(&mut rows, &mut dels, &mut adds, old_lines, new_lines);
    }
    (rows, anchors)
}

/// Row index of the first row of each contiguous run of changed lines. A change
/// row is any `Row::Line` that is not a context/context pair; hunk headers and
/// context rows break a run, so adjacent hunks yield separate anchors.
fn change_block_anchors(rows: &[Row]) -> Vec<usize> {
    let mut anchors = Vec::new();
    let mut in_block = false;
    for (i, row) in rows.iter().enumerate() {
        let is_change = matches!(
            row,
            Row::Line { old, new }
                if !(old.kind == LineKind::Context && new.kind == LineKind::Context)
        );
        if is_change {
            if !in_block {
                anchors.push(i);
                in_block = true;
            }
        } else {
            in_block = false;
        }
    }
    anchors
}

fn flush_run(
    rows: &mut Vec<Row>,
    dels: &mut Vec<usize>,
    adds: &mut Vec<usize>,
    old_lines: &[String],
    new_lines: &[String],
) {
    let n = dels.len().max(adds.len());
    for j in 0..n {
        let d = dels.get(j).copied();
        let a = adds.get(j).copied();
        let (oi, ni) = match (d, a) {
            (Some(d), Some(a)) => inline_ranges(
                old_lines.get(d - 1).map(String::as_str).unwrap_or(""),
                new_lines.get(a - 1).map(String::as_str).unwrap_or(""),
            ),
            _ => (Vec::new(), Vec::new()),
        };
        let old = match d {
            Some(d) => Cell {
                line_no: Some(d),
                kind: LineKind::Del,
                inline: oi,
            },
            None => Cell::filler(),
        };
        let new = match a {
            Some(a) => Cell {
                line_no: Some(a),
                kind: LineKind::Add,
                inline: ni,
            },
            None => Cell::filler(),
        };
        rows.push(Row::Line { old, new });
    }
    dels.clear();
    adds.clear();
}

/// Word-level diff of a paired line; returns changed byte ranges in (old, new).
pub fn inline_ranges(old: &str, new: &str) -> (Vec<(usize, usize)>, Vec<(usize, usize)>) {
    if old == new {
        return (Vec::new(), Vec::new());
    }
    let diff = similar::TextDiff::from_words(old, new);
    let mut o = Vec::new();
    let mut n = Vec::new();
    let mut op = 0usize;
    let mut np = 0usize;
    for ch in diff.iter_all_changes() {
        let len = ch.value().len();
        match ch.tag() {
            similar::ChangeTag::Equal => {
                op += len;
                np += len;
            }
            similar::ChangeTag::Delete => {
                push_range(&mut o, op, op + len);
                op += len;
            }
            similar::ChangeTag::Insert => {
                push_range(&mut n, np, np + len);
                np += len;
            }
        }
    }
    (o, n)
}

fn push_range(v: &mut Vec<(usize, usize)>, start: usize, end: usize) {
    if let Some(last) = v.last_mut() {
        if last.1 == start {
            last.1 = end;
            return;
        }
    }
    v.push((start, end));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn align_pairs_and_fills() {
        let hunks = vec![Hunk {
            old_start: 10,
            old_count: 4,
            new_start: 10,
            new_count: 3,
            kinds: vec![' ', '-', '-', '+', ' '],
        }];
        let old = lines(&[
            "", "", "", "", "", "", "", "", "", "ctx", "del one", "del two", "tail",
        ]);
        let new = lines(&["", "", "", "", "", "", "", "", "", "ctx", "add one", "tail"]);
        let (rows, anchors) = build_rows(&hunks, &old, &new);
        assert_eq!(anchors, vec![0]);
        assert_eq!(rows.len(), 5); // header + ctx + 2 change rows + ctx
        match &rows[2] {
            Row::Line { old, new } => {
                assert_eq!(old.line_no, Some(11));
                assert_eq!(old.kind, LineKind::Del);
                assert_eq!(new.line_no, Some(11));
                assert_eq!(new.kind, LineKind::Add);
                assert!(!old.inline.is_empty());
            }
            _ => panic!("expected change row"),
        }
        match &rows[3] {
            Row::Line { old, new } => {
                assert_eq!(old.line_no, Some(12));
                assert_eq!(new.kind, LineKind::Filler);
                assert_eq!(new.line_no, None);
            }
            _ => panic!("expected filler row"),
        }
        match &rows[4] {
            Row::Line { old, new } => {
                assert_eq!(old.line_no, Some(13));
                assert_eq!(new.line_no, Some(12));
                assert_eq!(old.kind, LineKind::Context);
            }
            _ => panic!("expected context row"),
        }
    }

    #[test]
    fn pure_addition_hunk() {
        let hunks = vec![Hunk {
            old_start: 0,
            old_count: 0,
            new_start: 1,
            new_count: 2,
            kinds: vec!['+', '+'],
        }];
        let new = lines(&["hello", "world"]);
        let (rows, anchors) = build_rows(&hunks, &[], &new);
        assert_eq!(anchors, vec![0]);
        assert_eq!(rows.len(), 3);
        match &rows[1] {
            Row::Line { old, new } => {
                assert_eq!(old.kind, LineKind::Filler);
                assert_eq!(new.line_no, Some(1));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn change_anchors_split_context_separated_runs() {
        // One hunk with two change runs separated by context lines.
        let hunks = vec![Hunk {
            old_start: 1,
            old_count: 4,
            new_start: 1,
            new_count: 4,
            kinds: vec!['+', ' ', ' ', '+'],
        }];
        let new = lines(&["add a", "ctx 1", "ctx 2", "add b"]);
        let (rows, _) = build_rows(&hunks, &[], &new);
        // rows: [header, +add a, ctx, ctx, +add b]
        assert_eq!(change_block_anchors(&rows), vec![1, 4]);
    }

    #[test]
    fn inline_ranges_word_diff() {
        let (o, n) = inline_ranges("foo bar baz", "foo qux baz");
        assert_eq!(o, vec![(4, 7)]);
        assert_eq!(n, vec![(4, 7)]);
    }

    #[test]
    fn inline_ranges_equal_lines() {
        let (o, n) = inline_ranges("same", "same");
        assert!(o.is_empty() && n.is_empty());
    }

    #[test]
    fn changed_full_lines_tracks_new_side() {
        let mut entry = FileEntry {
            path: "a.txt".into(),
            status: FileStatus::Modified,
            binary: false,
            hunks: vec![Hunk {
                old_start: 1,
                old_count: 2,
                new_start: 1,
                new_count: 2,
                kinds: vec![' ', '-', '+'],
            }],
            old_lines: lines(&["keep", "old"]),
            new_lines: lines(&["keep", "new"]),
            rows: Vec::new(),
            anchors: Vec::new(),
            change_anchors: Vec::new(),
            additions: 0,
            deletions: 0,
        };
        entry.finalize();
        assert_eq!(entry.additions, 1);
        assert_eq!(entry.deletions, 1);
        let set = entry.changed_full_lines();
        assert!(set.contains(&2));
        assert!(!set.contains(&1));
    }
}
