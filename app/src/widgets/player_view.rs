//! The full player in the expanded bottom sheet (CONCEPT.md §4.3).
//!
//! Layouts by width (`AdwMultiLayoutView`):
//! - wide (> 1000sp): lyrics and queue panes can both sit next to the cover
//! - medium: one pane at a time next to the cover
//! - narrow (≤ 640sp): the pane replaces the cover; a small cover moves
//!   into the header
//!
//! The cover's dominant color themes the view (theme.rs), with the blurred
//! cover as the backdrop.

use std::cell::{Cell, RefCell};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gatefold_core::library::covers::CoverSize;
use gettextrs::gettext;
use gtk::glib;

use super::mini_player::update_play_button;
use super::{CoverImage, LyricsView, QueueView};
use crate::application::GatefoldApplication;
use crate::objects::TrackObject;
use crate::player::Player;
use crate::{covers, format, theme};

/// Below this window width: one pane at a time, replacing the cover.
const NARROW_BELOW: i32 = 640;
/// From this window width, lyrics and queue fit next to each other.
const WIDE_FROM: i32 = 1400;
const MAIN_MIN: i32 = 620;
const MAIN_MAX: i32 = 820;
const PANE_WIDTH: i32 = 420;
/// Room left beside the player, so it reads as a card.
const SIDE_GAP: i32 = 24;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Width {
    #[default]
    Wide,
    Medium,
    Narrow,
}

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/sobakah/Gatefold/ui/player_view.ui")]
    pub struct PlayerView {
        #[template_child]
        pub breakpoint_bin: TemplateChild<adw::BreakpointBin>,
        #[template_child]
        pub overlay: TemplateChild<gtk::Overlay>,
        #[template_child]
        pub backdrop: TemplateChild<gtk::Picture>,
        #[template_child]
        pub toolbar: TemplateChild<adw::ToolbarView>,
        #[template_child]
        pub header_track: TemplateChild<gtk::Box>,
        #[template_child]
        pub header_cover: TemplateChild<CoverImage>,
        #[template_child]
        pub header_title: TemplateChild<gtk::Label>,
        #[template_child]
        pub layout_view: TemplateChild<adw::MultiLayoutView>,
        #[template_child]
        pub narrow_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub cover: TemplateChild<CoverImage>,
        #[template_child]
        pub lyrics_view: TemplateChild<LyricsView>,
        #[template_child]
        pub queue_view: TemplateChild<QueueView>,
        #[template_child]
        pub title: TemplateChild<gtk::Label>,
        #[template_child]
        pub spotify_mark: TemplateChild<gtk::Image>,
        #[template_child]
        pub subtitle: TemplateChild<gtk::Label>,
        #[template_child]
        pub position_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub seek: TemplateChild<gtk::Scale>,
        #[template_child]
        pub seek_adjustment: TemplateChild<gtk::Adjustment>,
        #[template_child]
        pub duration_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub play_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub repeat_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub lyrics_button: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub queue_button: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub info_popover: TemplateChild<gtk::Popover>,
        #[template_child]
        pub shuffle_options: TemplateChild<gtk::MenuButton>,
        #[template_child]
        pub repeat_options: TemplateChild<gtk::MenuButton>,
        #[template_child]
        pub info_button: TemplateChild<gtk::MenuButton>,
        #[template_child]
        pub devices_button: TemplateChild<gtk::MenuButton>,
        #[template_child]
        pub shuffle_button: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub devices_popover: TemplateChild<gtk::Popover>,
        #[template_child]
        pub remote_banner: TemplateChild<adw::Banner>,
        #[template_child]
        pub transport: TemplateChild<gtk::Box>,
        #[template_child]
        pub lyrics_content: TemplateChild<adw::ButtonContent>,
        #[template_child]
        pub queue_content: TemplateChild<adw::ButtonContent>,
        #[template_child]
        pub volume_button: TemplateChild<gtk::MenuButton>,
        #[template_child]
        pub volume_adjustment: TemplateChild<gtk::Adjustment>,
        pub width: Cell<Width>,
        /// The window's width (see set_available_width).
        pub available: Cell<i32>,
        /// Avoids re-entrant toggling while panes are updated.
        pub updating: Cell<bool>,
        /// The lyrics pane was closed for a Spotify song without lyrics and
        /// opens again with the next library song.
        pub lyrics_suspended: Cell<bool>,
        /// A pane opens or closes by itself (not the user's choice, so not
        /// remembered).
        pub pane_auto: Cell<bool>,
        /// What the artist and album links under the title open.
        pub artist_link: Cell<Option<i64>>,
        pub album_link: Cell<Option<i64>>,
        /// The Spotify song shown (not in the library), for its links.
        pub spotify_uri: RefCell<Option<String>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PlayerView {
        const NAME: &'static str = "GatefoldPlayerView";
        type Type = super::PlayerView;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            CoverImage::ensure_type();
            LyricsView::ensure_type();
            QueueView::ensure_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for PlayerView {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup();
        }
    }

    // The player lays out its child itself (no layout manager, see setup)
    // to ask for its target width rather than its content's.
    impl WidgetImpl for PlayerView {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::ConstantSize
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let Some(child) = self.obj().child() else {
                return (0, 0, -1, -1);
            };
            let (min, natural, _, _) = child.measure(orientation, for_size);
            if orientation == gtk::Orientation::Horizontal {
                let target = self.obj().target_width();
                (
                    min,
                    if target > 0 { target.max(min) } else { natural },
                    -1,
                    -1,
                )
            } else {
                (min, natural, -1, -1)
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(child) = self.obj().child() {
                child.allocate(width, height, baseline, None);
            }
        }
    }
    impl BinImpl for PlayerView {}
}

