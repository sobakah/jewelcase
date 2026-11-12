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

pub const APP_ID: &str = from_env!("JEWELCASE_APP_ID", "io.github.sobakah.Jewelcase.Devel");
pub const VERSION: &str = from_env!("JEWELCASE_VERSION", env!("CARGO_PKG_VERSION"));
/// `default` or `development` (Meson option `profile`).
pub const PROFILE: &str = from_env!("JEWELCASE_PROFILE", "development");
pub const LOCALEDIR: &str = from_env!("JEWELCASE_LOCALEDIR", "/usr/share/locale");
pub const GETTEXT_PACKAGE: &str = from_env!("JEWELCASE_GETTEXT_PACKAGE", "jewelcase");

/// Fixed for both profiles, so the Devel build loads the same resources.
pub const RESOURCE_BASE_PATH: &str = "/io/github/sobakah/Jewelcase/";

pub fn is_development() -> bool {
    PROFILE == "development"
}

/// Running as a Flatpak: host programs and their configs are out of reach.
pub fn is_flatpak() -> bool {
    std::path::Path::new("/.flatpak-info").exists()
}

/// Whether Jewelcase looks for its own new releases (Meson option
/// `update_check`; a Flatpak updates through its repository).
pub fn update_check() -> bool {
    from_env!("JEWELCASE_UPDATE_CHECK", "true") == "true"
}

/// lyrembed's last alias list, for when it can't be asked.
pub fn aliases_cache_path() -> std::path::PathBuf {
    gtk::glib::user_data_dir()
        .join(APP_ID)
        .join("lyrembed-aliases.txt")
}

/// `~/.local/share/<app id>/library.sqlite` (CONCEPT.md §12).
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
fn cover_dir() -> std::path::PathBuf {
    gtk::glib::user_cache_dir().join(APP_ID).join("covers")
}

/// `~/.local/share/<app id>/playlist-covers/`: playlist covers the user
/// chose. Like custom portraits they exist nowhere else, so they live with
/// the data, not in the cache.
fn playlist_cover_dir() -> std::path::PathBuf {
    gtk::glib::user_data_dir()
        .join(APP_ID)
        .join("playlist-covers")
}

/// Album covers, Spotify images and playlist mosaics in the cache folder;
/// chosen playlist covers in the data folder.
pub fn cover_cache() -> jewelcase_core::library::covers::CoverCache {
    jewelcase_core::library::covers::CoverCache::new(cover_dir())
        .with_chosen_dir(playlist_cover_dir())
}
