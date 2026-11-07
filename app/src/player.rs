//! Playback for the whole app: the queue, the GStreamer engine and the
//! `player.*` actions (CONCEPT.md §6, §7).

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::time::Duration;

use adw::prelude::*;
use gatefold_core::playback::{self, PlayerEvent};
use gatefold_core::queue::{Queue, RepeatMode};
use gatefold_core::similar::{self, Criteria, SongFeatures};
use gettextrs::gettext;
use gtk::glib::subclass::Signal;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};

use crate::config::APP_ID;
use crate::library::Library;
use crate::objects::{QueueEntry, TrackObject};
use crate::spotify::RemoteCommand;

/// How often the position is updated while playing (lyrics follow it).
const TICK: Duration = Duration::from_millis(150);
/// The queue is saved this long after its last change (§6.2).
const SAVE_DELAY: Duration = Duration::from_secs(2);
/// A song counts as played (for the history) after half of it, or this long.
const HISTORY_MIN_MS: u64 = 30_000;

/// After this many streamed songs fail in a row, playback stops.
const STREAM_FAILURES_TO_STOP: u32 = 3;

/// Songs handed to a Spotify device when something is started remotely.
pub const REMOTE_HAND_OVER: usize = 20;

/// "Previous" restarts the track instead when it has played this long.
const RESTART_THRESHOLD_MS: u64 = 3000;

/// Takes the controls' commands in remote mode.
type RemoteHandler = Box<dyn Fn(RemoteCommand)>;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    Stopped,
    Paused,
    Playing,
}

mod imp {
    use super::*;

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::Player)]
    pub struct Player {
        #[property(get, nullable)]
        pub current_track: RefCell<Option<TrackObject>>,
        #[property(get)]
        pub playing: Cell<bool>,
        /// Milliseconds.
        #[property(get)]
        pub position: Cell<u64>,
        /// Milliseconds.
        #[property(get)]
        pub duration: Cell<u64>,
        #[property(get)]
        pub shuffle: Cell<bool>,
        /// "off", "all" or "one".
        #[property(get)]
        pub repeat: RefCell<String>,
        /// The Spotify device being controlled (remote mode, §9.3).
        #[property(get, nullable)]
        pub remote_device: RefCell<Option<String>>,
        /// Where transport commands go in remote mode.
        pub remote: RefCell<Option<RemoteHandler>>,
        /// The current song plays from the Spotify stream (§9.4).
        pub streaming: Cell<bool>,
        /// Streamed songs that failed in a row.
        pub stream_failures: Cell<u32>,
        /// The local position, kept while remote.
        pub local_position: Cell<u64>,
        /// Remote position and when it was reported, to move it along.
        pub remote_clock: Cell<Option<(std::time::Instant, u64)>>,
        pub remote_tick: RefCell<Option<glib::SourceId>>,
        /// What the remote device plays next, for the queue view.
        pub remote_queue: OnceCell<gio::ListStore>,

        pub engine: OnceCell<playback::Player>,
        pub library: OnceCell<Library>,
        pub settings: OnceCell<gio::Settings>,
        pub actions: gio::SimpleActionGroup,
        pub queue: RefCell<Queue>,
        /// Queue entries from the current one on, for the queue view.
        pub queue_model: OnceCell<gio::ListStore>,
        pub tracks: RefCell<HashMap<i64, TrackObject>>,
        /// The library's similarity data, loaded when first needed.
        pub similarity: RefCell<Option<std::rc::Rc<Vec<SongFeatures>>>>,
        pub state: Cell<State>,
        /// Queue item handed to GStreamer for gapless playback.
        pub pending_next: Cell<Option<u64>>,
        /// "Next From: …" for queues not started from an album (an artist,
        /// a playlist). Not saved with the queue.
        pub context_name: RefCell<Option<String>>,
        pub tick: RefCell<Option<glib::SourceId>>,
        pub save: RefCell<Option<glib::SourceId>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Player {
        const NAME: &'static str = "GatefoldPlayer";
        type Type = super::Player;
    }

    #[glib::derived_properties]
    impl ObjectImpl for Player {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    // The queue's items or order changed.
                    Signal::builder("queue-changed").build(),
                    // A song was added to the play history.
                    Signal::builder("history-changed").build(),
                    // The position jumped (seek or new track); for MPRIS.
                    Signal::builder("seeked").build(),
                    // A message for the user (toast).
                    Signal::builder("message")
                        .param_types([String::static_type()])
                        .build(),
                ]
            })
        }

        fn constructed(&self) {
            self.parent_constructed();
            self.repeat.replace(RepeatMode::Off.as_str().to_owned());
            self.queue_model
                .set(gio::ListStore::new::<QueueEntry>())
                .expect("set once");
            self.remote_queue
                .set(gio::ListStore::new::<QueueEntry>())
                .expect("set once");
        }
    }
}

glib::wrapper! {
    pub struct Player(ObjectSubclass<imp::Player>);
}