glib::wrapper! {
    pub struct PlayerView(ObjectSubclass<imp::PlayerView>)
        @extends gtk::Widget, adw::Bin,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl PlayerView {
    fn setup(&self) {
        self.set_layout_manager(None::<gtk::LayoutManager>);
        let imp = self.imp();
        let player = GatefoldApplication::get().player();
        // The overlay sizes itself from the controls, not the backdrop.
        imp.overlay.set_measure_overlay(&*imp.toolbar, true);
        // The artist and album under the title open their pages.
        imp.subtitle.connect_activate_link(glib::clone!(
            #[weak(rename_to = view)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, uri| {
                let imp = view.imp();
                let (action, id) = match uri {
                    "artist" => ("win.show-artist", imp.artist_link.get()),
                    "album" => ("win.show-album", imp.album_link.get()),
                    "spotify-artist" | "spotify-album" => {
                        view.open_spotify_link(uri == "spotify-artist");
                        return glib::Propagation::Stop;
                    }
                    _ => return glib::Propagation::Proceed,
                };
                if let Some(id) = id {
                    let _ = view.activate_action(action, Some(&id.to_variant()));
                }
                glib::Propagation::Stop
            }
        ));

        player.connect_current_track_notify(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |player| view.update_track(player)
        ));
        // The Spotify icon before the title (§9).
        let mark = imp.spotify_mark.get();
        let show_mark = move |player: &Player| mark.set_visible(player.plays_from_spotify());
        show_mark(&player);
        let on_track = show_mark.clone();
        player.connect_current_track_notify(move |player| on_track(player));
        player.connect_remote_device_notify(move |player| show_mark(player));
        player.connect_playing_notify(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |player| update_play_button(&view.imp().play_button, player.playing())
        ));
        player.connect_position_notify(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |player| view.update_position(player)
        ));
        player.connect_duration_notify(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |player| view.update_position(player)
        ));
        player.connect_repeat_notify(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |player| view.update_repeat(player)
        ));
        // A rescan can bring new covers and colors.
        GatefoldApplication::get().library().connect_local(
            "reloaded",
            false,
            glib::clone!(
                #[weak(rename_to = view)]
                self,
                #[upgrade_or]
                None,
                move |_| {
                    view.update_track(&GatefoldApplication::get().player());
                    None
                }
            ),
        );
        adw::StyleManager::default().connect_dark_notify(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_| view.update_theme(GatefoldApplication::get().player().current_track())
        ));

        imp.seek.connect_change_value(glib::clone!(
            #[weak]
            player,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, _, value| {
                player.seek(value.max(0.0) as u64);
                glib::Propagation::Proceed
            }
        ));

        super::mini_player::make_end_time_toggle(
            &imp.duration_label,
            glib::clone!(
                #[weak(rename_to = view)]
                self,
                move || view.update_position(&GatefoldApplication::get().player())
            ),
        );

        super::mini_player::show_mixing(&imp.shuffle_button);
        imp.shuffle_options
            .set_popover(Some(&super::shuffle_options::shuffle_popover()));
        imp.repeat_options
            .set_popover(Some(&super::shuffle_options::repeat_popover()));

        super::device_popover::setup(
            &imp.devices_button,
            &imp.devices_popover,
            &imp.remote_banner,
        );

        // Song Info: built for the current song whenever it opens.
        imp.info_popover.connect_show(|popover| {
            let player = GatefoldApplication::get().player();
            let child: gtk::Widget = match player.current_track() {
                Some(track) => super::track_info::grid(&track, true).upcast(),
                None => gtk::Label::new(Some(&gettext("Not Playing"))).upcast(),
            };
            child.set_margin_top(6);
            child.set_margin_bottom(6);
            child.set_margin_start(6);
            child.set_margin_end(6);
            popover.set_child(Some(&child));
        });

        // Volume, shared with MPRIS and the saved setting.
        imp.volume_adjustment.set_value(player.volume());
        self.update_volume_icon(player.volume());
        imp.volume_adjustment.connect_value_changed(glib::clone!(
            #[weak(rename_to = view)]
            self,
            #[weak]
            player,
            move |adjustment| {
                let volume = adjustment.value();
                if (player.volume() - volume).abs() > f64::EPSILON {
                    player.set_volume(volume);
                }
                view.update_volume_icon(volume);
            }
        ));
        player.settings().connect_changed(
            Some("volume"),
            glib::clone!(
                #[weak(rename_to = view)]
                self,
                move |settings, key| {
                    let volume = settings.double(key);
                    let adjustment = &view.imp().volume_adjustment;
                    if (adjustment.value() - volume).abs() > f64::EPSILON {
                        adjustment.set_value(volume);
                    }
                }
            ),
        );

        // The panes as chosen in Preferences, or as they were left.
        let settings = player.settings();
        let open_at_start = |mode: &str, was: &str| match settings.string(mode).as_str() {
            "open" => true,
            "closed" => false,
            _ => settings.boolean(was),
        };
        imp.lyrics_button
            .set_active(open_at_start("lyrics-pane", "lyrics-pane-open"));
        imp.queue_button
            .set_active(open_at_start("queue-pane", "queue-pane-open"));
        for button in [&*imp.lyrics_button, &*imp.queue_button] {
            button.connect_toggled(glib::clone!(
                #[weak(rename_to = view)]
                self,
                move |button| view.pane_toggled(button)
            ));
        }

        self.update_track(&player);
        self.update_repeat(&player);
        self.update_panes();
        update_play_button(&imp.play_button, player.playing());
    }

    /// The window's width, from which the player picks its layout and its
    /// own width (it isn't full width: the main column stays in the
    /// middle, side panes widen it). Deciding from the window, not from
    /// the player's own width, keeps opening panes from changing the
    /// layout class.
    pub fn set_available_width(&self, available: i32) {
        let imp = self.imp();
        if imp.available.replace(available) == available {
            return;
        }
        let width = if available < NARROW_BELOW {
            Width::Narrow
        } else if available < WIDE_FROM {
            Width::Medium
        } else {
            Width::Wide
        };
        if width != imp.width.get() {
            self.set_width(width);
        } else {
            self.queue_resize();
        }
    }

    /// The width the player asks for: the main column plus open panes.
    fn target_width(&self) -> i32 {
        let imp = self.imp();
        let available = imp.available.get();
        if available <= 0 || imp.width.get() == Width::Narrow {
            return available.max(0);
        }
        let main = (available * 6 / 10).clamp(MAIN_MIN, MAIN_MAX);
        let panes = [&*imp.lyrics_button, &*imp.queue_button]
            .iter()
            .filter(|b| b.is_active())
            .count() as i32;
        (main + panes * PANE_WIDTH).min(available - SIDE_GAP)
    }

    fn set_width(&self, width: Width) {
        let imp = self.imp();
        imp.width.set(width);
        // Narrow: icon-only pane buttons and a tighter transport.
        let narrow = width == Width::Narrow;
        imp.lyrics_content.set_label(&if narrow {
            String::new()
        } else {
            gettext("Lyrics")
        });
        imp.queue_content.set_label(&if narrow {
            String::new()
        } else {
            gettext("Queue")
        });
        imp.transport.set_spacing(if narrow { 6 } else { 18 });
        self.update_panes();
    }

    /// Shows or hides the lyrics pane (Ctrl+L).
    pub fn toggle_lyrics(&self) {
        let button = &self.imp().lyrics_button;
        if !button.is_sensitive() {
            return;
        }
        button.set_active(!button.is_active());
    }

    /// Shows or hides the queue pane (Ctrl+U).
    pub fn toggle_queue(&self) {
        let button = &self.imp().queue_button;
        button.set_active(!button.is_active());
    }

    fn pane_toggled(&self, button: &gtk::ToggleButton) {
        let imp = self.imp();
        if imp.updating.get() {
            return;
        }
        // Without room for both, turning one pane on turns the other off.
        if button.is_active() && imp.width.get() != Width::Wide {
            let other = if button == &*imp.lyrics_button {
                &imp.queue_button
            } else {
                &imp.lyrics_button
            };
            imp.updating.set(true);
            other.set_active(false);
            imp.updating.set(false);
        }
        if !imp.pane_auto.get() {
            let player = GatefoldApplication::get().player();
            let settings = player.settings();
            // Closed only for a song without lyrics: still open.
            let lyrics = imp.lyrics_button.is_active() || imp.lyrics_suspended.get();
            let _ = settings.set_boolean("lyrics-pane-open", lyrics);
            let _ = settings.set_boolean("queue-pane-open", imp.queue_button.is_active());
        }
        self.update_panes();
    }

    fn update_panes(&self) {
        let imp = self.imp();
        let (lyrics, queue) = (imp.lyrics_button.is_active(), imp.queue_button.is_active());
        if imp.width.get() == Width::Medium && lyrics && queue {
            // Narrowed from wide with both open: keep the lyrics.
            imp.updating.set(true);
            imp.queue_button.set_active(false);
            imp.updating.set(false);
            return self.update_panes();
        }
        let narrow = imp.width.get() == Width::Narrow;
        imp.layout_view
            .set_layout_name(if narrow { "narrow" } else { "wide" });
        if narrow {
            imp.lyrics_view.set_visible(true);
            imp.queue_view.set_visible(true);
            let page = if queue {
                "queue"
            } else if lyrics {
                "lyrics"
            } else {
                "cover"
            };
            imp.narrow_stack.set_visible_child_name(page);
            imp.header_track.set_visible(page != "cover");
        } else {
            imp.lyrics_view.set_visible(lyrics);
            imp.queue_view.set_visible(queue);
            imp.header_track.set_visible(false);
        }
        self.queue_resize();
    }

    fn update_volume_icon(&self, volume: f64) {
        let icon = if volume <= 0.0 {
            "audio-volume-muted-symbolic"
        } else if volume < 0.34 {
            "audio-volume-low-symbolic"
        } else if volume < 0.67 {
            "audio-volume-medium-symbolic"
        } else {
            "audio-volume-high-symbolic"
        };
        let button = &self.imp().volume_button;
        button.set_icon_name(icon);
        // Translators: {} is the volume in percent.
        button.set_tooltip_text(Some(
            &gettext("Volume: {} %").replace("{}", &format!("{:.0}", volume * 100.0)),
        ));
    }

    /// "Artist — Album" under the title; both open their page: the
    /// library's for library songs, else Spotify's (looked up when
    /// clicked), or the library artist's page when the artist is there.
    fn set_subtitle(&self, track: &TrackObject) {
        let imp = self.imp();
        let album_id = track.track().album_id;
        let artist_id = album_id.and_then(|id| {
            GatefoldApplication::get()
                .library()
                .album_artists(id)
                .first()
                .map(|(id, _)| *id)
        });
        imp.album_link.set(album_id);
        imp.artist_link.set(artist_id);
        let spotify_uri = GatefoldApplication::get()
            .spotify()
            .stand_in_uri(track.id())
            .filter(|uri| uri.starts_with("spotify:track:"));
        let from_spotify = spotify_uri.is_some();
        imp.spotify_uri.replace(spotify_uri);
        let link = |text: &str, uri: &str, linked: bool, tooltip: &str| {
            let text = glib::markup_escape_text(text);
            if linked {
                format!(
                    "<a href=\"{uri}\" title=\"{}\">{text}</a>",
                    glib::markup_escape_text(tooltip)
                )
            } else {
                text.to_string()
            }
        };
        let artist = track.artist().map(|artist| {
            let (uri, linked) = match artist_id {
                Some(_) => ("artist", true),
                None => ("spotify-artist", from_spotify),
            };
            // Translators: tooltip of the artist link in the player.
            link(&artist, uri, linked, &gettext("Go to Artist"))
        });
        let album = track.track().album_title.as_deref().map(|album| {
            let (uri, linked) = match album_id {
                Some(_) => ("album", true),
                None => ("spotify-album", from_spotify),
            };
            // Translators: tooltip of the album link in the player.
            link(album, uri, linked, &gettext("Go to Album"))
        });
        let subtitle = match (artist, album) {
            (Some(artist), Some(album)) => format!("{artist} — {album}"),
            (Some(text), None) | (None, Some(text)) => text,
            (None, None) => String::new(),
        };
        imp.subtitle.set_markup(&subtitle);
    }

    /// The artist or album of a Spotify song was clicked: its page in the
    /// library if the library has it, else on Spotify.
    fn open_spotify_link(&self, artist: bool) {
        if let Some(uri) = self.imp().spotify_uri.borrow().clone() {
            super::open_song_page(uri, artist);
        }
    }

    fn update_track(&self, player: &Player) {
        let imp = self.imp();
        let track = player.current_track();
        let key = track.as_ref().and_then(|t| t.cover_key());
        imp.cover.set_cover(key);
        imp.header_cover.set_cover(key);
        let backdrop = key.and_then(|k| covers::texture(k, CoverSize::Grid));
        imp.backdrop.set_paintable(backdrop.as_ref());
        match &track {
            Some(track) => {
                imp.title.set_label(&track.title());
                imp.header_title.set_label(&track.title());
                self.set_subtitle(track);
                // Translators: accessible label of the cover; {} is a song title.
                imp.cover
                    .set_label(&gettext("Cover of {}").replace("{}", &track.title()));
            }
            None => {
                imp.title.set_label(&gettext("Not Playing"));
                imp.album_link.set(None);
                imp.artist_link.set(None);
                imp.subtitle.set_text(&gettext("Pick an album to start"));
            }
        }
        imp.seek.set_sensitive(track.is_some());
        // Spotify songs not in the library have lyrics only from LRCLIB
        // (when it's on in Preferences).
        let stand_in =
            track.as_ref().is_some_and(TrackObject::is_stand_in) && !crate::lrclib::enabled();
        imp.lyrics_button.set_sensitive(!stand_in);
        imp.lyrics_button.set_tooltip_text(Some(&if stand_in {
            gettext("No lyrics for songs that aren't in your library")
        } else {
            gettext("Lyrics (Ctrl+L)")
        }));
        imp.pane_auto.set(true);
        if stand_in && imp.lyrics_button.is_active() {
            imp.lyrics_suspended.set(true);
            imp.lyrics_button.set_active(false);
        } else if !stand_in && imp.lyrics_suspended.replace(false) {
            imp.lyrics_button.set_active(true);
        }
        imp.pane_auto.set(false);
        self.update_theme(track);
        self.update_position(player);
    }

    fn update_theme(&self, track: Option<TrackObject>) {
        let color = track
            .as_ref()
            .and_then(|t| t.cover_key())
            .and_then(covers::color);
        theme::apply(color, adw::StyleManager::default().is_dark());
        if color.is_some() {
            self.add_css_class("themed");
        } else {
            self.remove_css_class("themed");
        }
    }

    fn update_position(&self, player: &Player) {
        let imp = self.imp();
        let duration = player.duration();
        imp.seek_adjustment.set_upper(duration.max(1) as f64);
        imp.seek_adjustment.set_value(player.position() as f64);
        imp.position_label
            .set_label(&format::duration(player.position()));
        imp.duration_label.set_label(&format::end_time(
            player.position(),
            duration,
            super::mini_player::show_remaining(),
        ));
    }

    fn update_repeat(&self, player: &Player) {
        super::mini_player::update_repeat_button(&self.imp().repeat_button, player);
    }
}
