//! The `library-folders` setting (CONCEPT.md §3.1).
//!
//! Until the user changes it, the library is the XDG Music folder
//! (`~/Musik`). Once changed, the stored list is used as is, even when empty.

use std::path::PathBuf;

use adw::prelude::*;
use gtk::{gio, glib};

const KEY: &str = "library-folders";

pub fn load(settings: &gio::Settings) -> Vec<PathBuf> {
    if settings.user_value(KEY).is_none() {
        return glib::user_special_dir(glib::UserDirectory::Music)
            .into_iter()
            .collect();
    }
    settings
        .strv(KEY)
        .iter()
        .map(|folder| PathBuf::from(folder.as_str()))
        .collect()
}

/// Folders are validated before they get here, so they're all valid UTF-8.
pub fn store(settings: &gio::Settings, folders: &[PathBuf]) -> Result<(), glib::BoolError> {
    let folders: Vec<&str> = folders
        .iter()
        .filter_map(|folder| folder.to_str())
        .collect();
    settings.set_strv(KEY, folders.as_slice())
}
