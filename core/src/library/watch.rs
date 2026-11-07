//! Watches the library folders with inotify (CONCEPT.md §3.1).
//!
//! The watcher only reports that something changed; the caller debounces
//! and runs an incremental scan, which compares modification times.

use std::path::PathBuf;

use notify::event::{EventKind, ModifyKind};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};

pub struct LibraryWatcher {
    _watcher: RecommendedWatcher,
}

impl LibraryWatcher {
    /// Calls `on_change` (from a background thread) when files are created,
    /// changed, moved or removed in `folders`. Missing folders are skipped.
    pub fn new(folders: &[PathBuf], on_change: impl Fn() + Send + 'static) -> notify::Result<Self> {
        let mut watcher = RecommendedWatcher::new(
            move |result: notify::Result<notify::Event>| match result {
                Ok(event) if is_relevant(&event.kind) => on_change(),
                Ok(_) => {}
                Err(err) => tracing::warn!("File watcher error: {err}"),
            },
            Config::default().with_follow_symlinks(false),
        )?;
        // One folder that can't be watched (permissions, too many watches)
        // doesn't stop the others.
        for folder in folders.iter().filter(|f| f.is_dir()) {
            if let Err(err) = watcher.watch(folder, RecursiveMode::Recursive) {
                tracing::warn!("Cannot watch {}: {err}", folder.display());
            }
        }
        Ok(Self { _watcher: watcher })
    }
}

fn is_relevant(kind: &EventKind) -> bool {
    match kind {
        EventKind::Create(_) | EventKind::Remove(_) => true,
        EventKind::Modify(ModifyKind::Metadata(_)) => false,
        EventKind::Modify(_) => true,
        _ => false,
    }
}
