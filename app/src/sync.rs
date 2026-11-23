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
use jewelcase_core::db::sync::{self, Device, SyncDevice, SyncResult};

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
            tracing::info!(
                "Synced with {:?}, skipped {:?}",
                done.synced(),
                done.skipped
            );
            let found: Vec<(String, String, i64)> = done
                .devices
                .iter()
                .map(|d| (d.id.clone(), d.name.clone(), d.written))
                .collect();
            if let Err(err) =
                gio::Settings::new(APP_ID).set_value("sync-devices", &found.to_variant())
            {
                tracing::warn!("Cannot keep the devices in the sync folder: {err}");
            }
            JewelcaseApplication::get().library().reload_playlists();
        }
        Err(err) => tracing::warn!("The sync with Jewelcase Mobile failed: {err}"),
    }
    LAST.set(Some(result.clone()));
    result
}

/// The other devices found at the last sync (newest first), each synced unless turned off.
pub fn devices() -> Vec<SyncDevice> {
    let settings = gio::Settings::new(APP_ID);
    let off = settings.strv("sync-off-devices");
    settings
        .get::<Vec<(String, String, i64)>>("sync-devices")
        .into_iter()
        .map(|(id, name, written)| SyncDevice {
            synced: !off.iter().any(|o| o.as_str() == id),
            id,
            name,
            written,
        })
        .collect()
}

/// Turns syncing with `device` on (and syncs, to read it now) or off.
pub fn set_synced(device: &str, on: bool) {
    let settings = gio::Settings::new(APP_ID);
    let mut off: Vec<String> = settings
        .strv("sync-off-devices")
        .iter()
        .map(|o| o.to_string())
        .filter(|o| o != device)
        .collect();
    if !on {
        off.push(device.to_owned());
    }
    if let Err(err) = settings.set_strv("sync-off-devices", off) {
        tracing::warn!("Cannot keep the devices turned off: {err}");
    } else if on {
        sync_now();
    }
}

/// This installation's name in its sync file: the Devel build's its own, so the other
/// devices tell them apart.
fn device_name() -> &'static str {
    if crate::config::is_development() {
        "Jewelcase (Development)"
    } else {
        "Jewelcase"
    }
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
    let off: Vec<String> = settings
        .strv("sync-off-devices")
        .iter()
        .map(|o| o.to_string())
        .collect();
    db.sync(
        &Device {
            id: device,
            name: device_name(),
        },
        files,
        &off,
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

/// Writes this device's file. On a local folder a temporary file is moved into place, so a
/// reader never sees half of it. On a phone (`mtp://`, or a FUSE path over it: gvfs's, or the
/// portal's when the sync folder is on the phone) renames can't be trusted: gvfs's MTP backend
/// keeps the old name pointing at the renamed file, so the next sync's write to the temporary
/// file went into this one and its move deleted both. There the file is deleted and written
/// anew, without a rename.
fn write_own(folder: &gio::File, device: &str, text: &str) -> Result<(), String> {
    let fs_type = folder
        .query_filesystem_info(gio::FILE_ATTRIBUTE_FILESYSTEM_TYPE, gio::Cancellable::NONE)
        .ok()
        .and_then(|info| info.attribute_string(gio::FILE_ATTRIBUTE_FILESYSTEM_TYPE))
        .map(String::from);
    write_own_on(folder, device, text, fs_type.as_deref())
}

/// `write_own` on a folder whose filesystem type is `fs_type` (GIO's `filesystem::type`).
fn write_own_on(
    folder: &gio::File,
    device: &str,
    text: &str,
    fs_type: Option<&str>,
) -> Result<(), String> {
    let own = folder.child(sync::file_name(device));
    let error = |e: glib::Error| format!("{}: {e}", own.parse_name());
    if !renames_safely(fs_type) {
        match own.delete(gio::Cancellable::NONE) {
            Err(e) if !e.matches(gio::IOErrorEnum::NotFound) => return Err(error(e)),
            _ => {}
        }
        // Not replace_contents: on a FUSE path GLib replaces a file through a rename too.
        let stream = own
            .create(gio::FileCreateFlags::NONE, gio::Cancellable::NONE)
            .map_err(error)?;
        stream
            .write_all(text.as_bytes(), gio::Cancellable::NONE)
            .map_err(error)?;
        return stream.close(gio::Cancellable::NONE).map_err(error);
    }
    let temp = folder.child(format!("{}.tmp", sync::file_name(device)));
    temp.replace_contents(
        text.as_bytes(),
        None,
        false,
        gio::FileCreateFlags::NONE,
        gio::Cancellable::NONE,
    )
    .map_err(error)?;
    temp.move_(
        &own,
        gio::FileCopyFlags::OVERWRITE,
        gio::Cancellable::NONE,
        None,
    )
    .map_err(error)
}

/// Whether a rename onto an existing file can be trusted on a filesystem of this type: not on
/// a phone through gvfs (`mtpfs`), nor through FUSE (gvfs's mount, or the portal's, which may
/// be over it), nor when the type is unknown.
fn renames_safely(fs_type: Option<&str>) -> bool {
    fs_type.is_some_and(|t| t != "mtpfs" && t != "fuse" && !t.starts_with("fuse."))
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

    #[test]
    fn a_phones_folder_is_written_without_a_rename() {
        for fs_type in [None, Some("mtpfs"), Some("fuse"), Some("fuse.portal")] {
            assert!(!renames_safely(fs_type), "{fs_type:?}");
        }
        for fs_type in ["btrfs", "ext4", "tmpfs", "xfs"] {
            assert!(renames_safely(Some(fs_type)), "{fs_type}");
        }
    }

    #[test]
    fn its_file_is_written_anew_on_a_phone() {
        let folder = tempfile::tempdir().unwrap();
        write(
            &folder.path().join("jewelcase-sync-desktop.json"),
            "old, and longer",
        );
        let dir = gio::File::for_path(folder.path());

        for text in ["new", "newer"] {
            write_own_on(&dir, "desktop", text, Some("mtpfs")).unwrap();
            let own = folder.path().join("jewelcase-sync-desktop.json");
            assert_eq!(std::fs::read_to_string(own).unwrap(), text);
        }
        let names: Vec<_> = std::fs::read_dir(folder.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["jewelcase-sync-desktop.json"]);
    }
}
