//! The Spotify logins in the keyring (oo7, CONCEPT.md §9.3, §12). Runs on a
//! worker thread; oo7 brings its own async runtime (async-io).
#![cfg_attr(not(feature = "spotify-stream"), allow(dead_code))]

use gatefold_spotify::Tokens;
use gtk::gio;

/// Keyring attributes; `application` as in CONCEPT.md §12, `kind` tells
/// the Web API login from the streaming login.
fn attributes(kind: &'static str) -> [(&'static str, &'static str); 2] {
    [
        ("application", "io.github.sobakah.Gatefold"),
        ("kind", kind),
    ]
}

const LOGIN: &str = "spotify-login";
/// librespot's reusable credentials (CONCEPT.md §9.4).
const STREAM_LOGIN: &str = "spotify-stream-login";

pub async fn load() -> Result<Option<Tokens>, String> {
    Ok(load_text(LOGIN)
        .await?
        .and_then(|text| Tokens::from_json(&text)))
}

pub async fn save(tokens: Tokens) -> Result<(), String> {
    save_text(LOGIN, "Gatefold: Spotify login", tokens.to_json()).await
}

pub async fn delete() -> Result<(), String> {
    delete_kind(LOGIN).await
}

pub async fn load_stream() -> Result<Option<String>, String> {
    load_text(STREAM_LOGIN).await
}

pub async fn save_stream(text: String) -> Result<(), String> {
    save_text(STREAM_LOGIN, "Gatefold: Spotify streaming login", text).await
}

pub async fn delete_stream() -> Result<(), String> {
    delete_kind(STREAM_LOGIN).await
}

async fn load_text(kind: &'static str) -> Result<Option<String>, String> {
    run(move || async move {
        let keyring = oo7::Keyring::new().await?;
        keyring.unlock().await?;
        let items = keyring.search_items(&attributes(kind)).await?;
        let Some(item) = items.first() else {
            return Ok(None);
        };
        let secret = item.secret().await?;
        Ok(Some(
            String::from_utf8_lossy(secret.as_bytes()).into_owned(),
        ))
    })
    .await
}

async fn save_text(kind: &'static str, label: &'static str, text: String) -> Result<(), String> {
    run(move || async move {
        let keyring = oo7::Keyring::new().await?;
        keyring.unlock().await?;
        keyring
            .create_item(label, &attributes(kind), text, true)
            .await
    })
    .await
}

async fn delete_kind(kind: &'static str) -> Result<(), String> {
    run(move || async move {
        let keyring = oo7::Keyring::new().await?;
        keyring.delete(&attributes(kind)).await
    })
    .await
}

/// Headless tests (build-aux/headless.sh) never touch a keyring.
pub fn disabled() -> bool {
    std::env::var_os("GATEFOLD_NO_KEYRING").is_some()
}

async fn run<T, F, Fut>(job: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = oo7::Result<T>>,
{
    if disabled() {
        return Err("the keyring is disabled (GATEFOLD_NO_KEYRING)".into());
    }
    gio::spawn_blocking(move || futures_lite::future::block_on(job()))
        .await
        .map_err(|_| "the keyring job failed".to_owned())?
        .map_err(|e| e.to_string())
}
