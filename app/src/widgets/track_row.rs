//! A song row in the album panel. The chevron expands details in place
//! (CONCEPT.md §4.1); activating the row (double-click, Enter) plays it.

use std::cell::{OnceCell, RefCell};

use gettextrs::gettext;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::application::GatefoldApplication;
use crate::format;
use crate::objects::TrackObject;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct TrackRow {
        pub track: OnceCell<TrackObject>,
        pub number: gtk::Label,
        pub playing_icon: gtk::Image,
        pub revealer: gtk::Revealer,
        pub details_built: RefCell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TrackRow {
        const NAME: &'static str = "GatefoldTrackRow";
        type Type = super::TrackRow;
        type ParentType = gtk::ListBoxRow;
    }

    impl ObjectImpl for TrackRow {}
    impl WidgetImpl for TrackRow {}
    impl ListBoxRowImpl for TrackRow {}
}

glib::wrapper! {
    pub struct TrackRow(ObjectSubclass<imp::TrackRow>)
        @extends gtk::Widget, gtk::ListBoxRow,
        @implements gtk::Accessible, gtk::Actionable, gtk::Buildable, gtk::ConstraintTarget;
}

impl TrackRow {
    /// `album_artist` hides the track artist when it's the same.
    pub fn new(track: &TrackObject, album_artist: &str) -> Self {
        let row: Self = glib::Object::builder().build();
        row.add_css_class("track-row");
        let imp = row.imp();
        imp.track.set(track.clone()).expect("set once");

        let record = &track.track().record;
        imp.number.set_label(
            &record
                .tags
                .number
                .map_or_else(String::new, |n| n.to_string()),
        );
        imp.number.set_width_chars(2);
        imp.number.set_xalign(1.0);
        imp.number.add_css_class("dim-label");
        imp.number.add_css_class("numeric");
        imp.playing_icon
            .set_icon_name(Some("media-playback-start-symbolic"));
        imp.playing_icon.add_css_class("accent");
        imp.playing_icon.set_visible(false);
        imp.playing_icon.set_tooltip_text(Some(&gettext("Playing")));
        let number_box = gtk::Box::builder().width_request(24).build();
        number_box.append(&imp.number);
        number_box.append(&imp.playing_icon);
        imp.number.set_hexpand(true);

        let title = gtk::Label::builder()
            .label(track.title())
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .valign(gtk::Align::Center)
            .hexpand(true)
            .build();
        text.append(&title);
        if let Some(artist) = track
            .track()
            .record
            .tags
            .artist
            .as_deref()
            .filter(|a| !a.eq_ignore_ascii_case(album_artist))
        {
            text.append(
                &gtk::Label::builder()
                    .label(artist)
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .css_classes(["dim-label", "caption"])
                    .build(),
            );
        }

        let duration = gtk::Label::builder()
            .label(format::duration(track.duration_ms()))
            .css_classes(["dim-label", "numeric"])
            .build();

        let expand = gtk::ToggleButton::builder()
            .icon_name("pan-down-symbolic")
            .tooltip_text(gettext("Details"))
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular", "expand-button"])
            .build();
        expand.connect_toggled(glib::clone!(
            #[weak]
            row,
            move |button| row.set_expanded(button.is_active())
        ));

        let line = gtk::Box::builder()
            .spacing(12)
            .margin_start(12)
            .margin_end(6)
            .margin_top(6)
            .margin_bottom(6)
            .build();
        line.append(&number_box);
        line.append(&text);
        line.append(&duration);
        line.append(&expand);

        imp.revealer
            .set_transition_type(gtk::RevealerTransitionType::SlideDown);
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        content.append(&line);
        content.append(&imp.revealer);
        row.set_child(Some(&content));
        row.update_property(&[gtk::accessible::Property::Label(&track.title())]);
        row
    }

    pub fn track(&self) -> &TrackObject {
        self.imp().track.get().expect("set in new()")
    }

