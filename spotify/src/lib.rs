//! The Spotify account (CONCEPT.md §9.3, §9.5, §9.6): logging in with the
//! user's own client ID, the Connect player, playlists and search.
//!
//! Everything here is blocking and meant for a worker thread. Parsing is
//! separate from the HTTP calls (`model`), so it is tested offline.

pub mod auth;
pub mod model;

use std::sync::Mutex;
use std::time::Duration;

use serde_json::{Value, json};

pub use auth::Tokens;
pub use model::{
    Album, AlbumTrack, Artist, Device, PlaybackState, PlaylistEntry, PlaylistSummary, Track, User,
};

const API: &str = "https://api.spotify.com/v1";

/// Development aid: debug builds talk to a local stand-in for Spotify
/// when `GATEFOLD_SPOTIFY_TEST_SERVER` is set (e.g. `http://127.0.0.1:8975`).
pub(crate) fn test_server() -> Option<String> {
    if cfg!(debug_assertions) {
        std::env::var("GATEFOLD_SPOTIFY_TEST_SERVER").ok()
    } else {
        None
    }
}

fn api_base() -> String {
    test_server().map_or_else(|| API.to_owned(), |server| format!("{server}/v1"))
}
const MAX_JSON_BYTES: u64 = 8 * 1024 * 1024;
/// Spotify's limit for adding or removing items in one call.
pub const MAX_ITEMS_PER_CALL: usize = 100;
/// Since 2026, a search returns at most 10 results (§9.1).
pub const MAX_SEARCH_RESULTS: u32 = 10;
/// Between the pages of a long list.
const PAGE_PAUSE: Duration = Duration::from_millis(100);
/// Rate limits up to this long are waited out; longer ones are errors.
const MAX_RETRY_WAIT: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network error: {0}")]
    Http(#[from] ureq::Error),
    /// Spotify said no; `reason` is e.g. `PREMIUM_REQUIRED` or
    /// `NO_ACTIVE_DEVICE` for player calls.
    #[error("Spotify answered {status}: {message}")]
    Status {
        status: u16,
        message: String,
        reason: Option<String>,
    },
    #[error("too many requests, try again in {0:?}")]
    RateLimited(Duration),
    /// The login is gone (revoked, or the refresh token expired).
    #[error("the Spotify login is no longer valid")]
    LoggedOut,
    #[error("login failed: {0}")]
    Auth(String),
    #[error("cancelled")]
    Cancelled,
    #[error("unexpected answer from Spotify ({0})")]
    Format(&'static str),
}

impl Error {
    /// Worth trying again later (offline, rate limited, Spotify down):
    /// pending playlist changes stay in the outbox (§9.6).
    pub fn is_temporary(&self) -> bool {
        match self {
            Self::Http(_) | Self::RateLimited(_) => true,
            Self::Status { status, .. } => *status >= 500,
            _ => false,
        }
    }

    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Status { status, .. } => Some(*status),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(20)))
        .user_agent("Gatefold ( https://github.com/sobakah/gatefold )")
        .build()
        .into()
}

/// Exchanges the code from the browser for tokens (end of the login).
pub fn exchange_code(client_id: &str, code: &str, pkce: &auth::Pkce) -> Result<Tokens> {
    exchange_code_for(client_id, auth::REDIRECT_URI, code, pkce)
}

/// The same for another client (streaming logs in as librespot).
pub fn exchange_code_for(
    client_id: &str,
    redirect_uri: &str,
    code: &str,
    pkce: &auth::Pkce,
) -> Result<Tokens> {
    let response = agent().post(&auth::token_url()).send_form([
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("code_verifier", &pkce.verifier),
    ])?;
    let value = token_answer(response)?;
    Tokens::from_response(&value, None)
}

fn token_answer(mut response: ureq::http::Response<ureq::Body>) -> Result<Value> {
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .with_config()
        .limit(MAX_JSON_BYTES)
        .read_to_string()?;
    let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    match status {
        200 => Ok(value),
        // invalid_grant: the refresh token was revoked or expired.
        400 if value["error"].as_str() == Some("invalid_grant") => Err(Error::LoggedOut),
        _ => Err(Error::Auth(
            value["error_description"]
                .as_str()
                .or(value["error"].as_str())
                .unwrap_or("unknown error")
                .to_owned(),
        )),
    }
}

