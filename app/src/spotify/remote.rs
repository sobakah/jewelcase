//! Spotify Connect devices and remote mode (CONCEPT.md §9.3): the player
//! shows what a device plays and its buttons control it.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use gatefold_core::db::{SpotifyVersion, Track};
use gatefold_core::library::TrackRecord;
use gatefold_core::library::tags::TrackTags;
use gatefold_core::matching::{Confidence, MatchIndex, RemoteTrack};
use gatefold_core::queue::RepeatMode;
use gatefold_spotify::{Device, Error, PlayRequest};
use gettextrs::{gettext, ngettext};
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::Spotify;
use crate::application::GatefoldApplication;
use crate::objects::TrackObject;

/// How often a remote device is asked what it plays.
const POLL_INTERVAL: Duration = Duration::from_secs(3);
/// After a command, the next report comes sooner.
const AFTER_COMMAND: Duration = Duration::from_millis(700);
/// Volume changes are sent once the slider rests this long.
const VOLUME_DELAY: Duration = Duration::from_millis(300);
/// The device's queue is read again after this many reports.
const QUEUE_EVERY: u32 = 5;
/// Covers downloaded at the same time.
const COVER_DOWNLOADS: usize = 4;
/// Covers waiting to be downloaded at most (the oldest wishes go).
const COVER_WAITING: usize = 100;
/// A cover that couldn't be had is tried again after this.
const COVER_RETRY: Duration = Duration::from_secs(60);
/// Covers that arrive close together refresh the view once.
const COVER_REFRESH: Duration = Duration::from_millis(400);

/// What the player's controls do in remote mode.
#[derive(Debug, Clone, PartialEq)]
pub enum RemoteCommand {
    /// Library songs to play on the device (the first one now).
    Play(Vec<i64>),
    /// Library songs for the device's queue.
    Queue(Vec<i64>),
    PlayPause,
    Pause,
    Next,
    Previous,
    Seek(u64),
    /// 0.0 to 1.0.
    Volume(f64),
    Shuffle(bool),
    Repeat(RepeatMode),
}

#[derive(Default)]
pub struct RemoteState {
    device_id: Option<String>,
    playing: bool,
    poll: Option<glib::SourceId>,
    volume_timer: Option<glib::SourceId>,
    /// Spotify URI → library song, once looked up.
    matches: HashMap<String, Option<i64>>,
    /// Covers to download, by URL, the most wanted first.
    covers_wanted: VecDeque<String>,
    /// Covers being downloaded, by URL.
    loading: HashSet<String>,
    /// Covers that couldn't be had, and when.
    covers_failed: HashMap<String, Instant>,
    /// A view refresh for new covers is coming.
    covers_refresh: bool,
    /// Counts started hand-overs; an older one stops queueing.
    generation: u64,
    /// The device's queue needs reading again.
    queue_stale: bool,
    last_uri: Option<String>,
    polls: u32,
    /// The device's last state and queue as Spotify gave them, to show
    /// them again (covers arrived) without asking again.
    last_state: Option<gatefold_spotify::PlaybackState>,
    last_queue: Vec<gatefold_spotify::Track>,
}

impl Spotify {
    pub async fn devices(&self) -> gatefold_spotify::Result<Vec<Device>> {
        self.call(|client| client.devices()).await
    }

    pub fn remote_device_id(&self) -> Option<String> {
        self.imp().remote.borrow().device_id.clone()
    }

    /// Moves playback to a device: the current song (and what follows)
    /// when Spotify has them, else whatever Spotify last played.
    pub async fn play_on(&self, device: Device) {
        let Some(device_id) = device.id.clone() else {
            return;
        };
        let player = GatefoldApplication::get().player();
        let previous = self
            .imp()
            .remote
            .borrow_mut()
            .device_id
            .replace(device_id.clone());
        let mut started = false;
        if player.playing() && !player.is_remote() {
            let ids = player.upcoming_track_ids(crate::player::REMOTE_HAND_OVER);
            started = self.play_ids(ids, player.position()).await;
        }
        if !started {
            let id = device_id.clone();
            let was_playing = player.playing();
            if let Err(err) = self
                .call(move |client| client.transfer(&id, was_playing))
                .await
            {
                tracing::warn!("Cannot play on {}: {err}", device.name);
                self.message(&device_error(&err, &device.name));
                self.imp().remote.borrow_mut().device_id = previous;
                return;
            }
        }
        let weak = self.downgrade();
        player.enter_remote(&device.name, move |command| {
            if let Some(spotify) = weak.upgrade() {
                spotify.remote_command(command);
            }
        });
        self.poll_soon(AFTER_COMMAND);
    }

