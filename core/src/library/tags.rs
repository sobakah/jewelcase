//! Reads the tags of one audio file with lofty (CONCEPT.md §2, §3.5).

use std::fs::File;
use std::path::Path;

use lofty::config::ParseOptions;
use lofty::file::{AudioFile, FileType, TaggedFileExt};
use lofty::picture::PictureType;
use lofty::tag::{Accessor, ItemKey, Tag};

/// Everything the library stores about one file's tags and audio stream.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackTags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    /// `ALBUMARTISTS` (Picard convention): the single names of a joint credit.
    pub album_artists: Vec<String>,
    pub album_artist_sort: Option<String>,
    pub disc: Option<u32>,
    pub number: Option<u32>,
    pub year: Option<i32>,
    pub genres: Vec<String>,
    pub composer: Option<String>,
    pub mb_album_id: Option<String>,
    pub mb_release_group_id: Option<String>,
    pub mb_track_id: Option<String>,
    pub mb_album_artist_ids: Vec<String>,
    pub release_comment: Option<String>,
    pub release_type: Option<String>,
    pub compilation: bool,
    pub isrc: Option<String>,
    pub bpm: Option<f64>,
    pub lyrics: Option<Lyrics>,
    pub replaygain_track: Option<f64>,
    pub replaygain_album: Option<f64>,
    pub duration_ms: u64,
    pub format: String,
    pub bitrate: Option<u32>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u8>,
    pub channels: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lyrics {
    pub synced: bool,
    /// The first lines without LRC timestamps, for the track details.
    pub excerpt: String,
}

#[derive(Debug, thiserror::Error)]
pub enum TagError {
    #[error("cannot open file: {0}")]
    Io(#[from] std::io::Error),
    #[error("cannot read tags: {0}")]
    Lofty(#[from] lofty::error::FileParseError),
}

/// Reads tags and stream properties. Pictures are not kept; use
/// [`read_front_cover`] for those.
pub fn read_tags(path: &Path) -> Result<(TrackTags, Option<Vec<u8>>), TagError> {
    let tagged = lofty::read_from_path(path)?;
    let properties = tagged.properties();
    let mut tags = TrackTags {
        duration_ms: u64::try_from(properties.duration().as_millis()).unwrap_or(u64::MAX),
        format: format_name(tagged.file_type()).to_owned(),
        bitrate: properties.audio_bitrate(),
        sample_rate: properties.sample_rate(),
        bit_depth: properties.bit_depth(),
        channels: properties.channels(),
        ..TrackTags::default()
    };

    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    let cover = tag.and_then(front_cover);
    if let Some(tag) = tag {
        fill_from_tag(&mut tags, tag);
    }
    tags.release_comment = read_release_comment(path, tagged.file_type());
    Ok((tags, cover))
}

/// The embedded front cover (or the first picture) of a file.
pub fn read_front_cover(path: &Path) -> Result<Option<Vec<u8>>, TagError> {
    let tagged = lofty::read_from_path(path)?;
    Ok(tagged
        .primary_tag()
        .or_else(|| tagged.first_tag())
        .and_then(front_cover))
}

fn front_cover(tag: &Tag) -> Option<Vec<u8>> {
    let pictures = tag.pictures();
    pictures
        .iter()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures.first())
        .map(|p| p.data().to_vec())
}

fn fill_from_tag(tags: &mut TrackTags, tag: &Tag) {
    let text = |key: ItemKey| {
        tag.get_string(key)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let texts = |key: ItemKey| -> Vec<String> {
        tag.get_strings(key)
            .flat_map(|s| s.split('\0'))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect()
    };

    tags.title = text(ItemKey::TrackTitle);
    tags.artist = text(ItemKey::TrackArtist);
    tags.album = text(ItemKey::AlbumTitle);
    tags.album_artist = text(ItemKey::AlbumArtist);
    tags.album_artists = texts(ItemKey::AlbumArtists);
    tags.album_artist_sort = text(ItemKey::AlbumArtistSortOrder);
    tags.disc = tag.disk();
    tags.number = tag.track();
    tags.year = [
        ItemKey::RecordingDate,
        ItemKey::Year,
        ItemKey::ReleaseDate,
        ItemKey::OriginalReleaseDate,
    ]
    .into_iter()
    .find_map(|key| text(key).as_deref().and_then(parse_year));
    tags.genres = texts(ItemKey::Genre);
    tags.composer = text(ItemKey::Composer);
    tags.mb_album_id = text(ItemKey::MusicBrainzReleaseId);
    tags.mb_release_group_id = text(ItemKey::MusicBrainzReleaseGroupId);
    tags.mb_track_id = text(ItemKey::MusicBrainzTrackId);
    tags.mb_album_artist_ids = texts(ItemKey::MusicBrainzReleaseArtistId);
    tags.release_type = text(ItemKey::MusicBrainzReleaseType);
    tags.compilation = text(ItemKey::FlagCompilation).is_some_and(|v| v == "1" || v == "true");
    tags.isrc = text(ItemKey::Isrc).or_else(|| text(ItemKey::AppleXid).and_then(isrc_from_xid));
    tags.bpm = text(ItemKey::Bpm)
        .or_else(|| text(ItemKey::IntegerBpm))
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|bpm| *bpm > 0.0);
    tags.lyrics = text(ItemKey::Lyrics)
        .or_else(|| text(ItemKey::UnsyncLyrics))
        .and_then(|l| parse_lyrics(&l));
    tags.replaygain_track = text(ItemKey::ReplayGainTrackGain).and_then(|v| parse_gain(&v));
    tags.replaygain_album = text(ItemKey::ReplayGainAlbumGain).and_then(|v| parse_gain(&v));
}

/// `RELEASECOMMENT` isn't a generic lofty key, so it's read from the
/// format's own tag (Picard's names for MP4 and ID3v2).
fn read_release_comment(path: &Path, file_type: FileType) -> Option<String> {
    const DESCRIPTION: &str = "MusicBrainz Album Comment";
    let options = ParseOptions::new()
        .read_properties(false)
        .read_cover_art(false);
    let mut file = File::open(path).ok()?;
    let value = match file_type {
        FileType::Flac => lofty::flac::FlacFile::read_from(&mut file, options)
            .ok()?
            .vorbis_comments()?
            .get("RELEASECOMMENT")
            .map(str::to_owned),
        FileType::Opus | FileType::Vorbis => lofty::ogg::VorbisFile::read_from(&mut file, options)
            .ok()?
            .vorbis_comments()
            .get("RELEASECOMMENT")
            .map(str::to_owned),
        FileType::Mp4 => {
            let mp4 = lofty::mp4::Mp4File::read_from(&mut file, options).ok()?;
            let ident = lofty::mp4::AtomIdent::Freeform {
                mean: "com.apple.iTunes".into(),
                name: DESCRIPTION.into(),
            };
            mp4.ilst()?.get(&ident)?.data().find_map(|data| match data {
                lofty::mp4::AtomData::UTF8(text) => Some(text.clone()),
                _ => None,
            })
        }
        FileType::Mpeg => lofty::mpeg::MpegFile::read_from(&mut file, options)
            .ok()?
            .id3v2()?
            .get_user_text(DESCRIPTION)
            .map(str::to_owned),
        _ => None,
    };
    value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

fn format_name(file_type: FileType) -> &'static str {
    match file_type {
        FileType::Flac => "FLAC",
        FileType::Mp4 => "MPEG-4",
        FileType::Mpeg => "MP3",
        FileType::Opus => "Opus",
        FileType::Vorbis => "Ogg Vorbis",
        FileType::Wav => "WAV",
        FileType::Aiff => "AIFF",
        FileType::Aac => "AAC",
        FileType::Ape => "APE",
        FileType::WavPack => "WavPack",
        FileType::Mpc => "Musepack",
        FileType::Speex => "Speex",
        _ => "Audio",
    }
}

fn parse_year(date: &str) -> Option<i32> {
    let digits: String = date.chars().take_while(char::is_ascii_digit).collect();
    (digits.len() == 4).then(|| digits.parse().ok()).flatten()
}

/// "-7.53 dB" → -7.53
fn parse_gain(value: &str) -> Option<f64> {
    value
        .trim()
        .trim_end_matches(|c: char| c.is_alphabetic() || c.is_whitespace())
        .parse()
        .ok()
}

/// iTunes `xid ` atoms look like `label:isrc:DEQ021801392`.
fn isrc_from_xid(xid: String) -> Option<String> {
    let (_, isrc) = xid.split_once(":isrc:")?;
    let isrc = isrc.trim();
    (!isrc.is_empty()).then(|| isrc.to_owned())
}

fn parse_lyrics(text: &str) -> Option<Lyrics> {
    let mut synced = false;
    let lines: Vec<&str> = text
        .lines()
        .map(|line| {
            let stripped = strip_lrc_tags(line);
            synced |= stripped.len() != line.len();
            stripped.trim()
        })
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        return None;
    }
    let excerpt = lines.iter().take(2).copied().collect::<Vec<_>>().join("\n");
    Some(Lyrics { synced, excerpt })
}