/// Downloads a playlist or album image (Spotify's image server needs no
/// login). At most 4 MB.
pub fn download_image(url: &str) -> Result<Vec<u8>> {
    let mut response = agent().get(url).call()?;
    if !response.status().is_success() {
        return Err(status_error(response.status().as_u16(), ""));
    }
    Ok(response
        .body_mut()
        .with_config()
        .limit(4 * 1024 * 1024)
        .read_to_vec()?)
}

/// HTTP methods the API uses.
#[derive(Debug, Clone, Copy)]
enum Method {
    Get,
    Post,
    Put,
    Delete,
}

/// A logged-in account. Refreshes its access token by itself and reports
/// new tokens through `on_refresh`, so they can be stored.
pub struct Client {
    client_id: String,
    agent: ureq::Agent,
    tokens: Mutex<Tokens>,
    on_refresh: Box<dyn Fn(&Tokens) + Send + Sync>,
}

impl Client {
    pub fn new(
        client_id: &str,
        tokens: Tokens,
        on_refresh: impl Fn(&Tokens) + Send + Sync + 'static,
    ) -> Self {
        Self {
            client_id: client_id.to_owned(),
            agent: agent(),
            tokens: Mutex::new(tokens),
            on_refresh: Box::new(on_refresh),
        }
    }

    fn access_token(&self, force_refresh: bool) -> Result<String> {
        let mut tokens = self.tokens.lock().unwrap_or_else(|e| e.into_inner());
        if force_refresh || tokens.expired() {
            let response = self.agent.post(&auth::token_url()).send_form([
                ("grant_type", "refresh_token"),
                ("refresh_token", tokens.refresh_token.as_str()),
                ("client_id", self.client_id.as_str()),
            ])?;
            let value = token_answer(response)?;
            *tokens = Tokens::from_response(&value, Some(&tokens.refresh_token))?;
            (self.on_refresh)(&tokens);
        }
        Ok(tokens.access_token.clone())
    }

    /// One API call; `None` for answers without a body (204). Refreshes
    /// the token once on 401 and waits out short rate limits.
    fn call(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<Option<Value>> {
        let url = if path.starts_with("https://") || path.starts_with("http://") {
            path.to_owned()
        } else {
            format!("{}{path}", api_base())
        };
        let mut refreshed = false;
        let mut waited = false;
        loop {
            let token = self.access_token(false)?;
            let auth = format!("Bearer {token}");
            let response = match (method, body) {
                (Method::Get, _) => {
                    let mut request = self.agent.get(&url).header("Authorization", &auth);
                    for (k, v) in query {
                        request = request.query(*k, v);
                    }
                    request.call()
                }
                (Method::Delete, None) => {
                    let mut request = self.agent.delete(&url).header("Authorization", &auth);
                    for (k, v) in query {
                        request = request.query(*k, v);
                    }
                    request.call()
                }
                (Method::Delete, Some(body)) => {
                    let mut request = self
                        .agent
                        .delete(&url)
                        .header("Authorization", &auth)
                        .force_send_body();
                    for (k, v) in query {
                        request = request.query(*k, v);
                    }
                    request
                        .content_type("application/json")
                        .send(body.to_string())
                }
                (Method::Post | Method::Put, body) => {
                    let mut request = match method {
                        Method::Post => self.agent.post(&url),
                        _ => self.agent.put(&url),
                    }
                    .header("Authorization", &auth);
                    for (k, v) in query {
                        request = request.query(*k, v);
                    }
                    match body {
                        Some(body) => request
                            .content_type("application/json")
                            .send(body.to_string()),
                        None => request.header("Content-Length", "0").send_empty(),
                    }
                }
            }?;
            let status = response.status().as_u16();
            if status == 401 && !refreshed {
                refreshed = true;
                self.access_token(true)?;
                continue;
            }
            if status == 429 {
                let wait = response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.trim().parse().ok())
                    .map_or(Duration::from_secs(5), Duration::from_secs);
                if !waited && wait <= MAX_RETRY_WAIT {
                    waited = true;
                    std::thread::sleep(wait);
                    continue;
                }
                return Err(Error::RateLimited(wait));
            }
            return read_answer(response, status);
        }
    }

    fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value> {
        self.call(Method::Get, path, query, None)?
            .ok_or(Error::Format("empty answer"))
    }

    /// All pages of a paged list (`items` + `next`).
    fn get_all(&self, path: &str, query: &[(&str, String)]) -> Result<Vec<Value>> {
        let mut items = Vec::new();
        let mut page = self.get(path, query)?;
        loop {
            if let Some(list) = page["items"].as_array() {
                items.extend(list.iter().cloned());
            }
            match page["next"].as_str() {
                Some(next) if !next.is_empty() => {
                    // Long playlists: gently, Spotify limits requests.
                    std::thread::sleep(PAGE_PAUSE);
                    page = self.get(next, &[])?;
                }
                _ => return Ok(items),
            }
        }
    }

    // Account ------------------------------------------------------------

    pub fn me(&self) -> Result<User> {
        User::parse(&self.get("/me", &[])?).ok_or(Error::Format("user"))
    }

    // Player (§9.3) --------------------------------------------------------

    pub fn devices(&self) -> Result<Vec<Device>> {
        let value = self.get("/me/player/devices", &[])?;
        Ok(value["devices"]
            .as_array()
            .map(|devices| devices.iter().filter_map(Device::parse).collect())
            .unwrap_or_default())
    }

    /// What plays where; `None` if nothing does.
    pub fn playback(&self) -> Result<Option<PlaybackState>> {
        let value = self.call(
            Method::Get,
            "/me/player",
            &[("market", "from_token".into())],
            None,
        )?;
        Ok(value.map(|v| PlaybackState::parse(&v)))
    }

    /// Moves playback to a device.
    pub fn transfer(&self, device_id: &str, play: bool) -> Result<()> {
        self.call(
            Method::Put,
            "/me/player",
            &[],
            Some(&json!({"device_ids": [device_id], "play": play})),
        )
        .map(drop)
    }

    /// Resumes, or plays a context (playlist, album) from a position, or a
    /// list of tracks.
    pub fn play(&self, device_id: Option<&str>, what: Option<PlayRequest>) -> Result<()> {
        let query = device_query(device_id);
        let body = what.map(|what| match what {
            PlayRequest::Context {
                uri,
                offset,
                position_ms,
            } => json!({
                "context_uri": uri,
                "offset": {"position": offset},
                "position_ms": position_ms,
            }),
            PlayRequest::Tracks {
                uris,
                offset,
                position_ms,
            } => json!({
                "uris": uris,
                "offset": {"position": offset},
                "position_ms": position_ms,
            }),
        });
        self.call(Method::Put, "/me/player/play", &query, body.as_ref())
            .map(drop)
    }

    /// What the device plays after the current song (Spotify's queue
    /// and then its context, about 20 songs).
    pub fn queue(&self) -> Result<Vec<Track>> {
        let value = self.get("/me/player/queue", &[])?;
        Ok(model::parse_queue(&value))
    }

    /// Adds a track to the device's queue (after the current song).
    pub fn add_to_queue(&self, uri: &str) -> Result<()> {
        self.call(
            Method::Post,
            "/me/player/queue",
            &[("uri", uri.to_owned())],
            None,
        )
        .map(drop)
    }

    pub fn pause(&self) -> Result<()> {
        self.call(Method::Put, "/me/player/pause", &[], None)
            .map(drop)
    }

    pub fn next(&self) -> Result<()> {
        self.call(Method::Post, "/me/player/next", &[], None)
            .map(drop)
    }

    pub fn previous(&self) -> Result<()> {
        self.call(Method::Post, "/me/player/previous", &[], None)
            .map(drop)
    }

