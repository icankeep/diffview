//! Collect changed files by shelling out to git and parsing unified diffs.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::model::{FileEntry, FileStatus, Hunk};

const MAX_FILE_BYTES: usize = 2_000_000;

pub fn repo_root(start: &Path) -> Result<PathBuf> {
    let out = Command::new("git")
        .arg("-C")
        .arg(start)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("failed to run git")?;
    if !out.status.success() {
        bail!("{} is not inside a git repository", start.display());
    }
    Ok(PathBuf::from(
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
    ))
}

fn git_out(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .context("failed to run git")?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

fn git_out_ok(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}

/// All changes of worktree+index against `base`, plus untracked files.
pub fn collect(root: &Path, base: &str) -> Result<Vec<FileEntry>> {
    let diff = git_out(
        root,
        &[
            "diff",
            "--no-color",
            "--no-renames",
            "--no-ext-diff",
            base,
            "--",
        ],
    )?;
    let mut parsed = parse_unified_diff(&String::from_utf8_lossy(&diff));

    let untracked = git_out(root, &["ls-files", "--others", "--exclude-standard"])?;
    for p in String::from_utf8_lossy(&untracked).lines() {
        if !p.is_empty() {
            parsed.push(ParsedFile {
                path: PathBuf::from(p),
                status: FileStatus::Added,
                binary: false,
                hunks: Vec::new(),
            });
        }
    }

    let mut files: Vec<FileEntry> = parsed
        .into_iter()
        .map(|pf| load_entry(root, base, pf))
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// Snapshot every regular file under `root` as an addition for non-git dirs.
pub fn collect_directory(root: &Path) -> Result<Vec<FileEntry>> {
    if !root.is_dir() {
        bail!("{} is not a directory", root.display());
    }
    let mut files = Vec::new();
    collect_directory_inner(root, root, &mut files)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

fn collect_directory_inner(root: &Path, dir: &Path, files: &mut Vec<FileEntry>) -> Result<()> {
    for entry in
        std::fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let ty = entry.file_type()?;
        if ty.is_dir() {
            collect_directory_inner(root, &path, files)?;
        } else if ty.is_file() {
            files.push(load_directory_entry(root, &path));
        }
    }
    Ok(())
}

fn load_directory_entry(root: &Path, path: &Path) -> FileEntry {
    let rel = path.strip_prefix(root).unwrap_or(path).to_path_buf();
    let mut e = FileEntry {
        path: rel,
        status: FileStatus::Added,
        binary: false,
        hunks: Vec::new(),
        old_lines: Vec::new(),
        new_lines: Vec::new(),
        rows: Vec::new(),
        anchors: Vec::new(),
        change_anchors: Vec::new(),
        additions: 0,
        deletions: 0,
    };
    match std::fs::read(path) {
        Ok(bytes) if looks_binary(&bytes) => e.binary = true,
        Ok(bytes) => e.new_lines = to_lines(&bytes),
        Err(_) => e.binary = true,
    }
    if !e.binary && !e.new_lines.is_empty() {
        e.hunks.push(Hunk {
            old_start: 0,
            old_count: 0,
            new_start: 1,
            new_count: e.new_lines.len(),
            kinds: vec!['+'; e.new_lines.len()],
        });
    }
    e.finalize();
    e
}

fn load_entry(root: &Path, base: &str, pf: ParsedFile) -> FileEntry {
    let mut e = FileEntry {
        path: pf.path,
        status: pf.status,
        binary: pf.binary,
        hunks: pf.hunks,
        old_lines: Vec::new(),
        new_lines: Vec::new(),
        rows: Vec::new(),
        anchors: Vec::new(),
        change_anchors: Vec::new(),
        additions: 0,
        deletions: 0,
    };
    if matches!(e.status, FileStatus::Modified | FileStatus::Deleted) && !e.binary {
        let spec = format!("{}:{}", base, e.path.display());
        if let Some(bytes) = git_out_ok(root, &["show", &spec]) {
            if looks_binary(&bytes) {
                e.binary = true;
            } else {
                e.old_lines = to_lines(&bytes);
            }
        }
    }
    if e.status != FileStatus::Deleted && !e.binary {
        match std::fs::read(root.join(&e.path)) {
            Ok(bytes) if looks_binary(&bytes) => e.binary = true,
            Ok(bytes) => e.new_lines = to_lines(&bytes),
            Err(_) => {}
        }
    }
    // Untracked files come with no hunks: synthesize one whole-file addition.
    if e.status == FileStatus::Added && e.hunks.is_empty() && !e.binary && !e.new_lines.is_empty() {
        e.hunks.push(Hunk {
            old_start: 0,
            old_count: 0,
            new_start: 1,
            new_count: e.new_lines.len(),
            kinds: vec!['+'; e.new_lines.len()],
        });
    }
    e.finalize();
    e
}

fn looks_binary(bytes: &[u8]) -> bool {
    bytes.len() > MAX_FILE_BYTES || bytes.iter().take(4096).any(|&b| b == 0)
}

fn to_lines(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut v: Vec<String> = text
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).replace('\t', "    "))
        .collect();
    if text.ends_with('\n') {
        v.pop();
    }
    v
}

