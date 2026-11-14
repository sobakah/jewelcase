//! The app icon's disc (the `app-icon` setting, Preferences → App Icon).
//!
//! The dash and the app grid find the icon by the app ID in the icon theme,
//! and `~/.local/share/icons` comes before `/usr/share/icons` there. So a
//! disc other than the installed one is written to
//! `~/.local/share/icons/hicolor/scalable/apps/<app id>.svg`, and the
//! installed one comes back when that file is removed. The discs are
//! bundled as `jewelcase-icon-<name>` (data/resources/icons/scalable/apps),
//! which also puts them in the icon theme for the picker's previews.
//!
//! A Flatpak can't reach the host's icons, so it has no picker.

use std::io;
use std::path::{Path, PathBuf};

use gettextrs::gettext;
use gtk::{gio, glib, prelude::*};

use crate::config::{APP_ID, RESOURCE_BASE_PATH, is_development, is_flatpak};

/// The setting's values, in the picker's order; the first is the installed
/// icon.
pub const ICONS: [&str; 9] = [
    "water-slide",
    "two-slides",
    "waves",
    "ridges",
    "flow",
    "vaporwave",
    "pearl",
    "cross",
    "camouflage",
];

/// In every icon Jewelcase writes (each disc's comment has it), so only
/// those are replaced or removed, never an icon put there by hand.
const MARK: &str = "Preferences → App Icon";

/// GNOME's construction stripes across the case's lower part, as in
/// io.github.sobakah.Jewelcase.Devel.svg (a test keeps them the same).
const STRIPES: &str = r##"
    <!-- development builds: construction stripes across the case's lower part -->
    <clipPath id="case"><rect x="10" y="28" width="94" height="92" rx="8"/></clipPath>
    <g clip-path="url(#case)">
      <rect x="10" y="94" width="94" height="26" fill="#f6d32d"/>
      <path d="M-4 94h12l-26 26h-12zM20 94h12l-26 26h-12zM44 94h12l-26 26h-12zM68 94h12l-26 26h-12zM92 94h12l-26 26h-12zM116 94h12l-26 26h-12z" fill="#241f31"/>
    </g>
"##;

/// The name shown under the disc in the picker.
pub fn title(icon: &str) -> String {
    match icon {
        "two-slides" => gettext("Two Slides"),
        "waves" => gettext("Waves"),
        "ridges" => gettext("Ridges"),
        "flow" => gettext("Flow"),
        "vaporwave" => gettext("Vaporwave"),
        "pearl" => gettext("Pearl"),
        "cross" => gettext("Pink Cross"),
        "camouflage" => gettext("Camouflage"),
        _ => gettext("Water Slide"),
    }
}

/// The icon name of a disc's preview. Not the app ID for the first: a
/// chosen disc covers that one.
pub fn preview_name(icon: &str) -> String {
    format!("jewelcase-icon-{icon}")
}

/// Whether the picker is offered.
pub fn available() -> bool {
    !is_flatpak()
}

/// Installs the chosen disc now and whenever the setting changes.
pub fn start() {
    if !available() {
        return;
    }
    let settings = gio::Settings::new(APP_ID);
    apply(&settings);
    settings.connect_changed(Some("app-icon"), |settings, _| apply(settings));
    // Kept for the program's lifetime, like its handler.
    std::mem::forget(settings);
}

fn apply(settings: &gio::Settings) {
    let icon = settings.string("app-icon");
    let svg = match bundled(&icon) {
        Ok(svg) => svg,
        Err(err) => {
            tracing::warn!("No app icon {icon}: {err}");
            return;
        }
    };
    let svg = svg.map(|svg| {
        if is_development() {
            with_stripes(&svg)
        } else {
            svg
        }
    });
    let path = icon_path(&glib::user_data_dir(), APP_ID);
    match install(&path, svg.as_deref()) {
        Ok(Change::Unchanged) => {}
        Ok(change) => tracing::info!("App icon {icon}: {change:?} {}", path.display()),
        Err(err) => tracing::warn!("Cannot set the app icon at {}: {err}", path.display()),
    }
}