    pub fn seek(&self, position_ms: u64) -> Result<()> {
        self.call(
            Method::Put,
            "/me/player/seek",
            &[("position_ms", position_ms.to_string())],
            None,
        )
        .map(drop)
    }

    pub fn set_volume(&self, percent: u8) -> Result<()> {
        self.call(
            Method::Put,
            "/me/player/volume",
            &[("volume_percent", percent.min(100).to_string())],
            None,
        )
        .map(drop)
    }

    pub fn set_shuffle(&self, shuffle: bool) -> Result<()> {
        self.call(
            Method::Put,
            "/me/player/shuffle",
            &[("state", shuffle.to_string())],
            None,
        )
        .map(drop)
    }

    /// `"off"`, `"track"` or `"context"`.
    pub fn set_repeat(&self, repeat: &str) -> Result<()> {
        self.call(
            Method::Put,
            "/me/player/repeat",
            &[("state", repeat.to_owned())],
            None,
        )
        .map(drop)
    }

    // Playlists (§9.5, §9.6) ----------------------------------------------

    pub fn my_playlists(&self) -> Result<Vec<PlaylistSummary>> {
        Ok(self
            .get_all("/me/playlists", &[("limit", "50".into())])?
            .iter()
            .filter_map(PlaylistSummary::parse)
            .collect())
    }

    pub fn playlist(&self, id: &str) -> Result<PlaylistSummary> {
        PlaylistSummary::parse(&self.get(&format!("/playlists/{id}"), &[])?)
            .ok_or(Error::Format("playlist"))
    }

    pub fn playlist_items(&self, id: &str) -> Result<Vec<PlaylistEntry>> {
        Ok(self
            .get_all(
                &format!("/playlists/{id}/items"),
                &[("limit", "50".into()), ("market", "from_token".into())],
            )?
            .iter()
            .map(PlaylistEntry::parse)
            .collect())
    }

    /// Inserts `uris` at `position` (or appends); returns the new snapshot.
    pub fn add_items(&self, id: &str, uris: &[String], position: Option<u32>) -> Result<String> {
        let mut snapshot = None;
        for (index, chunk) in uris.chunks(MAX_ITEMS_PER_CALL).enumerate() {
            let mut body = json!({ "uris": chunk });
            if let Some(position) = position {
                body["position"] = json!(position as usize + index * MAX_ITEMS_PER_CALL);
            }
            snapshot = Some(self.write(Method::Post, &format!("/playlists/{id}/items"), &body)?);
        }
        snapshot.ok_or(Error::Format("nothing to add"))
    }

    /// Removes every occurrence of `uris` (Spotify can't remove a single
    /// occurrence any more).
    pub fn remove_items(
        &self,
        id: &str,
        uris: &[String],
        snapshot: Option<&str>,
    ) -> Result<String> {
        let mut snapshot = snapshot.map(str::to_owned);
        for chunk in uris.chunks(MAX_ITEMS_PER_CALL) {
            let items: Vec<Value> = chunk.iter().map(|uri| json!({ "uri": uri })).collect();
            let mut body = json!({ "items": items });
            if let Some(snapshot) = &snapshot {
                body["snapshot_id"] = json!(snapshot);
            }
            snapshot =
                Some(self.write(Method::Delete, &format!("/playlists/{id}/items"), &body)?);
        }
        snapshot.ok_or(Error::Format("nothing to remove"))
    }

    /// Moves `length` items from `start` to before `insert_before`
    /// (positions before the move).
    pub fn reorder(
        &self,
        id: &str,
        start: u32,
        insert_before: u32,
        length: u32,
        snapshot: Option<&str>,
    ) -> Result<String> {
        let mut body = json!({
            "range_start": start,
            "insert_before": insert_before,
            "range_length": length,
        });
        if let Some(snapshot) = snapshot {
            body["snapshot_id"] = json!(snapshot);
        }
        self.write(Method::Put, &format!("/playlists/{id}/items"), &body)
    }

