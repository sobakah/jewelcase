use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};

use std::cell::OnceCell;

use crate::config::{APP_ID, RESOURCE_BASE_PATH, VERSION};
use crate::library::Library;
use crate::player::Player;
use crate::preferences_dialog::PreferencesDialog;
use crate::window::GatefoldWindow;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct GatefoldApplication {
        pub library: OnceCell<Library>,
        pub player: OnceCell<Player>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for GatefoldApplication {
        const NAME: &'static str = "GatefoldApplication";
        type Type = super::GatefoldApplication;
        type ParentType = adw::Application;
    }

    impl ObjectImpl for GatefoldApplication {}

    impl ApplicationImpl for GatefoldApplication {
        fn startup(&self) {
            self.parent_startup();
            let app = self.obj();
            app.setup_actions();
            app.setup_accels();

            let library = Library::new();
            if let Err(err) = library.start() {
                tracing::error!("Cannot open the library database: {err}");
            }
            let player = Player::new(&library);
            self.library.set(library).expect("set once");
            self.player.set(player.clone()).expect("set once");
            player.restore();
            crate::mpris::start(&app, &player);
        }

        fn shutdown(&self) {
            if let Some(player) = self.player.get() {
                player.save();
            }
            self.parent_shutdown();
        }

        fn activate(&self) {
            self.parent_activate();
            let app = self.obj();
            match app.active_window() {
                Some(window) => window.present(),
                None => GatefoldWindow::new(&app).present(),
            }
        }
    }

    impl GtkApplicationImpl for GatefoldApplication {}
    impl AdwApplicationImpl for GatefoldApplication {}
}

glib::wrapper! {
    pub struct GatefoldApplication(ObjectSubclass<imp::GatefoldApplication>)
        @extends gio::Application, gtk::Application, adw::Application,
        @implements gio::ActionGroup, gio::ActionMap;
}

impl GatefoldApplication {
    pub fn new() -> Self {
        glib::Object::builder()
            .property("application-id", APP_ID)
            .property("resource-base-path", RESOURCE_BASE_PATH)
            .build()
    }

    /// The running application.
    pub fn get() -> Self {
        gio::Application::default()
            .and_downcast()
            .expect("the application is a GatefoldApplication")
    }

    pub fn library(&self) -> Library {
        self.imp().library.get().expect("set in startup()").clone()
    }

    pub fn player(&self) -> Player {
        self.imp().player.get().expect("set in startup()").clone()
    }

    fn setup_actions(&self) {
        let quit = gio::ActionEntry::builder("quit")
            .activate(|app: &Self, _, _| {
                // Closing (instead of quit()) runs close_request, which saves
                // the window state; the app exits with its last window.
                for window in app.windows() {
                    window.close();
                }
            })
            .build();
        let preferences = gio::ActionEntry::builder("preferences")
            .activate(|app: &Self, _, _| {
                PreferencesDialog::new().present(app.active_window().as_ref());
            })
            .build();
        let shortcuts = gio::ActionEntry::builder("shortcuts")
            .activate(|app: &Self, _, _| app.show_shortcuts())
            .build();
        let about = gio::ActionEntry::builder("about")
            .activate(|app: &Self, _, _| app.show_about())
            .build();
        let rescan = gio::ActionEntry::builder("rescan")
            .activate(|app: &Self, _, _| app.library().rescan())
            .build();
        self.add_action_entries([quit, preferences, shortcuts, about, rescan]);
    }

    fn setup_accels(&self) {
        // Keep in sync with data/resources/ui/shortcuts_dialog.blp.
        let accels: &[(&str, &[&str])] = &[
            ("app.quit", &["<Control>q"]),
            ("app.preferences", &["<Control>comma"]),
            ("app.shortcuts", &["<Control>question"]),
            ("window.close", &["<Control>w"]),
            ("win.toggle-search", &["<Control>f"]),
            ("win.view::albums", &["<Control>1"]),
            ("win.view::artists", &["<Control>2"]),
            ("win.view::playlists", &["<Control>3"]),
        ];
        for (action, keys) in accels {
            self.set_accels_for_action(action, keys);
        }
    }

    fn show_shortcuts(&self) {
        let builder =
            gtk::Builder::from_resource(&format!("{RESOURCE_BASE_PATH}ui/shortcuts_dialog.ui"));
        let dialog: adw::Dialog = builder
            .object("shortcuts_dialog")
            .expect("shortcuts_dialog.ui defines shortcuts_dialog");
        dialog.present(self.active_window().as_ref());
    }

    fn show_about(&self) {
        let about = adw::AboutDialog::builder()
            .application_name(gettext("Gatefold"))
            .application_icon(APP_ID)
            .developer_name("sobakah")
            .version(VERSION)
            .website("https://github.com/sobakah/gatefold")
            .issue_url("https://github.com/sobakah/gatefold/issues")
            .license_type(gtk::License::Gpl30)
            .build();
        about.present(self.active_window().as_ref());
    }
}

impl Default for GatefoldApplication {
    fn default() -> Self {
        Self::new()
    }
}
