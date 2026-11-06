//! The lyrics pane of the player (CONCEPT.md §5).
//!
//! Lines are centered; the active line is highlighted and scrolled into
//! the middle with a spring animation. Clicking a line seeks to it. After a
//! manual scroll, auto-scroll pauses for 3 seconds.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gatefold_lyrics::{Line, LyricsState};
use gettextrs::gettext;
use gtk::{gio, glib};

use crate::application::GatefoldApplication;
use crate::config::APP_ID;
use crate::objects::TrackObject;

/// Auto-scroll waits this long after the user scrolled.
const SCROLL_PAUSE: Duration = Duration::from_secs(3);

mod imp {
    use super::*;

    #[derive(Debug)]
    pub struct LyricsView {
        pub stack: gtk::Stack,
        pub scroller: gtk::ScrolledWindow,
        pub lines_box: gtk::Box,
        pub status: adw::StatusPage,
        pub rows: RefCell<Vec<(Option<u64>, gtk::Button)>>,
        pub active: Cell<Option<usize>>,
        pub synced: Cell<bool>,
        /// Which track the shown lyrics belong to.
        pub track_id: Cell<Option<i64>>,
        pub paused_until: Cell<Option<Instant>>,
        /// True while our own animation moves the scrollbar.
        pub auto_scrolling: Cell<bool>,
        pub animation: RefCell<Option<adw::SpringAnimation>>,
        pub settings: gio::Settings,
        pub lines: RefCell<Vec<Line>>,
    }

    impl Default for LyricsView {
        fn default() -> Self {
            Self {
                stack: gtk::Stack::builder()
                    .transition_type(gtk::StackTransitionType::Crossfade)
                    .build(),
                scroller: gtk::ScrolledWindow::builder()
                    .hscrollbar_policy(gtk::PolicyType::Never)
                    .vexpand(true)
                    .build(),
                lines_box: gtk::Box::builder()
                    .orientation(gtk::Orientation::Vertical)
                    .spacing(6)
                    .margin_top(24)
                    .margin_bottom(24)
                    .build(),
                status: adw::StatusPage::builder()
                    .icon_name("gatefold-lyrics-symbolic")
                    .css_classes(["compact"])
                    .build(),
                rows: RefCell::default(),
                active: Cell::default(),
                synced: Cell::default(),
                track_id: Cell::default(),
                paused_until: Cell::default(),
                auto_scrolling: Cell::default(),
                animation: RefCell::default(),
                settings: gio::Settings::new(APP_ID),
                lines: RefCell::default(),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LyricsView {
        const NAME: &'static str = "GatefoldLyricsView";
        type Type = super::LyricsView;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("lyrics-view");
        }
    }

    impl ObjectImpl for LyricsView {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup();
        }
    }

    impl WidgetImpl for LyricsView {}
    impl BinImpl for LyricsView {}
}

glib::wrapper! {
    pub struct LyricsView(ObjectSubclass<imp::LyricsView>)
        @extends gtk::Widget, adw::Bin,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl LyricsView {
    fn setup(&self) {
        let imp = self.imp();
        let clamp = adw::Clamp::builder()
            .maximum_size(640)
            .child(&imp.lines_box)
            .build();
        imp.scroller.set_child(Some(&clamp));
        imp.stack.add_named(&imp.scroller, Some("lyrics"));
        imp.stack.add_named(&imp.status, Some("status"));
        self.set_child(Some(&imp.stack));
        self.add_css_class("lyrics-view");

        // A scroll that isn't ours pauses auto-scroll.
        imp.scroller
            .vadjustment()
            .connect_value_changed(glib::clone!(
                #[weak(rename_to = view)]
                self,
                move |_| {
                    if !view.imp().auto_scrolling.get() {
                        view.imp()
                            .paused_until
                            .set(Some(Instant::now() + SCROLL_PAUSE));
                    }
                }
            ));

        let player = GatefoldApplication::get().player();
        player.connect_current_track_notify(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |player| view.set_track(player.current_track())
        ));
        player.connect_position_notify(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |player| view.set_position(player.position())
        ));
        imp.settings.connect_changed(
            Some("lyrics-layout"),
            glib::clone!(
                #[weak(rename_to = view)]
                self,
                move |_, _| view.rebuild()
            ),
        );
        self.set_track(player.current_track());
    }

    fn show_status(&self, icon: &str, title: &str, description: Option<&str>) {
        let imp = self.imp();
        imp.status.set_icon_name(Some(icon));
        imp.status.set_title(title);
        imp.status.set_description(description);
        imp.stack.set_visible_child_name("status");
    }