    /// Replaces all items (first 100 by replacing, the rest appended).
    pub fn replace_items(&self, id: &str, uris: &[String]) -> Result<String> {
        let first = &uris[..uris.len().min(MAX_ITEMS_PER_CALL)];
        let mut snapshot = self.write(
            Method::Put,
            &format!("/playlists/{id}/items"),
            &json!({ "uris": first }),
        )?;
        if uris.len() > MAX_ITEMS_PER_CALL {
            snapshot = self.add_items(id, &uris[MAX_ITEMS_PER_CALL..], None)?;
        }
        Ok(snapshot)
    }

    pub fn rename_playlist(&self, id: &str, name: &str) -> Result<()> {
        self.call(
            Method::Put,
            &format!("/playlists/{id}"),
            &[],
            Some(&json!({ "name": name })),
        )
        .map(drop)
    }

    /// A new, private playlist of the user's.
    pub fn create_playlist(&self, name: &str, description: &str) -> Result<PlaylistSummary> {
        let value = self
            .call(
                Method::Post,
                "/me/playlists",
                &[],
                Some(&json!({ "name": name, "description": description, "public": false })),
            )?
            .ok_or(Error::Format("playlist"))?;
        PlaylistSummary::parse(&value).ok_or(Error::Format("playlist"))
    }

    /// Sets the cover (a JPEG of at most 256 KB).
    pub fn upload_cover(&self, id: &str, jpeg: &[u8]) -> Result<()> {
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(jpeg);
        let token = self.access_token(false)?;
        let response = self
            .agent
            .put(&format!("{}/playlists/{id}/images", api_base()))
            .header("Authorization", &format!("Bearer {token}"))
            .content_type("image/jpeg")
            .send(encoded)?;
        let status = response.status().as_u16();
        read_answer(response, status).map(drop)
    }

    // Search -------------------------------------------------------------

    // Library (saved albums, followed artists) and discographies ------------

    /// The user's saved albums, the latest first (`user-library-read`).
    pub fn saved_albums(&self) -> Result<Vec<Album>> {
        let items = self.get_all("/me/albums", &[("limit", "50".into())])?;
        Ok(items
            .iter()
            .filter_map(|item| Album::parse(&item["album"]))
            .collect())
    }

    /// The artists the user follows (`user-follow-read`). Paged by cursor.
    pub fn followed_artists(&self) -> Result<Vec<Artist>> {
        let mut artists = Vec::new();
        let mut page = self.get(
            "/me/following",
            &[("type", "artist".into()), ("limit", "50".into())],
        )?;
        loop {
            let list = &page["artists"];
            if let Some(items) = list["items"].as_array() {
                artists.extend(items.iter().filter_map(Artist::parse));
            }
            match list["next"].as_str() {
                Some(next) if !next.is_empty() => {
                    std::thread::sleep(PAGE_PAUSE);
                    page = self.get(next, &[])?;
                }
                _ => return Ok(artists),
            }
        }
    }

    pub fn artist(&self, id: &str) -> Result<Artist> {
        Artist::parse(&self.get(&format!("/artists/{id}"), &[])?).ok_or(Error::Format("artist"))
    }

    /// An artist's albums, EPs and singles (no compilations they appear on).
    pub fn artist_albums(&self, id: &str) -> Result<Vec<Album>> {
        let items = self.get_all(
            &format!("/artists/{id}/albums"),
            &[
                ("include_groups", "album,single".into()),
                ("limit", "50".into()),
            ],
        )?;
        Ok(items.iter().filter_map(Album::parse).collect())
    }

