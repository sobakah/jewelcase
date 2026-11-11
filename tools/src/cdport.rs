//! cdport: getting CDs into the library and editing tags (CONCEPT.md
//! §10.2, §10.3).
//!
//! cdport offers a D-Bus interface, which the bus starts on demand once
//! `cdport --install-dbus-service` has run; it's the only way a Flatpak
//! can reach it. Without it, the player starts the program.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::{config_path, expand_home, read_json};

/// How to install it (pipx, from its repository).
pub const INSTALL_COMMAND: &str = "pipx install git+https://github.com/sobakah/cdport.git";
/// Lets the session bus start cdport when Jewelcase calls it.
pub const SERVICE_ARG: &str = "--install-dbus-service";
pub const SERVICE_COMMAND: &str = "cdport --install-dbus-service";

/// The program's name.
pub const PROGRAM: &str = "cdport";

/// cdport's D-Bus interface (`cdport/dbus_service.py`).
pub const BUS_NAME: &str = "io.github.sobakah.cdport";
pub const OBJECT_PATH: &str = "/io/github/sobakah/cdport";
pub const INTERFACE: &str = "io.github.sobakah.cdport1";

/// cdport's marker of an unfinished import (only its presence is checked;
/// it kept cdport's old name).
const STATE_FILE: &str = ".cdimport.json";

/// What the player reads from cdport's config (it never writes it), or
/// gets from its `GetInfo` over D-Bus.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub library_dir: Option<PathBuf>,
    pub work_dir: Option<PathBuf>,
    /// The preferred tag editor's name in `editors` ("eartag", …).
    pub editor: Option<String>,
}

impl Config {
    /// `~/.config/cdport/config.json`, or the one under cdport's old name
    /// (cdimport) where cdport hasn't moved it yet.
    pub fn read() -> Self {
        let new = config_path("cdport");
        Self::from_path(&if new.is_file() {
            new
        } else {
            config_path("cdimport")
        })
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

/// `cdport --web --no-browser [-l <library>]`. `library` is passed only
/// when cdport's own library folder lies outside the player's folders,
/// so imports always land where the player looks (§10.2).
pub fn import_args(program: &Path, library: Option<&Path>) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![program.into(), "--web".into(), "--no-browser".into()];
    if let Some(library) = library {
        args.push("-l".into());
        args.push(library.into());
    }
    args
}

/// `cdport <album dir> --web --no-browser …`: continue an unfinished import.
pub fn continue_args(program: &Path, album_dir: &Path, library: Option<&Path>) -> Vec<OsString> {
    let mut args = import_args(program, library);
    args.insert(1, album_dir.into());
    args
}

/// `cdport <album dir> --edit-tags --web --no-browser`: only its tag
/// editor, on one album folder.
pub fn edit_tags_args(program: &Path, album_dir: &Path) -> Vec<OsString> {
    vec![
        program.into(),
        album_dir.into(),
        "--edit-tags".into(),
        "--web".into(),
        "--no-browser".into(),
    ]
}

pub fn version_args(program: &Path) -> Vec<OsString> {
    vec![program.into(), "--version".into()]
}

/// The `-l` to pass: `None` if cdport's library folder is inside one of
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

/// Album folders in cdport's work folder with an unfinished import.
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
            continue_args(Path::new("cdport"), Path::new("/w/Album"), None),
            ["cdport", "/w/Album", "--web", "--no-browser"].map(OsString::from)
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
