//! LRC parsing and dual-line pairing (CONCEPT.md §5).

use unicode_script::{Script, UnicodeScript};

/// Lines with timestamps this close belong together (original + romanization).
const PAIR_TOLERANCE_MS: u64 = 50;

/// One lyric line, with an optional second line (romanization).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// `None` for unsynced lyrics.
    pub time_ms: Option<u64>,
    /// The original (non-Latin script wins when paired).
    pub primary: String,
    /// The romanization, if the lyrics have one for this line.
    pub secondary: Option<String>,
}

/// A parsed LRC line before pairing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimedLine {
    pub time_ms: u64,
    pub text: String,
}

/// Whether text contains LRC line timestamps (`[mm:ss.xx]`).
pub fn has_timestamps(text: &str) -> bool {
    text.lines().any(|line| {
        let line = line.trim_start();
        line.strip_prefix('[')
            .and_then(|rest| rest.split_once(']'))
            .is_some_and(|(tag, _)| parse_time(tag).is_some())
    })
}

/// Parses LRC text into timed lines, sorted by time. Metadata tags are
/// skipped, `[offset:…]` is applied, a line with several timestamps is
/// repeated, and enhanced-LRC word stamps (`<mm:ss.xx>`) are removed.
pub fn parse(text: &str) -> Vec<TimedLine> {
    let offset = text
        .lines()
        .filter_map(|line| line.trim().strip_prefix("[offset:"))
        .filter_map(|rest| rest.strip_suffix(']'))
        .find_map(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(0);

    let mut lines = Vec::new();
    for raw in text.lines() {
        let mut rest = raw.trim();
        let mut times = Vec::new();
        while let Some(after) = rest.strip_prefix('[') {
            let Some((tag, tail)) = after.split_once(']') else {
                break;
            };
            match parse_time(tag) {
                Some(ms) => times.push(ms),
                // [ar:…], [ti:…], [offset:…]: metadata, not lyrics.
                None if tag.contains(':') => {}
                None => break,
            }
            rest = tail.trim_start();
        }
        let text = strip_word_stamps(rest).trim().to_owned();
        for ms in times {
            // A positive offset shows lyrics earlier.
            let time_ms = (ms as i64 - offset).max(0) as u64;
            lines.push(TimedLine {
                time_ms,
                text: text.clone(),
            });
        }
    }
    // Stable: lines with equal timestamps keep file order (original first).
    lines.sort_by_key(|l| l.time_ms);
    lines
}

/// `mm:ss`, `mm:ss.xx` or `mm:ss.xxx` (also `:` before the fraction).
fn parse_time(tag: &str) -> Option<u64> {
    let (minutes, rest) = tag.split_once(':')?;
    let minutes: u64 = minutes.trim().parse().ok()?;
    let (seconds, fraction) = match rest.split_once(['.', ':']) {
        Some((s, f)) => (s, f),
        None => (rest, ""),
    };
    let seconds: u64 = seconds.trim().parse().ok()?;
    if seconds >= 60 || !fraction.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let fraction_ms = match fraction.len() {
        0 => 0,
        1 => fraction.parse::<u64>().ok()? * 100,
        2 => fraction.parse::<u64>().ok()? * 10,
        _ => fraction[..3].parse::<u64>().ok()?,
    };
    Some((minutes * 60 + seconds) * 1000 + fraction_ms)
}

fn strip_word_stamps(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        match rest[open..].find('>') {
            Some(close) if parse_time(&rest[open + 1..open + close]).is_some() => {
                rest = &rest[open + close + 1..];
            }
            _ => {
                out.push('<');
                rest = &rest[open + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Pairs lines with (nearly) the same timestamp: lyrembed's "below"
/// layout and CJK LRC files write the romanization as a second line with
/// the same timestamp. Non-Latin text becomes the primary line.
pub fn pair(lines: Vec<TimedLine>) -> Vec<Line> {
    let mut paired: Vec<Line> = Vec::with_capacity(lines.len());
    let mut iter = lines.into_iter().peekable();
    while let Some(first) = iter.next() {
        let second = iter
            .next_if(|next| {
                next.time_ms - first.time_ms <= PAIR_TOLERANCE_MS
                    && !next.text.is_empty()
                    && !first.text.is_empty()
            })
            .map(|second| second.text);
        let (primary, secondary) = match second {
            Some(second) if is_latin(&first.text) && !is_latin(&second) => {
                (second, Some(first.text))
            }
            Some(second) => (first.text, Some(second)),
            None => (first.text, None),
        };
        paired.push(Line {
            time_ms: Some(first.time_ms),
            primary,
            secondary,
        });
    }
    paired
}

/// Merges a separate romanization (sidecar or second frame) into `lines`
/// by timestamp.
pub fn merge_romanization(mut lines: Vec<Line>, romanized: &[TimedLine]) -> Vec<Line> {
    for line in lines.iter_mut().filter(|l| l.secondary.is_none()) {
        let Some(time) = line.time_ms else { continue };
        // The nearest one, skipping the original itself (a romanized file
        // can hold both lines at the same time).
        if let Some(r) = romanized
            .iter()
            .filter(|r| r.time_ms.abs_diff(time) <= PAIR_TOLERANCE_MS)
            .filter(|r| !r.text.is_empty() && r.text != line.primary)
            .min_by_key(|r| r.time_ms.abs_diff(time))
        {
            line.secondary = Some(r.text.clone());
        }
    }
    lines
}

/// Plain text lyrics: one unsynced line per text line.
pub fn plain(text: &str) -> Vec<Line> {
    text.lines()
        .map(|line| Line {
            time_ms: None,
            primary: line.trim().to_owned(),
            secondary: None,
        })
        .collect()
}

/// Only Latin letters (digits, punctuation and spaces don't count). A
/// mixed line like "사랑해 love" is the original, not a romanization.
pub fn is_latin(text: &str) -> bool {
    let mut other = 0;
    for c in text.chars().filter(|c| c.is_alphabetic()) {
        match c.script() {
            Script::Latin | Script::Common | Script::Inherited => {}
            _ => other += 1,
        }
    }
    other == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timed(time_ms: u64, text: &str) -> TimedLine {
        TimedLine {
            time_ms,
            text: text.into(),
        }
    }

    #[test]
    fn timestamps() {
        assert_eq!(parse_time("01:02.34"), Some(62_340));
        assert_eq!(parse_time("01:02.345"), Some(62_345));
        assert_eq!(parse_time("01:02"), Some(62_000));
        assert_eq!(parse_time("1:02:5"), Some(62_500));
        assert_eq!(parse_time("ar:BTS"), None);
        assert_eq!(parse_time("00:75.00"), None);
        assert!(has_timestamps("[ti:x]\n[00:01.00]Hello"));
        assert!(!has_timestamps("Hello\n[Chorus]"));
    }

    #[test]
    fn parses_lrc() {
        let lrc = "[ar:BTS]\n[offset:500]\n[00:12.30][01:10.00]Repeat\n[00:05.00]<00:05.00>Word <00:05.50>by word\n[00:20.00]\n";
        assert_eq!(
            parse(lrc),
            [
                timed(4_500, "Word by word"),
                timed(11_800, "Repeat"),
                timed(19_500, ""),
                timed(69_500, "Repeat"),
            ]
        );
    }

    #[test]
    fn pairs_lyrembed_below_layout() {
        let lrc = "[00:12.30]夜に駆ける\n[00:12.30]Yoru ni kakeru\n[00:15.00]English line\n[00:18.00]사랑해\n[00:18.02]saranghae";
        assert_eq!(
            pair(parse(lrc)),
            [
                Line {
                    time_ms: Some(12_300),
                    primary: "夜に駆ける".into(),
                    secondary: Some("Yoru ni kakeru".into()),
                },
                Line {
                    time_ms: Some(15_000),
                    primary: "English line".into(),
                    secondary: None,
                },
                Line {
                    time_ms: Some(18_000),
                    primary: "사랑해".into(),
                    secondary: Some("saranghae".into()),
                },
            ]
        );
    }

    #[test]
    fn latin_first_is_swapped() {
        let lines = pair(vec![timed(0, "Yoru ni kakeru"), timed(10, "夜に駆ける")]);
        assert_eq!(lines[0].primary, "夜に駆ける");
        assert_eq!(lines[0].secondary.as_deref(), Some("Yoru ni kakeru"));
    }

    #[test]
    fn merges_separate_romanization() {
        let lines = pair(parse("[00:01.00]東京\n[00:05.00]Tokyo"));
        let merged = merge_romanization(lines, &parse("[00:01.02]Tōkyō\n[00:05.00]Tokyo"));
        assert_eq!(merged[0].secondary.as_deref(), Some("Tōkyō"));
        assert_eq!(
            merged[1].secondary, None,
            "identical text isn't a romanization"
        );
        // Close lines get their own, the nearest one (Kyō is within reach
        // of 東 too, Higashi of 京).
        let lines = pair(parse("[00:01.00]東\n[00:01.06]京"));
        let merged = merge_romanization(lines, &parse("[00:01.02]Higashi\n[00:01.06]Kyō"));
        assert_eq!(merged[0].secondary.as_deref(), Some("Higashi"));
        assert_eq!(merged[1].secondary.as_deref(), Some("Kyō"));
        // A file with the original and the romanization at the same time:
        // the original is skipped, not taken and dropped.
        let lines = pair(parse("[00:01.00]東京"));
        let merged = merge_romanization(lines, &parse("[00:01.00]東京\n[00:01.00]Tōkyō"));
        assert_eq!(merged[0].secondary.as_deref(), Some("Tōkyō"));
    }

    #[test]
    fn scripts() {
        assert!(is_latin("Hello, world!"));
        assert!(is_latin("Ça va ? 123"));
        assert!(!is_latin("사랑해 love"));
        assert!(!is_latin("夜に駆ける"));
    }
}
