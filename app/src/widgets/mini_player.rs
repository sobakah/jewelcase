//! The bottom bar of the library window (CONCEPT.md §4.1): what's playing,
//! transport, shuffle and repeat, a seek bar, and the queue as a card.

use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use gtk::prelude::*;

use super::{CoverImage, QueueView};
use crate::application::JewelcaseApplication;
use crate::format;
use crate::player::Player;

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate, glib::Properties)]
    #[template(resource = "/io/github/sobakah/Jewelcase/ui/mini_player.ui")]
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
        pub shuffle_button: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub queue_popover: TemplateChild<gtk::Popover>,
        #[template_child]
        pub cover: TemplateChild<CoverImage>,
        #[template_child]
        pub text_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub title: TemplateChild<gtk::Label>,
        #[template_child]
        pub spotify_mark: TemplateChild<gtk::Image>,
        #[template_child]
        pub artist: TemplateChild<gtk::Label>,
        #[template_child]
        pub play_button: TemplateChild<gtk::Button>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MiniPlayer {
        const NAME: &'static str = "JewelcaseMiniPlayer";
        type Type = super::MiniPlayer;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            CoverImage::ensure_type();
            crate::widgets::Visualizer::ensure_type();
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
            let player = JewelcaseApplication::get().player();
            player.connect_current_track_notify(glib::clone!(
                #[weak]
                obj,
                move |player| obj.update_track(player)
            ));
            // The Spotify icon before the title (§9).
            let mark = self.spotify_mark.get();
            let show_mark = move |player: &Player| mark.set_visible(player.plays_from_spotify());
            show_mark(&player);
            let on_track = show_mark.clone();
            player.connect_current_track_notify(move |player| on_track(player));
            player.connect_remote_device_notify(move |player| show_mark(player));
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
            // A length learned while paused (a file that doesn't say).
            player.connect_duration_notify(glib::clone!(
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
            make_end_time_toggle(
                &self.duration_label,
                glib::clone!(
                    #[weak]
                    obj,
                    move || obj.update_progress(&JewelcaseApplication::get().player())
                ),
            );
            // Compact (narrow windows): a smaller cover, a title that may
            // shrink.
            obj.connect_compact_notify(|obj| {
                let imp = obj.imp();
                let compact = obj.compact();
                let size = if compact { 52 } else { 68 };
                imp.cover.set_min_size(size);
                imp.cover.set_natural_size(size);
                imp.text_box
                    .set_width_request(if compact { 60 } else { 180 });
            });
            obj.update_track(&player);
            update_repeat_button(&self.repeat_button, &player);
            show_shuffle(&self.shuffle_button);
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
        // No length known (yet): an empty bar that can't be dragged.
        let known = duration > 0;
        imp.seek_adjustment
            .set_value(if known { player.position() as f64 } else { 0.0 });
        imp.seek
            .set_sensitive(known && player.current_track().is_some());
        imp.position_label
            .set_label(&format::duration(player.position()));
        imp.duration_label.set_label(&format::end_time(
            player.position(),
            duration,
            show_remaining(),
        ));
    }
}

/// Clicking the time right of the seek bar (or Enter or Space on it)
/// switches between the song's length and the remaining time (a shared
/// setting).
pub(super) fn make_end_time_toggle(label: &gtk::Label, on_change: impl Fn() + 'static) {
    let player = JewelcaseApplication::get().player();
    let settings = player.settings().clone();
    label.set_tooltip_text(Some(&gettext("Show Remaining Time")));
    label.set_cursor_from_name(Some("pointer"));
    let click = gtk::GestureClick::new();
    let toggle_settings = settings.clone();
    click.connect_released(move |_, _, _, _| {
        let remaining = toggle_settings.boolean("show-remaining-time");
        let _ = toggle_settings.set_boolean("show-remaining-time", !remaining);
    });
    label.add_controller(click);
    // The keyboard too: focusable, Enter or Space.
    label.set_focusable(true);
    label.update_property(&[gtk::accessible::Property::Label(&gettext(
        "Show Remaining Time",
    ))]);
    let keys = gtk::EventControllerKey::new();
    let key_settings = settings.clone();
    keys.connect_key_pressed(move |_, key, _, _| {
        use gtk::gdk::Key;
        if matches!(
            key,
            Key::Return | Key::KP_Enter | Key::space | Key::KP_Space
        ) {
            let remaining = key_settings.boolean("show-remaining-time");
            let _ = key_settings.set_boolean("show-remaining-time", !remaining);
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    label.add_controller(keys);
    settings.connect_changed(Some("show-remaining-time"), move |_, _| on_change());
}

pub(super) fn show_remaining() -> bool {
    JewelcaseApplication::get()
        .player()
        .settings()
        .boolean("show-remaining-time")
}

/// Repeat's icon and tooltip for its current mode. The tooltip names the
/// mode: the three icons alone don't say which is which.
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
    super::set_accessible_label(button, &tooltip);
}

/// Shuffle's icon and tooltip, kept up to date. The tooltip names the
/// mode (off, songs, albums), and smart shuffle (mixing in similar songs,
/// §7) shows on the button: its own icon, highlighted even while plain
/// shuffle is off.
pub(super) fn show_shuffle(button: &gtk::ToggleButton) {
    let player = JewelcaseApplication::get().player();
    let settings = player.settings().clone();
    let update = glib::clone!(
        #[weak]
        button,
        move || update_shuffle_button(&button)
    );
    update();
    player.connect_shuffle_notify({
        let update = update.clone();
        move |_| update()
    });
    for key in ["shuffle-albums", "mix-similar"] {
        let update = update.clone();
        settings.connect_changed(Some(key), move |_, _| update());
    }
}

fn update_shuffle_button(button: &gtk::ToggleButton) {
    let player = JewelcaseApplication::get().player();
    let settings = player.settings();
    let mixing = settings.boolean("mix-similar");
    let albums = settings.boolean("shuffle-albums");
    let mode = shuffle_mode_text(player.shuffle(), albums, mixing, player.is_remote());
    if mixing {
        button.set_icon_name("jewelcase-smart-shuffle-symbolic");
        button.add_css_class("smart-shuffle");
    } else {
        button.set_icon_name("media-playlist-shuffle-symbolic");
        button.remove_css_class("smart-shuffle");
    }
    button.set_tooltip_text(Some(&mode.join("\n")));
    super::set_accessible_label(button, &mode.join(", "));
}

/// The lines of shuffle's tooltip. A Spotify device shuffles its own way:
/// only on or off, and Jewelcase mixes nothing in there.
fn shuffle_mode_text(on: bool, albums: bool, mixing: bool, remote: bool) -> Vec<String> {
    let mode = match (on, albums) {
        (false, _) => gettext("Shuffle: Off"),
        _ if remote => gettext("Shuffle: On"),
        (true, false) => gettext("Shuffle: Songs"),
        (true, true) => gettext("Shuffle: Albums"),
    };
    let mut lines = vec![mode];
    if mixing && !remote {
        lines.push(gettext("Mixing in similar songs"));
    }
    lines
}

pub(super) fn update_play_button(button: &gtk::Button, playing: bool) {
    if playing {
        button.set_icon_name("media-playback-pause-symbolic");
        super::set_accessible_label(button, &gettext("Pause"));
    } else {
        button.set_icon_name("media-playback-start-symbolic");
        super::set_accessible_label(button, &gettext("Play"));
    }
}

#[cfg(test)]
mod tests {
    use super::shuffle_mode_text;

    #[test]
    fn shuffle_tooltip_names_the_mode() {
        assert_eq!(
            shuffle_mode_text(false, false, false, false),
            ["Shuffle: Off"]
        );
        assert_eq!(
            shuffle_mode_text(false, true, false, false),
            ["Shuffle: Off"]
        );
        assert_eq!(
            shuffle_mode_text(true, false, false, false),
            ["Shuffle: Songs"]
        );
        assert_eq!(
            shuffle_mode_text(true, true, false, false),
            ["Shuffle: Albums"]
        );
        // Mixing works with shuffle off too: a second line.
        assert_eq!(
            shuffle_mode_text(false, false, true, false),
            ["Shuffle: Off", "Mixing in similar songs"]
        );
        // A Spotify device: on or off, and nothing mixed in.
        assert_eq!(shuffle_mode_text(true, true, true, true), ["Shuffle: On"]);
    }
}