    fn set_track(&self, track: Option<TrackObject>) {
        let imp = self.imp();
        let id = track.as_ref().map(TrackObject::id);
        if imp.track_id.get() == id && id.is_some() {
            return;
        }
        imp.track_id.set(id);
        imp.lines.replace(Vec::new());
        self.rebuild();
        let Some(track) = track else {
            self.show_status("gatefold-lyrics-symbolic", &gettext("No Lyrics"), None);
            return;
        };
        let path = track.track().record.path.clone();
        let title = track.track().record.tags.title.clone();
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = view)]
            self,
            async move {
                let state =
                    gio::spawn_blocking(move || gatefold_lyrics::load(&path, title.as_deref()))
                        .await
                        .unwrap_or(LyricsState::None);
                // The track may have changed while loading.
                if view.imp().track_id.get() == id {
                    view.show_lyrics(state);
                }
            }
        ));
    }

    fn show_lyrics(&self, state: LyricsState) {
        let imp = self.imp();
        match state {
            LyricsState::Lyrics(lyrics) => {
                imp.synced.set(lyrics.synced);
                imp.lines.replace(lyrics.lines);
                self.rebuild();
                imp.stack.set_visible_child_name("lyrics");
                let player = GatefoldApplication::get().player();
                self.set_position(player.position());
            }
            LyricsState::Instrumental => {
                self.show_status("folder-music-symbolic", &gettext("Instrumental"), None)
            }
            LyricsState::None => self.show_status(
                "gatefold-lyrics-symbolic",
                &gettext("No Lyrics"),
                Some(&gettext(
                    "This song has no lyrics in its tags or next to the file",
                )),
            ),
        }
    }

    /// Recreates the line widgets (new lyrics or a changed layout setting).
    fn rebuild(&self) {
        let imp = self.imp();
        while let Some(child) = imp.lines_box.first_child() {
            imp.lines_box.remove(&child);
        }
        imp.active.set(None);
        let layout = imp.settings.string("lyrics-layout");
        let synced = imp.synced.get();
        let mut rows = Vec::new();
        for line in imp.lines.borrow().iter() {
            let (big, small) = match (layout.as_str(), &line.secondary) {
                ("romanization-top", Some(secondary)) => {
                    (secondary.as_str(), Some(line.primary.as_str()))
                }
                ("original-only", _) => (line.primary.as_str(), None),
                (_, secondary) => (line.primary.as_str(), secondary.as_deref()),
            };
            let content = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(2)
                .build();
            content.append(
                &gtk::Label::builder()
                    // Instrumental breaks are empty lines.
                    .label(if big.is_empty() { "♪" } else { big })
                    .wrap(true)
                    .justify(gtk::Justification::Center)
                    .css_classes(["lyric-primary"])
                    .build(),
            );
            if let Some(small) = small {
                content.append(
                    &gtk::Label::builder()
                        .label(small)
                        .wrap(true)
                        .justify(gtk::Justification::Center)
                        .css_classes(["lyric-secondary"])
                        .build(),
                );
            }
            let button = gtk::Button::builder()
                .child(&content)
                .css_classes(["flat", "lyric-line"])
                .build();
            if synced {
                button.add_css_class("synced");
                if let Some(time) = line.time_ms {
                    button.connect_clicked(move |_| {
                        GatefoldApplication::get().player().seek(time);
                    });
                }
            } else {
                button.set_can_target(false);
            }
            imp.lines_box.append(&button);
            rows.push((line.time_ms, button));
        }
        imp.rows.replace(rows);
    }

    fn set_position(&self, position_ms: u64) {
        let imp = self.imp();
        if !imp.synced.get() {
            return;
        }
        let active = {
            let rows = imp.rows.borrow();
            rows.iter()
                .rposition(|(time, _)| time.is_some_and(|t| t <= position_ms))
        };
        if active == imp.active.get() {
            return;
        }
        tracing::debug!("Lyrics line {active:?} at {position_ms} ms");
        if let Some(previous) = imp.active.get()
            && let Some((_, button)) = imp.rows.borrow().get(previous)
        {
            button.remove_css_class("active");
        }
        imp.active.set(active);
        if let Some(index) = active
            && let Some((_, button)) = imp.rows.borrow().get(index)
        {
            button.add_css_class("active");
            self.scroll_to(button);
        }
    }

    /// Scrolls the line into the middle, unless the user scrolled recently.
    fn scroll_to(&self, line: &gtk::Button) {
        let imp = self.imp();
        if imp
            .paused_until
            .get()
            .is_some_and(|until| Instant::now() < until)
        {
            return;
        }
        let Some(point) = line.compute_point(
            &imp.lines_box,
            &gtk::graphene::Point::new(0.0, line.height() as f32 / 2.0),
        ) else {
            return;
        };
        let adjustment = imp.scroller.vadjustment();
        let target = (f64::from(point.y()) + f64::from(imp.lines_box.margin_top())
            - adjustment.page_size() / 2.0)
            .clamp(
                adjustment.lower(),
                (adjustment.upper() - adjustment.page_size()).max(0.0),
            );

        if let Some(animation) = imp.animation.take() {
            animation.pause();
        }
        let target_fn = adw::CallbackAnimationTarget::new(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |value| {
                let imp = view.imp();
                imp.auto_scrolling.set(true);
                imp.scroller.vadjustment().set_value(value);
                imp.auto_scrolling.set(false);
            }
        ));
        let animation = adw::SpringAnimation::new(
            &imp.scroller,
            adjustment.value(),
            target,
            adw::SpringParams::new(1.0, 1.0, 120.0),
            target_fn,
        );
        animation.set_epsilon(0.5);
        animation.play();
        imp.animation.replace(Some(animation));
    }
}
