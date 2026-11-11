mod analyzer;
mod application;
mod config;
mod covers;
mod format;
mod frame_stats;
mod icons;
mod library;
mod library_folders;
mod lrclib;
mod metadata;
mod mpris;
mod objects;
mod player;
mod preferences_dialog;
mod rename;
mod romanizer;
mod spotify;
mod style;
mod theme;
mod tools;
mod updates;
mod widgets;
mod window;

use adw::prelude::*;
use gettextrs::{LocaleCategory, bind_textdomain_codeset, bindtextdomain, gettext, textdomain};
use gtk::{gio, glib};
use tracing_subscriber::EnvFilter;

use crate::application::JewelcaseApplication;
use crate::config::{GETTEXT_PACKAGE, LOCALEDIR};

static GRESOURCE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/jewelcase.gresource"));

fn main() -> glib::ExitCode {
    init_i18n();
    init_logging();
    // Gatefold's library, settings and folders, before anything opens them.
    rename::migrate();
    register_resources();

    glib::set_application_name(&gettext("Jewelcase"));
    JewelcaseApplication::new().run()
}

fn init_i18n() {
    // SAFETY: called first thing in main, before any other thread exists.
    unsafe {
        gettextrs::setlocale(LocaleCategory::LcAll, "");
    }
    let result = bindtextdomain(GETTEXT_PACKAGE, LOCALEDIR)
        .and_then(|_| bind_textdomain_codeset(GETTEXT_PACKAGE, "UTF-8"))
        .and_then(|_| textdomain(GETTEXT_PACKAGE));
    if let Err(err) = result {
        eprintln!("Translations unavailable: {err}");
    }
}

/// Log level comes from `JEWELCASE_LOG` (e.g. `JEWELCASE_LOG=debug`).
fn init_logging() {
    // librespot's own messages say why streaming fails (CONCEPT.md §9.4).
    let default = if config::is_development() {
        "jewelcase=debug,jewelcase_core=debug,jewelcase_stream=debug,librespot_core=info,\
         librespot_playback=info,librespot_audio=info,librespot_metadata=info"
    } else {
        // Soloist's decisions too: they explain a song cut short. And
        // the hourly count of Spotify requests: it explains a block.
        "warn,jewelcase::spotify::soloist=info,jewelcase_spotify=info"
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("JEWELCASE_LOG").unwrap_or_else(|_| EnvFilter::new(default)),
        )
        .init();
}

fn register_resources() {
    assert!(
        !GRESOURCE.is_empty(),
        "built without resources; build Jewelcase with Meson (see README.md)"
    );
    let resource = gio::Resource::from_data(&glib::Bytes::from_static(GRESOURCE))
        .expect("embedded resource bundle is valid");
    gio::resources_register(&resource);
}
