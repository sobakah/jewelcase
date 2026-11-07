//! The Spotify login in the keyring (oo7, CONCEPT.md §9.3, §12). Runs on a
//! worker thread; oo7 brings its own async runtime (async-io).

use gatefold_spotify::Tokens;
use gtk::gio;

/// Keyring attributes; `application` as in CONCEPT.md §12.
const ATTRIBUTES: [(&str, &str); 2] = [
    ("application", "io.github.sobakah.Gatefold"),
    ("kind", "spotify-login"),
];

pub async fn load() -> Result<Option<Tokens>, String> {
    run(|| async {
        let keyring = oo7::Keyring::new().await?;
        keyring.unlock().await?;
        let items = keyring.search_items(&ATTRIBUTES).await?;
        let Some(item) = items.first() else {
            return Ok(None);
        };
        let secret = item.secret().await?;
        Ok(Tokens::from_json(&String::from_utf8_lossy(
            secret.as_bytes(),
        )))
    })
    .await
}

pub async fn save(tokens: Tokens) -> Result<(), String> {
    run(move || async move {
        let keyring = oo7::Keyring::new().await?;
        keyring.unlock().await?;
        keyring
            .create_item(
                "Gatefold: Spotify login",
                &ATTRIBUTES,
                tokens.to_json(),
                true,
            )
            .await
    })
    .await
}

pub async fn delete() -> Result<(), String> {
    run(|| async {
        let keyring = oo7::Keyring::new().await?;
        keyring.delete(&ATTRIBUTES).await
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