impl Player {
    pub fn new(library: &Library) -> Self {
        let player: Self = glib::Object::new();
        let imp = player.imp();
        imp.library.set(library.clone()).expect("set once");

        let settings = gio::Settings::new(APP_ID);
        settings.connect_changed(
            Some("replaygain"),
            glib::clone!(
                #[weak]
                player,
                move |_, _| player.apply_replaygain()
            ),
        );
        // Songs ↔ albums while shuffled: shuffle again the new way.
        settings.connect_changed(
            Some("shuffle-albums"),
            glib::clone!(
                #[weak]
                player,
                move |_, _| {
                    if player.imp().queue.borrow().is_shuffled() {
                        player.set_shuffle(false);
                        player.set_shuffle(true);
                    }
                }
            ),
        );
        // Mixing switched or changed: redo the queue's suggestions.
        for key in [
            "mix-similar",
            "mix-every",
            "mix-genre",
            "mix-bpm",
            "mix-artist",
            "mix-sound",
            "mix-bpm-tolerance",
        ] {
            settings.connect_changed(
                Some(key),
                glib::clone!(
                    #[weak]
                    player,
                    move |_, _| player.remix()
                ),
            );
        }
        imp.settings.set(settings).expect("set once");

        match playback::Player::new() {
            Ok((engine, events)) => {
                engine.set_volume(player.settings().double("volume"));
                imp.engine.set(engine).ok().expect("set once");
                glib::spawn_future_local(glib::clone!(
                    #[weak]
                    player,
                    async move {
                        while let Ok(event) = events.recv().await {
                            player.handle_event(event);
                        }
                    }
                ));
            }
            Err(err) => tracing::error!("Playback is unavailable: {err}"),
        }
        player.setup_actions();
        player.apply_replaygain();
        player
    }

    pub fn settings(&self) -> &gio::Settings {
        self.imp().settings.get().expect("set in new()")
    }

    fn library(&self) -> &Library {
        self.imp().library.get().expect("set in new()")
    }

    fn engine(&self) -> Option<&playback::Player> {
        self.imp().engine.get()
    }

    pub fn actions(&self) -> &gio::SimpleActionGroup {
        &self.imp().actions
    }

    /// The current track and what follows, for the queue view.
    pub fn queue_model(&self) -> &gio::ListStore {
        self.imp().queue_model.get().expect("set in constructed()")
    }

    pub fn has_previous(&self) -> bool {
        self.imp().queue.borrow().current().is_some()
    }

    pub fn has_next(&self) -> bool {
        self.imp().queue.borrow().peek_next(false).is_some()
    }

    fn setup_actions(&self) {
        let actions = &self.imp().actions;
        let entries = [
            gio::ActionEntry::builder("play-pause")
                .activate(glib::clone!(
                    #[weak(rename_to = player)]
                    self,
                    move |_: &gio::SimpleActionGroup, _, _| player.play_pause()
                ))
                .build(),
            gio::ActionEntry::builder("next")
                .activate(glib::clone!(
                    #[weak(rename_to = player)]
                    self,
                    move |_: &gio::SimpleActionGroup, _, _| player.next()
                ))
                .build(),
            gio::ActionEntry::builder("previous")
                .activate(glib::clone!(
                    #[weak(rename_to = player)]
                    self,
                    move |_: &gio::SimpleActionGroup, _, _| player.previous()
                ))
                .build(),
            gio::ActionEntry::builder("shuffle")
                .state(false.to_variant())
                .change_state(glib::clone!(
                    #[weak(rename_to = player)]
                    self,
                    move |_: &gio::SimpleActionGroup, _, value| {
                        if let Some(on) = value.and_then(|v| v.get::<bool>()) {
                            player.set_shuffle(on);
                        }
                    }
                ))
                .build(),
            gio::ActionEntry::builder("repeat")
                .activate(glib::clone!(
                    #[weak(rename_to = player)]
                    self,
                    move |_: &gio::SimpleActionGroup, _, _| player.cycle_repeat()
                ))
                .build(),
            gio::ActionEntry::builder("clear-queue")
                .activate(glib::clone!(
                    #[weak(rename_to = player)]
                    self,
                    move |_: &gio::SimpleActionGroup, _, _| player.clear_upcoming()
                ))
                .build(),
        ];
        actions.add_action_entries(entries);
        self.update_actions();
    }

    fn set_action_enabled(&self, name: &str, enabled: bool) {
        if let Some(action) = self
            .imp()
            .actions
            .lookup_action(name)
            .and_downcast::<gio::SimpleAction>()
        {
            action.set_enabled(enabled);
        }
    }

    fn update_actions(&self) {
        let (has_current, has_next, has_upcoming) = {
            let queue = self.imp().queue.borrow();
            let current = queue.current_index();
            (
                current.is_some(),
                queue.peek_next(false).is_some(),
                current.is_some_and(|c| c + 1 < queue.items().len()),
            )
        };
        if self.is_remote() {
            for name in ["play-pause", "next", "previous"] {
                self.set_action_enabled(name, true);
            }
            self.set_action_enabled("clear-queue", has_upcoming);
            return;
        }
        let has_engine = self.engine().is_some();
        self.set_action_enabled("play-pause", has_engine && has_current);
        self.set_action_enabled("next", has_engine && has_next);
        self.set_action_enabled("previous", has_engine && has_current);
        self.set_action_enabled("clear-queue", has_upcoming);
    }

    // Starting playback ------------------------------------------------------

    /// Plays an album from `start` (a track index), shuffled or in order.
    pub fn play_album(&self, album_id: i64, start: Option<usize>, shuffle: bool) {
        let tracks = self.library().album_tracks(album_id);
        if tracks.is_empty() {
            return;
        }
        let ids: Vec<i64> = tracks.iter().map(TrackObject::id).collect();
        self.cache_tracks(tracks);
        self.imp().context_name.replace(None);
        self.imp().queue.borrow_mut().play_context(
            &ids,
            start,
            Some(album_id),
            shuffle,
            &mut rand::rng(),
        );
        self.load_current(true, 0);
        self.mix_upcoming();
        self.queue_changed();
    }

