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

/// A count-in shows when the first line starts this late.
const COUNT_IN_FROM_MS: u64 = 5000;

/// How long scrolling to the next line takes.
const SCROLL_MS: u32 = 650;

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
        pub animation: RefCell<Option<adw::TimedAnimation>>,
        pub settings: gio::Settings,
        pub lines: RefCell<Vec<Line>>,
        /// The last loaded state, to skip rebuilding unchanged lyrics.
        pub state: RefCell<Option<LyricsState>>,
        /// The current track's album folder, for lyrembed.
        pub album_dir: RefCell<Option<std::path::PathBuf>>,
        pub status_actions: gtk::Box,
        /// Before a late first line: three dots that fill over its last
        /// three seconds, and the time it starts.
        pub count_in: RefCell<Option<(u64, gtk::Box)>>,
        /// A Spotify song's lyrics are to be asked for once the view shows
        /// (LRCLIB is only asked while the lyrics are on screen).
        pub online_pending: Cell<bool>,
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
                state: RefCell::default(),
                album_dir: RefCell::default(),
                status_actions: gtk::Box::builder()
                    .orientation(gtk::Orientation::Vertical)
                    .spacing(12)
                    .halign(gtk::Align::Center)
                    .build(),
                count_in: RefCell::default(),
                online_pending: Cell::default(),
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
        self.connect_map(|view| {
            if view.imp().online_pending.take()
                && let Some(track) = GatefoldApplication::get().player().current_track()
                && Some(track.id()) == view.imp().track_id.get()
            {
                view.load_online(track);
            }
        });
        let clamp = adw::Clamp::builder()
            .maximum_size(640)
            .child(&imp.lines_box)
            .build();
        imp.scroller.set_child(Some(&clamp));
        imp.stack.add_named(&imp.scroller, Some("lyrics"));
        imp.stack.add_named(&imp.status, Some("status"));
        // "No lyrics": get them with lyrembed (§10.1).
        let find = gtk::Button::builder()
            .label(gettext("_Find Automatically"))
            .use_underline(true)
            .css_classes(["pill", "suggested-action"])
            .build();
        find.connect_clicked(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_| {
                if let Some(dir) = view.imp().album_dir.borrow().clone() {
                    GatefoldApplication::get().tools().find_lyrics(&dir);
                }
            }
        ));
        imp.status_actions.append(&find);
        imp.status_actions.append(&self.lyrembed_button());
        imp.status.set_child(Some(&imp.status_actions));
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
                move |_, _| {
                    view.rebuild();
                    // From "original only": the romanization may be missing.
                    if let Some(LyricsState::Lyrics(lyrics)) = view.imp().state.borrow().clone() {
                        view.add_romanization(lyrics);
                    }
                }
            ),
        );
        // LRCLIB turned on or off: the Spotify song that plays shows (or
        // stops showing) its lyrics now.
        for key in ["lyrics-lrclib", "online-metadata"] {
            imp.settings.connect_changed(
                Some(key),
                glib::clone!(
                    #[weak(rename_to = view)]
                    self,
                    move |_, _| {
                        let track = GatefoldApplication::get().player().current_track();
                        if track.as_ref().is_some_and(TrackObject::is_stand_in) {
                            view.imp().track_id.set(None);
                            view.set_track(track);
                        }
                    }
                ),
            );
        }
        // Tools write lyrics into the files; rescans bring them here live.
        GatefoldApplication::get().library().connect_local(
            "reloaded",
            false,
            glib::clone!(
                #[weak(rename_to = view)]
                self,
                #[upgrade_or]
                None,
                move |_| {
                    view.reload();
                    None
                }
            ),
        );
        GatefoldApplication::get()
            .tools()
            .connect_lyrics_jobs_notify(glib::clone!(
                #[weak(rename_to = view)]
                self,
                move |_| {
                    if view.imp().state.borrow().as_ref() == Some(&LyricsState::None) {
                        view.show_lyrics(LyricsState::None);
                    }
                }
            ));
        self.set_track(player.current_track());
    }

    /// "Check & Edit in lyrembed…": lyrembed's page for the album.
    fn lyrembed_button(&self) -> gtk::Button {
        let button = gtk::Button::builder()
            .label(gettext("_Check & Edit in lyrembed…"))
            .use_underline(true)
            .halign(gtk::Align::Center)
            .css_classes(["pill"])
            .build();
        button.connect_clicked(glib::clone!(
            #[weak(rename_to = view)]
            self,
            move |_| {
                if let Some(dir) = view.imp().album_dir.borrow().clone() {
                    GatefoldApplication::get().tools().lyrembed_page(&dir);
                }
            }
        ));
        button
    }

    /// Reloads the current track's lyrics, keeping the view if unchanged.
    fn reload(&self) {
        let Some(track) = GatefoldApplication::get().player().current_track() else {
            return;
        };
        // A Spotify song has no file: its lyrics (LRCLIB's) stay.
        if track.is_stand_in() {
            return;
        }
        let id = Some(track.id());
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
                let imp = view.imp();
                if imp.track_id.get() == id && imp.state.borrow().as_ref() != Some(&state) {
                    view.show_lyrics(state);
                }
            }
        ));
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
        imp.state.replace(None);
        imp.album_dir.replace(track.as_ref().map(|t| {
            gatefold_core::library::grouping::album_folder(&t.track().record.path).to_path_buf()
        }));
        self.rebuild();
        let Some(track) = track else {
            self.show_status("gatefold-lyrics-symbolic", &gettext("No Lyrics"), None);
            return;
        };
        imp.online_pending.set(false);
        // Spotify gives apps no lyrics; LRCLIB may have them (Preferences).
        if track.is_stand_in() && crate::lrclib::enabled() {
            imp.status_actions.set_visible(false);
            self.show_status(
                "gatefold-lyrics-symbolic",
                &gettext("Looking for Lyrics…"),
                Some(&gettext("On LRCLIB")),
            );
            if self.is_mapped() {
                self.load_online(track);
            } else {
                imp.online_pending.set(true);
            }
            return;
        }
        if track.is_stand_in() {
            imp.status_actions.set_visible(false);
            self.show_status(
                "folder-remote-symbolic",
                &gettext("No Lyrics for This Song"),
                Some(&gettext("Lyrics show for songs that are in your library")),
            );
            return;
        }
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

    /// Non-Latin lyrics without romanization get it from lyrembed (§5.1),
    /// shown once it's there; not with the "original only" layout.
    fn add_romanization(&self, lyrics: gatefold_lyrics::Lyrics) {
        let imp = self.imp();
        if imp.settings.string("lyrics-layout") == "original-only"
            || !lyrics.needs_romanization()
            || *imp.lines.borrow() != lyrics.lines
        {
            return;
        }
        let id = imp.track_id.get();
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = view)]
            self,
            async move {
                let Some(romanized) = crate::romanizer::romanize(&lyrics).await else {
                    return;
                };
                let imp = view.imp();
                // Another song, or other lyrics, meanwhile.
                if imp.track_id.get() != id || *imp.lines.borrow() != lyrics.lines {
                    return;
                }
                imp.lines.replace(romanized.lines);
                view.rebuild();
                view.set_position(GatefoldApplication::get().player().position());
            }
        ));
    }

    /// A Spotify song's lyrics from LRCLIB.
    fn load_online(&self, track: TrackObject) {
        let id = Some(track.id());
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = view)]
            self,
            async move {
                let wanted = view.downgrade();
                let still_wanted = move || {
                    wanted
                        .upgrade()
                        .is_some_and(|v| v.imp().track_id.get() == id)
                };
                let state = crate::lrclib::lyrics(&track, still_wanted).await;
                if view.imp().track_id.get() != id {
                    return;
                }
                match state {
                    Some(LyricsState::None) => view.show_status(
                        "folder-remote-symbolic",
                        &gettext("No Lyrics for This Song"),
                        Some(&gettext("LRCLIB doesn't have lyrics for it")),
                    ),
                    Some(state) => view.show_lyrics(state),
                    None => view.show_status(
                        "folder-remote-symbolic",
                        &gettext("No Lyrics for This Song"),
                        Some(&gettext("LRCLIB can't be asked right now")),
                    ),
                }
            }
        ));
    }

    fn show_lyrics(&self, state: LyricsState) {
        let imp = self.imp();
        imp.state.replace(Some(state.clone()));
        let tools = GatefoldApplication::get().tools();
        let has_lyrembed = tools.lyrembed_usable();
        imp.status_actions
            .set_visible(state == LyricsState::None && has_lyrembed && tools.lyrics_jobs() == 0);
        if state == LyricsState::None && tools.lyrics_jobs() > 0 {
            return self.show_status(
                "gatefold-lyrics-symbolic",
                &gettext("Looking for Lyrics…"),
                Some(&gettext("lyrembed is searching for this album's lyrics")),
            );
        }
        match state {
            LyricsState::Lyrics(lyrics) => {
                imp.synced.set(lyrics.synced);
                imp.lines.replace(lyrics.lines.clone());
                self.rebuild();
                imp.stack.set_visible_child_name("lyrics");
                let player = GatefoldApplication::get().player();
                self.set_position(player.position());
                self.add_romanization(lyrics);
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
        // A count-in when the singing starts late.
        let first = imp.lines.borrow().iter().find_map(|l| l.time_ms);
        let count_in = first
            .filter(|&t| synced && t >= COUNT_IN_FROM_MS)
            .map(|first| {
                let dots = gtk::Box::builder()
                    .spacing(12)
                    .halign(gtk::Align::Center)
                    .margin_top(12)
                    .margin_bottom(12)
                    .css_classes(["count-in"])
                    .build();
                for _ in 0..3 {
                    dots.append(
                        &gtk::Box::builder()
                            .valign(gtk::Align::Center)
                            .css_classes(["count-dot"])
                            .build(),
                    );
                }
                imp.lines_box.append(&dots);
                (first, dots)
            });
        imp.count_in.replace(count_in);
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
        if !imp.lines.borrow().is_empty() && GatefoldApplication::get().tools().lyrembed_usable() {
            let button = self.lyrembed_button();
            button.set_margin_top(24);
            button.add_css_class("flat");
            button.remove_css_class("pill");
            imp.lines_box.append(&button);
        }
    }

    fn set_position(&self, position_ms: u64) {
        let imp = self.imp();
        if !imp.synced.get() {
            return;
        }
        self.update_count_in(position_ms);
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

    /// Fills one dot per second over the last three before the first line.
    fn update_count_in(&self, position_ms: u64) {
        let imp = self.imp();
        let count_in = imp.count_in.borrow();
        let Some((first, dots)) = count_in.as_ref() else {
            return;
        };
        let remaining = first.saturating_sub(position_ms);
        let filled = if remaining == 0 || remaining > 3000 {
            0
        } else {
            3 - (remaining - 1) / 1000
        };
        let mut dot = dots.first_child();
        let mut index = 0;
        while let Some(widget) = dot {
            if index < filled {
                widget.add_css_class("filled");
            } else {
                widget.remove_css_class("filled");
            }
            index += 1;
            dot = widget.next_sibling();
        }
        if remaining == 0 {
            dots.add_css_class("done");
        } else {
            dots.remove_css_class("done");
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
        // A smooth glide that eases in and out; no overshoot.
        let animation = adw::TimedAnimation::builder()
            .widget(&imp.scroller)
            .value_from(adjustment.value())
            .value_to(target)
            .duration(SCROLL_MS)
            .easing(adw::Easing::EaseInOutCubic)
            .target(&target_fn)
            .build();
        animation.play();
        imp.animation.replace(Some(animation));
    }
}
