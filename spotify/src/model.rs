//! What the Web API returns, parsed leniently: missing fields are `None`,
//! not errors, since Spotify drops fields for Development Mode apps (§9.1).

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: String,
    pub display_name: Option<String>,
}

impl User {
    pub(crate) fn parse(value: &Value) -> Option<Self> {
        Some(Self {
            id: value["id"].as_str()?.to_owned(),
            display_name: string(&value["display_name"]),
        })
    }
}

/// A Spotify Connect device (§9.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// `None` for devices that can't be controlled.
    pub id: Option<String>,
    pub name: String,
    /// "Computer", "Smartphone", "Speaker", …
    pub kind: String,
    pub is_active: bool,
    pub is_restricted: bool,
    pub volume_percent: Option<u8>,
    pub supports_volume: bool,
}

impl Device {
    pub(crate) fn parse(value: &Value) -> Option<Self> {
        Some(Self {
            id: string(&value["id"]),
            name: value["name"].as_str()?.to_owned(),
            kind: string(&value["type"]).unwrap_or_default(),
            is_active: value["is_active"].as_bool().unwrap_or(false),
            is_restricted: value["is_restricted"].as_bool().unwrap_or(false),
            volume_percent: value["volume_percent"].as_u64().map(|v| v.min(100) as u8),
            supports_volume: value["supports_volume"].as_bool().unwrap_or(true),
        })
    }

    /// A symbolic icon for the device's kind.
    pub fn icon_name(&self) -> &'static str {
        match self.kind.to_lowercase().as_str() {
            "smartphone" => "phone-symbolic",
            "tablet" => "tablet-symbolic",
            "speaker" | "avr" | "stb" | "audiodongle" => "audio-speakers-symbolic",
            "tv" | "castvideo" => "tv-symbolic",
            "automobile" => "audio-card-symbolic",
            _ => "computer-symbolic",
        }
    }
}

/// A track (also from a playlist, the player or a search).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Track {
    pub uri: String,
    pub id: Option<String>,
    pub name: String,
    pub artists: Vec<String>,
    /// The artists' Spotify IDs, in the same order (where Spotify gives
    /// them).
    pub artist_ids: Vec<String>,
    pub album: Option<String>,
    pub album_id: Option<String>,
    pub duration_ms: u64,
    pub isrc: Option<String>,
    /// The largest album image (640 px).
    pub image_url: Option<String>,
    /// A local file in the user's Spotify app; can't be played or matched
    /// by ID.
    pub is_local: bool,
    /// `false` if it's not available in the user's country.
    pub is_playable: bool,
}

impl Track {
    pub(crate) fn parse(value: &Value) -> Option<Self> {
        // Episodes and other types aren't tracks.
        if value["type"].as_str().is_some_and(|t| t != "track") {
            return None;
        }
        let uri = value["uri"].as_str()?.to_owned();
        let album = &value["album"];
        Some(Self {
            uri,
            id: string(&value["id"]),
            name: string(&value["name"]).unwrap_or_default(),
            artists: value["artists"]
                .as_array()
                .map(|artists| artists.iter().filter_map(|a| string(&a["name"])).collect())
                .unwrap_or_default(),
            artist_ids: value["artists"]
                .as_array()
                .map(|artists| artists.iter().filter_map(|a| string(&a["id"])).collect())
                .unwrap_or_default(),
            album: string(&album["name"]),
            album_id: string(&album["id"]),
            duration_ms: value["duration_ms"].as_u64().unwrap_or(0),
            isrc: string(&value["external_ids"]["isrc"]).map(|i| i.to_uppercase()),
            image_url: image(&album["images"]),
            is_local: value["is_local"].as_bool().unwrap_or(false),
            is_playable: value["is_playable"].as_bool().unwrap_or(true),
        })
    }

    /// "Artist, Artist".
    pub fn artist_line(&self) -> String {
        self.artists.join(", ")
    }

    /// The web page of the track.
    pub fn web_url(&self) -> Option<String> {
        self.id
            .as_ref()
            .map(|id| format!("https://open.spotify.com/track/{id}"))
    }
}

/// An album as lists show it: saved albums, an artist's releases.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Album {
    pub id: String,
    pub name: String,
    /// (Spotify ID, name) of each artist.
    pub artists: Vec<(Option<String>, String)>,
    /// "album", "single" (singles and EPs) or "compilation".
    pub album_type: String,
    pub year: Option<i32>,
    pub total_tracks: u32,
    /// The largest image (640 px).
    pub image_url: Option<String>,
    /// The barcode; only in full album objects (saved albums, one album).
    pub upc: Option<String>,
}