/// A disc's icon from the bundle; none for the installed one.
fn bundled(icon: &str) -> Result<Option<String>, glib::Error> {
    if icon == ICONS[0] {
        return Ok(None);
    }
    let path = format!("{RESOURCE_BASE_PATH}icons/scalable/apps/jewelcase-icon-{icon}.svg");
    let bytes = gio::resources_lookup_data(&path, gio::ResourceLookupFlags::NONE)?;
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

/// `<data dir>/icons/hicolor/scalable/apps/<app id>.svg`.
pub fn icon_path(data_dir: &Path, app_id: &str) -> PathBuf {
    data_dir
        .join("icons/hicolor/scalable/apps")
        .join(format!("{app_id}.svg"))
}

/// The disc with the development build's stripes, drawn over the case like
/// in the installed Devel icon (inside its scaled group, under nothing).
pub fn with_stripes(svg: &str) -> String {
    match svg.rfind("</g>") {
        Some(end) => format!("{}{STRIPES}  {}", svg[..end].trim_end(), &svg[end..]),
        None => svg.to_owned(),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Change {
    Written,
    Removed,
    Unchanged,
    /// An icon there that Jewelcase didn't write: left alone.
    NotOurs,
}

/// Writes `svg` as the icon at `path`, or removes Jewelcase's icon there
/// for `None`.
fn install(path: &Path, svg: Option<&str>) -> io::Result<Change> {
    let current = match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => return Err(err),
    };
    if current.as_ref().is_some_and(|text| !text.contains(MARK)) {
        return Ok(Change::NotOurs);
    }
    match (svg, current) {
        (None, None) => Ok(Change::Unchanged),
        (None, Some(_)) => {
            std::fs::remove_file(path)?;
            Ok(Change::Removed)
        }
        (Some(svg), Some(current)) if current == svg => Ok(Change::Unchanged),
        (Some(svg), _) => {
            let dir = path.parent().expect("an icon path has a folder");
            std::fs::create_dir_all(dir)?;
            // Renamed into place, so the icon theme never reads half a file.
            let partial = path.with_extension("svg.partial");
            std::fs::write(&partial, svg)?;
            std::fs::rename(&partial, path)?;
            Ok(Change::Written)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
    }

    fn disc(icon: &str) -> String {
        let path = repo().join(format!(
            "data/resources/icons/scalable/apps/jewelcase-icon-{icon}.svg"
        ));
        std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
    }

    /// Without comments and whitespace.
    fn drawing(svg: &str) -> String {
        let mut out = String::new();
        let mut rest = svg;
        while let Some(start) = rest.find("<!--") {
            out.push_str(&rest[..start]);
            let end = rest[start..].find("-->").expect("a closed comment");
            rest = &rest[start + end + 3..];
        }
        out.push_str(rest);
        out.split_whitespace().collect()
    }

    #[test]
    fn every_disc_is_bundled_and_marked() {
        let xml =
            std::fs::read_to_string(repo().join("data/resources/resources.gresource.xml")).unwrap();
        let schema =
            std::fs::read_to_string(repo().join("data/io.github.sobakah.Jewelcase.gschema.xml.in"))
                .unwrap();
        for icon in ICONS {
            assert!(
                schema.contains(&format!("<choice value=\"{icon}\"/>")),
                "{icon} is not a choice of app-icon"
            );
            let file = format!("icons/scalable/apps/{}.svg", preview_name(icon));
            assert!(
                xml.contains(&format!(">{file}<")) || xml.contains(&format!("alias=\"{file}\"")),
                "{icon} is not in resources.gresource.xml"
            );
            if icon == ICONS[0] {
                continue;
            }
            let svg = disc(icon);
            assert!(svg.contains(MARK), "{icon} lacks the mark");
            // Flatpak's and GNOME's checks find an SVG by `<svg` near its start.
            assert!(svg.find("<svg").is_some_and(|at| at < 100), "{icon}");
        }
    }

    #[test]
    fn the_development_stripes_match_the_installed_devel_icon() {
        let apps = repo().join("data/icons/hicolor/scalable/apps");
        let release =
            std::fs::read_to_string(apps.join("io.github.sobakah.Jewelcase.svg")).unwrap();
        let devel =
            std::fs::read_to_string(apps.join("io.github.sobakah.Jewelcase.Devel.svg")).unwrap();
        assert_eq!(drawing(&with_stripes(&release)), drawing(&devel));
    }

    #[test]
    fn a_disc_is_written_replaced_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = icon_path(dir.path(), "org.example.App");
        assert!(path.ends_with("icons/hicolor/scalable/apps/org.example.App.svg"));

        let waves = disc("waves");
        assert_eq!(install(&path, Some(&waves)).unwrap(), Change::Written);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), waves);
        assert_eq!(install(&path, Some(&waves)).unwrap(), Change::Unchanged);

        let pearl = with_stripes(&disc("pearl"));
        assert_eq!(install(&path, Some(&pearl)).unwrap(), Change::Written);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), pearl);

        assert_eq!(install(&path, None).unwrap(), Change::Removed);
        assert!(!path.exists());
        assert_eq!(install(&path, None).unwrap(), Change::Unchanged);
        // Nothing left over from writing.
        let left: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().collect();
        assert!(left.is_empty());
    }

    #[test]
    fn an_icon_put_there_by_hand_stays() {
        let dir = tempfile::tempdir().unwrap();
        let path = icon_path(dir.path(), "org.example.App");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let own = "<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
        std::fs::write(&path, own).unwrap();

        assert_eq!(
            install(&path, Some(&disc("ridges"))).unwrap(),
            Change::NotOurs
        );
        assert_eq!(install(&path, None).unwrap(), Change::NotOurs);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), own);
    }
}
