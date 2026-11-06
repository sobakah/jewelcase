//! The library as the UI sees it: albums in a list model, scans on a
//! background thread, and folder watching (CONCEPT.md §3.1).

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use adw::prelude::*;
use gatefold_core::db::{Database, Track};
use gatefold_core::library::covers::CoverCache;
use gatefold_core::library::scanner::{self, ScanProgress, ScanSummary};
use gatefold_core::library::watch::LibraryWatcher;
use gettextrs::gettext;
use gtk::glib::subclass::Signal;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};

use crate::config::{self, APP_ID};
use crate::library_folders;
use crate::objects::{AlbumObject, TrackObject};

/// Changes are collected this long before an incremental scan starts.
const WATCH_DEBOUNCE: Duration = Duration::from_secs(3);

enum ScanMessage {
    Progress(ScanProgress),
    Done(Result<ScanSummary, String>),
}

mod imp {
    use super::*;

    #[derive(Debug, Default, glib::Properties)]
    #[properties(wrapper_type = super::Library)]
    pub struct Library {
        /// True while a scan runs.
        #[property(get)]
        pub scanning: Cell<bool>,
        /// Shown in the scan banner.
        #[property(get)]
        pub scan_status: RefCell<String>,
        /// True if the library has no albums.
        #[property(get)]
        pub empty: Cell<bool>,

        pub albums: OnceCell<gio::ListStore>,
        pub db: OnceCell<RefCell<Database>>,
        pub settings: OnceCell<gio::Settings>,
        pub rescan_pending: Cell<bool>,
        pub watcher: RefCell<Option<WatcherHandle>>,
        pub watch_timeout: RefCell<Option<glib::SourceId>>,
    }

    pub struct WatcherHandle(#[allow(dead_code)] pub LibraryWatcher);

    impl std::fmt::Debug for WatcherHandle {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("LibraryWatcher")
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Library {
        const NAME: &'static str = "GatefoldLibrary";
        type Type = super::Library;
    }

    #[glib::derived_properties]
    impl ObjectImpl for Library {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    // Albums were reloaded from the database.
                    Signal::builder("reloaded").build(),
                    // A message for the user (toast).
                    Signal::builder("message")
                        .param_types([String::static_type()])
                        .build(),
                ]
            })
        }

        fn constructed(&self) {
            self.parent_constructed();
            self.empty.set(true);
            self.albums
                .set(gio::ListStore::new::<AlbumObject>())
                .expect("set once");
        }
    }
}

glib::wrapper! {
    pub struct Library(ObjectSubclass<imp::Library>);
}

impl Library {
    pub fn new() -> Self {
        glib::Object::new()
    }

    /// Opens the database, shows what's in it and starts a scan.
    pub fn start(&self) -> Result<(), String> {
        let imp = self.imp();
        let db = Database::open(&config::database_path()).map_err(|e| e.to_string())?;
        imp.db.set(RefCell::new(db)).map_err(|_| "started twice")?;

        let settings = gio::Settings::new(APP_ID);
        settings.connect_changed(
            Some("library-folders"),
            glib::clone!(
                #[weak(rename_to = library)]
                self,
                move |_, _| {
                    library.watch_folders();
                    library.rescan();
                }
            ),
        );
        imp.settings.set(settings).expect("set once");

        self.reload();
        self.watch_folders();
        self.rescan();
        Ok(())
    }

    /// Album groups, in database order (the window sorts them).
    pub fn albums(&self) -> &gio::ListStore {
        self.imp().albums.get().expect("set in constructed()")
    }

