//! Playback for the whole app: the queue, the GStreamer engine and the
//! `player.*` actions (CONCEPT.md §6, §7).

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use adw::prelude::*;
use gatefold_core::playback::{self, PlayerEvent};
use gatefold_core::queue::{Queue, RepeatMode};
use gettextrs::gettext;
use gtk::glib::subclass::Signal;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};

use crate::config::APP_ID;
use crate::library::Library;
use crate::objects::{QueueEntry, TrackObject};

/// How often the position is updated while playing (lyrics follow it).
const TICK: Duration = Duration::from_millis(150);
/// The queue is saved this long after its last change (§6.2).
const SAVE_DELAY: Duration = Duration::from_secs(2);
/// "Previous" restarts the track instead when it has played this long.
const RESTART_THRESHOLD_MS: u64 = 3000;

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

        pub engine: OnceCell<playback::Player>,
        pub library: OnceCell<Library>,
        pub settings: OnceCell<gio::Settings>,
        pub actions: gio::SimpleActionGroup,
        pub queue: RefCell<Queue>,
        /// Queue entries from the current one on, for the queue view.
        pub queue_model: OnceCell<gio::ListStore>,
        pub tracks: RefCell<HashMap<i64, TrackObject>>,
        pub state: Cell<State>,
        /// Queue item handed to GStreamer for gapless playback.
        pub pending_next: Cell<Option<u64>>,
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

    fn settings(&self) -> &gio::Settings {
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
        self.imp().queue.borrow_mut().play_context(
            &ids,
            start,
            Some(album_id),
            shuffle,
            &mut rand::rng(),
        );
        self.load_current(true, 0);
        self.queue_changed();
    }

    /// Plays a list of tracks (an artist's albums), in order or shuffled.
    pub fn play_tracks(&self, tracks: Vec<TrackObject>, shuffle: bool) {
        if tracks.is_empty() {
            return;
        }
        let ids: Vec<i64> = tracks.iter().map(TrackObject::id).collect();
        self.cache_tracks(tracks);
        let start = (!shuffle).then_some(0);
        self.imp()
            .queue
            .borrow_mut()
            .play_context(&ids, start, None, shuffle, &mut rand::rng());
        self.load_current(true, 0);
        self.queue_changed();
    }

    /// Inserts tracks after the current one.
    pub fn play_next(&self, tracks: Vec<TrackObject>) {
        let ids: Vec<i64> = tracks.iter().map(TrackObject::id).collect();
        let was_empty = self.imp().queue.borrow().current().is_none();
        self.cache_tracks(tracks);
        self.imp().queue.borrow_mut().play_next(&ids);
        self.after_adding(was_empty);
    }

    /// Adds tracks to the end of "Next up".
    pub fn add_to_queue(&self, tracks: Vec<TrackObject>) {
        let ids: Vec<i64> = tracks.iter().map(TrackObject::id).collect();
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
        let album = self.imp().queue.borrow().context_album()?;
        self.library()
            .db()?
            .album(album)
            .ok()
            .flatten()
            .map(|a| a.title)
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
        self.set_state(State::Stopped);
        self.set_position(0);
    }

    pub fn next(&self) {
        let advanced = self.imp().queue.borrow_mut().advance(false).is_some();
        if advanced {
            self.load_current(self.imp().state.get() == State::Playing, 0);
            self.queue_changed();
        }
    }

    pub fn previous(&self) {
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
        if let Some(engine) = self.engine()
            && self.imp().state.get() != State::Stopped
        {
            engine.seek(ms);
        }
        self.set_position(ms);
        self.emit_by_name::<()>("seeked", &[]);
        self.schedule_save();
    }

    pub fn set_shuffle(&self, shuffle: bool) {
        self.imp()
            .queue
            .borrow_mut()
            .set_shuffle(shuffle, &mut rand::rng());
        self.queue_changed();
        self.apply_replaygain();
    }

    pub fn set_repeat(&self, repeat: RepeatMode) {
        self.imp().queue.borrow_mut().set_repeat(repeat);
        self.queue_changed();
    }

    fn cycle_repeat(&self) {
        let repeat = self.imp().queue.borrow().repeat().cycle();
        self.set_repeat(repeat);
    }

    pub fn volume(&self) -> f64 {
        self.engine().map_or(1.0, playback::Player::volume)
    }

    pub fn set_volume(&self, volume: f64) {
        if let Some(engine) = self.engine() {
            engine.set_volume(volume);
        }
        let _ = self.settings().set_double("volume", volume.clamp(0.0, 1.0));
    }

    // Internals --------------------------------------------------------------

    /// Loads the queue's current track into GStreamer.
    fn load_current(&self, play: bool, start_ms: u64) {
        let imp = self.imp();
        imp.pending_next.set(None);
        let current = imp.queue.borrow().current().copied();
        let track = current.and_then(|item| self.track(item.track_id));
        self.set_current_track(track.clone());

        let Some(track) = track else {
            self.stop();
            return;
        };
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

    /// Hands the following track to GStreamer for gapless playback.
    fn update_next(&self) {
        let Some(engine) = self.engine() else { return };
        let next = {
            let queue = self.imp().queue.borrow();
            queue.peek_next(true).map(|i| queue.items()[i])
        };
        let next = next.and_then(|item| {
            let track = self.track(item.track_id)?;
            Some((playback::file_uri(&track.track().record.path)?, item.uid))
        });
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
        imp.current_track.replace(track);
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
                        if let Some(ms) = player.engine().and_then(playback::Player::position_ms) {
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

        if imp.shuffle.replace(shuffle) != shuffle {
            self.notify_shuffle();
        }
        // set_state(), not change_action_state(): the latter would call
        // back into set_shuffle().
        if let Some(action) = imp
            .actions
            .lookup_action("shuffle")
            .and_downcast::<gio::SimpleAction>()
        {
            action.set_state(&shuffle.to_variant());
        }
        if *imp.repeat.borrow() != repeat {
            imp.repeat.replace(repeat.to_owned());
            self.notify_repeat();
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
        let (queue, skipped) = Queue::restore(&snapshot, |id| {
            tracks.get(&id).is_some_and(|t| t.record.path.is_file())
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
        let position = self
            .engine()
            .filter(|_| self.imp().state.get() != State::Stopped)
            .and_then(playback::Player::position_ms)
            .unwrap_or_else(|| self.position());
        let snapshot = self.imp().queue.borrow().snapshot(position);
        if let Some(mut db) = self.library().db()
            && let Err(err) = db.save_queue(&snapshot)
        {
            tracing::warn!("Cannot save the queue: {err}");
        }
    }
}