    /// Plays a list of tracks (an artist's albums), in order or shuffled.
    /// `name` is the context for "Next From: …" (an artist or playlist);
    /// `start` the track to begin with.
    pub fn play_tracks(
        &self,
        tracks: Vec<TrackObject>,
        start: Option<usize>,
        shuffle: bool,
        name: Option<String>,
    ) {
        if tracks.is_empty() {
            return;
        }
        let ids: Vec<i64> = tracks.iter().map(TrackObject::id).collect();
        self.cache_tracks(tracks);
        self.imp().context_name.replace(name);
        let start = start.or((!shuffle).then_some(0));
        self.imp()
            .queue
            .borrow_mut()
            .play_context(&ids, start, None, shuffle, &mut rand::rng());
        self.load_current(true, 0);
        self.mix_upcoming();
        self.queue_changed();
    }

    /// Inserts tracks after the current one.
    pub fn play_next(&self, tracks: Vec<TrackObject>) {
        let ids: Vec<i64> = tracks.iter().map(TrackObject::id).collect();
        // Spotify's queue only takes songs at its end, which plays next.
        if self.forward(RemoteCommand::Queue(ids.clone())) {
            return;
        }
        let was_empty = self.imp().queue.borrow().current().is_none();
        self.cache_tracks(tracks);
        self.imp().queue.borrow_mut().play_next(&ids);
        self.after_adding(was_empty);
    }

    /// Adds tracks to the end of "Next up".
    pub fn add_to_queue(&self, tracks: Vec<TrackObject>) {
        let ids: Vec<i64> = tracks.iter().map(TrackObject::id).collect();
        if self.forward(RemoteCommand::Queue(ids.clone())) {
            return;
        }
        let was_empty = self.imp().queue.borrow().current().is_none();
        self.cache_tracks(tracks);
        self.imp().queue.borrow_mut().add(&ids);
        self.after_adding(was_empty);
    }

    fn after_adding(&self, was_empty: bool) {
        if was_empty {
            // Nothing was queued: show the first added track, paused.
            self.load_current(false, 0);
        }
        self.queue_changed();
        self.emit_by_name::<()>("message", &[&gettext("Added to the queue")]);
    }

    /// Plays the queue entry at `offset` in the queue model (0 = current).
    pub fn jump_to_upcoming(&self, offset: usize) {
        let index = self.imp().queue.borrow().current_index().unwrap_or(0) + offset;
        if self.imp().queue.borrow_mut().jump(index).is_some() {
            self.load_current(true, 0);
            self.queue_changed();
        }
    }

    /// Moves an upcoming entry (drag & drop); offsets as in the queue model.
    pub fn move_upcoming(&self, from: usize, to: usize) {
        let current = self.imp().queue.borrow().current_index().unwrap_or(0);
        let moved = self
            .imp()
            .queue
            .borrow_mut()
            .move_item(current + from, current + to);
        if moved {
            self.queue_changed();
        }
    }

    /// The album the queue was started from, for "Next From: …".
    pub fn context_title(&self) -> Option<String> {
        if let Some(name) = self.imp().context_name.borrow().clone() {
            return Some(name);
        }
        let album = self.imp().queue.borrow().context_album()?;
        self.library()
            .db()?
            .album(album)
            .ok()
            .flatten()
            .map(|a| a.title)
    }

    /// All queued tracks, for "Save as Playlist".
    pub fn queued_track_ids(&self) -> Vec<i64> {
        self.imp()
            .queue
            .borrow()
            .items()
            .iter()
            .map(|i| i.track_id)
            .collect()
    }

    /// The last `limit` played songs, oldest first.
    pub fn history(&self, limit: usize) -> Vec<TrackObject> {
        let library = self.library();
        let ids = library
            .db()
            .and_then(|db| db.history(limit).ok())
            .unwrap_or_default();
        let tracks = library.tracks(&ids);
        ids.iter()
            .filter_map(|id| tracks.get(id).cloned().map(TrackObject::new))
            .collect()
    }

    /// Empties "Recently Played".
    pub fn clear_history(&self) {
        let cleared = self.library().db().map(|mut db| db.clear_history());
        match cleared {
            Some(Ok(())) => self.emit_by_name::<()>("history-changed", &[]),
            Some(Err(err)) => tracing::warn!("Cannot clear the play history: {err}"),
            None => {}
        }
    }

    fn record_history(&self, track_id: i64) {
        let recorded = self
            .library()
            .db()
            .map(|mut db| db.add_history(track_id, crate::metadata::now()));
        match recorded {
            Some(Ok(())) => self.emit_by_name::<()>("history-changed", &[]),
            Some(Err(err)) => tracing::warn!("Cannot record the play history: {err}"),
            None => {}
        }
    }

    /// Plays `track` right away; the rest of the queue stays.
    pub fn play_now(&self, track: TrackObject) {
        let id = track.id();
        self.cache_tracks(vec![track]);
        let had_current = self.imp().queue.borrow().current().is_some();
        self.imp().queue.borrow_mut().play_next(&[id]);
        if had_current {
            self.imp().queue.borrow_mut().advance(false);
        }
        self.load_current(true, 0);
        self.queue_changed();
    }

    /// Removes the queue entry at `offset` in the queue model (> 0).
    pub fn remove_upcoming(&self, offset: usize) {
        let index = self.imp().queue.borrow().current_index().unwrap_or(0) + offset;
        if self.imp().queue.borrow_mut().remove(index) {
            self.queue_changed();
        }
    }

    fn clear_upcoming(&self) {
        self.imp().queue.borrow_mut().clear_upcoming();
        self.queue_changed();
    }

    fn cache_tracks(&self, tracks: Vec<TrackObject>) {
        let mut cache = self.imp().tracks.borrow_mut();
        for track in tracks {
            cache.insert(track.id(), track);
        }
    }

    fn track(&self, id: i64) -> Option<TrackObject> {
        if let Some(track) = self.imp().tracks.borrow().get(&id) {
            return Some(track.clone());
        }
        let track = self
            .library()
            .tracks(&[id])
            .remove(&id)
            .map(TrackObject::new)?;
        self.imp().tracks.borrow_mut().insert(id, track.clone());
        Some(track)
    }

