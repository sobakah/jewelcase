//! Light or dark (the `style` setting): Gatefold follows the desktop, or
//! stays light or dark.
//!
//! libadwaita learns the desktop's choice through the settings portal.
//! Some desktops (Xfce, MATE, Cinnamon, window managers) don't tell it
//! there and set a dark GTK theme instead (e.g. `Adwaita-dark`, or
//! `GTK_THEME=Adwaita:dark`); that one is followed too.

use adw::prelude::*;
use gtk::{gio, glib};

/// The setting's values, in the order of the preference row.
pub const STYLES: [&str; 3] = ["follow", "light", "dark"];

/// Applies the setting now and whenever it or the desktop's theme changes.
pub fn start() {
    let settings = gio::Settings::new(crate::config::APP_ID);
    apply(&settings);
    settings.connect_changed(Some("style"), |settings, _| apply(settings));
    if let Some(gtk_settings) = gtk::Settings::default() {
        gtk_settings.connect_gtk_theme_name_notify(glib::clone!(
            #[strong]
            settings,
            move |_| apply(&settings)
        ));
    }
    // Kept for the program's lifetime, like the handlers above.
    std::mem::forget(settings);
}

fn apply(settings: &gio::Settings) {
    let scheme = match settings.string("style").as_str() {
        "light" => adw::ColorScheme::ForceLight,
        "dark" => adw::ColorScheme::ForceDark,
        // Dark unless the desktop asks for light: a desktop that says
        // nothing then gets the dark its theme shows.
        _ if theme_is_dark() => adw::ColorScheme::PreferDark,
        _ => adw::ColorScheme::Default,
    };
    adw::StyleManager::default().set_color_scheme(scheme);
}

/// Whether the GTK theme the desktop set is a dark one.
fn theme_is_dark() -> bool {
    let name = std::env::var("GTK_THEME").ok().or_else(|| {
        gtk::Settings::default()
            .and_then(|s| s.gtk_theme_name())
            .map(String::from)
    });
    name.is_some_and(|name| name_is_dark(&name))
}

fn name_is_dark(name: &str) -> bool {
    let name = name.to_lowercase();
    name.ends_with(":dark") || name.split(['-', '_', ' ', ':']).any(|part| part == "dark")
}

#[cfg(test)]
mod tests {
    use super::name_is_dark;

    #[test]
    fn dark_theme_names() {
        for name in [
            "Adwaita-dark",
            "Adwaita:dark",
            "Arc-Dark",
            "Greybird_dark",
            "Mint-Y-Dark-Aqua",
        ] {
            assert!(name_is_dark(name), "{name}");
        }
        for name in [
            "Adwaita",
            "Breeze",
            "Greybird",
            "Darkly-ish-light",
            "Adwaita:light",
        ] {
            assert!(!name_is_dark(name), "{name}");
        }
    }
}
