//! The Spotify logins in the keyring (oo7, CONCEPT.md §9.3, §12). Runs on a
//! worker thread; oo7 brings its own async runtime (async-io).
//!
//! Without a keyring service (some desktops and window managers have
//! none, and in a Flatpak the secret portal may be missing), the logins
//! go to a file only the user can read (`file_store`), and Preferences say
//! so; they used to be lost at every restart.
#![cfg_attr(not(feature = "spotify-stream"), allow(dead_code))]

use std::sync::OnceLock;

use gtk::prelude::*;
use gtk::{gio, glib};
use jewelcase_spotify::Tokens;

/// Keyring attributes; `application` is the app ID (CONCEPT.md §12), so
/// the development build and the installed app keep their own logins;
/// `kind` tells the logins and keys apart.
fn attributes(kind: &'static str) -> [(&'static str, &'static str); 2] {
    [("application", crate::config::APP_ID), ("kind", kind)]
}

/// Where both builds kept their logins before: read once by the
/// development build when it has none of its own (under the old name too).
const SHARED_APPLICATIONS: [&str; 2] =
    ["io.github.sobakah.Jewelcase", "io.github.sobakah.Gatefold"];

/// Once the build has keyring entries of its own (or chose to have none,
/// by disconnecting), the shared ones aren't looked at again.
fn own_entry_marker(kind: &str) -> std::path::PathBuf {
    gtk::glib::user_data_dir()
        .join(crate::config::APP_ID)
        .join(format!("keyring-own-{kind}"))
}

/// Once Gatefold's entries (the old name, `rename.rs`) were looked for, or
/// the build saved or removed its own, they aren't looked at again (a
/// disconnect mustn't bring the old login back).
fn renamed_marker(kind: &str) -> std::path::PathBuf {
    gtk::glib::user_data_dir()
        .join(crate::config::APP_ID)
        .join(format!("keyring-renamed-{kind}"))
}

fn mark_renamed(kind: &str) {
    mark(&renamed_marker(kind));
}

fn mark(marker: &std::path::Path) {
    if let Some(dir) = marker.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(marker, b"");
}

/// A copy made once: `marker` is left only when `copy` succeeded, so a
/// copy that failed (a dismissed unlock prompt) is tried again next time.
async fn once<T, E>(
    marker: &std::path::Path,
    copy: impl std::future::Future<Output = Result<T, E>>,
) -> Result<T, E> {
    let result = copy.await;
    if result.is_ok() {
        mark(marker);
    }
    result
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
    save_text(LOGIN, "Jewelcase: Spotify login", tokens.to_json()).await
}

pub async fn delete() -> Result<(), String> {
    delete_kind(LOGIN).await
}

pub async fn load_stream() -> Result<Option<String>, String> {
    load_text(STREAM_LOGIN).await
}

pub async fn save_stream(text: String) -> Result<(), String> {
    save_text(STREAM_LOGIN, "Jewelcase: Spotify streaming login", text).await
}

pub async fn delete_stream() -> Result<(), String> {
    delete_kind(STREAM_LOGIN).await
}

