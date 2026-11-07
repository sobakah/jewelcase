//! Search results, grouped into Artists / Albums / Songs / Playlists
//! (CONCEPT.md §4.1), backed by the FTS5 index.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gatefold_core::db::SearchResults;
use gatefold_core::library::covers::CoverSize;
use gettextrs::gettext;
use gtk::glib;

use super::CoverImage;
use super::album_tile::artist_or_unknown;
use crate::application::GatefoldApplication;
use crate::objects::TrackObject;
use crate::{covers, format};

/// Results per section.
const LIMIT: usize = 8;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct SearchView {
        pub stack: gtk::Stack,
        pub sections: gtk::Box,
        pub empty: adw::StatusPage,
        /// Spotify results (§9.4): searched a moment after typing stops;
        /// older answers are dropped.
        pub spotify_generation: std::cell::Cell<u64>,
        pub spotify_timer: std::cell::RefCell<Option<glib::SourceId>>,
        pub last_query: std::cell::RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SearchView {
        const NAME: &'static str = "GatefoldSearchView";
        type Type = super::SearchView;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for SearchView {
        fn constructed(&self) {
            self.parent_constructed();
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
            obj.set_child(Some(&self.stack));
            // Spotify connected while a search shows: add its results.
            GatefoldApplication::get()
                .spotify()
                .connect_status_notify(glib::clone!(
                    #[weak]
                    obj,
                    move |spotify| {
                        let query = obj.imp().last_query.borrow().clone();
                        if spotify.is_connected() && !query.trim().is_empty() {
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
    pub fn search(&self, query: &str) {
        let imp = self.imp();
        imp.last_query.replace(query.to_owned());
        while let Some(child) = imp.sections.first_child() {
            imp.sections.remove(&child);
        }
        let library = GatefoldApplication::get().library();
        let results = library.search(query, LIMIT);
        let spotify = GatefoldApplication::get().spotify().is_connected()
            && query.trim().chars().count() >= 2;
        if results.is_empty() && !spotify {
            imp.empty
                .set_description(Some(&gettext("Nothing in your library matches")));
            imp.stack.set_visible_child_name("empty");
            return;
        }
        imp.stack.set_visible_child_name("results");
        self.add_artists(&results);
        self.add_albums(&results);
        self.add_songs(&results);
        self.add_playlists(&results);
        self.search_spotify(if spotify { Some(query.trim()) } else { None });
    }

    /// "On Spotify": songs from a Spotify search, to play, queue or add to
    /// a playlist; ones in the library are the library's.
    fn search_spotify(&self, query: Option<&str>) {
        let imp = self.imp();
        let generation = imp.spotify_generation.get() + 1;
        imp.spotify_generation.set(generation);
        if let Some(timer) = imp.spotify_timer.take() {
            timer.remove();
        }
        let Some(query) = query.map(str::to_owned) else {
            return;
        };
        let list = self.section(&gettext("On Spotify"));
        let waiting = adw::ActionRow::builder()
            .title(gettext("Searching…"))
            .build();
        list.append(&waiting);
        let timer = glib::timeout_add_local_once(
            std::time::Duration::from_millis(450),
            glib::clone!(
                #[weak(rename_to = view)]
                self,
                #[weak]
                list,
                move || {
                    view.imp().spotify_timer.replace(None);
                    glib::spawn_future_local(async move {
                        view.fill_spotify(&list, &query, generation).await;
                    });
                }
            ),
        );
        imp.spotify_timer.replace(Some(timer));
    }

    async fn fill_spotify(&self, list: &gtk::ListBox, query: &str, generation: u64) {
        let app = GatefoldApplication::get();
        let text = query.to_owned();
        let result = app
            .spotify()
            .call(move |client| client.search_tracks(&text, 10))
            .await;
        if self.imp().spotify_generation.get() != generation {
            return;
        }
        list.remove_all();
        let tracks = match result {
            Ok(tracks) => tracks,
            Err(err) => {
                tracing::warn!("Spotify search failed: {err}");
                list.append(
                    &adw::ActionRow::builder()
                        .title(gettext("Spotify can’t be searched right now"))
                        .build(),
                );
                return;
            }
        };
        let tracks: Vec<_> = tracks.into_iter().filter(|t| !t.is_local).collect();
        if tracks.is_empty() {
            list.append(
                &adw::ActionRow::builder()
                    .title(gettext("Nothing on Spotify"))
                    .build(),
            );
            return;
        }
        // In the library? Then it's the library's song (§9.5).
        let library = app.library();
        let aliases = library.aliases().clone();
        let local: Vec<Option<i64>> = library
            .db()
            .and_then(|db| db.match_tracks().ok())
            .map(|all| {
                let index = gatefold_core::matching::MatchIndex::new(all, &aliases);
                tracks
                    .iter()
                    .map(|t| {
                        index
                            .find(&gatefold_core::matching::RemoteTrack {
                                title: t.name.clone(),
                                artists: t.artists.clone(),
                                album: t.album.clone(),
                                duration_ms: t.duration_ms,
                                isrc: t.isrc.clone(),
                            })
                            .filter(|(_, c)| *c == gatefold_core::matching::Confidence::Sure)
                            .map(|(id, _)| id)
                    })
                    .collect()
            })
            .unwrap_or_else(|| vec![None; tracks.len()]);
        let mut covers = Vec::new();
        let mut urls = Vec::new();
        for (track, local) in tracks.iter().zip(local) {
            // A library song, or the Spotify song as one to stream.
            let id = local.or_else(|| {
                library.db().and_then(|mut db| {
                    db.stream_track_id(&gatefold_core::db::StreamInfo {
                        uri: track.uri.clone(),
                        title: track.name.clone(),
                        artist: Some(track.artist_line()),
                        album: track.album.clone(),
                        duration_ms: track.duration_ms,
                        isrc: track.isrc.clone(),
                        image_url: track.image_url.clone(),
                    })
                    .ok()
                })
            });
            let Some(object) = id
                .and_then(|id| library.tracks(&[id]).remove(&id))
                .map(TrackObject::new)
            else {
                continue;
            };
            let in_library = local.is_some();
            let mut subtitle = [Some(track.artist_line()), track.album.clone()]
                .into_iter()
                .flatten()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" — ");
            if in_library {
                subtitle = format!("{subtitle} · {}", gettext("In your library"));
            }
            let cover_key = object.cover_key().map(str::to_owned).or_else(|| {
                track
                    .image_url
                    .as_deref()
                    .map(crate::spotify::image_cover_key)
            });
            let cover = Self::small_cover(cover_key.as_deref());
            if !in_library && let Some(url) = &track.image_url {
                urls.push(url.clone());
                covers.push((cover.clone(), cover_key.clone()));
            }
            let row = Self::row(&track.name, &subtitle, &cover);
            row.add_suffix(
                &gtk::Label::builder()
                    .label(format::duration(track.duration_ms))
                    .css_classes(["dim-label", "numeric"])
                    .build(),
            );
            let queue = gtk::Button::builder()
                .icon_name("list-add-symbolic")
                .tooltip_text(gettext("Add to Queue"))
                .valign(gtk::Align::Center)
                .css_classes(["flat", "circular"])
                .build();
            let to_queue = object.clone();
            queue.connect_clicked(move |_| {
                GatefoldApplication::get()
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
            let id = object.id();
            playlist.connect_clicked(move |button| {
                super::playlist_picker::show(button, vec![id]);
            });
            row.add_suffix(&playlist);
            let to_play = object.clone();
            row.connect_activated(move |_| {
                GatefoldApplication::get().player().play_tracks(
                    vec![to_play.clone()],
                    Some(0),
                    false,
                    None,
                );
            });
            list.append(&row);
        }
        // Covers arrive after the rows.
        crate::spotify::fetch_covers(urls, move || {
            for (cover, key) in &covers {
                cover.set_cover(key.as_deref());
            }
        });
    }

    fn section(&self, title: &str) -> gtk::ListBox {
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        let section = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(9)
            .build();
        section.append(
            &gtk::Label::builder()
                .label(title)
                .xalign(0.0)
                .css_classes(["heading"])
                .build(),
        );
        section.append(&list);
        self.imp().sections.append(&section);
        list
    }

    fn row(title: &str, subtitle: &str, prefix: &impl IsA<gtk::Widget>) -> adw::ActionRow {
        let row = adw::ActionRow::builder()
            .use_markup(false)
            .title(title)
            .subtitle(subtitle)
            .activatable(true)
            .build();
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

    fn add_artists(&self, results: &SearchResults) {
        if results.artists.is_empty() {
            return;
        }
        let library = GatefoldApplication::get().library();
        let list = self.section(&gettext("Artists"));
        for id in &results.artists {
            let Some(artist) = library.artist(*id) else {
                continue;
            };
            let info = artist.artist();
            let picture = Self::small_cover(None);
            picture.set_placeholder_icon("avatar-default-symbolic");
            let portrait = info
                .portrait_key
                .as_deref()
                .and_then(|k| covers::portrait(k, CoverSize::Grid));
            picture.set_texture(portrait.as_ref());
            let row = Self::row(&info.name, &format::song_count(info.track_count), &picture);
            row.set_action_name(Some("win.show-artist"));
            row.set_action_target_value(Some(&id.to_variant()));
            list.append(&row);
        }
    }

    fn add_albums(&self, results: &SearchResults) {
        if results.albums.is_empty() {
            return;
        }
        let library = GatefoldApplication::get().library();
        let list = self.section(&gettext("Albums"));
        let mut shown = Vec::new();
        for id in &results.albums {
            // One row per album group (merged editions).
            let Some(group) = library.album_group(*id) else {
                continue;
            };
            if shown.contains(&group.group_key().to_owned()) {
                continue;
            }
            shown.push(group.group_key().to_owned());
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
            row.set_action_name(Some("win.show-album"));
            row.set_action_target_value(Some(&album.id.to_variant()));
            list.append(&row);
        }
    }

    fn add_songs(&self, results: &SearchResults) {
        if results.tracks.is_empty() {
            return;
        }
        let library = GatefoldApplication::get().library();
        let tracks = library.tracks(&results.tracks);
        let list = self.section(&gettext("Songs"));
        for id in &results.tracks {
            let Some(track) = tracks.get(id).cloned().map(TrackObject::new) else {
                continue;
            };
            let subtitle = [track.artist(), track.track().album_title.clone()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" — ");
            let row = Self::row(
                &track.title(),
                &subtitle,
                &Self::small_cover(track.cover_key()),
            );
            row.add_suffix(
                &gtk::Label::builder()
                    .label(format::duration(track.duration_ms()))
                    .css_classes(["dim-label", "numeric"])
                    .build(),
            );
            // Plays the song's album from this song.
            row.connect_activated(move |_| {
                let library = GatefoldApplication::get().library();
                let Some(album_id) = track.track().album_id else {
                    return;
                };
                let index = library
                    .album_tracks(album_id)
                    .iter()
                    .position(|t| t.id() == track.id());
                GatefoldApplication::get()
                    .player()
                    .play_album(album_id, index, false);
            });
            list.append(&row);
        }
    }

    fn add_playlists(&self, results: &SearchResults) {
        if results.playlists.is_empty() {
            return;
        }
        let library = GatefoldApplication::get().library();
        let list = self.section(&gettext("Playlists"));
        for id in &results.playlists {
            let Some(playlist) = library.playlist(*id) else {
                continue;
            };
            let row = Self::row(
                &playlist.playlist().name,
                &format::song_count(playlist.playlist().item_count),
                &Self::small_cover(playlist.cover_key().as_deref()),
            );
            row.set_action_name(Some("win.show-playlist"));
            row.set_action_target_value(Some(&id.to_variant()));
            list.append(&row);
        }
    }
}

impl Default for SearchView {
    fn default() -> Self {
        glib::Object::new()
    }
}
