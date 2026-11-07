use std::cell::{OnceCell, RefCell};
use std::cmp::Ordering;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};

use crate::application::GatefoldApplication;
use crate::config::{self, APP_ID};
use crate::objects::PlaylistObject;
use crate::objects::{AlbumObject, ArtistObject};
use crate::widgets::{
    AlbumPanel, AlbumTile, ArtistPage, ArtistTile, MiniPlayer, PlayerView, PlaylistPanel,
    PlaylistTile, SearchView, panel_resizer, playlist_picker,
};

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
        pub player_view: TemplateChild<PlayerView>,
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
        pub tool_revealer: TemplateChild<gtk::Revealer>,
        #[template_child]
        pub tool_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub cd_banner: TemplateChild<adw::Banner>,
        #[template_child]
        pub unfinished_banner: TemplateChild<adw::Banner>,
        #[template_child]
        pub albums_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub split_view: TemplateChild<adw::OverlaySplitView>,
        #[template_child]
        pub album_grid: TemplateChild<crate::widgets::SectionedGrid>,
        #[template_child]
        pub album_panel: TemplateChild<AlbumPanel>,
        #[template_child]
        pub mini_player: TemplateChild<MiniPlayer>,
        #[template_child]
        pub library_toolbar: TemplateChild<adw::ToolbarView>,
        #[template_child]
        pub analysis_revealer: TemplateChild<gtk::Revealer>,
        #[template_child]
        pub analysis_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub artist_split: TemplateChild<adw::OverlaySplitView>,
        #[template_child]
        pub artist_holder: TemplateChild<adw::Bin>,
        #[template_child]
        pub artists_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub artist_grid: TemplateChild<crate::widgets::SectionedGrid>,
        #[template_child]
        pub library_overlay: TemplateChild<gtk::Overlay>,
        #[template_child]
        pub search_dim: TemplateChild<gtk::Revealer>,
        #[template_child]
        pub search_revealer: TemplateChild<gtk::Revealer>,
        #[template_child]
        pub search_view: TemplateChild<SearchView>,
        #[template_child]
        pub playlist_button: TemplateChild<gtk::MenuButton>,
        #[template_child]
        pub playlists_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub playlist_split: TemplateChild<adw::OverlaySplitView>,
        #[template_child]
        pub playlist_grid: TemplateChild<gtk::FlowBox>,
        #[template_child]
        pub followed_grid: TemplateChild<gtk::FlowBox>,
        #[template_child]
        pub followed_toggle: TemplateChild<gtk::Button>,
        #[template_child]
        pub followed_content: TemplateChild<adw::ButtonContent>,
        #[template_child]
        pub followed_revealer: TemplateChild<gtk::Revealer>,
        #[template_child]
        pub playlist_panel: TemplateChild<PlaylistPanel>,
        pub settings: OnceCell<gio::Settings>,
        pub sorter: OnceCell<gtk::CustomSorter>,
        /// The album genre shown, or all (None).
        pub genre_filter: RefCell<Option<String>>,
        pub genre_filter_model: OnceCell<gtk::CustomFilter>,
        /// "Stop and Quit" was chosen while a tool ran: close once it ended.
        pub closing_after_tool: std::cell::Cell<bool>,
        pub genre_menu: gio::Menu,
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
            PlaylistPanel::ensure_type();
            SearchView::ensure_type();
            crate::widgets::SectionedGrid::ensure_type();
            klass.bind_template();

            klass.install_action("win.open-player", None, |win, _, _| {
                win.imp().bottom_sheet.set_open(true);
            });
            klass.install_action(
                "win.analysis-choice",
                Some(glib::VariantTy::STRING),
                |win, _, param| {
                    let Some(mode) = param.and_then(|p| p.get::<String>()) else {
                        return;
                    };
                    let settings = win.settings();
                    let _ = settings.set_string("analysis-mode", &mode);
                    let _ = settings.set_boolean("analysis-asked", true);
                    win.imp().analysis_revealer.set_reveal_child(false);
                },
            );
            klass.install_action("win.queue-card", None, |win, _, _| {
                win.imp().mini_player.show_queue();
            });
            klass.install_action("win.close-player", None, |win, _, _| {
                win.imp().bottom_sheet.set_open(false);
            });
            klass.install_action("win.toggle-lyrics", None, |win, _, _| {
                win.imp().bottom_sheet.set_open(true);
                win.imp().player_view.toggle_lyrics();
            });
            klass.install_action("win.toggle-queue", None, |win, _, _| {
                win.imp().bottom_sheet.set_open(true);
                win.imp().player_view.toggle_queue();
            });
            klass.install_action("win.toggle-search", None, |win, _, _| {
                let bar = &win.imp().search_bar;
                bar.set_search_mode(!bar.is_search_mode());
            });
            // Spotify albums and artists by Spotify ID (§9.8).
            klass.install_action(
                "win.show-spotify-album",
                Some(glib::VariantTy::STRING),
                |win, _, param| {
                    let spotify = GatefoldApplication::get().spotify();
                    if let Some(album) = param
                        .and_then(|p| p.get::<String>())
                        .and_then(|id| spotify.stored_album(&id))
                    {
                        win.show_spotify_album(&album);
                    }
                },
            );
            klass.install_action(
                "win.show-spotify-artist",
                Some(glib::VariantTy::STRING),
                |win, _, param| {
                    if let Some(id) = param.and_then(|p| p.get::<String>()) {
                        win.show_spotify_artist(&id);
                    }
                },
            );
            klass.install_action(
                "win.show-album",
                Some(glib::VariantTy::INT64),
                |win, _, param| {
                    let library = GatefoldApplication::get().library();
                    if let Some(album) = param
                        .and_then(|p| p.get::<i64>())
                        .and_then(|id| library.album_group(id))
                    {
                        win.imp().bottom_sheet.set_open(false);
                        win.imp().stack.set_visible_child_name("albums");
                        win.show_album(&album);
                    }
                },
            );
            klass.install_action(
                "win.show-playlist",
                Some(glib::VariantTy::INT64),
                |win, _, param| {
                    let library = GatefoldApplication::get().library();
                    if let Some(playlist) = param
                        .and_then(|p| p.get::<i64>())
                        .and_then(|id| library.playlist(id))
                    {
                        win.show_playlist(&playlist);
                    }
                },
            );
            klass.install_action_async("win.new-playlist", None, |win, _, _| async move {
                let name = playlist_picker::ask_name(
                    &win,
                    &gettextrs::gettext("New Playlist"),
                    "",
                    &gettextrs::gettext("_Create"),
                )
                .await;
                if let Some(name) = name {
                    let library = GatefoldApplication::get().library();
                    if let Some(playlist) = library
                        .create_playlist(&name, &[])
                        .and_then(|id| library.playlist(id))
                    {
                        win.show_playlist(&playlist);
                    }
                }
            });
            klass.install_action_async("win.import-playlist", None, |win, _, _| async move {
                win.import_playlist().await;
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
            }

            let settings = gio::Settings::new(APP_ID);
            settings
                .bind("library-view", &*self.stack, "visible-child-name")
                .build();
            // A fixed view to start with, else the last one.
            let start = settings.string("start-view");
            if start != "last" {
                self.stack.set_visible_child_name(&start);
            }
            obj.add_action(&settings.create_action("album-sort"));
            self.settings.set(settings).expect("settings are set once");
            obj.load_window_state();
            if config::is_development() {
                obj.setup_screenshot();
            }

            let player = GatefoldApplication::get().player();
            obj.insert_action_group("player", Some(player.actions()));
            obj.setup_shortcuts();
            obj.setup_albums();
            obj.setup_artists();
            obj.setup_playlists();
            obj.setup_artist_overlay();
            obj.setup_panel_resizers();
            obj.setup_floating_player();
            obj.setup_analysis_prompt();
            obj.setup_messages();
            obj.setup_tools();

            self.search_bar.set_key_capture_widget(Some(&*obj));
            obj.setup_search();
            obj.setup_spotify_shelves();

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
        let sorted = gtk::SortListModel::new(Some(library.albums().clone()), Some(sorter.clone()));
        // Genre filter (win.album-genre, from the sort menu).
        let filter = gtk::CustomFilter::new(glib::clone!(
            #[weak(rename_to = window)]
            self,
            #[upgrade_or]
            true,
            move |item| {
                let Some(genre) = window.imp().genre_filter.borrow().clone() else {
                    return true;
                };
                let album = item.downcast_ref::<AlbumObject>().expect("albums");
                GatefoldApplication::get()
                    .library()
                    .genres_of(album)
                    .iter()
                    .any(|g| g.eq_ignore_ascii_case(&genre))
            }
        ));
        let filtered = gtk::FilterListModel::new(Some(sorted), Some(filter.clone()));
        let _ = imp.genre_filter_model.set(filter);
        self.setup_genre_menu();
        imp.album_grid.setup(
            &filtered,
            // A tile's smallest width (120) and its padding.
            138,
            "album-grid",
            || AlbumTile::new().upcast(),
            |tile, item| {
                if let (Some(tile), Some(album)) = (
                    tile.downcast_ref::<AlbumTile>(),
                    item.downcast_ref::<AlbumObject>(),
                ) {
                    tile.bind(album);
                }
            },
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |item| {
                    if let Some(album) = item.downcast_ref::<AlbumObject>() {
                        window.show_album(album);
                    }
                }
            ),
        );

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
            move |entry| window.search_changed(&entry.text())
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
        library.albums().connect_items_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_, _, _, _| window.update_albums_page()
        ));

        imp.sorter.set(sorter).expect("set once");
        self.update_albums_page();
        self.update_banner(&library);
    }

    fn setup_artists(&self) {
        let imp = self.imp();
        let library = GatefoldApplication::get().library();

        imp.artist_grid.setup(
            library.artists(),
            // A tile's smallest width (120) and its padding.
            144,
            "artist-grid",
            || ArtistTile::new().upcast(),
            |tile, item| {
                if let (Some(tile), Some(artist)) = (
                    tile.downcast_ref::<ArtistTile>(),
                    item.downcast_ref::<ArtistObject>(),
                ) {
                    tile.bind(artist);
                    // Lookups run lazily, for artists that are shown (§8).
                    GatefoldApplication::get().library().request_lookup(artist);
                }
            },
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |item| {
                    if let Some(artist) = item.downcast_ref::<ArtistObject>() {
                        window.show_artist(artist.id());
                    }
                }
            ),
        );

        library.artists().connect_items_changed(glib::clone!(
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
        self.update_artists_page();
    }

    fn setup_playlists(&self) {
        let imp = self.imp();
        let library = GatefoldApplication::get().library();
        // Own playlists, and below them, collapsed by default, followed
        // Spotify playlists (Spotify doesn't show their songs, §9.5).
        let settings = self.settings();
        let readable = gtk::FilterListModel::new(
            Some(library.playlists().clone()),
            Some(gtk::CustomFilter::new(|item| {
                item.downcast_ref::<PlaylistObject>()
                    .is_some_and(|p| p.playlist().readable)
            })),
        );
        let followed = gtk::FilterListModel::new(
            Some(library.playlists().clone()),
            Some(gtk::CustomFilter::new(|item| {
                item.downcast_ref::<PlaylistObject>()
                    .is_some_and(|p| !p.playlist().readable)
            })),
        );
        let tile = |item: &glib::Object| -> gtk::Widget {
            let tile = PlaylistTile::new();
            if let Some(playlist) = item.downcast_ref::<PlaylistObject>() {
                tile.bind(playlist);
                tile.update_property(&[gtk::accessible::Property::Label(
                    &playlist.playlist().name,
                )]);
            }
            tile.upcast()
        };
        imp.playlist_grid.bind_model(Some(&readable), tile);
        imp.followed_grid.bind_model(Some(&followed), tile);
        for (grid, model) in [
            (imp.playlist_grid.get(), readable.clone()),
            (imp.followed_grid.get(), followed.clone()),
        ] {
            grid.connect_child_activated(move |grid, child| {
                let Some(playlist) = model
                    .item(child.index() as u32)
                    .and_downcast::<PlaylistObject>()
                else {
                    return;
                };
                if let Some(window) = grid.root().and_downcast::<GatefoldWindow>() {
                    window.show_playlist(&playlist);
                }
            });
        }
        let toggle = imp.followed_toggle.get();
        let content = imp.followed_content.get();
        let revealer = imp.followed_revealer.get();
        let update_toggle = glib::clone!(
            #[weak]
            toggle,
            #[weak]
            content,
            #[weak]
            revealer,
            #[strong]
            settings,
            move |followed: &gtk::FilterListModel| {
                let count = followed.n_items();
                let open = settings.boolean("show-followed-playlists");
                toggle.set_visible(count > 0);
                // Translators: {} is a number of playlists.
                content.set_label(
                    &gettext("Followed Playlists ({})").replace("{}", &count.to_string()),
                );
                content.set_icon_name(if open {
                    "pan-down-symbolic"
                } else {
                    "pan-end-symbolic"
                });
                revealer.set_reveal_child(open && count > 0);
            }
        );
        update_toggle(&followed);
        let on_items = update_toggle.clone();
        followed.connect_items_changed(move |followed, _, _, _| on_items(followed));
        let on_setting = followed.clone();
        settings.connect_changed(Some("show-followed-playlists"), move |_, _| {
            update_toggle(&on_setting);
        });
        toggle.connect_clicked(glib::clone!(
            #[strong]
            settings,
            move |_| {
                let open = settings.boolean("show-followed-playlists");
                let _ = settings.set_boolean("show-followed-playlists", !open);
            }
        ));
        imp.playlist_panel.connect_local(
            "close",
            false,
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                #[upgrade_or]
                None,
                move |_| {
                    window.imp().playlist_split.set_show_sidebar(false);
                    None
                }
            ),
        );
        library.playlists().connect_items_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |store, _, _, _| {
                let page = if store.n_items() == 0 {
                    "empty"
                } else {
                    "playlists"
                };
                window.imp().playlists_stack.set_visible_child_name(page);
            }
        ));
        let page = if library.playlists().n_items() == 0 {
            "empty"
        } else {
            "playlists"
        };
        imp.playlists_stack.set_visible_child_name(page);
        // The "+" menu belongs to the Playlists view.
        let button = imp.playlist_button.get();
        imp.stack
            .bind_property("visible-child-name", &button, "visible")
            .transform_to(|_, name: Option<String>| Some(name.as_deref() == Some("playlists")))
            .sync_create()
            .build();
    }

    pub fn show_playlist(&self, playlist: &PlaylistObject) {
        let imp = self.imp();
        imp.bottom_sheet.set_open(false);
        imp.search_bar.set_search_mode(false);
        imp.stack.set_visible_child_name("playlists");
        imp.playlist_panel.set_playlist(playlist);
        reveal_sidebar(&imp.playlist_split);
    }

    async fn import_playlist(&self) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some(&gettextrs::gettext("Playlists (M3U, XSPF)")));
        for pattern in ["*.m3u", "*.m3u8", "*.xspf", "*.M3U", "*.M3U8", "*.XSPF"] {
            filter.add_pattern(pattern);
        }
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title(gettextrs::gettext("Import Playlist"))
            .modal(true)
            .filters(&filters)
            .build();
        let Ok(file) = dialog.open_future(Some(self)).await else {
            return;
        };
        let Some(path) = file.path() else { return };
        let library = GatefoldApplication::get().library();
        let message = match library.import_playlist(path).await {
            Ok((id, matched, total)) => {
                if let Some(playlist) = library.playlist(id) {
                    self.show_playlist(&playlist);
                }
                // Translators: {matched} of {total} playlist entries.
                gettextrs::gettext("Imported: {matched} of {total} songs are in your library")
                    .replace("{matched}", &matched.to_string())
                    .replace("{total}", &total.to_string())
            }
            Err(err) => {
                tracing::warn!("Playlist import failed: {err}");
                gettextrs::gettext("The playlist couldn't be imported")
            }
        };
        imp_toast(self, &message);
    }

    /// The saved albums and followed artists on Spotify that aren't in the
    /// library: shelves above the grids (§9.8).
    fn setup_spotify_shelves(&self) {
        let imp = self.imp();
        // Translators: {} is a number of albums.
        imp.album_grid
            .setup_spotify(&gettext("Saved on Spotify ({})"), "show-spotify-albums");
        // Translators: {} is a number of artists.
        imp.artist_grid
            .setup_spotify(&gettext("Followed on Spotify ({})"), "show-spotify-artists");
        let app = GatefoldApplication::get();
        let spotify = app.spotify();
        spotify.connect_local(
            "library-changed",
            false,
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                #[upgrade_or]
                None,
                move |_| {
                    window.refresh_spotify_shelves();
                    None
                }
            ),
        );
        spotify.connect_status_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.refresh_spotify_shelves()
        ));
        app.library().connect_local(
            "reloaded",
            false,
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                #[upgrade_or]
                None,
                move |_| {
                    window.refresh_spotify_shelves();
                    None
                }
            ),
        );
        self.refresh_spotify_shelves();
    }

    fn refresh_spotify_shelves(&self) {
        use crate::widgets::ShelfItem;
        let imp = self.imp();
        let app = GatefoldApplication::get();
        let spotify = app.spotify();
        let library = app.library();
        if !spotify.is_connected() {
            imp.album_grid.set_spotify(Vec::new(), false, |_| {});
            imp.artist_grid.set_spotify(Vec::new(), false, |_| {});
            return;
        }
        let needs = spotify.needs_library_permission();
        let albums: Vec<_> = spotify
            .saved_albums()
            .into_iter()
            .filter(|album| library.local_album_for(album).is_none())
            .collect();
        let items = albums
            .iter()
            .map(|album| ShelfItem {
                cover_key: album.cover_key(),
                title: album.name.clone(),
                subtitle: album.artist.clone(),
                artist: false,
            })
            .collect();
        imp.album_grid.set_spotify(
            items,
            needs,
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |index| {
                    if let Some(album) = albums.get(index) {
                        window.show_spotify_album(album);
                    }
                }
            ),
        );
        // Followed artists the library doesn't have (by Spotify ID from the
        // online lookup, or by name).
        let local = library.artists();
        let local: Vec<(String, Option<String>)> = (0..local.n_items())
            .filter_map(|i| local.item(i).and_downcast::<ArtistObject>())
            .map(|a| {
                let info = a.artist();
                (info.name.to_lowercase(), info.meta.spotify_id.clone())
            })
            .collect();
        let artists: Vec<_> = spotify
            .followed_artists()
            .into_iter()
            .filter(|artist| {
                !local.iter().any(|(name, id)| {
                    id.as_deref() == Some(artist.spotify_id.as_str())
                        || *name == artist.name.to_lowercase()
                })
            })
            .collect();
        let items = artists
            .iter()
            .map(|artist| ShelfItem {
                cover_key: artist.cover_key(),
                title: artist.name.clone(),
                subtitle: artist.genres.first().cloned().unwrap_or_default(),
                artist: true,
            })
            .collect();
        imp.artist_grid.set_spotify(
            items,
            needs,
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |index| {
                    if let Some(artist) = artists.get(index) {
                        window.show_spotify_artist(&artist.spotify_id);
                    }
                }
            ),
        );
    }

    /// A Spotify album in the album panel (§9.8).
    pub fn show_spotify_album(&self, album: &gatefold_core::db::SpotifyAlbum) {
        let imp = self.imp();
        imp.bottom_sheet.set_open(false);
        imp.search_bar.set_search_mode(false);
        imp.stack.set_visible_child_name("albums");
        imp.album_panel.set_spotify_album(album);
        reveal_sidebar(&imp.split_view);
    }

    /// A Spotify artist's page (§9.8): their albums and singles there.
    pub fn show_spotify_artist(&self, spotify_id: &str) {
        let imp = self.imp();
        imp.bottom_sheet.set_open(false);
        imp.search_bar.set_search_mode(false);
        imp.stack.set_visible_child_name("artists");
        let page = ArtistPage::new_spotify(spotify_id);
        self.open_artist_page(page);
    }

    /// Search results float over the library while searching (§4.1).
    fn search_changed(&self, text: &str) {
        let imp = self.imp();
        let query = text.trim();
        if query.is_empty() || !imp.search_bar.is_search_mode() {
            self.show_search_results(false);
        } else {
            imp.search_view.search(query);
            self.show_search_results(true);
        }
    }

    fn show_search_results(&self, show: bool) {
        let imp = self.imp();
        for revealer in [&*imp.search_dim, &*imp.search_revealer] {
            if show {
                revealer.set_visible(true);
            }
            revealer.set_reveal_child(show);
        }
    }

    /// The search field takes the typing as soon as search opens; Escape,
    /// the search button or a click beside the results closes it, as does
    /// opening a result.
    fn setup_search(&self) {
        let imp = self.imp();
        imp.search_bar.connect_entry(&*imp.search_entry);
        imp.search_bar
            .connect_search_mode_enabled_notify(glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |bar| {
                    if bar.is_search_mode() {
                        window.imp().search_entry.grab_focus();
                    }
                    window.search_changed(&window.imp().search_entry.text());
                }
            ));
        // Hidden once faded out, so they don't catch clicks.
        for revealer in [&*imp.search_dim, &*imp.search_revealer] {
            revealer.connect_child_revealed_notify(|revealer| {
                if !revealer.reveals_child() && !revealer.is_child_revealed() {
                    revealer.set_visible(false);
                }
            });
        }
        let click = gtk::GestureClick::new();
        click.set_button(0);
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        click.connect_pressed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |gesture, _, x, y| {
                let imp = window.imp();
                if !imp.search_revealer.reveals_child() {
                    return;
                }
                let card = imp.search_view.upcast_ref::<gtk::Widget>();
                let on_card = gesture
                    .widget()
                    .and_then(|overlay| overlay.pick(x, y, gtk::PickFlags::DEFAULT))
                    .is_some_and(|picked| &picked == card || picked.is_ancestor(card));
                if !on_card {
                    imp.search_bar.set_search_mode(false);
                }
            }
        ));
        imp.library_overlay.add_controller(click);
        // Escape in the results too, not only in the field.
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, key, _, _| {
                if key == gtk::gdk::Key::Escape {
                    window.imp().search_bar.set_search_mode(false);
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            }
        ));
        imp.search_revealer.add_controller(keys);
        // Down from the field into the results.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, key, _, _| {
                let imp = window.imp();
                if key == gtk::gdk::Key::Down
                    && imp.search_revealer.reveals_child()
                    && imp.search_view.child_focus(gtk::DirectionType::TabForward)
                {
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            }
        ));
        imp.search_entry.add_controller(keys);
    }

    fn update_artists_page(&self) {
        let imp = self.imp();
        let library = GatefoldApplication::get().library();
        let page = if !self.settings().boolean("online-metadata-asked") {
            "ask"
        } else if library.artists().n_items() == 0 {
            "empty"
        } else {
            "artists"
        };
        imp.artists_stack.set_visible_child_name(page);
    }

    /// Opens an artist's page in the Artists view.
    pub fn show_artist(&self, artist_id: i64) {
        let imp = self.imp();
        imp.bottom_sheet.set_open(false);
        imp.search_bar.set_search_mode(false);
        imp.stack.set_visible_child_name("artists");
        if self
            .artist_page()
            .is_some_and(|page| page.artist_id() == artist_id)
        {
            reveal_sidebar(&imp.artist_split);
            return;
        }
        self.open_artist_page(ArtistPage::new(artist_id));
    }

    fn open_artist_page(&self, page: ArtistPage) {
        let imp = self.imp();
        imp.split_view
            .bind_property("collapsed", &page.split_view(), "collapsed")
            .sync_create()
            .build();
        page.connect_local(
            "close",
            false,
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                #[upgrade_or]
                None,
                move |_| {
                    window.imp().artist_split.set_show_sidebar(false);
                    None
                }
            ),
        );
        panel_resizer::attach(&page.split_view(), self.settings());
        imp.artist_holder.set_child(Some(&page));
        reveal_sidebar(&imp.artist_split);
    }

    /// Follows the window width: a collapsed split view ignores the width
    /// fraction, so keep the artist panel at three quarters of the window,
    /// with the grid showing beside it (all of it on narrow windows); and
    /// the player picks its layout and width from it.
    fn setup_artist_overlay(&self) {
        self.connect_realize(|window| {
            let Some(surface) = window.surface() else {
                return;
            };
            let update = glib::clone!(
                #[weak]
                window,
                move |surface: &gtk::gdk::Surface| {
                    let width = surface.width();
                    let panel = if width < 600 { width } else { width * 3 / 4 };
                    let imp = window.imp();
                    imp.artist_split.set_max_sidebar_width(f64::from(panel));
                    imp.player_view.set_available_width(width);
                }
            );
            update(&surface);
            surface.connect_width_notify(update);
        });
    }

    /// Wide windows: the mini player floats over the content, which keeps
    /// scroll room for it (`.floating-player .bottom-room`). Narrow: the
    /// bar spans the bottom, and the content (with the view switcher)
    /// ends above it.
    fn setup_floating_player(&self) {
        let update = glib::clone!(
            #[weak(rename_to = window)]
            self,
            move || {
                let imp = window.imp();
                let floating = !imp.bottom_sheet.is_full_width();
                imp.library_toolbar.set_margin_bottom(if floating {
                    0
                } else {
                    imp.bottom_sheet.bottom_bar_height()
                });
                if floating {
                    window.add_css_class("floating-player");
                } else {
                    window.remove_css_class("floating-player");
                }
            }
        );
        let on_width = update.clone();
        self.imp()
            .bottom_sheet
            .connect_full_width_notify(move |_| on_width());
        let on_height = update.clone();
        self.imp()
            .bottom_sheet
            .connect_bottom_bar_height_notify(move |_| on_height());
        update();
    }

    /// Asks once whether to analyze songs for smart shuffle, and again when
    /// a feature that needs it is turned on while it's off.
    fn setup_analysis_prompt(&self) {
        let settings = self.settings();
        let update_label = glib::clone!(
            #[weak(rename_to = window)]
            self,
            move || {
                let total = GatefoldApplication::get().analyzer().total();
                // About 1.3 s per song in the background.
                let minutes = (u64::from(total) * 13 / 10 / 60).max(1);
                // Translators: {} is a number of minutes.
                window.imp().analysis_label.set_label(
                    &gettext(
                        "Analyze your songs for smart shuffle? It finds songs with a similar tempo and sound; in the background it takes about {} min.",
                    )
                    .replace("{}", &minutes.to_string()),
                );
            }
        );
        update_label();
        let on_total = update_label.clone();
        GatefoldApplication::get()
            .analyzer()
            .connect_total_notify(move |_| on_total());
        let off = settings.string("analysis-mode") == "off";
        if off && !settings.boolean("analysis-asked") {
            self.imp().analysis_revealer.set_reveal_child(true);
        }
        for key in [
            "mix-similar",
            "keep-playing-similar",
            "mix-bpm",
            "mix-sound",
        ] {
            settings.connect_changed(
                Some(key),
                glib::clone!(
                    #[weak(rename_to = window)]
                    self,
                    move |settings, key| {
                        if settings.boolean(key) && settings.string("analysis-mode") == "off" {
                            window.imp().analysis_revealer.set_reveal_child(true);
                        }
                    }
                ),
            );
        }
        settings.connect_changed(
            Some("analysis-mode"),
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |settings, _| {
                    if settings.string("analysis-mode") != "off" {
                        window.imp().analysis_revealer.set_reveal_child(false);
                    }
                }
            ),
        );
    }

    fn setup_panel_resizers(&self) {
        let imp = self.imp();
        panel_resizer::attach(&imp.split_view, self.settings());
        panel_resizer::attach(&imp.playlist_split, self.settings());
    }

    fn artist_page(&self) -> Option<ArtistPage> {
        self.imp()
            .artist_holder
            .child()
            .and_downcast::<ArtistPage>()
    }

    fn setup_tools(&self) {
        let tools = GatefoldApplication::get().tools();
        let update = glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |tools: &crate::tools::Tools| window.update_tool_banners(tools)
        );
        let on_page = update.clone();
        tools.connect_page_status_notify(move |tools| on_page(tools));
        let on_cd = update.clone();
        tools.connect_cd_present_notify(move |tools| on_cd(tools));
        let on_unfinished = update.clone();
        tools.connect_unfinished_imports_notify(move |tools| on_unfinished(tools));
        tools.connect_local(
            "detected",
            false,
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                #[upgrade_or]
                None,
                move |values| {
                    let tools = values[0].get::<crate::tools::Tools>().ok()?;
                    window.update_tool_banners(&tools);
                    window.offer_cdimport_folder(&tools);
                    None
                }
            ),
        );
        self.update_tool_banners(&tools);
        // A CD import or lyrembed page needs Gatefold while it runs (its
        // output comes through Gatefold): closing asks first, then stops it
        // cleanly and closes when it's done.
        self.connect_close_request(|window| {
            let tools = GatefoldApplication::get().tools();
            if !tools.page_running() || window.imp().closing_after_tool.get() {
                return glib::Propagation::Proceed;
            }
            window.ask_stop_tool();
            glib::Propagation::Stop
        });
    }

    fn ask_stop_tool(&self) {
        let dialog = adw::AlertDialog::new(
            Some(&gettext("A Tool Is Still Running")),
            Some(&gettext(
                "Closing Gatefold stops it. A CD import stops cleanly and can be continued later.",
            )),
        );
        dialog.add_responses(&[
            ("keep", &gettext("_Keep Running")),
            ("stop", &gettext("_Stop and Quit")),
        ]);
        dialog.set_response_appearance("stop", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("keep"));
        dialog.set_close_response("keep");
        dialog.connect_response(
            Some("stop"),
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move |_, _| window.close_after_tool()
            ),
        );
        dialog.present(Some(self));
    }

    fn close_after_tool(&self) {
        self.imp().closing_after_tool.set(true);
        let tools = GatefoldApplication::get().tools();
        tools.stop_page();
        let close = glib::clone!(
            #[weak(rename_to = window)]
            self,
            move || window.close()
        );
        let when_done = close.clone();
        tools.connect_page_status_notify(move |tools| {
            if !tools.page_running() {
                when_done();
            }
        });
        // Should it hang, Gatefold still closes.
        glib::timeout_add_seconds_local_once(30, close);
    }

    fn update_tool_banners(&self, tools: &crate::tools::Tools) {
        let imp = self.imp();
        let status = tools.page_status();
        imp.tool_label.set_label(&status);
        imp.tool_revealer.set_reveal_child(!status.is_empty());
        let can_import = tools.detected().cdimport.is_some() && status.is_empty();
        imp.cd_banner.set_revealed(can_import && tools.cd_present());
        let unfinished = tools.unfinished_imports();
        imp.unfinished_banner.set_title(
            &gettextrs::ngettext(
                "{} unfinished CD import",
                "{} unfinished CD imports",
                unfinished,
            )
            .replace("{}", &unfinished.to_string()),
        );
        imp.unfinished_banner
            .set_revealed(can_import && !tools.cd_present() && unfinished > 0);
    }

    /// Once: if cdimport saves albums outside the music folders, offer its
    /// folder as one (§10.2).
    fn offer_cdimport_folder(&self, tools: &crate::tools::Tools) {
        let settings = self.settings();
        if tools.detected().cdimport.is_none() || settings.boolean("cdimport-folder-offered") {
            return;
        }
        let Some(dir) = gatefold_tools::cdimport::Config::read().library_dir else {
            return;
        };
        let folders = crate::library_folders::load(settings);
        let covered = folders
            .iter()
            .any(|f| dir.starts_with(f) || f.starts_with(&dir));
        let _ = settings.set_boolean("cdimport-folder-offered", true);
        if covered || !dir.is_dir() {
            return;
        }
        // Translators: {} is a folder path.
        let toast = adw::Toast::builder()
            .title(
                gettextrs::gettext("cdimport saves albums to {}")
                    .replace("{}", &dir.display().to_string()),
            )
            .button_label(gettextrs::gettext("_Add as Music Folder"))
            .action_name("app.add-cdimport-folder")
            .timeout(0)
            .build();
        self.imp().toast_overlay.add_toast(toast);
    }

    /// After a cdimport run: "New album: <title> — Show" (§10.2).
    fn announce_imported_albums(&self) {
        let tools = GatefoldApplication::get().tools();
        let Some(since) = tools.import_started() else {
            return;
        };
        let library = GatefoldApplication::get().library();
        let albums = library.albums();
        let new: Vec<AlbumObject> = (0..albums.n_items())
            .filter_map(|i| albums.item(i).and_downcast::<AlbumObject>())
            .filter(|a| a.album().added >= since)
            .collect();
        for album in &new {
            // Translators: {} is an album title.
            let toast = adw::Toast::builder()
                .title(gettextrs::gettext("New album: {}").replace("{}", &album.album().title))
                .button_label(gettextrs::gettext("_Show"))
                .action_name("win.show-album")
                .action_target(&album.album().id.to_variant())
                .build();
            self.imp().toast_overlay.add_toast(toast);
        }
        if !new.is_empty() && tools.page_status().is_empty() {
            tools.forget_import();
        }
    }

    fn setup_messages(&self) {
        let app = GatefoldApplication::get();
        for source in [
            app.library().upcast::<glib::Object>(),
            app.player().upcast(),
            app.tools().upcast(),
            app.spotify().upcast(),
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
                        imp_toast(&window, &message);
                        None
                    }
                ),
            );
        }
    }

    fn show_album(&self, album: &AlbumObject) {
        let imp = self.imp();
        imp.search_bar.set_search_mode(false);
        imp.stack.set_visible_child_name("albums");
        imp.album_panel.set_album(album);
        reveal_sidebar(&imp.split_view);
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

    /// "Genre" in the sort menu: All Genres, then the library's genres.
    fn setup_genre_menu(&self) {
        let imp = self.imp();
        let action = gio::SimpleAction::new_stateful(
            "album-genre",
            Some(glib::VariantTy::STRING),
            &"".to_variant(),
        );
        action.connect_change_state(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |action, value| {
                let Some(genre) = value.and_then(|v| v.get::<String>()) else {
                    return;
                };
                action.set_state(&genre.to_variant());
                window.set_genre_filter((!genre.is_empty()).then_some(genre));
            }
        ));
        self.add_action(&action);
        if let Some(menu) = imp.sort_button.menu_model().and_downcast::<gio::Menu>() {
            menu.append_submenu(Some(&gettext("_Genre")), &imp.genre_menu);
        }
        self.fill_genre_menu();
    }

    fn fill_genre_menu(&self) {
        let imp = self.imp();
        let menu = &imp.genre_menu;
        menu.remove_all();
        let all = gio::Menu::new();
        all.append(Some(&gettext("All Genres")), Some("win.album-genre::"));
        menu.append_section(None, &all);
        let genres = gio::Menu::new();
        let library_genres = GatefoldApplication::get().library().all_genres();
        for genre in &library_genres {
            let item = gio::MenuItem::new(Some(genre), None);
            item.set_action_and_target_value(Some("win.album-genre"), Some(&genre.to_variant()));
            genres.append_item(&item);
        }
        menu.append_section(None, &genres);
        // A filtered genre that's gone: show everything again.
        let current = imp.genre_filter.borrow().clone();
        if let Some(current) = current
            && !library_genres
                .iter()
                .any(|g| g.eq_ignore_ascii_case(&current))
        {
            gio::prelude::ActionGroupExt::change_action_state(
                self,
                "album-genre",
                &"".to_variant(),
            );
        }
    }

    fn set_genre_filter(&self, genre: Option<String>) {
        let imp = self.imp();
        let active = genre.is_some();
        imp.genre_filter.replace(genre);
        if let Some(filter) = imp.genre_filter_model.get() {
            filter.changed(gtk::FilterChange::Different);
        }
        // The sort button shows that a filter is on.
        if active {
            imp.sort_button.add_css_class("accent");
            imp.sort_button
                .set_tooltip_text(Some(&gettext("Sort and Filter Albums (Filtered)")));
        } else {
            imp.sort_button.remove_css_class("accent");
            imp.sort_button
                .set_tooltip_text(Some(&gettext("Sort and Filter Albums")));
        }
    }

    fn library_reloaded(&self) {
        let imp = self.imp();
        self.fill_genre_menu();
        self.announce_imported_albums();
        // Leave the page of an artist that's no longer in the library
        // (a Spotify artist's page never was).
        let library = GatefoldApplication::get().library();
        if let Some(page) = self.artist_page()
            && page.spotify_artist_id().is_none()
            && library.artist(page.artist_id()).is_none()
        {
            imp.artist_split.set_show_sidebar(false);
            imp.artist_holder.set_child(None::<&gtk::Widget>);
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
        let page = if library.albums().n_items() == 0 && library.scanning() {
            "scanning"
        } else if library.albums().n_items() == 0 {
            "empty"
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
    /// `GATEFOLD_SCREENSHOT_ACTIONS` lists actions to run first, separated
    /// by spaces, with an optional `::target` (GVariant text; integers are
    /// passed as int64 IDs), e.g. `win.open-player win.show-playlist::1`.
    /// `GATEFOLD_SCREENSHOT_ALBUM=N` (or a title) opens that album and
    /// `GATEFOLD_SCREENSHOT_PLAY=1` plays it; `GATEFOLD_SCREENSHOT_ARTIST=name`
    /// opens an artist page; `GATEFOLD_SCREENSHOT_PANEL_ACTIONS` runs album
    /// panel actions (`panel.add-to-playlist`);
    /// `GATEFOLD_SCREENSHOT_PREFS=<page>` opens Preferences;
    /// `GATEFOLD_SCREENSHOT_SEARCH=text` searches;
    /// `GATEFOLD_SCREENSHOT_ADD_SONGS=text` searches in the playlist panel;
    /// `GATEFOLD_SCREENSHOT_SETTINGS="key=value …"` sets GSettings first;
    /// `GATEFOLD_SCREENSHOT_IMPORT=file.m3u8` imports and shows a playlist.
    /// Without `GATEFOLD_SCREENSHOT`, only that album step runs.
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
        // `GATEFOLD_SCREENSHOT_SETTINGS="mix-similar=true mix-every=1"`:
        // GSettings values (GVariant text), set first.
        if let Ok(values) = std::env::var("GATEFOLD_SCREENSHOT_SETTINGS") {
            let settings = self.settings();
            for pair in values.split_whitespace() {
                let Some((key, value)) = pair.split_once('=') else {
                    continue;
                };
                let value =
                    glib::Variant::parse(None, value).unwrap_or_else(|_| value.to_variant());
                if let Err(err) = settings.set_value(key, &value) {
                    tracing::warn!("Screenshot setting {key}: {err}");
                }
            }
        }
        // `GATEFOLD_SCREENSHOT_SIZE=400x760`: the window size.
        if let Some((w, h)) = std::env::var("GATEFOLD_SCREENSHOT_SIZE")
            .ok()
            .and_then(|size| {
                let (w, h) = size.split_once('x')?;
                Some((w.parse().ok()?, h.parse().ok()?))
            })
        {
            self.set_default_size(w, h);
        }
        let window = self.downgrade();
        let first_step = delay.clamp(1, 2);
        glib::timeout_add_seconds_local_once(first_step, move || {
            let Some(window) = window.upgrade() else {
                return;
            };
            for action in actions.split_whitespace() {
                let (name, target) = match action.split_once("::") {
                    Some((name, target)) => (name, Some(screenshot_target(target))),
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
                    let model = window.imp().album_grid.local_model()?;
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
                    // `GATEFOLD_SCREENSHOT_SEEK_END=5000`: 2 s later, jump to
                    // that long before the end (crossfade tests).
                    if let Some(before) = std::env::var("GATEFOLD_SCREENSHOT_SEEK_END")
                        .ok()
                        .and_then(|v| v.parse::<u64>().ok())
                    {
                        glib::timeout_add_seconds_local_once(2, move || {
                            let player = GatefoldApplication::get().player();
                            player.seek(player.duration().saturating_sub(before));
                        });
                    }
                }
                // e.g. `panel.lyrembed`: actions of the album panel.
                let panel_actions =
                    std::env::var("GATEFOLD_SCREENSHOT_PANEL_ACTIONS").unwrap_or_default();
                for action in panel_actions.split_whitespace() {
                    let panel = window.imp().album_panel.get();
                    if let Err(err) = WidgetExt::activate_action(&panel, action, None) {
                        tracing::warn!("Screenshot panel action {action}: {err}");
                    }
                }
            }
            // `GATEFOLD_SCREENSHOT_SEARCH=text`: search for it.
            if let Ok(text) = std::env::var("GATEFOLD_SCREENSHOT_SEARCH") {
                window.imp().search_bar.set_search_mode(true);
                window.imp().search_entry.set_text(&text);
            }
            // `GATEFOLD_SCREENSHOT_ADD_SONGS=text`: search in the playlist panel.
            if let Ok(text) = std::env::var("GATEFOLD_SCREENSHOT_ADD_SONGS") {
                window.imp().playlist_panel.search_for_songs(&text);
            }
            // `GATEFOLD_SCREENSHOT_IMPORT=file.m3u8`: import and show a playlist.
            if let Ok(path) = std::env::var("GATEFOLD_SCREENSHOT_IMPORT") {
                let window = window.clone();
                glib::spawn_future_local(async move {
                    let library = GatefoldApplication::get().library();
                    match library.import_playlist(path.into()).await {
                        Ok((id, matched, total)) => {
                            tracing::info!("Imported {matched} of {total}");
                            if let Some(playlist) = library.playlist(id) {
                                window.show_playlist(&playlist);
                            }
                        }
                        Err(err) => tracing::warn!("Import failed: {err}"),
                    }
                });
            }
            // `GATEFOLD_SCREENSHOT_STREAM=1`: start streaming (the settings
            // above came after the app's start).
            if std::env::var_os("GATEFOLD_SCREENSHOT_STREAM").is_some() {
                GatefoldApplication::get().spotify().start_streaming();
            }
            // `GATEFOLD_SCREENSHOT_SPOTIFY=1`: connect (with the stand-in
            // server of GATEFOLD_SPOTIFY_TEST_SERVER).
            if std::env::var_os("GATEFOLD_SCREENSHOT_SPOTIFY").is_some() {
                let spotify = GatefoldApplication::get().spotify();
                glib::spawn_future_local(async move { spotify.connect().await });
            }
            // `GATEFOLD_SCREENSHOT_PLAYLIST=name`: open it once it exists.
            if let Ok(name) = std::env::var("GATEFOLD_SCREENSHOT_PLAYLIST") {
                let library = GatefoldApplication::get().library();
                let open = glib::clone!(
                    #[weak]
                    window,
                    #[upgrade_or]
                    false,
                    move || {
                        let library = GatefoldApplication::get().library();
                        let store = library.playlists();
                        let found = (0..store.n_items())
                            .filter_map(|i| store.item(i).and_downcast::<PlaylistObject>())
                            .find(|p| p.playlist().name == name);
                        if let Some(playlist) = &found
                            && window.imp().playlist_panel.playlist_id() != Some(playlist.id())
                        {
                            window.show_playlist(playlist);
                        }
                        found.is_some()
                    }
                );
                if !open() {
                    library.connect_local("playlists-changed", false, move |_| {
                        open();
                        None
                    });
                }
            }
            // `GATEFOLD_SCREENSHOT_EDITS="move:0:2 remove:1 add:Title
            // rename:Name"`: edits of the open playlist, 3 s in.
            if let Ok(edits) = std::env::var("GATEFOLD_SCREENSHOT_EDITS") {
                let window = window.downgrade();
                glib::timeout_add_seconds_local_once(3, move || {
                    let Some(window) = window.upgrade() else {
                        return;
                    };
                    let Some(id) = window.imp().playlist_panel.playlist_id() else {
                        return;
                    };
                    let library = GatefoldApplication::get().library();
                    for edit in edits.split_whitespace() {
                        let parts: Vec<&str> = edit.split(':').collect();
                        match parts.as_slice() {
                            ["move", from, to] => library.move_playlist_item(
                                id,
                                from.parse().unwrap_or(0),
                                to.parse().unwrap_or(0),
                            ),
                            ["remove", index] => {
                                let items = library.playlist_items(id);
                                if let Some(item) = items.get(index.parse::<usize>().unwrap_or(0)) {
                                    library.remove_playlist_item(id, item.id);
                                }
                            }
                            ["add", title] => {
                                let title = title.replace('_', " ");
                                let found = library.search(&title, 1).tracks;
                                library.add_to_playlist(id, &found, false);
                            }
                            ["rename", name] => {
                                library.rename_playlist(id, &name.replace('_', " "))
                            }
                            _ => tracing::warn!("Unknown edit {edit}"),
                        }
                    }
                });
            }
            // `GATEFOLD_SCREENSHOT_QUEUE=title`: add that song to the queue
            // 5 s in (after a device took over, it goes to the device).
            if let Ok(title) = std::env::var("GATEFOLD_SCREENSHOT_QUEUE") {
                glib::timeout_add_seconds_local_once(5, move || {
                    let library = GatefoldApplication::get().library();
                    let ids = library.search(&title, 1).tracks;
                    let tracks = library
                        .tracks(&ids)
                        .into_values()
                        .map(crate::objects::TrackObject::new)
                        .collect();
                    GatefoldApplication::get().player().add_to_queue(tracks);
                });
            }
            // `GATEFOLD_SCREENSHOT_PUBLISH=1`: "Create on Spotify" for the
            // open playlist once connected, without asking.
            if std::env::var_os("GATEFOLD_SCREENSHOT_PUBLISH").is_some() {
                let spotify = GatefoldApplication::get().spotify();
                let window = window.downgrade();
                spotify.connect_status_notify(move |spotify| {
                    let Some(window) = window.upgrade() else {
                        return;
                    };
                    let panel = window.imp().playlist_panel.get();
                    if spotify.is_connected()
                        && let Some(id) = panel.playlist_id()
                    {
                        let spotify = spotify.clone();
                        glib::spawn_future_local(async move { spotify.publish(id, &panel).await });
                    }
                });
            }
            // `GATEFOLD_SCREENSHOT_TIMED_ACTIONS="8:player.play-pause 30:…"`:
            // window actions at those seconds.
            if let Ok(list) = std::env::var("GATEFOLD_SCREENSHOT_TIMED_ACTIONS") {
                for entry in list.split_whitespace() {
                    let Some((at, action)) = entry.split_once(':') else {
                        continue;
                    };
                    let Ok(at) = at.parse::<u32>() else { continue };
                    let action = action.to_owned();
                    let window = window.downgrade();
                    glib::timeout_add_seconds_local_once(at, move || {
                        let (name, target) = match action.split_once("::") {
                            Some((name, target)) => (name, Some(screenshot_target(target))),
                            None => (action.as_str(), None),
                        };
                        if let Some(window) = window.upgrade()
                            && let Err(err) =
                                WidgetExt::activate_action(&window, name, target.as_ref())
                        {
                            tracing::warn!("Screenshot action {action}: {err}");
                        }
                    });
                }
            }
            // `GATEFOLD_SCREENSHOT_CLICK_TILE=1`: the first tile of the grids
            // clicked 9 s in.
            if std::env::var_os("GATEFOLD_SCREENSHOT_CLICK_TILE").is_some() {
                let window = window.downgrade();
                glib::timeout_add_seconds_local_once(9, move || {
                    if let Some(window) = window.upgrade() {
                        match window.imp().stack.visible_child_name().as_deref() {
                            Some("artists") => window.imp().artist_grid.click_first_tile(),
                            _ => window.imp().album_grid.click_first_tile(),
                        }
                    }
                });
            }
            // `GATEFOLD_SCREENSHOT_SCROLL_END=1`: the grids scrolled to their
            // end 9 s in (Spotify's section).
            if std::env::var_os("GATEFOLD_SCREENSHOT_SCROLL_END").is_some() {
                let window = window.downgrade();
                glib::timeout_add_seconds_local_once(9, move || {
                    if let Some(window) = window.upgrade() {
                        window.imp().album_grid.scroll_to_end();
                        window.imp().artist_grid.scroll_to_end();
                    }
                });
            }
            // `GATEFOLD_SCREENSHOT_LATE_PANEL_ACTIONS=panel.play`: album panel
            // actions 11 s in (after late actions opened an album).
            if let Ok(actions) = std::env::var("GATEFOLD_SCREENSHOT_LATE_PANEL_ACTIONS") {
                let window = window.downgrade();
                glib::timeout_add_seconds_local_once(11, move || {
                    let Some(window) = window.upgrade() else {
                        return;
                    };
                    let panel = window.imp().album_panel.get();
                    for action in actions.split_whitespace() {
                        if let Err(err) = WidgetExt::activate_action(&panel, action, None) {
                            tracing::warn!("Screenshot panel action {action}: {err}");
                        }
                    }
                });
            }
            // `GATEFOLD_SCREENSHOT_LATE_ACTIONS=player.shuffle::true`: window
            // actions 8 s in (after playback started).
            if let Ok(actions) = std::env::var("GATEFOLD_SCREENSHOT_LATE_ACTIONS") {
                let window = window.downgrade();
                glib::timeout_add_seconds_local_once(8, move || {
                    let Some(window) = window.upgrade() else {
                        return;
                    };
                    for action in actions.split_whitespace() {
                        let (name, target) = match action.split_once("::") {
                            Some((name, target)) => (name, Some(screenshot_target(target))),
                            None => (action, None),
                        };
                        if let Err(err) = WidgetExt::activate_action(&window, name, target.as_ref())
                        {
                            tracing::warn!("Screenshot action {name}: {err}");
                        }
                    }
                });
            }
            // `GATEFOLD_SCREENSHOT_PLAYLIST_ACTIONS=playlist.play`: actions of
            // the playlist panel, 4 s in.
            if let Ok(actions) = std::env::var("GATEFOLD_SCREENSHOT_PLAYLIST_ACTIONS") {
                let window = window.downgrade();
                glib::timeout_add_seconds_local_once(4, move || {
                    let Some(window) = window.upgrade() else {
                        return;
                    };
                    let panel = window.imp().playlist_panel.get();
                    for action in actions.split_whitespace() {
                        if let Err(err) = WidgetExt::activate_action(&panel, action, None) {
                            tracing::warn!("Screenshot playlist action {action}: {err}");
                        }
                    }
                });
            }
            // `GATEFOLD_SCREENSHOT_DEVICE=name`: play on that Spotify device
            // once connected.
            if let Ok(name) = std::env::var("GATEFOLD_SCREENSHOT_DEVICE") {
                let spotify = GatefoldApplication::get().spotify();
                spotify.connect_status_notify(move |spotify| {
                    if !spotify.is_connected() {
                        return;
                    }
                    let (spotify, name) = (spotify.clone(), name.clone());
                    glib::spawn_future_local(async move {
                        let devices = spotify.devices().await.unwrap_or_default();
                        if let Some(device) = devices.into_iter().find(|d| d.name == name) {
                            spotify.play_on(device).await;
                        }
                    });
                });
            }
            // `GATEFOLD_SCREENSHOT_PREFS=integrations`: a Preferences page.
            if let Ok(page) = std::env::var("GATEFOLD_SCREENSHOT_PREFS") {
                let dialog = crate::preferences_dialog::PreferencesDialog::new();
                dialog.set_visible_page_name(&page);
                dialog.present(Some(&window));
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

/// A message toast: short confirmations ("Added to Queue") go quickly,
/// longer messages stay a bit longer.
fn imp_toast(window: &GatefoldWindow, message: &str) {
    let toast = adw::Toast::new(message);
    // Song titles like "A & B" are text, not markup.
    toast.set_use_markup(false);
    toast.set_timeout(if message.chars().count() <= 40 { 2 } else { 4 });
    window.imp().toast_overlay.add_toast(toast);
}

/// Shows a split view's sidebar. Revealing it before the split view has had
/// its first allocation (its page was just made visible) makes it hand the
/// sidebar a bogus negative width, so then wait for one frame.
fn reveal_sidebar(split: &adw::OverlaySplitView) {
    if split.is_mapped() && split.width() > 0 {
        split.set_show_sidebar(true);
        return;
    }
    let ticks = std::cell::Cell::new(0);
    split.add_tick_callback(move |split, _| {
        ticks.set(ticks.get() + 1);
        if ticks.get() < 2 {
            return glib::ControlFlow::Continue;
        }
        split.set_show_sidebar(true);
        glib::ControlFlow::Break
    });
}

/// A screenshot action's target: GVariant text ("true", "42"), else a
/// plain string. Integers are IDs, which the actions take as int64.
fn screenshot_target(target: &str) -> glib::Variant {
    match glib::Variant::parse(None, target) {
        Ok(v) => v.get::<i32>().map_or(v, |n| i64::from(n).to_variant()),
        Err(_) => target.to_variant(),
    }
}
