//! The Spotify logins in the keyring (oo7, CONCEPT.md §9.3, §12). Runs on a
//! worker thread; oo7 brings its own async runtime (async-io).
#![cfg_attr(not(feature = "spotify-stream"), allow(dead_code))]

use gatefold_spotify::Tokens;
use gtk::gio;

/// Keyring attributes; `application` is the app ID (CONCEPT.md §12), so
/// the development build and the installed app keep their own logins;
/// `kind` tells the logins and keys apart.
fn attributes(kind: &'static str) -> [(&'static str, &'static str); 2] {
    [("application", crate::config::APP_ID), ("kind", kind)]
}

/// Where both builds kept their logins before: read once by the
/// development build when it has none of its own.
const SHARED_APPLICATION: &str = "io.github.sobakah.Gatefold";

/// Once the build has keyring entries of its own (or chose to have none,
/// by disconnecting), the shared ones aren't looked at again.
fn own_entry_marker(kind: &str) -> std::path::PathBuf {
    gtk::glib::user_data_dir()
        .join(crate::config::APP_ID)
        .join(format!("keyring-own-{kind}"))
}

fn mark_own_entry(kind: &str) {
    let marker = own_entry_marker(kind);
    if let Some(dir) = marker.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(marker, b"");
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
        if let Some(item) = items.first() {
            let secret = item.secret().await?;
            return Ok(Some(
                String::from_utf8_lossy(secret.as_bytes()).into_owned(),
            ));
        }
        if crate::config::APP_ID == SHARED_APPLICATION || own_entry_marker(kind).exists() {
            return Ok(None);
        }
        // The development build's login from before it had its own: copied,
        // the installed app's stays as it is.
        let shared = keyring
            .search_items(&[("application", SHARED_APPLICATION), ("kind", kind)])
            .await?;
        let Some(item) = shared.first() else {
            return Ok(None);
        };
        let text = String::from_utf8_lossy(item.secret().await?.as_bytes()).into_owned();
        let label = item.label().await?;
        keyring
            .create_item(&label, &attributes(kind), text.clone(), true)
            .await?;
        mark_own_entry(kind);
        Ok(Some(text))
    })
    .await
}

async fn save_text(kind: &'static str, label: &'static str, text: String) -> Result<(), String> {
    mark_own_entry(kind);
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
    mark_own_entry(kind);
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

/// The user's Spotify Soloist API key (CONCEPT.md §9.4).
const SOLOIST_KEY: &str = "soloist-api-key";

pub async fn load_soloist_key() -> Result<Option<String>, String> {
    load_text(SOLOIST_KEY).await
}

pub async fn save_soloist_key(key: String) -> Result<(), String> {
    save_text(SOLOIST_KEY, "Gatefold: Spotify Soloist API key", key).await
}

pub async fn delete_soloist_key() -> Result<(), String> {
    delete_kind(SOLOIST_KEY).await
}
