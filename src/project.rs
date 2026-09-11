use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::Result;
use crate::layout::{PaneNode, SplitAxis, split_pane};
use crate::session::{SessionSnapshot, SnapshotPane, SnapshotTab};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Project {
    windows: Vec<ProjectWindow>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectWindow {
    name: String,
    panes: Vec<ProjectPane>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectPane {
    #[serde(default = "default_directory")]
    cwd: PathBuf,
    command: Option<String>,
    #[serde(default)]
    split: Split,
}

fn default_directory() -> PathBuf {
    PathBuf::from(".")
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Split {
    #[default]
    Right,
    Down,
}

pub(super) fn load(path: &Path) -> Result<SessionSnapshot> {
    let path = fs::canonicalize(path)?;
    let source = fs::read_to_string(&path)?;
    parse(
        &source,
        path.parent().ok_or("layout has no parent directory")?,
    )
}

fn parse(source: &str, base: &Path) -> Result<SessionSnapshot> {
    let project: Project = toml::from_str(source)?;
    let pane_count: usize = project
        .windows
        .iter()
        .map(|window| window.panes.len())
        .sum();
    if project.windows.is_empty() || pane_count > 128 {
        return Err("a project layout requires windows and supports at most 128 panes".into());
    }
    let mut tabs = Vec::new();
    let mut next_id = 1;
    for window in project.windows {
        if window.panes.is_empty()
            || window.name.trim().is_empty()
            || window.name.chars().any(char::is_control)
        {
            return Err("each project window needs a nonempty name and at least one pane".into());
        }
        let id = next_id;
        let mut root = PaneNode::Leaf(id);
        let mut panes = Vec::new();
        for pane in window.panes {
            let cwd = fs::canonicalize(base.join(&pane.cwd))
                .map_err(|error| format!("layout directory '{}': {error}", pane.cwd.display()))?;
            if !cwd.is_dir() {
                return Err(
                    format!("layout directory '{}' is not a directory", cwd.display()).into(),
                );
            }
            if pane
                .command
                .as_ref()
                .is_some_and(|command| command.contains('\0') || command.trim().is_empty())
            {
                return Err("pane commands must be nonempty and cannot contain NUL".into());
            }
            if next_id != id {
                split_pane(
                    &mut root,
                    next_id - 1,
                    next_id,
                    match pane.split {
                        Split::Right => SplitAxis::Vertical,
                        Split::Down => SplitAxis::Horizontal,
                    },
                );
            }
            panes.push(SnapshotPane {
                scrollback: None,
                id: next_id,
                cwd: Some(cwd),
                command: pane.command,
            });
            next_id += 1;
        }
        tabs.push(SnapshotTab {
            id,
            name: window.name,
            root,
            panes,
        });
    }
    Ok(SessionSnapshot::new(tabs, None, 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_directories_are_relative_to_layout_and_splits_are_preserved() {
        let base = std::env::current_dir().unwrap();
        let snapshot = parse(
            r#"
[[windows]]
name = "dev"
[[windows.panes]]
cwd = "src"
command = "cargo check"
[[windows.panes]]
split = "down"
"#,
            &base,
        )
        .unwrap();
        assert_eq!(
            snapshot.tabs[0].panes[0].cwd,
            Some(fs::canonicalize(base.join("src")).unwrap())
        );
        assert!(matches!(
            snapshot.tabs[0].root,
            PaneNode::Split {
                axis: SplitAxis::Horizontal,
                ..
            }
        ));
        assert_eq!(
            snapshot.tabs[0].panes[0].command.as_deref(),
            Some("cargo check")
        );
    }

    #[test]
    fn invalid_layouts_fail_before_starting_processes() {
        let base = std::env::current_dir().unwrap();
        for source in [
            "windows = []",
            "[[windows]]\nname='empty'\npanes=[]",
            "[[windows]]\nname='bad'\n[[windows.panes]]\ncwd='missing-rustmux-dir'",
            "[[windows]]\nname='bad'\n[[windows.panes]]\nsplit='diagonal'",
        ] {
            assert!(parse(source, &base).is_err());
        }
    }
}