impl Album {
    pub(crate) fn parse(value: &Value) -> Option<Self> {
        Some(Self {
            id: value["id"].as_str()?.to_owned(),
            name: string(&value["name"]).unwrap_or_default(),
            artists: value["artists"]
                .as_array()
                .map(|artists| {
                    artists
                        .iter()
                        .filter_map(|a| Some((string(&a["id"]), string(&a["name"])?)))
                        .collect()
                })
                .unwrap_or_default(),
            album_type: string(&value["album_type"])
                .unwrap_or_else(|| "album".into())
                .to_lowercase(),
            // "2018", "2018-08", "2018-08-24".
            year: string(&value["release_date"])
                .and_then(|d| d.get(..4).and_then(|y| y.parse().ok())),
            total_tracks: value["total_tracks"].as_u64().unwrap_or(0) as u32,
            image_url: image(&value["images"]),
            upc: string(&value["external_ids"]["upc"]),
        })
    }

    /// "Artist, Artist".
    pub fn artist_line(&self) -> String {
        self.artists
            .iter()
            .map(|(_, name)| name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn uri(&self) -> String {
        format!("spotify:album:{}", self.id)
    }

    pub fn web_url(&self) -> String {
        format!("https://open.spotify.com/album/{}", self.id)
    }
}

/// A song of an album, in album order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlbumTrack {
    /// With the album's name and cover filled in.
    pub track: Track,
    pub disc: u32,
    pub number: u32,
}

/// An artist (`GET /me/following`, `GET /artists/{id}`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Artist {
    pub id: String,
    pub name: String,
    pub image_url: Option<String>,
    pub genres: Vec<String>,
}

impl Artist {
    pub(crate) fn parse(value: &Value) -> Option<Self> {
        Some(Self {
            id: value["id"].as_str()?.to_owned(),
            name: string(&value["name"]).unwrap_or_default(),
            image_url: image(&value["images"]),
            genres: value["genres"]
                .as_array()
                .map(|g| g.iter().filter_map(string).collect())
                .unwrap_or_default(),
        })
    }

    pub fn web_url(&self) -> String {
        format!("https://open.spotify.com/artist/{}", self.id)
    }
}

/// One of the user's playlists (`GET /me/playlists`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistSummary {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub owner_id: Option<String>,
    pub owner_name: Option<String>,
    pub collaborative: bool,
    pub public: Option<bool>,
    pub snapshot_id: String,
    /// `None` when Spotify doesn't show the entries: in Development Mode,
    /// playlists the user neither owns nor collaborates on (§9.5).
    pub total: Option<u32>,
    pub image_url: Option<String>,
}

impl PlaylistSummary {
    pub(crate) fn parse(value: &Value) -> Option<Self> {
        let owner = &value["owner"];
        // Renamed from "tracks" in 2026; accept both.
        let items = if value["items"].is_object() {
            &value["items"]
        } else {
            &value["tracks"]
        };
        Some(Self {
            id: value["id"].as_str()?.to_owned(),
            name: string(&value["name"]).unwrap_or_default(),
            description: string(&value["description"]).filter(|d| !d.is_empty()),
            owner_id: string(&owner["id"]),
            owner_name: string(&owner["display_name"]).or_else(|| string(&owner["id"])),
            collaborative: value["collaborative"].as_bool().unwrap_or(false),
            public: value["public"].as_bool(),
            snapshot_id: string(&value["snapshot_id"]).unwrap_or_default(),
            total: items["total"].as_u64().map(|t| t as u32),
            image_url: image(&value["images"]),
        })
    }

    /// The user may change it: theirs, or collaborative (§9.6).
    pub fn editable_by(&self, user_id: &str) -> bool {
        self.collaborative || self.owner_id.as_deref() == Some(user_id)
    }

    pub fn uri(&self) -> String {
        format!("spotify:playlist:{}", self.id)
    }
}

/// A position in a playlist. Every position is kept, even episodes or
/// unavailable entries, so positions match Spotify's when editing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistEntry {
    /// `None` when Spotify gives no URI (rare, e.g. removed content).
    pub uri: Option<String>,
    /// `None` for anything that isn't a track.
    pub track: Option<Track>,
    /// When it was added (Unix seconds); Spotify knows it for most.
    pub added_at: Option<i64>,
}

