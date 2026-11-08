//! Audio playback with GStreamer's `playbin3` (CONCEPT.md §2).
//!
//! - **Gapless:** the next track's URI is handed to playbin in
//!   `about-to-finish`, so it starts without a gap. [`PlayerEvent::NextQueued`]
//!   says which queue item that was; the following
//!   [`PlayerEvent::StreamStarted`] means it is now audible.
//! - **ReplayGain:** `rgvolume ! rglimiter` as playbin's audio filter;
//!   ReplayGain tags win over R128 ones (`prefer_replaygain_tags`).
//!   Album or track gain can switch at any time; turning ReplayGain on or
//!   off applies from the next [`Player::load`].
//! - **Visualizer:** a `spectrum` element ends both filters (with and
//!   without ReplayGain), so it sees the levelled music, not the volume
//!   slider. It posts only while asked ([`Player::set_spectrum`]); frames
//!   wait in a short queue until the sink plays them
//!   ([`Player::spectrum_now`]), so the bars move with what's heard.
//!
//! - **M4A workaround:** after a seek inside an MP4 file, GStreamer's next
//!   gapless track starts only after a delay as long as the seek jump (seen
//!   with GStreamer 1.28, playbin and playbin3, any next format). So after
//!   such a seek the hand-over is skipped: the track ends with
//!   [`PlayerEvent::EndOfStream`] and the caller loads the next one.
//!
//! Bus messages are forwarded from GStreamer's threads through a channel,
//! so this module doesn't need a GLib main loop.

use std::sync::{Arc, Mutex};

use gst::prelude::*;

