//! Streaming with Spotify Soloist (CONCEPT.md §9.4): Spotify's official
//! headless client plays Spotify songs that aren't in the library.
//!
//! Gatefold uses a Soloist that already runs (kept by soloistd, which also
//! downloads and updates it), or starts one itself with the user's API key.
//! Soloist plays the audio itself; Gatefold tells it what to play and
//! follows its reports.

use std::rc::Rc;

use gatefold_core::queue::RepeatMode;
use gatefold_soloist::{Config, Event, Item, Position, Repeat, Soloist, Status};
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
    upcoming: Vec<Item>,
    /// Showing what Soloist plays for someone else (started from the
    /// Spotify app, or a followed playlist): the receiver (§9.4).
    receiving: bool,
    /// Soloist moving on right after Gatefold paused it isn't someone
    /// else playing.
    quiet_until: Option<std::time::Instant>,
}

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

    /// Gatefold quits: a Soloist it started stops (and waits for it).
    pub fn shutdown_streaming(&self) {
        let old = self.imp().stream.borrow_mut().soloist.soloist.take();
        if let Some(old) = old
            && let Ok(old) = Rc::try_unwrap(old)
        {
            old.shutdown();
        }
    }

    /// Stops using Soloist (one Gatefold started is stopped too).
    pub(super) fn soloist_stop(&self) {
        let old = {
            let mut stream = self.imp().stream.borrow_mut();
            let state = &mut stream.soloist;
            state.generation += 1;
            let old = state.soloist.take();
            let (generation, reconnects) = (state.generation, state.reconnects);
            *state = SoloistState {
                generation,
                reconnects,
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
            Event::Playback {
                status,
                item,
                position,
                is_active,
                ..
            } => {
                {
                    let mut stream = self.imp().stream.borrow_mut();
                    let state = &mut stream.soloist;
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
            stream.soloist.quiet_until = Some(std::time::Instant::now() + QUIET);
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
            state.quiet_until = Some(std::time::Instant::now() + QUIET);
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
    fn receiver_show(&self, queue: bool) {
        let (current, status, position, upcoming) = {
            let state = &self.imp().stream.borrow().soloist;
            (
                state.current.clone(),
                state.status,
                state.position,
                state.upcoming.clone(),
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
                    false,
                    RepeatMode::Off,
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
            RemoteCommand::Shuffle(on) => soloist.set_shuffle(on),
            RemoteCommand::Repeat(mode) => soloist.set_repeat(match mode {
                RepeatMode::Off => Repeat::Off,
                RepeatMode::All => Repeat::Context,
                RepeatMode::One => Repeat::Track,
            }),
            RemoteCommand::Queue(ids) => {
                glib::spawn_future_local(glib::clone!(
                    #[weak(rename_to = spotify)]
                    self,
                    async move {
                        let mut added = 0u32;
                        for id in ids {
                            if let Ok(gatefold_core::db::SpotifyVersion { uri: Some(uri), .. }) =
                                spotify.find_version(id).await
                                && let Some(soloist) = spotify.soloist()
                            {
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
