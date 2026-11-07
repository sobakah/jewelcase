//! Streaming with Spotify Soloist (CONCEPT.md §9.4): Spotify's official
//! headless client plays Spotify songs that aren't in the library.
//!
//! Gatefold uses a Soloist that already runs (kept by soloistd, which also
//! downloads and updates it), or starts one itself with the user's API key.
//! Soloist plays the audio itself; Gatefold tells it what to play and
//! follows its reports.

use std::rc::Rc;

use gatefold_core::queue::RepeatMode;
use gatefold_soloist::{Config, Event, Item, Options, Position, Repeat, Soloist, Status};
use gettextrs::gettext;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::remote::RemoteCommand;
use super::stream::StreamNote;
use super::{Spotify, keyring};
use crate::application::GatefoldApplication;
use crate::config::APP_ID;

/// The device name of a Soloist Gatefold starts.
pub const DEVICE_NAME: &str = "Gatefold";
/// Quiet reconnects to a running Soloist, and the wait before each.
const RECONNECTS: u32 = 5;
const RECONNECT_DELAY_S: u32 = 3;
/// A song that hasn't started this long after asking is unavailable.
const START_TIMEOUT_S: u32 = 10;

#[derive(Default)]
pub struct SoloistState {
    soloist: Option<Rc<Soloist>>,
    /// "gatefold" (started here), "soloistd" or "soloist" (already running).
    pub(super) source: Option<&'static str>,
    /// Its name in Spotify's device lists (hidden from Play On…).
    pub(super) device_name: Option<String>,
    logged_in: bool,
    /// The active Spotify Connect device (else it's made so before playing:
    /// after a restart, or when the phone played meanwhile).
    is_active: bool,
    told_pairing: bool,
    /// What Gatefold asked to play, and whether it's playing now.
    wanted: Option<String>,
    started: bool,
    /// Loaded without playing (a restored queue): played on "play".
    pending: Option<(String, u64)>,
    seek_after_start: Option<u64>,
    position: Option<Position>,
    /// Counts starts; events of an older Soloist are ignored.
    generation: u64,
    /// Shuffle and repeat were reset for this Soloist.
    prepared: bool,
    /// Quiet attempts to reach a running Soloist again (soloistd restarts
    /// it after a settings change or an update).
    reconnects: u32,
    /// What Soloist plays, as it reports it.
    current: Option<Item>,
    status: Option<Status>,
    /// Soloist's shuffle and repeat (shown in receiver mode).
    options: Options,
    /// A repeat mode just chosen: Soloist sets it with two commands and
    /// reports the state in between ("off"), which isn't shown.
    pending_repeat: Option<(Repeat, std::time::Instant)>,
    upcoming: Vec<Item>,
    /// Showing what Soloist plays for someone else (started from the
    /// Spotify app, or a followed playlist): the receiver (§9.4).
    receiving: bool,
    /// Soloist moving on right after Gatefold paused it isn't someone
    /// else playing.
    quiet_until: Option<std::time::Instant>,
    /// Gatefold's next song when it's a Spotify song too: handed to
    /// Soloist's queue shortly before the end, so it follows seamlessly.
    next_uri: Option<String>,
    queued_next: Option<String>,
    /// Songs handed to Soloist's queue that Gatefold didn't need after all
    /// (Next pressed, the queue changed): they're still in Soloist's queue
    /// (it can't be emptied), so when Soloist gets to one, Gatefold's song
    /// is over.
    stale_queue: Vec<String>,
    /// Soloist moved on to the handed-over song: no new "play" for it.
    chained: Option<String>,
    /// Muted for the last moment of a song followed by a local one, so
    /// Soloist's own next song isn't heard before it's paused.
    ducked: bool,
    /// When Gatefold last paused Soloist: a pause it didn't ask for at the
    /// very end is the song ending.
    pause_asked: Option<std::time::Instant>,
    /// Counts reschedules of the end timers; an older timer does nothing
    /// (removing one that already ran would panic).
    end_generation: u64,
    /// Gatefold's volume (0 to 1), before the Spotify offset.
    volume: f64,
}

/// Before the end, the next Spotify song goes into Soloist's queue …
const HAND_OVER_BEFORE_MS: u64 = 10_000;
/// … or, with a local song next, Soloist is muted.
const DUCK_BEFORE_MS: u64 = 400;
/// Soloist pausing this close to the end (Spotify with autoplay off stops
/// on the song) is the song ending.
const PAUSED_AT_END_MS: u64 = 2_500;
/// A pause Gatefold asked for is reported within this.
const PAUSE_ANSWER: std::time::Duration = std::time::Duration::from_secs(3);

impl SoloistState {
    /// Soloist's reported shuffle and repeat; while a chosen repeat mode
    /// is on its way, the steps in between are skipped.
    fn take_options(&mut self, mut options: Options) {
        if let Some((wanted, since)) = self.pending_repeat {
            if options.repeat == wanted || since.elapsed() > REPEAT_SETTLE {
                self.pending_repeat = None;
            } else {
                options.repeat = wanted;
            }
        }
        self.options = options;
    }
}