    // Transport --------------------------------------------------------------

    pub fn play_pause(&self) {
        if self.forward(RemoteCommand::PlayPause) {
            return;
        }
        if self.imp().streaming.get() {
            let spotify = self.spotify();
            match self.imp().state.get() {
                State::Playing => {
                    spotify.stream_pause();
                    self.set_state(State::Paused);
                }
                State::Paused => {
                    spotify.stream_play();
                    self.set_state(State::Playing);
                }
                State::Stopped => self.load_current(true, self.position()),
            }
            return;
        }
        let Some(engine) = self.engine() else { return };
        match self.imp().state.get() {
            State::Playing => {
                engine.pause();
                self.set_state(State::Paused);
            }
            State::Paused => {
                engine.play();
                self.set_state(State::Playing);
            }
            State::Stopped => self.load_current(true, self.position()),
        }
    }

    pub fn play(&self) {
        if self.imp().state.get() != State::Playing {
            self.play_pause();
        }
    }

    pub fn pause(&self) {
        if self.imp().state.get() == State::Playing {
            self.play_pause();
        }
    }

    pub fn stop(&self) {
        if let Some(engine) = self.engine() {
            engine.stop();
        }
        if self.imp().streaming.get() {
            self.spotify().stream_stop();
        }
        self.set_state(State::Stopped);
        self.set_position(0);
    }

    pub fn next(&self) {
        if self.forward(RemoteCommand::Next) {
            return;
        }
        let advanced = self.imp().queue.borrow_mut().advance(false).is_some();
        if advanced {
            self.load_current(self.imp().state.get() == State::Playing, 0);
            self.queue_changed();
        }
    }

    pub fn previous(&self) {
        if self.forward(RemoteCommand::Previous) {
            return;
        }
        if self.position() > RESTART_THRESHOLD_MS {
            self.seek(0);
            return;
        }
        let moved = self.imp().queue.borrow_mut().go_previous().is_some();
        if moved {
            self.load_current(self.imp().state.get() == State::Playing, 0);
            self.queue_changed();
        } else {
            self.seek(0);
        }
    }

    pub fn seek(&self, ms: u64) {
        let ms = ms.min(self.duration());
        if self.forward(RemoteCommand::Seek(ms)) {
            self.imp()
                .remote_clock
                .set(Some((std::time::Instant::now(), ms)));
            self.set_position(ms);
            self.emit_by_name::<()>("seeked", &[]);
            return;
        }
        if self.imp().streaming.get() {
            if self.imp().state.get() != State::Stopped {
                self.spotify().stream_seek(ms);
            }
        } else if let Some(engine) = self.engine()
            && self.imp().state.get() != State::Stopped
        {
            engine.seek(ms);
        }
        self.set_position(ms);
        self.emit_by_name::<()>("seeked", &[]);
        self.schedule_save();
    }

    pub fn set_shuffle(&self, shuffle: bool) {
        if self.forward(RemoteCommand::Shuffle(shuffle)) {
            return;
        }
        // "Shuffle albums" (§7): albums in random order, each in order.
        let albums = self.settings().boolean("shuffle-albums");
        let album_of = |id: i64| self.track(id).and_then(|t| t.track().album_id);
        self.imp().queue.borrow_mut().set_shuffle_by(
            shuffle,
            albums.then_some(album_of),
            &mut rand::rng(),
        );
        // Suggestions go where the new order puts album songs.
        self.mix_upcoming();
        self.queue_changed();
        self.apply_replaygain();
    }

    pub fn set_repeat(&self, repeat: RepeatMode) {
        if self.forward(RemoteCommand::Repeat(repeat)) {
            return;
        }
        self.imp().queue.borrow_mut().set_repeat(repeat);
        self.queue_changed();
    }

    fn cycle_repeat(&self) {
        let repeat = if self.is_remote() {
            RepeatMode::from_str_lossy(&self.repeat()).cycle()
        } else {
            self.imp().queue.borrow().repeat().cycle()
        };
        self.set_repeat(repeat);
    }

    pub fn volume(&self) -> f64 {
        self.engine().map_or(1.0, playback::Player::volume)
    }

    pub fn set_volume(&self, volume: f64) {
        if self.forward(RemoteCommand::Volume(volume.clamp(0.0, 1.0))) {
            return;
        }
        if let Some(engine) = self.engine() {
            engine.set_volume(volume);
        }
        if self.imp().streaming.get() {
            self.spotify().stream_volume(volume);
        }
        let _ = self.settings().set_double("volume", volume.clamp(0.0, 1.0));
    }

    // Remote mode (§9.3) ------------------------------------------------

    pub fn is_remote(&self) -> bool {
        self.imp().remote.borrow().is_some()
    }

    /// The song plays from Spotify: streamed, or on a Spotify device.
    pub fn plays_from_spotify(&self) -> bool {
        self.is_remote() || self.current_track().is_some_and(|t| t.is_stand_in())
    }

    /// Sends a command to the remote device; false when playing here.
    fn forward(&self, command: RemoteCommand) -> bool {
        match self.imp().remote.borrow().as_ref() {
            Some(handler) => {
                handler(command);
                true
            }
            None => false,
        }
    }

    /// Hands the controls to a Spotify device. Local playback pauses.
    pub fn enter_remote(&self, device: &str, handler: impl Fn(RemoteCommand) + 'static) {
        let imp = self.imp();
        if !self.is_remote() {
            if imp.state.get() == State::Playing
                && let Some(engine) = self.engine()
            {
                engine.pause();
                self.set_state(State::Paused);
            }
            imp.local_position.set(self.position());
            self.save();
        }
        imp.remote.replace(Some(Box::new(handler)));
        self.set_remote_device(Some(device));
        self.update_actions();
    }