    /// Plays library songs on the device: the first one Spotify has right
    /// away, the rest go into the device's queue as they're found. Returns
    /// whether something started.
    async fn play_ids(&self, ids: Vec<i64>, position_ms: u64) -> bool {
        let generation = {
            let mut state = self.imp().remote.borrow_mut();
            state.generation += 1;
            state.generation
        };
        let library = GatefoldApplication::get().library();
        let title_of = |id: i64| {
            library
                .tracks(&[id])
                .remove(&id)
                .and_then(|t| t.record.tags.title)
                .unwrap_or_default()
        };
        let mut start = None;
        let mut skipped = Vec::new();
        for (index, id) in ids.iter().enumerate().take(10) {
            // Playing isn't editing: a likely match is good enough.
            match self.find_version(*id).await {
                Ok(SpotifyVersion { uri: Some(uri), .. }) => {
                    start = Some((index, uri));
                    break;
                }
                Ok(_) => skipped.push(title_of(*id)),
                Err(err) => {
                    self.message(&super::sync::search_error(&err));
                    return false;
                }
            }
        }
        let Some((index, uri)) = start else {
            if let Some(first) = skipped.first() {
                // Translators: {} is a song title.
                self.message(&gettext("“{}” isn't on Spotify").replace("{}", first));
            }
            return false;
        };
        if let Some(first) = skipped.first() {
            // Translators: {} is a song title.
            self.message(&gettext("“{}” isn't on Spotify; skipped").replace("{}", first));
        }
        let device = self.remote_device_id();
        let position = if index == 0 { position_ms } else { 0 };
        let result = self
            .call(move |client| {
                client.play(
                    device.as_deref(),
                    Some(PlayRequest::Tracks {
                        uris: vec![uri],
                        offset: 0,
                        position_ms: position,
                    }),
                )
            })
            .await;
        if let Err(err) = result {
            tracing::warn!("Cannot play on the device: {err}");
            let device = GatefoldApplication::get()
                .player()
                .remote_device()
                .unwrap_or_default();
            self.message(&device_error(&err, &device));
            return false;
        }
        self.poll_soon(AFTER_COMMAND);
        // What follows goes into the device's queue, unless something else
        // is started meanwhile.
        let later = ids[index + 1..].to_vec();
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = spotify)]
            self,
            async move {
                for id in later {
                    if spotify.imp().remote.borrow().generation != generation {
                        break;
                    }
                    let uri = match spotify.find_version(id).await {
                        Ok(SpotifyVersion { uri: Some(uri), .. }) => uri,
                        Ok(_) => continue,
                        Err(_) => break,
                    };
                    if spotify.imp().remote.borrow().generation != generation {
                        break;
                    }
                    if let Err(err) = spotify.call(move |c| c.add_to_queue(&uri)).await {
                        tracing::info!("Cannot queue on the device: {err}");
                        break;
                    }
                    spotify.imp().remote.borrow_mut().queue_stale = true;
                }
            }
        ));
        true
    }

    /// "Play Next" / "Add to Queue" in remote mode: into the device's
    /// queue (Spotify only adds at its end, which plays next).
    async fn queue_ids(&self, ids: Vec<i64>) {
        let mut added = 0;
        let mut missing = 0;
        for id in ids {
            let uri = match self.find_version(id).await {
                Ok(SpotifyVersion { uri: Some(uri), .. }) => uri,
                Ok(_) => {
                    missing += 1;
                    continue;
                }
                Err(err) => {
                    self.message(&super::sync::search_error(&err));
                    break;
                }
            };
            match self.call(move |c| c.add_to_queue(&uri)).await {
                Ok(()) => added += 1,
                Err(err) => {
                    tracing::warn!("Cannot queue on the device: {err}");
                    self.message(&device_error(&err, ""));
                    break;
                }
            }
        }
        let device = GatefoldApplication::get()
            .player()
            .remote_device()
            .unwrap_or_default();
        if added > 0 {
            self.message(
                &ngettext(
                    "Added {n} song to the queue on {device}",
                    "Added {n} songs to the queue on {device}",
                    added,
                )
                .replace("{n}", &added.to_string())
                .replace("{device}", &device),
            );
        }
        if missing > 0 {
            self.message(
                &ngettext(
                    "{} song isn't on Spotify",
                    "{} songs aren't on Spotify",
                    missing,
                )
                .replace("{}", &missing.to_string()),
            );
        }
        self.imp().remote.borrow_mut().queue_stale = true;
        self.poll_soon(AFTER_COMMAND);
    }

    /// Plays a Spotify playlist on the remote device, from an entry.
    pub async fn play_playlist_remote(&self, spotify_id: &str, offset: u32) {
        let device = self.remote_device_id();
        let uri = format!("spotify:playlist:{spotify_id}");
        let result = self
            .call(move |client| {
                client.play(
                    device.as_deref(),
                    Some(PlayRequest::Context {
                        uri,
                        offset,
                        position_ms: 0,
                    }),
                )
            })
            .await;
        if let Err(err) = result {
            tracing::warn!("Cannot play the playlist remotely: {err}");
            self.message(&gettext("The playlist couldn't be played on Spotify"));
        }
        self.poll_soon(AFTER_COMMAND);
    }

    /// Back to this computer; `pause` stops the device.
    pub fn play_here(&self, pause: bool) {
        // Showing what Soloist plays for someone else (§9.4).
        if self.soloist_receiving() {
            self.end_receiver(pause);
            return;
        }
        let player = GatefoldApplication::get().player();
        let was_remote = player.is_remote();
        {
            let mut state = self.imp().remote.borrow_mut();
            state.device_id = None;
            if let Some(poll) = state.poll.take() {
                poll.remove();
            }
        }
        if !was_remote {
            return;
        }
        player.leave_remote();
        if pause {
            glib::spawn_future_local(glib::clone!(
                #[weak(rename_to = spotify)]
                self,
                async move {
                    if let Err(err) = spotify.call(|client| client.pause()).await {
                        tracing::info!("Cannot pause the Spotify device: {err}");
                    }
                }
            ));
        }
    }

    pub(super) fn remote_command(&self, command: RemoteCommand) {
        let playing = self.imp().remote.borrow().playing;
        match command {
            RemoteCommand::Play(ids) => {
                glib::spawn_future_local(glib::clone!(
                    #[weak(rename_to = spotify)]
                    self,
                    async move {
                        spotify.play_ids(ids, 0).await;
                    }
                ));
                return;
            }
            RemoteCommand::Queue(ids) => {
                glib::spawn_future_local(glib::clone!(
                    #[weak(rename_to = spotify)]
                    self,
                    async move { spotify.queue_ids(ids).await }
                ));
                return;
            }
            _ => {}
        }
        if let RemoteCommand::Volume(volume) = command {
            let mut state = self.imp().remote.borrow_mut();
            if let Some(timer) = state.volume_timer.take() {
                timer.remove();
            }
            state.volume_timer = Some(glib::timeout_add_local_once(
                VOLUME_DELAY,
                glib::clone!(
                    #[weak(rename_to = spotify)]
                    self,
                    move || {
                        spotify.imp().remote.borrow_mut().volume_timer = None;
                        spotify
                            .send(move |client| client.set_volume((volume * 100.0).round() as u8));
                    }
                ),
            ));
            return;
        }
        // Show it right away; the next report confirms it.
        if matches!(command, RemoteCommand::PlayPause | RemoteCommand::Pause) {
            let now_playing = command == RemoteCommand::PlayPause && !playing;
            self.imp().remote.borrow_mut().playing = now_playing;
        }
        self.send(move |client| match command {
            RemoteCommand::PlayPause if playing => client.pause(),
            RemoteCommand::PlayPause => client.play(None, None),
            RemoteCommand::Pause => client.pause(),
            RemoteCommand::Next => client.next(),
            RemoteCommand::Previous => client.previous(),
            RemoteCommand::Seek(ms) => client.seek(ms),
            RemoteCommand::Shuffle(on) => client.set_shuffle(on),
            RemoteCommand::Repeat(mode) => client.set_repeat(match mode {
                RepeatMode::Off => "off",
                RepeatMode::All => "context",
                RepeatMode::One => "track",
            }),
            RemoteCommand::Volume(_) | RemoteCommand::Play(_) | RemoteCommand::Queue(_) => Ok(()),
        });
    }

    /// A player call, then a quick report.
    fn send(
        &self,
        job: impl FnOnce(&gatefold_spotify::Client) -> gatefold_spotify::Result<()> + Send + 'static,
    ) {
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = spotify)]
            self,
            async move {
                if let Err(err) = spotify.call(job).await {
                    tracing::warn!("Spotify player command failed: {err}");
                    let device = GatefoldApplication::get()
                        .player()
                        .remote_device()
                        .unwrap_or_default();
                    spotify.message(&device_error(&err, &device));
                }
                spotify.poll_soon(AFTER_COMMAND);
            }
        ));
    }

    fn poll_soon(&self, delay: Duration) {
        let mut state = self.imp().remote.borrow_mut();
        if let Some(poll) = state.poll.take() {
            poll.remove();
        }
        state.poll = Some(glib::timeout_add_local_once(
            delay,
            glib::clone!(
                #[weak(rename_to = spotify)]
                self,
                move || {
                    spotify.imp().remote.borrow_mut().poll = None;
                    glib::spawn_future_local(async move { spotify.poll().await });
                }
            ),
        ));
    }

    /// Asks Spotify what plays and shows it.
    async fn poll(&self) {
        let player = GatefoldApplication::get().player();
        if !player.is_remote() {
            return;
        }
        let result = self.call(|client| client.playback()).await;
        // "Play Here" while asking: the answer is for a mode that ended.
        if !player.is_remote() {
            return;
        }
        match result {
            Ok(Some(state)) => {
                if let Some(device) = &state.device {
                    // Playback moved to another device (e.g. the phone).
                    player.set_remote_device(Some(&device.name));
                    self.imp().remote.borrow_mut().device_id = device.id.clone();
                }
                self.imp().remote.borrow_mut().playing = state.is_playing;
                let track = match &state.track {
                    Some(track) => self.track_objects(std::slice::from_ref(track)).await.pop(),
                    None => None,
                };
                if !player.is_remote() {
                    return;
                }
                // The queue, when the song changed, after adding, and now
                // and then.
                let uri = state.track.as_ref().map(|t| t.uri.clone());
                let read_queue = {
                    let mut remote = self.imp().remote.borrow_mut();
                    remote.polls += 1;
                    let read = remote.last_uri != uri
                        || std::mem::take(&mut remote.queue_stale)
                        || remote.polls.is_multiple_of(QUEUE_EVERY);
                    remote.last_uri = uri;
                    read
                };
                if read_queue {
                    match self.call(|client| client.queue()).await {
                        Ok(queue) => {
                            let mut tracks: Vec<TrackObject> = track.iter().cloned().collect();
                            tracks.extend(self.track_objects(&queue).await);
                            self.imp().remote.borrow_mut().last_queue = queue;
                            if player.is_remote() {
                                player.set_remote_queue(tracks);
                            }
                        }
                        Err(err) => tracing::info!("Cannot read the device's queue: {err}"),
                    }
                }
                player.show_remote(
                    track,
                    state.is_playing,
                    state.progress_ms,
                    state.shuffle,
                    repeat_mode(&state.repeat),
                );
                self.imp().remote.borrow_mut().last_state = Some(state);
            }
            Ok(None) => {
                self.imp().remote.borrow_mut().playing = false;
                player.show_remote(None, false, 0, false, RepeatMode::Off);
            }
            Err(Error::LoggedOut) => return,
            // Too many requests: as long as Spotify asks, not every 3 s.
            Err(Error::RateLimited(wait)) => {
                tracing::info!("Spotify asks to wait {wait:?} before asking what plays");
                self.poll_soon(wait.max(POLL_INTERVAL));
                return;
            }
            Err(err) => tracing::info!("Cannot ask Spotify what plays: {err}"),
        }
        if player.is_remote() {
            self.poll_soon(POLL_INTERVAL);
        }
    }

    /// The device's song and queue again, from the last answers (their
    /// covers arrived): no new requests.
    async fn show_remote_again(&self) {
        let player = GatefoldApplication::get().player();
        let (state, queue) = {
            let remote = self.imp().remote.borrow();
            (remote.last_state.clone(), remote.last_queue.clone())
        };
        let Some(state) = state else { return };
        let track = match &state.track {
            Some(track) => self.track_objects(std::slice::from_ref(track)).await.pop(),
            None => None,
        };
        let mut tracks: Vec<TrackObject> = track.iter().cloned().collect();
        tracks.extend(self.track_objects(&queue).await);
        if !player.is_remote() || self.soloist_receiving() {
            return;
        }
        player.set_remote_queue(tracks);
        player.show_remote(
            track,
            state.is_playing,
            player.position(),
            state.shuffle,
            repeat_mode(&state.repeat),
        );
    }

    /// Library songs where the library has them (lyrics, cover, details),
    /// else stand-ins with Spotify's covers.
    pub(super) async fn track_objects(
        &self,
        tracks: &[gatefold_spotify::Track],
    ) -> Vec<TrackObject> {
        let library = GatefoldApplication::get().library();
        let unknown: Vec<&gatefold_spotify::Track> = tracks
            .iter()
            .filter(|t| !self.imp().remote.borrow().matches.contains_key(&t.uri))
            .collect();
        if !unknown.is_empty() {
            let aliases = library.aliases().clone();
            let found: Vec<(String, Option<i64>)> = library
                .db()
                .and_then(|db| db.match_tracks().ok())
                .map(|local| {
                    let index = MatchIndex::new(local, &aliases);
                    unknown
                        .iter()
                        .map(|track| {
                            let id = index
                                .find(&RemoteTrack {
                                    title: track.name.clone(),
                                    artists: track.artists.clone(),
                                    album: track.album.clone(),
                                    duration_ms: track.duration_ms,
                                    isrc: track.isrc.clone(),
                                })
                                .filter(|(_, confidence)| *confidence == Confidence::Sure)
                                .map(|(id, _)| id);
                            (track.uri.clone(), id)
                        })
                        .collect()
                })
                .unwrap_or_default();
            self.imp().remote.borrow_mut().matches.extend(found);
        }
        let ids: Vec<i64> = tracks
            .iter()
            .filter_map(|t| {
                self.imp()
                    .remote
                    .borrow()
                    .matches
                    .get(&t.uri)
                    .copied()
                    .flatten()
            })
            .collect();
        let mut local = library.tracks(&ids);
        let mut objects = Vec::with_capacity(tracks.len());
        let mut missing = Vec::new();
        for track in tracks {
            let id = self
                .imp()
                .remote
                .borrow()
                .matches
                .get(&track.uri)
                .copied()
                .flatten();
            if let Some(found) = id.and_then(|id| {
                local
                    .remove(&id)
                    .or_else(|| library.tracks(&[id]).remove(&id))
            }) {
                objects.push(TrackObject::new(found));
                continue;
            }
            let cover_key = track.image_url.as_deref().and_then(|url| {
                let key = remote_cover_key(url);
                if cover_cache().has(&key) {
                    Some(key)
                } else {
                    missing.push(url.to_owned());
                    None
                }
            });
            objects.push(TrackObject::new(stand_in(track, cover_key)));
        }
        self.want_covers(missing);
        objects
    }

    /// Downloads Spotify's covers in the background, in the order given
    /// and before ones wished for earlier (the song playing comes first,
    /// then what's next). The view is shown again as they arrive.
    fn want_covers(&self, urls: Vec<String>) {
        if urls.is_empty() {
            return;
        }
        {
            let mut state = self.imp().remote.borrow_mut();
            let state = &mut *state;
            state
                .covers_failed
                .retain(|_, at| at.elapsed() < COVER_RETRY);
            let new: Vec<String> = urls
                .into_iter()
                .filter(|url| {
                    !state.loading.contains(url) && !state.covers_failed.contains_key(url)
                })
                .collect();
            state.covers_wanted.retain(|url| !new.contains(url));
            for url in new.into_iter().rev() {
                state.covers_wanted.push_front(url);
            }
            state.covers_wanted.truncate(COVER_WAITING);
        }
        self.download_covers();
    }

    /// Starts downloads while fewer than `COVER_DOWNLOADS` run.
    fn download_covers(&self) {
        loop {
            let url = {
                let mut state = self.imp().remote.borrow_mut();
                if state.loading.len() >= COVER_DOWNLOADS {
                    return;
                }
                let Some(url) = state.covers_wanted.pop_front() else {
                    return;
                };
                if state.loading.contains(&url) {
                    continue;
                }
                state.loading.insert(url.clone());
                url
            };
            glib::spawn_future_local(glib::clone!(
                #[weak(rename_to = spotify)]
                self,
                async move {
                    let job_url = url.clone();
                    let made = gtk::gio::spawn_blocking(move || {
                        let key = remote_cover_key(&job_url);
                        let covers = cover_cache();
                        covers.has(&key)
                            || gatefold_spotify::download_image(&job_url)
                                .ok()
                                .is_some_and(|data| covers.generate(&key, data).is_ok())
                    })
                    .await
                    .unwrap_or(false);
                    {
                        let mut state = spotify.imp().remote.borrow_mut();
                        state.loading.remove(&url);
                        if !made {
                            state.covers_failed.insert(url, Instant::now());
                        }
                    }
                    if made {
                        spotify.refresh_for_covers();
                    }
                    spotify.download_covers();
                }
            ));
        }
    }

    /// Shows the songs again, now with their covers.
    fn refresh_for_covers(&self) {
        if std::mem::replace(&mut self.imp().remote.borrow_mut().covers_refresh, true) {
            return;
        }
        glib::timeout_add_local_once(
            COVER_REFRESH,
            glib::clone!(
                #[weak(rename_to = spotify)]
                self,
                move || {
                    spotify.imp().remote.borrow_mut().covers_refresh = false;
                    if spotify.soloist_receiving() {
                        spotify.receiver_show(true);
                    } else if GatefoldApplication::get().player().is_remote() {
                        glib::spawn_future_local(async move {
                            spotify.show_remote_again().await;
                        });
                    }
                }
            ),
        );
    }
}