/// Removes leading `[mm:ss.xx]` / `[ar:…]` tags of an LRC line.
fn strip_lrc_tags(mut line: &str) -> &str {
    while let Some(rest) = line.trim_start().strip_prefix('[') {
        match rest.split_once(']') {
            Some((inner, after)) if inner.contains(':') => line = after,
            _ => break,
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn years() {
        assert_eq!(parse_year("2021-05-03"), Some(2021));
        assert_eq!(parse_year("1999"), Some(1999));
        assert_eq!(parse_year("99"), None);
    }

    #[test]
    fn gains() {
        assert_eq!(parse_gain("-7.53 dB"), Some(-7.53));
        assert_eq!(parse_gain("+1.20 dB"), Some(1.2));
        assert_eq!(parse_gain("bogus"), None);
    }

    #[test]
    fn isrc() {
        assert_eq!(
            isrc_from_xid("label:isrc:DEQ021801392".into()),
            Some("DEQ021801392".into())
        );
        assert_eq!(isrc_from_xid("label:upc:123".into()), None);
    }

    #[test]
    fn lyrics_excerpt() {
        let lrc = "[ar:BTS]\n[00:01.00]First line\n[00:03.50][00:40.00]Second line\n\nThird";
        assert_eq!(
            parse_lyrics(lrc),
            Some(Lyrics {
                synced: true,
                excerpt: "First line\nSecond line".into()
            })
        );
        let plain = "Hello\nWorld";
        assert!(!parse_lyrics(plain).unwrap().synced);
        assert_eq!(parse_lyrics("[ar:x]\n\n"), None);
    }
}
