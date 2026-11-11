//! Logging in (CONCEPT.md §9.3): OAuth Authorization Code with PKCE in the
//! system browser, answered on a loopback port. No client secret: every
//! user registers their own client ID (§9.1).

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::Rng;
use rand::distr::Alphanumeric;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{Error, Result};

/// The loopback port Spotify sends the browser back to. Fixed, because
/// the redirect URI must be registered exactly in the user's Spotify app.
pub const REDIRECT_PORT: u16 = 8974;
pub const REDIRECT_URI: &str = "http://127.0.0.1:8974/callback";

/// Everything Jewelcase does: the player (§9.3), reading and editing
/// playlists (§9.5, §9.6) and their covers.
pub const SCOPES: &[&str] = &[
    "user-read-playback-state",
    "user-modify-playback-state",
    "user-read-currently-playing",
    "playlist-read-private",
    "playlist-read-collaborative",
    "playlist-modify-private",
    "playlist-modify-public",
    "ugc-image-upload",
    // Saved albums and followed artists (§9.8).
    "user-library-read",
    "user-follow-read",
    // Saving albums, unfollowing and deleting playlists (`/me/library`).
    "user-library-modify",
    // Following artists (`/me/library` too).
    "user-follow-modify",
];

const AUTHORIZE_URL: &str = "https://accounts.spotify.com/authorize";
const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";

pub(crate) fn token_url() -> String {
    crate::test_server().map_or_else(|| TOKEN_URL.to_owned(), |s| format!("{s}/api/token"))
}

/// The secret half of a login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
    /// Ties the browser's answer to this login.
    pub state: String,
}

impl Pkce {
    pub fn new() -> Self {
        let verifier = random_string(64);
        Self {
            challenge: challenge(&verifier),
            verifier,
            state: random_string(24),
        }
    }
}

impl Default for Pkce {
    fn default() -> Self {
        Self::new()
    }
}

