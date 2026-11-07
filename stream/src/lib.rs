//! Spotify streaming (CONCEPT.md §9.4), optional and unofficial: librespot
//! plays Spotify tracks, and its decoded samples go into a GStreamer
//! pipeline with Gatefold's volume, on the same output as local songs.
//!
//! librespot needs Tokio; it runs on its own thread. Everything here can be
//! called from the GTK main thread and never blocks it.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use gst::prelude::*;
use librespot_core::authentication::Credentials as LibrespotCredentials;
use librespot_core::{Session, SessionConfig, SpotifyUri};
use librespot_playback::audio_backend::{Sink, SinkError, SinkResult};
use librespot_playback::config::{NormalisationType, PlayerConfig};
use librespot_playback::convert::Converter;
use librespot_playback::decoder::AudioPacket;
use librespot_playback::mixer::NoOpVolume;
use librespot_playback::player::{Player, PlayerEvent};
use librespot_protocol::authentication::AuthenticationType;
use tokio::sync::mpsc;

/// The client ID librespot logs in with (Spotify's own desktop client);
/// streaming needs it, the user's own Web API app can't stream.
pub const CLIENT_ID: &str = "65b708073fc0480ea92a077233ca87bd";
/// Where the browser comes back to after the streaming login.
pub const REDIRECT_PORT: u16 = 5588;
pub const REDIRECT_URI: &str = "http://127.0.0.1:5588/login";
pub const SCOPES: &[&str] = &["streaming"];

/// ReplayGain's reference is about −18 LUFS, Spotify's −14: streamed songs
/// are lowered by the difference so they're as loud as local ones.
const PREGAIN_DB: f64 = -4.0;
const SAMPLE_RATE: u64 = 44_100;
const CHANNELS: u64 = 2;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("GStreamer: {0}")]
    Gst(String),
}

/// A login librespot can reuse without the browser (kept in the keyring).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    pub username: String,
    data: Vec<u8>,
}

impl Credentials {
    pub fn to_json(&self) -> String {
        serde_json::json!({ "username": self.username, "data": BASE64.encode(&self.data) })
            .to_string()
    }

    pub fn from_json(text: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(text).ok()?;
        Some(Self {
            username: value["username"].as_str()?.to_owned(),
            data: BASE64.decode(value["data"].as_str()?).ok()?,
        })
    }
}

/// How to log in.
#[derive(Debug, Clone)]
pub enum Login {
    /// An access token with the `streaming` scope (first login).
    Token(String),
    Stored(Credentials),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    /// Logged in; store these to log in again without the browser.
    Connected(Credentials),
    ConnectFailed(String),
    Playing {
        position_ms: u64,
    },
    Paused {
        position_ms: u64,
    },
    EndOfTrack,
    /// The track can't be streamed (region, removed).
    Unavailable,
}

enum Command {
    Connect(Login, bool),
    Load(String, u32, bool),
    Preload(String),
    Play,
    Pause,
    Stop,
    Seek(u32),
}

/// Where the position is, from librespot's reports.
#[derive(Default)]
struct Clock {
    position_ms: u64,
    since: Option<Instant>,
}

impl Clock {
    fn now(&self) -> u64 {
        self.position_ms + self.since.map_or(0, |at| at.elapsed().as_millis() as u64)
    }
}

pub struct StreamPlayer {
    commands: mpsc::UnboundedSender<Command>,
    clock: Arc<Mutex<Clock>>,
    volume: gst::Element,
}

