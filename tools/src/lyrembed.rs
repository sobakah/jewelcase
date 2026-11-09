//! lyrembed: lyrics search, checking and romanization (CONCEPT.md §10.1).
//!
//! From 1.2.0 on, lyrembed offers a D-Bus interface, which the bus starts
//! on demand once `lyrembed --install-dbus-service` has run; it's the only
//! way a Flatpak can reach it. Without it, the player starts the program.

use std::ffi::OsString;
use std::path::Path;

use crate::{Version, config_path, read_json};

/// `lyrembed romanize --stdio` (§10.4) came with lyrembed 1.1.1; probing
/// decides (`romanize --help`), not the version.
/// How to install it (pipx, from its repository, with romanization).
pub const INSTALL_COMMAND: &str =
    "pipx install \"lyrembed[all] @ git+https://github.com/sobakah/lyrembed.git\"";
/// Updating a pipx install (its page offers the same).
pub const UPGRADE_COMMAND: &str = "pipx upgrade lyrembed";
/// Lets the session bus start lyrembed when Gatefold calls it.
pub const SERVICE_ARG: &str = "--install-dbus-service";
pub const SERVICE_COMMAND: &str = "lyrembed --install-dbus-service";

/// The first version with the D-Bus interface.
pub const DBUS_VERSION: Version = Version(1, 2, 0);

/// lyrembed's D-Bus interface (`lyrembed/dbus_service.py`). It opens its
/// pages in the browser itself: their address never comes over the bus.
pub const BUS_NAME: &str = "io.github.sobakah.lyrembed";
pub const OBJECT_PATH: &str = "/io/github/sobakah/lyrembed";
pub const INTERFACE: &str = "io.github.sobakah.lyrembed1";

/// `lyrembed <album dir> --auto`: tag tracks with confident matches.
pub fn auto_args(program: &Path, album_dir: &Path) -> Vec<OsString> {
    vec![
        program.into(),
        album_dir.into(),
        "--auto".into(),
        "--color".into(),
        "never".into(),
    ]
}

/// `lyrembed <album dir> --no-browser`: the album's page, opened by the app.
pub fn page_args(program: &Path, album_dir: &Path) -> Vec<OsString> {
    vec![program.into(), album_dir.into(), "--no-browser".into()]
}

pub fn version_args(program: &Path) -> Vec<OsString> {
    vec![program.into(), "--version".into()]
}

/// What a `--auto` run's exit code means (§10.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoResult {
    /// Done; may also mean nothing confident was found.
    Done,
    /// Some files couldn't be written.
    SomeFailed,
    /// Unusable folder or config.
    Unusable,
    Cancelled,
    Other(i32),
}

pub fn auto_result(exit_code: i32) -> AutoResult {
    match exit_code {
        0 => AutoResult::Done,
        1 => AutoResult::SomeFailed,
        2 => AutoResult::Unusable,
        130 => AutoResult::Cancelled,
        code => AutoResult::Other(code),
    }
}

/// What the player reads from lyrembed's config (it never writes it).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    /// "below" keeps the original script for dual-line display.
    pub romanization_layout: Option<String>,
    pub max_file_count: Option<u64>,
}

impl Config {
    pub fn read() -> Self {
        Self::from_path(&config_path("lyrembed"))
    }

    pub fn from_path(path: &Path) -> Self {
        let Some(json) = read_json(path) else {
            return Self::default();
        };
        let settings = json.get("settings").unwrap_or(&json);
        Self {
            romanization_layout: settings
                .get("romanization_layout")
                .and_then(|v| v.as_str())
                .map(str::to_owned),
            max_file_count: settings.get("max_file_count").and_then(|v| v.as_u64()),
        }
    }

    /// lyrembed's default ("replace") drops the original script (§10.1).
    pub fn keeps_original(&self) -> bool {
        self.romanization_layout.as_deref() == Some("below")
    }
}

/// Whether this lyrembed can romanize for display (`romanize --stdio`,
/// §5.1). `help` is the output of `lyrembed romanize --help`.
pub fn supports_romanize_stdio(help: &str) -> bool {
    help.contains("--stdio")
}

/// Shown when automatic romanization isn't possible.
pub fn romanize_requirement(version: Option<Version>) -> String {
    match version {
        Some(version) => format!(
            "lyrembed {version} has no “romanize --stdio” yet, which automatic romanization needs (lyrembed 1.1.1 or newer)"
        ),
        None => "Automatic romanization needs lyrembed".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes() {
        assert_eq!(auto_result(0), AutoResult::Done);
        assert_eq!(auto_result(1), AutoResult::SomeFailed);
        assert_eq!(auto_result(2), AutoResult::Unusable);
        assert_eq!(auto_result(130), AutoResult::Cancelled);
        assert_eq!(auto_result(9), AutoResult::Other(9));
    }

    #[test]
    fn reads_config() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{"settings": {"romanization_layout": "below", "max_file_count": 250}}"#,
        )
        .unwrap();
        let config = Config::from_path(&path);
        assert!(config.keeps_original());
        assert_eq!(config.max_file_count, Some(250));
        assert!(!Config::from_path(&dir.path().join("missing.json")).keeps_original());
    }

    #[test]
    fn romanize_probe() {
        assert!(supports_romanize_stdio(
            "usage: lyrembed romanize [--stdio]"
        ));
        assert!(!supports_romanize_stdio("usage: lyrembed [-h] [--version]"));
    }
}
