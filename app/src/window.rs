use std::cell::{OnceCell, RefCell};
use std::cmp::Ordering;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};

use crate::application::JewelcaseApplication;
use crate::config::{self, APP_ID};
use crate::objects::PlaylistObject;
use crate::objects::{AlbumObject, ArtistObject};
use crate::widgets::{
    AlbumPanel, AlbumTile, ArtistPage, ArtistTile, MiniPlayer, PlayerView, PlaylistPanel,
    PlaylistTile, SearchView, panel_resizer, playlist_picker,
};

/// How long a Spotify request runs before the header bar's spinner shows,
/// and how long it stays after the last one.
const LOADING_DELAY: std::time::Duration = std::time::Duration::from_millis(400);
const LOADING_LINGER: std::time::Duration = std::time::Duration::from_millis(300);

/// The keys that put the albums in jewel cases (see `setup_jewel_cases`).
const KONAMI: [gtk::gdk::Key; 10] = {
    use gtk::gdk::Key;
    [
        Key::Up,
        Key::Up,
        Key::Down,
        Key::Down,
        Key::Left,
        Key::Right,
        Key::Left,
        Key::Right,
        Key::b,
        Key::a,
    ]
};

/// How much of `KONAMI` has been typed after `key`, given `progress`
/// before it.
fn konami_step(progress: usize, key: gtk::gdk::Key) -> usize {
    if KONAMI.get(progress) == Some(&key) {
        return progress + 1;
    }
    // A third ↑ still leaves the first two; any other ↑ starts again.
    match key {
        gtk::gdk::Key::Up if progress == 2 => 2,
        gtk::gdk::Key::Up => 1,
        _ => 0,
    }
}

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/sobakah/Jewelcase/ui/window.ui")]
    pub struct JewelcaseWindow {
        #[template_child]
        pub toast_overlay: TemplateChild<adw::ToastOverlay>,
        /// Albums of the current cdport import already announced: each
        /// reload during the import shows only the new ones.
        pub announced: RefCell<std::collections::HashSet<i64>>,
        /// The "album-sort" setting, kept here: the sorter compares often.
        pub album_sort: RefCell<String>,
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
        pub spotify_loading: TemplateChild<gtk::Revealer>,
        /// Shows `spotify_loading` once a request takes a moment.
        pub loading_delay: RefCell<Option<glib::SourceId>>,
        #[template_child]
        pub tool_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub cd_banner: TemplateChild<adw::Banner>,
        #[template_child]
        pub update_banner: TemplateChild<adw::Banner>,
        #[template_child]
        pub unfinished_banner: TemplateChild<adw::Banner>,
        #[template_child]
        pub albums_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub split_view: TemplateChild<crate::widgets::SlideView>,
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
        pub artist_split: TemplateChild<crate::widgets::SlideView>,
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
        pub playlist_split: TemplateChild<crate::widgets::SlideView>,
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
    impl ObjectSubclass for JewelcaseWindow {
        const NAME: &'static str = "JewelcaseWindow";
        type Type = super::JewelcaseWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            MiniPlayer::ensure_type();
            PlayerView::ensure_type();
            AlbumPanel::ensure_type();
            PlaylistPanel::ensure_type();
            SearchView::ensure_type();
            crate::widgets::SectionedGrid::ensure_type();
            crate::widgets::SlideView::ensure_type();
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
                    let spotify = JewelcaseApplication::get().spotify();
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
                    let library = JewelcaseApplication::get().library();
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
                    let library = JewelcaseApplication::get().library();
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
                    let library = JewelcaseApplication::get().library();
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

    impl ObjectImpl for JewelcaseWindow {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            if config::is_development() {
                obj.add_css_class("devel");
            }
            crate::frame_stats::watch(obj.upcast_ref());
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

            let player = JewelcaseApplication::get().player();
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
            obj.setup_spotify_loading();
            obj.setup_jewel_cases();

            // Sorting only applies to the album grid.
            let sort_button = self.sort_button.get();
            self.stack
                .bind_property("visible-child-name", &sort_button, "visible")
                .transform_to(|_, name: Option<String>| Some(name.as_deref() == Some("albums")))
                .sync_create()
                .build();
        }
    }

    impl WidgetImpl for JewelcaseWindow {}

    impl WindowImpl for JewelcaseWindow {
        fn close_request(&self) -> glib::Propagation {
            if let Err(err) = self.obj().save_window_state() {
                tracing::warn!("Failed to save window state: {err}");
            }
            self.parent_close_request()
        }
    }

    impl ApplicationWindowImpl for JewelcaseWindow {}
    impl AdwApplicationWindowImpl for JewelcaseWindow {}
}

