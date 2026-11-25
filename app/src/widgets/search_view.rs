//! Search results (CONCEPT.md §4.1), by kind: Artists, Albums, Songs,
//! Playlists. Each section lists the library's matches (FTS5 index) and
//! then what Spotify found that isn't in the library; an icon on every
//! row says which is which. Spotify is asked once per search (all four
//! kinds in one request), a moment after typing stops, and its answers are
//! kept for the session. Two toggle groups above the results show one kind
//! or all, and from the library, Spotify or both (as Jewelcase Mobile's
//! chips). With one kind shown, *More From Spotify* under its results
//! asks for the next ten of that kind (one request a click). With an empty
//! field the card shows the last searches.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib::subclass::Signal;
use gtk::{gio, glib};
use jewelcase_core::db::SearchResults;
use jewelcase_core::library::covers::CoverSize;

use super::CoverImage;
use super::album_tile::artist_or_unknown;
use crate::application::JewelcaseApplication;
use crate::config::APP_ID;
use crate::objects::TrackObject;
use crate::{covers, format};

/// Library results per section; with one kind shown, that kind's.
const LIMIT: usize = 8;
const LIMIT_ONE_KIND: usize = 30;
/// Searches kept for "Recent Searches".
const RECENT: usize = 5;
/// Spotify is asked this long after the last keystroke.
const SPOTIFY_DELAY: std::time::Duration = std::time::Duration::from_millis(450);

/// Covers of Spotify results, downloaded after the rows: (cover, image
/// address).
type SpotifyCovers = Vec<(CoverImage, String)>;

/// Which kind of results the search shows: all, or one (the first toggle group).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    All,
    Artists,
    Albums,
    Songs,
    Playlists,
}

impl Kind {
    /// The toggles' names, in order.
    const NAMES: [(&'static str, Kind); 5] = [
        ("all", Kind::All),
        ("artists", Kind::Artists),
        ("albums", Kind::Albums),
        ("songs", Kind::Songs),
        ("playlists", Kind::Playlists),
    ];

    fn from_name(name: &str) -> Kind {
        Self::NAMES
            .iter()
            .find(|(n, _)| *n == name)
            .map_or(Kind::All, |(_, kind)| *kind)
    }

    fn shows(self, kind: Kind) -> bool {
        self == Kind::All || self == kind
    }

    /// Spotify's name for this one kind (`None` for all).
    fn spotify(self) -> Option<jewelcase_spotify::SearchKind> {
        use jewelcase_spotify::SearchKind;
        match self {
            Kind::All => None,
            Kind::Artists => Some(SearchKind::Artists),
            Kind::Albums => Some(SearchKind::Albums),
            Kind::Songs => Some(SearchKind::Tracks),
            Kind::Playlists => Some(SearchKind::Playlists),
        }
    }

    /// The library's results of this kind only.
    fn keep(self, results: SearchResults) -> SearchResults {
        SearchResults {
            artists: if self.shows(Kind::Artists) {
                results.artists
            } else {
                Vec::new()
            },
            albums: if self.shows(Kind::Albums) {
                results.albums
            } else {
                Vec::new()
            },
            tracks: if self.shows(Kind::Songs) {
                results.tracks
            } else {
                Vec::new()
            },
            playlists: if self.shows(Kind::Playlists) {
                results.playlists
            } else {
                Vec::new()
            },
        }
    }
}

/// Where the results come from: both, the library or Spotify (the second
/// toggle group). It narrows whatever kind is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Everywhere,
    Library,
    Spotify,
}

