//! A song row in the album panel. The chevron expands details in place
//! (CONCEPT.md §4.1); activating the row (double-click, Enter, or the play
//! button that replaces the number on hover) plays it.

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
        pub lead: gtk::Stack,
        pub playing: std::cell::Cell<bool>,
        pub hovered: std::cell::Cell<bool>,
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
        imp.playing_icon.set_tooltip_text(Some(&gettext("Playing")));
        let play = gtk::Button::builder()
            .icon_name("media-playback-start-symbolic")
            .tooltip_text(gettext("Play"))
            .valign(gtk::Align::Center)
            .halign(gtk::Align::End)
            .css_classes(["flat", "circular", "row-play"])
            .build();
        play.connect_clicked(glib::clone!(
            #[weak]
            row,
            move |_| {
                row.activate();
            }
        ));
        imp.lead
            .set_transition_type(gtk::StackTransitionType::Crossfade);
        imp.lead.set_transition_duration(100);
        imp.lead.set_width_request(34);
        imp.lead.add_named(&imp.number, Some("number"));
        imp.lead.add_named(&imp.playing_icon, Some("playing"));
        imp.lead.add_named(&play, Some("play"));
        // The play button shows while the pointer is over the row.
        let motion = gtk::EventControllerMotion::new();
        motion.connect_enter(glib::clone!(
            #[weak]
            row,
            move |_, _, _| row.set_hovered(true)
        ));
        motion.connect_leave(glib::clone!(
            #[weak]
            row,
            move |_| row.set_hovered(false)
        ));
        row.add_controller(motion);
        row.update_lead();

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

        let add_to_queue = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text(gettext("Add to Queue"))
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular", "row-action"])
            .build();
        add_to_queue.connect_clicked(glib::clone!(
            #[strong]
            track,
            move |_| {
                GatefoldApplication::get()
                    .player()
                    .add_to_queue(vec![track.clone()]);
            }
        ));
        let add_to_playlist = gtk::Button::builder()
            .icon_name("view-list-bullet-symbolic")
            .tooltip_text(gettext("Add to Playlist…"))
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular", "row-action"])
            .build();
        let track_id = track.id();
        add_to_playlist.connect_clicked(move |button| {
            super::playlist_picker::show(button, vec![track_id]);
        });

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
            .spacing(6)
            .margin_start(6)
            .margin_end(6)
            .margin_top(6)
            .margin_bottom(6)
            .build();
        line.append(&imp.lead);
        line.append(&text);
        line.append(&duration);
        line.append(&add_to_queue);
        line.append(&add_to_playlist);
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
        self.imp().playing.set(playing);
        if playing {
            self.add_css_class("playing");
        } else {
            self.remove_css_class("playing");
        }
        self.update_lead();
    }

    fn set_hovered(&self, hovered: bool) {
        self.imp().hovered.set(hovered);
        self.update_lead();
    }

    /// Number, "playing" icon, or (under the pointer) the play button.
    fn update_lead(&self) {
        let imp = self.imp();
        let page = if imp.hovered.get() {
            "play"
        } else if imp.playing.get() {
            "playing"
        } else {
            "number"
        };
        imp.lead.set_visible_child_name(page);
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
        let grid = super::track_info::grid(track, false);

        let play_next = gtk::Button::builder()
            .label(gettext("Play Next"))
            .css_classes(["pill", "small"])
            .build();
        let track = track.clone();
        play_next.connect_clicked(move |_| {
            GatefoldApplication::get()
                .player()
                .play_next(vec![track.clone()]);
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
        let actions = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .column_spacing(6)
            .row_spacing(6)
            .max_children_per_line(4)
            .build();
        actions.append(&play_next);
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