glib::wrapper! {
    pub struct JewelcaseWindow(ObjectSubclass<imp::JewelcaseWindow>)
        @extends gtk::Widget, gtk::Window, gtk::ApplicationWindow, adw::ApplicationWindow,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable,
                    gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl JewelcaseWindow {
    pub fn new(app: &JewelcaseApplication) -> Self {
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
        let library = JewelcaseApplication::get().library();

        imp.album_sort
            .replace(self.settings().string("album-sort").into());
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
                JewelcaseApplication::get()
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
            // A tile's padding (style.css: .grid-tile).
            18,
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
                move |settings, key| {
                    window.imp().album_sort.replace(settings.string(key).into());
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
        let library = JewelcaseApplication::get().library();

        imp.artist_grid.setup(
            library.artists(),
            // A tile's padding (style.css: .artist-grid .grid-tile).
            24,
            "artist-grid",
            || ArtistTile::new().upcast(),
            |tile, item| {
                if let (Some(tile), Some(artist)) = (
                    tile.downcast_ref::<ArtistTile>(),
                    item.downcast_ref::<ArtistObject>(),
                ) {
                    tile.bind(artist);
                    // Lookups run lazily, for artists that are shown (§8).
                    JewelcaseApplication::get().library().request_lookup(artist);
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
        let library = JewelcaseApplication::get().library();
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
            // Its cell in the flow box only as wide as the tile (a wider
            // cell, highlighted, would look like a larger tile).
            // (After the flow box has added it: not while inserting.)
            tile.connect_notify_local(Some("parent"), |tile, _| {
                let tile = tile.downgrade();
                glib::idle_add_local_once(move || {
                    if let Some(child) = tile.upgrade().and_then(|t| t.parent()) {
                        child.set_halign(gtk::Align::Center);
                    }
                });
            });
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
                if let Some(window) = grid.root().and_downcast::<JewelcaseWindow>() {
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
        let library = JewelcaseApplication::get().library();
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

    /// A small spinner in the header bar while something loads from
    /// Spotify. Only once a request takes a moment (quick ones would make
    /// it flicker), and faded out when the last answer is in.
    fn setup_spotify_loading(&self) {
        let revealer = self.imp().spotify_loading.get();
        // Hidden once faded out, so it takes no room.
        revealer.connect_child_revealed_notify(|revealer| {
            if !revealer.reveals_child() {
                revealer.set_visible(false);
            }
        });
        let spotify = JewelcaseApplication::get().spotify();
        spotify.connect_loading_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |spotify| window.spotify_loading_changed(spotify.loading())
        ));
    }

    /// A hidden extra: ↑ ↑ ↓ ↓ ← → ← → B A puts the albums in jewel
    /// cases (`jewel-cases`), and again takes them out.
    fn setup_jewel_cases(&self) {
        let progress = std::rc::Rc::new(std::cell::Cell::new(0usize));
        let keys = gtk::EventControllerKey::new();
        // Before the grids and type-to-search see the keys.
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, key, _, modifiers| {
                let plain = !modifiers.intersects(
                    gtk::gdk::ModifierType::CONTROL_MASK
                        | gtk::gdk::ModifierType::ALT_MASK
                        | gtk::gdk::ModifierType::SUPER_MASK,
                );
                let next = if plain {
                    konami_step(progress.get(), key.to_lower())
                } else {
                    0
                };
                progress.set(next);
                if next <= KONAMI.len() - 2 {
                    return glib::Propagation::Proceed;
                }
                // B and A finish the code: they don't start a search.
                if next == KONAMI.len() {
                    progress.set(0);
                    window.toggle_jewel_cases();
                }
                glib::Propagation::Stop
            }
        ));
        self.add_controller(keys);
    }

    fn toggle_jewel_cases(&self) {
        let settings = self.settings();
        let on = !settings.boolean("jewel-cases");
        let _ = settings.set_boolean("jewel-cases", on);
        self.show_toast(&if on {
            gettext("Your albums are back in their jewel cases")
        } else {
            gettext("Your albums are out of their jewel cases")
        });
    }

    fn spotify_loading_changed(&self, loading: bool) {
        let imp = self.imp();
        let shown = imp.spotify_loading.reveals_child();
        // A timer still waiting: to show it (while hidden) or to hide it
        // (while shown).
        let pending = imp.loading_delay.take();
        let (show, delay) = match (loading, shown) {
            // Shown, and the next request started: it stays.
            (true, true) => {
                if let Some(timer) = pending {
                    timer.remove();
                }
                return;
            }
            // Already waiting to show.
            (true, false) if pending.is_some() => {
                imp.loading_delay.replace(pending);
                return;
            }
            (true, false) => (true, LOADING_DELAY),
            // Hidden a little later: requests that follow one another
            // (an artist, then their albums) keep it.
            (false, true) => (false, LOADING_LINGER),
            (false, false) => {
                if let Some(timer) = pending {
                    timer.remove();
                }
                return;
            }
        };
        if let Some(timer) = pending {
            timer.remove();
        }
        let timer = glib::timeout_add_local_once(
            delay,
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                move || {
                    let imp = window.imp();
                    imp.loading_delay.take();
                    if show {
                        imp.spotify_loading.set_visible(true);
                    }
                    imp.spotify_loading.set_reveal_child(show);
                }
            ),
        );
        imp.loading_delay.replace(Some(timer));
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
        let app = JewelcaseApplication::get();
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
        let app = JewelcaseApplication::get();
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
        let mut local_names = std::collections::HashSet::new();
        let mut local_ids = std::collections::HashSet::new();
        for a in (0..local.n_items()).filter_map(|i| local.item(i).and_downcast::<ArtistObject>()) {
            let info = a.artist();
            local_names.insert(info.name.to_lowercase());
            if let Some(id) = info.meta.spotify_id.clone() {
                local_ids.insert(id);
            }
        }
        let artists: Vec<_> = spotify
            .followed_artists()
            .into_iter()
            .filter(|artist| {
                !local_ids.contains(&artist.spotify_id)
                    && !local_names.contains(&artist.name.to_lowercase())
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
    pub fn show_spotify_album(&self, album: &jewelcase_core::db::SpotifyAlbum) {
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
        if !imp.search_bar.is_search_mode() {
            self.show_search_results(false);
        } else if query.is_empty() {
            // An empty field shows the last searches, if there are any.
            self.show_search_results(imp.search_view.show_recent());
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
                    let imp = window.imp();
                    if bar.is_search_mode() {
                        imp.search_entry.grab_focus();
                    } else {
                        // Searched, then closed: a recent search.
                        imp.search_view.remember(&imp.search_entry.text());
                    }
                    window.search_changed(&imp.search_entry.text());
                }
            ));
        // A recent search clicked: searched again.
        imp.search_view.connect_local(
            "query-chosen",
            false,
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                #[upgrade_or]
                None,
                move |values| {
                    let query = values[1].get::<String>().ok()?;
                    let entry = &window.imp().search_entry;
                    entry.set_text(&query);
                    entry.grab_focus();
                    entry.set_position(-1);
                    None
                }
            ),
        );
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
        let library = JewelcaseApplication::get().library();
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

    /// Follows the window width: the player picks its layout and width
    /// from it. (The artist page's width: panel_resizer, three quarters of
    /// the window until its edge is dragged.)
    fn setup_artist_overlay(&self) {
        self.connect_realize(|window| {
            let Some(surface) = window.surface() else {
                return;
            };
            let update = glib::clone!(
                #[weak]
                window,
                move |surface: &gtk::gdk::Surface| {
                    window
                        .imp()
                        .player_view
                        .set_available_width(surface.width());
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
                let total = JewelcaseApplication::get().analyzer().total();
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
        JewelcaseApplication::get()
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
        panel_resizer::attach_artist_overlay(&imp.artist_split, self.settings());
    }

    fn artist_page(&self) -> Option<ArtistPage> {
        self.imp()
            .artist_holder
            .child()
            .and_downcast::<ArtistPage>()
    }

    fn setup_tools(&self) {
        let tools = JewelcaseApplication::get().tools();
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
                    window.offer_cdport_folder(&tools);
                    None
                }
            ),
        );
        self.update_tool_banners(&tools);
        crate::updates::start(self);
        // A CD import or lyrembed page needs Jewelcase while it runs (its
        // output comes through Jewelcase): closing asks first, then stops it
        // cleanly and closes when it's done.
        self.connect_close_request(|window| {
            let tools = JewelcaseApplication::get().tools();
            if !tools.page_needs_jewelcase() || window.imp().closing_after_tool.get() {
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
                "Closing Jewelcase stops it. A CD import stops cleanly and can be continued later.",
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
        let tools = JewelcaseApplication::get().tools();
        // The tool may have finished while the question was shown: then
        // nothing changes any more to wait for.
        if !tools.page_needs_jewelcase() {
            self.close();
            return;
        }
        tools.stop_page();
        let close = glib::clone!(
            #[weak(rename_to = window)]
            self,
            move || window.close()
        );
        let when_done = close.clone();
        tools.connect_page_status_notify(move |tools| {
            if !tools.page_needs_jewelcase() {
                when_done();
            }
        });
        // Should it hang, Jewelcase still closes.
        glib::timeout_add_seconds_local_once(30, close);
    }

    /// Shows the first update found (`None`: hides the banner).
    pub fn set_update_banner(&self, title: Option<&str>) {
        let banner = &self.imp().update_banner;
        if let Some(title) = title {
            banner.set_title(title);
        }
        banner.set_revealed(title.is_some());
    }

    pub fn show_toast(&self, message: &str) {
        imp_toast(self, message);
    }

    fn update_tool_banners(&self, tools: &crate::tools::Tools) {
        let imp = self.imp();
        let status = tools.page_status();
        imp.tool_label.set_label(&status);
        imp.tool_revealer.set_reveal_child(!status.is_empty());
        let can_import = tools.detected().cdport.usable() && status.is_empty();
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

    /// Once: if cdport saves albums outside the music folders, offer its
    /// folder as one (§10.2).
    fn offer_cdport_folder(&self, tools: &crate::tools::Tools) {
        let settings = self.settings();
        let cdport = tools.detected().cdport;
        if !cdport.usable() || settings.boolean("cdport-folder-offered") {
            return;
        }
        let Some(dir) = cdport.config.library_dir else {
            return;
        };
        let folders = crate::library_folders::load(settings);
        let covered = folders
            .iter()
            .any(|f| dir.starts_with(f) || f.starts_with(&dir));
        // Not there yet (cdport makes it on the first import): offered
        // once it is.
        if !covered && !dir.is_dir() {
            return;
        }
        let _ = settings.set_boolean("cdport-folder-offered", true);
        if covered {
            return;
        }
        // Translators: {} is a folder path.
        let toast = adw::Toast::builder()
            .title(
                gettextrs::gettext("cdport saves albums to {}")
                    .replace("{}", &dir.display().to_string()),
            )
            .button_label(gettextrs::gettext("_Add as Music Folder"))
            .action_name("app.add-cdport-folder")
            .timeout(0)
            .build();
        self.imp().toast_overlay.add_toast(toast);
    }

    /// After a cdport run: "New album: <title> — Show" (§10.2).
    fn announce_imported_albums(&self) {
        let tools = JewelcaseApplication::get().tools();
        let Some(since) = tools.import_started() else {
            return;
        };
        let library = JewelcaseApplication::get().library();
        let albums = library.albums();
        let new: Vec<AlbumObject> = {
            let mut announced = self.imp().announced.borrow_mut();
            (0..albums.n_items())
                .filter_map(|i| albums.item(i).and_downcast::<AlbumObject>())
                .filter(|a| a.album().added >= since)
                .filter(|a| announced.insert(a.album().id))
                .collect()
        };
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
        // The import is over: what it added is announced (also when it
        // added nothing, so a later album isn't taken for its).
        if tools.page_status().is_empty() {
            tools.forget_import();
            self.imp().announced.borrow_mut().clear();
        }
    }

    fn setup_messages(&self) {
        let app = JewelcaseApplication::get();
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
        // A tool that isn't set up: the instructions are in Preferences.
        app.tools().connect_local(
            "missing",
            false,
            glib::clone!(
                #[weak(rename_to = window)]
                self,
                #[upgrade_or]
                None,
                move |values| {
                    let name = values[1].get::<String>().ok()?;
                    // Translators: {} is a program name (lyrembed, cdport).
                    let toast = adw::Toast::builder()
                        .title(gettextrs::gettext("{} isn't set up yet").replace("{}", &name))
                        .button_label(gettextrs::gettext("_How to Set Up"))
                        .action_name("app.show-integrations")
                        .build();
                    window.imp().toast_overlay.add_toast(toast);
                    None
                }
            ),
        );
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
        match self.imp().album_sort.borrow().as_str() {
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
        let library_genres = JewelcaseApplication::get().library().all_genres();
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
        let library = JewelcaseApplication::get().library();
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
            let library = JewelcaseApplication::get().library();
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
        let library = JewelcaseApplication::get().library();
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

    /// Development aid: `JEWELCASE_SCREENSHOT=out.png` renders the window to
    /// a PNG after `JEWELCASE_SCREENSHOT_DELAY` seconds (default 8) and quits.
    /// The steps below run after 2 seconds.
    /// `JEWELCASE_SCREENSHOT_ACTIONS` lists actions to run first, separated
    /// by spaces, with an optional `::target` (GVariant text; integers are
    /// passed as int64 IDs), e.g. `win.open-player win.show-playlist::1`.
    /// `JEWELCASE_SCREENSHOT_ALBUM=N` (or a title) opens that album and
    /// `JEWELCASE_SCREENSHOT_PLAY=1` plays it; `JEWELCASE_SCREENSHOT_ARTIST=name`
    /// opens an artist page (`JEWELCASE_SCREENSHOT_ARTIST_ALBUM=1` then opens
    /// its first album); `JEWELCASE_SCREENSHOT_PANEL_ACTIONS` runs album
    /// panel actions (`panel.add-to-playlist`);
    /// `JEWELCASE_SCREENSHOT_PREFS=<page>` opens Preferences;
    /// `JEWELCASE_SCREENSHOT_SEARCH=text` searches;
    /// `JEWELCASE_SCREENSHOT_ADD_SONGS=text` searches songs to add in the
    /// playlist panel (`JEWELCASE_SCREENSHOT_FILTER_SONGS`: its own songs);
    /// `JEWELCASE_SCREENSHOT_SETTINGS="key=value …"` sets GSettings first;
    /// `JEWELCASE_SCREENSHOT_IMPORT=file.m3u8` imports and shows a playlist.
    /// Without `JEWELCASE_SCREENSHOT`, only that album step runs.
    fn setup_screenshot(&self) {
        // Without JEWELCASE_SCREENSHOT, only the album and actions steps run
        // (for playback tests); the app keeps running.
        let path = std::env::var_os("JEWELCASE_SCREENSHOT");
        if path.is_none() && std::env::var_os("JEWELCASE_SCREENSHOT_ALBUM").is_none() {
            return;
        }
        let delay = std::env::var("JEWELCASE_SCREENSHOT_DELAY")
            .ok()
            .and_then(|d| d.parse().ok())
            .unwrap_or(8);
        let actions = std::env::var("JEWELCASE_SCREENSHOT_ACTIONS").unwrap_or_default();
        // `JEWELCASE_SCREENSHOT_SETTINGS="mix-similar=true mix-every=1"`:
        // GSettings values (GVariant text), set first.
        if let Ok(values) = std::env::var("JEWELCASE_SCREENSHOT_SETTINGS") {
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
        // `JEWELCASE_SCREENSHOT_SIZE=400x760`: the window size.
        if let Some((w, h)) = std::env::var("JEWELCASE_SCREENSHOT_SIZE")
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
            let album = std::env::var("JEWELCASE_SCREENSHOT_ALBUM")
                .ok()
                .and_then(|wanted| {
                    let model = window.imp().album_grid.local_model()?;
                    (0..model.n_items())
                        .filter_map(|i| model.item(i).and_downcast::<AlbumObject>())
                        .enumerate()
                        .find(|(i, a)| wanted == i.to_string() || wanted == a.album().title)
                        .map(|(_, a)| a)
                });
            let artist = std::env::var("JEWELCASE_SCREENSHOT_ARTIST")
                .ok()
                .and_then(|name| {
                    let library = JewelcaseApplication::get().library();
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
                if std::env::var_os("JEWELCASE_SCREENSHOT_PLAY").is_some() {
                    JewelcaseApplication::get()
                        .player()
                        .play_album(album.album().id, None, false);
                    // `JEWELCASE_SCREENSHOT_SEEK_END=5000`: 2 s later, jump to
                    // that long before the end (crossfade tests).
                    if let Some(before) = std::env::var("JEWELCASE_SCREENSHOT_SEEK_END")
                        .ok()
                        .and_then(|v| v.parse::<u64>().ok())
                    {
                        glib::timeout_add_seconds_local_once(2, move || {
                            let player = JewelcaseApplication::get().player();
                            player.seek(player.duration().saturating_sub(before));
                        });
                    }
                }
                // e.g. `panel.lyrembed`: actions of the album panel.
                let panel_actions =
                    std::env::var("JEWELCASE_SCREENSHOT_PANEL_ACTIONS").unwrap_or_default();
                for action in panel_actions.split_whitespace() {
                    let panel = window.imp().album_panel.get();
                    if let Err(err) = WidgetExt::activate_action(&panel, action, None) {
                        tracing::warn!("Screenshot panel action {action}: {err}");
                    }
                }
            }
            // `JEWELCASE_SCREENSHOT_PAGE_ACTIONS="panel.toggle-saved"`: 4 s
            // later, actions of the album panel or the artist page (also
            // one opened by the actions above).
            let page_actions =
                std::env::var("JEWELCASE_SCREENSHOT_PAGE_ACTIONS").unwrap_or_default();
            if !page_actions.is_empty() {
                let window = window.clone();
                glib::timeout_add_seconds_local_once(4, move || {
                    let imp = window.imp();
                    let page = imp.artist_holder.child();
                    for action in page_actions.split_whitespace() {
                        let done = page
                            .iter()
                            .chain(std::iter::once(&imp.album_panel.get().upcast()))
                            .any(|target| WidgetExt::activate_action(target, action, None).is_ok());
                        if !done {
                            tracing::warn!("Screenshot page action {action}: not found");
                        }
                    }
                });
            }
            // `JEWELCASE_SCREENSHOT_ARTIST_ALBUM=1`: 3 s later, the artist
            // page opens its first album.
            if std::env::var_os("JEWELCASE_SCREENSHOT_ARTIST_ALBUM").is_some() {
                let window = window.clone();
                glib::timeout_add_seconds_local_once(3, move || {
                    if let Some(page) = window.artist_page() {
                        page.open_first_album();
                    }
                });
            }
            // `JEWELCASE_SCREENSHOT_SEARCH=text`: search for it.
            if let Ok(text) = std::env::var("JEWELCASE_SCREENSHOT_SEARCH") {
                window.imp().search_bar.set_search_mode(true);
                window.imp().search_entry.set_text(&text);
            }
            // `JEWELCASE_SCREENSHOT_ADD_SONGS=text`: search songs to add in
            // the playlist panel; `JEWELCASE_SCREENSHOT_FILTER_SONGS=text`:
            // its own songs. 2 s in, once the playlist is open (opening one
            // clears its search).
            let add = std::env::var("JEWELCASE_SCREENSHOT_ADD_SONGS").ok();
            let filter = std::env::var("JEWELCASE_SCREENSHOT_FILTER_SONGS").ok();
            if add.is_some() || filter.is_some() {
                let window = window.clone();
                glib::timeout_add_seconds_local_once(2, move || {
                    let panel = &window.imp().playlist_panel;
                    if let Some(text) = &add {
                        // `JEWELCASE_SCREENSHOT_ADD_SPOTIFY=1`: Spotify too.
                        if std::env::var_os("JEWELCASE_SCREENSHOT_ADD_SPOTIFY").is_some() {
                            panel.search_spotify_too();
                        }
                        panel.search_for_songs(text);
                    }
                    if let Some(text) = &filter {
                        panel.filter_songs(text);
                    }
                });
            }
            // `JEWELCASE_SCREENSHOT_IMPORT=file.m3u8`: import and show a playlist.
            if let Ok(path) = std::env::var("JEWELCASE_SCREENSHOT_IMPORT") {
                let window = window.clone();
                glib::spawn_future_local(async move {
                    let library = JewelcaseApplication::get().library();
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
            // `JEWELCASE_SCREENSHOT_STREAM=1`: start streaming (the settings
            // above came after the app's start).
            if std::env::var_os("JEWELCASE_SCREENSHOT_STREAM").is_some() {
                JewelcaseApplication::get().spotify().start_streaming();
            }
            // `JEWELCASE_SCREENSHOT_SPOTIFY=1`: connect (with the stand-in
            // server of JEWELCASE_SPOTIFY_TEST_SERVER).
            if std::env::var_os("JEWELCASE_SCREENSHOT_SPOTIFY").is_some() {
                let spotify = JewelcaseApplication::get().spotify();
                glib::spawn_future_local(async move { spotify.connect().await });
            }
            // `JEWELCASE_SCREENSHOT_PLAYLIST=name`: open it once it exists.
            if let Ok(name) = std::env::var("JEWELCASE_SCREENSHOT_PLAYLIST") {
                let library = JewelcaseApplication::get().library();
                let open = glib::clone!(
                    #[weak]
                    window,
                    #[upgrade_or]
                    false,
                    move || {
                        let library = JewelcaseApplication::get().library();
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
            // `JEWELCASE_SCREENSHOT_EDITS="move:0:2 remove:1 add:Title
            // rename:Name cover:/path.jpg unspotify:keep|drop"`: edits of the
            // open playlist, 3 s in.
            if let Ok(edits) = std::env::var("JEWELCASE_SCREENSHOT_EDITS") {
                let window = window.downgrade();
                glib::timeout_add_seconds_local_once(3, move || {
                    let Some(window) = window.upgrade() else {
                        return;
                    };
                    let Some(id) = window.imp().playlist_panel.playlist_id() else {
                        return;
                    };
                    let library = JewelcaseApplication::get().library();
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
                            ["unspotify", keep] => JewelcaseApplication::get()
                                .spotify()
                                .remove_from_spotify(id, *keep == "keep"),
                            ["cover", path] => {
                                if let Ok(data) = std::fs::read(path) {
                                    let library = library.clone();
                                    glib::spawn_future_local(async move {
                                        let _ = library.set_playlist_cover(id, data).await;
                                    });
                                }
                            }
                            _ => tracing::warn!("Unknown edit {edit}"),
                        }
                    }
                });
            }
            // `JEWELCASE_SCREENSHOT_QUEUE=title`: add that song to the queue
            // 5 s in (after a device took over, it goes to the device).
            if let Ok(title) = std::env::var("JEWELCASE_SCREENSHOT_QUEUE") {
                glib::timeout_add_seconds_local_once(5, move || {
                    let library = JewelcaseApplication::get().library();
                    let ids = library.search(&title, 1).tracks;
                    let tracks = library
                        .tracks(&ids)
                        .into_values()
                        .map(crate::objects::TrackObject::new)
                        .collect();
                    JewelcaseApplication::get().player().add_to_queue(tracks);
                });
            }
            // `JEWELCASE_SCREENSHOT_PUBLISH=1`: "Create on Spotify" for the
            // open playlist once connected, without asking.
            if std::env::var_os("JEWELCASE_SCREENSHOT_PUBLISH").is_some() {
                let spotify = JewelcaseApplication::get().spotify();
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
            // `JEWELCASE_SCREENSHOT_TIMED_ACTIONS="8:player.play-pause 30:…"`:
            // window actions at those seconds.
            if let Ok(list) = std::env::var("JEWELCASE_SCREENSHOT_TIMED_ACTIONS") {
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
            // `JEWELCASE_SCREENSHOT_CLICK_TILE=1`: the first tile of the grids
            // clicked 9 s in.
            if std::env::var_os("JEWELCASE_SCREENSHOT_CLICK_TILE").is_some() {
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
            // `JEWELCASE_SCREENSHOT_SCROLL_END=1`: the grids scrolled to their
            // end 9 s in (Spotify's section).
            if std::env::var_os("JEWELCASE_SCREENSHOT_SCROLL_END").is_some() {
                let window = window.downgrade();
                glib::timeout_add_seconds_local_once(9, move || {
                    if let Some(window) = window.upgrade() {
                        window.imp().album_grid.scroll_to_end();
                        window.imp().artist_grid.scroll_to_end();
                    }
                });
            }
            // `JEWELCASE_SCREENSHOT_LATE_PANEL_ACTIONS=panel.play`: album panel
            // actions 11 s in (after late actions opened an album).
            if let Ok(actions) = std::env::var("JEWELCASE_SCREENSHOT_LATE_PANEL_ACTIONS") {
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
            // `JEWELCASE_SCREENSHOT_LATE_ACTIONS=player.shuffle::true`: window
            // actions 8 s in (after playback started).
            if let Ok(actions) = std::env::var("JEWELCASE_SCREENSHOT_LATE_ACTIONS") {
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
            // `JEWELCASE_SCREENSHOT_SONG_MENU=picker`: 10 s in, the player's
            // "Add to Playlist…"; `=history:N`: the menu of the Nth
            // Recently Played song, `=history:N:song.add-to-queue` one of its
            // actions.
            if let Ok(which) = std::env::var("JEWELCASE_SCREENSHOT_SONG_MENU") {
                let window = window.downgrade();
                glib::timeout_add_seconds_local_once(10, move || {
                    if let Some(window) = window.upgrade() {
                        window.imp().player_view.open_song_menu(&which);
                    }
                });
            }
            // `JEWELCASE_SCREENSHOT_PLAYLIST_ACTIONS=playlist.play`: actions of
            // the playlist panel, 4 s in.
            if let Ok(actions) = std::env::var("JEWELCASE_SCREENSHOT_PLAYLIST_ACTIONS") {
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
            // `JEWELCASE_SCREENSHOT_DEVICE=name`: play on that Spotify device
            // once connected.
            if let Ok(name) = std::env::var("JEWELCASE_SCREENSHOT_DEVICE") {
                let spotify = JewelcaseApplication::get().spotify();
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
            // `JEWELCASE_SCREENSHOT_PREFS=integrations`: a Preferences page.
            if let Ok(page) = std::env::var("JEWELCASE_SCREENSHOT_PREFS") {
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
fn imp_toast(window: &JewelcaseWindow, message: &str) {
    let toast = adw::Toast::new(message);
    // Song titles like "A & B" are text, not markup.
    toast.set_use_markup(false);
    toast.set_timeout(if message.chars().count() <= 40 { 2 } else { 4 });
    window.imp().toast_overlay.add_toast(toast);
}

/// A toast with Undo in the active window (`undo` runs on the button).
pub fn undo_toast(message: &str, undo: impl Fn() + 'static) {
    button_toast(message, &gettext("_Undo"), 5, undo);
}

/// A toast with a button in the active window.
pub fn button_toast(message: &str, label: &str, timeout: u32, clicked: impl Fn() + 'static) {
    let Some(window) = JewelcaseApplication::get()
        .active_window()
        .and_downcast::<JewelcaseWindow>()
    else {
        return;
    };
    let toast = adw::Toast::new(message);
    toast.set_use_markup(false);
    toast.set_button_label(Some(label));
    toast.set_timeout(timeout);
    toast.connect_button_clicked(move |_| clicked());
    window.imp().toast_overlay.add_toast(toast);
}

/// Shows a panel. Shown before its view is on screen (its page was just
/// made visible), it would appear without sliding in, so then wait for
/// a frame.
fn reveal_sidebar(split: &crate::widgets::SlideView) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use gtk::gdk::Key;

    fn typed(keys: &[Key]) -> usize {
        keys.iter()
            .fold(0, |progress, key| konami_step(progress, *key))
    }

    #[test]
    fn konami_code() {
        assert_eq!(typed(&KONAMI), KONAMI.len());
        // An extra ↑ at the start, or a slip and a new start.
        let mut extra = vec![Key::Up];
        extra.extend(KONAMI);
        assert_eq!(typed(&extra), KONAMI.len());
        let mut slip = vec![Key::Up, Key::Up, Key::Down, Key::x];
        slip.extend(KONAMI);
        assert_eq!(typed(&slip), KONAMI.len());
        // Wrong order, or letters alone: nothing.
        assert_eq!(typed(&[Key::Up, Key::Down, Key::Up, Key::Down]), 0);
        assert_eq!(typed(&[Key::b, Key::a]), 0);
        assert_eq!(typed(&KONAMI[..9]), 9);
    }
}
