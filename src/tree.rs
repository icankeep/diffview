//! Build a directory tree from changed file paths and flatten it for display.

use crate::model::FileEntry;

#[derive(Debug)]
pub struct Node {
    pub name: String,
    pub children: Vec<Node>,
    /// Index into the file list for leaf nodes.
    pub file: Option<usize>,
    pub expanded: bool,
}

#[derive(Debug)]
pub struct Tree {
    pub roots: Vec<Node>,
}

#[derive(Debug, Clone)]
pub struct TreeRow {
    pub depth: usize,
    pub label: String,
    pub file: Option<usize>,
    pub expanded: bool,
    /// Child-index path from the roots to this node, for toggling.
    pub node_path: Vec<usize>,
}

pub fn build(files: &[FileEntry]) -> Tree {
    let mut roots: Vec<Node> = Vec::new();
    for (idx, f) in files.iter().enumerate() {
        let comps: Vec<String> = f
            .path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        if !comps.is_empty() {
            insert(&mut roots, &comps, idx);
        }
    }
    sort_nodes(&mut roots);
    collapse(&mut roots);
    Tree { roots }
}

fn insert(nodes: &mut Vec<Node>, comps: &[String], file: usize) {
    let (head, rest) = comps.split_first().unwrap();
    if rest.is_empty() {
        nodes.push(Node {
            name: head.clone(),
            children: Vec::new(),
            file: Some(file),
            expanded: false,
        });
        return;
    }
    let i = match nodes
        .iter()
        .position(|n| n.file.is_none() && n.name == *head)
    {
        Some(i) => i,
        None => {
            nodes.push(Node {
                name: head.clone(),
                children: Vec::new(),
                file: None,
                expanded: true,
            });
            nodes.len() - 1
        }
    };
    insert(&mut nodes[i].children, rest, file);
}

fn sort_nodes(nodes: &mut [Node]) {
    nodes.sort_by(|a, b| {
        a.file
            .is_some()
            .cmp(&b.file.is_some())
            .then_with(|| a.name.cmp(&b.name))
    });
    for n in nodes.iter_mut() {
        sort_nodes(&mut n.children);
    }
}

/// Merge single-child directory chains: a > b > c becomes "a/b/c".
fn collapse(nodes: &mut Vec<Node>) {
    for n in nodes.iter_mut() {
        while n.file.is_none() && n.children.len() == 1 && n.children[0].file.is_none() {
            let child = n.children.remove(0);
            n.name = format!("{}/{}", n.name, child.name);
            n.children = child.children;
        }
        collapse(&mut n.children);
    }
}

impl Tree {
    pub fn flatten(&self) -> Vec<TreeRow> {
        fn walk(nodes: &[Node], depth: usize, path: &mut Vec<usize>, rows: &mut Vec<TreeRow>) {
            for (i, n) in nodes.iter().enumerate() {
                path.push(i);
                rows.push(TreeRow {
                    depth,
                    label: n.name.clone(),
                    file: n.file,
                    expanded: n.expanded,
                    node_path: path.clone(),
                });
                if n.file.is_none() && n.expanded {
                    walk(&n.children, depth + 1, path, rows);
                }
                path.pop();
            }
        }
        let mut rows = Vec::new();
        let mut path = Vec::new();
        walk(&self.roots, 0, &mut path, &mut rows);
        rows
    }

    pub fn toggle(&mut self, node_path: &[usize]) {
        let mut nodes = &mut self.roots;
        for (k, &i) in node_path.iter().enumerate() {
            let Some(n) = nodes.get_mut(i) else { return };
            if k == node_path.len() - 1 {
                if n.file.is_none() {
                    n.expanded = !n.expanded;
                }
                return;
            }
            nodes = &mut n.children;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FileEntry, FileStatus};

    fn entry(path: &str) -> FileEntry {
        FileEntry {
            path: path.into(),
            status: FileStatus::Modified,
            binary: false,
            hunks: Vec::new(),
            old_lines: Vec::new(),
            new_lines: Vec::new(),
            rows: Vec::new(),
            anchors: Vec::new(),
            additions: 0,
            deletions: 0,
        }
    }

    #[test]
    fn collapses_and_flattens() {
        let files = vec![entry("a/b/c.rs"), entry("a/b/d.rs"), entry("x.txt")];
        let tree = build(&files);
        let rows = tree.flatten();
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, vec!["a/b", "c.rs", "d.rs", "x.txt"]);
        assert_eq!(rows[0].file, None);
        assert_eq!(rows[1].file, Some(0));
        assert_eq!(rows[1].depth, 1);
        assert_eq!(rows[3].depth, 0);
    }

    #[test]
    fn toggle_hides_children() {
        let files = vec![entry("a/b/c.rs"), entry("x.txt")];
        let mut tree = build(&files);
        let rows = tree.flatten();
        assert_eq!(rows.len(), 3);
        tree.toggle(&rows[0].node_path);
        assert_eq!(tree.flatten().len(), 2);
        tree.toggle(&rows[0].node_path);
        assert_eq!(tree.flatten().len(), 3);
    }
}
