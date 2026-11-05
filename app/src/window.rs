use std::cell::OnceCell;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib};

use crate::application::GatefoldApplication;
use crate::config::{self, APP_ID};
use crate::widgets::{MiniPlayer, PlayerView};

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/sobakah/Gatefold/ui/window.ui")]
    pub struct GatefoldWindow {
        #[template_child]
        pub bottom_sheet: TemplateChild<adw::BottomSheet>,
        #[template_child]
        pub stack: TemplateChild<adw::ViewStack>,
        #[template_child]
        pub search_bar: TemplateChild<gtk::SearchBar>,
        pub settings: OnceCell<gio::Settings>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for GatefoldWindow {
        const NAME: &'static str = "GatefoldWindow";
        type Type = super::GatefoldWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            MiniPlayer::ensure_type();
            PlayerView::ensure_type();
            klass.bind_template();

            klass.install_action("win.open-player", None, |win, _, _| {
                win.imp().bottom_sheet.set_open(true);
            });
            klass.install_action("win.close-player", None, |win, _, _| {
                win.imp().bottom_sheet.set_open(false);
            });
            klass.install_action("win.toggle-search", None, |win, _, _| {
                let bar = &win.imp().search_bar;
                bar.set_search_mode(!bar.is_search_mode());
            });
            klass.install_action(
                "win.view",
                Some(glib::VariantTy::STRING),
                |win, _, param| {
                    if let Some(name) = param.and_then(|p| p.get::<String>()) {
                        win.imp().bottom_sheet.set_open(false);
                        win.imp().stack.set_visible_child_name(&name);
                    }
                },
            );
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for GatefoldWindow {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            if config::is_development() {
                obj.add_css_class("devel");
            }

            let settings = gio::Settings::new(APP_ID);
            settings
                .bind("library-view", &*self.stack, "visible-child-name")
                .build();
            self.settings.set(settings).expect("settings are set once");
            obj.load_window_state();

            self.search_bar.set_key_capture_widget(Some(&*obj));
        }
    }

    impl WidgetImpl for GatefoldWindow {}

    impl WindowImpl for GatefoldWindow {
        fn close_request(&self) -> glib::Propagation {
            if let Err(err) = self.obj().save_window_state() {
                tracing::warn!("Failed to save window state: {err}");
            }
            self.parent_close_request()
        }
    }

    impl ApplicationWindowImpl for GatefoldWindow {}
    impl AdwApplicationWindowImpl for GatefoldWindow {}
}

glib::wrapper! {
    pub struct GatefoldWindow(ObjectSubclass<imp::GatefoldWindow>)
        @extends gtk::Widget, gtk::Window, gtk::ApplicationWindow, adw::ApplicationWindow,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable,
                    gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl GatefoldWindow {
    pub fn new(app: &GatefoldApplication) -> Self {
        glib::Object::builder().property("application", app).build()
    }

    fn settings(&self) -> &gio::Settings {
        self.imp()
            .settings
            .get()
            .expect("settings are set in constructed()")
    }

    fn load_window_state(&self) {
        let settings = self.settings();
        self.set_default_size(settings.int("window-width"), settings.int("window-height"));
        if settings.boolean("is-maximized") {
            self.maximize();
        }
    }

    fn save_window_state(&self) -> Result<(), glib::BoolError> {
        let settings = self.settings();
        let (width, height) = self.default_size();
        settings.set_int("window-width", width)?;
        settings.set_int("window-height", height)?;
        settings.set_boolean("is-maximized", self.is_maximized())?;
        Ok(())
    }
}
