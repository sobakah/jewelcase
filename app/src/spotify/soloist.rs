//! Streaming with Spotify Soloist (CONCEPT.md §9.4): Spotify's official
//! headless client plays Spotify songs that aren't in the library.
//!
//! Jewelcase uses a Soloist that already runs (kept by soloistd, which also
//! downloads and updates it), or starts one itself with the user's API key.
//! Soloist plays the audio itself; Jewelcase tells it what to play and
//! follows its reports.

use std::rc::Rc;

use gettextrs::gettext;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use jewelcase_core::queue::RepeatMode;
use jewelcase_soloist::{Config, Event, Item, Options, Position, Repeat, Soloist, Status};

use super::remote::RemoteCommand;
use super::stream::StreamNote;
use super::{Spotify, keyring};
use crate::application::JewelcaseApplication;
use crate::config::APP_ID;

/// The device name of a Soloist Jewelcase starts; development builds
/// (the test Flatpak) get their own, next to the installed app's.
pub fn own_device_name() -> &'static str {
    if crate::config::is_development() {
        "Jewelcase Devel"
    } else {
        "Jewelcase"
    }
}
/// Quiet reconnects to a running Soloist, and the wait before each.
/// Lets go of a Soloist off the main thread: stopping one Jewelcase
/// started waits for it to end (up to 3 s).
fn drop_soloist(soloist: Rc<Soloist>) {
    if let Ok(soloist) = Rc::try_unwrap(soloist) {
        std::thread::spawn(move || drop(soloist));
    }
}

const RECONNECTS: u32 = 5;
const RECONNECT_DELAY_S: u32 = 3;
/// A song that hasn't started this long after asking is unavailable.
const START_TIMEOUT_S: u32 = 10;
/// After a started Soloist's connection closed, its exit is awaited this
/// long (it says whether the build expired).
const EXIT_WAIT_S: u32 = 3;

#[derive(Default)]
pub struct SoloistState {
    soloist: Option<Rc<Soloist>>,
    /// "jewelcase" (started here), "soloistd" or "soloist" (already running).
    pub(super) source: Option<&'static str>,
    /// Its name in Spotify's device lists (hidden from Play On…).
    pub(super) device_name: Option<String>,
    logged_in: bool,
    /// The active Spotify Connect device (else it's made so before playing:
    /// after a restart, or when the phone played meanwhile).
    is_active: bool,
    told_pairing: bool,
    /// What Jewelcase asked to play, and whether it's playing now.
    wanted: Option<String>,
    started: bool,
    /// Counts Jewelcase's requests to play, so a start timeout belongs to
    /// one request (the same song asked for again gets its own time).
    loads: u64,
    /// When Jewelcase's song started (Soloist reports a start more than
    /// once).
    started_at: Option<std::time::Instant>,
    /// Jewelcase's songs that ended right after starting, in a row: a
    /// loop, stopped (`RUNAWAY_ENDS`).
    quick_ends: u32,
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
    /// Soloist moving on right after Jewelcase paused it isn't someone
    /// else playing.
    quiet_until: Option<std::time::Instant>,
    /// Jewelcase's next song when it's a Spotify song too: handed to
    /// Soloist's queue as soon as the current one plays, so it follows
    /// seamlessly and Next in the Spotify app skips to it.
    next_uri: Option<String>,
    queued_next: Option<String>,
    /// Songs handed to Soloist's queue that Jewelcase didn't need after all
    /// (Next pressed, the queue changed): they're still in Soloist's queue
    /// (it can't be emptied), so when Soloist gets to one, Jewelcase's song
    /// is over.
    stale_queue: Vec<String>,
    /// Left-overs that just left Soloist's queue report: when one starts
    /// right after (the queue report can come first), it's still known.
    pruned: Vec<(String, std::time::Instant)>,
    /// Soloist moved on to the handed-over song: no new "play" for it.
    chained: Option<String>,
    /// Muted for the last moment of a song followed by a local one, so
    /// Soloist's own next song isn't heard before it's paused.
    ducked: bool,
    /// When Jewelcase last paused Soloist: a pause it didn't ask for at the
    /// very end is the song ending.
    pause_asked: Option<std::time::Instant>,
    /// Counts reschedules of the end timers; an older timer does nothing
    /// (removing one that already ran would panic).
    end_generation: u64,
    /// Jewelcase's volume (0 to 1), before the Spotify offset.
    volume: f64,
    /// Jewelcase's last seek in its playing song: when, and to where (ms).
    last_seek: Option<(std::time::Instant, u64)>,
}