impl Source {
    const NAMES: [(&'static str, Source); 3] = [
        ("everywhere", Source::Everywhere),
        ("library", Source::Library),
        ("spotify", Source::Spotify),
    ];

    fn from_name(name: &str) -> Source {
        Self::NAMES
            .iter()
            .find(|(n, _)| *n == name)
            .map_or(Source::Everywhere, |(_, source)| *source)
    }

    fn library(self) -> bool {
        self != Source::Spotify
    }

    fn spotify(self) -> bool {
        self != Source::Library
    }
}

/// The four sections, in order.
#[derive(Debug, Clone)]
pub struct Sections {
    artists: Section,
    albums: Section,
    songs: Section,
    playlists: Section,
    /// "Searching Spotify…", or why it can't be searched.
    status: gtk::Label,
}

#[derive(Debug, Clone)]
pub struct Section {
    container: gtk::Box,
    list: gtk::ListBox,
    /// Library rows come first, also those found through Spotify.
    library_rows: Rc<std::cell::Cell<i32>>,
}

impl Sections {
    /// The section of one kind (`None` for all).
    fn of(&self, kind: Kind) -> Option<&Section> {
        match kind {
            Kind::All => None,
            Kind::Artists => Some(&self.artists),
            Kind::Albums => Some(&self.albums),
            Kind::Songs => Some(&self.songs),
            Kind::Playlists => Some(&self.playlists),
        }
    }
}

impl Section {
    fn append(&self, row: &impl IsA<gtk::Widget>, in_library: bool) {
        if in_library {
            self.list.insert(row, self.library_rows.get());
            self.library_rows.set(self.library_rows.get() + 1);
        } else {
            self.list.append(row);
        }
        self.container.set_visible(true);
    }
}

/// What's already listed from the library, so Spotify's matches of it
/// aren't listed twice.
#[derive(Debug, Default)]
pub struct Shown {
    artists: Vec<i64>,
    album_groups: Vec<String>,
    tracks: Vec<i64>,
    playlists: Vec<i64>,
}

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct SearchView {
        /// The filters above the results (not over the recent searches).
        pub filters: adw::Bin,
        pub kinds: adw::ToggleGroup,
        pub sources: adw::ToggleGroup,
        pub stack: gtk::Stack,
        pub sections: gtk::Box,
        pub empty: adw::StatusPage,
        pub current: std::cell::RefCell<Option<Sections>>,
        pub shown: std::cell::RefCell<Shown>,
        /// Spotify results (§9.4): searched a moment after typing stops;
        /// older answers are dropped.
        pub spotify_generation: std::cell::Cell<u64>,
        pub spotify_timer: std::cell::RefCell<Option<glib::SourceId>>,
        /// Spotify's answers this session, by query (lowercase).
        pub spotify_cache:
            std::cell::RefCell<HashMap<String, Rc<jewelcase_spotify::SearchResults>>>,
        pub last_query: std::cell::RefCell<String>,
        /// The query Spotify was asked for (trimmed), for *More From
        /// Spotify*.
        pub spotify_query: std::cell::RefCell<String>,
        pub settings: std::cell::OnceCell<gio::Settings>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SearchView {
        const NAME: &'static str = "JewelcaseSearchView";
        type Type = super::SearchView;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for SearchView {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                // A recent search was clicked: put it into the field.
                vec![
                    Signal::builder("query-chosen")
                        .param_types([String::static_type()])
                        .build(),
                ]
            })
        }

        fn constructed(&self) {
            self.parent_constructed();
            let _ = self.settings.set(gio::Settings::new(APP_ID));
            self.sections.set_orientation(gtk::Orientation::Vertical);
            self.sections.set_spacing(24);
            self.sections.set_margin_top(18);
            self.sections.set_margin_bottom(18);
            self.sections.set_margin_start(18);
            self.sections.set_margin_end(18);
            // A card over the library (window.blp): as tall as the results,
            // scrolling once it reaches the window's bottom.
            let scroller = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .propagate_natural_height(true)
                .child(&self.sections)
                .build();
            self.empty.set_icon_name(Some("edit-find-symbolic"));
            self.empty.set_title(&gettext("No Results"));
            self.empty.add_css_class("compact");
            self.stack.set_vhomogeneous(false);
            self.stack.set_interpolate_size(true);
            self.stack.add_named(&scroller, Some("results"));
            self.stack.add_named(&self.empty, Some("empty"));
            let obj = self.obj();
            obj.set_overflow(gtk::Overflow::Hidden);
            // The filters: a kind, and a source. Kept while the app runs.
            for (name, label) in [
                ("all", gettext("All")),
                ("artists", gettext("Artists")),
                ("albums", gettext("Albums")),
                ("songs", gettext("Songs")),
                ("playlists", gettext("Playlists")),
            ] {
                self.kinds
                    .add(adw::Toggle::builder().name(name).label(label).build());
            }
            for (name, label) in [
                ("everywhere", gettext("Everywhere")),
                ("library", gettext("Library")),
                ("spotify", gettext("Spotify")),
            ] {
                self.sources
                    .add(adw::Toggle::builder().name(name).label(label).build());
            }
            self.kinds.set_active_name(Some("all"));
            self.sources.set_active_name(Some("everywhere"));
            crate::widgets::set_accessible_label(
                self.kinds.upcast_ref::<gtk::Widget>(),
                &gettext("What to Show"),
            );
            crate::widgets::set_accessible_label(
                self.sources.upcast_ref::<gtk::Widget>(),
                &gettext("Where From"),
            );
            for group in [&self.kinds, &self.sources] {
                group.connect_active_name_notify(glib::clone!(
                    #[weak]
                    obj,
                    move |_| {
                        let query = obj.imp().last_query.borrow().clone();
                        if !query.trim().is_empty() {
                            obj.search(&query);
                        }
                    }
                ));
            }
            // The two groups side by side, each on its own line when the
            // card is narrow (at their natural widths: shrunk, their labels
            // would be cut to "A…").
            let row = adw::WrapBox::builder()
                .child_spacing(12)
                .line_spacing(6)
                .wrap_policy(adw::WrapPolicy::Natural)
                .margin_top(12)
                .margin_start(18)
                .margin_end(18)
                .build();
            row.append(&self.kinds);
            row.append(&self.sources);
            self.filters.set_child(Some(&row));
            self.filters.set_visible(false);
            let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
            content.append(&self.filters);
            content.append(&self.stack);
            obj.set_child(Some(&content));
            // Spotify connected while a search shows: add its results.
            JewelcaseApplication::get()
                .spotify()
                .connect_status_notify(glib::clone!(
                    #[weak]
                    obj,
                    move |spotify| {
                        let query = obj.imp().last_query.borrow().clone();
                        // Only results on screen (not a search closed long ago).
                        if spotify.is_connected() && !query.trim().is_empty() && obj.is_mapped() {
                            obj.search(&query);
                        }
                    }
                ));
        }
    }

    impl WidgetImpl for SearchView {}
    impl BinImpl for SearchView {}
}

