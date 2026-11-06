use std::cell::{OnceCell, RefCell};
use std::cmp::Ordering;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gio, glib};

use crate::application::GatefoldApplication;
use crate::config::{self, APP_ID};
use crate::objects::{AlbumObject, ArtistObject};
use crate::widgets::{AlbumPanel, AlbumTile, ArtistPage, ArtistTile, MiniPlayer, PlayerView};

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/sobakah/Gatefold/ui/window.ui")]
    pub struct GatefoldWindow {
        #[template_child]
        pub toast_overlay: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub bottom_sheet: TemplateChild<adw::BottomSheet>,
        #[template_child]
        pub stack: TemplateChild<adw::ViewStack>,
        #[template_child]
        pub sort_button: TemplateChild<gtk::MenuButton>,
        #[template_child]
        pub search_bar: TemplateChild<gtk::SearchBar>,
        #[template_child]
        pub search_entry: TemplateChild<gtk::SearchEntry>,
        #[template_child]
        pub scan_banner: TemplateChild<adw::Banner>,
        #[template_child]
        pub albums_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub split_view: TemplateChild<adw::OverlaySplitView>,
        #[template_child]
        pub album_grid: TemplateChild<gtk::GridView>,
        #[template_child]
        pub album_panel: TemplateChild<AlbumPanel>,
        #[template_child]
        pub artists_nav: TemplateChild<adw::NavigationView>,
        #[template_child]
        pub artists_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub artist_grid: TemplateChild<gtk::GridView>,
        pub artist_filter: OnceCell<gtk::CustomFilter>,
        pub artists_filtered: OnceCell<gtk::FilterListModel>,
        pub settings: OnceCell<gio::Settings>,
        pub filter: OnceCell<gtk::CustomFilter>,
        pub sorter: OnceCell<gtk::CustomSorter>,
        pub filtered: OnceCell<gtk::FilterListModel>,
        pub search: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for GatefoldWindow {
        const NAME: &'static str = "GatefoldWindow";
        type Type = super::GatefoldWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            MiniPlayer::ensure_type();
            PlayerView::ensure_type();
            AlbumPanel::ensure_type();
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
                "win.show-artist",
                Some(glib::VariantTy::INT64),
                |win, _, param| {
                    if let Some(id) = param.and_then(|p| p.get::<i64>()) {
                        win.show_artist(id);
                    }
                },
            );
            klass.install_action(
                "win.online-metadata",
                Some(glib::VariantTy::BOOLEAN),
                |win, _, param| {
                    let allow = param.and_then(|p| p.get::<bool>()).unwrap_or(false);
                    let settings = win.settings();
                    let _ = settings.set_boolean("online-metadata", allow);
                    let _ = settings.set_boolean("online-metadata-asked", true);
                },
            );
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
                obj.setup_screenshot();
            }

            let settings = gio::Settings::new(APP_ID);
            settings
                .bind("library-view", &*self.stack, "visible-child-name")
                .build();
            obj.add_action(&settings.create_action("album-sort"));
            self.settings.set(settings).expect("settings are set once");
            obj.load_window_state();

            let player = GatefoldApplication::get().player();
            obj.insert_action_group("player", Some(player.actions()));
            obj.setup_shortcuts();
            obj.setup_albums();
            obj.setup_artists();
            obj.setup_messages();

            self.search_bar.set_key_capture_widget(Some(&*obj));

            // Sorting only applies to the album grid.
            let sort_button = self.sort_button.get();
            self.stack
                .bind_property("visible-child-name", &sort_button, "visible")
                .transform_to(|_, name: Option<String>| Some(name.as_deref() == Some("albums")))
                .sync_create()
                .build();
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

    /// Playback keys run in the bubble phase, so text entries and focused
    /// buttons get Space first (CONCEPT.md §11).
    fn setup_shortcuts(&self) {
        let controller = gtk::ShortcutController::new();
        controller.set_propagation_phase(gtk::PropagationPhase::Bubble);
        for (accel, action) in [
            ("space", "player.play-pause"),
            ("<Control>Left", "player.previous"),
            ("<Control>Right", "player.next"),
        ] {
            controller.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string(accel),
                Some(gtk::NamedAction::new(action)),
            ));
        }
        self.add_controller(controller);
    }

    fn setup_albums(&self) {
        let imp = self.imp();
        let library = GatefoldApplication::get().library();

        let filter = gtk::CustomFilter::new(glib::clone!(
            #[weak(rename_to = window)]
            self,
            #[upgrade_or]
            true,
            move |item| {
                let album = item.downcast_ref::<AlbumObject>().expect("albums");
                window.matches_search(album)
            }
        ));
        let sorter = gtk::CustomSorter::new(glib::clone!(
            #[weak(rename_to = window)]
            self,
            #[upgrade_or]
            gtk::Ordering::Equal,
            move |a, b| {
                let a = a.downcast_ref::<AlbumObject>().expect("albums");
                let b = b.downcast_ref::<AlbumObject>().expect("albums");
                window.compare_albums(a, b).into()
            }
        ));
        let filtered =
            gtk::FilterListModel::new(Some(library.albums().clone()), Some(filter.clone()));
        let sorted = gtk::SortListModel::new(Some(filtered.clone()), Some(sorter.clone()));
        imp.album_grid
            .set_model(Some(&gtk::NoSelection::new(Some(sorted))));

        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
            item.set_child(Some(&AlbumTile::new()));
        });
        factory.connect_bind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
            if let (Some(tile), Some(album)) = (
                item.child().and_downcast::<AlbumTile>(),
                item.item().and_downcast::<AlbumObject>(),
            ) {
                tile.bind(&album);
                item.set_accessible_label(&album.album().title);
            }
        });
        imp.album_grid.set_factory(Some(&factory));
        imp.album_grid.connect_activate(|grid, position| {
            let Some(album) = grid
                .model()
                .and_then(|m| m.item(position))
                .and_downcast::<AlbumObject>()
            else {
                return;
            };
            if let Some(window) = grid.root().and_downcast::<GatefoldWindow>() {
                window.show_album(&album);
            }
        });

        imp.album_panel.connect_local(
            "close",
            false,
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                #[upgrade_or]
                None,
                move |_| {
                    window.imp().split_view.set_show_sidebar(false);
                    None
                }
            ),
        );

        imp.search_entry.connect_search_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |entry| {
                window.imp().search.replace(entry.text().to_lowercase());
                window.filter_changed();
                if let Some(filter) = window.imp().artist_filter.get() {
                    filter.changed(gtk::FilterChange::Different);
                }
            }
        ));
        self.settings().connect_changed(
            Some("album-sort"),
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_, _| {
                    if let Some(sorter) = window.imp().sorter.get() {
                        sorter.changed(gtk::SorterChange::Different);
                    }
                }
            ),
        );

        library.connect_local(
            "reloaded",
            false,
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                #[upgrade_or]
                None,
                move |_| {
                    window.library_reloaded();
                    None
                }
            ),
        );
        library.connect_scanning_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |library| {
                window.update_banner(library);
                window.update_albums_page();
            }
        ));
        library.connect_scan_status_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |library| window.update_banner(library)
        ));
        filtered.connect_items_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _, _, _| window.update_albums_page()
        ));

        imp.filter.set(filter).expect("set once");
        imp.sorter.set(sorter).expect("set once");
        imp.filtered.set(filtered).expect("set once");
        self.update_albums_page();
        self.update_banner(&library);
    }

    fn setup_artists(&self) {
        let imp = self.imp();
        let library = GatefoldApplication::get().library();

        let filter = gtk::CustomFilter::new(glib::clone!(
            #[weak(rename_to = window)]
            self,
            #[upgrade_or]
            true,
            move |item| {
                let artist = item.downcast_ref::<ArtistObject>().expect("artists");
                let search = window.imp().search.borrow();
                search.is_empty()
                    || artist
                        .artist()
                        .name
                        .to_lowercase()
                        .contains(search.as_str())
            }
        ));
        let filtered =
            gtk::FilterListModel::new(Some(library.artists().clone()), Some(filter.clone()));
        imp.artist_grid
            .set_model(Some(&gtk::NoSelection::new(Some(filtered.clone()))));

        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
            item.set_child(Some(&ArtistTile::new()));
        });
        factory.connect_bind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
            if let (Some(tile), Some(artist)) = (
                item.child().and_downcast::<ArtistTile>(),
                item.item().and_downcast::<ArtistObject>(),
            ) {
                tile.bind(&artist);
                item.set_accessible_label(&artist.artist().name);
                // Lookups run lazily, for artists that are shown (§8).
                GatefoldApplication::get().library().request_lookup(&artist);
            }
        });
        imp.artist_grid.set_factory(Some(&factory));
        imp.artist_grid.connect_activate(|grid, position| {
            let Some(artist) = grid
                .model()
                .and_then(|m| m.item(position))
                .and_downcast::<ArtistObject>()
            else {
                return;
            };
            if let Some(window) = grid.root().and_downcast::<GatefoldWindow>() {
                window.show_artist(artist.id());
            }
        });

        filtered.connect_items_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _, _, _| window.update_artists_page()
        ));
        self.settings().connect_changed(
            Some("online-metadata-asked"),
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_, _| window.update_artists_page()
            ),
        );
        imp.artist_filter.set(filter).expect("set once");
        imp.artists_filtered.set(filtered).expect("set once");
        self.update_artists_page();
    }

    fn update_artists_page(&self) {
        let imp = self.imp();
        let library = GatefoldApplication::get().library();
        let shown = imp.artists_filtered.get().map_or(0, |f| f.n_items());
        let page = if !self.settings().boolean("online-metadata-asked") {
            "ask"
        } else if library.artists().n_items() == 0 {
            "empty"
        } else if shown == 0 {
            "no-results"
        } else {
            "artists"
        };
        imp.artists_stack.set_visible_child_name(page);
    }

    /// Opens an artist's page in the Artists view.
    pub fn show_artist(&self, artist_id: i64) {
        let imp = self.imp();
        imp.bottom_sheet.set_open(false);
        imp.stack.set_visible_child_name("artists");
        if imp
            .artists_nav
            .visible_page()
            .and_downcast::<ArtistPage>()
            .is_some_and(|page| page.artist_id() == artist_id)
        {
            return;
        }
        imp.artists_nav.pop_to_tag("artists");
        let page = ArtistPage::new(artist_id);
        imp.split_view
            .bind_property("collapsed", &page.split_view(), "collapsed")
            .sync_create()
            .build();
        imp.artists_nav.push(&page);
    }

    fn setup_messages(&self) {
        let app = GatefoldApplication::get();
        for source in [
            app.library().upcast::<glib::Object>(),
            app.player().upcast(),
        ] {
            source.connect_local(
                "message",
                false,
                glib::clone!(
                    #[weak(rename_to = window)]
                    self,
                    #[upgrade_or]
                    None,
                    move |values| {
                        let message = values[1].get::<String>().ok()?;
                        window
                            .imp()
                            .toast_overlay
                            .add_toast(adw::Toast::new(&message));
                        None
                    }
                ),
            );
        }
    }

    fn show_album(&self, album: &AlbumObject) {
        let imp = self.imp();
        imp.album_panel.set_album(album);
        imp.split_view.set_show_sidebar(true);
    }

    fn matches_search(&self, album: &AlbumObject) -> bool {
        let search = self.imp().search.borrow();
        if search.is_empty() {
            return true;
        }
        album.versions().iter().any(|v| {
            v.title.to_lowercase().contains(search.as_str())
                || v.artist_credit.to_lowercase().contains(search.as_str())
        })
    }

    fn compare_albums(&self, a: &AlbumObject, b: &AlbumObject) -> Ordering {
        let (a, b) = (a.album(), b.album());
        let by_artist = || {
            a.artist_sort
                .cmp(&b.artist_sort)
                .then(a.year.cmp(&b.year))
                .then(a.sort_title.cmp(&b.sort_title))
        };
        match self.settings().string("album-sort").as_str() {
            "title" => a.sort_title.cmp(&b.sort_title).then_with(by_artist),
            // Newest first.
            "year" => b.year.cmp(&a.year).then_with(by_artist),
            "added" => b.added.cmp(&a.added).then_with(by_artist),
            _ => by_artist(),
        }
    }

    fn filter_changed(&self) {
        if let Some(filter) = self.imp().filter.get() {
            filter.changed(gtk::FilterChange::Different);
        }
    }

    fn library_reloaded(&self) {
        let imp = self.imp();
        // Leave the page of an artist that's no longer in the library.
        let library = GatefoldApplication::get().library();
        if let Some(page) = imp.artists_nav.visible_page().and_downcast::<ArtistPage>()
            && library.artist(page.artist_id()).is_none()
        {
            imp.artists_nav.pop_to_tag("artists");
        }
        self.update_artists_page();
        // Show the reloaded version of the open album, or close the panel.
        if let Some(open) = imp.album_panel.album() {
            let library = GatefoldApplication::get().library();
            let albums = library.albums();
            let reloaded = (0..albums.n_items())
                .filter_map(|i| albums.item(i).and_downcast::<AlbumObject>())
                .find(|a| a.group_key() == open.group_key());
            match reloaded {
                Some(album) => imp.album_panel.set_album(&album),
                None => imp.split_view.set_show_sidebar(false),
            }
        }
        self.update_albums_page();
    }

    fn update_albums_page(&self) {
        let imp = self.imp();
        let library = GatefoldApplication::get().library();
        let shown = imp.filtered.get().map_or(0, |f| f.n_items());
        let page = if library.albums().n_items() == 0 && library.scanning() {
            "scanning"
        } else if library.albums().n_items() == 0 {
            "empty"
        } else if shown == 0 {
            "no-results"
        } else {
            "albums"
        };
        imp.albums_stack.set_visible_child_name(page);
    }

    fn update_banner(&self, library: &crate::library::Library) {
        let banner = &self.imp().scan_banner;
        banner.set_title(&library.scan_status());
        banner.set_revealed(library.scanning());
    }

    /// Development aid: `GATEFOLD_SCREENSHOT=out.png` renders the window to
    /// a PNG after `GATEFOLD_SCREENSHOT_DELAY` seconds (default 8) and quits.
    /// The steps below run after 2 seconds.
    /// `GATEFOLD_SCREENSHOT_ACTIONS` lists actions to run first, e.g.
    /// `win.open-player`; `GATEFOLD_SCREENSHOT_ALBUM=N` (or a title) opens that album
    /// and `GATEFOLD_SCREENSHOT_PLAY=1` plays it; `GATEFOLD_SCREENSHOT_ARTIST=name`
    /// opens an artist page. Without `GATEFOLD_SCREENSHOT`,
    /// only that album step runs.
    fn setup_screenshot(&self) {
        // Without GATEFOLD_SCREENSHOT, only the album and actions steps run
        // (for playback tests); the app keeps running.
        let path = std::env::var_os("GATEFOLD_SCREENSHOT");
        if path.is_none() && std::env::var_os("GATEFOLD_SCREENSHOT_ALBUM").is_none() {
            return;
        }
        let delay = std::env::var("GATEFOLD_SCREENSHOT_DELAY")
            .ok()
            .and_then(|d| d.parse().ok())
            .unwrap_or(8);
        let actions = std::env::var("GATEFOLD_SCREENSHOT_ACTIONS").unwrap_or_default();
        let window = self.downgrade();
        let first_step = delay.clamp(1, 2);
        glib::timeout_add_seconds_local_once(first_step, move || {
            let Some(window) = window.upgrade() else {
                return;
            };
            for action in actions.split_whitespace() {
                let (name, target) = match action.split_once("::") {
                    // GVariant text ("true", "42"), else a plain string.
                    Some((name, target)) => (
                        name,
                        Some(
                            glib::Variant::parse(None, target)
                                .unwrap_or_else(|_| target.to_variant()),
                        ),
                    ),
                    None => (action, None),
                };
                if let Err(err) = WidgetExt::activate_action(&window, name, target.as_ref()) {
                    tracing::warn!("Screenshot action {name}: {err}");
                }
            }
            // An index into the grid, or an album title.
            let album = std::env::var("GATEFOLD_SCREENSHOT_ALBUM")
                .ok()
                .and_then(|wanted| {
                    let model = window.imp().album_grid.model()?;
                    (0..model.n_items())
                        .filter_map(|i| model.item(i).and_downcast::<AlbumObject>())
                        .enumerate()
                        .find(|(i, a)| wanted == i.to_string() || wanted == a.album().title)
                        .map(|(_, a)| a)
                });
            let artist = std::env::var("GATEFOLD_SCREENSHOT_ARTIST")
                .ok()
                .and_then(|name| {
                    let library = GatefoldApplication::get().library();
                    let artists = library.artists();
                    (0..artists.n_items())
                        .filter_map(|i| artists.item(i).and_downcast::<ArtistObject>())
                        .find(|a| a.artist().name == name)
                });
            if let Some(artist) = artist {
                window.show_artist(artist.id());
            }
            if let Some(album) = album {
                window.show_album(&album);
                if std::env::var_os("GATEFOLD_SCREENSHOT_PLAY").is_some() {
                    GatefoldApplication::get()
                        .player()
                        .play_album(album.album().id, None, false);
                }
            }
            let Some(path) = path else { return };
            let window = window.downgrade();
            glib::timeout_add_seconds_local_once(
                delay.saturating_sub(first_step).max(1),
                move || {
                    let Some(window) = window.upgrade() else {
                        return;
                    };
                    let paintable = gtk::WidgetPaintable::new(Some(&window));
                    let (w, h) = (window.width(), window.height());
                    let snapshot = gtk::Snapshot::new();
                    paintable.snapshot(&snapshot, f64::from(w), f64::from(h));
                    let node = snapshot.to_node();
                    let renderer = window.native().and_then(|n| n.renderer());
                    if let (Some(node), Some(renderer)) = (node, renderer) {
                        let texture = renderer.render_texture(&node, None);
                        match texture.save_to_png(&path) {
                            Ok(()) => tracing::info!("Screenshot saved to {}", path.display()),
                            Err(err) => tracing::warn!("Screenshot failed: {err}"),
                        }
                    }
                    if let Some(app) = window.application() {
                        app.activate_action("quit", None);
                    }
                },
            );
        });
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