async fn load_text(kind: &'static str) -> Result<Option<String>, String> {
    if uses_file().await {
        return file_store::load(kind);
    }
    // Saved there while no keyring was running. Saving to the keyring
    // removes the file's copy, so one that's there is newer than the
    // keyring's (whose refresh token Spotify has replaced since), and it
    // works also when the keyring can't be unlocked.
    if let Some(text) = file_store::load(kind).ok().flatten() {
        return Ok(Some(text));
    }
    let found = run(move || async move {
        let keyring = oo7::Keyring::new().await?;
        keyring.unlock().await?;
        let items = keyring.search_items(&attributes(kind)).await?;
        if let Some(item) = items.first() {
            let secret = item.secret().await?;
            return Ok(Some(
                String::from_utf8_lossy(secret.as_bytes()).into_owned(),
            ));
        }
        // This build's login under the old name (Gatefold): copied once;
        // Gatefold's entry stays as it was.
        let former = crate::rename::former_app_id();
        let renamed = renamed_marker(kind);
        if !renamed.exists() {
            let copied = once(&renamed, async {
                let old = keyring
                    .search_items(&[("application", former.as_str()), ("kind", kind)])
                    .await?;
                let Some(item) = old.first() else {
                    return Ok(None);
                };
                let text = String::from_utf8_lossy(item.secret().await?.as_bytes()).into_owned();
                let label = item.label().await?.replacen("Gatefold", "Jewelcase", 1);
                keyring
                    .create_item(&label, &attributes(kind), text.clone(), true)
                    .await?;
                Ok::<_, oo7::Error>(Some(text))
            })
            .await?;
            if copied.is_some() {
                return Ok(copied);
            }
        }
        if SHARED_APPLICATIONS.contains(&crate::config::APP_ID) || own_entry_marker(kind).exists() {
            return Ok(None);
        }
        // The development build's login from before it had its own: copied,
        // the installed app's stays as it is.
        for shared in SHARED_APPLICATIONS {
            let found = keyring
                .search_items(&[("application", shared), ("kind", kind)])
                .await?;
            let Some(item) = found.first() else {
                continue;
            };
            let text = String::from_utf8_lossy(item.secret().await?.as_bytes()).into_owned();
            let label = item.label().await?;
            keyring
                .create_item(&label, &attributes(kind), text.clone(), true)
                .await?;
            mark_own_entry(kind);
            return Ok(Some(text));
        }
        Ok(None)
    })
    .await?;
    Ok(found)
}

async fn save_text(kind: &'static str, label: &'static str, text: String) -> Result<(), String> {
    mark_own_entry(kind);
    mark_renamed(kind);
    if uses_file().await {
        return file_store::save(kind, &text);
    }
    run(move || async move {
        let keyring = oo7::Keyring::new().await?;
        keyring.unlock().await?;
        keyring
            .create_item(label, &attributes(kind), text, true)
            .await
    })
    .await?;
    // In the keyring now: no copy stays in the file (one that can't be
    // read doesn't make the save fail).
    if let Err(err) = file_store::delete(kind) {
        tracing::warn!(
            "Cannot remove the login from {}: {err}",
            file_store::path().display()
        );
    }
    Ok(())
}

async fn delete_kind(kind: &'static str) -> Result<(), String> {
    mark_own_entry(kind);
    mark_renamed(kind);
    // Both places, whatever the other says: a login must not survive a
    // disconnect because one of them failed.
    let in_file = file_store::delete(kind);
    if uses_file().await {
        return in_file;
    }
    let in_keyring = run(move || async move {
        let keyring = oo7::Keyring::new().await?;
        keyring.delete(&attributes(kind)).await
    })
    .await;
    if let Err(err) = &in_file {
        tracing::warn!(
            "Cannot remove the login from {}: {err}",
            file_store::path().display()
        );
    }
    in_keyring
}

/// Whether the logins go to the file, as there's no keyring service:
/// neither the Secret Service (`org.freedesktop.secrets`, from GNOME
/// Keyring, KWallet, KeePassXC…) nor, in a Flatpak, the secret portal.
/// Asked once per run.
pub async fn uses_file() -> bool {
    static NO_KEYRING: OnceLock<bool> = OnceLock::new();
    if disabled() {
        return false;
    }
    if let Some(answer) = NO_KEYRING.get() {
        return *answer;
    }
    let available = gio::spawn_blocking(keyring_available).await.unwrap_or(true);
    if !available {
        tracing::warn!(
            "No keyring service; Spotify logins are kept in {}",
            file_store::path().display()
        );
    }
    *NO_KEYRING.get_or_init(|| !available)
}

fn keyring_available() -> bool {
    let Ok(bus) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else {
        return false;
    };
    let call = |name: &str, path: &str, interface: &str, method: &str, args: glib::Variant| {
        bus.call_sync(
            Some(name),
            path,
            interface,
            method,
            Some(&args),
            None,
            gio::DBusCallFlags::NONE,
            2000,
            gio::Cancellable::NONE,
        )
    };
    let dbus = |method: &str, args: glib::Variant| {
        call(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            method,
            args,
        )
    };
    const SERVICE: &str = "org.freedesktop.secrets";
    let running = dbus("NameHasOwner", (SERVICE,).to_variant())
        .ok()
        .and_then(|reply| reply.get::<(bool,)>())
        .is_some_and(|(owned,)| owned);
    let startable = || {
        dbus("ListActivatableNames", ().to_variant())
            .ok()
            .and_then(|reply| reply.get::<(Vec<String>,)>())
            .is_some_and(|(names,)| names.iter().any(|n| n == SERVICE))
    };
    // In a Flatpak, oo7 keeps the logins in a file of its own, encrypted
    // with a key from the secret portal.
    let portal = || {
        crate::config::is_flatpak()
            && call(
                "org.freedesktop.portal.Desktop",
                "/org/freedesktop/portal/desktop",
                "org.freedesktop.DBus.Properties",
                "Get",
                ("org.freedesktop.portal.Secret", "version").to_variant(),
            )
            .is_ok()
    };
    running || startable() || portal()
}

