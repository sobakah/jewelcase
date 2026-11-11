//! Light or dark (the `style` setting): Jewelcase follows the desktop, or
//! stays light or dark.
//!
//! libadwaita learns the desktop's choice through the settings portal.
//! GNOME and KDE tell it there. Other desktops (Xfce, MATE, Cinnamon,
//! window managers) often don't, and set a dark GTK theme instead
//! (e.g. `Adwaita-dark`, `Mint-Y-Dark`, or `GTK_THEME=Adwaita:dark`); there
//! that one is followed too. GTK's own `gtk-theme-name` can't tell:
//! libadwaita replaces it with its empty theme, so the name is read where
//! these desktops keep it.

use std::path::{Path, PathBuf};

use adw::prelude::*;
use gtk::{gio, glib};

/// The setting's values, in the order of the preference row.
pub const STYLES: [&str; 3] = ["follow", "light", "dark"];

/// Where desktops keep the GTK theme in GSettings (Cinnamon, MATE, then
/// the GNOME schema other desktops share).
const THEME_SCHEMAS: [&str; 3] = [
    "org.cinnamon.desktop.interface",
    "org.mate.interface",
    "org.gnome.desktop.interface",
];

/// Applies the setting now and whenever it or the desktop's theme changes.
pub fn start() {
    let settings = gio::Settings::new(crate::config::APP_ID);
    apply(&settings);
    settings.connect_changed(Some("style"), |settings, _| apply(settings));
    if !desktop_reports_scheme() {
        for schema in theme_settings() {
            schema.connect_changed(
                Some("gtk-theme"),
                glib::clone!(
                    #[strong]
                    settings,
                    move |_, _| apply(&settings)
                ),
            );
            // Kept for the program's lifetime, like its handler.
            std::mem::forget(schema);
        }
    }
    std::mem::forget(settings);
}

fn apply(settings: &gio::Settings) {
    let scheme = match settings.string("style").as_str() {
        "light" => adw::ColorScheme::ForceLight,
        "dark" => adw::ColorScheme::ForceDark,
        // Dark unless the desktop asks for light: a desktop that says
        // nothing then gets the dark its theme shows.
        _ if !desktop_reports_scheme() && theme_is_dark() => adw::ColorScheme::PreferDark,
        _ => adw::ColorScheme::Default,
    };
    adw::StyleManager::default().set_color_scheme(scheme);
}

/// GNOME and KDE report light or dark through the portal; their GTK theme
/// may say otherwise (GNOME's legacy theme setting) and mustn't win.
fn desktop_reports_scheme() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|desktops| {
        desktops
            .split(':')
            .any(|d| d.eq_ignore_ascii_case("GNOME") || d.eq_ignore_ascii_case("KDE"))
    })
}

/// The installed schemas of `THEME_SCHEMAS`.
fn theme_settings() -> Vec<gio::Settings> {
    let Some(source) = gio::SettingsSchemaSource::default() else {
        return Vec::new();
    };
    THEME_SCHEMAS
        .iter()
        .filter(|id| source.lookup(id, true).is_some())
        .map(|id| gio::Settings::new(id))
        .collect()
}

/// Whether the GTK theme the desktop set is a dark one.
fn theme_is_dark() -> bool {
    if let Ok(theme) = std::env::var("GTK_THEME") {
        return name_is_dark(&theme);
    }
    let config = glib::user_config_dir();
    let mut names: Vec<String> = theme_settings()
        .iter()
        .map(|s| s.string("gtk-theme").to_string())
        .collect();
    names.extend(xfce_theme(&config));
    let ini = settings_ini(&config);
    names.extend(ini.theme);
    ini.prefer_dark || names.iter().any(|name| name_is_dark(name))
}

/// Xfce's theme (`xsettings`' `/Net/ThemeName`).
fn xfce_theme(config: &Path) -> Option<String> {
    let path = config.join("xfce4/xfconf/xfce-perchannel-xml/xsettings.xml");
    let text = std::fs::read_to_string(path).ok()?;
    xfce_theme_from(&text)
}

fn xfce_theme_from(text: &str) -> Option<String> {
    let line = text
        .lines()
        .find(|line| line.contains("name=\"ThemeName\""))?;
    let (_, value) = line.split_once("value=\"")?;
    value.split('"').next().map(str::to_owned)
}

#[derive(Debug, Default, PartialEq, Eq)]
struct SettingsIni {
    theme: Option<String>,
    prefer_dark: bool,
}

/// GTK's `settings.ini` (window managers, nwg-look, lxappearance): GTK 4's,
/// else GTK 3's.
fn settings_ini(config: &Path) -> SettingsIni {
    ["gtk-4.0", "gtk-3.0"]
        .iter()
        .map(|dir| PathBuf::from(config).join(dir).join("settings.ini"))
        .find_map(|path| std::fs::read_to_string(path).ok())
        .map(|text| settings_ini_from(&text))
        .unwrap_or_default()
}

fn settings_ini_from(text: &str) -> SettingsIni {
    let mut ini = SettingsIni::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        match key.trim() {
            "gtk-theme-name" => ini.theme = Some(value.to_owned()),
            "gtk-application-prefer-dark-theme" => {
                ini.prefer_dark = value == "1" || value.eq_ignore_ascii_case("true");
            }
            _ => {}
        }
    }
    ini
}

fn name_is_dark(name: &str) -> bool {
    let name = name.to_lowercase();
    name.ends_with(":dark") || name.split(['-', '_', ' ', ':']).any(|part| part == "dark")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_theme_files() {
        let xml = r#"<channel name="xsettings" version="1.0">
  <property name="Net" type="empty">
    <property name="ThemeName" type="string" value="Greybird-dark"/>"#;
        assert_eq!(xfce_theme_from(xml).as_deref(), Some("Greybird-dark"));
        let ini = settings_ini_from(
            "[Settings]\ngtk-theme-name=Arc-Dark\ngtk-application-prefer-dark-theme=1\n",
        );
        assert_eq!(ini.theme.as_deref(), Some("Arc-Dark"));
        assert!(ini.prefer_dark);
        assert!(!settings_ini_from("[Settings]\ngtk-theme-name=Adwaita\n").prefer_dark);
    }

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
