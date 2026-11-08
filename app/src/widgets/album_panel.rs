//! The album side panel: cover, title, edition, actions and the tracklist
//! grouped by disc (CONCEPT.md §4.1, §3.4).

use std::cell::RefCell;
use std::sync::OnceLock;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use gtk::glib::subclass::Signal;

use super::album_tile::artist_or_unknown;
use super::{CoverBackdrop, CoverImage, TrackRow};
use crate::application::GatefoldApplication;
use crate::format;
use crate::objects::{AlbumObject, TrackObject};

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/sobakah/Gatefold/ui/album_panel.ui")]
    pub struct AlbumPanel {
        #[template_child]
        pub cover: TemplateChild<CoverImage>,
        #[template_child]
        pub backdrop: TemplateChild<CoverBackdrop>,
        #[template_child]
        pub title: TemplateChild<gtk::Label>,
        #[template_child]
        pub edition: TemplateChild<gtk::Label>,
        #[template_child]
        pub artists: TemplateChild<adw::WrapBox>,
        #[template_child]
        pub meta: TemplateChild<gtk::Label>,
        #[template_child]
        pub tracks: TemplateChild<gtk::Box>,
        #[template_child]
        pub versions_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub versions_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub versions: TemplateChild<gtk::DropDown>,

        pub album: RefCell<Option<AlbumObject>>,
        /// Index into the album's versions.
        pub version: RefCell<usize>,
        pub track_objects: RefCell<Vec<TrackObject>>,
        pub rows: RefCell<Vec<TrackRow>>,
        pub versions_handler: RefCell<Option<glib::SignalHandlerId>>,
        /// A Spotify album shown instead of a library one (§9.8).
        pub spotify: RefCell<Option<gatefold_core::db::SpotifyAlbum>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AlbumPanel {
        const NAME: &'static str = "GatefoldAlbumPanel";
        type Type = super::AlbumPanel;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            CoverImage::ensure_type();
            CoverBackdrop::ensure_type();
            klass.bind_template();
            klass.install_action("panel.close", None, |panel, _, _| {
                panel.emit_by_name::<()>("close", &[]);
            });
            klass.install_action("panel.play", None, |panel, _, _| panel.play(None, false));
            klass.install_action("panel.shuffle", None, |panel, _, _| panel.play(None, true));
            klass.install_action("panel.play-next", None, |panel, _, _| {
                let tracks = panel.imp().track_objects.borrow().clone();
                GatefoldApplication::get().player().play_next(tracks);
            });
            klass.install_action("panel.add-to-playlist", None, |panel, _, _| {
                let ids: Vec<i64> = panel
                    .imp()
                    .track_objects
                    .borrow()
                    .iter()
                    .map(TrackObject::id)
                    .collect();
                super::super::playlist_picker::show(panel, ids);
            });
            klass.install_action("panel.edit-tags", None, |panel, _, _| {
                let files = panel.track_paths();
                GatefoldApplication::get().tools().edit_tags(files);
            });
            klass.install_action("panel.find-lyrics", None, |panel, _, _| {
                if let Some(dir) = panel.album_dir() {
                    GatefoldApplication::get().tools().find_lyrics(&dir);
                }
            });
            klass.install_action("panel.lyrembed", None, |panel, _, _| {
                if let Some(dir) = panel.album_dir() {
                    GatefoldApplication::get().tools().lyrembed_page(&dir);
                }
            });
            klass.install_action("panel.open-spotify", None, |panel, _, _| {
                if let Some(album) = panel.imp().spotify.borrow().as_ref() {
                    crate::spotify::open_on_spotify("album", &album.spotify_id);
                }
            });
            klass.install_action("panel.add-to-queue", None, |panel, _, _| {
                let tracks = panel.imp().track_objects.borrow().clone();
                GatefoldApplication::get().player().add_to_queue(tracks);
            });
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for AlbumPanel {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| vec![Signal::builder("close").build()])
        }

        fn constructed(&self) {
            self.parent_constructed();
            self.backdrop.follow(&self.cover);
            let obj = self.obj();
            let player = GatefoldApplication::get().player();
            let id = player.connect_current_track_notify(glib::clone!(
                #[weak]
                obj,
                move |_| obj.update_playing()
            ));
            crate::widgets::disconnect_with(&*obj, &player, id);
        }
    }

    impl WidgetImpl for AlbumPanel {}
    impl BinImpl for AlbumPanel {}
}