impl StreamPlayer {
    /// Starts the streaming thread and the output pipeline (a fakesink
    /// with `silent`, for tests). Nothing connects until `connect`.
    pub fn new(silent: bool) -> Result<(Self, async_channel::Receiver<StreamEvent>), Error> {
        gst::init().map_err(|e| Error::Gst(e.to_string()))?;
        let sink = if silent {
            "fakesink sync=true"
        } else {
            "autoaudiosink"
        };
        let pipeline = gst::parse::launch(&format!(
            "appsrc name=src format=time is-live=false block=true max-bytes=70560 \
               caps=audio/x-raw,format=F32LE,rate={SAMPLE_RATE},channels={CHANNELS},layout=interleaved \
             ! audioconvert ! audioresample ! volume name=volume ! {sink}"
        ))
        .map_err(|e| Error::Gst(e.to_string()))?
        .downcast::<gst::Pipeline>()
        .map_err(|_| Error::Gst("not a pipeline".into()))?;
        let appsrc = pipeline
            .by_name("src")
            .and_downcast::<gst_app::AppSrc>()
            .ok_or_else(|| Error::Gst("no appsrc".into()))?;
        let volume = pipeline
            .by_name("volume")
            .ok_or_else(|| Error::Gst("no volume".into()))?;

        let (commands, mut receiver) = mpsc::unbounded_channel();
        let (events, event_receiver) = async_channel::unbounded();
        let clock: Arc<Mutex<Clock>> = Arc::default();
        let thread_clock = clock.clone();
        std::thread::Builder::new()
            .name("spotify-stream".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(err) => {
                        let _ = events.send_blocking(StreamEvent::ConnectFailed(err.to_string()));
                        return;
                    }
                };
                runtime.block_on(run(&mut receiver, events, thread_clock, pipeline, appsrc));
            })
            .map_err(|e| Error::Gst(e.to_string()))?;
        Ok((
            Self {
                commands,
                clock,
                volume,
            },
            event_receiver,
        ))
    }

    /// Logs in; `normalise`: match local ReplayGain loudness.
    pub fn connect(&self, login: Login, normalise: bool) {
        let _ = self.commands.send(Command::Connect(login, normalise));
    }

    /// `uri`: `spotify:track:…`.
    pub fn load(&self, uri: &str, start_ms: u64, play: bool) {
        self.set_clock(start_ms, false);
        let _ = self.commands.send(Command::Load(
            uri.to_owned(),
            start_ms.min(u64::from(u32::MAX)) as u32,
            play,
        ));
    }

    /// Fetches the next track early, for a seamless change.
    pub fn preload(&self, uri: &str) {
        let _ = self.commands.send(Command::Preload(uri.to_owned()));
    }

    pub fn play(&self) {
        let _ = self.commands.send(Command::Play);
    }

    pub fn pause(&self) {
        let position = self.position_ms();
        self.set_clock(position, false);
        let _ = self.commands.send(Command::Pause);
    }

    pub fn stop(&self) {
        self.set_clock(0, false);
        let _ = self.commands.send(Command::Stop);
    }

    pub fn seek(&self, ms: u64) {
        let playing = self
            .clock
            .lock()
            .map(|c| c.since.is_some())
            .unwrap_or(false);
        self.set_clock(ms, playing);
        let _ = self
            .commands
            .send(Command::Seek(ms.min(u64::from(u32::MAX)) as u32));
    }

    /// 0.0 to 1.0, like local playback.
    pub fn set_volume(&self, volume: f64) {
        self.volume.set_property("volume", volume.clamp(0.0, 1.0));
    }

    pub fn position_ms(&self) -> u64 {
        self.clock.lock().map(|c| c.now()).unwrap_or(0)
    }

    fn set_clock(&self, ms: u64, playing: bool) {
        if let Ok(mut clock) = self.clock.lock() {
            clock.position_ms = ms;
            clock.since = playing.then(Instant::now);
        }
    }
}

