//! The bottom bar of the library window (CONCEPT.md §4.1): what's playing,
//! transport, shuffle and repeat, a seek bar, and the queue as a card.

use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use gtk::prelude::*;

use super::{CoverImage, QueueView};
use crate::application::GatefoldApplication;
use crate::format;
use crate::player::Player;

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate, glib::Properties)]
    #[template(resource = "/io/github/sobakah/Gatefold/ui/mini_player.ui")]
    #[properties(wrapper_type = super::MiniPlayer)]
    pub struct MiniPlayer {
        /// On narrow windows: only the essentials (no shuffle, repeat or
        /// queue card; the full player has them).
        #[property(get, set)]
        pub compact: std::cell::Cell<bool>,
        #[template_child]
        pub seek: TemplateChild<gtk::Scale>,
        #[template_child]
        pub seek_adjustment: TemplateChild<gtk::Adjustment>,
        #[template_child]
        pub position_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub duration_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub repeat_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub queue_popover: TemplateChild<gtk::Popover>,
        #[template_child]
        pub cover: TemplateChild<CoverImage>,
        #[template_child]
        pub title: TemplateChild<gtk::Label>,
        #[template_child]
        pub artist: TemplateChild<gtk::Label>,
        #[template_child]
        pub play_button: TemplateChild<gtk::Button>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MiniPlayer {
        const NAME: &'static str = "GatefoldMiniPlayer";
        type Type = super::MiniPlayer;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            CoverImage::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for MiniPlayer {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            let player = GatefoldApplication::get().player();
            player.connect_current_track_notify(glib::clone!(
                #[weak]
                obj,
                move |player| obj.update_track(player)
            ));
            player.connect_playing_notify(glib::clone!(
                #[weak]
                obj,
                move |player| super::update_play_button(&obj.imp().play_button, player.playing())
            ));
            player.connect_position_notify(glib::clone!(
                #[weak]
                obj,
                move |player| obj.update_progress(player)
            ));
            player.connect_repeat_notify(glib::clone!(
                #[weak]
                obj,
                move |player| update_repeat_button(&obj.imp().repeat_button, player)
            ));
            self.seek.connect_change_value(glib::clone!(
                #[weak]
                player,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |_, _, value| {
                    player.seek(value.max(0.0) as u64);
                    glib::Propagation::Proceed
                }
            ));
            // The queue card is built when first opened.
            self.queue_popover.connect_show(|popover| {
                if popover.child().is_none() {
                    let queue = QueueView::default();
                    queue.set_size_request(380, 460);
                    popover.set_child(Some(&queue));
                }
            });
            obj.update_track(&player);
            update_repeat_button(&self.repeat_button, &player);
            super::update_play_button(&self.play_button, player.playing());
        }
    }

    impl WidgetImpl for MiniPlayer {}
    impl BinImpl for MiniPlayer {}
}

glib::wrapper! {
    pub struct MiniPlayer(ObjectSubclass<imp::MiniPlayer>)
        @extends gtk::Widget, adw::Bin,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl MiniPlayer {
    /// Opens the queue card.
    pub fn show_queue(&self) {
        self.imp().queue_popover.popup();
    }

    fn update_track(&self, player: &Player) {
        let imp = self.imp();
        let track = player.current_track();
        imp.cover
            .set_cover(track.as_ref().and_then(|t| t.cover_key()));
        match &track {
            Some(track) => {
                imp.title.set_label(&track.title());
                let artist = track.artist().unwrap_or_default();
                imp.artist.set_visible(!artist.is_empty());
                imp.artist.set_label(&artist);
            }
            None => {
                imp.title.set_label(&gettext("Not Playing"));
                imp.artist.set_visible(false);
            }
        }
        self.update_progress(player);
    }

    fn update_progress(&self, player: &Player) {
        let imp = self.imp();
        let duration = player.duration();
        imp.seek_adjustment.set_upper(duration.max(1) as f64);
        imp.seek_adjustment.set_value(player.position() as f64);
        imp.seek.set_sensitive(player.current_track().is_some());
        imp.position_label
            .set_label(&format::duration(player.position()));
        imp.duration_label.set_label(&format::duration(duration));
    }
}

/// Repeat's icon and tooltip for its current mode.
pub(super) fn update_repeat_button(button: &gtk::Button, player: &Player) {
    let (icon, tooltip) = match player.repeat().as_str() {
        "all" => ("media-playlist-repeat-symbolic", gettext("Repeat: All")),
        "one" => (
            "media-playlist-repeat-song-symbolic",
            gettext("Repeat: One Song"),
        ),
        _ => (
            "media-playlist-consecutive-symbolic",
            gettext("Repeat: Off"),
        ),
    };
    button.set_icon_name(icon);
    button.set_tooltip_text(Some(&tooltip));
}

pub(super) fn update_play_button(button: &gtk::Button, playing: bool) {
    if playing {
        button.set_icon_name("media-playback-pause-symbolic");
        button.set_tooltip_text(Some(&gettext("Pause")));
    } else {
        button.set_icon_name("media-playback-start-symbolic");
        button.set_tooltip_text(Some(&gettext("Play")));
    }
}
