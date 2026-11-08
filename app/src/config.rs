//! Build configuration. Meson passes these as environment variables to Cargo
//! (see `app/meson.build`); the fallbacks keep plain `cargo check` working.

macro_rules! from_env {
    ($name:literal, $default:expr) => {
        match option_env!($name) {
            Some(value) => value,
            None => $default,
        }
    };
}

pub const APP_ID: &str = from_env!("GATEFOLD_APP_ID", "io.github.sobakah.Gatefold.Devel");
pub const VERSION: &str = from_env!("GATEFOLD_VERSION", env!("CARGO_PKG_VERSION"));
/// `default` or `development` (Meson option `profile`).
pub const PROFILE: &str = from_env!("GATEFOLD_PROFILE", "development");
pub const LOCALEDIR: &str = from_env!("GATEFOLD_LOCALEDIR", "/usr/share/locale");
pub const GETTEXT_PACKAGE: &str = from_env!("GATEFOLD_GETTEXT_PACKAGE", "gatefold");

/// Fixed for both profiles, so the Devel build loads the same resources.
pub const RESOURCE_BASE_PATH: &str = "/io/github/sobakah/Gatefold/";

pub fn is_development() -> bool {
    PROFILE == "development"
}

/// Whether Gatefold looks for its own new releases (Meson option
/// `update_check`; a Flatpak updates through its repository).
pub fn update_check() -> bool {
    from_env!("GATEFOLD_UPDATE_CHECK", "true") == "true"
}

/// `~/.local/share/<app id>/library.sqlite` (CONCEPT.md §12).
/// lyrembed's last alias list, for when it can't be asked.
pub fn aliases_cache_path() -> std::path::PathBuf {
    gtk::glib::user_data_dir()
        .join(APP_ID)
        .join("lyrembed-aliases.txt")
}

pub fn database_path() -> std::path::PathBuf {
    gtk::glib::user_data_dir()
        .join(APP_ID)
        .join("library.sqlite")
}

/// `~/.local/share/<app id>/artists/`: artist portraits (CONCEPT.md §8).
/// Custom ones must survive cache cleanups, so they live with the data.
pub fn portrait_dir() -> std::path::PathBuf {
    gtk::glib::user_data_dir().join(APP_ID).join("artists")
}

/// `~/.cache/<app id>/covers/` (CONCEPT.md §4.3).
pub fn cover_dir() -> std::path::PathBuf {
    gtk::glib::user_cache_dir().join(APP_ID).join("covers")
}