    pub fn set_playing(&self, playing: bool) {
        let imp = self.imp();
        imp.number.set_visible(!playing);
        imp.playing_icon.set_visible(playing);
        if playing {
            self.add_css_class("playing");
        } else {
            self.remove_css_class("playing");
        }
    }

    fn set_expanded(&self, expanded: bool) {
        let imp = self.imp();
        if expanded && !imp.details_built.replace(true) {
            imp.revealer.set_child(Some(&self.build_details()));
        }
        imp.revealer.set_reveal_child(expanded);
    }

    fn build_details(&self) -> gtk::Widget {
        let track = self.track();
        let tags = &track.track().record.tags;
        let grid = gtk::Grid::builder()
            .column_spacing(12)
            .row_spacing(3)
            .build();
        let mut row = 0;
        let mut add = |key: String, value: String| {
            grid.attach(
                &gtk::Label::builder()
                    .label(key)
                    .xalign(1.0)
                    .yalign(0.0)
                    .css_classes(["dim-label", "caption"])
                    .build(),
                0,
                row,
                1,
                1,
            );
            grid.attach(
                &gtk::Label::builder()
                    .label(value)
                    .xalign(0.0)
                    .wrap(true)
                    .selectable(true)
                    .hexpand(true)
                    .css_classes(["caption"])
                    .build(),
                1,
                row,
                1,
                1,
            );
            row += 1;
        };
        if let Some(artist) = &tags.artist {
            add(gettext("Artist"), artist.clone());
        }
        if let Some(composer) = &tags.composer {
            add(gettext("Composer"), composer.clone());
        }
        if !tags.genres.is_empty() {
            add(gettext("Genre"), tags.genres.join(", "));
        }
        if let Some(bpm) = tags.bpm {
            add(gettext("BPM"), format!("{bpm:.0}"));
        }
        add(gettext("Format"), format::audio_format(tags));
        if let Some(lyrics) = &tags.lyrics {
            let kind = if lyrics.synced {
                gettext("Synced lyrics")
            } else {
                gettext("Lyrics")
            };
            add(kind, lyrics.excerpt.clone());
        }

        let play_next = gtk::Button::builder()
            .label(gettext("Play Next"))
            .css_classes(["pill", "small"])
            .build();
        let add_to_queue = gtk::Button::builder()
            .label(gettext("Add to Queue"))
            .css_classes(["pill", "small"])
            .build();
        let track = track.clone();
        play_next.connect_clicked(glib::clone!(
            #[strong]
            track,
            move |_| GatefoldApplication::get()
                .player()
                .play_next(vec![track.clone()])
        ));
        add_to_queue.connect_clicked(move |_| {
            GatefoldApplication::get()
                .player()
                .add_to_queue(vec![track.clone()]);
        });
        let edit_tags = gtk::Button::builder()
            .label(gettext("Edit Tags…"))
            .css_classes(["pill", "small"])
            .build();
        let path = self.track().track().record.path.clone();
        edit_tags.connect_clicked(move |_| {
            GatefoldApplication::get()
                .tools()
                .edit_tags(vec![path.clone()]);
        });
        let add_to_playlist = gtk::Button::builder()
            .label(gettext("Add to Playlist…"))
            .css_classes(["pill", "small"])
            .build();
        let track_id = self.track().id();
        add_to_playlist.connect_clicked(move |button| {
            super::playlist_picker::show(button, vec![track_id]);
        });
        let actions = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .column_spacing(6)
            .row_spacing(6)
            .max_children_per_line(4)
            .build();
        actions.append(&play_next);
        actions.append(&add_to_queue);
        actions.append(&add_to_playlist);
        actions.append(&edit_tags);

        let details = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_start(48)
            .margin_end(12)
            .margin_bottom(12)
            .css_classes(["track-details"])
            .build();
        details.append(&grid);
        details.append(&actions);
        details.upcast()
    }
}
