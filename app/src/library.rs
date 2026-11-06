//! The library as the UI sees it: albums in a list model, scans on a
//! background thread, and folder watching (CONCEPT.md §3.1).

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use adw::prelude::*;
use gatefold_core::db::{Album, Database, Track};
use gatefold_core::library::artists::AlbumRole;
use gatefold_core::library::artists::Aliases;
use gatefold_core::library::covers::CoverCache;
use gatefold_core::library::scanner::{self, ScanProgress, ScanSummary};
use gatefold_core::library::watch::LibraryWatcher;
use gatefold_metadata::{Candidate, PortraitCandidate, Providers};
use gettextrs::gettext;
use gtk::glib::subclass::Signal;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};

use crate::config::{self, APP_ID};
use crate::library_folders;
use crate::metadata::{self, ArtistChanged, Job, MetadataService};
use crate::objects::{AlbumObject, ArtistObject, TrackObject};

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
        pub artists: OnceCell<gio::ListStore>,
        pub metadata: OnceCell<Metadata>,
        pub lookups: RefCell<HashSet<i64>>,
        pub db: OnceCell<RefCell<Database>>,
        pub settings: OnceCell<gio::Settings>,
        pub rescan_pending: Cell<bool>,
        pub watcher: RefCell<Option<WatcherHandle>>,
        pub watch_timeout: RefCell<Option<glib::SourceId>>,
    }

    pub struct WatcherHandle(#[allow(dead_code)] pub LibraryWatcher);

    pub struct Metadata(pub MetadataService);

    impl std::fmt::Debug for Metadata {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("MetadataService")
        }
    }

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
                    // An artist's portrait, links or bio changed.
                    Signal::builder("artist-changed")
                        .param_types([i64::static_type()])
                        .build(),
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
            self.artists
                .set(gio::ListStore::new::<ArtistObject>())
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
        settings.connect_changed(
            Some("online-metadata"),
            glib::clone!(
                #[weak(rename_to = library)]
                self,
                move |_, _| {
                    // Fresh objects rebind the visible tiles, which then
                    // request their (now online) lookups.
                    library.imp().lookups.borrow_mut().clear();
                    library.reload_artists();
                    // -1: every artist (open pages show or hide links).
                    library.emit_by_name::<()>("artist-changed", &[&-1i64]);
                }
            ),
        );
        imp.settings.set(settings).expect("set once");

        let (updates, changes) = async_channel::unbounded::<ArtistChanged>();
        let service = MetadataService::start(updates);
        let _ = imp.metadata.set(imp::Metadata(service));
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = library)]
            self,
            async move {
                while let Ok(ArtistChanged(id)) = changes.recv().await {
                    library.artist_changed(id);
                }
            }
        ));

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
        self.reload_artists();
        let store = self.albums();
        store.splice(0, store.n_items(), &objects);
        let empty = objects.is_empty();
        if self.imp().empty.replace(empty) != empty {
            self.notify_empty();
        }
        self.emit_by_name::<()>("reloaded", &[]);
    }

    // Artists (CONCEPT.md §3.3, §4.2, §8) -----------------------------------

    /// Album artists, by sort name.
    pub fn artists(&self) -> &gio::ListStore {
        self.imp().artists.get().expect("set in constructed()")
    }

    fn reload_artists(&self) {
        let artists = match self.db().map(|db| db.artists()) {
            Some(Ok(artists)) => artists,
            Some(Err(err)) => {
                tracing::warn!("Cannot load artists: {err}");
                return;
            }
            None => return,
        };
        let objects: Vec<ArtistObject> = artists.into_iter().map(ArtistObject::new).collect();
        let store = self.artists();
        store.splice(0, store.n_items(), &objects);
    }

    /// A fresh copy of an artist from the database.
    pub fn artist(&self, id: i64) -> Option<ArtistObject> {
        let artist = self.db()?.artist(id).ok()??;
        Some(ArtistObject::new(artist))
    }

    pub fn artist_albums(&self, artist_id: i64) -> Vec<(Album, AlbumRole)> {
        self.db()
            .and_then(|db| db.artist_albums(artist_id).ok())
            .unwrap_or_default()
    }

    pub fn appears_on(&self, artist_id: i64) -> Vec<TrackObject> {
        self.db()
            .and_then(|db| db.artist_appears_on(artist_id).ok())
            .unwrap_or_default()
            .into_iter()
            .map(TrackObject::new)
            .collect()
    }

    /// The album artists of an album: (artist id, name).
    pub fn album_artists(&self, album_id: i64) -> Vec<(i64, String)> {
        self.db()
            .and_then(|db| db.album_artists(album_id).ok())
            .unwrap_or_default()
    }

    /// The album group containing the album with this ID.
    pub fn album_group(&self, album_id: i64) -> Option<AlbumObject> {
        let store = self.albums();
        (0..store.n_items())
            .filter_map(|i| store.item(i).and_downcast::<AlbumObject>())
            .find(|a| a.versions().iter().any(|v| v.id == album_id))
    }

    fn settings(&self) -> &gio::Settings {
        self.imp().settings.get().expect("set in start()")
    }

    pub fn online_enabled(&self) -> bool {
        self.settings().boolean("online-metadata")
    }

    fn providers(&self) -> Providers {
        let settings = self.settings();
        let key = settings.string("fanart-api-key").trim().to_owned();
        Providers {
            musicbrainz: true,
            wikidata: settings.boolean("metadata-wikidata"),
            fanart_key: (settings.boolean("metadata-fanart") && !key.is_empty()).then_some(key),
        }
    }

    fn send_job(&self, job: Job) {
        if let Some(metadata) = self.imp().metadata.get() {
            metadata.0.send(job);
        }
    }

    /// Looks the artist up (lazily, when shown) unless that was done recently.
    pub fn request_lookup(&self, artist: &ArtistObject) {
        let meta = &artist.artist().meta;
        let online = self.online_enabled();
        let age = metadata::now() - meta.looked_up.unwrap_or(0);
        let due = match meta.lookup_state.as_deref() {
            None => true,
            Some("offline") => online,
            Some("done") => age > metadata::REFRESH_DONE,
            Some(_) => online && age > metadata::REFRESH_FAILED,
        };
        if due && self.imp().lookups.borrow_mut().insert(artist.id()) {
            self.send_job(Job::Lookup {
                artist_id: artist.id(),
                online,
                providers: self.providers(),
                language: metadata::language(),
            });
        }
    }

    /// Looks the artist up again now, optionally as a chosen MusicBrainz artist.
    pub fn lookup_again(&self, artist_id: i64, mbid: Option<&str>) {
        if let Some(mut db) = self.db()
            && let Err(err) = db.reset_artist_lookup(artist_id, mbid)
        {
            tracing::warn!("Cannot reset the lookup of artist {artist_id}: {err}");
        }
        self.imp().lookups.borrow_mut().insert(artist_id);
        self.send_job(Job::Lookup {
            artist_id,
            online: self.online_enabled(),
            providers: self.providers(),
            language: metadata::language(),
        });
    }

    pub async fn search_artists(&self, name: &str) -> Result<Vec<Candidate>, String> {
        let (reply, answer) = async_channel::bounded(1);
        self.send_job(Job::Search {
            name: name.to_owned(),
            reply,
        });
        answer.recv().await.map_err(|e| e.to_string())?
    }

    /// Portrait candidates from every provider: (portrait key, candidate).
    pub async fn portrait_candidates(
        &self,
        artist_id: i64,
    ) -> Result<Vec<(String, PortraitCandidate)>, String> {
        let (reply, answer) = async_channel::bounded(1);
        self.send_job(Job::Candidates {
            artist_id,
            providers: self.providers(),
            reply,
        });
        answer.recv().await.map_err(|e| e.to_string())?
    }

    /// Uses an image file's contents as the artist's portrait.
    pub async fn set_custom_portrait(&self, artist_id: i64, data: Vec<u8>) -> Result<(), String> {
        let (reply, answer) = async_channel::bounded(1);
        self.send_job(Job::SetCustom {
            artist_id,
            data,
            reply,
        });
        answer.recv().await.map_err(|e| e.to_string())?
    }

    /// Uses an already cached portrait (a candidate), or `None` for automatic.
    pub fn choose_portrait(&self, artist_id: i64, key: Option<&str>) {
        if let Some(mut db) = self.db()
            && let Err(err) = db.set_custom_portrait(artist_id, key)
        {
            tracing::warn!("Cannot save the portrait choice: {err}");
        }
        self.artist_changed(artist_id);
    }

    fn artist_changed(&self, id: i64) {
        self.imp().lookups.borrow_mut().remove(&id);
        if let Some(artist) = self.artist(id) {
            let store = self.artists();
            if let Some(position) = (0..store.n_items()).find(|&i| {
                store
                    .item(i)
                    .and_downcast::<ArtistObject>()
                    .is_some_and(|a| a.id() == id)
            }) {
                store.splice(position, 1, &[artist]);
            }
        }
        self.emit_by_name::<()>("artist-changed", &[&id]);
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
                    let aliases = Aliases::from_lyrembed();
                    let progress = |p| {
                        let _ = tx.send_blocking(ScanMessage::Progress(p));
                    };
                    scanner::scan(&mut db, &folders, &aliases, &covers, &progress)
                        .map_err(|e| e.to_string())
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
