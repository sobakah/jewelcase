//! The bottom bar of the library window (CONCEPT.md §4.1): what's playing,
//! transport buttons and a thin progress bar.

use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use gtk::prelude::*;

use super::CoverImage;
use crate::application::GatefoldApplication;
use crate::player::Player;

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/sobakah/Gatefold/ui/mini_player.ui")]
    pub struct MiniPlayer {
        #[template_child]
        pub progress: TemplateChild<gtk::ProgressBar>,
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
            obj.update_track(&player);
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
        let duration = player.duration();
        let fraction = if duration > 0 {
            player.position() as f64 / duration as f64
        } else {
            0.0
        };
        self.imp().progress.set_fraction(fraction.clamp(0.0, 1.0));
    }
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