    pub fn set_remote_device(&self, device: Option<&str>) {
        let imp = self.imp();
        if imp.remote_device.borrow().as_deref() != device {
            imp.remote_device.replace(device.map(str::to_owned));
            self.notify_remote_device();
        }
    }

    /// Shows what the remote device plays.
    pub fn show_remote(
        &self,
        track: Option<TrackObject>,
        playing: bool,
        position: u64,
        shuffle: bool,
        repeat: RepeatMode,
    ) {
        let imp = self.imp();
        if !self.is_remote() {
            return;
        }
        let changed = imp.current_track.borrow().as_ref().map(TrackObject::id)
            != track.as_ref().map(TrackObject::id);
        let duration = track.as_ref().map_or(0, TrackObject::duration_ms);
        imp.current_track.replace(track);
        if imp.duration.replace(duration) != duration {
            self.notify_duration();
        }
        if changed {
            self.notify_current_track();
        }
        imp.remote_clock
            .set(playing.then(|| (std::time::Instant::now(), position)));
        self.set_position(position);
        if changed {
            self.emit_by_name::<()>("seeked", &[]);
        }
        if imp.playing.replace(playing) != playing {
            self.notify_playing();
        }
        self.show_shuffle_repeat(shuffle, repeat);
        // Move the position along between reports (lyrics follow it).
        if playing && imp.remote_tick.borrow().is_none() {
            let source = glib::timeout_add_local(
                TICK,
                glib::clone!(
                    #[weak(rename_to = player)]
                    self,
                    #[upgrade_or]
                    glib::ControlFlow::Break,
                    move || {
                        if let Some((at, ms)) = player.imp().remote_clock.get() {
                            let now = ms + at.elapsed().as_millis() as u64;
                            player.set_position(now.min(player.duration()));
                        }
                        glib::ControlFlow::Continue
                    }
                ),
            );
            imp.remote_tick.replace(Some(source));
        } else if !playing && let Some(source) = imp.remote_tick.take() {
            source.remove();
        }
    }

    /// Shows shuffle and repeat (the queue's, or the remote device's).
    fn show_shuffle_repeat(&self, shuffle: bool, repeat: RepeatMode) {
        let imp = self.imp();
        if imp.shuffle.replace(shuffle) != shuffle {
            self.notify_shuffle();
        }
        if let Some(action) = imp
            .actions
            .lookup_action("shuffle")
            .and_downcast::<gio::SimpleAction>()
        {
            action.set_state(&shuffle.to_variant());
        }
        if *imp.repeat.borrow() != repeat.as_str() {
            imp.repeat.replace(repeat.as_str().to_owned());
            self.notify_repeat();
        }
    }

    /// The device's current song and queue (remote mode).
    pub fn remote_queue(&self) -> &gio::ListStore {
        self.imp().remote_queue.get().expect("set in constructed()")
    }

    pub fn set_remote_queue(&self, tracks: Vec<TrackObject>) {
        let entries: Vec<QueueEntry> = tracks
            .into_iter()
            .enumerate()
            .map(|(offset, track)| {
                QueueEntry::new(offset, track, gatefold_core::queue::Origin::Context)
            })
            .collect();
        let model = self.remote_queue();
        model.splice(0, model.n_items(), &entries);
    }

    /// Back to playing here, paused where the local queue was.
    pub fn leave_remote(&self) {
        let imp = self.imp();
        if imp.remote.take().is_none() {
            return;
        }
        if let Some(source) = imp.remote_tick.take() {
            source.remove();
        }
        imp.remote_clock.set(None);
        self.remote_queue().remove_all();
        self.set_remote_device(None);
        let current = imp.queue.borrow().current().copied();
        let track = current.and_then(|item| self.track(item.track_id));
        let duration = track.as_ref().map_or(0, TrackObject::duration_ms);
        imp.current_track.replace(track);
        imp.duration.replace(duration);
        self.notify_duration();
        self.notify_current_track();
        self.set_position(imp.local_position.get());
        self.emit_by_name::<()>("seeked", &[]);
        self.set_state(if imp.state.get() == State::Stopped {
            State::Stopped
        } else {
            State::Paused
        });
        if imp.playing.replace(false) {
            self.notify_playing();
        }
        self.queue_changed();
    }

    /// Plays the queue's current song here (after remote mode ended).
    pub fn play_current(&self) {
        self.load_current(true, 0);
    }

    /// The current track and up to `limit` after it (remote hand-over).
    pub fn upcoming_track_ids(&self, limit: usize) -> Vec<i64> {
        let queue = self.imp().queue.borrow();
        let start = queue.current_index().unwrap_or(0);
        queue
            .items()
            .iter()
            .skip(start)
            .take(limit)
            .map(|i| i.track_id)
            .collect()
    }

    // Internals --------------------------------------------------------------

    /// Loads the queue's current track into GStreamer.
    fn load_current(&self, play: bool, start_ms: u64) {
        // Remote mode: what was started plays on the device; the local
        // queue only feeds it.
        if self.is_remote() {
            if play {
                let ids = self.upcoming_track_ids(REMOTE_HAND_OVER);
                self.forward(RemoteCommand::Play(ids));
            }
            return;
        }
        let imp = self.imp();
        imp.pending_next.set(None);
        let current = imp.queue.borrow().current().copied();
        let track = current.and_then(|item| self.track(item.track_id));
        self.set_current_track(track.clone());

        let Some(track) = track else {
            self.stop();
            return;
        };
        // A Spotify song that isn't in the library: streamed (§9.4).
        if track.is_stand_in() {
            self.load_stream(&track, play, start_ms);
            return;
        }
        if imp.streaming.replace(false) {
            self.spotify().stream_stop();
        }
        let Some(engine) = self.engine() else { return };
        let Some(uri) = playback::file_uri(&track.track().record.path) else {
            return;
        };
        self.apply_replaygain();
        engine.load(&uri, start_ms, play);
        self.set_position(start_ms);
        self.set_state(if play { State::Playing } else { State::Paused });
        self.emit_by_name::<()>("seeked", &[]);
        self.update_next();
    }

