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
//! GNOME Shell looks at the theme folder's (`icons/hicolor`) modification
//! time before an icon lookup, at most every 5 s, and reloads every icon when
//! it changed; a file written deeper down doesn't change it, so it's moved
//! forward after each change (as `touch` after installing icons does).
//!
//! A Flatpak reaches the host's `~/.local/share/icons` only when allowed
//! (`--filesystem=xdg-data/icons:create`, as the test Flatpak is); without
//! it there's no picker.

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

/// The discs in the picker: the chosen one first, then the others in
/// [`ICONS`]'s order.
pub fn picker_order(chosen: &str) -> Vec<&'static str> {
    let chosen = ICONS
        .iter()
        .find(|icon| **icon == chosen)
        .unwrap_or(&ICONS[0]);
    std::iter::once(*chosen)
        .chain(ICONS.iter().copied().filter(|icon| icon != chosen))
        .collect()
}

/// The icon name of a disc's preview. Not the app ID for the first: a
/// chosen disc covers that one.
pub fn preview_name(icon: &str) -> String {
    format!("jewelcase-icon-{icon}")
}

/// Whether the picker is offered.
pub fn available() -> bool {
    !is_flatpak() || icons_home().is_dir()
}

/// The host's data folder: in a Flatpak, `glib::user_data_dir()` is the
/// sandbox's own (`~/.var/app/<app id>/data`), which the dash doesn't read.
fn data_home() -> PathBuf {
    if !is_flatpak() {
        return glib::user_data_dir();
    }
    std::env::var_os("HOST_XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| glib::home_dir().join(".local/share"))
}

/// `<data home>/icons`, where the icon goes.
fn icons_home() -> PathBuf {
    data_home().join("icons")
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
    let data_dir = data_home();
    let path = icon_path(&data_dir, APP_ID);
    match set_icon(&data_dir, APP_ID, svg.as_deref()) {
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

/// Installs (or removes, for `None`) the icon and tells the icon theme.
fn set_icon(data_dir: &Path, app_id: &str, svg: Option<&str>) -> io::Result<Change> {
    let change = install(&icon_path(data_dir, app_id), svg)?;
    if matches!(change, Change::Written | Change::Removed) {
        touch_theme(&data_dir.join("icons/hicolor"))?;
    }
    Ok(change)
}

/// Moves the theme folder's modification time forward: to now, or a second
/// on when it is there already (the shell compares whole seconds).
fn touch_theme(dir: &Path) -> io::Result<()> {
    let folder = std::fs::File::open(dir)?;
    let before = folder.metadata()?.modified()?;
    let now = std::time::SystemTime::now();
    let whole = |time: std::time::SystemTime| {
        time.duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_secs())
            .unwrap_or(0)
    };
    let to = if whole(now) > whole(before) {
        now
    } else {
        before + std::time::Duration::from_secs(1)
    };
    folder.set_modified(to)
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
    fn the_chosen_disc_comes_first_in_the_picker() {
        assert_eq!(picker_order("water-slide"), ICONS);
        let order = picker_order("pearl");
        assert_eq!(order[0], "pearl");
        assert_eq!(order.len(), ICONS.len());
        let rest: Vec<_> = ICONS
            .iter()
            .copied()
            .filter(|icon| *icon != "pearl")
            .collect();
        assert_eq!(order[1..], rest);
        // A value from a newer version: the installed one first.
        assert_eq!(picker_order("unknown"), ICONS);
    }

    #[test]
    fn the_theme_folder_is_touched_so_the_shell_reloads_the_icon() {
        let dir = tempfile::tempdir().unwrap();
        let theme = dir.path().join("icons/hicolor");
        let modified = || std::fs::metadata(&theme).unwrap().modified().unwrap();

        assert_eq!(
            set_icon(dir.path(), "org.example.App", Some(&disc("waves"))).unwrap(),
            Change::Written
        );
        // Written twice within a second still moves on (whole seconds).
        let first = modified();
        set_icon(dir.path(), "org.example.App", Some(&disc("flow"))).unwrap();
        let second = modified();
        assert!(second.duration_since(first).unwrap().as_secs() >= 1);
        // Back to the installed icon: the theme changes again.
        assert_eq!(
            set_icon(dir.path(), "org.example.App", None).unwrap(),
            Change::Removed
        );
        assert!(modified().duration_since(second).unwrap().as_secs() >= 1);
        // Nothing changed: left alone.
        let last = modified();
        set_icon(dir.path(), "org.example.App", None).unwrap();
        assert_eq!(modified(), last);
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