#[derive(Debug)]
pub struct ParsedFile {
    pub path: PathBuf,
    pub status: FileStatus,
    pub binary: bool,
    pub hunks: Vec<Hunk>,
}

/// Parse `git diff` output into per-file hunk structure (kinds only; line
/// contents are re-read from HEAD blobs and the worktree for rendering).
pub fn parse_unified_diff(text: &str) -> Vec<ParsedFile> {
    let mut files: Vec<ParsedFile> = Vec::new();
    let mut cur: Option<ParsedFile> = None;
    // Remaining (old, new) body lines of the hunk being read; None outside hunks.
    let mut hunk_left: Option<(usize, usize)> = None;

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some(f) = cur.take() {
                files.push(f);
            }
            hunk_left = None;
            cur = Some(ParsedFile {
                path: parse_b_path(rest),
                status: FileStatus::Modified,
                binary: false,
                hunks: Vec::new(),
            });
            continue;
        }
        let Some(f) = cur.as_mut() else { continue };

        if let Some((old_left, new_left)) = hunk_left.as_mut() {
            if line.starts_with('\\') {
                continue; // "\ No newline at end of file"
            }
            let (kind, _) = line.split_at(if line.is_empty() { 0 } else { 1 });
            let kind = kind.chars().next().unwrap_or(' ');
            let kind = match kind {
                '-' => '-',
                '+' => '+',
                _ => ' ',
            };
            match kind {
                '-' => *old_left = old_left.saturating_sub(1),
                '+' => *new_left = new_left.saturating_sub(1),
                _ => {
                    *old_left = old_left.saturating_sub(1);
                    *new_left = new_left.saturating_sub(1);
                }
            }
            if let Some(h) = f.hunks.last_mut() {
                h.kinds.push(kind);
            }
            if *old_left == 0 && *new_left == 0 {
                hunk_left = None;
            }
            continue;
        }

        if line.starts_with("new file mode") {
            f.status = FileStatus::Added;
        } else if line.starts_with("deleted file mode") {
            f.status = FileStatus::Deleted;
        } else if line.starts_with("Binary files ") || line == "GIT binary patch" {
            f.binary = true;
        } else if let Some(rest) = line.strip_prefix("+++ ") {
            if let Some(p) = header_path(rest, "b/") {
                f.path = p;
            }
        } else if let Some(rest) = line.strip_prefix("--- ") {
            if f.status == FileStatus::Deleted {
                if let Some(p) = header_path(rest, "a/") {
                    f.path = p;
                }
            }
        } else if let Some(h) = parse_hunk_header(line) {
            hunk_left = Some((h.old_count, h.new_count));
            let done = h.old_count == 0 && h.new_count == 0;
            f.hunks.push(h);
            if done {
                hunk_left = None;
            }
        }
    }
    if let Some(f) = cur.take() {
        files.push(f);
    }
    files
}

fn parse_hunk_header(line: &str) -> Option<Hunk> {
    let rest = line.strip_prefix("@@ -")?;
    let (old_part, rest) = rest.split_once(" +")?;
    let (new_part, _) = rest.split_once(" @@")?;
    let (old_start, old_count) = parse_range(old_part)?;
    let (new_start, new_count) = parse_range(new_part)?;
    Some(Hunk {
        old_start,
        old_count,
        new_start,
        new_count,
        kinds: Vec::new(),
    })
}

fn parse_range(s: &str) -> Option<(usize, usize)> {
    match s.split_once(',') {
        Some((a, b)) => Some((a.parse().ok()?, b.parse().ok()?)),
        None => Some((s.parse().ok()?, 1)),
    }
}

/// Best-effort path from a "diff --git a/X b/Y" header; the "+++ b/..." line
/// that follows is authoritative and overrides this.
fn parse_b_path(rest: &str) -> PathBuf {
    match rest.rfind(" b/") {
        Some(idx) => PathBuf::from(unquote(&rest[idx + 3..])),
        None => PathBuf::from(rest),
    }
}

/// Path from a "--- a/X" / "+++ b/X" header line; None for /dev/null.
fn header_path(rest: &str, prefix: &str) -> Option<PathBuf> {
    let rest = rest.trim_end();
    if rest == "/dev/null" {
        return None;
    }
    let raw = unquote(rest);
    raw.strip_prefix(prefix).map(PathBuf::from)
}

