//! The Spotify account (CONCEPT.md §9.3, §9.5, §9.6): logging in with the
//! user's own client ID, the Connect player, playlists and search.
//!
//! Everything here is blocking and meant for a worker thread. Parsing is
//! separate from the HTTP calls (`model`), so it is tested offline.

pub mod auth;
pub mod model;

use std::sync::Mutex;
use std::time::{Duration, SystemTime};

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
/// The page size every list accepts.
const SMALL_PAGE: &str = "10";
/// Between the pages of a long list.
const PAGE_PAUSE: Duration = Duration::from_millis(100);
/// Rate limits up to this long are waited out; longer ones are errors.
const MAX_RETRY_WAIT: Duration = Duration::from_secs(10);
/// How often the number of requests is logged (see `RequestCount`).
const COUNT_PERIOD: Duration = Duration::from_secs(3600);

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

    /// About this request only (Spotify found its query malformed, an
    /// answer it couldn't read): other requests may still work. Anything
    /// else (offline, rate limited, refused, logged out) holds for the
    /// requests after it too.
    pub fn is_about_request(&self) -> bool {
        matches!(
            self,
            Self::Status {
                status: 400 | 404,
                ..
            } | Self::Format(_)
        )
    }

    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Status { status, .. } => Some(*status),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// What a request asks for, as far as Spotify's rate limits go: the
/// first part of its path ("search", "artists", "playlists"), two for the
/// account's ("me/player", "me/albums").
fn request_kind(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let path = path.split_once("/v1/").map_or(path, |(_, rest)| rest);
    let mut parts = path.trim_start_matches('/').split('/');
    match (parts.next(), parts.next()) {
        (Some("me"), Some(second)) => format!("me/{second}"),
        (Some(first), _) => first.to_owned(),
        _ => String::new(),
    }
}

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
        // The login server is busy or down: try again later, like the API.
        429 => Err(Error::RateLimited(std::time::Duration::from_secs(30))),
        500.. => Err(Error::Status {
            status,
            message: "the Spotify login server isn't available".into(),
            reason: None,
        }),
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