impl PlaylistEntry {
    pub(crate) fn parse(row: &Value) -> Self {
        // The object moved from "track" to "item" in 2026, and "track"
        // may now be a boolean.
        let object = if row["item"].is_object() {
            &row["item"]
        } else {
            &row["track"]
        };
        // A song replaced for the user's country ("relinked") comes as the
        // replacement; the playlist holds the original, which removing must
        // name (and which plays as the replacement anyway).
        let uri = string(&object["linked_from"]["uri"]).or_else(|| string(&object["uri"]));
        let track = Track::parse(object).map(|mut track| {
            if row["is_local"].as_bool() == Some(true) {
                track.is_local = true;
            }
            if let Some(uri) = &uri {
                track.uri.clone_from(uri);
            }
            track
        });
        let added_at = row["added_at"].as_str().and_then(unix_time);
        Self {
            uri,
            track,
            added_at,
        }
    }
}

/// What a device plays (`GET /me/player`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlaybackState {
    pub device: Option<Device>,
    pub is_playing: bool,
    pub progress_ms: u64,
    pub shuffle: bool,
    /// "off", "track" or "context".
    pub repeat: String,
    pub track: Option<Track>,
    pub context_uri: Option<String>,
}

impl PlaybackState {
    pub(crate) fn parse(value: &Value) -> Self {
        Self {
            device: Device::parse(&value["device"]),
            is_playing: value["is_playing"].as_bool().unwrap_or(false),
            progress_ms: value["progress_ms"].as_u64().unwrap_or(0),
            shuffle: value["shuffle_state"].as_bool().unwrap_or(false),
            repeat: string(&value["repeat_state"]).unwrap_or_else(|| "off".into()),
            track: Track::parse(&value["item"]),
            context_uri: string(&value["context"]["uri"]),
        }
    }
}

/// The upcoming tracks of `GET /me/player/queue` (episodes left out).
pub(crate) fn parse_queue(value: &Value) -> Vec<Track> {
    value["queue"]
        .as_array()
        .map(|items| items.iter().filter_map(Track::parse).collect())
        .unwrap_or_default()
}

