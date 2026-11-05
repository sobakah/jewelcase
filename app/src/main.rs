mod application;
mod config;
mod library_folders;
mod preferences_dialog;
mod widgets;
mod window;

use adw::prelude::*;
use gettextrs::{LocaleCategory, bind_textdomain_codeset, bindtextdomain, gettext, textdomain};
use gtk::{gio, glib};
use tracing_subscriber::EnvFilter;

use crate::application::GatefoldApplication;
use crate::config::{GETTEXT_PACKAGE, LOCALEDIR};

static GRESOURCE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/gatefold.gresource"));

fn main() -> glib::ExitCode {
    init_i18n();
    init_logging();
    register_resources();

    glib::set_application_name(&gettext("Gatefold"));
    GatefoldApplication::new().run()
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

/// Log level comes from `GATEFOLD_LOG` (e.g. `GATEFOLD_LOG=debug`).
fn init_logging() {
    let default = if config::is_development() {
        "gatefold=debug,gatefold_core=debug"
    } else {
        "warn"
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("GATEFOLD_LOG").unwrap_or_else(|_| EnvFilter::new(default)),
        )
        .init();
}

fn register_resources() {
    assert!(
        !GRESOURCE.is_empty(),
        "built without resources; build Gatefold with Meson (see README.md)"
    );
    let resource = gio::Resource::from_data(&glib::Bytes::from_static(GRESOURCE))
        .expect("embedded resource bundle is valid");
    gio::resources_register(&resource);
}
