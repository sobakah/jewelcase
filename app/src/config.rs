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
