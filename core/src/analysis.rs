//! Song analysis for smart shuffle (CONCEPT.md §7): a bliss feature vector
//! for "sounds similar" and the tempo in BPM, once per file content.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

use bliss_audio::Song;
use bliss_audio::decoder::PreAnalyzedSong;
use gst::prelude::*;
use gst_app::AppSink;

/// What one song's analysis found.
#[derive(Debug, Clone, PartialEq)]
pub struct SongAnalysis {
    /// bliss features (`features_version` of bliss), for distances.
    pub features: Vec<f32>,
    pub features_version: u8,
    /// Detected tempo; `None` if the detector had no answer.
    pub bpm: Option<f64>,
    /// Where the music starts and ends, for smart crossfades.
    pub edges: Option<Edges>,
}

/// Where a song's music is, from its loudness over time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edges {
    /// The silence at the start ends here.
    pub lead_in_ms: u64,
    /// The song gets quiet here: a fade-out or quiet ending starts.
    pub outro_ms: u64,
    /// The music is over here (silence follows).
    pub end_ms: u64,
}

/// 50 ms of samples.
const WINDOW: usize = SAMPLE_RATE as usize / 20;

/// Finds a song's edges: silence is 40 dB below its typical loudness
/// (the 90th percentile of 50 ms windows), "quiet" 15 dB below, judged
/// over one-second stretches so short breaks don't count.
pub fn edges(samples: &[f32]) -> Option<Edges> {
    let rms: Vec<f32> = samples
        .chunks(WINDOW)
        .map(|w| (w.iter().map(|s| s * s).sum::<f32>() / w.len().max(1) as f32).sqrt())
        .collect();
    if rms.len() < 40 {
        return None;
    }
    let mut sorted = rms.clone();
    sorted.sort_by(f32::total_cmp);
    let reference = sorted[sorted.len() * 9 / 10];
    if reference <= 1e-5 {
        return None;
    }
    let silence = reference * 0.01; // −40 dB
    let quiet = reference * 0.178; // −15 dB
    let ms = |windows: usize| (windows * 50) as u64;
    let lead_in = rms.iter().position(|r| *r > silence)?;
    let end = rms.iter().rposition(|r| *r > silence)? + 1;
    // One-second averages (20 windows) for the quiet ending.
    let smooth: Vec<f32> = (0..rms.len())
        .map(|i| {
            let window = &rms[i.saturating_sub(19)..=i];
            window.iter().sum::<f32>() / window.len() as f32
        })
        .collect();
    let outro = smooth[..end]
        .iter()
        .rposition(|r| *r > quiet)
        .map_or(end, |i| i + 1)
        .min(end);
    Some(Edges {
        lead_in_ms: ms(lead_in),
        outro_ms: ms(outro),
        end_ms: ms(end),
    })
}

/// The bliss feature version results are stored with; older ones are
/// analyzed again.
pub const FEATURES_VERSION: u8 = 3;

/// Identifies a file's audio independently of its path, so moved or
/// renamed files (and identical files in merged editions) are analyzed
/// once. Samples three 64 KiB blocks in the middle of the file, where the
/// audio is, so tag edits that don't move the audio keep the key.
pub fn content_key(path: &Path) -> std::io::Result<String> {
    const BLOCK: u64 = 64 * 1024;
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let mut hash = Fnv64::new();
    if len <= 4 * BLOCK {
        let mut all = Vec::new();
        file.read_to_end(&mut all)?;
        hash.write(&all);
    } else {
        let mut buffer = vec![0; BLOCK as usize];
        for percent in [30, 50, 70] {
            file.seek(SeekFrom::Start(len / 100 * percent))?;
            file.read_exact(&mut buffer)?;
            hash.write(&buffer);
        }
    }
    Ok(format!("{:016x}", hash.finish()))
}

/// Analyzes one file; slow (decodes the whole song), run it on a worker
/// thread. Decodes with GStreamer like playback, so whatever plays can be
/// analyzed; `bpmdetect` listens to the same samples bliss gets.
pub fn analyze(path: &Path) -> Result<SongAnalysis, String> {
    let (samples, bpm) = decode(path)?;
    if samples.len() < SAMPLE_RATE as usize {
        return Err("less than a second of audio".into());
    }
    let duration = Duration::from_secs_f64(samples.len() as f64 / f64::from(SAMPLE_RATE));
    let edges = edges(&samples);
    let song = Song::try_from(PreAnalyzedSong {
        path: path.to_path_buf(),
        duration,
        sample_array: samples,
        ..PreAnalyzedSong::default()
    })
    .map_err(|e| e.to_string())?;
    Ok(SongAnalysis {
        edges,
        features: song.analysis.as_vec(),
        features_version: FEATURES_VERSION,
        bpm: bpm
            .filter(|b| (30.0..=300.0).contains(b))
            .map(|b| (b * 10.0).round() / 10.0),
    })
}

/// What bliss expects: mono f32 at 22050 Hz.
const SAMPLE_RATE: u32 = 22_050;

/// Up to 20 minutes are analyzed; longer pieces are cut there.
const MAX_SAMPLES: usize = SAMPLE_RATE as usize * 60 * 20;

