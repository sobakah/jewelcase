//! cdimport: getting CDs into the library (CONCEPT.md §10.2).

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::{config_path, expand_home, read_json};

pub const INSTALL_COMMAND: &str = "pipx install cdimport";

/// cdimport's marker of an unfinished import (only its presence is checked).
const STATE_FILE: &str = ".cdimport.json";

/// What the player reads from cdimport's config (it never writes it).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub library_dir: Option<PathBuf>,
    pub work_dir: Option<PathBuf>,
    /// The preferred tag editor's name in `editors` ("eartag", …).
    pub editor: Option<String>,
}

impl Config {
    pub fn read() -> Self {
        Self::from_path(&config_path("cdimport"))
    }

    pub fn from_path(path: &Path) -> Self {
        let Some(json) = read_json(path) else {
            return Self::default();
        };
        let dir = |key: &str| {
            json.get(key)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(expand_home)
        };
        Self {
            library_dir: dir("library_dir"),
            work_dir: dir("work_dir"),
            editor: json
                .get("editor")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
        }
    }
}

/// `cdimport --web --no-browser [-l <library>]`. `library` is passed only
/// when cdimport's own library folder lies outside the player's folders,
/// so imports always land where the player looks (§10.2).
pub fn import_args(program: &Path, library: Option<&Path>) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![program.into(), "--web".into(), "--no-browser".into()];
    if let Some(library) = library {
        args.push("-l".into());
        args.push(library.into());
    }
    args
}

/// `cdimport <album dir> --web --no-browser …`: continue an unfinished import.
pub fn continue_args(program: &Path, album_dir: &Path, library: Option<&Path>) -> Vec<OsString> {
    let mut args = import_args(program, library);
    args.insert(1, album_dir.into());
    args
}

pub fn version_args(program: &Path) -> Vec<OsString> {
    vec![program.into(), "--version".into()]
}

/// The `-l` to pass: `None` if cdimport's library folder is inside one of
/// `folders` (its own choice already ends up in the library), else the
/// player's first folder.
pub fn library_arg<'a>(config: &Config, folders: &'a [PathBuf]) -> Option<&'a Path> {
    let inside = config
        .library_dir
        .as_ref()
        .is_some_and(|dir| folders.iter().any(|f| dir.starts_with(f)));
    if inside {
        None
    } else {
        folders.first().map(PathBuf::as_path)
    }
}

/// Album folders in cdimport's work folder with an unfinished import.
pub fn unfinished_imports(work_dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    find_state_files(work_dir, 0, &mut found);
    found.sort();
    found
}

fn find_state_files(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    if dir.join(STATE_FILE).is_file() {
        found.push(dir.to_path_buf());
        return;
    }
    if depth >= 3 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            find_state_files(&entry.path(), depth + 1, found);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_config_and_expands_home() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        fs::write(
            &path,
            r#"{"work_dir": "~/Temp", "library_dir": "/m/Redo", "editor": "eartag"}"#,
        )
        .unwrap();
        let config = Config::from_path(&path);
        assert!(config.work_dir.unwrap().ends_with("Temp"));
        assert_eq!(config.library_dir, Some(PathBuf::from("/m/Redo")));
        assert_eq!(config.editor.as_deref(), Some("eartag"));
    }

    #[test]
    fn library_argument() {
        let folders = vec![PathBuf::from("/home/u/Musik")];
        let inside = Config {
            library_dir: Some("/home/u/Musik/Redo".into()),
            ..Config::default()
        };
        assert_eq!(library_arg(&inside, &folders), None);
        let outside = Config {
            library_dir: Some("/data/Rips".into()),
            ..Config::default()
        };
        assert_eq!(
            library_arg(&outside, &folders),
            Some(Path::new("/home/u/Musik"))
        );
        assert_eq!(
            continue_args(Path::new("cdimport"), Path::new("/w/Album"), None),
            ["cdimport", "/w/Album", "--web", "--no-browser"].map(OsString::from)
        );
    }

    #[test]
    fn finds_unfinished_imports() {
        let dir = tempfile::TempDir::new().unwrap();
        let album = dir.path().join("Artist/Album");
        fs::create_dir_all(&album).unwrap();
        fs::create_dir_all(dir.path().join("Done")).unwrap();
        fs::write(album.join(STATE_FILE), "{}").unwrap();
        assert_eq!(unfinished_imports(dir.path()), [album]);
        assert!(unfinished_imports(&dir.path().join("missing")).is_empty());
    }
}