/// Undo git's C-style quoting of paths with special characters.
fn unquote(s: &str) -> String {
    let Some(inner) = s.strip_prefix('"').and_then(|x| x.strip_suffix('"')) else {
        return s.to_string();
    };
    let b = inner.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() {
            i += 1;
            match b[i] {
                b'n' => {
                    out.push(b'\n');
                    i += 1;
                }
                b't' => {
                    out.push(b'\t');
                    i += 1;
                }
                b'\\' | b'"' => {
                    out.push(b[i]);
                    i += 1;
                }
                b'0'..=b'7' => {
                    let mut val: u32 = 0;
                    let mut k = 0;
                    while k < 3 && i < b.len() && (b'0'..=b'7').contains(&b[i]) {
                        val = val * 8 + u32::from(b[i] - b'0');
                        i += 1;
                        k += 1;
                    }
                    out.push(val as u8);
                }
                c => {
                    out.push(b'\\');
                    out.push(c);
                    i += 1;
                }
            }
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
diff --git a/src/a.rs b/src/a.rs
index 1111111..2222222 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,3 +1,4 @@
 fn main() {
-    let x = 1;
+    let x = 2;
+    let y = 3;
 }
diff --git a/new.txt b/new.txt
new file mode 100644
index 0000000..1111111
--- /dev/null
+++ b/new.txt
@@ -0,0 +1,2 @@
+hello
+world
diff --git a/gone.txt b/gone.txt
deleted file mode 100644
index 1111111..0000000
--- a/gone.txt
+++ /dev/null
@@ -1 +0,0 @@
-bye
diff --git a/img.png b/img.png
index 1111111..2222222 100644
Binary files a/img.png and b/img.png differ
";

    #[test]
    fn parses_multiple_files() {
        let files = parse_unified_diff(SAMPLE);
        assert_eq!(files.len(), 4);

        assert_eq!(files[0].path, PathBuf::from("src/a.rs"));
        assert_eq!(files[0].status, FileStatus::Modified);
        assert_eq!(files[0].hunks.len(), 1);
        assert_eq!(files[0].hunks[0].kinds, vec![' ', '-', '+', '+', ' ']);
        assert_eq!(files[0].hunks[0].old_start, 1);
        assert_eq!(files[0].hunks[0].new_count, 4);

        assert_eq!(files[1].path, PathBuf::from("new.txt"));
        assert_eq!(files[1].status, FileStatus::Added);
        assert_eq!(files[1].hunks[0].kinds, vec!['+', '+']);

        assert_eq!(files[2].path, PathBuf::from("gone.txt"));
        assert_eq!(files[2].status, FileStatus::Deleted);
        assert_eq!(files[2].hunks[0].kinds, vec!['-']);

        assert_eq!(files[3].path, PathBuf::from("img.png"));
        assert!(files[3].binary);
        assert!(files[3].hunks.is_empty());
    }

    #[test]
    fn hunk_body_lines_starting_with_dashes_are_content() {
        // A deleted line whose content starts with "-- " must not be
        // mistaken for a "--- a/..." header; count tracking handles this.
        let diff = "\
diff --git a/x b/x
index 1111111..2222222 100644
--- a/x
+++ b/x
@@ -1,2 +1,1 @@
--- not a header
 keep
";
        let files = parse_unified_diff(diff);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].hunks[0].kinds, vec!['-', ' ']);
    }

    #[test]
    fn unquotes_paths() {
        assert_eq!(unquote("\"a\\\\b \\\"c\\\".txt\""), "a\\b \"c\".txt");
        assert_eq!(unquote("plain.txt"), "plain.txt");
        assert_eq!(unquote("\"\\344\\270\\255.txt\""), "中.txt");
    }

    #[test]
    fn collects_directory_files_as_additions() {
        let root = temp_root("diffview-dir-additions");
        let nested = root.join("src");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.join("README.md"), "hello\nworld\n").unwrap();
        std::fs::write(nested.join("main.rs"), "fn main() {}\n").unwrap();

        let files = collect_directory(&root).unwrap();

        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, PathBuf::from("README.md"));
        assert_eq!(files[0].status, FileStatus::Added);
        assert_eq!(files[0].new_lines, vec!["hello", "world"]);
        assert_eq!(files[0].hunks[0].kinds, vec!['+', '+']);
        assert_eq!(files[1].path, PathBuf::from("src/main.rs"));
        assert_eq!(files[1].additions, 1);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn marks_directory_binary_files_without_text_hunks() {
        let root = temp_root("diffview-dir-binary");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("image.bin"), b"abc\0def").unwrap();

        let files = collect_directory(&root).unwrap();

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, PathBuf::from("image.bin"));
        assert_eq!(files[0].status, FileStatus::Added);
        assert!(files[0].binary);
        assert!(files[0].hunks.is_empty());

        std::fs::remove_dir_all(root).unwrap();
    }

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        root
    }
}