/// The streaming thread: commands in, events out.
async fn run(
    commands: &mut mpsc::UnboundedReceiver<Command>,
    events: async_channel::Sender<StreamEvent>,
    clock: Arc<Mutex<Clock>>,
    pipeline: gst::Pipeline,
    appsrc: gst_app::AppSrc,
) {
    let mut player: Option<Arc<Player>> = None;
    let mut session: Option<Session> = None;
    while let Some(command) = commands.recv().await {
        let parse = |uri: &str| SpotifyUri::from_uri(uri).ok();
        match command {
            Command::Connect(login, normalise) => {
                if let Some(old) = session.take() {
                    old.shutdown();
                }
                player = None;
                let credentials = match login {
                    Login::Token(token) => LibrespotCredentials::with_access_token(token),
                    Login::Stored(stored) => LibrespotCredentials {
                        username: Some(stored.username),
                        auth_type: AuthenticationType::AUTHENTICATION_STORED_SPOTIFY_CREDENTIALS,
                        auth_data: stored.data,
                    },
                };
                let new_session = Session::new(SessionConfig::default(), None);
                match new_session.connect(credentials, false).await {
                    Ok(()) => {
                        let config = PlayerConfig {
                            normalisation: normalise,
                            normalisation_type: NormalisationType::Auto,
                            normalisation_pregain_db: PREGAIN_DB,
                            ..PlayerConfig::default()
                        };
                        let (sink_pipeline, sink_src) = (pipeline.clone(), appsrc.clone());
                        let new_player = Player::new(
                            config,
                            new_session.clone(),
                            Box::new(NoOpVolume),
                            move || Box::new(GstSink::new(sink_pipeline, sink_src)),
                        );
                        tokio::spawn(forward_events(
                            new_player.get_player_event_channel(),
                            events.clone(),
                            clock.clone(),
                        ));
                        let _ = events
                            .send(StreamEvent::Connected(Credentials {
                                username: new_session.username(),
                                data: new_session.auth_data(),
                            }))
                            .await;
                        player = Some(new_player);
                        session = Some(new_session);
                    }
                    Err(err) => {
                        let _ = events
                            .send(StreamEvent::ConnectFailed(err.to_string()))
                            .await;
                    }
                }
            }
            Command::Load(uri, start, play) => match (&player, parse(&uri)) {
                (Some(player), Some(uri)) => player.load(uri, play, start),
                _ => {
                    let _ = events.send(StreamEvent::Unavailable).await;
                }
            },
            Command::Preload(uri) => {
                if let (Some(player), Some(uri)) = (&player, parse(&uri)) {
                    player.preload(uri);
                }
            }
            Command::Play => {
                if let Some(player) = &player {
                    player.play();
                }
            }
            Command::Pause => {
                if let Some(player) = &player {
                    player.pause();
                }
            }
            Command::Stop => {
                if let Some(player) = &player {
                    player.stop();
                }
            }
            Command::Seek(ms) => {
                if let Some(player) = &player {
                    player.seek(ms);
                }
            }
        }
    }
    let _ = pipeline.set_state(gst::State::Null);
}

async fn forward_events(
    mut player_events: mpsc::UnboundedReceiver<PlayerEvent>,
    events: async_channel::Sender<StreamEvent>,
    clock: Arc<Mutex<Clock>>,
) {
    while let Some(event) = player_events.recv().await {
        let set = |ms: u32, playing: bool| {
            if let Ok(mut clock) = clock.lock() {
                clock.position_ms = u64::from(ms);
                clock.since = playing.then(Instant::now);
            }
        };
        let event = match event {
            PlayerEvent::Playing { position_ms, .. } => {
                set(position_ms, true);
                Some(StreamEvent::Playing {
                    position_ms: u64::from(position_ms),
                })
            }
            PlayerEvent::PositionCorrection { position_ms, .. }
            | PlayerEvent::Seeked { position_ms, .. } => {
                let playing = clock.lock().map(|c| c.since.is_some()).unwrap_or(false);
                set(position_ms, playing);
                None
            }
            PlayerEvent::Paused { position_ms, .. } => {
                set(position_ms, false);
                Some(StreamEvent::Paused {
                    position_ms: u64::from(position_ms),
                })
            }
            PlayerEvent::EndOfTrack { .. } => {
                if let Ok(mut clock) = clock.lock() {
                    clock.since = None;
                }
                Some(StreamEvent::EndOfTrack)
            }
            PlayerEvent::Unavailable { .. } => Some(StreamEvent::Unavailable),
            _ => None,
        };
        if let Some(event) = event
            && events.send(event).await.is_err()
        {
            break;
        }
    }
}