    // Streaming (§9.4) --------------------------------------------------

    fn spotify(&self) -> crate::spotify::Spotify {
        crate::application::GatefoldApplication::get().spotify()
    }

    fn stream_uri(&self, track: &TrackObject) -> Option<String> {
        self.library().db()?.stream_uri(track.id()).ok().flatten()
    }

    fn load_stream(&self, track: &TrackObject, play: bool, start_ms: u64) {
        let imp = self.imp();
        // Not playing yet (a restored queue): it waits for "play", by when
        // streaming has connected; play_pause() then loads it.
        if !play {
            if imp.streaming.replace(false) {
                self.spotify().stream_stop();
            }
            if let Some(engine) = self.engine() {
                engine.stop();
            }
            self.set_position(start_ms);
            self.set_state(State::Stopped);
            self.emit_by_name::<()>("seeked", &[]);
            return;
        }
        let spotify = self.spotify();
        let uri = self.stream_uri(track);
        let (Some(uri), true) = (uri, spotify.can_stream()) else {
            // Translators: {} is a song title.
            let text =
                gettext("“{}” is only on Spotify; turn on streaming in Preferences to play it")
                    .replace("{}", &track.title());
            self.emit_by_name::<()>("message", &[&text]);
            imp.streaming.set(false);
            self.set_state(State::Stopped);
            return;
        };
        if let Some(engine) = self.engine() {
            engine.stop();
        }
        imp.streaming.set(true);
        spotify.stream_volume(self.volume());
        spotify.stream_load(&uri, start_ms, play);
        self.set_position(start_ms);
        self.set_state(if play { State::Playing } else { State::Paused });
        self.emit_by_name::<()>("seeked", &[]);
        self.update_next();
    }

    /// What the stream reports about the current song.
    pub fn stream_note(&self, note: crate::spotify::StreamNote) {
        use crate::spotify::StreamNote;
        let imp = self.imp();
        if !imp.streaming.get() {
            return;
        }
        match note {
            StreamNote::Playing => {
                imp.stream_failures.set(0);
                if imp.state.get() != State::Playing {
                    self.set_state(State::Playing);
                }
            }
            StreamNote::Paused => {
                if imp.state.get() == State::Playing {
                    self.set_state(State::Paused);
                }
            }
            StreamNote::Ended => {
                imp.stream_failures.set(0);
                self.handle_event(PlayerEvent::EndOfStream);
            }
            StreamNote::Unavailable => {
                // Several in a row: streaming itself fails; stop instead of
                // racing through the queue.
                let failures = imp.stream_failures.get() + 1;
                imp.stream_failures.set(failures);
                if failures >= STREAM_FAILURES_TO_STOP {
                    imp.stream_failures.set(0);
                    self.stop();
                    self.emit_by_name::<()>(
                        "message",
                        &[&gettext(
                            "Spotify streaming can't play songs right now; Gatefold stopped",
                        )],
                    );
                    return;
                }
                self.handle_event(PlayerEvent::Error("Spotify can't play this song".into()))
            }
        }
    }

    /// Covers of streamed songs were downloaded: the player and the queue
    /// show them.
    pub fn covers_arrived(&self) {
        if self.current_track().is_some_and(|t| t.is_stand_in()) {
            self.notify_current_track();
        }
        if !self.is_remote() {
            self.queue_changed();
        }
    }

    /// Streaming was turned off: a streamed song stops.
    pub fn stream_gone(&self) {
        if self.imp().streaming.replace(false) {
            self.set_state(State::Stopped);
            self.set_position(0);
        }
    }

    /// Where playback is, from GStreamer or the stream.
    fn engine_position(&self) -> Option<u64> {
        if self.imp().streaming.get() {
            Some(self.spotify().stream_position())
        } else {
            self.engine().and_then(playback::Player::position_ms)
        }
    }

    // Smart shuffle (§7) ------------------------------------------------

    /// Forgets the similarity data (new analysis results, a rescan).
    pub fn reload_similarity(&self) {
        self.imp().similarity.replace(None);
    }

    fn similarity(&self) -> std::rc::Rc<Vec<SongFeatures>> {
        if let Some(data) = self.imp().similarity.borrow().as_ref() {
            return data.clone();
        }
        let data = std::rc::Rc::new(
            self.library()
                .db()
                .and_then(|db| db.similarity_features().ok())
                .unwrap_or_default(),
        );
        self.imp().similarity.replace(Some(data.clone()));
        data
    }

    /// Songs not to suggest: queued ones and those played lately.
    fn mix_exclusions(&self) -> HashSet<i64> {
        let mut exclude: HashSet<i64> = self.queued_track_ids().into_iter().collect();
        let hours = i64::from(self.settings().int("mix-skip-recent-hours"));
        if hours > 0
            && let Some(played) = self
                .library()
                .db()
                .and_then(|db| db.played_since(crate::metadata::now() - hours * 3600).ok())
        {
            exclude.extend(played);
        }
        exclude
    }

