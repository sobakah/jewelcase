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

use std::cell::Cell;

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
        pub width: Cell<Width>,
        /// Avoids re-entrant toggling while panes are updated.
        pub updating: Cell<bool>,
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

    impl WidgetImpl for PlayerView {}
    impl BinImpl for PlayerView {}
}

glib::wrapper! {
    pub struct PlayerView(ObjectSubclass<imp::PlayerView>)
        @extends gtk::Widget, adw::Bin,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl PlayerView {
    fn setup(&self) {
        let imp = self.imp();
        let player = GatefoldApplication::get().player();
        // The overlay sizes itself from the controls, not the backdrop.
        imp.overlay.set_measure_overlay(&*imp.toolbar, true);
        self.setup_breakpoints();

        player.connect_current_track_notify(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |player| view.update_track(player)
        ));
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

    fn setup_breakpoints(&self) {
        let imp = self.imp();
        // Only the last matching breakpoint applies: medium, then narrow.
        for (condition, width) in [
            ("max-width: 1000sp", Width::Medium),
            ("max-width: 640sp", Width::Narrow),
        ] {
            let breakpoint = adw::Breakpoint::new(
                adw::BreakpointCondition::parse(condition).expect("valid breakpoint"),
            );
            breakpoint.connect_apply(glib::clone!(
                #[weak(rename_to = view)]
                self,
                move |_| view.set_width(width)
            ));
            breakpoint.connect_unapply(glib::clone!(
                #[weak(rename_to = view)]
                self,
                move |_| {
                    // Back to the next wider class.
                    let wider = match width {
                        Width::Narrow => Width::Medium,
                        _ => Width::Wide,
                    };
                    if view.imp().width.get() == width {
                        view.set_width(wider);
                    }
                }
            ));
            imp.breakpoint_bin.add_breakpoint(breakpoint);
        }
    }

    fn set_width(&self, width: Width) {
        self.imp().width.set(width);
        self.update_panes();
    }

    /// Shows or hides the lyrics pane (Ctrl+L).
    pub fn toggle_lyrics(&self) {
        let button = &self.imp().lyrics_button;
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
                let artist = track.artist();
                let album = track.track().album_title.clone();
                let subtitle = match (artist, album) {
                    (Some(artist), Some(album)) => format!("{artist} — {album}"),
                    (Some(text), None) | (None, Some(text)) => text,
                    (None, None) => String::new(),
                };
                imp.subtitle.set_label(&subtitle);
                // Translators: accessible label of the cover; {} is a song title.
                imp.cover
                    .set_label(&gettext("Cover of {}").replace("{}", &track.title()));
            }
            None => {
                imp.title.set_label(&gettext("Not Playing"));
                imp.subtitle.set_label(&gettext("Pick an album to start"));
            }
        }
        imp.seek.set_sensitive(track.is_some());
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
        imp.duration_label.set_label(&format::duration(duration));
    }

    fn update_repeat(&self, player: &Player) {
        super::mini_player::update_repeat_button(&self.imp().repeat_button, player);
    }
}
