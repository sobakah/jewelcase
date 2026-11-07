//! Spotify Connect devices and remote mode (CONCEPT.md §9.3): the player
//! shows what a device plays and its buttons control it.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use gatefold_core::db::{SpotifyVersion, Track};
use gatefold_core::library::TrackRecord;
use gatefold_core::library::tags::TrackTags;
use gatefold_core::matching::{Confidence, MatchIndex, RemoteTrack};
use gatefold_core::queue::RepeatMode;
use gatefold_spotify::{Device, Error, PlayRequest};
use gettextrs::gettext;
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
/// Songs taken along when moving playback to a device.
const HAND_OVER_SONGS: usize = 20;

/// What the player's controls do in remote mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RemoteCommand {
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
    /// Covers being downloaded, by URL.
    loading: HashSet<String>,
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
        let mut uris = Vec::new();
        let mut later = Vec::new();
        if player.playing() && !player.is_remote() {
            let ids = player.upcoming_track_ids(HAND_OVER_SONGS);
            if let Some(first) = ids.first()
                && let Some(version) = self.find_version(*first).await
                && let (Some(uri), Some(Confidence::Sure)) = (version.uri, version.confidence)
            {
                uris.push(uri);
                later = ids[1..].to_vec();
            }
        }
        let position = player.position();
        let was_playing = player.playing();
        let id = device_id.clone();
        let result = self
            .call(move |client| {
                if uris.is_empty() {
                    client.transfer(&id, was_playing)
                } else {
                    client.play(
                        Some(&id),
                        Some(PlayRequest::Tracks {
                            uris,
                            offset: 0,
                            position_ms: position,
                        }),
                    )
                }
            })
            .await;
        if let Err(err) = result {
            tracing::warn!("Cannot play on {}: {err}", device.name);
            self.message(&device_error(&err, &device.name));
            return;
        }
        self.imp().remote.borrow_mut().device_id = Some(device_id.clone());
        // What follows goes into the device's queue, as it's found.
        if !later.is_empty() {
            glib::spawn_future_local(glib::clone!(
                #[weak(rename_to = spotify)]
                self,
                async move {
                    for id in later {
                        if spotify.remote_device_id().as_deref() != Some(device_id.as_str()) {
                            break;
                        }
                        let Some(SpotifyVersion {
                            uri: Some(uri),
                            confidence: Some(Confidence::Sure),
                            ..
                        }) = spotify.find_version(id).await
                        else {
                            continue;
                        };
                        if let Err(err) = spotify.call(move |c| c.add_to_queue(&uri)).await {
                            tracing::info!("Cannot queue on the device: {err}");
                            break;
                        }
                    }
                }
            ));
        }
        let weak = self.downgrade();
        player.enter_remote(&device.name, move |command| {
            if let Some(spotify) = weak.upgrade() {
                spotify.remote_command(command);
            }
        });
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

    fn remote_command(&self, command: RemoteCommand) {
        let playing = self.imp().remote.borrow().playing;
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
            RemoteCommand::Volume(_) => Ok(()),
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
        match self.call(|client| client.playback()).await {
            Ok(Some(state)) => {
                if let Some(device) = &state.device {
                    // Playback moved to another device (e.g. the phone).
                    player.set_remote_device(Some(&device.name));
                    self.imp().remote.borrow_mut().device_id = device.id.clone();
                }
                self.imp().remote.borrow_mut().playing = state.is_playing;
                let track = match &state.track {
                    Some(track) => Some(self.track_object(track).await),
                    None => None,
                };
                if !player.is_remote() {
                    return;
                }
                player.show_remote(
                    track,
                    state.is_playing,
                    state.progress_ms,
                    state.shuffle,
                    match state.repeat.as_str() {
                        "context" => RepeatMode::All,
                        "track" => RepeatMode::One,
                        _ => RepeatMode::Off,
                    },
                );
            }
            Ok(None) => {
                self.imp().remote.borrow_mut().playing = false;
                player.show_remote(None, false, 0, false, RepeatMode::Off);
            }
            Err(Error::LoggedOut) => return,
            Err(err) => tracing::info!("Cannot ask Spotify what plays: {err}"),
        }
        if player.is_remote() {
            self.poll_soon(POLL_INTERVAL);
        }
    }

    /// The library song when it's in the library (lyrics, cover, details),
    /// else a stand-in with Spotify's cover.
    async fn track_object(&self, track: &gatefold_spotify::Track) -> TrackObject {
        let library = GatefoldApplication::get().library();
        let known = self.imp().remote.borrow().matches.get(&track.uri).copied();
        let local = match known {
            Some(local) => local,
            None => {
                let aliases = library.aliases().clone();
                let local = library.db().and_then(|db| {
                    let index = MatchIndex::new(db.match_tracks().ok()?, &aliases);
                    index
                        .find(&RemoteTrack {
                            title: track.name.clone(),
                            artists: track.artists.clone(),
                            album: track.album.clone(),
                            duration_ms: track.duration_ms,
                            isrc: track.isrc.clone(),
                        })
                        .filter(|(_, confidence)| *confidence == Confidence::Sure)
                        .map(|(id, _)| id)
                });
                self.imp()
                    .remote
                    .borrow_mut()
                    .matches
                    .insert(track.uri.clone(), local);
                local
            }
        };
        if let Some(id) = local
            && let Some(found) = library.tracks(&[id]).remove(&id)
        {
            return TrackObject::new(found);
        }
        let cover_key = match &track.image_url {
            Some(url) => self.remote_cover(url).await,
            None => None,
        };
        TrackObject::new(stand_in(track, cover_key))
    }

    /// Spotify's cover in the cover cache; `None` while it can't be had.
    async fn remote_cover(&self, url: &str) -> Option<String> {
        let key = gatefold_core::library::covers::cover_key('s', url.as_bytes());
        let covers = gatefold_core::library::covers::CoverCache::new(crate::config::cover_dir());
        if covers.has(&key) {
            return Some(key);
        }
        if !self
            .imp()
            .remote
            .borrow_mut()
            .loading
            .insert(url.to_owned())
        {
            return None;
        }
        let (job_url, job_key) = (url.to_owned(), key.clone());
        let made = gtk::gio::spawn_blocking(move || {
            gatefold_spotify::download_image(&job_url)
                .ok()
                .is_some_and(|data| covers.generate(&job_key, data).is_ok())
        })
        .await
        .unwrap_or(false);
        self.imp().remote.borrow_mut().loading.remove(url);
        made.then_some(key)
    }
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