glib::wrapper! {
    pub struct SearchView(ObjectSubclass<imp::SearchView>)
        @extends gtk::Widget, adw::Bin,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl SearchView {
    fn settings(&self) -> &gio::Settings {
        self.imp().settings.get().expect("set in constructed")
    }

    fn clear(&self) {
        let imp = self.imp();
        while let Some(child) = imp.sections.first_child() {
            imp.sections.remove(&child);
        }
        imp.current.replace(None);
        imp.shown.replace(Shown::default());
        // A Spotify search on its way is for another query now.
        imp.spotify_generation.set(imp.spotify_generation.get() + 1);
        if let Some(timer) = imp.spotify_timer.take() {
            timer.remove();
        }
    }

    /// The kind and source chosen in the filters.
    fn filters(&self) -> (Kind, Source) {
        let imp = self.imp();
        (
            Kind::from_name(imp.kinds.active_name().as_deref().unwrap_or("all")),
            Source::from_name(imp.sources.active_name().as_deref().unwrap_or("everywhere")),
        )
    }

    /// Chooses the filters by their names (the screenshot hook).
    pub fn set_filters(&self, kind: &str, source: &str) {
        self.imp().kinds.set_active_name(Some(kind));
        self.imp().sources.set_active_name(Some(source));
    }

    pub fn search(&self, query: &str) {
        let imp = self.imp();
        imp.last_query.replace(query.to_owned());
        self.clear();
        imp.filters.set_visible(true);
        let (kind, source) = self.filters();
        let library = JewelcaseApplication::get().library();
        let results = if source.library() {
            let limit = if kind == Kind::All {
                LIMIT
            } else {
                LIMIT_ONE_KIND
            };
            kind.keep(library.search(query, limit))
        } else {
            SearchResults::default()
        };
        let connected = JewelcaseApplication::get().spotify().is_connected();
        let spotify = source.spotify() && connected && query.trim().chars().count() >= 2;
        if results.is_empty() && !spotify {
            let why = match source {
                Source::Spotify if !connected => gettext("Spotify isn’t connected"),
                Source::Spotify => gettext("Type at least two letters to search Spotify"),
                _ => gettext("Nothing in your library matches"),
            };
            imp.empty.set_description(Some(&why));
            imp.stack.set_visible_child_name("empty");
            return;
        }
        imp.stack.set_visible_child_name("results");
        let sections = self.build_sections();
        self.add_artists(&sections.artists, &results);
        self.add_albums(&sections.albums, &results);
        self.add_songs(&sections.songs, &results);
        self.add_playlists(&sections.playlists, &results);
        imp.current.replace(Some(sections));
        if spotify {
            self.search_spotify(query.trim());
        }
    }

    // Recent searches ----------------------------------------------------------

    /// The last searches, for an empty field; false if there are none.
    pub fn show_recent(&self) -> bool {
        self.clear();
        self.imp().filters.set_visible(false);
        let recent = self.settings().strv("recent-searches");
        if recent.is_empty() {
            return false;
        }
        let imp = self.imp();
        imp.stack.set_visible_child_name("results");
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        let heading = gtk::Box::builder().spacing(6).build();
        heading.append(
            &gtk::Label::builder()
                .label(gettext("Recent Searches"))
                .xalign(0.0)
                .hexpand(true)
                .css_classes(["heading"])
                .build(),
        );
        let clear = gtk::Button::builder()
            .label(gettext("_Clear"))
            .use_underline(true)
            .css_classes(["flat", "caption"])
            .build();
        clear.connect_clicked(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_| {
                let _ = view
                    .settings()
                    .set_strv("recent-searches", Vec::<String>::new());
                view.clear();
                view.imp().empty.set_description(None);
                view.imp().stack.set_visible_child_name("empty");
            }
        ));
        heading.append(&clear);
        let section = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(9)
            .build();
        section.append(&heading);
        section.append(&list);
        imp.sections.append(&section);
        for query in recent.iter() {
            let query = query.to_string();
            let icon = gtk::Image::from_icon_name("document-open-recent-symbolic");
            let row = Self::row(&query, "", &icon);
            row.connect_activated(glib::clone!(
                #[weak(rename_to = view)]
                self,
                move |_| view.emit_by_name::<()>("query-chosen", &[&query])
            ));
            list.append(&row);
        }
        true
    }

    /// A search that was used (a result opened, or the search closed with
    /// it): first in "Recent Searches".
    pub fn remember(&self, query: &str) {
        let query = query.trim();
        if query.chars().count() < 2 {
            return;
        }
        let mut recent: Vec<String> = self
            .settings()
            .strv("recent-searches")
            .iter()
            .map(|q| q.to_string())
            .filter(|q| q.to_lowercase() != query.to_lowercase())
            .collect();
        recent.insert(0, query.to_owned());
        recent.truncate(RECENT);
        let _ = self.settings().set_strv("recent-searches", recent);
    }

    // Sections -------------------------------------------------------------------

    fn build_sections(&self) -> Sections {
        let section = |title: &str| {
            let list = gtk::ListBox::builder()
                .selection_mode(gtk::SelectionMode::None)
                .css_classes(["boxed-list"])
                .build();
            // Opening a result makes the search a recent one.
            list.connect_row_activated(glib::clone!(
                #[weak(rename_to = view)]
                self,
                move |_, _| {
                    let query = view.imp().last_query.borrow().clone();
                    view.remember(&query);
                }
            ));
            let container = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(9)
                .visible(false)
                .build();
            container.append(
                &gtk::Label::builder()
                    .label(title)
                    .xalign(0.0)
                    .css_classes(["heading"])
                    .build(),
            );
            container.append(&list);
            self.imp().sections.append(&container);
            Section {
                container,
                list,
                library_rows: Rc::default(),
            }
        };
        let artists = section(&gettext("Artists"));
        let albums = section(&gettext("Albums"));
        let songs = section(&gettext("Songs"));
        let playlists = section(&gettext("Playlists"));
        let status = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .visible(false)
            .css_classes(["dim-label", "caption"])
            .build();
        self.imp().sections.append(&status);
        Sections {
            artists,
            albums,
            songs,
            playlists,
            status,
        }
    }

    fn row(title: &str, subtitle: &str, prefix: &impl IsA<gtk::Widget>) -> adw::ActionRow {
        // Texts after building: in the builder they'd be read as markup
        // before use-markup is off ("&TEAM").
        let row = adw::ActionRow::builder()
            .use_markup(false)
            .activatable(true)
            .build();
        row.set_title(title);
        row.set_subtitle(subtitle);
        row.add_prefix(prefix);
        row
    }

    fn small_cover(key: Option<&str>) -> CoverImage {
        let cover: CoverImage = glib::Object::builder()
            .property("min-size", 40)
            .property("natural-size", 40)
            .build();
        cover.set_cover(key);
        cover
    }

    /// In the library, or only on Spotify: the last thing on every row.
    fn source_icon(in_library: bool) -> gtk::Image {
        if in_library {
            gtk::Image::builder()
                .icon_name("folder-music-symbolic")
                .valign(gtk::Align::Center)
                .tooltip_text(gettext("In your library"))
                .css_classes(["dim-label"])
                .build()
        } else {
            super::spotify_logo(&gettext("On Spotify, not in your library"))
        }
    }

    // The library's results --------------------------------------------------------

    fn add_artists(&self, section: &Section, results: &SearchResults) {
        for id in &results.artists {
            self.library_artist(section, *id);
        }
    }

    fn library_artist(&self, section: &Section, id: i64) {
        let library = JewelcaseApplication::get().library();
        let Some(artist) = library.artist(id) else {
            return;
        };
        self.imp().shown.borrow_mut().artists.push(id);
        let info = artist.artist();
        let picture = Self::small_cover(None);
        picture.set_placeholder_icon("avatar-default-symbolic");
        let portrait = info
            .portrait_key
            .as_deref()
            .and_then(|k| covers::portrait(k, CoverSize::Grid));
        picture.set_texture(portrait.as_ref());
        let row = Self::row(&info.name, &format::song_count(info.track_count), &picture);
        row.add_suffix(&Self::source_icon(true));
        row.set_action_name(Some("win.show-artist"));
        row.set_action_target_value(Some(&id.to_variant()));
        section.append(&row, true);
    }

    fn add_albums(&self, section: &Section, results: &SearchResults) {
        let library = JewelcaseApplication::get().library();
        for id in &results.albums {
            if let Some(group) = library.album_group(*id) {
                self.library_album(section, &group);
            }
        }
    }

    /// One row per album group (merged editions).
    fn library_album(&self, section: &Section, group: &crate::objects::AlbumObject) {
        let key = group.group_key().to_owned();
        if self.imp().shown.borrow().album_groups.contains(&key) {
            return;
        }
        self.imp().shown.borrow_mut().album_groups.push(key);
        let album = group.album();
        let mut subtitle = artist_or_unknown(&album.artist_credit);
        if let Some(year) = album.year {
            subtitle.push_str(&format!(" · {year}"));
        }
        let row = Self::row(
            &album.title,
            &subtitle,
            &Self::small_cover(album.cover_key.as_deref()),
        );
        row.add_suffix(&Self::source_icon(true));
        row.set_action_name(Some("win.show-album"));
        row.set_action_target_value(Some(&album.id.to_variant()));
        section.append(&row, true);
    }

    fn add_songs(&self, section: &Section, results: &SearchResults) {
        let library = JewelcaseApplication::get().library();
        let mut tracks = library.tracks(&results.tracks);
        for id in &results.tracks {
            if let Some(track) = tracks.remove(id) {
                self.song(section, TrackObject::new(track), None, None);
            }
        }
    }

    /// A song row: a library song (plays its album from it), or a Spotify
    /// song to stream (`spotify`: its cover image address).
    fn song(
        &self,
        section: &Section,
        track: TrackObject,
        subtitle: Option<String>,
        spotify: Option<(Option<String>, &mut SpotifyCovers)>,
    ) {
        let in_library = !track.is_stand_in();
        if in_library {
            if self.imp().shown.borrow().tracks.contains(&track.id()) {
                return;
            }
            self.imp().shown.borrow_mut().tracks.push(track.id());
        }
        let subtitle = subtitle.unwrap_or_else(|| {
            [track.artist(), track.track().album_title.clone()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" — ")
        });
        let cover = Self::small_cover(track.cover_key());
        if let Some((Some(url), covers)) = spotify {
            covers.push((cover.clone(), url));
        }
        let row = Self::row(&track.title(), &subtitle, &cover);
        row.add_suffix(
            &gtk::Label::builder()
                .label(format::duration(track.duration_ms()))
                .css_classes(["dim-label", "numeric"])
                .build(),
        );
        let queue = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text(gettext("Add to Queue"))
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular"])
            .build();
        let to_queue = track.clone();
        queue.connect_clicked(move |_| {
            JewelcaseApplication::get()
                .player()
                .add_to_queue(vec![to_queue.clone()]);
        });
        row.add_suffix(&queue);
        let playlist = gtk::Button::builder()
            .icon_name("view-list-bullet-symbolic")
            .tooltip_text(gettext("Add to Playlist…"))
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular"])
            .build();
        let id = track.id();
        playlist.connect_clicked(move |button| {
            super::playlist_picker::show(button, vec![id]);
        });
        row.add_suffix(&playlist);
        row.add_suffix(&Self::source_icon(in_library));
        row.connect_activated(move |_| {
            let app = JewelcaseApplication::get();
            // A library song plays its album from it; a Spotify song alone.
            if let Some(album_id) = track.track().album_id.filter(|_| !track.is_stand_in()) {
                let index = app
                    .library()
                    .album_tracks(album_id)
                    .iter()
                    .position(|t| t.id() == track.id());
                app.player().play_album(album_id, index, false);
            } else {
                app.player()
                    .play_tracks(vec![track.clone()], Some(0), false, None);
            }
        });
        section.append(&row, in_library);
    }

    fn add_playlists(&self, section: &Section, results: &SearchResults) {
        for id in &results.playlists {
            self.library_playlist(section, *id);
        }
    }

    fn library_playlist(&self, section: &Section, id: i64) {
        let library = JewelcaseApplication::get().library();
        let Some(playlist) = library.playlist(id) else {
            return;
        };
        if self.imp().shown.borrow().playlists.contains(&id) {
            return;
        }
        self.imp().shown.borrow_mut().playlists.push(id);
        let row = Self::row(
            &playlist.playlist().name,
            &format::song_count(playlist.playlist().item_count),
            &Self::small_cover(playlist.cover_key().as_deref()),
        );
        row.add_suffix(&Self::source_icon(true));
        row.set_action_name(Some("win.show-playlist"));
        row.set_action_target_value(Some(&id.to_variant()));
        section.append(&row, true);
    }

    // Spotify's results ---------------------------------------------------------

    /// Asks Spotify (or takes this session's answer), then adds what it
    /// found to the sections.
    fn search_spotify(&self, query: &str) {
        let imp = self.imp();
        let generation = imp.spotify_generation.get();
        let key = query.to_lowercase();
        imp.spotify_query.replace(query.to_owned());
        if let Some(cached) = imp.spotify_cache.borrow().get(&key).cloned() {
            self.show_spotify(&cached);
            return;
        }
        if let Some(sections) = imp.current.borrow().as_ref() {
            sections.status.set_label(&gettext("Searching Spotify…"));
            sections.status.set_visible(true);
        }
        let query = query.to_owned();
        let timer = glib::timeout_add_local_once(
            SPOTIFY_DELAY,
            glib::clone!(
                #[weak(rename_to = view)]
                self,
                move || {
                    view.imp().spotify_timer.replace(None);
                    glib::spawn_future_local(async move {
                        view.fetch_spotify(query, key, generation).await;
                    });
                }
            ),
        );
        imp.spotify_timer.replace(Some(timer));
    }

    async fn fetch_spotify(&self, query: String, key: String, generation: u64) {
        let result = JewelcaseApplication::get()
            .spotify()
            .call(move |client| client.search_all(&query))
            .await;
        let imp = self.imp();
        if let Ok(found) = &result {
            imp.spotify_cache
                .borrow_mut()
                .insert(key, Rc::new(found.clone()));
        }
        // Typed on meanwhile: the answer is kept, but not shown.
        if imp.spotify_generation.get() != generation {
            return;
        }
        match result {
            Ok(found) => self.show_spotify(&found),
            Err(err) => {
                tracing::warn!("Spotify search failed: {err}");
                if let Some(sections) = imp.current.borrow().as_ref() {
                    sections.status.set_label(&format!(
                        "{} {}",
                        gettext("Spotify can’t be searched right now."),
                        crate::spotify::search_error(&err)
                    ));
                    sections.status.set_visible(true);
                }
            }
        }
    }

    fn show_spotify(&self, found: &jewelcase_spotify::SearchResults) {
        let Some(sections) = self.imp().current.borrow().clone() else {
            return;
        };
        sections.status.set_visible(false);
        let mut covers = SpotifyCovers::new();
        let (kind, source) = self.filters();
        if kind.shows(Kind::Artists) {
            self.spotify_artists(&sections.artists, &found.artists, &mut covers);
        }
        if kind.shows(Kind::Albums) {
            self.spotify_albums(&sections.albums, &found.albums, &mut covers);
        }
        if kind.shows(Kind::Songs) {
            self.spotify_songs(&sections.songs, &found.tracks, &mut covers);
        }
        if kind.shows(Kind::Playlists) {
            self.spotify_playlists(&sections.playlists, &found.playlists, &mut covers);
        }
        if let (Some(section), Some(spotify_kind)) = (sections.of(kind), kind.spotify())
            && let Some(offset) = found.next_offset(spotify_kind)
        {
            self.more_row(section, kind, offset);
        }
        let nothing = [
            &sections.artists,
            &sections.albums,
            &sections.songs,
            &sections.playlists,
        ]
        .iter()
        .all(|s| !s.container.get_visible());
        if nothing {
            let imp = self.imp();
            imp.empty
                .set_description(Some(&if source == Source::Spotify {
                    gettext("Nothing on Spotify matches")
                } else {
                    gettext("Nothing in your library or on Spotify matches")
                }));
            imp.stack.set_visible_child_name("empty");
        }
        Self::load_covers(covers);
    }

    /// Covers arrive after the rows.
    fn load_covers(covers: SpotifyCovers) {
        let urls: Vec<String> = covers.iter().map(|(_, url)| url.clone()).collect();
        crate::spotify::fetch_covers(urls, move || {
            for (cover, url) in &covers {
                cover.set_cover(Some(&crate::spotify::image_cover_key(url)));
            }
        });
    }

    /// *More From Spotify* at the end of the one kind shown: the next
    /// results of that kind from `offset` on, one request a click.
    fn more_row(&self, section: &Section, kind: Kind, offset: u32) {
        let row = adw::ButtonRow::builder()
            .title(gettext("More From Spotify"))
            .start_icon_name("list-add-symbolic")
            .build();
        let generation = self.imp().spotify_generation.get();
        row.connect_activated(glib::clone!(
            #[weak(rename_to = view)]
            self,
            #[strong]
            section,
            move |row| {
                row.set_sensitive(false);
                glib::spawn_future_local(glib::clone!(
                    #[weak]
                    view,
                    #[strong]
                    section,
                    #[weak]
                    row,
                    async move {
                        view.fetch_more(&section, &row, kind, offset, generation)
                            .await;
                    }
                ));
            }
        ));
        section.append(&row, false);
    }

    async fn fetch_more(
        &self,
        section: &Section,
        row: &adw::ButtonRow,
        kind: Kind,
        offset: u32,
        generation: u64,
    ) {
        let Some(spotify_kind) = kind.spotify() else {
            return;
        };
        let imp = self.imp();
        let query = imp.spotify_query.borrow().clone();
        let asked = query.clone();
        let result = JewelcaseApplication::get()
            .spotify()
            .call(move |client| client.search_more(&asked, spotify_kind, offset))
            .await;
        // Typed on, or another filter: these rows are gone.
        if imp.spotify_generation.get() != generation {
            return;
        }
        let page = match result {
            Ok(page) => page,
            Err(err) => {
                tracing::warn!("Spotify search failed: {err}");
                row.set_sensitive(true);
                if let Some(sections) = imp.current.borrow().as_ref() {
                    sections.status.set_label(&format!(
                        "{} {}",
                        gettext("Spotify can’t be searched right now."),
                        crate::spotify::search_error(&err)
                    ));
                    sections.status.set_visible(true);
                }
                return;
            }
        };
        // Kept with the first answer, so the filters can change and come back.
        let key = query.to_lowercase();
        let next = page.next_offset(spotify_kind);
        if let Some(cached) = imp.spotify_cache.borrow_mut().get_mut(&key) {
            Rc::make_mut(cached).extend(spotify_kind, page.clone());
        }
        section.list.remove(row);
        let mut covers = SpotifyCovers::new();
        match kind {
            Kind::Artists => self.spotify_artists(section, &page.artists, &mut covers),
            Kind::Albums => self.spotify_albums(section, &page.albums, &mut covers),
            Kind::Songs => self.spotify_songs(section, &page.tracks, &mut covers),
            Kind::Playlists => self.spotify_playlists(section, &page.playlists, &mut covers),
            Kind::All => {}
        }
        Self::load_covers(covers);
        if let Some(offset) = next {
            self.more_row(section, kind, offset);
        }
    }

    fn spotify_artists(
        &self,
        section: &Section,
        artists: &[jewelcase_spotify::Artist],
        covers: &mut SpotifyCovers,
    ) {
        let library = JewelcaseApplication::get().library();
        for artist in artists {
            if let Some(id) = library.artist_for_spotify(&artist.id, &artist.name) {
                if !self.imp().shown.borrow().artists.contains(&id) {
                    self.library_artist(section, id);
                }
                continue;
            }
            let picture = Self::small_cover(None);
            picture.set_placeholder_icon("avatar-default-symbolic");
            if let Some(url) = &artist.image_url {
                covers.push((picture.clone(), url.clone()));
            }
            let subtitle = if artist.genres.is_empty() {
                gettext("Artist")
            } else {
                artist
                    .genres
                    .iter()
                    .take(2)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let row = Self::row(&artist.name, &subtitle, &picture);
            row.add_suffix(&Self::source_icon(false));
            row.set_action_name(Some("win.show-spotify-artist"));
            row.set_action_target_value(Some(&artist.id.to_variant()));
            section.append(&row, false);
        }
    }

    fn spotify_albums(
        &self,
        section: &Section,
        albums: &[jewelcase_spotify::Album],
        covers: &mut SpotifyCovers,
    ) {
        let library = JewelcaseApplication::get().library();
        for album in albums {
            let album = crate::spotify::album_from(album);
            if let Some(group) = library.local_album_for(&album) {
                self.library_album(section, &group);
                continue;
            }
            let cover = Self::small_cover(None);
            if let Some(url) = &album.image_url {
                covers.push((cover.clone(), url.clone()));
            }
            let mut subtitle = album.artist.clone();
            if let Some(year) = album.year {
                subtitle.push_str(&format!(" · {year}"));
            }
            let row = Self::row(&album.name, &subtitle, &cover);
            row.add_suffix(&Self::source_icon(false));
            row.connect_activated(move |row| {
                if let Some(window) = row.root().and_downcast::<crate::window::JewelcaseWindow>() {
                    window.show_spotify_album(&album);
                }
            });
            section.append(&row, false);
        }
    }

    fn spotify_songs(
        &self,
        section: &Section,
        tracks: &[jewelcase_spotify::Track],
        covers: &mut SpotifyCovers,
    ) {
        let library = JewelcaseApplication::get().library();
        let index = library.match_index();
        for track in tracks.iter().filter(|t| !t.is_local) {
            // In the library? Then it's the library's song (§9.5).
            let local = index
                .find(&jewelcase_core::matching::RemoteTrack {
                    title: track.name.clone(),
                    artists: track.artists.clone(),
                    album: track.album.clone(),
                    duration_ms: track.duration_ms,
                    isrc: track.isrc.clone(),
                })
                .filter(|(_, c)| *c == jewelcase_core::matching::Confidence::Sure)
                .map(|(id, _)| id);
            if let Some(id) = local {
                if let Some(song) = library.tracks(&[id]).remove(&id) {
                    self.song(section, TrackObject::new(song), None, None);
                }
                continue;
            }
            // A Spotify song to stream (negative ID, §9.4).
            let id = library.db().and_then(|mut db| {
                db.stream_track_id(&jewelcase_core::db::StreamInfo {
                    uri: track.uri.clone(),
                    title: track.name.clone(),
                    artist: Some(track.artist_line()),
                    album: track.album.clone(),
                    duration_ms: track.duration_ms,
                    isrc: track.isrc.clone(),
                    image_url: track.image_url.clone(),
                })
                .ok()
            });
            let Some(object) = id
                .and_then(|id| library.tracks(&[id]).remove(&id))
                .map(TrackObject::new)
            else {
                continue;
            };
            let subtitle = [Some(track.artist_line()), track.album.clone()]
                .into_iter()
                .flatten()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" — ");
            self.song(
                section,
                object,
                Some(subtitle),
                Some((track.image_url.clone(), covers)),
            );
        }
    }

    fn spotify_playlists(
        &self,
        section: &Section,
        playlists: &[jewelcase_spotify::PlaylistSummary],
        covers: &mut SpotifyCovers,
    ) {
        let library = JewelcaseApplication::get().library();
        let store = library.playlists();
        let linked = |spotify_id: &str| {
            (0..store.n_items())
                .filter_map(|i| {
                    store
                        .item(i)
                        .and_downcast::<crate::objects::PlaylistObject>()
                })
                .find(|p| p.playlist().spotify_id.as_deref() == Some(spotify_id))
                .map(|p| p.id())
        };
        for playlist in playlists {
            if let Some(id) = linked(&playlist.id) {
                self.library_playlist(section, id);
                continue;
            }
            let cover = Self::small_cover(None);
            if let Some(url) = &playlist.image_url {
                covers.push((cover.clone(), url.clone()));
            }
            let subtitle = playlist
                .owner_name
                .as_deref()
                // Translators: {} is a Spotify user name.
                .map(|owner| gettext("Playlist by {}").replace("{}", owner))
                .unwrap_or_default();
            let row = Self::row(&playlist.name, &subtitle, &cover);
            row.add_suffix(&Self::source_icon(false));
            // Spotify doesn't show apps others' playlists' songs: it plays
            // in Soloist, or opens in Spotify.
            let id = playlist.id.clone();
            row.set_tooltip_text(Some(&gettext("Plays with Soloist, or opens in Spotify")));
            row.connect_activated(move |_| {
                let spotify = JewelcaseApplication::get().spotify();
                let uri = format!("spotify:playlist:{id}");
                if !(spotify.can_stream() && spotify.soloist_play_context(&uri)) {
                    crate::spotify::open_on_spotify("playlist", &id);
                }
            });
            section.append(&row, false);
        }
    }
}

impl Default for SearchView {
    fn default() -> Self {
        glib::Object::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_keeps_its_own_results() {
        let results = SearchResults {
            artists: vec![1],
            albums: vec![2],
            tracks: vec![3],
            playlists: vec![4],
        };
        assert_eq!(Kind::All.keep(results.clone()), results);
        assert_eq!(
            Kind::Albums.keep(results.clone()),
            SearchResults {
                albums: vec![2],
                ..SearchResults::default()
            }
        );
        assert!(Kind::Songs.shows(Kind::Songs) && !Kind::Songs.shows(Kind::Artists));
    }

    #[test]
    fn the_toggles_names_choose_the_filters() {
        assert_eq!(Kind::from_name("playlists"), Kind::Playlists);
        assert_eq!(Kind::from_name("anything"), Kind::All);
        assert_eq!(Source::from_name("spotify"), Source::Spotify);
        assert!(Source::Library.library() && !Source::Library.spotify());
        assert!(Source::Everywhere.library() && Source::Everywhere.spotify());
    }
}