/// Extra gain on top of ReplayGain's −18 LUFS reference: +4 dB is about
/// Spotify's and most players' −14 LUFS, so local and Spotify songs play
/// equally loud. It applies to the fallback gain too.
pub const PREAMP_DB: f64 = 4.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerEvent {
    /// A new stream started playing (after `load` or a gapless switch).
    StreamStarted,
    /// The next item was handed to playbin for gapless playback.
    NextQueued(u64),
    /// A state change or seek finished; the position is valid now.
    AsyncDone,
    /// The stream's length is known now, or estimated anew.
    DurationChanged,
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

/// The next song's URI, queue entry and gain for songs without
/// ReplayGain tags (`fallback-gain`, see `set_fallback_gain`).
type NextTrack = Option<(String, u64, Option<f64>)>;

/// What decides whether the next track may start gaplessly.
#[derive(Debug, Default)]
struct GaplessState {
    current_is_mp4: bool,
    /// The current MP4 stream was seeked (see the module docs).
    seeked_mp4: bool,
}

fn is_mp4(uri: &str) -> bool {
    let lower = uri.to_lowercase();
    [".m4a", ".mp4", ".m4b", ".m4p"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

/// Bands the `spectrum` element measures (linear, up to half the rate).
const SPECTRUM_BANDS: u32 = 1024;
/// A frame every 40 ms: 25 a second.
const SPECTRUM_INTERVAL_MS: u64 = 40;
/// Quieter bands (dB) count as silence.
const SPECTRUM_THRESHOLD_DB: i32 = -80;
/// Frames waiting for the sink, at most (a few seconds).
const SPECTRUM_QUEUE: usize = 100;

/// One spectrum measurement.
#[derive(Debug, Clone, PartialEq)]
pub struct SpectrumFrame {
    /// When it's heard: the end of the measured stretch, as running time.
    running_time_ns: u64,
    /// dB per band, from 0 Hz to half the rate.
    pub magnitudes: Vec<f32>,
    pub rate: u32,
}

#[derive(Debug, Default)]
struct SpectrumQueue {
    frames: std::collections::VecDeque<SpectrumFrame>,
    /// The last frame due (kept, so several widgets can show it).
    current: Option<SpectrumFrame>,
}

pub struct Player {
    playbin: gst::Element,
    filter: gst::Element,
    /// Without ReplayGain: only the spectrum.
    plain_filter: gst::Element,
    spectrums: [gst::Element; 2],
    spectrum: Arc<Mutex<SpectrumQueue>>,
    latency_ns: Mutex<u64>,
    rgvolume: gst::Element,
    next: Arc<Mutex<NextTrack>>,
    gapless: Arc<Mutex<GaplessState>>,
    /// The gapless next song's gain, until its stream starts.
    pending_gain: Arc<Mutex<Option<f64>>>,
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

        let filter = gst::parse::bin_from_description(
            "rgvolume name=rgvolume ! rglimiter ! spectrum name=spectrum",
            true,
        )?
        .upcast::<gst::Element>();
        let plain_filter = gst::parse::bin_from_description("spectrum name=spectrum", true)?
            .upcast::<gst::Element>();
        let spectrum_of = |bin: &gst::Element| {
            let spectrum = bin
                .downcast_ref::<gst::Bin>()
                .and_then(|bin| bin.by_name("spectrum"))
                .ok_or_else(|| gst::glib::bool_error!("spectrum"))?;
            spectrum.set_property("bands", SPECTRUM_BANDS);
            spectrum.set_property("threshold", SPECTRUM_THRESHOLD_DB);
            spectrum.set_property("interval", SPECTRUM_INTERVAL_MS * 1_000_000);
            spectrum.set_property("post-messages", false);
            spectrum.set_property("message-phase", false);
            Ok::<_, gst::glib::BoolError>(spectrum)
        };
        let spectrums = [spectrum_of(&filter)?, spectrum_of(&plain_filter)?];
        let rgvolume = filter
            .downcast_ref::<gst::Bin>()
            .and_then(|bin| bin.by_name("rgvolume"))
            .ok_or_else(|| gst::glib::bool_error!("rgvolume"))?;
        prefer_replaygain_tags(&rgvolume);
        let pending_gain: Arc<Mutex<Option<f64>>> = Arc::default();
        set_gain_at_stream_start(&rgvolume, pending_gain.clone());
        rgvolume.set_property("pre-amp", PREAMP_DB);
        // Room above the peak for positive gains (the pre-amp, quiet
        // songs); rglimiter keeps the result from clipping.
        rgvolume.set_property("headroom", 6.0f64);
        playbin.set_property("audio-filter", &filter);

        let (tx, rx) = async_channel::unbounded();
        let bus = playbin.bus().expect("playbin has a bus");
        let bus_tx = tx.clone();
        let spectrum: Arc<Mutex<SpectrumQueue>> = Arc::default();
        let spectrum_queue = spectrum.clone();
        bus.set_sync_handler(move |_, message| {
            use gst::MessageView;
            if let MessageView::Element(element) = message.view()
                && let Some(frame) = element
                    .structure()
                    .filter(|s| s.name() == "spectrum")
                    .and_then(|s| spectrum_frame(s, message.src()))
            {
                if let Ok(mut queue) = spectrum_queue.lock() {
                    if queue.frames.len() >= SPECTRUM_QUEUE {
                        queue.frames.pop_front();
                    }
                    queue.frames.push_back(frame);
                }
                return gst::BusSyncReply::Drop;
            }
            let event = match message.view() {
                MessageView::StreamStart(_) => Some(PlayerEvent::StreamStarted),
                MessageView::AsyncDone(_) => Some(PlayerEvent::AsyncDone),
                MessageView::DurationChanged(_) => Some(PlayerEvent::DurationChanged),
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
        let gapless: Arc<Mutex<GaplessState>> = Arc::default();
        let next_slot = next.clone();
        let gapless_state = gapless.clone();
        let next_gain = pending_gain.clone();
        playbin.connect("about-to-finish", false, move |values| {
            let playbin = values[0].get::<gst::Element>().ok()?;
            let mut state = gapless_state.lock().ok()?;
            if state.seeked_mp4 {
                // Ends with EOS instead; the caller loads the next track.
                return None;
            }
            let next = next_slot.lock().ok()?.take();
            if let Some((uri, uid, gain)) = next {
                state.current_is_mp4 = is_mp4(&uri);
                // Set when its stream starts (see set_gain_at_stream_start).
                if let Ok(mut pending) = next_gain.lock() {
                    *pending = gain;
                }
                playbin.set_property("uri", &uri);
                let _ = tx.send_blocking(PlayerEvent::NextQueued(uid));
            }
            None
        });

        let player = Self {
            playbin,
            filter,
            plain_filter,
            spectrums,
            spectrum,
            latency_ns: Mutex::new(0),
            rgvolume,
            next,
            gapless,
            pending_gain,
            pending_seek_ms: Mutex::new(None),
            replaygain: Mutex::new(true),
        };
        Ok((player, rx))
    }

    /// Lets the player react to its own events; call it for every event
    /// before handling it.
    pub fn handle_event(&self, event: &PlayerEvent) {
        if *event == PlayerEvent::AsyncDone {
            // How far the sink plays behind: frames wait that long.
            let mut query = gst::query::Latency::new();
            if self.playbin.query(&mut query) {
                let (_, min, _) = query.result();
                if let Ok(mut latency) = self.latency_ns.lock() {
                    *latency = min.nseconds();
                }
            }
        }
        if *event == PlayerEvent::AsyncDone
            && let Some(ms) = self.pending_seek_ms.lock().ok().and_then(|mut s| s.take())
        {
            self.seek(ms);
        }
    }

    /// Starts `uri` from `start_ms`, playing or paused.
    pub fn load(&self, uri: &str, start_ms: u64, play: bool) {
        self.set_next(None);
        if let Ok(mut pending) = self.pending_gain.lock() {
            *pending = None;
        }
        let _ = self.playbin.set_state(gst::State::Ready);
        self.clear_spectrum();
        let replaygain = self.replaygain.lock().map_or(true, |r| *r);
        let filter = if replaygain {
            &self.filter
        } else {
            &self.plain_filter
        };
        self.playbin.set_property("audio-filter", filter);
        self.playbin.set_property("uri", uri);
        if let Ok(mut gapless) = self.gapless.lock() {
            *gapless = GaplessState {
                current_is_mp4: is_mp4(uri),
                seeked_mp4: false,
            };
        }
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
        if let Ok(mut gapless) = self.gapless.lock()
            && gapless.current_is_mp4
        {
            gapless.seeked_mp4 = true;
        }
        let flags = gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE;
        if let Err(err) = self
            .playbin
            .seek_simple(flags, gst::ClockTime::from_mseconds(ms))
        {
            tracing::debug!("Seek to {ms} ms failed: {err}");
        }
        // Running time starts again after a flushing seek (frames from
        // before it are also dropped in `spectrum_now`).
        self.clear_spectrum();
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

    /// The track to start gaplessly when the current one ends, with its
    /// fallback gain (set exactly when it starts).
    pub fn set_next(&self, next: Option<(String, u64, Option<f64>)>) {
        if let Ok(mut slot) = self.next.lock() {
            *slot = next;
        }
    }

    /// Measures the spectrum for the visualizer (off: no cost).
    pub fn set_spectrum(&self, enabled: bool) {
        for spectrum in &self.spectrums {
            spectrum.set_property("post-messages", enabled);
        }
        if !enabled {
            self.clear_spectrum();
        }
    }

    fn clear_spectrum(&self) {
        if let Ok(mut queue) = self.spectrum.lock() {
            *queue = SpectrumQueue::default();
        }
    }

    /// The spectrum of what's heard now (the newest frame the sink has
    /// played), if one is due.
    pub fn spectrum_now(&self) -> Option<SpectrumFrame> {
        let now = self.playbin.current_running_time()?.nseconds();
        let latency = self.latency_ns.lock().map_or(0, |l| *l);
        let heard = now.saturating_sub(latency);
        let mut queue = self.spectrum.lock().ok()?;
        // Far ahead of now: from before a seek; they'd hold back the rest.
        let ahead = heard.saturating_add(2_000_000_000);
        queue.frames.retain(|f| f.running_time_ns <= ahead);
        while queue
            .frames
            .front()
            .is_some_and(|f| f.running_time_ns <= heard)
        {
            queue.current = queue.frames.pop_front();
        }
        // Far older than now (a pause, a stall): nothing is heard.
        let fresh = queue.current.as_ref().is_some_and(|f| {
            heard.saturating_sub(f.running_time_ns) < 4 * SPECTRUM_INTERVAL_MS * 1_000_000
        });
        if fresh { queue.current.clone() } else { None }
    }

    pub fn set_replaygain(&self, enabled: bool) {
        if let Ok(mut replaygain) = self.replaygain.lock() {
            *replaygain = enabled;
        }
    }

    /// The gain (dB, before the pre-amp) for a song without ReplayGain
    /// tags: its measured one, or minus the pre-amp to play it as it is.
    pub fn set_fallback_gain(&self, db: f64) {
        self.rgvolume
            .set_property("fallback-gain", db.clamp(-60.0, 60.0));
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

/// A gapless next song's fallback gain is set when its stream starts in
/// rgvolume (streaming thread), before its first sample: set from the main
/// loop it came a moment late, and a song without tags after a tagged one
/// started unlevelled and then jumped.
fn set_gain_at_stream_start(rgvolume: &gst::Element, pending: Arc<Mutex<Option<f64>>>) {
    let Some(pad) = rgvolume.static_pad("sink") else {
        return;
    };
    pad.add_probe(gst::PadProbeType::EVENT_DOWNSTREAM, move |pad, info| {
        let Some(gst::PadProbeData::Event(event)) = &info.data else {
            return gst::PadProbeReturn::Ok;
        };
        if event.type_() == gst::EventType::StreamStart
            && let Some(gain) = pending.lock().ok().and_then(|mut p| p.take())
            && let Some(rgvolume) = pad.parent_element()
        {
            rgvolume.set_property("fallback-gain", gain.clamp(-60.0, 60.0));
        }
        gst::PadProbeReturn::Ok
    });
}

/// rgvolume prefers R128 gain tags over ReplayGain ones. When a file has
/// both, the ReplayGain ones are used: R128 tags belong to Opus, and
/// cyanrip writes them into FLAC files 10 dB off (its ReplayGain gain
/// plus 5 dB instead of minus).
fn prefer_replaygain_tags(rgvolume: &gst::Element) {
    const R128: [&str; 2] = ["r128-track-gain", "r128-album-gain"];
    let Some(pad) = rgvolume.static_pad("sink") else {
        return;
    };
    pad.add_probe(gst::PadProbeType::EVENT_DOWNSTREAM, |_, info| {
        let Some(gst::PadProbeData::Event(event)) = &mut info.data else {
            return gst::PadProbeReturn::Ok;
        };
        let gst::EventView::Tag(tag) = event.view() else {
            return gst::PadProbeReturn::Ok;
        };
        let tags = tag.tag();
        let has_replaygain = tags.generic("replaygain-track-gain").is_some()
            || tags.generic("replaygain-album-gain").is_some();
        if !has_replaygain || R128.iter().all(|name| tags.generic(*name).is_none()) {
            return gst::PadProbeReturn::Ok;
        }
        let mut cleaned = tags.to_owned();
        {
            let cleaned = cleaned.make_mut();
            for name in R128 {
                cleaned.remove_generic(name);
            }
        }
        let seqnum = event.seqnum();
        *event = gst::event::Tag::builder(cleaned).seqnum(seqnum).build();
        gst::PadProbeReturn::Ok
    });
}

/// A `spectrum` message as a frame (the rate from the element's caps).
fn spectrum_frame(
    structure: &gst::StructureRef,
    source: Option<&gst::Object>,
) -> Option<SpectrumFrame> {
    let end = structure
        .get::<u64>("running-time")
        .ok()?
        .saturating_add(structure.get::<u64>("duration").unwrap_or(0));
    let magnitudes = structure
        .get::<gst::List>("magnitude")
        .ok()?
        .iter()
        .filter_map(|v| v.get::<f32>().ok())
        .collect::<Vec<f32>>();
    let rate = source
        .and_then(|s| s.downcast_ref::<gst::Element>())
        .and_then(|e| e.static_pad("sink"))
        .and_then(|pad| pad.current_caps())
        .and_then(|caps| caps.structure(0)?.get::<i32>("rate").ok())
        .unwrap_or(44_100);
    Some(SpectrumFrame {
        running_time_ns: end,
        magnitudes,
        rate: rate.max(1) as u32,
    })
}

/// `count` bars from a frame, low to high, each 0 (silent) to 1 (loud):
/// log-spaced from 50 Hz to 16 kHz, each the loudest band in its range,
/// tilted against music's falling highs (+5 dB an octave above 1 kHz,
/// −3 below) so all bars move. The scale fits ReplayGain'd music
/// (measured on the reference library: about −45 dB in a typical bar).
pub fn spectrum_bars(frame: &SpectrumFrame, count: usize) -> Vec<f32> {
    const LOW_HZ: f32 = 50.0;
    const HIGH_HZ: f32 = 16_000.0;
    const FLOOR_DB: f32 = -62.0;
    const CEILING_DB: f32 = -30.0;
    let n = frame.magnitudes.len();
    if n == 0 || count == 0 {
        return vec![0.0; count];
    }
    let nyquist = frame.rate as f32 / 2.0;
    let band_hz = nyquist / n as f32;
    let high = HIGH_HZ.min(nyquist);
    let ratio = (high / LOW_HZ).powf(1.0 / count as f32);
    (0..count)
        .map(|i| {
            let lo = LOW_HZ * ratio.powi(i as i32);
            let hi = lo * ratio;
            let first = ((lo / band_hz) as usize).min(n - 1);
            let last = ((hi / band_hz).ceil() as usize).clamp(first + 1, n);
            let db = frame.magnitudes[first..last]
                .iter()
                .copied()
                .fold(f32::MIN, f32::max);
            // At the threshold: silence, whatever the tilt.
            if db <= SPECTRUM_THRESHOLD_DB as f32 + 1.0 {
                return 0.0;
            }
            let center = (lo * hi).sqrt();
            let octaves = (center / 1000.0).log2();
            let tilted = db + octaves * if octaves > 0.0 { 5.0 } else { 3.0 };
            ((tilted - FLOOR_DB) / (CEILING_DB - FLOOR_DB)).clamp(0.0, 1.0)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spectrum_bars_low_to_high() {
        // A loud band at about 100 Hz, quiet elsewhere.
        let mut magnitudes = vec![-80.0f32; 1024];
        let band_hz = 48_000.0 / 2.0 / 1024.0;
        magnitudes[(100.0 / band_hz) as usize] = -15.0;
        let frame = SpectrumFrame {
            running_time_ns: 0,
            magnitudes,
            rate: 48_000,
        };
        let bars = spectrum_bars(&frame, 32);
        assert_eq!(bars.len(), 32);
        let loudest = bars
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        // 100 Hz is one octave above 50 Hz, of about 8.3 octaves.
        assert!((2..=5).contains(&loudest), "loudest bar {loudest}");
        assert!(bars.iter().all(|b| (0.0..=1.0).contains(b)));
        assert_eq!(bars[31], 0.0);
        // Nothing measured: silent bars.
        let empty = SpectrumFrame {
            running_time_ns: 0,
            magnitudes: Vec::new(),
            rate: 48_000,
        };
        assert_eq!(spectrum_bars(&empty, 5), vec![0.0; 5]);
    }

    /// rgvolume's target gain after a tag event with these tags.
    fn target_gain(tags: &[(&str, f64)]) -> Option<f64> {
        gst::init().ok()?;
        // Not installed (e.g. in a build container): nothing to check.
        let rgvolume = gst::ElementFactory::make("rgvolume").build().ok()?;
        prefer_replaygain_tags(&rgvolume);
        rgvolume.set_property("pre-amp", 0.0f64);
        rgvolume.set_state(gst::State::Paused).ok()?;
        let pad = rgvolume.static_pad("sink")?;
        pad.send_event(gst::event::StreamStart::new("test"));
        let caps = gst::Caps::builder("audio/x-raw")
            .field("format", "F32LE")
            .field("layout", "interleaved")
            .field("rate", 48_000i32)
            .field("channels", 2i32)
            .build();
        pad.send_event(gst::event::Caps::new(&caps));
        let mut list = gst::TagList::new();
        {
            let list = list.get_mut().unwrap();
            for (name, value) in tags {
                list.add_generic(*name, value, gst::TagMergeMode::Append)
                    .unwrap();
            }
        }
        pad.send_event(gst::event::Tag::new(list));
        let gain = rgvolume.property::<f64>("target-gain");
        let _ = rgvolume.set_state(gst::State::Null);
        Some(gain)
    }

    #[test]
    fn replaygain_tags_win_over_r128_ones() {
        // LALALALA as cyanrip tagged it: R128 10 dB off.
        let Some(both) = target_gain(&[
            ("replaygain-track-gain", -11.79),
            ("replaygain-track-peak", 0.99),
            ("r128-track-gain", -6.79),
        ]) else {
            return;
        };
        assert!((both - -11.79).abs() < 0.01, "{both}");
        // Only R128 (Opus): still used, as ReplayGain (+5 dB).
        let r128 = target_gain(&[("r128-track-gain", -16.79)]).unwrap();
        assert!((r128 - -11.79).abs() < 0.01, "{r128}");
    }

    #[test]
    fn a_gapless_songs_gain_is_set_when_its_stream_starts() {
        if gst::init().is_err() {
            return;
        }
        let Ok(rgvolume) = gst::ElementFactory::make("rgvolume").build() else {
            return;
        };
        let pending: Arc<Mutex<Option<f64>>> = Arc::default();
        set_gain_at_stream_start(&rgvolume, pending.clone());
        rgvolume.set_property("fallback-gain", -4.0f64);
        *pending.lock().unwrap() = Some(2.5);
        assert_eq!(rgvolume.property::<f64>("fallback-gain"), -4.0);
        rgvolume.set_state(gst::State::Paused).unwrap();
        let pad = rgvolume.static_pad("sink").unwrap();
        pad.send_event(gst::event::StreamStart::new("next"));
        assert_eq!(rgvolume.property::<f64>("fallback-gain"), 2.5);
        assert_eq!(*pending.lock().unwrap(), None, "used once");
        let _ = rgvolume.set_state(gst::State::Null);
    }
}