/// How long a chosen repeat mode wins over Soloist's reports.
const REPEAT_SETTLE: std::time::Duration = std::time::Duration::from_secs(2);

/// How long after pausing Soloist its own moves are ignored.
const QUIET: std::time::Duration = std::time::Duration::from_secs(3);

impl Spotify {
    fn soloist(&self) -> Option<Rc<Soloist>> {
        self.imp().stream.borrow().soloist.soloist.clone()
    }

    /// Uses a running Soloist, or starts one.
    pub(super) fn soloist_start(&self) {
        self.soloist_stop();
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = spotify)]
            self,
            async move { spotify.soloist_connect().await }
        ));
    }

    async fn soloist_connect(&self) {
        self.set_stream_status("connecting");
        // A running one: soloistd's or the user's own, or one Gatefold
        // started before and that's still there.
        let own_dir = glib::user_data_dir().join(APP_ID).join("soloist");
        let running = gtk::gio::spawn_blocking(move || {
            gatefold_soloist::find_running().or_else(|| {
                let (addr, port) = gatefold_soloist::runtime_address(&own_dir)?;
                std::net::TcpStream::connect((addr.as_str(), port)).ok()?;
                Some(gatefold_soloist::Running {
                    source: "gatefold",
                    addr,
                    port,
                })
            })
        })
        .await
        .ok()
        .flatten();
        let (soloist, events, source) = match running {
            Some(running) => {
                tracing::info!(
                    "Using the running Soloist ({}) at {}:{}",
                    running.source,
                    running.addr,
                    running.port
                );
                let (soloist, events) = Soloist::attach(&running.addr, running.port);
                (soloist, events, running.source)
            }
            None => {
                let configured = self.settings().string("soloist-path");
                let configured = (!configured.trim().is_empty())
                    .then(|| std::path::PathBuf::from(configured.trim()));
                let Some(binary) = gatefold_soloist::find_binary(configured.as_deref()) else {
                    self.set_stream_status("missing");
                    return;
                };
                // Development aid: headless tests have no keyring.
                let test_key = std::env::var("GATEFOLD_SOLOIST_API_KEY")
                    .ok()
                    .filter(|_| cfg!(debug_assertions));
                let key = match test_key
                    .map(Some)
                    .map(Ok)
                    .unwrap_or(keyring::load_soloist_key().await)
                {
                    Ok(Some(key)) if !key.trim().is_empty() => key.trim().to_owned(),
                    Ok(_) => {
                        self.set_stream_status("needs-key");
                        return;
                    }
                    Err(err) => {
                        tracing::warn!("Cannot read the Soloist API key: {err}");
                        self.set_stream_status("needs-key");
                        return;
                    }
                };
                let config = Config {
                    binary,
                    api_key: key,
                    device_name: DEVICE_NAME.into(),
                    data_dir: glib::user_data_dir().join(APP_ID).join("soloist"),
                    cache_dir: glib::user_cache_dir().join(APP_ID).join("soloist"),
                    initial_volume: (GatefoldApplication::get().player().volume() * 100.0).round()
                        as u8,
                };
                match Soloist::start(&config) {
                    Ok((soloist, events)) => (soloist, events, "gatefold"),
                    Err(err) => {
                        tracing::warn!("{err}");
                        self.set_stream_status("error");
                        return;
                    }
                }
            }
        };
        let generation = {
            let mut stream = self.imp().stream.borrow_mut();
            let state = &mut stream.soloist;
            state.generation += 1;
            state.soloist = Some(Rc::new(soloist));
            state.source = Some(source);
            state.prepared = false;
            state.generation
        };
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = spotify)]
            self,
            async move {
                while let Ok(event) = events.recv().await {
                    if spotify.imp().stream.borrow().soloist.generation != generation {
                        break;
                    }
                    spotify.soloist_event(event);
                }
            }
        ));
    }

    /// Gatefold quits: a Soloist it started stops (and waits for it); a
    /// running one (soloistd's) gets its volume back and stops Gatefold's
    /// song, so nothing plays on uncontrolled.
    pub fn shutdown_streaming(&self) {
        let (old, ours, ducked, volume) = {
            let mut stream = self.imp().stream.borrow_mut();
            let state = &mut stream.soloist;
            (
                state.soloist.take(),
                state.wanted.is_some() && !state.receiving,
                state.ducked,
                state.volume,
            )
        };
        let Some(old) = old else { return };
        if old.is_attached() {
            if ducked {
                old.set_volume(self.soloist_percent(volume));
            }
            if ours {
                old.pause();
            }
        }
        if let Ok(old) = Rc::try_unwrap(old) {
            // Sends what's queued (the pause) before the process ends.
            old.close(std::time::Duration::from_millis(500));
        }
    }

    /// Stops using Soloist (one Gatefold started is stopped too). The
    /// receiver ends and a streamed song stops, so nothing waits for a
    /// Soloist that's gone.
    pub(super) fn soloist_stop(&self) {
        if self.soloist_receiving() {
            self.end_receiver(false);
        }
        let (ducked, volume) = {
            let state = &self.imp().stream.borrow().soloist;
            (state.ducked, state.volume)
        };
        let old = {
            let mut stream = self.imp().stream.borrow_mut();
            let state = &mut stream.soloist;
            state.generation += 1;
            let old = state.soloist.take();
            let (generation, reconnects, end_generation) =
                (state.generation, state.reconnects, state.end_generation);
            *state = SoloistState {
                generation,
                reconnects,
                end_generation,
                ..SoloistState::default()
            };
            old
        };
        if let Some(old) = old {
            if old.is_attached() {
                if ducked {
                    old.set_volume(self.soloist_percent(volume));
                }
                old.pause();
            }
            // Stopping a started one waits a moment: off the main thread.
            if let Ok(old) = Rc::try_unwrap(old) {
                std::thread::spawn(move || drop(old));
            }
        }
        GatefoldApplication::get().player().stream_gone();
    }

    fn soloist_event(&self, event: Event) {
        match event {
            Event::Connected => {}
            Event::Auth {
                logged_in,
                is_active,
                device_name,
            } => {
                let first_pairing = {
                    let mut stream = self.imp().stream.borrow_mut();
                    let state = &mut stream.soloist;
                    state.logged_in = logged_in;
                    state.is_active = is_active;
                    state.device_name = device_name.clone();
                    !logged_in && !std::mem::replace(&mut state.told_pairing, true)
                };
                if logged_in {
                    self.imp().stream.borrow_mut().soloist.reconnects = 0;
                    self.set_stream_status("ready");
                } else {
                    self.set_stream_status("pairing");
                    if first_pairing {
                        // Translators: {} is a device name.
                        self.message(
                            &gettext(
                                "To link Soloist, choose “{}” in the Spotify app’s device list",
                            )
                            .replace("{}", device_name.as_deref().unwrap_or(DEVICE_NAME)),
                        );
                    }
                }
            }
            Event::Device { is_active } => {
                self.imp().stream.borrow_mut().soloist.is_active = is_active;
            }
            Event::Options(options) => {
                self.imp().stream.borrow_mut().soloist.take_options(options);
                if self.soloist_receiving() {
                    self.receiver_show(false);
                }
            }
            Event::Playback {
                status,
                item,
                position,
                is_active,
                options,
                ..
            } => {
                {
                    let mut stream = self.imp().stream.borrow_mut();
                    let state = &mut stream.soloist;
                    if let Some(options) = options {
                        state.take_options(options);
                    }
                    if let Some(is_active) = is_active {
                        state.is_active = is_active;
                    }
                    if position.is_some() {
                        state.position = position;
                    }
                    state.current = item.clone();
                    state.status = status;
                }
                if let Some(item) = item {
                    self.soloist_item(&item.uri);
                }
                if let Some(status) = status {
                    self.soloist_status(status);
                }
                self.receiver_check();
            }
            Event::TrackChanged(item) => {
                self.imp().stream.borrow_mut().soloist.current = item.clone();
                if let Some(item) = item {
                    self.soloist_item(&item.uri);
                }
                self.receiver_check();
            }
            Event::Status(status) => {
                self.imp().stream.borrow_mut().soloist.status = Some(status);
                self.soloist_status(status);
                self.receiver_check();
            }
            Event::Position(position) => {
                self.imp().stream.borrow_mut().soloist.position = Some(position);
                if self.soloist_receiving() {
                    self.receiver_show(false);
                } else {
                    // A seek (or a correction) moves the end.
                    self.schedule_end();
                }
            }
            Event::Queue(items) => {
                self.imp().stream.borrow_mut().soloist.upcoming = items;
                if self.soloist_receiving() {
                    self.receiver_show(true);
                }
            }
            Event::Volume(_) => {}
            Event::CommandError(message) => {
                tracing::warn!("Soloist: {message}");
                let waiting = {
                    let state = &self.imp().stream.borrow().soloist;
                    state.wanted.is_some() && !state.started
                };
                if waiting {
                    self.soloist_unavailable();
                }
            }
            Event::Exited { code, expired } => {
                tracing::warn!("Soloist exited ({code:?})");
                if self.soloist_receiving() {
                    self.end_receiver(false);
                }
                self.soloist_gone(if expired { "expired" } else { "error" });
                self.message(&if expired {
                    gettext(
                        "This Soloist build has expired; download the new one from Spotify (soloistd updates it by itself)",
                    )
                } else {
                    gettext("Soloist stopped")
                });
            }
            Event::Disconnected => {
                if self.soloist().is_none() {
                    return;
                }
                if self.soloist_receiving() {
                    self.end_receiver(false);
                }
                // A running Soloist (soloistd's) restarts now and then: try
                // again quietly a few times before saying so.
                let retry = {
                    let mut stream = self.imp().stream.borrow_mut();
                    let state = &mut stream.soloist;
                    let attached = state.source != Some("gatefold");
                    let retry = attached && state.reconnects < RECONNECTS;
                    if retry {
                        state.reconnects += 1;
                    }
                    retry
                };
                if retry {
                    GatefoldApplication::get().player().stream_gone();
                    self.soloist_stop();
                    self.set_stream_status("connecting");
                    glib::timeout_add_seconds_local_once(
                        RECONNECT_DELAY_S,
                        glib::clone!(
                            #[weak(rename_to = spotify)]
                            self,
                            move || spotify.start_streaming()
                        ),
                    );
                    return;
                }
                self.imp().stream.borrow_mut().soloist.reconnects = 0;
                self.soloist_gone("error");
                self.message(&gettext("Soloist can't be reached"));
            }
        }
    }

    fn soloist_gone(&self, status: &str) {
        let mut stream = self.imp().stream.borrow_mut();
        stream.soloist.soloist = None;
        stream.soloist.wanted = None;
        stream.soloist.started = false;
        drop(stream);
        self.set_stream_status(status);
        GatefoldApplication::get().player().stream_gone();
    }

    /// Soloist plays `uri`: Gatefold's song started, or Soloist moved on
    /// (the song ended and it continued, or someone skipped in the app).
    fn soloist_item(&self, uri: &str) {
        let (wanted, started) = {
            let state = &self.imp().stream.borrow().soloist;
            (state.wanted.clone(), state.started)
        };
        if wanted.as_deref() == Some(uri) {
            if !started {
                let seek = {
                    let mut stream = self.imp().stream.borrow_mut();
                    stream.soloist.started = true;
                    stream.soloist.seek_after_start.take()
                };
                if let (Some(ms), Some(soloist)) = (seek, self.soloist()) {
                    soloist.seek(ms);
                }
                self.schedule_end();
            } else if self.take_stale(uri) {
                // The same song again, from what was left in Soloist's
                // queue: Gatefold's song is over.
                self.soloist_ended();
            }
        } else if started {
            let (chained, expected) = {
                let state = &self.imp().stream.borrow().soloist;
                (
                    state.queued_next.as_deref() == Some(uri),
                    state.next_uri.as_deref() == Some(uri),
                )
            };
            if chained && !expected {
                // Handed over, but Gatefold's queue changed meanwhile.
                self.imp().stream.borrow_mut().soloist.queued_next = None;
                self.soloist_ended();
            } else if chained {
                // The handed-over song plays: it's Gatefold's next one.
                {
                    let mut stream = self.imp().stream.borrow_mut();
                    let state = &mut stream.soloist;
                    state.queued_next = None;
                    state.chained = Some(uri.to_owned());
                    state.wanted = Some(uri.to_owned());
                    state.started = true;
                    state.position = None;
                }
                GatefoldApplication::get()
                    .player()
                    .stream_note(StreamNote::Ended);
            } else if self.take_stale(uri) || GatefoldApplication::get().player().playing() {
                // Soloist moved on while Gatefold played (the song ended,
                // or someone skipped in the app): Gatefold's next song.
                self.soloist_ended();
            } else {
                // Gatefold's song was paused and someone else plays now
                // (cast from the phone): theirs is shown, not stopped.
                self.soloist_release();
            }
        }
    }

    /// Whether `uri` was left in Soloist's queue (and now isn't).
    fn take_stale(&self, uri: &str) -> bool {
        let mut stream = self.imp().stream.borrow_mut();
        let stale = &mut stream.soloist.stale_queue;
        match stale.iter().position(|u| u == uri) {
            Some(index) => {
                stale.remove(index);
                true
            }
            None => false,
        }
    }

    /// A song handed to Soloist's queue that Gatefold won't play next.
    fn forget_queued(state: &mut SoloistState) {
        if let Some(uri) = state.queued_next.take() {
            state.stale_queue.push(uri);
            let excess = state.stale_queue.len().saturating_sub(5);
            state.stale_queue.drain(..excess);
        }
    }

    /// Gatefold lets go of its song (someone else plays now): its player
    /// stops the stream, Soloist plays on and the receiver shows it.
    fn soloist_release(&self) {
        {
            let mut stream = self.imp().stream.borrow_mut();
            let state = &mut stream.soloist;
            state.wanted = None;
            state.started = false;
            state.end_generation += 1;
            Self::forget_queued(state);
        }
        self.restore_volume();
        GatefoldApplication::get().player().stream_gone();
    }

    fn soloist_status(&self, status: Status) {
        let (playing_ours, started) = {
            let state = &self.imp().stream.borrow().soloist;
            (state.wanted.is_some() && state.started, state.started)
        };
        if !playing_ours {
            return;
        }
        let player = GatefoldApplication::get().player();
        match status {
            Status::Playing => {
                // Muted for the end, then paused and played again: heard
                // again (the end timers mute it once more if needed).
                self.restore_volume();
                player.stream_note(StreamNote::Playing);
                self.schedule_end();
            }
            Status::Paused if self.soloist_paused_at_end() => {
                tracing::debug!("Soloist paused at the end of the song: next one");
                self.soloist_ended();
            }
            Status::Paused => {
                player.stream_note(StreamNote::Paused);
                // The end timers wait while paused.
                self.schedule_end();
            }
            Status::Idle if started => self.soloist_ended(),
            _ => {}
        }
    }

    /// Soloist paused by itself at the end of Gatefold's song (nothing in
    /// its queue and no autoplay): not a pause, the song is over.
    fn soloist_paused_at_end(&self) -> bool {
        let state = &self.imp().stream.borrow().soloist;
        if state
            .pause_asked
            .is_some_and(|at| at.elapsed() < PAUSE_ANSWER)
        {
            return false;
        }
        if state.ducked {
            return true;
        }
        let duration = state
            .current
            .as_ref()
            .filter(|c| state.wanted.as_deref() == Some(c.uri.as_str()))
            .and_then(|c| c.duration_ms)
            .unwrap_or_else(|| GatefoldApplication::get().player().duration());
        let position = state.position.map_or(0, |p| p.now());
        duration > 0 && position + PAUSED_AT_END_MS >= duration
    }

    /// Gatefold's song is over: Soloist is paused before it plays anything
    /// else, and the queue moves on.
    fn soloist_ended(&self) {
        {
            let mut stream = self.imp().stream.borrow_mut();
            stream.soloist.wanted = None;
            stream.soloist.started = false;
            stream.soloist.quiet_until = Some(std::time::Instant::now() + QUIET);
            Self::forget_queued(&mut stream.soloist);
        }
        if let Some(soloist) = self.soloist() {
            soloist.pause();
        }
        self.restore_volume();
        GatefoldApplication::get()
            .player()
            .stream_note(StreamNote::Ended);
    }

    /// Gatefold's next song: a Spotify song (handed over before the end)
    /// or not (Soloist is muted at the very end).
    pub(super) fn soloist_next(&self, uri: Option<&str>) {
        self.imp().stream.borrow_mut().soloist.next_uri = uri.map(str::to_owned);
        self.schedule_end();
    }

    /// Timers for the end of the current song (see `soloist_next`).
    fn schedule_end(&self) {
        let (generation, wanted, remaining) = {
            let mut stream = self.imp().stream.borrow_mut();
            let state = &mut stream.soloist;
            state.end_generation += 1;
            // Wall-clock timers: only while the song plays (a pause
            // reschedules them on resume).
            if !state.started || state.receiving || state.status != Some(Status::Playing) {
                return;
            }
            let Some(wanted) = state.wanted.clone() else {
                return;
            };
            let duration = state
                .current
                .as_ref()
                .filter(|c| c.uri == wanted)
                .and_then(|c| c.duration_ms)
                .unwrap_or_else(|| GatefoldApplication::get().player().duration());
            let position = state.position.map_or(0, |p| p.now());
            if duration == 0 || position >= duration {
                return;
            }
            (state.end_generation, wanted, duration - position)
        };
        let still = move |spotify: &Spotify| {
            let state = &spotify.imp().stream.borrow().soloist;
            state.end_generation == generation && state.wanted.as_deref() == Some(wanted.as_str())
        };
        glib::timeout_add_local_once(
            std::time::Duration::from_millis(remaining.saturating_sub(HAND_OVER_BEFORE_MS)),
            glib::clone!(
                #[weak(rename_to = spotify)]
                self,
                #[strong]
                still,
                move || {
                    if !still(&spotify) {
                        return;
                    }
                    let next = {
                        let state = &spotify.imp().stream.borrow().soloist;
                        // Not the song itself (Repeat One, the same song
                        // twice): Soloist would play it without an end.
                        state.next_uri.clone().filter(|next| {
                            state.queued_next.is_none() && Some(next) != state.wanted.as_ref()
                        })
                    };
                    if let (Some(next), Some(soloist)) = (next, spotify.soloist()) {
                        soloist.add_to_queue(&next);
                        spotify.imp().stream.borrow_mut().soloist.queued_next = Some(next);
                    }
                }
            ),
        );
        glib::timeout_add_local_once(
            std::time::Duration::from_millis(remaining.saturating_sub(DUCK_BEFORE_MS)),
            glib::clone!(
                #[weak(rename_to = spotify)]
                self,
                move || {
                    let local_next = spotify.imp().stream.borrow().soloist.next_uri.is_none();
                    if still(&spotify)
                        && local_next
                        && let Some(soloist) = spotify.soloist()
                    {
                        soloist.set_volume(0);
                        spotify.imp().stream.borrow_mut().soloist.ducked = true;
                    }
                }
            ),
        );
    }

    fn restore_volume(&self) {
        let ducked = std::mem::take(&mut self.imp().stream.borrow_mut().soloist.ducked);
        if ducked {
            let volume = self.imp().stream.borrow().soloist.volume;
            self.soloist_volume(volume);
        }
    }

    /// Soloist's volume: Gatefold's, lowered by the offset for Spotify's
    /// louder songs (Preferences, default −4 dB).
    fn soloist_percent(&self, volume: f64) -> u8 {
        let offset_db = f64::from(self.settings().int("spotify-volume-offset"));
        let factor = 10f64.powf(offset_db / 20.0);
        (volume.clamp(0.0, 1.0) * factor * 100.0)
            .round()
            .clamp(0.0, 100.0) as u8
    }

    fn soloist_unavailable(&self) {
        {
            let mut stream = self.imp().stream.borrow_mut();
            stream.soloist.wanted = None;
            stream.soloist.started = false;
        }
        GatefoldApplication::get()
            .player()
            .stream_note(StreamNote::Unavailable);
    }

    pub(super) fn soloist_load(&self, uri: &str, start_ms: u64, play: bool) {
        // Soloist already moved on to it (handed over before the end).
        let chained = {
            let mut stream = self.imp().stream.borrow_mut();
            let chained = stream.soloist.chained.take();
            play && chained.as_deref() == Some(uri)
        };
        if chained {
            self.restore_volume();
            self.schedule_end();
            return;
        }
        Self::forget_queued(&mut self.imp().stream.borrow_mut().soloist);
        self.restore_volume();
        let Some(soloist) = self.soloist() else {
            GatefoldApplication::get()
                .player()
                .stream_note(StreamNote::Unavailable);
            return;
        };
        if !play {
            let mut stream = self.imp().stream.borrow_mut();
            stream.soloist.pending = Some((uri.to_owned(), start_ms));
            stream.soloist.wanted = None;
            stream.soloist.started = false;
            return;
        }
        let (prepare, generation, active) = {
            let mut stream = self.imp().stream.borrow_mut();
            let state = &mut stream.soloist;
            let active = std::mem::replace(&mut state.is_active, true);
            state.pending = None;
            state.wanted = Some(uri.to_owned());
            state.started = false;
            state.seek_after_start = (start_ms > 0).then_some(start_ms);
            state.position = None;
            (
                !std::mem::replace(&mut state.prepared, true),
                state.generation,
                active,
            )
        };
        // Linked once, but maybe not the active device right now.
        if !active {
            soloist.activate();
        }
        // Gatefold's queue decides what comes next, not Soloist's.
        if prepare {
            soloist.set_shuffle(false);
            soloist.set_repeat(Repeat::Off);
        }
        soloist.play(Some(uri));
        // No start in time: Spotify can't play it.
        let uri = uri.to_owned();
        glib::timeout_add_seconds_local_once(
            START_TIMEOUT_S,
            glib::clone!(
                #[weak(rename_to = spotify)]
                self,
                move || {
                    let waiting = {
                        let state = &spotify.imp().stream.borrow().soloist;
                        state.generation == generation
                            && state.wanted.as_deref() == Some(uri.as_str())
                            && !state.started
                    };
                    if waiting {
                        tracing::warn!("Soloist didn't start {uri}");
                        spotify.soloist_unavailable();
                    }
                }
            ),
        );
    }

    pub(super) fn soloist_play(&self) {
        let pending = self.imp().stream.borrow_mut().soloist.pending.take();
        match pending {
            Some((uri, start)) => self.soloist_load(&uri, start, true),
            None => {
                if let Some(soloist) = self.soloist() {
                    soloist.play(None);
                }
            }
        }
    }

    pub(super) fn soloist_pause(&self) {
        self.imp().stream.borrow_mut().soloist.pause_asked = Some(std::time::Instant::now());
        if let Some(soloist) = self.soloist() {
            soloist.pause();
        }
    }

    /// Gatefold plays something else: Soloist pauses.
    pub(super) fn soloist_stop_playback(&self) {
        self.restore_volume();
        let was_ours = {
            let mut stream = self.imp().stream.borrow_mut();
            let state = &mut stream.soloist;
            state.pending = None;
            state.started = false;
            state.end_generation += 1;
            state.quiet_until = Some(std::time::Instant::now() + QUIET);
            Self::forget_queued(state);
            state.wanted.take().is_some()
        };
        if was_ours && let Some(soloist) = self.soloist() {
            soloist.pause();
        }
    }

    pub(super) fn soloist_seek(&self, ms: u64) {
        // Back from the muted last moment: heard again.
        self.restore_volume();
        if let Some(soloist) = self.soloist() {
            soloist.seek(ms);
        }
        let mut stream = self.imp().stream.borrow_mut();
        let speed = stream.soloist.position.map_or(1.0, |p| p.speed);
        stream.soloist.position = Some(Position {
            position_ms: ms,
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64),
            speed,
        });
    }

    pub(super) fn soloist_volume(&self, volume: f64) {
        self.imp().stream.borrow_mut().soloist.volume = volume;
        if let Some(soloist) = self.soloist() {
            soloist.set_volume(self.soloist_percent(volume));
        }
    }

    pub(super) fn soloist_position(&self) -> u64 {
        let state = &self.imp().stream.borrow().soloist;
        if state.started {
            state.position.map_or(0, |p| p.now())
        } else {
            0
        }
    }

    /// Soloist's own name in device lists, so Play On… doesn't offer it.
    pub fn soloist_device_name(&self) -> Option<String> {
        let state = &self.imp().stream.borrow().soloist;
        state.soloist.as_ref()?;
        state
            .device_name
            .clone()
            .or_else(|| (state.source == Some("gatefold")).then(|| DEVICE_NAME.to_owned()))
    }

    // The receiver (§9.4) ---------------------------------------------------

    pub fn soloist_receiving(&self) -> bool {
        self.imp().stream.borrow().soloist.receiving
    }

    /// Soloist plays something Gatefold didn't start: show it, and let the
    /// controls steer it.
    fn receiver_check(&self) {
        if self.soloist_receiving() {
            self.receiver_show(false);
            return;
        }
        let start = {
            let state = &self.imp().stream.borrow().soloist;
            state.logged_in
                && state.wanted.is_none()
                && state.status == Some(Status::Playing)
                && state.current.is_some()
                && state
                    .quiet_until
                    .is_none_or(|until| std::time::Instant::now() >= until)
        };
        if start {
            self.start_receiver();
        }
    }

    fn start_receiver(&self) {
        let player = GatefoldApplication::get().player();
        if player.is_remote() {
            // Already steering a Spotify device; that one is in charge.
            return;
        }
        let name = {
            let mut stream = self.imp().stream.borrow_mut();
            stream.soloist.receiving = true;
            stream
                .soloist
                .device_name
                .clone()
                .unwrap_or_else(|| DEVICE_NAME.to_owned())
        };
        tracing::info!("Soloist plays for someone else: showing it");
        let weak = self.downgrade();
        player.enter_remote(&name, move |command| {
            if let Some(spotify) = weak.upgrade() {
                spotify.receiver_command(command);
            }
        });
        player.set_remote_receiver(true);
        if let Some(soloist) = self.soloist() {
            soloist.request_queue(20);
        }
        self.receiver_show(true);
    }

    /// Back to Gatefold's own playback; `pause` stops Soloist.
    pub(super) fn end_receiver(&self, pause: bool) {
        {
            let mut stream = self.imp().stream.borrow_mut();
            stream.soloist.receiving = false;
            stream.soloist.quiet_until = Some(std::time::Instant::now() + QUIET);
        }
        if pause && let Some(soloist) = self.soloist() {
            soloist.pause();
        }
        GatefoldApplication::get().player().leave_remote();
    }

    /// Shows Soloist's song (as the library song when it matches), its
    /// position and, with `queue`, what comes next.
    pub(super) fn receiver_show(&self, queue: bool) {
        let (current, status, position, upcoming, options) = {
            let state = &self.imp().stream.borrow().soloist;
            (
                state.current.clone(),
                state.status,
                state.position,
                state.upcoming.clone(),
                state.options,
            )
        };
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = spotify)]
            self,
            async move {
                let track = match &current {
                    Some(item) => spotify.track_objects(&[as_track(item)]).await.pop(),
                    None => None,
                };
                let player = GatefoldApplication::get().player();
                if !spotify.soloist_receiving() || !player.is_remote() {
                    return;
                }
                player.show_remote(
                    track.clone(),
                    status == Some(Status::Playing),
                    position.map_or(0, |p| p.now()),
                    options.shuffle,
                    match options.repeat {
                        Repeat::Off => RepeatMode::Off,
                        Repeat::Context => RepeatMode::All,
                        Repeat::Track => RepeatMode::One,
                    },
                );
                if queue {
                    let next: Vec<gatefold_spotify::Track> =
                        upcoming.iter().map(as_track).collect();
                    let mut tracks: Vec<_> = track.into_iter().collect();
                    tracks.extend(spotify.track_objects(&next).await);
                    if spotify.soloist_receiving() {
                        player.set_remote_queue(tracks);
                    }
                }
            }
        ));
    }

    fn receiver_command(&self, command: RemoteCommand) {
        let Some(soloist) = self.soloist() else {
            return;
        };
        let playing = self.imp().stream.borrow().soloist.status == Some(Status::Playing);
        match command {
            RemoteCommand::PlayPause if playing => soloist.pause(),
            RemoteCommand::PlayPause => soloist.play(None),
            RemoteCommand::Pause => soloist.pause(),
            RemoteCommand::Next => soloist.skip_next(),
            RemoteCommand::Previous => soloist.skip_previous(),
            RemoteCommand::Seek(ms) => soloist.seek(ms),
            RemoteCommand::Volume(volume) => {
                soloist.set_volume((volume.clamp(0.0, 1.0) * 100.0).round() as u8);
            }
            // Shown right away; Soloist confirms with options_changed.
            RemoteCommand::Shuffle(on) => {
                soloist.set_shuffle(on);
                self.imp().stream.borrow_mut().soloist.options.shuffle = on;
                self.receiver_show(false);
            }
            RemoteCommand::Repeat(mode) => {
                let repeat = match mode {
                    RepeatMode::Off => Repeat::Off,
                    RepeatMode::All => Repeat::Context,
                    RepeatMode::One => Repeat::Track,
                };
                soloist.set_repeat(repeat);
                {
                    let mut stream = self.imp().stream.borrow_mut();
                    stream.soloist.options.repeat = repeat;
                    stream.soloist.pending_repeat = Some((repeat, std::time::Instant::now()));
                }
                self.receiver_show(false);
            }
            RemoteCommand::Queue(ids) => {
                glib::spawn_future_local(glib::clone!(
                    #[weak(rename_to = spotify)]
                    self,
                    async move {
                        let mut added = 0u32;
                        for id in ids {
                            let uri = match spotify.find_version(id).await {
                                Ok(gatefold_core::db::SpotifyVersion {
                                    uri: Some(uri), ..
                                }) => uri,
                                Ok(_) => continue,
                                // About this song's query: the next one.
                                // Else it holds for the other songs too
                                // (see resolve_entries).
                                Err(err) if err.is_about_request() => continue,
                                Err(err) => {
                                    spotify.message(&super::search_error(&err));
                                    break;
                                }
                            };
                            if let Some(soloist) = spotify.soloist() {
                                soloist.add_to_queue(&uri);
                                added += 1;
                            }
                        }
                        if added > 0 {
                            spotify.message(
                                &gettextrs::ngettext(
                                    "Added {} song to the queue",
                                    "Added {} songs to the queue",
                                    added,
                                )
                                .replace("{}", &added.to_string()),
                            );
                        }
                        if let Some(soloist) = spotify.soloist() {
                            soloist.request_queue(20);
                        }
                    }
                ));
            }
            // Gatefold plays its own queue: the receiver ends.
            RemoteCommand::Play(_) => {
                glib::idle_add_local_once(glib::clone!(
                    #[weak(rename_to = spotify)]
                    self,
                    move || {
                        spotify.end_receiver(true);
                        GatefoldApplication::get().player().play_current();
                    }
                ));
            }
        }
    }

    /// Plays a Spotify playlist, album or song in Soloist (e.g. a followed
    /// playlist whose songs Spotify doesn't list); the receiver shows it.
    pub fn soloist_play_context(&self, uri: &str) -> bool {
        let Some(soloist) = self.soloist() else {
            return false;
        };
        // Gatefold's own Spotify song gives way; the receiver shows this.
        if self.imp().stream.borrow().soloist.wanted.is_some() {
            self.soloist_release();
        }
        let active = {
            let mut stream = self.imp().stream.borrow_mut();
            stream.soloist.quiet_until = None;
            std::mem::replace(&mut stream.soloist.is_active, true)
        };
        if !active {
            soloist.activate();
        }
        soloist.play(Some(uri));
        true
    }

    /// Where the Soloist in use comes from (for Preferences).
    pub fn soloist_source(&self) -> Option<&'static str> {
        self.imp().stream.borrow().soloist.source
    }
}