fn random_string(len: usize) -> String {
    rand::rng()
        .sample_iter(Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

/// S256: base64url(sha256(verifier)), unpadded.
fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// The page to open in the browser.
pub fn authorize_url(client_id: &str, pkce: &Pkce) -> String {
    authorize_url_for(client_id, REDIRECT_URI, SCOPES, pkce)
}

/// The same for another client (streaming logs in as librespot).
pub fn authorize_url_for(
    client_id: &str,
    redirect_uri: &str,
    scopes: &[&str],
    pkce: &Pkce,
) -> String {
    let scope = scopes.join(" ");
    let params = [
        ("client_id", client_id),
        ("response_type", "code"),
        ("redirect_uri", redirect_uri),
        ("code_challenge_method", "S256"),
        ("code_challenge", &pkce.challenge),
        ("state", &pkce.state),
        ("scope", &scope),
    ];
    let query: Vec<String> = params
        .iter()
        .map(|(k, v)| format!("{k}={}", encode(v)))
        .collect();
    let base =
        crate::test_server().map_or_else(|| AUTHORIZE_URL.to_owned(), |s| format!("{s}/authorize"));
    format!("{base}?{}", query.join("&"))
}

/// Access and refresh token, as kept in the keyring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds.
    pub expires_at: u64,
}

impl Tokens {
    /// Expired, or about to.
    pub fn expired(&self) -> bool {
        now() + 60 >= self.expires_at
    }

    pub fn to_json(&self) -> String {
        serde_json::json!({
            "access_token": self.access_token,
            "refresh_token": self.refresh_token,
            "expires_at": self.expires_at,
        })
        .to_string()
    }

    pub fn from_json(text: &str) -> Option<Self> {
        let value: Value = serde_json::from_str(text).ok()?;
        Some(Self {
            access_token: value["access_token"].as_str()?.to_owned(),
            refresh_token: value["refresh_token"].as_str()?.to_owned(),
            expires_at: value["expires_at"].as_u64()?,
        })
    }

    /// From the token endpoint's answer; a refresh may not send a new
    /// refresh token, then the old one stays valid.
    pub(crate) fn from_response(value: &Value, old_refresh: Option<&str>) -> Result<Self> {
        let access_token = value["access_token"]
            .as_str()
            .ok_or(Error::Format("token"))?
            .to_owned();
        let refresh_token = value["refresh_token"]
            .as_str()
            .or(old_refresh)
            .ok_or(Error::Format("token"))?
            .to_owned();
        let expires_in = value["expires_in"].as_u64().unwrap_or(3600);
        Ok(Self {
            access_token,
            refresh_token,
            expires_at: now() + expires_in,
        })
    }
}

pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Waits for the browser on the loopback port.
pub struct Callback {
    listener: TcpListener,
    path: &'static str,
}

impl Callback {
    /// Takes the port; fails if something else has it.
    pub fn bind() -> std::io::Result<Self> {
        Self::bind_at(REDIRECT_PORT, "/callback")
    }

    /// Another port and path (the streaming login's).
    pub fn bind_at(port: u16, path: &'static str) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        listener.set_nonblocking(true)?;
        Ok(Self { listener, path })
    }

    /// The authorization code, once the browser comes back (blocking).
    /// Gives up after `timeout` or when `cancel` is set.
    pub fn wait(&self, state: &str, timeout: Duration, cancel: &AtomicBool) -> Result<String> {
        let start = Instant::now();
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            if start.elapsed() > timeout {
                return Err(Error::Auth("the login timed out".into()));
            }
            match self.listener.accept() {
                Ok((stream, _)) => {
                    if let Some(result) = answer(stream, state, self.path) {
                        return result;
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(err) => return Err(Error::Auth(err.to_string())),
            }
        }
    }
}

/// Reads one request; `None` for anything that isn't the callback (a
/// favicon request), which is answered and ignored.
fn answer(stream: TcpStream, state: &str, expected: &str) -> Option<Result<String>> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let target = line.split_whitespace().nth(1).unwrap_or("");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut writer = &stream;
    if path != expected {
        let _ = writer
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return None;
    }
    // Not this login's answer (another page asking this address): it
    // can't end the login.
    let Some(result) = parse_callback(query, state) else {
        let _ = writer.write_all(
            b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        return None;
    };
    let (title, text) = match &result {
        Ok(_) => (
            "Connected to Spotify",
            "Jewelcase is connected. You can close this tab.",
        ),
        Err(_) => (
            "Not connected",
            "Jewelcase couldn't connect to Spotify. You can close this tab and try again.",
        ),
    };
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>{title}</title>\
         <body style=\"font-family:sans-serif;text-align:center;margin-top:20vh\">\
         <h1>{title}</h1><p>{text}</p></body>"
    );
    let _ = write!(
        writer,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    Some(result)
}

/// The code or error of the answer; `None` when it isn't for this login
/// (the state doesn't match).
fn parse_callback(query: &str, state: &str) -> Option<Result<String>> {
    let mut code = None;
    let mut got_state = None;
    let mut error = None;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = decode(value);
        match key {
            "code" => code = Some(value),
            "state" => got_state = Some(value),
            "error" => error = Some(value),
            _ => {}
        }
    }
    if got_state.as_deref() != Some(state) {
        return None;
    }
    if let Some(error) = error {
        return Some(Err(if error == "access_denied" {
            Error::Cancelled
        } else {
            Error::Auth(error)
        }));
    }
    Some(
        code.filter(|c| !c.is_empty())
            .ok_or_else(|| Error::Auth("no authorization code".into())),
    )
}

/// Percent-encoding for query values (RFC 3986 unreserved characters
/// stay as they are).
pub(crate) fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            byte => out.push(byte),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_follows_rfc_7636() {
        // The example from RFC 7636, appendix B.
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let pkce = Pkce::new();
        assert_eq!(pkce.verifier.len(), 64);
        assert_ne!(Pkce::new().state, pkce.state);
    }

    #[test]
    fn authorize_url_carries_everything() {
        let pkce = Pkce {
            verifier: "v".into(),
            challenge: "c-_".into(),
            state: "s1".into(),
        };
        let url = authorize_url("abc123", &pkce);
        assert!(url.starts_with("https://accounts.spotify.com/authorize?client_id=abc123&"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A8974%2Fcallback"));
        assert!(url.contains("code_challenge=c-_&"));
        assert!(url.contains("scope=user-read-playback-state%20user-modify-playback-state"));
    }

    #[test]
    fn callback_answers_are_checked() {
        assert_eq!(
            parse_callback("code=AQ%2Bx&state=s1", "s1")
                .unwrap()
                .unwrap(),
            "AQ+x"
        );
        // Not this login's: ignored, the login goes on waiting.
        assert!(parse_callback("code=AQ&state=other", "s1").is_none());
        assert!(parse_callback("error=access_denied", "s1").is_none());
        assert!(matches!(
            parse_callback("error=access_denied&state=s1", "s1"),
            Some(Err(Error::Cancelled))
        ));
        assert!(parse_callback("state=s1", "s1").unwrap().is_err());
        assert_eq!(decode("a%20b+c%"), "a b c%");
    }

    #[test]
    fn the_browser_is_answered_on_the_loopback_port() {
        let Ok(callback) = Callback::bind() else {
            return; // The port is taken (a login is running).
        };
        let browser = std::thread::spawn(|| {
            let mut favicon = TcpStream::connect(("127.0.0.1", REDIRECT_PORT)).unwrap();
            favicon
                .write_all(b"GET /favicon.ico HTTP/1.1\r\n\r\n")
                .unwrap();
            let mut stream = TcpStream::connect(("127.0.0.1", REDIRECT_PORT)).unwrap();
            stream
                .write_all(b"GET /callback?code=xyz&state=st HTTP/1.1\r\nHost: x\r\n\r\n")
                .unwrap();
            let mut page = String::new();
            std::io::Read::read_to_string(&mut stream, &mut page).unwrap();
            page
        });
        let cancel = AtomicBool::new(false);
        let code = callback
            .wait("st", Duration::from_secs(5), &cancel)
            .unwrap();
        assert_eq!(code, "xyz");
        assert!(browser.join().unwrap().contains("Connected to Spotify"));
    }

    #[test]
    fn tokens_round_trip() {
        let tokens = Tokens {
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_at: 99,
        };
        assert_eq!(Tokens::from_json(&tokens.to_json()), Some(tokens.clone()));
        assert!(tokens.expired());
        let refreshed = Tokens::from_response(
            &serde_json::json!({"access_token": "b", "expires_in": 3600}),
            Some("r"),
        )
        .unwrap();
        assert_eq!(refreshed.refresh_token, "r");
        assert!(!refreshed.expired());
    }
}
