//! The sync with Jewelcase Mobile through sync files (CONCEPT.md §6.4; the format is Jewelcase
//! Mobile's `docs/SYNC.md`): `sync-folder` is a folder both reach (a cloud folder kept in step,
//! or one copied over USB); Jewelcase writes `jewelcase-sync-<device>.json` there and reads the
//! phone's. It syncs at start, when the window comes back (at most every five minutes), at
//! quit, and with *Sync Now* in Preferences. The merge is `Database::sync` (core).
//!
//! A phone connected over USB (MTP, mounted by GNOME's gvfs) is a sync folder too, when one of
//! its usual folders holds another device's sync file: Jewelcase offers to sync with it, or
//! syncs at once with `sync-connected-phone`.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;

use adw::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};
use jewelcase_core::db::sync::{self, Device, SyncResult};

use crate::application::JewelcaseApplication;
use crate::config::APP_ID;
use crate::window::JewelcaseWindow;

const DUE_SECONDS: i64 = 5 * 60;

/// Where on a phone's storage Jewelcase Mobile's sync folder usually is: the storage itself,
/// then these folders in it.
const PHONE_FOLDERS: [&str; 3] = ["Music", "Download", "Documents"];

thread_local! {
    /// The last sync's outcome, for Preferences.
    static LAST: RefCell<Option<Result<SyncResult, String>>> = const { RefCell::new(None) };
    static LAST_RUN: Cell<i64> = const { Cell::new(0) };
    /// The volume monitor, kept for its signals.
    static MONITOR: RefCell<Option<gio::VolumeMonitor>> = const { RefCell::new(None) };
    /// The offers on screen, by the phone's mount root, gone with the phone.
    static OFFERS: RefCell<Vec<(String, adw::Toast)>> = const { RefCell::new(Vec::new()) };
    /// The phones being synced with, by mount root.
    static SYNCING: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// The last sync's outcome (None: none this session).
pub fn last() -> Option<Result<SyncResult, String>> {
    LAST.with_borrow(Clone::clone)
}

/// When the window comes back: a sync, unless one ran in the last five minutes.
pub fn sync_if_due() {
    if glib::real_time() / 1_000_000 - LAST_RUN.get() >= DUE_SECONDS {
        sync_now();
    }
}

/// Syncs now, if a folder is set (None: none set).
pub fn sync_now() -> Option<Result<SyncResult, String>> {
    let settings = gio::Settings::new(APP_ID);
    let folder = PathBuf::from(settings.string("sync-folder").as_str());
    if folder.as_os_str().is_empty() {
        return None;
    }
    let folder = gio::File::for_path(folder);
    let result = device_id(&settings).and_then(|device| {
        let files = read_files(&folder)?;
        let (text, result) = merge(&settings, &device, &files)?;
        write_own(&folder, &device, &text)?;
        Ok(result)
    });
    Some(finish(result))
}

fn finish(result: Result<SyncResult, String>) -> Result<SyncResult, String> {
    LAST_RUN.set(glib::real_time() / 1_000_000);
    match &result {
        Ok(done) => {
            tracing::info!("Synced with {:?}, skipped {:?}", done.devices, done.skipped);
            JewelcaseApplication::get().library().reload_playlists();
        }
        Err(err) => tracing::warn!("The sync with Jewelcase Mobile failed: {err}"),
    }
    LAST.set(Some(result.clone()));
    result
}

/// This installation's ID in the sync files, made at the first sync.
fn device_id(settings: &gio::Settings) -> Result<String, String> {
    match settings.string("sync-device").as_str() {
        "" => {
            let id = glib::uuid_string_random().to_string();
            settings
                .set_string("sync-device", &id)
                .map_err(|e| e.to_string())?;
            Ok(id)
        }
        id => Ok(id.to_owned()),
    }
}

/// Merges the other devices' files into the library and returns this device's file.
fn merge(
    settings: &gio::Settings,
    device: &str,
    files: &[(String, String)],
) -> Result<(String, SyncResult), String> {
    // lyrembed's alias list as last read, for the phone's matching.
    let aliases = std::fs::read_to_string(crate::config::aliases_cache_path()).ok();
    let folders = crate::library_folders::load(settings);
    let library = JewelcaseApplication::get().library();
    let Some(mut db) = library.db() else {
        return Err("the library isn't open".into());
    };
    db.sync(
        &Device {
            id: device,
            name: "Jewelcase",
        },
        files,
        &folders,
        aliases.as_deref(),
        sync::now_ms(),
    )
    .map_err(|e| e.to_string())
}

fn is_sync_file(name: &str) -> bool {
    name.starts_with(sync::PREFIX) && name.ends_with(sync::SUFFIX)
}

/// The sync files in a folder (name and text), through GIO: a local folder or one on a phone.
fn read_files(folder: &gio::File) -> Result<Vec<(String, String)>, String> {
    let mut files = Vec::new();
    for name in file_names(folder)? {
        if !is_sync_file(&name) {
            continue;
        }
        let text = folder
            .child(&name)
            .load_contents(gio::Cancellable::NONE)
            .map_err(|e| e.to_string())
            .and_then(|(bytes, _)| String::from_utf8(bytes.to_vec()).map_err(|e| e.to_string()));
        match text {
            Ok(text) => files.push((name, text)),
            Err(err) => tracing::warn!("Cannot read {name}: {err}"),
        }
    }
    Ok(files)
}

fn file_names(folder: &gio::File) -> Result<Vec<String>, String> {
    let children = folder
        .enumerate_children(
            gio::FILE_ATTRIBUTE_STANDARD_NAME,
            gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
            gio::Cancellable::NONE,
        )
        .map_err(|e| format!("{}: {e}", folder.parse_name()))?;
    Ok(children
        .flatten()
        .map(|info| info.name().to_string_lossy().into_owned())
        .collect())
}

/// Writes this device's file, whole or not at all: a temporary file moved into place.
fn write_own(folder: &gio::File, device: &str, text: &str) -> Result<(), String> {
    write_own_with(folder, device, text, |from, to| {
        from.move_(
            to,
            gio::FileCopyFlags::OVERWRITE,
            gio::Cancellable::NONE,
            None,
        )
    })
}

/// `write_own` with the move given. A phone's folder through gvfs's FUSE mount (the portal's
/// path when the sync folder is on the phone) takes a rename onto an existing file as done
/// but only deletes the target, the temporary file keeping its name: then it is moved again,
/// onto no file now.
fn write_own_with(
    folder: &gio::File,
    device: &str,
    text: &str,
    move_file: impl Fn(&gio::File, &gio::File) -> Result<(), glib::Error>,
) -> Result<(), String> {
    let own = folder.child(sync::file_name(device));
    let temp = folder.child(format!("{}.tmp", sync::file_name(device)));
    temp.replace_contents(
        text.as_bytes(),
        None,
        false,
        gio::FileCreateFlags::NONE,
        gio::Cancellable::NONE,
    )
    .map_err(|e| e.to_string())?;
    move_file(&temp, &own).map_err(|e| e.to_string())?;
    if temp.query_exists(gio::Cancellable::NONE) {
        tracing::info!("{} stayed after its move: moved again", temp.parse_name());
        move_file(&temp, &own).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The folder on a connected phone that holds another device's sync file (`own`: this
/// device's file name, which doesn't count): each storage (an MTP mount's top folders), and
/// in it Music, Download and Documents.
fn find_on_phone(root: &gio::File, own: &str) -> Option<gio::File> {
    let has_other = |folder: &gio::File| {
        file_names(folder)
            .is_ok_and(|names| names.iter().any(|name| is_sync_file(name) && name != own))
    };
    let mut storages = file_names(root).unwrap_or_default();
    storages.sort();
    for storage in storages {
        let storage = root.child(storage);
        let candidates =
            std::iter::once(storage.clone()).chain(PHONE_FOLDERS.iter().map(|f| storage.child(f)));
        for folder in candidates {
            if has_other(&folder) {
                return Some(folder);
            }
        }
    }
    None
}

/// Watches for phones connected over USB (once, with the first window): those already there
/// and those to come.
pub fn watch_phones() {
    if MONITOR.with_borrow(Option::is_some) {
        return;
    }
    let monitor = gio::VolumeMonitor::get();
    monitor.connect_mount_added(|_, mount| look_at(mount));
    monitor.connect_mount_removed(|_, mount| {
        let uri = mount.root().uri().to_string();
        OFFERS.with_borrow_mut(|offers| {
            offers.retain(|(root, toast)| {
                if *root == uri {
                    toast.dismiss();
                }
                *root != uri
            });
        });
    });
    for mount in monitor.mounts() {
        look_at(&mount);
    }
    MONITOR.set(Some(monitor));
}

/// A mount that is a phone (MTP): its sync folder looked for away from the main loop, then
/// offered or synced.
fn look_at(mount: &gio::Mount) {
    let root = mount.root();
    if root.uri_scheme().as_deref() != Some("mtp") {
        return;
    }
    let settings = gio::Settings::new(APP_ID);
    let Ok(device) = device_id(&settings) else {
        return;
    };
    let uri = root.uri().to_string();
    glib::spawn_future_local(async move {
        let own = sync::file_name(&device);
        let Ok(Some(folder)) = gio::spawn_blocking(move || find_on_phone(&root, &own)).await else {
            return;
        };
        let name = phone_name(&uri);
        tracing::info!("Jewelcase Mobile's sync file on {name}: {}", folder.uri());
        if settings.boolean("sync-connected-phone") {
            sync_with_phone(folder, name, uri).await;
        } else {
            offer(folder, name, uri);
        }
    });
}

/// "Sync with Jewelcase Mobile on <phone>?" until the phone goes or the button is pressed
/// (`root`: the phone's mount root, which names it in `OFFERS`).
fn offer(folder: gio::File, name: String, root: String) {
    let Some(window) = JewelcaseApplication::get()
        .active_window()
        .and_downcast::<JewelcaseWindow>()
    else {
        return;
    };
    if OFFERS.with_borrow(|offers| offers.iter().any(|(r, _)| *r == root)) {
        return;
    }
    // Translators: {} is a phone's name, such as "Pixel 9".
    let toast = adw::Toast::new(&gettext("Sync with Jewelcase Mobile on {}?").replace("{}", &name));
    toast.set_use_markup(false);
    toast.set_button_label(Some(&gettext("_Sync")));
    toast.set_timeout(0);
    let to_sync = root.clone();
    toast.connect_button_clicked(move |_| {
        let (folder, name, root) = (folder.clone(), name.clone(), to_sync.clone());
        glib::spawn_future_local(sync_with_phone(folder, name, root));
    });
    toast.connect_dismissed(|toast| {
        OFFERS.with_borrow_mut(|offers| offers.retain(|(_, t)| t != toast));
    });
    OFFERS.with_borrow_mut(|offers| offers.push((root, toast.clone())));
    window.add_toast(toast);
}

/// The phone's name: GNOME's MTP volume monitor names its mount after the phone ("Pixel 9"),
/// while gvfs's own mount of it, shadowed by that one, is just "mtp".
fn phone_name(root: &str) -> String {
    let mounts = MONITOR.with_borrow(|monitor| {
        monitor
            .as_ref()
            .map(VolumeMonitorExt::mounts)
            .unwrap_or_default()
    });
    let same: Vec<gio::Mount> = mounts
        .into_iter()
        .filter(|mount| mount.root().uri() == root)
        .collect();
    same.iter()
        .find(|mount| !mount.is_shadowed())
        .or(same.first())
        .map_or_else(|| root.to_owned(), |mount| mount.name().to_string())
}

/// A sync with the folder on a phone (`root`: its mount root; the same phone may come as two
/// mounts): reading and writing away from the main loop (MTP is slow), the merge on it.
async fn sync_with_phone(folder: gio::File, name: String, root: String) {
    if SYNCING.with_borrow(|syncing| syncing.contains(&root)) {
        return;
    }
    SYNCING.with_borrow_mut(|syncing| syncing.push(root.clone()));
    let settings = gio::Settings::new(APP_ID);
    let result = async {
        let device = device_id(&settings)?;
        let from = folder.clone();
        let files = gio::spawn_blocking(move || read_files(&from))
            .await
            .map_err(|_| "reading the phone's files failed".to_owned())??;
        let (text, result) = merge(&settings, &device, &files)?;
        let to = folder.clone();
        gio::spawn_blocking(move || write_own(&to, &device, &text))
            .await
            .map_err(|_| "writing to the phone failed".to_owned())??;
        Ok(result)
    }
    .await;
    SYNCING.with_borrow_mut(|syncing| syncing.retain(|r| *r != root));
    let message = match finish(result) {
        // Translators: {} is a phone's name.
        Ok(_) => gettext("Synced with Jewelcase Mobile on {}").replace("{}", &name),
        // Translators: the first {} is a phone's name, the second the error.
        Err(err) => gettext("The sync with {} failed: {}")
            .replacen("{}", &name, 1)
            .replacen("{}", &err, 1),
    };
    if let Some(window) = JewelcaseApplication::get()
        .active_window()
        .and_downcast::<JewelcaseWindow>()
    {
        window.show_toast(&message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &std::path::Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn finds_the_phones_sync_file_in_its_music_folder() {
        let phone = tempfile::tempdir().unwrap();
        let storage = phone.path().join("Internal shared storage");
        write(&storage.join("Download/notes.txt"), "");
        write(&storage.join("Music/jewelcase-sync-phone.json"), "{}");
        let root = gio::File::for_path(phone.path());
        let found = find_on_phone(&root, "jewelcase-sync-desktop.json").unwrap();
        assert_eq!(found.path().unwrap(), storage.join("Music"));
    }

    #[test]
    fn the_desktops_own_file_alone_is_no_sync_folder() {
        let phone = tempfile::tempdir().unwrap();
        let storage = phone.path().join("Internal shared storage");
        write(&storage.join("Music/jewelcase-sync-desktop.json"), "{}");
        write(
            &storage.join("Download/jewelcase-sync-phone.json.tmp"),
            "{}",
        );
        let root = gio::File::for_path(phone.path());
        assert!(find_on_phone(&root, "jewelcase-sync-desktop.json").is_none());
    }

    #[test]
    fn reads_the_sync_files_and_writes_its_own_whole() {
        let folder = tempfile::tempdir().unwrap();
        write(&folder.path().join("jewelcase-sync-phone.json"), "phone");
        write(&folder.path().join("song.flac"), "");
        write(&folder.path().join("jewelcase-sync-desktop.json"), "old");
        let dir = gio::File::for_path(folder.path());

        let mut files = read_files(&dir).unwrap();
        files.sort();
        assert_eq!(
            files,
            [
                ("jewelcase-sync-desktop.json".to_owned(), "old".to_owned()),
                ("jewelcase-sync-phone.json".to_owned(), "phone".to_owned()),
            ]
        );

        write_own(&dir, "desktop", "new").unwrap();
        let own = folder.path().join("jewelcase-sync-desktop.json");
        assert_eq!(std::fs::read_to_string(own).unwrap(), "new");
        assert!(
            !folder
                .path()
                .join("jewelcase-sync-desktop.json.tmp")
                .exists()
        );
    }

    /// gvfs's FUSE mount of a phone: a rename onto an existing file deletes that file and
    /// leaves the source where it was, and says it worked.
    fn fuse_mtp_move(from: &gio::File, to: &gio::File) -> Result<(), glib::Error> {
        if to.query_exists(gio::Cancellable::NONE) {
            return to.delete(gio::Cancellable::NONE);
        }
        from.move_(to, gio::FileCopyFlags::NONE, gio::Cancellable::NONE, None)
    }

    #[test]
    fn its_file_lands_under_its_name_on_a_phone_through_fuse() {
        let folder = tempfile::tempdir().unwrap();
        write(&folder.path().join("jewelcase-sync-desktop.json"), "old");
        let dir = gio::File::for_path(folder.path());

        write_own_with(&dir, "desktop", "new", fuse_mtp_move).unwrap();
        let own = folder.path().join("jewelcase-sync-desktop.json");
        assert_eq!(std::fs::read_to_string(own).unwrap(), "new");
        assert!(
            !folder
                .path()
                .join("jewelcase-sync-desktop.json.tmp")
                .exists()
        );
    }
}