/// The song as mono 22050 Hz samples, and the tempo `bpmdetect` found.
fn decode(path: &Path) -> Result<(Vec<f32>, Option<f64>), String> {
    let uri = gst::glib::filename_to_uri(path, None).map_err(|e| e.to_string())?;
    let pipeline = gst::parse::launch(&format!(
        "uridecodebin3 uri=\"{uri}\" ! audioconvert ! audioresample \
         ! audio/x-raw,format=F32LE,channels=1,rate={SAMPLE_RATE} \
         ! bpmdetect ! appsink name=sink sync=false"
    ))
    .map_err(|e| e.to_string())?
    .downcast::<gst::Pipeline>()
    .map_err(|_| "not a pipeline".to_string())?;
    let sink = pipeline
        .by_name("sink")
        .and_downcast::<AppSink>()
        .ok_or("no sink")?;
    let bus = pipeline.bus().ok_or("no bus")?;
    pipeline
        .set_state(gst::State::Playing)
        .map_err(|e| e.to_string())?;

    let mut samples: Vec<f32> = Vec::new();
    let mut bpm = None;
    let mut error = None;
    let mut idle = 0;
    loop {
        match sink.try_pull_sample(gst::ClockTime::from_mseconds(100)) {
            Some(sample) => {
                idle = 0;
                if let Some(buffer) = sample.buffer()
                    && let Ok(map) = buffer.map_readable()
                {
                    samples.extend(
                        map.as_slice()
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .map(|c| f32::from_le_bytes(*c)),
                    );
                }
            }
            None if sink.is_eos() => break,
            // Samples arrive much faster than real time; give up when
            // nothing comes for a while.
            None => {
                idle += 1;
                if idle > 100 {
                    error = Some("decoding stalled".to_string());
                }
            }
        }
        while let Some(message) = bus.pop() {
            match message.view() {
                gst::MessageView::Tag(tag) => {
                    if let Some(value) = tag.tags().get::<gst::tags::BeatsPerMinute>() {
                        bpm = Some(value.get());
                    }
                }
                gst::MessageView::Error(err) => error = Some(err.error().to_string()),
                _ => {}
            }
        }
        if error.is_some() || samples.len() > MAX_SAMPLES {
            break;
        }
    }
    let _ = pipeline.set_state(gst::State::Null);
    match error {
        Some(error) if samples.len() < SAMPLE_RATE as usize * 10 => Err(error),
        _ => Ok((samples, bpm)),
    }
}

/// Pauses between songs so the analysis stays in the background.
pub const PAUSE_BETWEEN: Duration = Duration::from_millis(200);

/// 64-bit FNV-1a: stable across runs and versions (unlike std's hasher).
struct Fnv64(u64);

impl Fnv64 {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

/// Packs features for the database (little-endian f32).
pub fn features_to_blob(features: &[f32]) -> Vec<u8> {
    features.iter().flat_map(|f| f.to_le_bytes()).collect()
}

pub fn features_from_blob(blob: &[u8]) -> Vec<f32> {
    blob.as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_key_ignores_the_path_and_sees_the_content() {
        let dir = tempfile::tempdir().unwrap();
        let data: Vec<u8> = (0..600_000u32).map(|i| (i % 251) as u8).collect();
        let a = dir.path().join("a.flac");
        let b = dir.path().join("moved.flac");
        std::fs::write(&a, &data).unwrap();
        std::fs::write(&b, &data).unwrap();
        assert_eq!(content_key(&a).unwrap(), content_key(&b).unwrap());
        let mut other = data.clone();
        other[300_000] ^= 0xff;
        std::fs::write(&b, &other).unwrap();
        assert_ne!(content_key(&a).unwrap(), content_key(&b).unwrap());
    }

    #[test]
    fn edges_find_silence_and_the_fade_out() {
        let rate = SAMPLE_RATE as usize;
        let tone = |seconds: f32, amplitude: f32| -> Vec<f32> {
            (0..(seconds * rate as f32) as usize)
                .map(|i| amplitude * (i as f32 * 0.05).sin())
                .collect()
        };
        // 1 s silence, 20 s music, a 4 s fade-out, 2 s silence.
        let mut samples = vec![0.0; rate];
        samples.extend(tone(20.0, 0.5));
        samples.extend((0..4 * rate).map(|i| {
            let fade = 1.0 - i as f32 / (4 * rate) as f32;
            0.5 * fade * fade * (i as f32 * 0.05).sin()
        }));
        samples.extend(vec![0.0; 2 * rate]);
        let found = edges(&samples).unwrap();
        assert!((950..=1050).contains(&found.lead_in_ms), "{found:?}");
        // Quiet (−15 dB) well into the fade, the end close to its end.
        assert!((21_500..=24_500).contains(&found.outro_ms), "{found:?}");
        assert!((24_000..=25_100).contains(&found.end_ms), "{found:?}");
        assert!(found.outro_ms < found.end_ms);
        assert_eq!(edges(&vec![0.0; rate * 5]), None);
    }

    #[test]
    fn features_round_trip_through_blobs() {
        let features = vec![0.5, -1.25, 3.0];
        assert_eq!(features_from_blob(&features_to_blob(&features)), features);
    }
}