/// Unix seconds of a UTC time like `2024-05-01T12:30:00Z` (Spotify's
/// form; fractions of seconds are ignored).
pub(crate) fn unix_time(text: &str) -> Option<i64> {
    let (date, time) = text.trim_end_matches('Z').split_once('T')?;
    let mut date = date.split('-').map(str::parse::<i64>);
    let (year, month, day) = (date.next()?.ok()?, date.next()?.ok()?, date.next()?.ok()?);
    let time = time.split('.').next()?;
    let mut time = time.split(':').map(str::parse::<i64>);
    let (hour, minute, second) = (time.next()?.ok()?, time.next()?.ok()?, time.next()?.ok()?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Days since 1970-01-01 (Howard Hinnant's days_from_civil).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second)
}

fn string(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned)
}

/// The largest image (640 px for album covers): covers are scaled down
/// from it, and the player shows them large.
fn image(images: &Value) -> Option<String> {
    let images = images.as_array()?;
    images
        .iter()
        .max_by_key(|image| image["width"].as_i64().unwrap_or(0))
        .and_then(|image| string(&image["url"]))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn spotify_times_become_unix_seconds() {
        assert_eq!(unix_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(unix_time("2024-02-29T12:30:15Z"), Some(1_709_209_815));
        assert_eq!(unix_time("2026-11-07T21:00:00.123Z"), Some(1_794_085_200));
        assert_eq!(unix_time("yesterday"), None);
        let row = json!({"added_at": "2020-01-01T00:00:00Z", "item": {
            "type": "track", "uri": "spotify:track:a", "id": "a", "name": "Song",
            "duration_ms": 1000, "artists": [{"name": "A", "id": "artist-a"}]
        }});
        let entry = PlaylistEntry::parse(&row);
        assert_eq!(entry.added_at, Some(1_577_836_800));
        assert_eq!(entry.track.unwrap().artist_ids, ["artist-a"]);
    }

    #[test]
    fn a_relinked_entry_keeps_the_playlists_uri() {
        let row = json!({"item": {
            "type": "track", "uri": "spotify:track:replacement", "id": "replacement",
            "name": "Song", "duration_ms": 1000, "artists": [{"name": "A"}],
            "linked_from": {"uri": "spotify:track:original", "id": "original"}
        }});
        let entry = PlaylistEntry::parse(&row);
        assert_eq!(entry.uri.as_deref(), Some("spotify:track:original"));
        assert_eq!(entry.track.unwrap().uri, "spotify:track:original");
    }

    fn track_json() -> Value {
        json!({
            "type": "track",
            "uri": "spotify:track:abc",
            "id": "abc",
            "name": "Dynamite",
            "artists": [{"name": "BTS"}],
            "album": {
                "name": "Dynamite (DayTime Version)",
                "id": "alb",
                "images": [
                    {"url": "big", "width": 640, "height": 640},
                    {"url": "mid", "width": 300, "height": 300},
                    {"url": "small", "width": 64, "height": 64}
                ]
            },
            "duration_ms": 199053,
            "external_ids": {"isrc": "usa2p2025832"},
            "is_local": false
        })
    }

    #[test]
    fn tracks_parse() {
        let track = Track::parse(&track_json()).unwrap();
        assert_eq!(track.name, "Dynamite");
        assert_eq!(track.artists, ["BTS"]);
        assert_eq!(track.isrc.as_deref(), Some("USA2P2025832"));
        assert_eq!(track.image_url.as_deref(), Some("big"));
        assert!(track.is_playable);
        assert!(Track::parse(&json!({"type": "episode", "uri": "spotify:episode:x"})).is_none());
    }

    #[test]
    fn playlist_rows_accept_old_and_new_shapes() {
        let new =
            PlaylistEntry::parse(&json!({"item": track_json(), "track": true, "is_local": false}));
        assert_eq!(new.track.unwrap().uri, "spotify:track:abc");
        let old = PlaylistEntry::parse(&json!({"track": track_json()}));
        assert_eq!(old.uri.as_deref(), Some("spotify:track:abc"));
        let episode =
            PlaylistEntry::parse(&json!({"item": {"type": "episode", "uri": "spotify:episode:e"}}));
        assert_eq!(episode.uri.as_deref(), Some("spotify:episode:e"));
        assert!(episode.track.is_none());
    }

    #[test]
    fn playlists_parse_and_know_who_may_edit() {
        let own = PlaylistSummary::parse(&json!({
            "id": "p1", "name": "Mine", "snapshot_id": "s",
            "owner": {"id": "me", "display_name": "Max"},
            "collaborative": false,
            "items": {"total": 12},
            "images": [{"url": "u", "width": null}]
        }))
        .unwrap();
        assert_eq!(own.total, Some(12));
        assert!(own.editable_by("me"));
        assert!(!own.editable_by("other"));
        let followed = PlaylistSummary::parse(&json!({
            "id": "p2", "name": "Theirs", "snapshot_id": "s",
            "owner": {"id": "spotify"}, "collaborative": false
        }))
        .unwrap();
        assert_eq!(followed.total, None);
        assert_eq!(followed.owner_name.as_deref(), Some("spotify"));
        let old = PlaylistSummary::parse(&json!({"id": "p3", "tracks": {"total": 3}})).unwrap();
        assert_eq!(old.total, Some(3));
    }

    #[test]
    fn queues_parse() {
        let queue = parse_queue(&json!({
            "currently_playing": track_json(),
            "queue": [track_json(), {"type": "episode", "uri": "spotify:episode:e"}]
        }));
        assert_eq!(queue.len(), 1);
        assert!(parse_queue(&json!({})).is_empty());
    }

    #[test]
    fn playback_state_parses() {
        let state = PlaybackState::parse(&json!({
            "device": {"id": "d", "name": "Kitchen", "type": "Speaker", "is_active": true,
                       "volume_percent": 40, "supports_volume": true},
            "is_playing": true, "progress_ms": 1234, "shuffle_state": false,
            "repeat_state": "context", "item": track_json(),
            "context": {"uri": "spotify:playlist:p1"}
        }));
        assert_eq!(state.device.as_ref().unwrap().name, "Kitchen");
        assert_eq!(state.device.unwrap().icon_name(), "audio-speakers-symbolic");
        assert_eq!(state.repeat, "context");
        assert_eq!(state.track.unwrap().duration_ms, 199053);
        assert_eq!(state.context_uri.as_deref(), Some("spotify:playlist:p1"));
    }
}
