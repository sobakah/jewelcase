//! The sync with Jewelcase Mobile through sync files (CONCEPT.md §6.4; the format is Jewelcase
//! Mobile's `docs/SYNC.md`): `sync-folder` is a folder both reach (a cloud folder kept in step,
//! or one copied over USB); Jewelcase writes `jewelcase-sync-<device>.json` there and reads the
//! phone's. It syncs at start, when the window comes back (at most every five minutes), at
//! quit, and with *Sync Now* in Preferences. The merge is `Database::sync` (core).

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};

use gtk::{gio, glib, prelude::*};
use jewelcase_core::db::sync::{self, Device, SyncResult};

use crate::application::JewelcaseApplication;
use crate::config::APP_ID;

const DUE_SECONDS: i64 = 5 * 60;

thread_local! {
    /// The last sync's outcome, for Preferences.
    static LAST: RefCell<Option<Result<SyncResult, String>>> = const { RefCell::new(None) };
    static LAST_RUN: Cell<i64> = const { Cell::new(0) };
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
    LAST_RUN.set(glib::real_time() / 1_000_000);
    let result = run(&settings, &folder);
    match &result {
        Ok(done) => tracing::info!("Synced with {:?}, skipped {:?}", done.devices, done.skipped),
        Err(err) => tracing::warn!("The sync with Jewelcase Mobile failed: {err}"),
    }
    LAST.set(Some(result.clone()));
    Some(result)
}

fn run(settings: &gio::Settings, folder: &Path) -> Result<SyncResult, String> {
    let device = match settings.string("sync-device").as_str() {
        "" => {
            let id = glib::uuid_string_random().to_string();
            settings
                .set_string("sync-device", &id)
                .map_err(|e| e.to_string())?;
            id
        }
        id => id.to_owned(),
    };
    let mut files = Vec::new();
    for entry in std::fs::read_dir(folder).map_err(|e| format!("{}: {e}", folder.display()))? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(sync::PREFIX) || !name.ends_with(sync::SUFFIX) {
            continue;
        }
        match std::fs::read_to_string(entry.path()) {
            Ok(text) => files.push((name, text)),
            Err(err) => tracing::warn!("Cannot read {name}: {err}"),
        }
    }
    // lyrembed's alias list as last read, for the phone's matching.
    let aliases = std::fs::read_to_string(crate::config::aliases_cache_path()).ok();
    let folders = crate::library_folders::load(settings);
    let library = JewelcaseApplication::get().library();
    let (text, result) = {
        let Some(mut db) = library.db() else {
            return Err("the library isn't open".into());
        };
        db.sync(
            &Device {
                id: &device,
                name: "Jewelcase",
            },
            &files,
            &folders,
            aliases.as_deref(),
            sync::now_ms(),
        )
        .map_err(|e| e.to_string())?
    };
    // Whole or not at all: a temporary file renamed into place.
    let own = folder.join(sync::file_name(&device));
    let temp = folder.join(format!("{}.tmp", sync::file_name(&device)));
    std::fs::write(&temp, text).map_err(|e| format!("{}: {e}", temp.display()))?;
    std::fs::rename(&temp, &own).map_err(|e| format!("{}: {e}", own.display()))?;
    library.reload_playlists();
    Ok(result)
}
