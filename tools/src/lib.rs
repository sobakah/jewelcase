//! Gatefold's companion tools (CONCEPT.md §10): lyrembed, cdport and the
//! tag editor (Ear Tag by default).
//!
//! The command line is the stable interface: arguments, exit codes,
//! `--version` and the "`<tool> is running at <url>`" line; cdport also
//! has a D-Bus interface (its names are in `cdport`). This crate builds
//! commands and reads the tools' configs; running them is the app's job
//! (`gio::Subprocess`, D-Bus calls).

pub mod cdport;
pub mod editor;
pub mod lyrembed;

use std::env;
use std::path::{Path, PathBuf};

/// A version like `1.2.0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u32, pub u32, pub u32);

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// The version in `--version` output ("lyrembed 1.1.0").
pub fn parse_version(output: &str) -> Option<Version> {
    output.split_whitespace().find_map(|word| {
        let mut parts = word.trim_start_matches('v').split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next().map_or(Some(0), |p| {
            p.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .ok()
        })?;
        Some(Version(major, minor, patch))
    })
}

/// Finds a program: the override, else `PATH`, else `~/.local/bin` (apps
/// started from the GNOME menu often lack it in `PATH`, §10.0).
pub fn find_program(name: &str, override_path: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = override_path.filter(|p| !p.as_os_str().is_empty()) {
        return is_executable(path).then(|| path.to_path_buf());
    }
    let mut dirs: Vec<PathBuf> = env::var_os("PATH")
        .map(|path| env::split_paths(&path).collect())
        .unwrap_or_default();
    if let Some(home) = env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/bin"));
    }
    dirs.into_iter()
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The page address from a line like
/// `lyrembed is running at http://127.0.0.1:8123/?token=abc`.
pub fn page_url(line: &str) -> Option<String> {
    let (_, rest) = line.split_once(" is running at ")?;
    let url = rest.split_whitespace().next()?;
    let local = url.starts_with("http://127.0.0.1:") || url.starts_with("http://localhost:");
    (local && url.contains("token=")).then(|| url.to_owned())
}

/// `~/…` → the home folder.
pub fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ if path == "~" => env::var_os("HOME").map(PathBuf::from).unwrap_or_default(),
        _ => PathBuf::from(path),
    }
}

/// How to install pipx on Fedora (other distributions have a pipx package
/// too), and to put the programs it installs on PATH.
pub const PIPX_INSTALL_COMMAND: &str = "sudo dnf install pipx";
pub const PIPX_PATH_COMMAND: &str = "pipx ensurepath";

/// `~/.config/<tool>/config.json`.
pub fn config_path(tool: &str) -> PathBuf {
    let base = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| expand_home("~/.config"));
    base.join(tool).join("config.json")
}

pub(crate) fn read_json(path: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("lyrembed 1.1.0\n"), Some(Version(1, 1, 0)));
        assert_eq!(parse_version("cdimport v1.2"), Some(Version(1, 2, 0)));
        assert_eq!(parse_version("tool 2.0.1rc1"), Some(Version(2, 0, 1)));
        assert_eq!(parse_version("no version"), None);
        assert!(Version(1, 2, 0) > Version(1, 1, 9));
    }

    #[test]
    fn page_urls() {
        assert_eq!(
            page_url("lyrembed is running at http://127.0.0.1:8123/?token=a-b_c").as_deref(),
            Some("http://127.0.0.1:8123/?token=a-b_c")
        );
        assert_eq!(
            page_url("cdimport is running at https://evil.example/?token=x"),
            None
        );
        assert_eq!(page_url("cdimport is running at http://127.0.0.1:1/"), None);
        assert_eq!(page_url("Scanning…"), None);
    }

    #[test]
    fn programs_are_found_by_override() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let tool = dir.path().join("lyrembed");
        std::fs::write(&tool, "#!/bin/sh\n").unwrap();
        assert_eq!(find_program("x", Some(&tool)), None, "not executable");
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(find_program("x", Some(&tool)), Some(tool));
    }
}