fn repeat_mode(repeat: &str) -> RepeatMode {
    match repeat {
        "context" => RepeatMode::All,
        "track" => RepeatMode::One,
        _ => RepeatMode::Off,
    }
}

fn remote_cover_key(url: &str) -> String {
    gatefold_core::library::covers::cover_key('s', url.as_bytes())
}

fn cover_cache() -> gatefold_core::library::covers::CoverCache {
    gatefold_core::library::covers::CoverCache::new(crate::config::cover_dir())
}

/// A stand-in track for a Spotify song that isn't in the library. Its ID
/// is negative, so it never collides with a library song.
fn stand_in(track: &gatefold_spotify::Track, cover_key: Option<String>) -> Track {
    let id = -(i64::from(fnv(&track.uri) & 0x7fff_ffff) + 1);
    Track {
        record: TrackRecord {
            id,
            path: std::path::PathBuf::new(),
            tags: TrackTags {
                title: Some(track.name.clone()),
                artist: Some(track.artist_line()),
                album: track.album.clone(),
                duration_ms: track.duration_ms,
                isrc: track.isrc.clone(),
                format: "Spotify".into(),
                ..TrackTags::default()
            },
            cover_key: cover_key.clone(),
            added: 0,
        },
        album_id: None,
        album_title: track.album.clone(),
        album_artist: track.artists.first().cloned(),
        album_cover_key: cover_key,
        album_disc: None,
    }
}

fn fnv(text: &str) -> u32 {
    text.bytes().fold(0x811c_9dc5, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    })
}

/// What to tell the user when a device doesn't do what was asked.
fn device_error(err: &Error, device: &str) -> String {
    match err {
        Error::Status {
            reason: Some(reason),
            ..
        } if reason == "PREMIUM_REQUIRED" => gettext("Controlling devices needs Spotify Premium"),
        Error::Status { status: 404, .. } => {
            // Translators: {} is a device name, e.g. "Kitchen".
            gettext("“{}” isn't available any more").replace("{}", device)
        }
        Error::Status { status: 403, .. } => gettext("Spotify doesn't allow this right now"),
        _ => gettext("Couldn't reach Spotify"),
    }
}