glib::wrapper! {
    pub struct AlbumPanel(ObjectSubclass<imp::AlbumPanel>)
        @extends gtk::Widget, adw::Bin,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl AlbumPanel {
    pub fn album(&self) -> Option<AlbumObject> {
        self.imp().album.borrow().clone()
    }

    pub fn set_album(&self, album: &AlbumObject) {
        let imp = self.imp();
        let same_group = imp
            .album
            .borrow()
            .as_ref()
            .is_some_and(|a| a.group_key() == album.group_key());
        let version = if same_group {
            (*imp.version.borrow()).min(album.versions().len() - 1)
        } else {
            0
        };
        imp.album.replace(Some(album.clone()));
        imp.spotify.replace(None);
        self.set_spotify_mode(false);
        self.set_songs_actions(true);
        self.setup_versions(album, version);
        self.show_version(version);
    }

    /// Playing and queueing need the songs (a Spotify album's load first).
    fn set_songs_actions(&self, enabled: bool) {
        for action in [
            "panel.play",
            "panel.shuffle",
            "panel.play-next",
            "panel.add-to-queue",
            "panel.add-to-playlist",
        ] {
            self.action_set_enabled(action, enabled);
        }
    }

    /// Menu entries for files (tags, lyrics) or for Spotify.
    fn set_spotify_mode(&self, spotify: bool) {
        for action in ["panel.edit-tags", "panel.find-lyrics", "panel.lyrembed"] {
            self.action_set_enabled(action, !spotify);
        }
        self.action_set_enabled("panel.open-spotify", spotify);
    }

    pub fn spotify_album(&self) -> Option<gatefold_core::db::SpotifyAlbum> {
        self.imp().spotify.borrow().clone()
    }

    /// A Spotify album (§9.8): its songs play from the library where it has
    /// them and stream otherwise.
    pub fn set_spotify_album(&self, album: &gatefold_core::db::SpotifyAlbum) {
        let imp = self.imp();
        imp.album.replace(None);
        imp.spotify.replace(Some(album.clone()));
        self.set_spotify_mode(true);
        self.set_songs_actions(false);
        imp.versions_box.set_visible(false);
        imp.cover.set_cover(album.cover_key().as_deref());
        imp.cover
            .set_label(&gettext("Cover of {}").replace("{}", &album.name));
        imp.title.set_label(&album.name);
        imp.edition.set_visible(false);
        self.fill_spotify_artists(album);
        let mut meta = Vec::new();
        if let Some(year) = album.year {
            meta.push(year.to_string());
        }
        meta.push(match album.album_type.as_str() {
            "single" if album.total_tracks > 3 => gettext("EP"),
            "single" => gettext("Single"),
            "compilation" => gettext("Compilation"),
            _ => gettext("Album"),
        });
        meta.push(format::song_count(album.total_tracks));
        meta.push(gettext("On Spotify"));
        imp.meta.set_label(&meta.join(" · "));
        self.fill_tracks(&[], "");
        imp.track_objects.replace(Vec::new());
        let waiting = adw::Spinner::builder()
            .height_request(32)
            .margin_top(24)
            .build();
        imp.tracks.append(&waiting);
        let id = album.spotify_id.clone();
        let artist = album.artist.clone();
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = panel)]
            self,
            async move {
                let tracks = GatefoldApplication::get().spotify().album_tracks(&id).await;
                let still = panel
                    .imp()
                    .spotify
                    .borrow()
                    .as_ref()
                    .is_some_and(|a| a.spotify_id == id);
                if !still {
                    return;
                }
                match tracks {
                    Some(tracks) => {
                        panel.set_songs_actions(!tracks.is_empty());
                        panel.fill_tracks(&tracks, &artist);
                        panel.imp().track_objects.replace(tracks);
                        panel.update_playing();
                    }
                    None => {
                        panel.fill_tracks(&[], "");
                        panel.imp().tracks.append(
                            &gtk::Label::builder()
                                .label(gettext("Spotify can’t be reached right now"))
                                .css_classes(["dim-label"])
                                .margin_top(24)
                                .build(),
                        );
                    }
                }
            }
        ));
    }

    /// The album's artists: links to library artists of that name.
    /// Each artist links to their page: the library's when they're in it,
    /// else their Spotify page.
    fn fill_spotify_artists(&self, album: &gatefold_core::db::SpotifyAlbum) {
        let imp = self.imp();
        imp.artists.remove_all();
        let library = GatefoldApplication::get().library();
        for (name, spotify_id) in spotify_credits(album) {
            let local = library.artist_for_spotify(spotify_id.unwrap_or_default(), &name);
            let (action, target) = match (local, spotify_id) {
                (Some(id), _) => ("win.show-artist", id.to_variant()),
                (None, Some(id)) => ("win.show-spotify-artist", id.to_variant()),
                (None, None) => {
                    imp.artists.append(
                        &gtk::Label::builder()
                            .label(&name)
                            .css_classes(["heading"])
                            .build(),
                    );
                    continue;
                }
            };
            imp.artists.append(
                &gtk::Button::builder()
                    .label(&name)
                    .action_name(action)
                    .action_target(&target)
                    .tooltip_text(gettext("Go to {}").replace("{}", &name))
                    .css_classes(["flat", "artist-link", "heading"])
                    .build(),
            );
        }
    }

    fn setup_versions(&self, album: &AlbumObject, selected: usize) {
        let imp = self.imp();
        if let Some(handler) = imp.versions_handler.take() {
            imp.versions.disconnect(handler);
        }
        let versions = album.versions();
        imp.versions_box.set_visible(versions.len() > 1);
        if versions.len() < 2 {
            return;
        }
        let names: Vec<String> = versions
            .iter()
            .map(|v| v.edition.clone().unwrap_or_else(|| gettext("Standard")))
            .collect();
        // Translators: {} is a list of album editions, e.g. "L version, F version".
        imp.versions_label
            .set_label(&gettext("Versions: {}").replace("{}", &names.join(", ")));
        let model = gtk::StringList::new(&names.iter().map(String::as_str).collect::<Vec<_>>());
        imp.versions.set_model(Some(&model));
        imp.versions.set_selected(selected as u32);
        let handler = imp.versions.connect_selected_notify(glib::clone!(
            #[weak(rename_to = panel)]
            self,
            move |dropdown| panel.show_version(dropdown.selected() as usize)
        ));
        imp.versions_handler.replace(Some(handler));
    }

    fn show_version(&self, index: usize) {
        let imp = self.imp();
        let Some(album) = self.album() else { return };
        let Some(info) = album.versions().get(index) else {
            return;
        };
        imp.version.replace(index);

        imp.cover.set_cover(info.cover_key.as_deref());
        // Translators: accessible label of an album cover; {} is the album title.
        imp.cover
            .set_label(&gettext("Cover of {}").replace("{}", &info.title));
        imp.title.set_label(&info.title);
        imp.edition
            .set_label(info.edition.as_deref().unwrap_or_default());
        imp.edition.set_visible(info.edition.is_some());
        self.fill_artists(info);

        let mut meta = Vec::new();
        if let Some(year) = info.year {
            meta.push(year.to_string());
        }
        meta.push(release_type_name(info));
        let genres = GatefoldApplication::get().library().genres_of(&album);
        if !genres.is_empty() {
            meta.push(genres.into_iter().take(3).collect::<Vec<_>>().join(", "));
        }
        meta.push(format::song_count(info.track_count));
        meta.push(format::length(info.duration_ms));
        imp.meta.set_label(&meta.join(" · "));

        let tracks = GatefoldApplication::get().library().album_tracks(info.id);
        self.fill_tracks(&tracks, &info.artist_credit);
        imp.track_objects.replace(tracks);
        self.update_playing();
    }

    /// The files of the shown version.
    fn track_paths(&self) -> Vec<std::path::PathBuf> {
        self.imp()
            .track_objects
            .borrow()
            .iter()
            .map(|t| t.track().record.path.clone())
            .collect()
    }

    /// The album's folder (above disc subfolders), for lyrembed.
    fn album_dir(&self) -> Option<std::path::PathBuf> {
        let tracks = self.imp().track_objects.borrow();
        let path = &tracks.first()?.track().record.path;
        Some(gatefold_core::library::grouping::album_folder(path).to_path_buf())
    }

    fn fill_artists(&self, album: &gatefold_core::db::Album) {
        let imp = self.imp();
        imp.artists.remove_all();
        let linked = GatefoldApplication::get().library().album_artists(album.id);
        if linked.is_empty() {
            // Compilations and unknown artists have no page.
            imp.artists.append(
                &gtk::Label::builder()
                    .label(artist_or_unknown(&album.artist_credit))
                    .wrap(true)
                    .xalign(0.0)
                    .css_classes(["heading"])
                    .build(),
            );
            return;
        }
        for (id, name) in linked {
            let button = gtk::Button::builder()
                .label(&name)
                .action_name("win.show-artist")
                .action_target(&id.to_variant())
                // Translators: tooltip of an artist link; {} is the name.
                .tooltip_text(gettext("Go to {}").replace("{}", &name))
                .css_classes(["flat", "artist-link", "heading"])
                .build();
            imp.artists.append(&button);
        }
    }

    fn fill_tracks(&self, tracks: &[TrackObject], album_artist: &str) {
        let imp = self.imp();
        while let Some(child) = imp.tracks.first_child() {
            imp.tracks.remove(&child);
        }
        let multi_disc = tracks.iter().any(|t| t.disc() != 1);
        let mut rows = Vec::with_capacity(tracks.len());
        let mut current_disc = None;
        let mut list: Option<gtk::ListBox> = None;
        for track in tracks {
            let disc = track.disc();
            if list.is_none() || current_disc != Some(disc) {
                current_disc = Some(disc);
                if multi_disc {
                    // Translators: {} is a disc number.
                    let header = gtk::Label::builder()
                        .label(gettext("Disc {}").replace("{}", &disc.to_string()))
                        .xalign(0.0)
                        .css_classes(["heading"])
                        .build();
                    imp.tracks.append(&header);
                }
                let new_list = gtk::ListBox::builder()
                    .selection_mode(gtk::SelectionMode::None)
                    .activate_on_single_click(false)
                    .css_classes(["boxed-list"])
                    .build();
                new_list.connect_row_activated(glib::clone!(
                    #[weak(rename_to = panel)]
                    self,
                    move |_, row| {
                        if let Some(row) = row.downcast_ref::<TrackRow>() {
                            let id = row.track().id();
                            let index = panel
                                .imp()
                                .track_objects
                                .borrow()
                                .iter()
                                .position(|t| t.id() == id);
                            panel.play(index, false);
                        }
                    }
                ));
                imp.tracks.append(&new_list);
                list = Some(new_list);
            }
            let row = TrackRow::new(track, album_artist);
            row.set_tooltip_text(Some(&gettext("Double-click to play")));
            if let Some(list) = &list {
                list.append(&row);
            }
            rows.push(row);
        }
        imp.rows.replace(rows);
    }

    /// Points out one song: highlighted for a moment and scrolled to
    /// (e.g. opened from "Appears On").
    pub fn highlight_track(&self, track_id: i64) {
        let Some(row) = self
            .imp()
            .rows
            .borrow()
            .iter()
            .find(|r| r.track().id() == track_id)
            .cloned()
        else {
            return;
        };
        // After the panel has opened and laid out its rows.
        glib::timeout_add_local_once(std::time::Duration::from_millis(250), move || {
            row.add_css_class("highlighted");
            row.grab_focus();
            glib::timeout_add_local_once(std::time::Duration::from_millis(2500), move || {
                row.remove_css_class("highlighted");
            });
        });
    }

    fn update_playing(&self) {
        let current = GatefoldApplication::get()
            .player()
            .current_track()
            .map(|t| t.id());
        for row in self.imp().rows.borrow().iter() {
            row.set_playing(Some(row.track().id()) == current);
        }
    }

    fn play(&self, start: Option<usize>, shuffle: bool) {
        let imp = self.imp();
        if let Some(album) = imp.spotify.borrow().as_ref() {
            let tracks = imp.track_objects.borrow().clone();
            GatefoldApplication::get().player().play_tracks(
                tracks,
                start,
                shuffle,
                Some(album.name.clone()),
            );
            return;
        }
        let Some(album) = self.album() else { return };
        let Some(info) = album.versions().get(*imp.version.borrow()) else {
            return;
        };
        GatefoldApplication::get()
            .player()
            .play_album(info.id, start, shuffle);
    }
}