    /// A song like `seed_id`, not in `exclude`.
    fn suggest_with(&self, seed_id: i64, exclude: &HashSet<i64>) -> Option<i64> {
        let settings = self.settings();
        let pool = self.similarity();
        let seed = pool.iter().find(|f| f.track_id == seed_id)?;
        let criteria = Criteria {
            genre: settings.boolean("mix-genre"),
            bpm: settings.boolean("mix-bpm"),
            artist: settings.boolean("mix-artist"),
            sound: settings.boolean("mix-sound"),
            bpm_tolerance: f64::from(settings.int("mix-bpm-tolerance")) / 100.0,
        };
        similar::suggest(seed, &pool, exclude, &criteria, &mut rand::rng())
    }

    /// A song like `seed_id` that isn't queued or recently played.
    fn suggest_after(&self, seed_id: i64) -> Option<i64> {
        self.suggest_with(seed_id, &self.mix_exclusions())
    }

    /// Mixes suggestions into the upcoming queue, one after every few
    /// album songs (or takes them out when mixing is off). The caller
    /// updates the queue (queue_changed).
    fn mix_upcoming(&self) {
        let imp = self.imp();
        imp.queue.borrow_mut().remove_upcoming_suggestions();
        let settings = self.settings();
        if !settings.boolean("mix-similar") {
            return;
        }
        let every = settings.int("mix-every").max(1) as usize;
        let mut exclude = self.mix_exclusions();
        let added = imp.queue.borrow_mut().mix_throughout(every, |seed| {
            let id = self.suggest_with(seed, &exclude)?;
            exclude.insert(id);
            Some(id)
        });
        tracing::debug!("Mixed {added} suggestions into the queue");
    }

    /// Mix settings changed: redo the suggestions in the queue.
    fn remix(&self) {
        self.mix_upcoming();
        self.queue_changed();
    }

    /// "Keep playing similar music when the queue ends": nothing after the
    /// current song? Add a suggestion, so gapless playback has a next one.
    /// True if one was added (and the queue updated).
    fn keep_playing(&self) -> bool {
        let imp = self.imp();
        if !self.settings().boolean("keep-playing-similar") {
            return false;
        }
        let current = {
            let queue = imp.queue.borrow();
            if queue.peek_next(true).is_some() {
                return false;
            }
            queue.current().map(|i| i.track_id)
        };
        let Some(current) = current else { return false };
        let Some(id) = self.suggest_after(current) else {
            return false;
        };
        tracing::debug!("Keep playing: track {id} after {current}");
        imp.queue.borrow_mut().append_suggestion(id);
        self.queue_changed();
        true
    }

    /// Hands the following track to GStreamer for gapless playback.
    fn update_next(&self) {
        let Some(engine) = self.engine() else { return };
        // A suggestion was added: queue_changed() already handed it over.
        if self.keep_playing() {
            return;
        }
        let next = {
            let queue = self.imp().queue.borrow();
            queue.peek_next(true).map(|i| queue.items()[i])
        };
        let next = next.and_then(|item| self.track(item.track_id).map(|t| (t, item.uid)));
        // While a song streams, the stream learns what follows: a Spotify
        // song (taken early) or none (a local one).
        if self.imp().streaming.get() {
            let next_uri = next
                .as_ref()
                .filter(|(track, _)| track.is_stand_in())
                .and_then(|(track, _)| self.stream_uri(track));
            self.spotify().stream_next(next_uri.as_deref());
        }
        // A streamed song next: local playback ends with EOS and the song
        // is loaded then.
        if let Some((track, _)) = &next
            && track.is_stand_in()
        {
            engine.set_next(None);
            return;
        }
        let next = next
            .and_then(|(track, uid)| Some((playback::file_uri(&track.track().record.path)?, uid)));
        engine.set_next(next);
    }

    fn handle_event(&self, event: PlayerEvent) {
        let imp = self.imp();
        if let Some(engine) = self.engine() {
            engine.handle_event(&event);
        }
        match event {
            PlayerEvent::NextQueued(uid) => imp.pending_next.set(Some(uid)),
            PlayerEvent::StreamStarted => {
                if let Some(uid) = imp.pending_next.take() {
                    let found = imp.queue.borrow_mut().jump_to_uid(uid);
                    if found {
                        let current = imp.queue.borrow().current().copied();
                        self.set_current_track(current.and_then(|i| self.track(i.track_id)));
                        self.set_position(0);
                        self.emit_by_name::<()>("seeked", &[]);
                        self.update_next();
                        self.queue_changed();
                    }
                }
            }
            PlayerEvent::EndOfStream => {
                // No gapless hand-over happened (see playback.rs): play the
                // next track now, if there is one.
                let next = imp.queue.borrow_mut().advance(true).is_some();
                if next {
                    self.load_current(true, 0);
                    self.queue_changed();
                    return;
                }
                // The queue ran out: rewind to its start, ready to play again.
                if let Some(engine) = self.engine() {
                    engine.stop();
                }
                self.set_state(State::Stopped);
                imp.queue.borrow_mut().jump(0);
                let current = imp.queue.borrow().current().copied();
                self.set_current_track(current.and_then(|i| self.track(i.track_id)));
                self.set_position(0);
                self.queue_changed();
            }
            PlayerEvent::Error(message) => {
                tracing::warn!("Playback error: {message}");
                let title = self.current_track().map(|t| t.title()).unwrap_or_default();
                // Translators: {} is a song title.
                let text = gettext("“{}” can't be played").replace("{}", &title);
                self.emit_by_name::<()>("message", &[&text]);
                let advanced = imp.queue.borrow_mut().advance(false).is_some();
                if advanced && imp.queue.borrow().current_index() != Some(0) {
                    self.load_current(true, 0);
                    self.queue_changed();
                } else {
                    self.stop();
                }
            }
            PlayerEvent::AsyncDone => {}
        }
    }