/// What a request sends.
#[derive(Clone, Copy)]
enum Body<'a> {
    Empty,
    Json(&'a Value),
    /// Base64 of a JPEG, as Spotify takes playlist covers.
    Jpeg(&'a str),
}

/// A logged-in account. Refreshes its access token by itself and reports
/// new tokens through `on_refresh`, so they can be stored.
pub struct Client {
    client_id: String,
    agent: ureq::Agent,
    tokens: Mutex<Tokens>,
    on_refresh: Box<dyn Fn(&Tokens) + Send + Sync>,
    /// Kinds of requests Spotify said to stop for a long time (a rate
    /// limit of hours), and until when. Asking anyway during that time
    /// can make the wait longer, so those fail here without asking.
    blocked: Mutex<std::collections::HashMap<String, SystemTime>>,
    /// Told about each new block, to keep it across restarts.
    on_block: Box<OnBlock>,
    requests: Mutex<RequestCount>,
}

/// The requests sent, by kind, for the log: once an hour, and with a long
/// rate limit, so it shows what used up Spotify's allowance.
#[derive(Debug)]
struct RequestCount {
    since: std::time::Instant,
    kinds: std::collections::BTreeMap<String, u32>,
}

impl Default for RequestCount {
    fn default() -> Self {
        Self {
            since: std::time::Instant::now(),
            kinds: std::collections::BTreeMap::new(),
        }
    }
}

impl RequestCount {
    /// "artists 34, search 5 (in 12 min)": the counts, and the time they
    /// were counted in (it only rolls over at the next request, so it may
    /// be longer than COUNT_PERIOD).
    fn summary(&self) -> String {
        let kinds = self
            .kinds
            .iter()
            .map(|(kind, n)| format!("{kind} {n}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{kinds} (in {} min)", self.since.elapsed().as_secs() / 60)
    }

    fn add(&mut self, kind: &str) {
        if self.since.elapsed() >= COUNT_PERIOD {
            if !self.kinds.is_empty() {
                tracing::info!("Spotify requests: {}", self.summary());
            }
            *self = Self::default();
        }
        *self.kinds.entry(kind.to_owned()).or_default() += 1;
    }
}

/// Told a kind of request (see `request_kind`) and until when it's blocked.
type OnBlock = dyn Fn(&str, SystemTime) + Send + Sync;

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
            blocked: Mutex::default(),
            on_block: Box::new(|_, _| {}),
            requests: Mutex::default(),
        }
    }

    /// Blocks kept from before (kind, until; see `request_kind`), and
    /// where to report new ones.
    pub fn with_blocks(
        mut self,
        blocks: impl IntoIterator<Item = (String, SystemTime)>,
        on_block: impl Fn(&str, SystemTime) + Send + Sync + 'static,
    ) -> Self {
        let now = SystemTime::now();
        self.blocked = Mutex::new(
            blocks
                .into_iter()
                .filter(|(_, until)| *until > now)
                .collect(),
        );
        self.on_block = Box::new(on_block);
        self
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
        let body = body.map_or(Body::Empty, Body::Json);
        self.call_with(method, path, query, body)
    }

    /// Like [`Client::call`], with any body.
    fn call_with(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Body<'_>,
    ) -> Result<Option<Value>> {
        let url = if path.starts_with("https://") || path.starts_with("http://") {
            path.to_owned()
        } else {
            format!("{}{path}", api_base())
        };
        let kind = request_kind(&url);
        if let Some(until) = self.blocked.lock().ok().and_then(|b| b.get(&kind).copied())
            && let Ok(left) = until.duration_since(SystemTime::now())
        {
            return Err(Error::RateLimited(left));
        }
        let mut refreshed = false;
        let mut waited = false;
        loop {
            let token = self.access_token(false)?;
            let auth = format!("Bearer {token}");
            if let Ok(mut requests) = self.requests.lock() {
                requests.add(&kind);
            }
            let response = match (method, body) {
                (Method::Get, _) => {
                    let mut request = self.agent.get(&url).header("Authorization", &auth);
                    for (k, v) in query {
                        request = request.query(*k, v);
                    }
                    request.call()
                }
                (Method::Delete, Body::Empty) => {
                    let mut request = self.agent.delete(&url).header("Authorization", &auth);
                    for (k, v) in query {
                        request = request.query(*k, v);
                    }
                    request.call()
                }
                (Method::Delete, body) => {
                    let mut request = self
                        .agent
                        .delete(&url)
                        .header("Authorization", &auth)
                        .force_send_body();
                    for (k, v) in query {
                        request = request.query(*k, v);
                    }
                    match body {
                        Body::Jpeg(data) => request.content_type("image/jpeg").send(data),
                        Body::Json(json) => request
                            .content_type("application/json")
                            .send(json.to_string()),
                        Body::Empty => unreachable!("matched above"),
                    }
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
                        Body::Json(json) => request
                            .content_type("application/json")
                            .send(json.to_string()),
                        Body::Jpeg(data) => request.content_type("image/jpeg").send(data),
                        Body::Empty => request.header("Content-Length", "0").send_empty(),
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
                if wait > MAX_RETRY_WAIT {
                    let sent = self
                        .requests
                        .lock()
                        .map(|r| r.summary())
                        .unwrap_or_default();
                    tracing::warn!(
                        "Spotify asks to wait {wait:?} before asking for {kind} again \
                         (requests sent: {sent})"
                    );
                    let until = SystemTime::now() + wait;
                    if let Ok(mut blocked) = self.blocked.lock() {
                        blocked.insert(kind.clone(), until);
                    }
                    (self.on_block)(&kind, until);
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

    /// The first page of a list; when Spotify refuses the page size (it
    /// lowered several to at most 10 for Development Mode apps in 2026,
    /// e.g. an artist's albums), again with 10.
    fn get_first_page(&self, path: &str, query: &[(&str, String)]) -> Result<Value> {
        match self.get(path, query) {
            Err(Error::Status { status: 400, .. })
                if query.iter().any(|(k, v)| *k == "limit" && v != SMALL_PAGE) =>
            {
                let smaller: Vec<(&str, String)> = query
                    .iter()
                    .map(|(k, v)| {
                        if *k == "limit" {
                            (*k, SMALL_PAGE.to_owned())
                        } else {
                            (*k, v.clone())
                        }
                    })
                    .collect();
                self.get(path, &smaller)
            }
            result => result,
        }
    }

    /// All pages of a paged list (`items` + `next`).
    fn get_all(&self, path: &str, query: &[(&str, String)]) -> Result<Vec<Value>> {
        let mut items = Vec::new();
        let mut page = self.get_first_page(path, query)?;
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

    /// Removes a playlist from the user's library (`DELETE /me/library`,
    /// since February 2026): unfollows someone else's playlist, deletes
    /// one's own (Spotify keeps it restorable for a while).
    pub fn remove_playlist_from_library(&self, id: &str) -> Result<()> {
        self.remove_from_library(&format!("spotify:playlist:{id}"))
    }

    /// Saves an item to the user's library (`PUT /me/library`, since
    /// February 2026): saves an album (`user-library-modify`), follows an
    /// artist (`user-follow-modify`).
    pub fn save_to_library(&self, uri: &str) -> Result<()> {
        self.call(
            Method::Put,
            "/me/library",
            &[("uris", uri.to_owned())],
            None,
        )
        .map(drop)
    }

    /// Whether an item is in the user's library: an album saved, an
    /// artist followed (`GET /me/library/contains`).
    pub fn library_contains(&self, uri: &str) -> Result<bool> {
        let value = self.get("/me/library/contains", &[("uris", uri.to_owned())])?;
        value
            .as_array()
            .and_then(|answers| answers.first())
            .and_then(Value::as_bool)
            .ok_or(Error::Format("library contains"))
    }

    /// Takes an item out of the user's library (`DELETE /me/library`):
    /// removes a saved album, unfollows an artist.
    pub fn remove_from_library(&self, uri: &str) -> Result<()> {
        self.call(
            Method::Delete,
            "/me/library",
            &[("uris", uri.to_owned())],
            None,
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
        // Through call_with like every other request: a 401 refreshes the
        // token instead of counting as logged out, rate limits are waited
        // out or recorded.
        self.call_with(
            Method::Put,
            &format!("/playlists/{id}/images"),
            &[],
            Body::Jpeg(&encoded),
        )
        .map(drop)
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
        let mut page = self.get_first_page(
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
                // Spotify refuses more for apps in Development Mode
                // (2026); asking for 50 first cost a request each time.
                ("limit", SMALL_PAGE.into()),
            ],
        )?;
        Ok(items.iter().filter_map(Album::parse).collect())
    }

    /// One song: its album and artists (for links to their pages).
    pub fn track(&self, id: &str) -> Result<Track> {
        Track::parse(&self.get(&format!("/tracks/{id}"), &[])?).ok_or(Error::Format("track"))
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
    ///
    /// Without `market=from_token`: since 2026 Spotify answers a search
    /// that names it with "403 Insufficient client scope" (it would need
    /// `user-read-private`), though playlist reads still accept it.
    /// Songs, artists, albums and playlists for a search, in one request
    /// (at most `MAX_SEARCH_RESULTS` of each).
    pub fn search_all(&self, query: &str) -> Result<SearchResults> {
        let value = self.get(
            "/search",
            &[
                ("q", query.to_owned()),
                ("type", "track,artist,album,playlist".into()),
                ("limit", MAX_SEARCH_RESULTS.to_string()),
            ],
        )?;
        let items = |kind: &str| value[kind]["items"].as_array().cloned().unwrap_or_default();
        Ok(SearchResults {
            tracks: items("tracks").iter().filter_map(Track::parse).collect(),
            artists: items("artists").iter().filter_map(Artist::parse).collect(),
            albums: items("albums").iter().filter_map(Album::parse).collect(),
            // Spotify lists removed playlists as null.
            playlists: items("playlists")
                .iter()
                .filter_map(PlaylistSummary::parse)
                .collect(),
        })
    }

    pub fn search_tracks(&self, query: &str, limit: u32) -> Result<Vec<Track>> {
        let value = self.get(
            "/search",
            &[
                ("q", query.to_owned()),
                ("type", "track".into()),
                ("limit", limit.clamp(1, MAX_SEARCH_RESULTS).to_string()),
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

/// What a search found on Spotify, each kind in Spotify's order.
#[derive(Debug, Clone, Default)]
pub struct SearchResults {
    pub tracks: Vec<Track>,
    pub artists: Vec<Artist>,
    pub albums: Vec<Album>,
    pub playlists: Vec<PlaylistSummary>,
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
    fn a_kept_block_is_answered_without_asking_spotify() {
        let tokens = Tokens {
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_at: u64::MAX,
        };
        let hour = Duration::from_secs(3600);
        let client = Client::new("id", tokens, |_| {}).with_blocks(
            [
                ("search".to_owned(), SystemTime::now() + hour),
                // Over already: dropped.
                ("artists".to_owned(), SystemTime::now() - hour),
            ],
            |_, _| {},
        );
        // No network needed: refused here.
        match client.search_tracks("isrc:X", 5) {
            Err(Error::RateLimited(left)) => assert!(left <= hour && left > hour / 2),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(client.blocked.lock().unwrap().len(), 1);
    }

    #[test]
    fn rate_limits_are_kept_per_kind_of_request() {
        assert_eq!(request_kind("https://api.spotify.com/v1/search"), "search");
        assert_eq!(
            request_kind("https://api.spotify.com/v1/artists/abc/albums?offset=10"),
            "artists"
        );
        assert_eq!(request_kind("/me/player/devices"), "me/player");
        assert_eq!(
            request_kind("http://127.0.0.1:8975/v1/me/albums"),
            "me/albums"
        );
    }

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