    pub fn db(&self) -> Option<std::cell::RefMut<'_, Database>> {
        self.imp().db.get().map(RefCell::borrow_mut)
    }

    pub fn album_tracks(&self, album_id: i64) -> Vec<TrackObject> {
        let Some(db) = self.db() else {
            return Vec::new();
        };
        match db.album_tracks(album_id) {
            Ok(tracks) => tracks.into_iter().map(TrackObject::new).collect(),
            Err(err) => {
                tracing::warn!("Cannot load tracks of album {album_id}: {err}");
                Vec::new()
            }
        }
    }

    pub fn tracks(&self, ids: &[i64]) -> HashMap<i64, Track> {
        let Some(db) = self.db() else {
            return HashMap::new();
        };
        db.tracks(ids).unwrap_or_else(|err| {
            tracing::warn!("Cannot load tracks: {err}");
            HashMap::new()
        })
    }

    /// Reads the albums from the database into the list model.
    fn reload(&self) {
        let albums = match self.db().map(|db| db.albums()) {
            Some(Ok(albums)) => albums,
            Some(Err(err)) => {
                tracing::warn!("Cannot load albums: {err}");
                return;
            }
            None => return,
        };
        // Group editions, keeping the first-seen order.
        let mut order: Vec<String> = Vec::new();
        let mut groups: HashMap<String, Vec<_>> = HashMap::new();
        for album in albums {
            if !groups.contains_key(&album.group_key) {
                order.push(album.group_key.clone());
            }
            groups
                .entry(album.group_key.clone())
                .or_default()
                .push(album);
        }
        let objects: Vec<AlbumObject> = order
            .into_iter()
            .filter_map(|key| groups.remove(&key))
            .map(|mut versions| {
                // The group's own key is the preferred version (grouping.rs).
                versions.sort_by_key(|a| (a.edition.is_some(), a.edition.clone(), a.id));
                AlbumObject::new(versions)
            })
            .collect();

        crate::covers::clear();
        let store = self.albums();
        store.splice(0, store.n_items(), &objects);
        let empty = objects.is_empty();
        if self.imp().empty.replace(empty) != empty {
            self.notify_empty();
        }
        self.emit_by_name::<()>("reloaded", &[]);
    }

    fn folders(&self) -> Vec<PathBuf> {
        self.imp()
            .settings
            .get()
            .map(library_folders::load)
            .unwrap_or_default()
    }

    /// Starts an incremental scan, or queues one if a scan is running.
    pub fn rescan(&self) {
        let imp = self.imp();
        if imp.scanning.get() {
            imp.rescan_pending.set(true);
            return;
        }
        imp.scanning.set(true);
        self.notify_scanning();
        self.set_scan_status(&gettext("Looking for music…"));

        let folders = self.folders();
        let (tx, rx) = async_channel::unbounded();
        std::thread::spawn(move || {
            let result = Database::open(&config::database_path())
                .map_err(|e| e.to_string())
                .and_then(|mut db| {
                    let covers = CoverCache::new(config::cover_dir());
                    let progress = |p| {
                        let _ = tx.send_blocking(ScanMessage::Progress(p));
                    };
                    scanner::scan(&mut db, &folders, &covers, &progress).map_err(|e| e.to_string())
                });
            let _ = tx.send_blocking(ScanMessage::Done(result));
        });

        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = library)]
            self,
            async move {
                while let Ok(message) = rx.recv().await {
                    match message {
                        ScanMessage::Progress(ScanProgress::AlbumsReady) => library.reload(),
                        ScanMessage::Progress(progress) => library.show_progress(progress),
                        ScanMessage::Done(result) => {
                            library.scan_done(result);
                            break;
                        }
                    }
                }
            }
        ));
    }

    fn show_progress(&self, progress: ScanProgress) {
        let status = match progress {
            // Translators: {done} and {total} are numbers of files.
            ScanProgress::Reading { done, total } => gettext("Reading songs… {done} of {total}")
                .replace("{done}", &done.to_string())
                .replace("{total}", &total.to_string()),
            // Translators: {done} and {total} are numbers of covers.
            ScanProgress::Covers { done, total } => gettext("Preparing covers… {done} of {total}")
                .replace("{done}", &done.to_string())
                .replace("{total}", &total.to_string()),
            ScanProgress::AlbumsReady => return,
        };
        self.set_scan_status(&status);
    }

    fn set_scan_status(&self, status: &str) {
        self.imp().scan_status.replace(status.to_owned());
        self.notify_scan_status();
    }

    fn scan_done(&self, result: Result<ScanSummary, String>) {
        let imp = self.imp();
        match result {
            Ok(summary) => {
                tracing::info!("Scan finished: {summary:?}");
                if summary.failed > 0 {
                    // Translators: {} is a number of files.
                    let message = gettext("{} files couldn't be read")
                        .replace("{}", &summary.failed.to_string());
                    self.emit_by_name::<()>("message", &[&message]);
                }
            }
            Err(err) => {
                tracing::warn!("Scan failed: {err}");
                self.emit_by_name::<()>("message", &[&gettext("The library couldn't be scanned")]);
            }
        }
        self.reload();
        imp.scanning.set(false);
        self.notify_scanning();
        if imp.rescan_pending.take() {
            self.rescan();
        }
    }

    fn watch_folders(&self) {
        let imp = self.imp();
        imp.watcher.replace(None);
        let (tx, rx) = async_channel::unbounded::<()>();
        let watcher = LibraryWatcher::new(&self.folders(), move || {
            let _ = tx.try_send(());
        });
        match watcher {
            Ok(watcher) => {
                imp.watcher.replace(Some(imp::WatcherHandle(watcher)));
            }
            Err(err) => {
                tracing::warn!("Cannot watch the library folders: {err}");
                return;
            }
        }
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = library)]
            self,
            async move {
                while rx.recv().await.is_ok() {
                    library.schedule_rescan();
                }
            }
        ));
    }

    fn schedule_rescan(&self) {
        let imp = self.imp();
        if let Some(source) = imp.watch_timeout.take() {
            source.remove();
        }
        let source = glib::timeout_add_local_once(
            WATCH_DEBOUNCE,
            glib::clone!(
                #[weak(rename_to = library)]
                self,
                move || {
                    library.imp().watch_timeout.replace(None);
                    library.rescan();
                }
            ),
        );
        imp.watch_timeout.replace(Some(source));
    }
}

impl Default for Library {
    fn default() -> Self {
        Self::new()
    }
}