/// The logins without a keyring: one JSON object (kind → text) in a file
/// only the user can read (0600), next to the library database.
mod file_store {
    use std::collections::BTreeMap;
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::path::{Path, PathBuf};

    pub fn path() -> PathBuf {
        gtk::glib::user_data_dir()
            .join(crate::config::APP_ID)
            .join("logins.json")
    }

    fn read(path: &Path) -> Result<BTreeMap<String, String>, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| e.to_string()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(err) => Err(err.to_string()),
        }
    }

    fn write(path: &Path, entries: &BTreeMap<String, String>) -> Result<(), String> {
        if entries.is_empty() {
            return match std::fs::remove_file(path) {
                Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err.to_string()),
                _ => Ok(()),
            };
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        // Written beside it and renamed: never half a file, never readable
        // by others for a moment.
        let temporary = path.with_extension("json.new");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        // Also when an old temporary file was left with other permissions.
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
        let text = serde_json::to_string(entries).map_err(|e| e.to_string())?;
        file.write_all(text.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|e| e.to_string())?;
        std::fs::rename(&temporary, path).map_err(|e| e.to_string())
    }

    pub fn load(kind: &str) -> Result<Option<String>, String> {
        load_at(&path(), kind)
    }

    pub fn save(kind: &str, text: &str) -> Result<(), String> {
        save_at(&path(), kind, text)
    }

    pub fn delete(kind: &str) -> Result<(), String> {
        delete_at(&path(), kind)
    }

    fn load_at(path: &Path, kind: &str) -> Result<Option<String>, String> {
        Ok(read(path)?.remove(kind))
    }

    fn save_at(path: &Path, kind: &str, text: &str) -> Result<(), String> {
        let mut entries = read(path)?;
        entries.insert(kind.to_owned(), text.to_owned());
        write(path, &entries)
    }

    fn delete_at(path: &Path, kind: &str) -> Result<(), String> {
        let mut entries = read(path)?;
        if entries.remove(kind).is_some() {
            write(path, &entries)?;
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn kept_private_and_removed_when_empty() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("sub/logins.json");
            assert_eq!(load_at(&path, "a").unwrap(), None);
            save_at(&path, "a", "one").unwrap();
            save_at(&path, "b", "two").unwrap();
            assert_eq!(load_at(&path, "a").unwrap().as_deref(), Some("one"));
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
            delete_at(&path, "a").unwrap();
            assert_eq!(load_at(&path, "a").unwrap(), None);
            assert_eq!(load_at(&path, "b").unwrap().as_deref(), Some("two"));
            delete_at(&path, "b").unwrap();
            assert!(!path.exists());
        }
    }
}

/// Headless tests (build-aux/headless.sh) never touch a keyring.
pub fn disabled() -> bool {
    std::env::var_os("JEWELCASE_NO_KEYRING").is_some()
}

async fn run<T, F, Fut>(job: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = oo7::Result<T>>,
{
    if disabled() {
        return Err("the keyring is disabled (JEWELCASE_NO_KEYRING)".into());
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
    save_text(SOLOIST_KEY, "Jewelcase: Spotify Soloist API key", key).await
}

pub async fn delete_soloist_key() -> Result<(), String> {
    delete_kind(SOLOIST_KEY).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_copy_is_tried_again() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("data/keyring-renamed-token");
        let failed: Result<(), &str> =
            futures_lite::future::block_on(once(&marker, async { Err("prompt dismissed") }));
        assert!(failed.is_err());
        assert!(!marker.exists());
        let copied: Result<Option<&str>, &str> =
            futures_lite::future::block_on(once(&marker, async { Ok(Some("login")) }));
        assert_eq!(copied, Ok(Some("login")));
        assert!(marker.exists());
    }
}
