//! New-vertex defaults remembered across launches. Drawings are never stored;
//! only the style and label mode new vertices start with.
use super::*;
use std::path::{Path, PathBuf};

pub(super) fn path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".local/state/graph-editor/defaults"))
}

/// Unknown keys and malformed lines are ignored.
pub(super) fn load(app: &mut App, path: &Path) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    for line in text.lines() {
        match line.split_once('=') {
            Some(("style", name)) => {
                if let Some(style) = VertexStyle::from_name(name.trim()) {
                    app.new_style = style;
                }
            }
            Some(("labels", mode)) => app.new_unlabeled = mode.trim() == "none",
            _ => {}
        }
    }
}

/// Best effort: failing to remember defaults must not fail the session.
pub(super) fn save(app: &App, path: &Path) {
    let text = format!(
        "style={}\nlabels={}\n",
        app.new_style.name(),
        if app.new_unlabeled { "none" } else { "numbered" }
    );
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, text);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::test_support::app;
    #[test]
    fn defaults_round_trip_and_ignore_garbage() {
        let path = std::env::temp_dir()
            .join(format!("graph-prefs-{}", std::process::id()))
            .join("defaults");
        let mut first = app();
        first.new_style = VertexStyle::Circles;
        first.new_unlabeled = true;
        save(&first, &path);
        let mut second = app();
        load(&mut second, &path);
        assert_eq!(second.new_style, VertexStyle::Circles);
        assert!(second.new_unlabeled);
        std::fs::write(&path, "style=hexagons\njunk\nlabels=numbered\n").unwrap();
        load(&mut second, &path);
        assert_eq!(second.new_style, VertexStyle::Circles);
        assert!(!second.new_unlabeled);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
