use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};

use std::cell::OnceCell;

use crate::config::{APP_ID, RESOURCE_BASE_PATH, VERSION};
use crate::library::Library;
use crate::player::Player;
use crate::preferences_dialog::PreferencesDialog;
use crate::tools::Tools;
use crate::window::GatefoldWindow;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct GatefoldApplication {
        pub library: OnceCell<Library>,
        pub player: OnceCell<Player>,
        pub tools: OnceCell<Tools>,
        pub analyzer: OnceCell<crate::analyzer::Analyzer>,
        pub spotify: OnceCell<crate::spotify::Spotify>,
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

            let tools = Tools::new();
            tools.start();
            self.tools.set(tools).expect("set once");

            let library = Library::new();
            if let Err(err) = library.start() {
                tracing::error!("Cannot open the library database: {err}");
            }
            let player = Player::new(&library);
            self.library.set(library).expect("set once");
            let analyzer = crate::analyzer::Analyzer::new();
            analyzer.start();
            // New or changed songs after a scan: analyze them too.
            let to_wake = analyzer.clone();
            self.library
                .get()
                .expect("just set")
                .connect_local("reloaded", false, move |_| {
                    to_wake.wake();
                    None
                });
            // Smart shuffle picks from fresh data.
            let to_reload = player.clone();
            analyzer.connect_local("results-changed", false, move |_| {
                to_reload.reload_similarity();
                None
            });
            let to_reload = player.clone();
            self.library
                .get()
                .expect("just set")
                .connect_local("reloaded", false, move |_| {
                    to_reload.reload_similarity();
                    to_reload.library_reloaded();
                    None
                });
            self.analyzer.set(analyzer).expect("set once");
            self.player.set(player.clone()).expect("set once");
            // Before the queue: a restored Spotify song asks for it.
            let spotify = crate::spotify::Spotify::new();
            self.spotify.set(spotify.clone()).expect("set once");
            player.restore();
            crate::mpris::start(&app, &player);
            spotify.start();
        }

        fn shutdown(&self) {
            if let Some(player) = self.player.get() {
                player.save();
            }
            if let Some(spotify) = self.spotify.get() {
                spotify.shutdown_streaming();
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

    pub fn tools(&self) -> Tools {
        self.imp().tools.get().expect("set in startup()").clone()
    }

    pub fn player(&self) -> Player {
        self.imp().player.get().expect("set in startup()").clone()
    }

    pub fn spotify(&self) -> crate::spotify::Spotify {
        self.imp().spotify.get().expect("set in startup()").clone()
    }

    pub fn analyzer(&self) -> crate::analyzer::Analyzer {
        self.imp().analyzer.get().expect("set in startup()").clone()
    }

    fn setup_actions(&self) {
        let quit = gio::ActionEntry::builder("quit")
            .activate(|app: &Self, _, _| {
                // Closing (instead of quit()) runs close_request, which saves
                // the window state; the app exits with its last window.
                for window in app.windows() {
                    // An open dialog would take the close request itself.
                    if let Some(dialog) = window
                        .downcast_ref::<adw::ApplicationWindow>()
                        .and_then(|w| w.visible_dialog())
                    {
                        dialog.force_close();
                    }
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
        let import_cd = gio::ActionEntry::builder("import-cd")
            .activate(|app: &Self, _, _| app.tools().import_cd())
            .build();
        let continue_import = gio::ActionEntry::builder("continue-import")
            .activate(|app: &Self, _, _| app.tools().continue_import())
            .build();
        let show_page = gio::ActionEntry::builder("tool-show-page")
            .activate(|app: &Self, _, _| app.tools().show_page())
            .build();
        let stop = gio::ActionEntry::builder("tool-stop")
            .activate(|app: &Self, _, _| app.tools().stop_page())
            .build();
        let add_cdimport_folder = gio::ActionEntry::builder("add-cdimport-folder")
            .activate(|app: &Self, _, _| app.add_cdimport_folder())
            .build();
        self.add_action_entries([
            quit,
            preferences,
            shortcuts,
            about,
            rescan,
            import_cd,
            continue_import,
            show_page,
            stop,
            add_cdimport_folder,
        ]);
    }

    fn setup_accels(&self) {
        // Keep in sync with data/resources/ui/shortcuts_dialog.blp.
        let accels: &[(&str, &[&str])] = &[
            ("app.quit", &["<Control>q"]),
            ("app.preferences", &["<Control>comma"]),
            ("app.shortcuts", &["<Control>question"]),
            ("window.close", &["<Control>w"]),
            ("win.toggle-search", &["<Control>f"]),
            ("win.toggle-lyrics", &["<Control>l"]),
            ("win.toggle-queue", &["<Control>u"]),
            ("win.view::albums", &["<Control>1"]),
            ("win.view::artists", &["<Control>2"]),
            ("win.view::playlists", &["<Control>3"]),
        ];
        for (action, keys) in accels {
            self.set_accels_for_action(action, keys);
        }
    }

    /// Library folder agreement (§10.2): cdimport's library folder becomes a
    /// music folder, if the user accepts the offer.
    fn add_cdimport_folder(&self) {
        let Some(dir) = gatefold_tools::cdimport::Config::read().library_dir else {
            return;
        };
        let settings = gio::Settings::new(APP_ID);
        let mut folders = crate::library_folders::load(&settings);
        let home = glib::home_dir();
        match gatefold_core::library::folders::validate_new_folder(&dir, &folders, Some(&home)) {
            Ok(folder) => {
                folders.push(folder);
                if let Err(err) = crate::library_folders::store(&settings, &folders) {
                    tracing::warn!("Cannot save music folders: {err}");
                }
            }
            Err(err) => tracing::info!("cdimport's folder can't be added: {err}"),
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
            // bliss-audio is GPL-3.0-only (the code itself is "or later").
            .license_type(gtk::License::Gpl30Only)
            .build();
        about.present(self.active_window().as_ref());
    }
}

impl Default for GatefoldApplication {
    fn default() -> Self {
        Self::new()
    }
}