/// With a local song next, Soloist is muted this long before the end.
const DUCK_BEFORE_MS: u64 = 400;
/// Soloist pausing this close to the end (Spotify with autoplay off stops
/// on the song) is the song ending.
const PAUSED_AT_END_MS: u64 = 2_500;
/// Soloist changing songs with more than this left of Jewelcase's song was
/// someone choosing in the Spotify app, not the song ending.
const CHOSEN_ELSEWHERE_MS: u64 = 15_000;
/// A left-over that left Soloist's queue report is still recognised
/// this long (the report can come before the song starts).
const PRUNED_KEPT: std::time::Duration = std::time::Duration::from_secs(10);
/// Jewelcase's song ending this soon after it started is suspicious ...
const QUICK_END: std::time::Duration = std::time::Duration::from_secs(2);
/// ... and this many in a row are a loop: Jewelcase stops.
const RUNAWAY_ENDS: u32 = 4;
/// A pause Jewelcase asked for is reported within this.
const PAUSE_ANSWER: std::time::Duration = std::time::Duration::from_secs(3);
/// Soloist moving on this soon after a seek, long before the end, was the
/// seek going wrong (seen after Next), not the song ending.
const SEEK_SLIP: std::time::Duration = std::time::Duration::from_secs(3);

/// Soloist changed songs with `left` ms of Jewelcase's song left: if that
/// was right after Jewelcase's seek (`last_seek`) and long before the end,
/// where Jewelcase's song should go on (ms); else `None`.
fn seek_slip(
    last_seek: Option<(std::time::Instant, u64)>,
    left: Option<u64>,
    now: std::time::Instant,
) -> Option<u64> {
    let (at, ms) = last_seek?;
    let since = now.checked_duration_since(at)?;
    (since < SEEK_SLIP && left? > CHOSEN_ELSEWHERE_MS).then(|| ms + since.as_millis() as u64)
}

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
        // Turned off or started again while this waits (for the keyring,
        // say): this attempt stops, so only one Soloist starts.
        let attempt = self.imp().stream.borrow().soloist.generation;
        let superseded =
            |spotify: &Self| spotify.imp().stream.borrow().soloist.generation != attempt;
        // A running one: soloistd's or the user's own, or one Jewelcase
        // started before and that's still there.
        let own_dir = glib::user_data_dir().join(APP_ID).join("soloist");
        let running = gtk::gio::spawn_blocking(move || {
            jewelcase_soloist::find_running().or_else(|| {
                let (addr, port) = jewelcase_soloist::runtime_address(&own_dir)?;
                std::net::TcpStream::connect((addr.as_str(), port)).ok()?;
                Some(jewelcase_soloist::Running {
                    source: "jewelcase",
                    addr,
                    port,
                })
            })
        })
        .await
        .ok()
        .flatten();
        if superseded(self) {
            return;
        }
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
                let Some(binary) = jewelcase_soloist::find_binary(configured.as_deref()) else {
                    self.set_stream_status("missing");
                    return;
                };
                // Development aid: headless tests have no keyring.
                let test_key = std::env::var("JEWELCASE_SOLOIST_API_KEY")
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
                if superseded(self) {
                    return;
                }
                let config = Config {
                    binary,
                    api_key: key,
                    device_name: own_device_name().into(),
                    data_dir: glib::user_data_dir().join(APP_ID).join("soloist"),
                    cache_dir: glib::user_cache_dir().join(APP_ID).join("soloist"),
                    initial_volume: (JewelcaseApplication::get().player().volume() * 100.0).round()
                        as u8,
                };
                // Asks the program for its options first: not on the main
                // thread.
                let started = gtk::gio::spawn_blocking(move || Soloist::start(&config)).await;
                match started {
                    Ok(Ok((soloist, events))) => {
                        if superseded(self) {
                            drop_soloist(Rc::new(soloist));
                            return;
                        }
                        (soloist, events, "jewelcase")
                    }
                    Ok(Err(err)) => {
                        tracing::warn!("{err}");
                        self.set_stream_status("error");
                        return;
                    }
                    Err(_) => {
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

    /// Jewelcase quits: a Soloist it started stops (and waits for it); a
    /// running one (soloistd's) gets its volume back and stops Jewelcase's
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

    /// Stops using Soloist (one Jewelcase started is stopped too). The
    /// receiver ends and a streamed song stops, so nothing waits for a
    /// Soloist that's gone.
    pub(super) fn soloist_stop(&self) {
        if self.soloist_receiving() {
            self.end_receiver(false);
        }
        let (ducked, volume, ours) = {
            let state = &self.imp().stream.borrow().soloist;
            // Jewelcase's own song, not one the phone plays through it.
            (
                state.ducked,
                state.volume,
                state.wanted.is_some() && !state.receiving,
            )
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
                // Someone else's music (soloistd's Soloist, played from the
                // phone) goes on.
                if ours {
                    old.pause();
                }
            }
            drop_soloist(old);
        }
        // Nothing streams now: songs don't go to a Soloist that's gone.
        // (A restart sets "connecting" right after.)
        if self.stream_backend() == super::stream::Backend::Soloist {
            self.set_stream_status("off");
        }
        JewelcaseApplication::get().player().stream_gone();
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
                            .replace("{}", device_name.as_deref().unwrap_or(own_device_name())),
                        );
                    }
                }
            }
            Event::Device { is_active } => {
                self.imp().stream.borrow_mut().soloist.is_active = is_active;
                self.left_device(is_active);
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
                let left = self.ours_left();
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
                    self.soloist_item(&item.uri, left);
                }
                if let Some(status) = status {
                    self.soloist_status(status);
                }
                self.receiver_check();
                if is_active == Some(false) {
                    self.left_device(false);
                }
            }
            Event::TrackChanged(item) => {
                let left = self.ours_left();
                self.imp().stream.borrow_mut().soloist.current = item.clone();
                if let Some(item) = item {
                    self.soloist_item(&item.uri, left);
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
                let pruned = {
                    let mut stream = self.imp().stream.borrow_mut();
                    let state = &mut stream.soloist;
                    // Left-over songs Soloist doesn't have any more (played,
                    // or its queue was emptied) can't come back.
                    let (kept, gone): (Vec<String>, Vec<String>) = state
                        .stale_queue
                        .drain(..)
                        .partition(|uri| items.iter().any(|item| item.uri == *uri));
                    state.stale_queue = kept;
                    let now = std::time::Instant::now();
                    state
                        .pruned
                        .retain(|(_, at)| now.duration_since(*at) < PRUNED_KEPT);
                    state
                        .pruned
                        .extend(gone.iter().map(|uri| (uri.clone(), now)));
                    state.upcoming = items;
                    !gone.is_empty()
                };
                if self.soloist_receiving() {
                    self.receiver_show(true);
                } else if pruned {
                    // Nothing left in the way: the next song can be handed over.
                    self.schedule_end();
                }
            }
            Event::Volume(_) => {}
            Event::CommandError { command, message } => {
                tracing::warn!("Soloist refused {command:?}: {message}");
                // Only the song's own start failing means it can't play
                // (not shuffle or repeat sent before it).
                let waiting = {
                    let state = &self.imp().stream.borrow().soloist;
                    state.wanted.is_some()
                        && !state.started
                        && command.as_deref().is_none_or(|c| c == "play")
                };
                if waiting {
                    self.soloist_unavailable();
                }
            }
            Event::Exited { code, expired } => {
                tracing::warn!("Soloist exited ({code:?})");
                // Already reported (it stopped answering long before).
                if self.soloist().is_none() {
                    return;
                }
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
                // One Jewelcase started: its exit follows and says why
                // (expired or not); this says so only if it doesn't come.
                let (own, generation) = {
                    let state = &self.imp().stream.borrow().soloist;
                    (state.source == Some("jewelcase"), state.generation)
                };
                if own {
                    glib::timeout_add_seconds_local_once(
                        EXIT_WAIT_S,
                        glib::clone!(
                            #[weak(rename_to = spotify)]
                            self,
                            move || {
                                let still_there = {
                                    let state = &spotify.imp().stream.borrow().soloist;
                                    state.generation == generation && state.soloist.is_some()
                                };
                                if still_there {
                                    spotify.soloist_lost();
                                }
                            }
                        ),
                    );
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
                    let attached = state.source != Some("jewelcase");
                    let retry = attached && state.reconnects < RECONNECTS;
                    if retry {
                        state.reconnects += 1;
                    }
                    retry
                };
                if retry {
                    JewelcaseApplication::get().player().stream_gone();
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
                self.soloist_lost();
            }
        }
    }

    /// Soloist stopped answering for good.
    fn soloist_lost(&self) {
        if self.soloist_receiving() {
            self.end_receiver(false);
        }
        self.imp().stream.borrow_mut().soloist.reconnects = 0;
        self.soloist_gone("error");
        self.message(&gettext("Soloist can't be reached"));
    }

    /// Soloist is no longer the active device (the phone moved playback
    /// elsewhere): the receiver ends, its controls would steer nothing.
    fn left_device(&self, is_active: bool) {
        if !is_active && self.soloist_receiving() {
            tracing::info!("Soloist is no longer the active device; leaving the receiver");
            self.end_receiver(false);
        }
    }

    fn soloist_gone(&self, status: &str) {
        let mut stream = self.imp().stream.borrow_mut();
        let old = stream.soloist.soloist.take();
        stream.soloist.wanted = None;
        stream.soloist.started = false;
        drop(stream);
        if let Some(old) = old {
            drop_soloist(old);
        }
        self.set_stream_status(status);
        JewelcaseApplication::get().player().stream_gone();
    }

    /// Soloist plays `uri`: Jewelcase's song started, or Soloist moved on
    /// (the song ended and it continued, or someone skipped in the app).
    /// How much of Jewelcase's song was left (ms), while it plays.
    fn ours_left(&self) -> Option<u64> {
        let state = &self.imp().stream.borrow().soloist;
        let wanted = state.wanted.as_deref()?;
        if !state.started {
            return None;
        }
        let duration = state
            .current
            .as_ref()
            .filter(|c| c.uri == wanted)
            .and_then(|c| c.duration_ms)?;
        let position = state.position?.now();
        Some(duration.saturating_sub(position))
    }

    /// Soloist plays `uri`; `left`: how much of Jewelcase's song was left
    /// just before.
    fn soloist_item(&self, uri: &str, left: Option<u64>) {
        let (wanted, started) = {
            let state = &self.imp().stream.borrow().soloist;
            (state.wanted.clone(), state.started)
        };
        if wanted.as_deref() == Some(uri) {
            if !started {
                // Muted while skipping past a queued song: heard now.
                self.restore_volume();
                let seek = {
                    let mut stream = self.imp().stream.borrow_mut();
                    stream.soloist.started = true;
                    stream.soloist.started_at = Some(std::time::Instant::now());
                    stream.soloist.seek_after_start.take()
                };
                if let (Some(ms), Some(soloist)) = (seek, self.soloist()) {
                    soloist.seek(ms);
                }
                self.schedule_end();
            } else if self.started_a_while_ago() && self.take_stale(uri) {
                // The same song again, from what was left in Soloist's
                // queue: Jewelcase's song is over. (Not Soloist reporting
                // the start twice: the song was loaded while a copy of it
                // waited in the queue.)
                self.soloist_ended("the copy left in Soloist's queue plays");
            }
        } else if started
            && let Some(resume) = {
                let state = &self.imp().stream.borrow().soloist;
                seek_slip(state.last_seek, left, std::time::Instant::now())
            }
        {
            self.resume_after_slip(uri, resume);
        } else if started {
            let (chained, expected) = {
                let state = &self.imp().stream.borrow().soloist;
                (
                    state.queued_next.as_deref() == Some(uri),
                    state.next_uri.as_deref() == Some(uri),
                )
            };
            if chained && !expected {
                // Handed over, but Jewelcase's queue changed meanwhile.
                self.imp().stream.borrow_mut().soloist.queued_next = None;
                self.soloist_ended("the handed-over song, but Jewelcase's queue changed");
            } else if chained {
                // The handed-over song plays: it's Jewelcase's next one.
                tracing::info!(
                    "Soloist moved on to the handed-over {uri} ({} ms left)",
                    left.map_or_else(|| "?".to_owned(), |l| l.to_string())
                );
                {
                    let mut stream = self.imp().stream.borrow_mut();
                    let state = &mut stream.soloist;
                    state.queued_next = None;
                    state.chained = Some(uri.to_owned());
                    state.wanted = Some(uri.to_owned());
                    state.started = true;
                    state.started_at = Some(std::time::Instant::now());
                    // At its start until Soloist reports, so a song picked
                    // in the app meanwhile is seen as picked, not as this
                    // one ending.
                    state.position = Some(jewelcase_soloist::Position::at_start());
                }
                JewelcaseApplication::get()
                    .player()
                    .stream_note(StreamNote::Ended);
            } else if self.take_stale(uri) {
                // What was left in Soloist's queue: Jewelcase's song is over.
                self.soloist_ended("a song left in Soloist's queue plays");
            } else if JewelcaseApplication::get().player().playing()
                && left.is_some_and(|left| left > CHOSEN_ELSEWHERE_MS)
            {
                // Long before the end: someone picked a song or skipped in
                // the Spotify app. Theirs plays, and the app stays in
                // charge (the receiver shows it).
                tracing::info!("Another song was chosen in the Spotify app; it keeps playing");
                self.soloist_release();
            } else if JewelcaseApplication::get().player().playing() {
                // Soloist moved on at the end (its autoplay): Jewelcase's
                // next song.
                self.soloist_ended("Soloist moved on by itself at the end");
            } else {
                // Jewelcase's song was paused and someone else plays now
                // (cast from the phone): theirs is shown, not stopped.
                self.soloist_release();
            }
        }
    }

    /// Soloist moved on to `uri` right after Jewelcase's seek (it does so
    /// now and then in a song it plays from its queue, e.g. after Next):
    /// Jewelcase's song is started again where the seek went. A song taken
    /// from Soloist's queue is no longer there; it's handed over again.
    fn resume_after_slip(&self, uri: &str, resume_ms: u64) {
        let wanted = {
            let mut stream = self.imp().stream.borrow_mut();
            let state = &mut stream.soloist;
            if state.queued_next.as_deref() == Some(uri) {
                state.queued_next = None;
            }
            state.last_seek = None;
            state.started = false;
            state.end_generation += 1;
            state.seek_after_start = Some(resume_ms);
            state.position = None;
            state.wanted.clone()
        };
        let (Some(wanted), Some(soloist)) = (wanted, self.soloist()) else {
            return;
        };
        tracing::info!(
            "Soloist moved on to {uri} right after a seek; {wanted} goes on at {resume_ms} ms"
        );
        soloist.play(Some(&wanted));
        self.watch_start(&wanted);
    }

    fn started_a_while_ago(&self) -> bool {
        let state = &self.imp().stream.borrow().soloist;
        state.started_at.is_some_and(|at| at.elapsed() >= QUICK_END)
    }

    /// Whether `uri` was left in Soloist's queue (and now isn't).
    fn take_stale(&self, uri: &str) -> bool {
        let mut stream = self.imp().stream.borrow_mut();
        let state = &mut stream.soloist;
        if let Some(index) = state.stale_queue.iter().position(|u| u == uri) {
            state.stale_queue.remove(index);
            return true;
        }
        let now = std::time::Instant::now();
        match state
            .pruned
            .iter()
            .position(|(u, at)| u == uri && now.duration_since(*at) < PRUNED_KEPT)
        {
            Some(index) => {
                state.pruned.remove(index);
                true
            }
            None => false,
        }
    }

    /// Jewelcase leaves its song while its next one waits in Soloist's
    /// queue: Soloist skips to that one, muted, so it isn't left there
    /// (the queue can't be emptied otherwise; the song that follows is
    /// started right after). Without a song of Jewelcase's playing, it's
    /// a left-over.
    fn consume_queued(&self) {
        let queued = {
            let mut stream = self.imp().stream.borrow_mut();
            let state = &mut stream.soloist;
            if state.started && state.wanted.is_some() {
                state.queued_next.take()
            } else {
                Self::forget_queued(state);
                None
            }
        };
        let Some(uri) = queued else {
            return;
        };
        match self.soloist() {
            Some(soloist) => {
                tracing::info!("Soloist: skipping past {uri} in its queue");
                soloist.set_volume(0);
                self.imp().stream.borrow_mut().soloist.ducked = true;
                soloist.skip_next();
            }
            None => self.imp().stream.borrow_mut().soloist.stale_queue.push(uri),
        }
    }

    /// A song handed to Soloist's queue that Jewelcase won't play next.
    fn forget_queued(state: &mut SoloistState) {
        if let Some(uri) = state.queued_next.take() {
            state.stale_queue.push(uri);
            let excess = state.stale_queue.len().saturating_sub(5);
            state.stale_queue.drain(..excess);
        }
    }

    /// Jewelcase lets go of its song (someone else plays now): its player
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
        JewelcaseApplication::get().player().stream_gone();
    }

    fn soloist_status(&self, status: Status) {
        let (playing_ours, started) = {
            let state = &self.imp().stream.borrow().soloist;
            (state.wanted.is_some() && state.started, state.started)
        };
        if !playing_ours {
            return;
        }
        let player = JewelcaseApplication::get().player();
        match status {
            Status::Playing => {
                // Muted for the end, then paused and played again: heard
                // again (the end timers mute it once more if needed).
                self.restore_volume();
                player.stream_note(StreamNote::Playing);
                self.schedule_end();
            }
            Status::Paused if self.soloist_paused_at_end() => {
                self.soloist_ended("Soloist paused at the end");
            }
            Status::Paused => {
                player.stream_note(StreamNote::Paused);
                // The end timers wait while paused.
                self.schedule_end();
            }
            Status::Idle if started => self.soloist_ended("Soloist is idle"),
            _ => {}
        }
    }

    /// Soloist paused by itself at the end of Jewelcase's song (nothing in
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
            .unwrap_or_else(|| JewelcaseApplication::get().player().duration());
        let position = state.position.map_or(0, |p| p.now());
        duration > 0 && position + PAUSED_AT_END_MS >= duration
    }

    /// Jewelcase's song is over: Soloist is paused before it plays anything
    /// else, and the queue moves on.
    fn soloist_ended(&self, why: &str) {
        let uri = self.imp().stream.borrow().soloist.wanted.clone();
        tracing::info!(
            "Soloist: {} is over ({why})",
            uri.as_deref().unwrap_or("the song")
        );
        let runaway = {
            let mut stream = self.imp().stream.borrow_mut();
            let state = &mut stream.soloist;
            state.wanted = None;
            state.started = false;
            state.quiet_until = Some(std::time::Instant::now() + QUIET);
            Self::forget_queued(state);
            let quick = state
                .started_at
                .take()
                .is_some_and(|at| at.elapsed() < QUICK_END);
            state.quick_ends = if quick { state.quick_ends + 1 } else { 0 };
            let runaway = state.quick_ends >= RUNAWAY_ENDS;
            if runaway {
                state.quick_ends = 0;
            }
            runaway
        };
        if let Some(soloist) = self.soloist() {
            soloist.pause();
        }
        self.restore_volume();
        let player = JewelcaseApplication::get().player();
        if runaway {
            // Songs ending as soon as they start: something keeps Soloist
            // moving on. Stopped instead of racing through the queue.
            tracing::warn!("Soloist's songs keep ending right away; stopping");
            player.stop();
            self.message(&gettext(
                "Soloist kept skipping songs, so Jewelcase stopped. Play again to go on.",
            ));
            return;
        }
        player.stream_note(StreamNote::Ended);
    }

    /// Jewelcase's next song: a Spotify song (handed over while the
    /// current one plays) or not (Soloist is muted at the very end).
    pub(super) fn soloist_next(&self, uri: Option<&str>) {
        self.imp().stream.borrow_mut().soloist.next_uri = uri.map(str::to_owned);
        self.schedule_end();
    }

    /// Jewelcase's next Spotify song goes into Soloist's queue while the
    /// current one plays: it follows seamlessly, and the Spotify app's
    /// Next skips to it (taken as Jewelcase's next song, `soloist_item`).
    /// Soloist's queue can't be emptied, so only one song at a time, and
    /// none while a song Jewelcase didn't need after all is still in it
    /// (Soloist plays that one first; it ends Jewelcase's song).
    fn hand_over_next(&self) {
        let next = {
            let state = &self.imp().stream.borrow().soloist;
            // Not the song itself (Repeat One, the same song twice):
            // Soloist would play it without an end.
            state.next_uri.clone().filter(|next| {
                state.queued_next.is_none()
                    && state.stale_queue.is_empty()
                    && Some(next) != state.wanted.as_ref()
            })
        };
        if let (Some(next), Some(soloist)) = (next, self.soloist()) {
            tracing::info!("Soloist: handing {next} to its queue");
            soloist.add_to_queue(&next);
            self.imp().stream.borrow_mut().soloist.queued_next = Some(next);
        }
    }

    /// The next song handed over, and a timer for the end of the current
    /// one (see `soloist_next`).
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
                .unwrap_or_else(|| JewelcaseApplication::get().player().duration());
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
        self.hand_over_next();
        glib::timeout_add_local_once(
            std::time::Duration::from_millis(remaining.saturating_sub(DUCK_BEFORE_MS)),
            glib::clone!(
                #[weak(rename_to = spotify)]
                self,
                move || {
                    // Muted unless Soloist goes on with Jewelcase's next
                    // song: a local one follows, or Soloist would play a
                    // song Jewelcase doesn't want (or its own).
                    let follows = {
                        let state = &spotify.imp().stream.borrow().soloist;
                        state.next_uri.is_some()
                            && state.queued_next == state.next_uri
                            && state.stale_queue.is_empty()
                    };
                    if still(&spotify)
                        && !follows
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

    /// Soloist's volume: Jewelcase's, lowered by the offset for Spotify's
    /// louder songs (Preferences, default −4 dB).
    fn soloist_percent(&self, volume: f64) -> u8 {
        let offset_db = f64::from(self.settings().int("spotify-volume-offset"));
        let factor = 10f64.powf(offset_db / 20.0);
        (volume.clamp(0.0, 1.0) * factor * 100.0)
            .round()
            .clamp(0.0, 100.0) as u8
    }

    /// Jewelcase gives up on the song; Soloist is paused, so it doesn't
    /// start it later after all.
    fn soloist_unavailable(&self) {
        {
            let mut stream = self.imp().stream.borrow_mut();
            stream.soloist.wanted = None;
            stream.soloist.started = false;
        }
        if let Some(soloist) = self.soloist() {
            self.imp().stream.borrow_mut().soloist.pause_asked = Some(std::time::Instant::now());
            soloist.pause();
        }
        JewelcaseApplication::get()
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
        // Jewelcase's song is always started directly, never by skipping to
        // it in Soloist's queue: Soloist slips on from a song it skipped to
        // (to the next one handed over, or on a seek). Another song waiting
        // there is skipped past, muted until the new song starts
        // (soloist_item); the song itself is left there (a skip to it would
        // look like its start), as a left-over that ends it.
        self.restore_volume();
        {
            let mut stream = self.imp().stream.borrow_mut();
            let state = &mut stream.soloist;
            if state.queued_next.as_deref() == Some(uri) {
                Self::forget_queued(state);
            }
        }
        self.consume_queued();
        let Some(soloist) = self.soloist() else {
            JewelcaseApplication::get()
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
        let (prepare, active) = {
            let mut stream = self.imp().stream.borrow_mut();
            let state = &mut stream.soloist;
            let active = std::mem::replace(&mut state.is_active, true);
            state.pending = None;
            state.wanted = Some(uri.to_owned());
            state.started = false;
            state.seek_after_start = (start_ms > 0).then_some(start_ms);
            state.position = None;
            state.last_seek = None;
            (!std::mem::replace(&mut state.prepared, true), active)
        };
        // Linked once, but maybe not the active device right now.
        if !active {
            soloist.activate();
        }
        // Jewelcase's queue decides what comes next, not Soloist's.
        if prepare {
            soloist.set_shuffle(false);
            soloist.set_repeat(Repeat::Off);
        }
        tracing::info!("Soloist: playing {uri}");
        soloist.play(Some(uri));
        self.watch_start(uri);
    }

    /// No start of `uri` in time: Spotify can't play it.
    fn watch_start(&self, uri: &str) {
        let (generation, load) = {
            let mut stream = self.imp().stream.borrow_mut();
            let state = &mut stream.soloist;
            state.loads += 1;
            (state.generation, state.loads)
        };
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
                            && state.loads == load
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

    /// Jewelcase plays something else: Soloist pauses.
    pub(super) fn soloist_stop_playback(&self) {
        self.restore_volume();
        // Not left in Soloist's queue (paused right after).
        self.consume_queued();
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
        stream.soloist.last_seek = (stream.soloist.started && stream.soloist.wanted.is_some())
            .then(|| (std::time::Instant::now(), ms));
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
            .or_else(|| (state.source == Some("jewelcase")).then(|| own_device_name().to_owned()))
    }

    // The receiver (§9.4) ---------------------------------------------------

    pub fn soloist_receiving(&self) -> bool {
        self.imp().stream.borrow().soloist.receiving
    }

    /// Soloist plays something Jewelcase didn't start: show it, and let the
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
        let player = JewelcaseApplication::get().player();
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
                .unwrap_or_else(|| own_device_name().to_owned())
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

    /// Back to Jewelcase's own playback; `pause` stops Soloist.
    pub(super) fn end_receiver(&self, pause: bool) {
        {
            let mut stream = self.imp().stream.borrow_mut();
            stream.soloist.receiving = false;
            stream.soloist.quiet_until = Some(std::time::Instant::now() + QUIET);
        }
        if pause && let Some(soloist) = self.soloist() {
            soloist.pause();
        }
        JewelcaseApplication::get().player().leave_remote();
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
                let player = JewelcaseApplication::get().player();
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
                    let next: Vec<jewelcase_spotify::Track> =
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
                                Ok(jewelcase_core::db::SpotifyVersion {
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
            // Jewelcase plays its own queue: the receiver ends.
            RemoteCommand::Play(_) => {
                glib::idle_add_local_once(glib::clone!(
                    #[weak(rename_to = spotify)]
                    self,
                    move || {
                        spotify.end_receiver(true);
                        JewelcaseApplication::get().player().play_current();
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
        // Jewelcase's own Spotify song gives way; the receiver shows this.
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
fn as_track(item: &Item) -> jewelcase_spotify::Track {
    jewelcase_spotify::Track {
        uri: item.uri.clone(),
        id: item.uri.strip_prefix("spotify:track:").map(str::to_owned),
        name: item.name.clone(),
        artists: item.artists.clone(),
        album: item.album.clone(),
        duration_ms: item.duration_ms.unwrap_or(0),
        image_url: item.cover_url.clone(),
        is_playable: true,
        ..jewelcase_spotify::Track::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soloist_moving_on_right_after_a_seek_is_a_slip() {
        let now = std::time::Instant::now();
        let second = std::time::Duration::from_secs(1);
        let seek = Some((now - second, 51_000));
        // A second after a seek to 0:51, minutes left: the song goes on
        // where it should be by now.
        assert_eq!(seek_slip(seek, Some(140_000), now), Some(52_000));
        // Near the end: the song really ended.
        assert_eq!(seek_slip(seek, Some(5_000), now), None);
        // Long after the seek: someone chose a song in the Spotify app.
        let old = Some((now - 10 * second, 51_000));
        assert_eq!(seek_slip(old, Some(140_000), now), None);
        // No seek, or nothing known about what was left.
        assert_eq!(seek_slip(None, Some(140_000), now), None);
        assert_eq!(seek_slip(seek, None, now), None);
    }

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
