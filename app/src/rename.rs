//! Jewelcase was called Gatefold until November 2026 (app ID
//! `io.github.sobakah.Gatefold`). On the first start under the new name,
//! what Gatefold kept moves over, so the library, settings and logins stay:
//!
//! - its data, cache and config folders (the library database, covers,
//!   Soloist's data, the logins file) are renamed;
//! - its settings are copied in dconf, while Jewelcase has none of its own;
//! - a Gatefold launcher pinned to GNOME's dash becomes Jewelcase's.
//!
//! Keyring entries are copied where they're read (`spotify/keyring.rs`).
//! A Flatpak can't reach Gatefold's sandbox (another app ID), so it starts
//! afresh.

use std::path::Path;
use std::process::{Command, Stdio};

use gtk::{gio, glib, prelude::*};

use crate::config::{APP_ID, is_flatpak};

/// This build's app ID under the old name (the development build's too).
pub fn former_app_id() -> String {
    APP_ID.replacen("Jewelcase", "Gatefold", 1)
}

/// Moves Gatefold's folders and settings over, once. Runs before anything
/// opens them.
pub fn migrate() {
    if is_flatpak() {
        return;
    }
    let former = former_app_id();
    for base in [
        glib::user_data_dir(),
        glib::user_cache_dir(),
        glib::user_config_dir(),
    ] {
        move_folder(&base.join(&former), &base.join(APP_ID));
    }
    copy_settings(&dconf_path(&former), &dconf_path(APP_ID));
    if !crate::config::is_development() {
        replace_favorite(&format!("{former}.desktop"), &format!("{APP_ID}.desktop"));
    }
}

fn move_folder(old: &Path, new: &Path) {
    if !old.is_dir() || new.exists() {
        return;
    }
    match std::fs::rename(old, new) {
        Ok(()) => tracing::info!("Moved {} to {}", old.display(), new.display()),
        Err(err) => tracing::warn!("Cannot move {} to {}: {err}", old.display(), new.display()),
    }
}

/// `io.github.sobakah.Jewelcase` → `/io/github/sobakah/Jewelcase/`.
fn dconf_path(app_id: &str) -> String {
    format!("/{}/", app_id.replace('.', "/"))
}

/// Gatefold's settings, when the settings live in dconf and Jewelcase has
/// none yet. Through the `dconf` program: Gatefold's schema is gone, so
/// GSettings can't read them.
fn copy_settings(old: &str, new: &str) {
    let backend = gio::SettingsBackend::default();
    if backend.type_().name() != "DConfSettingsBackend" {
        return;
    }
    let dump = |path: &str| -> Option<String> {
        let output = Command::new("dconf")
            .args(["dump", path])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    };
    let Some(settings) = dump(old).and_then(|dump| own_settings(&dump)) else {
        return;
    };
    if dump(new).is_none_or(|dump| own_settings(&dump).is_some()) {
        return;
    }
    let loaded = Command::new("dconf")
        .args(["load", new])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child
                .stdin
                .take()
                .expect("stdin is piped")
                .write_all(settings.as_bytes())?;
            child.wait()
        });
    match loaded {
        Ok(status) if status.success() => tracing::info!("Copied the settings from {old}"),
        Ok(status) => tracing::warn!("Cannot copy the settings from {old}: dconf {status}"),
        Err(err) => tracing::warn!("Cannot copy the settings from {old}: {err}"),
    }
}

/// The app's own keys in a `dconf dump` of its path (the `[/]` section),
/// or `None` without any: the dump also holds the paths below it, and the
/// development build's settings live there (`…/Jewelcase/Devel/`).
fn own_settings(dump: &str) -> Option<String> {
    let mut own = false;
    let mut keys = Vec::new();
    for line in dump.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            own = line == "[/]";
        } else if own && !line.is_empty() {
            keys.push(line);
        }
    }
    (!keys.is_empty()).then(|| format!("[/]\n{}\n", keys.join("\n")))
}

/// GNOME Shell's pinned apps: Gatefold's launcher becomes Jewelcase's, in
/// its place.
fn replace_favorite(old: &str, new: &str) {
    const SCHEMA: &str = "org.gnome.shell";
    let installed = gio::SettingsSchemaSource::default()
        .and_then(|source| source.lookup(SCHEMA, true))
        .is_some_and(|schema| schema.has_key("favorite-apps"));
    if !installed {
        return;
    }
    let shell = gio::Settings::new(SCHEMA);
    let favorites: Vec<String> = shell
        .strv("favorite-apps")
        .iter()
        .map(|s| s.to_string())
        .collect();
    if !favorites.iter().any(|f| f == old) || favorites.iter().any(|f| f == new) {
        return;
    }
    let changed: Vec<&str> = favorites
        .iter()
        .map(|f| if f == old { new } else { f.as_str() })
        .collect();
    if let Err(err) = shell.set_strv("favorite-apps", changed.as_slice()) {
        tracing::warn!("Cannot replace Gatefold's launcher in the dash: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dconf_paths() {
        assert_eq!(
            dconf_path("io.github.sobakah.Gatefold.Devel"),
            "/io/github/sobakah/Gatefold/Devel/"
        );
    }

    #[test]
    fn only_the_apps_own_settings_count() {
        // Only the development build's settings below it: none of its own.
        assert_eq!(own_settings("[Devel]\nvolume=0.5\n"), None);
        assert_eq!(own_settings(""), None);
        assert_eq!(
            own_settings("[/]\nvolume=0.5\ntile-size=180\n\n[Devel]\nvolume=1.0\n"),
            Some("[/]\nvolume=0.5\ntile-size=180\n".to_owned())
        );
    }
}
