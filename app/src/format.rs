//! Formatting of durations and audio details for display.

use gettextrs::{gettext, ngettext};
use jewelcase_core::library::tags::TrackTags;

/// 3:12, or 1:02:03 for long tracks.
pub fn duration(ms: u64) -> String {
    let secs = ms / 1000;
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// "48 min" or "1 h 12 min", for album lengths.
pub fn length(ms: u64) -> String {
    let minutes = (ms + 30_000) / 60_000;
    if minutes >= 60 {
        // Translators: hours and minutes, e.g. "1 h 12 min".
        gettext("{h} h {m} min")
            .replace("{h}", &(minutes / 60).to_string())
            .replace("{m}", &(minutes % 60).to_string())
    } else {
        // Translators: a length in minutes, e.g. "48 min".
        gettext("{} min").replace("{}", &minutes.to_string())
    }
}

/// How long ago `then` (Unix seconds) was, by calendar day here: "today",
/// "yesterday", "3 days ago".
pub fn days_ago(then: i64) -> String {
    // Days since 1970 in local time.
    fn local_day(unix: i64) -> i64 {
        let offset = gtk::glib::DateTime::from_unix_local(unix)
            .map(|d| d.utc_offset().as_seconds())
            .unwrap_or(0);
        (unix + offset).div_euclid(86_400)
    }
    let days = local_day(crate::metadata::now()) - local_day(then);
    match days {
        ..=0 => gettext("today"),
        1 => gettext("yesterday"),
        n => ngettext("{} day ago", "{} days ago", n.min(u32::MAX as i64) as u32)
            .replace("{}", &n.to_string()),
    }
}

pub fn song_count(count: u32) -> String {
    ngettext("{} song", "{} songs", count).replace("{}", &count.to_string())
}

/// "FLAC · 16 bit · 44.1 kHz · 900 kbps"
pub fn audio_format(tags: &TrackTags) -> String {
    let mut parts = vec![tags.format.clone()];
    if let Some(bits) = tags.bit_depth {
        // Translators: audio bit depth, e.g. "24 bit".
        parts.push(gettext("{} bit").replace("{}", &bits.to_string()));
    }
    if let Some(rate) = tags.sample_rate {
        let khz = f64::from(rate) / 1000.0;
        let khz = if rate % 1000 == 0 {
            format!("{khz:.0}")
        } else {
            format!("{khz:.1}")
        };
        parts.push(format!("{khz} kHz"));
    }
    if let Some(bitrate) = tags.bitrate {
        parts.push(format!("{bitrate} kbps"));
    }
    parts.join(" · ")
}

/// The time label right of a seek bar: the length, or what's left ("-1:23").
pub fn end_time(position_ms: u64, duration_ms: u64, remaining: bool) -> String {
    if remaining {
        format!("-{}", duration(duration_ms.saturating_sub(position_ms)))
    } else {
        duration(duration_ms)
    }
}