    /// An album and its songs.
    pub fn album(&self, id: &str) -> Result<(Album, Vec<AlbumTrack>)> {
        let value = self.get(&format!("/albums/{id}"), &[])?;
        let album = Album::parse(&value).ok_or(Error::Format("album"))?;
        let mut items: Vec<Value> = value["tracks"]["items"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut next = value["tracks"]["next"].as_str().map(str::to_owned);
        while let Some(url) = next.filter(|n| !n.is_empty()) {
            std::thread::sleep(PAGE_PAUSE);
            let page = self.get(&url, &[])?;
            items.extend(page["items"].as_array().cloned().unwrap_or_default());
            next = page["next"].as_str().map(str::to_owned);
        }
        let tracks = items
            .iter()
            .filter_map(|item| {
                let mut track = Track::parse(item)?;
                // Album songs come without their album.
                track.album = Some(album.name.clone());
                track.album_id = Some(album.id.clone());
                track.image_url.clone_from(&album.image_url);
                Some(AlbumTrack {
                    track,
                    disc: item["disc_number"].as_u64().unwrap_or(1) as u32,
                    number: item["track_number"].as_u64().unwrap_or(0) as u32,
                })
            })
            .collect();
        Ok((album, tracks))
    }

    /// Artists by name (a few; for artists without a known Spotify ID).
    pub fn search_artists(&self, name: &str) -> Result<Vec<Artist>> {
        let value = self.get(
            "/search",
            &[
                ("q", name.to_owned()),
                ("type", "artist".into()),
                ("limit", "5".into()),
            ],
        )?;
        Ok(value["artists"]["items"]
            .as_array()
            .map(|items| items.iter().filter_map(Artist::parse).collect())
            .unwrap_or_default())
    }

    /// Up to 10 tracks; `query` may use filters like `isrc:USA2P2025832`
    /// or `track:… artist:…`.
    pub fn search_tracks(&self, query: &str, limit: u32) -> Result<Vec<Track>> {
        let value = self.get(
            "/search",
            &[
                ("q", query.to_owned()),
                ("type", "track".into()),
                ("limit", limit.clamp(1, MAX_SEARCH_RESULTS).to_string()),
                ("market", "from_token".into()),
            ],
        )?;
        Ok(value["tracks"]["items"]
            .as_array()
            .map(|items| items.iter().filter_map(Track::parse).collect())
            .unwrap_or_default())
    }

    /// A write that answers with a snapshot ID.
    fn write(&self, method: Method, path: &str, body: &Value) -> Result<String> {
        let value = self
            .call(method, path, &[], Some(body))?
            .ok_or(Error::Format("no snapshot"))?;
        value["snapshot_id"]
            .as_str()
            .map(str::to_owned)
            .ok_or(Error::Format("no snapshot"))
    }
}

/// What to play.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayRequest {
    Context {
        uri: String,
        offset: u32,
        position_ms: u64,
    },
    Tracks {
        uris: Vec<String>,
        offset: u32,
        position_ms: u64,
    },
}

fn device_query(device_id: Option<&str>) -> Vec<(&'static str, String)> {
    device_id
        .map(|id| vec![("device_id", id.to_owned())])
        .unwrap_or_default()
}

fn read_answer(
    mut response: ureq::http::Response<ureq::Body>,
    status: u16,
) -> Result<Option<Value>> {
    let text = response
        .body_mut()
        .with_config()
        .limit(MAX_JSON_BYTES)
        .read_to_string()
        .unwrap_or_default();
    if (200..300).contains(&status) {
        if text.trim().is_empty() {
            return Ok(None);
        }
        // Some player calls answer 200 with a non-JSON body.
        return Ok(serde_json::from_str(&text).ok());
    }
    Err(status_error(status, &text))
}

fn status_error(status: u16, text: &str) -> Error {
    let value: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let error = &value["error"];
    let message = error["message"]
        .as_str()
        .or(error.as_str())
        .unwrap_or("no details")
        .to_owned();
    let reason = error["reason"].as_str().map(str::to_owned);
    if status == 401 {
        return Error::LoggedOut;
    }
    Error::Status {
        status,
        message,
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_carry_spotifys_reason() {
        let error = status_error(
            403,
            r#"{"error":{"status":403,"message":"Player command failed: Premium required","reason":"PREMIUM_REQUIRED"}}"#,
        );
        match error {
            Error::Status { status, reason, .. } => {
                assert_eq!(status, 403);
                assert_eq!(reason.as_deref(), Some("PREMIUM_REQUIRED"));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(status_error(503, "").is_temporary());
        assert!(!status_error(404, "").is_temporary());
        assert!(matches!(status_error(401, ""), Error::LoggedOut));
    }
}
