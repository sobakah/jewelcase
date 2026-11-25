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
/// when `JEWELCASE_SPOTIFY_TEST_SERVER` is set (e.g. `http://127.0.0.1:8975`).
pub(crate) fn test_server() -> Option<String> {
    if cfg!(debug_assertions) {
        std::env::var("JEWELCASE_SPOTIFY_TEST_SERVER").ok()
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

/// How many of an artist's releases [`Client::artist_albums_start`] reads:
/// one page.
pub const ARTIST_ALBUMS_START: u32 = 10;

fn artist_albums_query(offset: u32) -> Vec<(&'static str, String)> {
    let mut query = vec![
        ("include_groups", "album,single".into()),
        // Spotify refuses more for apps in Development Mode (2026);
        // asking for 50 first cost a request each time.
        ("limit", ARTIST_ALBUMS_START.to_string()),
    ];
    if offset > 0 {
        query.push(("offset", offset.to_string()));
    }
    query
}

/// A page of an artist's releases, and Spotify's count of them all when
/// another page follows.
fn artist_albums_page(page: &Value) -> (Vec<Album>, Option<u32>) {
    let albums = page["items"]
        .as_array()
        .map(|items| items.iter().filter_map(Album::parse).collect())
        .unwrap_or_default();
    let more = page["next"]
        .as_str()
        .filter(|next| !next.is_empty())
        .map(|_| {
            page["total"]
                .as_u64()
                .map_or(0, |n| n.min(u32::MAX as u64) as u32)
        });
    (albums, more)
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(20)))
        .user_agent("Jewelcase ( https://github.com/sobakah/jewelcase )")
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
            self.log();
        }
        *self.kinds.entry(kind.to_owned()).or_default() += 1;
    }

    /// Logs the counts, if any, and starts counting anew; the summary
    /// logged, for the tests.
    fn log(&mut self) -> Option<String> {
        let summary = (!self.kinds.is_empty()).then(|| self.summary());
        if let Some(summary) = &summary {
            tracing::info!("Spotify requests: {summary}");
        }
        *self = Self::default();
        summary
    }
}

