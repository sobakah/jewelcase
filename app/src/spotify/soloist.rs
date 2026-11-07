//! Streaming with Spotify Soloist (CONCEPT.md §9.4): Spotify's official
//! headless client plays Spotify songs that aren't in the library.
//!
//! Gatefold uses a Soloist that already runs (kept by soloistd, which also
//! downloads and updates it), or starts one itself with the user's API key.
//! Soloist plays the audio itself; Gatefold tells it what to play and
//! follows its reports.

use std::rc::Rc;

use gatefold_soloist::{Config, Event, Position, Repeat, Soloist, Status};
use gettextrs::gettext;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::stream::StreamNote;
use super::{Spotify, keyring};
use crate::application::GatefoldApplication;
use crate::config::APP_ID;

/// The device name of a Soloist Gatefold starts.
pub const DEVICE_NAME: &str = "Gatefold";
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
}

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
        // A running one (soloistd's, or the user's own).
        let running = gtk::gio::spawn_blocking(gatefold_soloist::find_running)
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

    /// Stops using Soloist (one Gatefold started is stopped too).
    pub(super) fn soloist_stop(&self) {
        let old = {
            let mut stream = self.imp().stream.borrow_mut();
            let state = &mut stream.soloist;
            state.generation += 1;
            let old = state.soloist.take();
            let generation = state.generation;
            *state = SoloistState {
                generation,
                ..SoloistState::default()
            };
            old
        };
        if let Some(old) = old {
            if old.is_attached() {
                old.pause();
            }
            // Stopping a started one waits a moment: off the main thread.
            if let Ok(old) = Rc::try_unwrap(old) {
                std::thread::spawn(move || drop(old));
            }
        }
    }

    fn soloist_event(&self, event: Event) {
        match event {
            Event::Connected => {}
            Event::Auth {
                logged_in,
                device_name,
                ..
            } => {
                let first_pairing = {
                    let mut stream = self.imp().stream.borrow_mut();
                    let state = &mut stream.soloist;
                    state.logged_in = logged_in;
                    state.device_name = device_name.clone();
                    !logged_in && !std::mem::replace(&mut state.told_pairing, true)
                };
                if logged_in {
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
            Event::Playback {
                status,
                item,
                position,
                ..
            } => {
                if let Some(position) = position {
                    self.imp().stream.borrow_mut().soloist.position = Some(position);
                }
                if let Some(item) = item {
                    self.soloist_item(&item.uri);
                }
                if let Some(status) = status {
                    self.soloist_status(status);
                }
            }
            Event::TrackChanged(item) => {
                if let Some(item) = item {
                    self.soloist_item(&item.uri);
                }
            }
            Event::Status(status) => self.soloist_status(status),
            Event::Position(position) => {
                self.imp().stream.borrow_mut().soloist.position = Some(position);
            }
            Event::Volume(_) | Event::Queue(_) => {}
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
                if self.soloist().is_some() {
                    self.soloist_gone("error");
                    self.message(&gettext("Soloist can't be reached"));
                }
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
            }
        } else if started {
            self.soloist_ended();
        }
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
            Status::Playing => player.stream_note(StreamNote::Playing),
            Status::Paused => player.stream_note(StreamNote::Paused),
            Status::Idle if started => self.soloist_ended(),
            _ => {}
        }
    }

    /// Gatefold's song is over: Soloist is paused before it plays anything
    /// else, and the queue moves on.
    fn soloist_ended(&self) {
        {
            let mut stream = self.imp().stream.borrow_mut();
            stream.soloist.wanted = None;
            stream.soloist.started = false;
        }
        if let Some(soloist) = self.soloist() {
            soloist.pause();
        }
        GatefoldApplication::get()
            .player()
            .stream_note(StreamNote::Ended);
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
        let (prepare, generation) = {
            let mut stream = self.imp().stream.borrow_mut();
            let state = &mut stream.soloist;
            state.pending = None;
            state.wanted = Some(uri.to_owned());
            state.started = false;
            state.seek_after_start = (start_ms > 0).then_some(start_ms);
            state.position = None;
            (
                !std::mem::replace(&mut state.prepared, true),
                state.generation,
            )
        };
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
        if let Some(soloist) = self.soloist() {
            soloist.pause();
        }
    }

    /// Gatefold plays something else: Soloist pauses.
    pub(super) fn soloist_stop_playback(&self) {
        let was_ours = {
            let mut stream = self.imp().stream.borrow_mut();
            let state = &mut stream.soloist;
            state.pending = None;
            state.started = false;
            state.wanted.take().is_some()
        };
        if was_ours && let Some(soloist) = self.soloist() {
            soloist.pause();
        }
    }

    pub(super) fn soloist_seek(&self, ms: u64) {
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
        if let Some(soloist) = self.soloist() {
            soloist.set_volume((volume.clamp(0.0, 1.0) * 100.0).round() as u8);
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

    /// Where the Soloist in use comes from (for Preferences).
    pub fn soloist_source(&self) -> Option<&'static str> {
        self.imp().stream.borrow().soloist.source
    }
}