/// librespot's output: samples into the GStreamer pipeline.
struct GstSink {
    pipeline: gst::Pipeline,
    appsrc: gst_app::AppSrc,
    /// Samples (per channel) pushed so far, for the timestamps.
    frames: u64,
}

impl GstSink {
    fn new(pipeline: gst::Pipeline, appsrc: gst_app::AppSrc) -> Self {
        Self {
            pipeline,
            appsrc,
            frames: 0,
        }
    }
}

impl Sink for GstSink {
    fn start(&mut self) -> SinkResult<()> {
        self.pipeline
            .set_state(gst::State::Playing)
            .map(drop)
            .map_err(|e| SinkError::StateChange(e.to_string()))
    }

    fn stop(&mut self) -> SinkResult<()> {
        self.pipeline
            .set_state(gst::State::Paused)
            .map(drop)
            .map_err(|e| SinkError::StateChange(e.to_string()))
    }

    fn write(&mut self, packet: AudioPacket, converter: &mut Converter) -> SinkResult<()> {
        let AudioPacket::Samples(samples) = packet else {
            return Ok(());
        };
        let samples = converter.f64_to_f32(&samples);
        let frames = samples.len() as u64 / CHANNELS;
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut buffer = gst::Buffer::from_mut_slice(bytes);
        {
            let buffer = buffer.get_mut().expect("a new buffer");
            buffer.set_pts(gst::ClockTime::from_nseconds(
                self.frames * 1_000_000_000 / SAMPLE_RATE,
            ));
            buffer.set_duration(gst::ClockTime::from_nseconds(
                frames * 1_000_000_000 / SAMPLE_RATE,
            ));
        }
        self.frames += frames;
        self.appsrc
            .push_buffer(buffer)
            .map(drop)
            .map_err(|e| SinkError::OnWrite(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_round_trip() {
        let credentials = Credentials {
            username: "max".into(),
            data: vec![0, 1, 2, 250],
        };
        assert_eq!(
            Credentials::from_json(&credentials.to_json()),
            Some(credentials)
        );
        assert_eq!(Credentials::from_json("{}"), None);
    }

    #[test]
    fn the_clock_moves_only_while_playing() {
        let mut clock = Clock {
            position_ms: 1000,
            since: None,
        };
        assert_eq!(clock.now(), 1000);
        clock.since = Some(Instant::now() - std::time::Duration::from_millis(500));
        assert!(clock.now() >= 1500);
    }

    #[test]
    fn samples_reach_the_pipeline() {
        // The sink on its own: a second of silence into a fakesink.
        let (player, _events) = StreamPlayer::new(true).unwrap();
        player.set_volume(0.5);
        let pipeline = gst::parse::launch(
            "appsrc name=src format=time caps=audio/x-raw,format=F32LE,rate=44100,channels=2,layout=interleaved ! fakesink sync=false",
        )
        .unwrap()
        .downcast::<gst::Pipeline>()
        .unwrap();
        let appsrc = pipeline
            .by_name("src")
            .and_downcast::<gst_app::AppSrc>()
            .unwrap();
        let mut sink = GstSink::new(pipeline.clone(), appsrc);
        sink.start().unwrap();
        let mut converter = Converter::new(None);
        for _ in 0..10 {
            sink.write(AudioPacket::Samples(vec![0.0; 8820]), &mut converter)
                .unwrap();
        }
        assert_eq!(sink.frames, 44_100);
        sink.stop().unwrap();
        let _ = pipeline.set_state(gst::State::Null);
    }
}
