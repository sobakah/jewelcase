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
            self.sections.set_margin_bottom(24);
            let clamp = adw::Clamp::builder()
                .maximum_size(720)
                .margin_start(12)
                .margin_end(12)
                .child(&self.sections)
                .build();
            let scroller = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(&clamp)
                .build();
            self.empty.set_icon_name(Some("edit-find-symbolic"));
            self.empty.set_title(&gettext("No Results"));
            self.stack.add_named(&scroller, Some("results"));
            self.stack.add_named(&self.empty, Some("empty"));
            self.obj().set_child(Some(&self.stack));
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
        while let Some(child) = imp.sections.first_child() {
            imp.sections.remove(&child);
        }
        let library = GatefoldApplication::get().library();
        let results = library.search(query, LIMIT);
        if results.is_empty() {
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
            .title(title)
            .subtitle(subtitle)
            .use_markup(false)
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