impl Drop for Client {
    /// A disconnect or a new login: the requests of this one in the log.
    fn drop(&mut self) {
        self.log_requests();
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

    /// Logs the requests counted since the last hourly count: at quit, or
    /// sessions shorter than an hour would leave none in the log.
    pub fn log_requests(&self) {
        if let Ok(mut requests) = self.requests.lock() {
            requests.log();
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

    /// The first [`ARTIST_ALBUMS_START`] of an artist's albums, EPs and
    /// singles, in one request, and Spotify's count of them all when there
    /// are more. Spotify allows only so many requests about artists a day
    /// (shared by all apps on an account): reading all of them took up to
    /// nine requests per artist page.
    pub fn artist_albums_start(&self, id: &str) -> Result<(Vec<Album>, Option<u32>)> {
        let page = self.get(&format!("/artists/{id}/albums"), &artist_albums_query(0))?;
        Ok(artist_albums_page(&page))
    }

    /// An artist's albums, EPs and singles (no compilations they appear
    /// on) from `offset` on, all pages.
    pub fn artist_albums_from(&self, id: &str, offset: u32) -> Result<Vec<Album>> {
        let items = self.get_all(
            &format!("/artists/{id}/albums"),
            &artist_albums_query(offset),
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

    /// Songs, artists, albums and playlists for a search, in one request
    /// (at most `MAX_SEARCH_RESULTS` of each).
    ///
    /// Without `market=from_token`: since 2026 Spotify answers a search
    /// that names it with "403 Insufficient client scope" (it would need
    /// `user-read-private`), though playlist reads still accept it.
    pub fn search_all(&self, query: &str) -> Result<SearchResults> {
        let value = self.get("/search", &search_query(query, &SearchKind::ALL, 0))?;
        Ok(search_page(&value))
    }

    /// The next results of one kind, from `offset` on (*More From
    /// Spotify*: one request of `MAX_SEARCH_RESULTS`).
    pub fn search_more(&self, query: &str, kind: SearchKind, offset: u32) -> Result<SearchResults> {
        let value = self.get("/search", &search_query(query, &[kind], offset))?;
        Ok(search_page(&value))
    }

    /// A release found elsewhere (Deezer's new releases): by its barcode
    /// (the `upc:` filter), else the album of exactly this title by this
    /// artist among those a search finds.
    pub fn find_album(
        &self,
        upc: Option<&str>,
        title: &str,
        artist: &str,
    ) -> Result<Option<Album>> {
        if let Some(upc) = upc.filter(|u| !u.is_empty()) {
            if let Some(album) = self
                .search_albums(&format!("upc:{upc}"))?
                .into_iter()
                .next()
            {
                return Ok(Some(album));
            }
            std::thread::sleep(PAGE_PAUSE);
        }
        let found = self.search_albums(&format!("{title} {artist}"))?;
        Ok(pick_album(found, title, artist))
    }

    fn search_albums(&self, query: &str) -> Result<Vec<Album>> {
        let value = self.get(
            "/search",
            &[
                ("q", query.to_owned()),
                ("type", "album".into()),
                ("limit", MAX_SEARCH_RESULTS.to_string()),
            ],
        )?;
        Ok(value["albums"]["items"]
            .as_array()
            .map(|items| items.iter().filter_map(Album::parse).collect())
            .unwrap_or_default())
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

/// Of albums a search found, the one of exactly `title` (letters and
/// digits, ignoring case) with an artist named `artist`.
pub fn pick_album(found: Vec<Album>, title: &str, artist: &str) -> Option<Album> {
    let key = |text: &str| -> String {
        text.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    };
    let (title, artist) = (key(title), key(artist));
    found.into_iter().find(|album| {
        key(&album.name) == title && album.artists.iter().any(|(_, name)| key(name) == artist)
    })
}

/// What a search found on Spotify, each kind in Spotify's order.
#[derive(Debug, Clone, Default)]
pub struct SearchResults {
    pub tracks: Vec<Track>,
    pub artists: Vec<Artist>,
    pub albums: Vec<Album>,
    pub playlists: Vec<PlaylistSummary>,
    /// The kinds Spotify has more of, with where their next page starts.
    pub more: Vec<(SearchKind, u32)>,
}

impl SearchResults {
    /// How many of a kind there are.
    pub fn count(&self, kind: SearchKind) -> usize {
        match kind {
            SearchKind::Tracks => self.tracks.len(),
            SearchKind::Artists => self.artists.len(),
            SearchKind::Albums => self.albums.len(),
            SearchKind::Playlists => self.playlists.len(),
        }
    }

    /// Where the next page of a kind starts, if Spotify has more of it.
    pub fn next_offset(&self, kind: SearchKind) -> Option<u32> {
        self.more
            .iter()
            .find(|(k, _)| *k == kind)
            .map(|(_, at)| *at)
    }

    /// The next page of one kind added to this one (`page` from
    /// `search_more`): its results after these, and whether there's more
    /// after it.
    pub fn extend(&mut self, kind: SearchKind, page: SearchResults) {
        match kind {
            SearchKind::Tracks => self.tracks.extend(page.tracks),
            SearchKind::Artists => self.artists.extend(page.artists),
            SearchKind::Albums => self.albums.extend(page.albums),
            SearchKind::Playlists => self.playlists.extend(page.playlists),
        }
        self.more.retain(|(k, _)| *k != kind);
        self.more
            .extend(page.more.into_iter().filter(|(k, _)| *k == kind));
    }
}

/// A kind of search result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SearchKind {
    Tracks,
    Artists,
    Albums,
    Playlists,
}

impl SearchKind {
    pub const ALL: [SearchKind; 4] = [
        SearchKind::Tracks,
        SearchKind::Artists,
        SearchKind::Albums,
        SearchKind::Playlists,
    ];

    /// Its name in the request's `type`.
    fn param(self) -> &'static str {
        match self {
            SearchKind::Tracks => "track",
            SearchKind::Artists => "artist",
            SearchKind::Albums => "album",
            SearchKind::Playlists => "playlist",
        }
    }

    /// Its key in the answer.
    fn key(self) -> &'static str {
        match self {
            SearchKind::Tracks => "tracks",
            SearchKind::Artists => "artists",
            SearchKind::Albums => "albums",
            SearchKind::Playlists => "playlists",
        }
    }
}

/// A search's query: these kinds, `MAX_SEARCH_RESULTS` of each, from
/// `offset` on.
fn search_query(query: &str, kinds: &[SearchKind], offset: u32) -> Vec<(&'static str, String)> {
    let kinds: Vec<&str> = kinds.iter().map(|k| k.param()).collect();
    let mut params = vec![
        ("q", query.to_owned()),
        ("type", kinds.join(",")),
        ("limit", MAX_SEARCH_RESULTS.to_string()),
    ];
    if offset > 0 {
        params.push(("offset", offset.to_string()));
    }
    params
}

/// What a search's answer holds, and which kinds have a next page (from
/// Spotify's paging: its offset and items, removed ones included, so a
/// next page never repeats one).
fn search_page(value: &serde_json::Value) -> SearchResults {
    let items = |kind: SearchKind| {
        value[kind.key()]["items"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    };
    SearchResults {
        tracks: items(SearchKind::Tracks)
            .iter()
            .filter_map(Track::parse)
            .collect(),
        artists: items(SearchKind::Artists)
            .iter()
            .filter_map(Artist::parse)
            .collect(),
        albums: items(SearchKind::Albums)
            .iter()
            .filter_map(Album::parse)
            .collect(),
        // Spotify lists removed playlists as null.
        playlists: items(SearchKind::Playlists)
            .iter()
            .filter_map(PlaylistSummary::parse)
            .collect(),
        more: SearchKind::ALL
            .into_iter()
            .filter(|k| {
                value[k.key()]["next"]
                    .as_str()
                    .is_some_and(|n| !n.is_empty())
            })
            .map(|k| {
                let page = &value[k.key()];
                let offset = page["offset"].as_u64().unwrap_or(0) as u32;
                (k, offset + items(k).len() as u32)
            })
            .collect(),
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
    fn found_albums_are_picked_by_exact_title_and_artist() {
        let album = |id: &str, name: &str, artist: &str| Album {
            id: id.into(),
            name: name.into(),
            artists: vec![(None, artist.into())],
            album_type: "single".into(),
            year: None,
            total_tracks: 1,
            image_url: None,
            upc: None,
        };
        let found = || {
            vec![
                album("1", "Echo (Remixes)", "The Band"),
                album("2", "Echo", "Someone Else"),
                album("3", "ECHO", "The Band"),
            ]
        };
        assert_eq!(
            pick_album(found(), "Echo", "the band").map(|a| a.id),
            Some("3".into())
        );
        assert_eq!(pick_album(found(), "Echo!", "Nobody").map(|a| a.id), None);
    }

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

    /// An artist page reads one page of releases; Spotify's total says how
    /// many a Show All would add.
    #[test]
    fn the_first_page_of_releases_says_how_many_there_are() {
        let item = |id: &str| serde_json::json!({"id": id, "name": id, "album_type": "album"});
        let page = serde_json::json!({
            "items": [item("a"), item("b")],
            "total": 52,
            "next": "https://api.spotify.com/v1/artists/x/albums?offset=10&limit=10",
        });
        let (albums, more) = artist_albums_page(&page);
        assert_eq!(albums.len(), 2);
        assert_eq!(more, Some(52));
        let last = serde_json::json!({"items": [item("a")], "total": 1, "next": null});
        assert_eq!(artist_albums_page(&last).1, None);
        assert_eq!(
            artist_albums_query(10).last(),
            Some(&("offset", "10".to_owned()))
        );
    }

    /// *More From Spotify*: a search says which kinds have a next page,
    /// and the next page of one kind is asked from where the shown end.
    #[test]
    fn a_search_says_which_kinds_have_more() {
        let page = serde_json::json!({
            "artists": {"items": [{"id": "a", "name": "A"}], "offset": 0, "next": "https://api.spotify.com/v1/search?offset=1"},
            "albums": {"items": [], "next": null},
            // Removed playlists come as null, but count for the next page.
            "playlists": {"items": [null, {"id": "p", "name": "P"}], "offset": 0, "next": "https://api.spotify.com/v1/search?offset=2"},
        });
        let mut found = search_page(&page);
        assert_eq!(found.artists.len(), 1);
        assert_eq!(found.playlists.len(), 1);
        assert_eq!(found.next_offset(SearchKind::Artists), Some(1));
        assert_eq!(found.next_offset(SearchKind::Playlists), Some(2));
        assert_eq!(found.next_offset(SearchKind::Albums), None);

        let query = search_query("echo", &[SearchKind::Artists], 10);
        assert!(query.contains(&("type", "artist".to_owned())));
        assert!(query.contains(&("offset", "10".to_owned())));
        assert!(
            !search_query("echo", &SearchKind::ALL, 0)
                .iter()
                .any(|(k, _)| *k == "offset")
        );

        let next = search_page(&serde_json::json!({
            "artists": {"items": [{"id": "b", "name": "B"}], "offset": 1, "next": null},
        }));
        found.extend(SearchKind::Artists, next);
        assert_eq!(found.count(SearchKind::Artists), 2);
        assert_eq!(found.next_offset(SearchKind::Artists), None);
        // The other kinds' pages stay.
        assert_eq!(found.next_offset(SearchKind::Playlists), Some(2));
    }

    /// The count is logged at quit too: sessions shorter than an hour left
    /// none, and a block came without the requests that led to it.
    #[test]
    fn the_request_count_is_logged_before_an_hour_is_over() {
        let mut count = RequestCount::default();
        assert_eq!(count.log(), None);
        count.add("artists");
        count.add("artists");
        assert_eq!(count.log().as_deref(), Some("artists 2 (in 0 min)"));
        assert_eq!(count.log(), None);
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