fn release_type_name(album: &gatefold_core::db::Album) -> String {
    use gatefold_core::library::titles::ReleaseType;
    if album.soundtrack {
        return gettext("Soundtrack");
    }
    if album.compilation {
        return gettext("Compilation");
    }
    match album.release_type {
        ReleaseType::Album => gettext("Album"),
        ReleaseType::Ep => gettext("EP"),
        ReleaseType::Single => gettext("Single"),
    }
}

/// A Spotify album's artists with their Spotify IDs. The credit is the
/// names joined with ", "; a name with a comma in it ("Tyler, the
/// Creator") keeps the IDs from lining up, so then only a sole artist
/// gets one.
fn spotify_credits(album: &gatefold_core::db::SpotifyAlbum) -> Vec<(String, Option<&str>)> {
    let names: Vec<&str> = album.artist.split(", ").filter(|n| !n.is_empty()).collect();
    if names.len() == album.artist_ids.len() {
        names
            .into_iter()
            .zip(album.artist_ids.iter().map(|id| Some(id.as_str())))
            .map(|(name, id)| (name.to_owned(), id))
            .collect()
    } else if album.artist_ids.len() == 1 {
        vec![(album.artist.clone(), Some(album.artist_ids[0].as_str()))]
    } else {
        names
            .into_iter()
            .map(|name| (name.to_owned(), None))
            .collect()
    }
}
