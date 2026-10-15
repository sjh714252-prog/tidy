//! Text rendering of a scanned tree, `tree`-command style.

use std::fmt::Write as _;

use humansize::{format_size, BINARY};

use super::scan::DirNode;

#[derive(Debug, Clone)]
pub struct TreeOptions {
    /// Depth to print (None = everything).
    pub max_depth: Option<usize>,
    /// Print individual files, not only directories.
    pub show_files: bool,
    /// Append size to each line.
    pub show_size: bool,
    /// Collapse a directory's file list after this many entries.
    pub max_files_per_dir: usize,
}

impl Default for TreeOptions {
    fn default() -> Self {
        Self {
            max_depth: None,
            show_files: true,
            show_size: true,
            max_files_per_dir: 20,
        }
    }
}

pub fn render_tree(root: &DirNode, opts: &TreeOptions) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{}  [{} files, {} dirs, {}]",
        root.name,
        root.file_count,
        root.dir_count,
        format_size(root.total_size, BINARY)
    );
    render_children(root, "", 0, opts, &mut out);
    out
}

fn render_children(
    node: &DirNode,
    prefix: &str,
    depth: usize,
    opts: &TreeOptions,
    out: &mut String,
) {
    if let Some(max) = opts.max_depth {
        if depth >= max {
            return;
        }
    }

    let dir_count = node.dirs.len();
    let shown_files = if opts.show_files {
        node.files.len().min(opts.max_files_per_dir)
    } else {
        0
    };
    let hidden_files = if opts.show_files {
        node.files.len() - shown_files
    } else {
        0
    };
    let total = dir_count + shown_files + usize::from(hidden_files > 0);
    let mut i = 0;

    for child in node.dirs.values() {
        i += 1;
        let last = i == total;
        let (branch, next) = if last {
            ("└── ", "    ")
        } else {
            ("├── ", "│   ")
        };
        let size = if opts.show_size {
            format!(
                "  [{} files, {}]",
                child.file_count,
                format_size(child.total_size, BINARY)
            )
        } else {
            String::new()
        };
        let _ = writeln!(out, "{prefix}{branch}{}/{size}", child.name);
        render_children(child, &format!("{prefix}{next}"), depth + 1, opts, out);
    }

    for f in node.files.iter().take(shown_files) {
        i += 1;
        let last = i == total;
        let branch = if last { "└── " } else { "├── " };
        let size = if opts.show_size {
            format!("  ({})", format_size(f.size, BINARY))
        } else {
            String::new()
        };
        let _ = writeln!(out, "{prefix}{branch}{}{size}", f.name);
    }

    if hidden_files > 0 {
        let _ = writeln!(out, "{prefix}└── … {hidden_files} more files");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::scan::{scan, ScanOptions};
    use std::fs;

    #[test]
    fn renders_tree_text() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path();
        fs::create_dir_all(r.join("업무")).unwrap();
        fs::write(r.join("업무/a.txt"), b"12345").unwrap();
        fs::write(r.join("b.txt"), b"1").unwrap();
        let res = scan(r, &ScanOptions::default()).unwrap();
        let text = render_tree(&res.tree, &TreeOptions::default());
        assert!(text.contains("├── 업무/"));
        assert!(text.contains("│   └── a.txt  (5 B)"));
        assert!(text.contains("└── b.txt  (1 B)"));
    }

    #[test]
    fn collapses_long_file_lists() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..25 {
            fs::write(tmp.path().join(format!("f{i:02}.txt")), b"x").unwrap();
        }
        let res = scan(tmp.path(), &ScanOptions::default()).unwrap();
        let text = render_tree(&res.tree, &TreeOptions::default());
        assert!(text.contains("… 5 more files"));
    }
}
