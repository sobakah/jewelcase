//! Audio playback with GStreamer's `playbin3` (CONCEPT.md §2).
//!
//! - **Gapless:** the next track's URI is handed to playbin in
//!   `about-to-finish`, so it starts without a gap. [`PlayerEvent::NextQueued`]
//!   says which queue item that was; the following
//!   [`PlayerEvent::StreamStarted`] means it is now audible.
//! - **ReplayGain:** `rgvolume ! rglimiter` as playbin's audio filter.
//!   Album or track gain can switch at any time; turning ReplayGain on or
//!   off applies from the next [`Player::load`].
//!
//! Bus messages are forwarded from GStreamer's threads through a channel,
//! so this module doesn't need a GLib main loop.

use std::sync::{Arc, Mutex};

use gst::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerEvent {
    /// A new stream started playing (after `load` or a gapless switch).
    StreamStarted,
    /// The next item was handed to playbin for gapless playback.
    NextQueued(u64),
    /// A state change or seek finished; the position is valid now.
    AsyncDone,
    EndOfStream,
    Error(String),
}

#[derive(Debug, thiserror::Error)]
pub enum PlaybackError {
    #[error("GStreamer is not available: {0}")]
    Init(#[from] gst::glib::Error),
    #[error("a GStreamer element is missing: {0}")]
    MissingElement(#[from] gst::glib::BoolError),
}

/// The URI and queue item uid of the track to play next.
type NextTrack = Option<(String, u64)>;

pub struct Player {
    playbin: gst::Element,
    filter: gst::Element,
    rgvolume: gst::Element,
    next: Arc<Mutex<NextTrack>>,
    pending_seek_ms: Mutex<Option<u64>>,
    replaygain: Mutex<bool>,
}

impl Player {
    pub fn new() -> Result<(Self, async_channel::Receiver<PlayerEvent>), PlaybackError> {
        gst::init()?;
        let playbin = gst::ElementFactory::make("playbin3").build()?;
        // Audio only: no video sinks for cover art streams.
        playbin.set_property_from_str("flags", "audio+soft-volume");
        // GATEFOLD_SILENT=1 plays into a fakesink (in real time) for tests.
        if std::env::var_os("GATEFOLD_SILENT").is_some() {
            let sink = gst::ElementFactory::make("fakesink")
                .property("sync", true)
                .build()?;
            playbin.set_property("audio-sink", &sink);
        }

        let filter = gst::parse::bin_from_description("rgvolume name=rgvolume ! rglimiter", true)?
            .upcast::<gst::Element>();
        let rgvolume = filter
            .downcast_ref::<gst::Bin>()
            .and_then(|bin| bin.by_name("rgvolume"))
            .ok_or_else(|| gst::glib::bool_error!("rgvolume"))?;
        playbin.set_property("audio-filter", &filter);

        let (tx, rx) = async_channel::unbounded();
        let bus = playbin.bus().expect("playbin has a bus");
        let bus_tx = tx.clone();
        bus.set_sync_handler(move |_, message| {
            use gst::MessageView;
            let event = match message.view() {
                MessageView::StreamStart(_) => Some(PlayerEvent::StreamStarted),
                MessageView::AsyncDone(_) => Some(PlayerEvent::AsyncDone),
                MessageView::Eos(_) => Some(PlayerEvent::EndOfStream),
                MessageView::Error(err) => Some(PlayerEvent::Error(err.error().to_string())),
                _ => None,
            };
            if let Some(event) = event {
                let _ = bus_tx.send_blocking(event);
            }
            gst::BusSyncReply::Drop
        });

        let next: Arc<Mutex<NextTrack>> = Arc::default();
        let next_slot = next.clone();
        playbin.connect("about-to-finish", false, move |values| {
            let playbin = values[0].get::<gst::Element>().ok()?;
            let next = next_slot.lock().ok()?.take();
            if let Some((uri, uid)) = next {
                playbin.set_property("uri", &uri);
                let _ = tx.send_blocking(PlayerEvent::NextQueued(uid));
            }
            None
        });

        let player = Self {
            playbin,
            filter,
            rgvolume,
            next,
            pending_seek_ms: Mutex::new(None),
            replaygain: Mutex::new(true),
        };
        Ok((player, rx))
    }

    /// Lets the player react to its own events; call it for every event
    /// before handling it.
    pub fn handle_event(&self, event: &PlayerEvent) {
        if *event == PlayerEvent::AsyncDone
            && let Some(ms) = self.pending_seek_ms.lock().ok().and_then(|mut s| s.take())
        {
            self.seek(ms);
        }
    }

    /// Starts `uri` from `start_ms`, playing or paused.
    pub fn load(&self, uri: &str, start_ms: u64, play: bool) {
        self.set_next(None);
        let _ = self.playbin.set_state(gst::State::Ready);
        let replaygain = self.replaygain.lock().map_or(true, |r| *r);
        let filter = replaygain.then_some(&self.filter);
        self.playbin.set_property("audio-filter", filter);
        self.playbin.set_property("uri", uri);
        if let Ok(mut pending) = self.pending_seek_ms.lock() {
            *pending = (start_ms > 0).then_some(start_ms);
        }
        self.set_state(if play {
            gst::State::Playing
        } else {
            gst::State::Paused
        });
    }

    pub fn play(&self) {
        self.set_state(gst::State::Playing);
    }

    pub fn pause(&self) {
        self.set_state(gst::State::Paused);
    }

    pub fn stop(&self) {
        self.set_next(None);
        self.set_state(gst::State::Ready);
    }

    fn set_state(&self, state: gst::State) {
        if let Err(err) = self.playbin.set_state(state) {
            tracing::warn!("Cannot switch playback to {state:?}: {err}");
        }
    }

    pub fn seek(&self, ms: u64) {
        let flags = gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE;
        if let Err(err) = self
            .playbin
            .seek_simple(flags, gst::ClockTime::from_mseconds(ms))
        {
            tracing::debug!("Seek to {ms} ms failed: {err}");
        }
    }

    pub fn position_ms(&self) -> Option<u64> {
        if let Some(pending) = self.pending_seek_ms.lock().ok().and_then(|p| *p) {
            return Some(pending);
        }
        self.playbin
            .query_position::<gst::ClockTime>()
            .map(|t| t.mseconds())
    }

    pub fn duration_ms(&self) -> Option<u64> {
        self.playbin
            .query_duration::<gst::ClockTime>()
            .map(|t| t.mseconds())
    }

    /// The track to start gaplessly when the current one ends.
    pub fn set_next(&self, next: Option<(String, u64)>) {
        if let Ok(mut slot) = self.next.lock() {
            *slot = next;
        }
    }

    pub fn set_replaygain(&self, enabled: bool) {
        if let Ok(mut replaygain) = self.replaygain.lock() {
            *replaygain = enabled;
        }
    }

    /// Album gain (album context) or track gain (shuffle).
    pub fn set_album_gain(&self, album: bool) {
        self.rgvolume.set_property("album-mode", album);
    }

    /// Linear volume, 0.0–1.0.
    pub fn set_volume(&self, volume: f64) {
        self.playbin.set_property("volume", volume.clamp(0.0, 1.0));
    }

    pub fn volume(&self) -> f64 {
        self.playbin.property("volume")
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.playbin.set_state(gst::State::Null);
    }
}

/// A `file://` URI for a local path.
pub fn file_uri(path: &std::path::Path) -> Option<String> {
    gst::glib::filename_to_uri(path, None)
        .ok()
        .map(|u| u.to_string())
}