/// Soloist's song in the shape remote mode shows.
fn as_track(item: &Item) -> gatefold_spotify::Track {
    gatefold_spotify::Track {
        uri: item.uri.clone(),
        id: item.uri.strip_prefix("spotify:track:").map(str::to_owned),
        name: item.name.clone(),
        artists: item.artists.clone(),
        album: item.album.clone(),
        duration_ms: item.duration_ms.unwrap_or(0),
        image_url: item.cover_url.clone(),
        is_playable: true,
        ..gatefold_spotify::Track::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chosen_repeat_mode_skips_the_step_in_between() {
        let mut state = SoloistState::default();
        let all = Options {
            shuffle: false,
            repeat: Repeat::Context,
        };
        state.take_options(all);
        // Context → track: Soloist reports "off" in between.
        state.pending_repeat = Some((Repeat::Track, std::time::Instant::now()));
        state.take_options(Options {
            repeat: Repeat::Off,
            ..all
        });
        assert_eq!(state.options.repeat, Repeat::Track);
        state.take_options(Options {
            repeat: Repeat::Track,
            ..all
        });
        assert_eq!(state.options.repeat, Repeat::Track);
        assert!(state.pending_repeat.is_none());
        // A wish that never came true stops counting.
        state.pending_repeat = Some((
            Repeat::Context,
            std::time::Instant::now() - REPEAT_SETTLE * 2,
        ));
        state.take_options(Options {
            repeat: Repeat::Off,
            ..all
        });
        assert_eq!(state.options.repeat, Repeat::Off);
    }
}