    fn apply_replaygain(&self) {
        let Some(engine) = self.engine() else { return };
        let mode = self.settings().string("replaygain");
        engine.set_replaygain(mode != "off");
        let album = match mode.as_str() {
            "album" => true,
            "track" => false,
            // Album gain while an album plays in order, track gain in shuffle.
            _ => !self.imp().queue.borrow().is_shuffled(),
        };
        engine.set_album_gain(album);
    }

    fn set_current_track(&self, track: Option<TrackObject>) {
        let imp = self.imp();
        let changed = imp.current_track.borrow().as_ref().map(TrackObject::id)
            != track.as_ref().map(TrackObject::id);
        let duration = track.as_ref().map_or(0, TrackObject::duration_ms);
        let previous = imp.current_track.replace(track);
        // Leaving a song that really played: it goes into the history.
        if changed && let Some(previous) = previous {
            let played = imp.position.get();
            if played > 0 && played >= (previous.duration_ms() / 2).min(HISTORY_MIN_MS) {
                self.record_history(previous.id());
            }
        }
        if imp.duration.replace(duration) != duration {
            self.notify_duration();
        }
        if changed {
            self.notify_current_track();
        }
    }

    fn set_state(&self, state: State) {
        let imp = self.imp();
        imp.state.set(state);
        let playing = state == State::Playing;
        if imp.playing.replace(playing) != playing {
            self.notify_playing();
        }
        if playing && imp.tick.borrow().is_none() {
            let source = glib::timeout_add_local(
                TICK,
                glib::clone!(
                    #[weak(rename_to = player)]
                    self,
                    #[upgrade_or]
                    glib::ControlFlow::Break,
                    move || {
                        if let Some(ms) = player.engine_position() {
                            player.set_position(ms);
                        }
                        glib::ControlFlow::Continue
                    }
                ),
            );
            imp.tick.replace(Some(source));
        } else if !playing && let Some(source) = imp.tick.take() {
            source.remove();
        }
        self.update_actions();
        self.schedule_save();
    }

    fn set_position(&self, ms: u64) {
        if self.imp().position.replace(ms) != ms {
            self.notify_position();
        }
    }

    fn queue_changed(&self) {
        let imp = self.imp();
        let (shuffle, repeat, upcoming) = {
            let queue = imp.queue.borrow();
            let start = queue.current_index().unwrap_or(0);
            let items: Vec<_> = queue
                .items()
                .iter()
                .skip(start)
                .map(|i| (i.track_id, i.origin))
                .collect();
            (queue.is_shuffled(), queue.repeat().as_str(), items)
        };
        let objects: Vec<QueueEntry> = upcoming
            .iter()
            .enumerate()
            .filter_map(|(offset, &(id, origin))| {
                Some(QueueEntry::new(offset, self.track(id)?, origin))
            })
            .collect();
        let model = self.queue_model();
        model.splice(0, model.n_items(), &objects);

        // In remote mode, shuffle and repeat show the device's.
        if !self.is_remote() {
            // set_state(), not change_action_state() (inside): the latter
            // would call back into set_shuffle().
            self.show_shuffle_repeat(shuffle, RepeatMode::from_str_lossy(repeat));
        }
        self.update_next();
        self.update_actions();
        self.emit_by_name::<()>("queue-changed", &[]);
        self.schedule_save();
    }

    // Persistence (§6.2) -----------------------------------------------------

    /// Restores the saved queue, paused at the saved position.
    pub fn restore(&self) {
        if !self.settings().boolean("restore-queue") {
            return;
        }
        let library = self.library().clone();
        let snapshot = match library.db().map(|db| db.load_queue()) {
            Some(Ok(Some(snapshot))) => snapshot,
            Some(Ok(None)) | None => return,
            Some(Err(err)) => {
                tracing::warn!("Cannot load the saved queue: {err}");
                return;
            }
        };
        let ids: Vec<i64> = snapshot.items.iter().map(|i| i.track_id).collect();
        let tracks = library.tracks(&ids);
        // Streamed songs have no file; they wait for streaming.
        let (queue, skipped) = Queue::restore(&snapshot, |id| {
            tracks
                .get(&id)
                .is_some_and(|t| t.record.id < 0 || t.record.path.is_file())
        });
        self.cache_tracks(tracks.into_values().map(TrackObject::new).collect());
        self.imp().queue.replace(queue);
        if skipped > 0 {
            // Translators: {} is a number of songs.
            let text = gettext("{} songs in the queue are no longer in the library")
                .replace("{}", &skipped.to_string());
            self.emit_by_name::<()>("message", &[&text]);
        }
        if self.imp().queue.borrow().current().is_some() {
            self.load_current(false, snapshot.position_ms);
        }
        self.queue_changed();
    }

    fn schedule_save(&self) {
        let imp = self.imp();
        if imp.save.borrow().is_some() {
            return;
        }
        let source = glib::timeout_add_local_once(
            SAVE_DELAY,
            glib::clone!(
                #[weak(rename_to = player)]
                self,
                move || {
                    player.imp().save.replace(None);
                    player.save();
                }
            ),
        );
        imp.save.replace(Some(source));
    }

    /// Writes the queue to the database now.
    pub fn save(&self) {
        if let Some(source) = self.imp().save.take() {
            source.remove();
        }
        let live = self.imp().state.get() != State::Stopped && !self.is_remote();
        let position = live
            .then(|| self.engine_position())
            .flatten()
            .unwrap_or_else(|| self.position());
        let snapshot = self.imp().queue.borrow().snapshot(position);
        if let Some(mut db) = self.library().db()
            && let Err(err) = db.save_queue(&snapshot)
        {
            tracing::warn!("Cannot save the queue: {err}");
        }
    }
}
