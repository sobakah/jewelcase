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
    pub album: Option<String>,
    pub album_id: Option<String>,
    pub duration_ms: u64,
    pub isrc: Option<String>,
    /// An album image about 300 px wide.
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
        let track = Track::parse(object).map(|mut track| {
            if row["is_local"].as_bool() == Some(true) {
                track.is_local = true;
            }
            track
        });
        Self {
            uri: string(&object["uri"]),
            track,
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

fn string(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned)
}

/// The image closest to 300 px wide (lists are largest first).
fn image(images: &Value) -> Option<String> {
    let images = images.as_array()?;
    images
        .iter()
        .min_by_key(|image| {
            let width = image["width"].as_i64().unwrap_or(300);
            (width - 300).abs()
        })
        .and_then(|image| string(&image["url"]))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

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
        assert_eq!(track.image_url.as_deref(), Some("mid"));
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
