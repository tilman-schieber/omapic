//! Shell command history — the one thing omapic remembers between runs.
//! It is about the tool, not about any images: a plain text file under
//! `$XDG_STATE_HOME/omapic`, most recent command last.

use std::path::{Path, PathBuf};

const KEPT: usize = 200;

pub fn default_path() -> Option<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".local/state")))?;
    Some(state.join("omapic/shell-history"))
}

/// Most recent first.
pub fn load(path: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines().rev().map(str::to_string).filter(|l| !l.trim().is_empty()).collect()
}

/// Put `command` at the top; an older copy of it goes.
pub fn remember(path: &Path, command: &str) -> std::io::Result<()> {
    let command = command.trim().replace('\n', " ");
    let mut entries = load(path);
    entries.retain(|e| *e != command);
    entries.insert(0, command);
    entries.truncate(KEPT);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text: String = entries.iter().rev().map(|e| format!("{e}\n")).collect();
    std::fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_first_without_duplicates() {
        let path = std::env::temp_dir().join(format!("omapic-history-{}/h", std::process::id()));
        let _ = std::fs::remove_file(&path);
        assert!(load(&path).is_empty());
        for command in ["a {}", "b {}", "a {}", "  c {+} "] {
            remember(&path, command).unwrap();
        }
        assert_eq!(load(&path), ["c {+}", "a {}", "b {}"]);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
