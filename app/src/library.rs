//! The library as the UI sees it: albums in a list model, scans on a
//! background thread, and folder watching (CONCEPT.md §3.1).

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use adw::prelude::*;
use gettextrs::gettext;
use gtk::glib::subclass::Signal;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};
use jewelcase_core::db::LinkedPlaylist;
use jewelcase_core::db::{Album, Database, PlaylistItem, SearchResults, Track};
use jewelcase_core::library::artists::AlbumRole;
use jewelcase_core::library::artists::Aliases;
use jewelcase_core::library::scanner::{self, ScanProgress, ScanSummary};
use jewelcase_core::library::watch::LibraryWatcher;
use jewelcase_core::playlist_files::{self, Entry as PlaylistFileEntry, Format};
use jewelcase_metadata::{Candidate, PortraitCandidate, Providers};

use crate::application::JewelcaseApplication;
use crate::config::{self, APP_ID};
use crate::library_folders;
use crate::metadata::{self, ArtistChanged, Job, MetadataService};
use crate::objects::{AlbumObject, ArtistObject, PlaylistObject, TrackObject};

/// Changes are collected this long before an incremental scan starts.
const WATCH_DEBOUNCE: Duration = Duration::from_secs(3);

/// The library's albums by artist and barcode, to find a Spotify album's
/// library copy quickly (§9.8).
#[derive(Debug, Default)]
pub struct AlbumIndex {
    entries: Vec<AlbumEntry>,
    by_artist: HashMap<String, Vec<usize>>,
    by_barcode: HashMap<String, Vec<usize>>,
}

#[derive(Debug)]
struct AlbumEntry {
    group: AlbumObject,
    title: String,
    artist: String,
    year: Option<i32>,
    tracks: u32,
    barcode: Option<String>,
}

enum ScanMessage {
    /// lyrembed's aliases, before scanning: the library read again during
    /// the scan matches with them.
    Aliases(Aliases),
    Progress(ScanProgress),
    Done(Result<ScanSummary, String>),
}

/// What the grids show, read from the database (on any thread).
struct LoadedLibrary {
    albums: Vec<jewelcase_core::db::Album>,
    genres: HashMap<i64, Vec<String>>,
    barcodes: HashMap<i64, String>,
    artists: Vec<jewelcase_core::db::Artist>,
    index: jewelcase_core::matching::MatchIndex,
}

/// Once: custom playlist covers from the cache folder, where versions up
/// to 0.5.1 kept them, into the data folder (`config::cover_cache`).
fn move_chosen_covers(db: &mut Database) {
    const KEY: &str = "chosen-covers-moved";
    if db.app_state(KEY).ok().flatten().is_some() {
        return;
    }
    match config::cover_cache().move_chosen_from_cache() {
        Ok(moved) => {
            if moved > 0 {
                tracing::info!("Moved {moved} custom playlist covers out of the cache");
            }
            let _ = db.set_app_state(KEY, "1");
        }
        // Tried again next start.
        Err(err) => tracing::warn!("Cannot move custom playlist covers: {err}"),
    }
}

/// Reads the library and matches the Spotify playlist entries to it again
/// (§9.5), in one place for the main thread and a worker.
fn load_library(db: &mut Database, aliases: &Aliases) -> jewelcase_core::db::Result<LoadedLibrary> {
    let index = jewelcase_core::matching::MatchIndex::new(db.match_tracks()?, aliases);
    if !db.linked_playlists()?.is_empty() {
        match db.rematch_spotify_items(&index) {
            Ok(0) => {}
            Ok(changed) => tracing::info!("{changed} Spotify playlist entries matched again"),
            Err(err) => tracing::warn!("Cannot match Spotify entries: {err}"),
        }
    }
    Ok(LoadedLibrary {
        albums: db.albums()?,
        genres: db.album_genres()?,
        barcodes: db.album_barcodes()?,
        artists: db.artists()?,
        index,
    })
}

/// How often the library folders are checked without a watcher event.
const PERIODIC_SCAN_S: u32 = 30 * 60;

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
        pub playlists: OnceCell<gio::ListStore>,
        pub metadata: OnceCell<Metadata>,
        /// ISRCs are being looked up on MusicBrainz.
        pub isrc_lookup: Cell<bool>,
        /// Counts reloads; an older background reload is dropped.
        pub reload_generation: Cell<u64>,
        /// Related artists are being asked for on ListenBrainz.
        pub related_lookup: Cell<bool>,
        /// ListenBrainz asked to wait until then.
        pub related_paused: Cell<Option<std::time::Instant>>,
        pub lookups: RefCell<HashSet<i64>>,
        /// Each album's genres, most common first (filter, album panel).
        pub album_genres: RefCell<HashMap<i64, Vec<String>>>,
        /// The next scan regroups even if nothing changed ("Rescan Library").
        pub force_rescan: Cell<bool>,
        /// See `album_index()`; dropped when the library is read again.
        pub album_index: RefCell<Option<std::rc::Rc<super::AlbumIndex>>>,
        /// See `match_index()`; dropped when the library is read again.
        pub match_index: RefCell<Option<std::rc::Rc<jewelcase_core::matching::MatchIndex>>>,
        /// Barcodes by album, to recognize albums on Spotify (§9.8).
        pub album_barcodes: RefCell<HashMap<i64, String>>,
        /// lyrembed's alias groups, from the last scan (search, artist pages).
        pub aliases: RefCell<Aliases>,
        pub db: OnceCell<RefCell<Database>>,
        pub settings: OnceCell<gio::Settings>,
        pub rescan_pending: Cell<bool>,
        pub watcher: RefCell<Option<WatcherHandle>>,
        /// Drives coming and going (a library folder on a USB drive or a
        /// network share mounted later).
        pub volumes: std::cell::OnceCell<gio::VolumeMonitor>,
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
        const NAME: &'static str = "JewelcaseLibrary";
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
                    // A playlist was created, renamed, changed or deleted.
                    Signal::builder("playlists-changed").build(),
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
            self.playlists
                .set(gio::ListStore::new::<PlaylistObject>())
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
        let mut db = Database::open(&config::database_path()).map_err(|e| e.to_string())?;
        move_chosen_covers(&mut db);
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
        self.watch_mounts();
        self.rescan();
        // Now and then without a hint from inotify: changes another
        // machine made on a network share aren't reported. A scan that
        // finds nothing changed only compares file times.
        glib::timeout_add_seconds_local(
            PERIODIC_SCAN_S,
            glib::clone!(
                #[weak(rename_to = library)]
                self,
                #[upgrade_or]
                glib::ControlFlow::Break,
                move || {
                    library.schedule_rescan();
                    glib::ControlFlow::Continue
                }
            ),
        );
        Ok(())
    }

    /// A drive mounted or unmounted: its folders are watched (inotify can't
    /// watch what wasn't there), and a rescan picks up its songs.
    fn watch_mounts(&self) {
        let monitor = gio::VolumeMonitor::get();
        let changed = glib::clone!(
            #[weak(rename_to = library)]
            self,
            move || {
                library.watch_folders();
                library.schedule_rescan();
            }
        );
        let on_added = changed.clone();
        monitor.connect_mount_added(move |_, _| on_added());
        monitor.connect_mount_removed(move |_, _| changed());
        let _ = self.imp().volumes.set(monitor);
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
    /// Reads the library and shows it (on start, where nothing else runs).
    fn reload(&self) {
        let aliases = self.aliases().clone();
        let loaded = match self.db() {
            Some(mut db) => load_library(&mut db, &aliases),
            None => return,
        };
        match loaded {
            Ok(loaded) => self.show_library(loaded),
            Err(err) => tracing::warn!("Cannot load the library: {err}"),
        }
    }

    /// After a scan: the library is read on a worker thread with its own
    /// connection (the window doesn't wait for the queries, nor for
    /// matching Spotify entries again), then shown. A newer reload wins.
    fn reload_in_background(&self) {
        let imp = self.imp();
        let generation = imp.reload_generation.get() + 1;
        imp.reload_generation.set(generation);
        let aliases = self.aliases().clone();
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = library)]
            self,
            async move {
                let loaded = gio::spawn_blocking(move || {
                    let mut db =
                        Database::open(&config::database_path()).map_err(|e| e.to_string())?;
                    load_library(&mut db, &aliases).map_err(|e| e.to_string())
                })
                .await
                .unwrap_or_else(|_| Err("the library job failed".into()));
                if library.imp().reload_generation.get() != generation {
                    return;
                }
                match loaded {
                    Ok(loaded) => library.show_library(loaded),
                    Err(err) => tracing::warn!("Cannot load the library: {err}"),
                }
            }
        ));
    }

    /// Puts what `load_library` read into the grids (main thread).
    fn show_library(&self, loaded: LoadedLibrary) {
        let LoadedLibrary {
            albums,
            genres,
            barcodes,
            artists,
            index,
        } = loaded;
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
        self.imp().album_genres.replace(genres);
        self.imp().album_barcodes.replace(barcodes);

        crate::covers::clear();
        // Songs or aliases changed: matched afresh (done in load_library).
        self.imp()
            .match_index
            .replace(Some(std::rc::Rc::new(index)));
        self.imp().album_index.replace(None);
        let artists: Vec<ArtistObject> = artists.into_iter().map(ArtistObject::new).collect();
        let store = self.artists();
        store.splice(0, store.n_items(), &artists);
        self.reload_playlists();
        let store = self.albums();
        store.splice(0, store.n_items(), &objects);
        let empty = objects.is_empty();
        if self.imp().empty.replace(empty) != empty {
            self.notify_empty();
        }
        self.emit_by_name::<()>("reloaded", &[]);
    }

    // Artists (CONCEPT.md §3.3, §4.2, §8) -----------------------------------

    /// An album group's genres (all its versions), most common first.
    pub fn genres_of(&self, album: &AlbumObject) -> Vec<String> {
        let all = self.imp().album_genres.borrow();
        let mut genres: Vec<String> = Vec::new();
        for version in album.versions() {
            for genre in all.get(&version.id).into_iter().flatten() {
                if !genres.iter().any(|g| g.eq_ignore_ascii_case(genre)) {
                    genres.push(genre.clone());
                }
            }
        }
        genres
    }

    /// All genres in the library, by how many albums have them.
    pub fn all_genres(&self) -> Vec<String> {
        let all = self.imp().album_genres.borrow();
        let mut counts: Vec<(String, u32)> = Vec::new();
        for genre in all.values().flatten() {
            match counts
                .iter_mut()
                .find(|(g, _)| g.eq_ignore_ascii_case(genre))
            {
                Some((_, count)) => *count += 1,
                None => counts.push((genre.clone(), 1)),
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        counts.into_iter().map(|(g, _)| g).collect()
    }

    /// The library indexed for matching Spotify songs (§9.5): built once
    /// per library load, not for every search, album or remote song. Get
    /// it before borrowing the database (building it reads the songs).
    pub fn match_index(&self) -> std::rc::Rc<jewelcase_core::matching::MatchIndex> {
        if let Some(index) = self.imp().match_index.borrow().as_ref() {
            return index.clone();
        }
        let tracks = self
            .db()
            .and_then(|db| db.match_tracks().ok())
            .unwrap_or_default();
        let index = std::rc::Rc::new(jewelcase_core::matching::MatchIndex::new(
            tracks,
            &self.aliases(),
        ));
        self.imp().match_index.replace(Some(index.clone()));
        index
    }

    /// lyrembed's alias groups (empty without lyrembed).
    pub fn aliases(&self) -> std::cell::Ref<'_, Aliases> {
        self.imp().aliases.borrow()
    }

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

    /// The library's album artist for a Spotify artist: by Spotify ID,
    /// else by name (lyrembed's aliases count).
    pub fn artist_for_spotify(&self, spotify_id: &str, name: &str) -> Option<i64> {
        let aliases = self.aliases();
        let wanted = aliases.identity(name);
        let store = self.artists();
        let mut by_name = None;
        for i in 0..store.n_items() {
            let Some(artist) = store.item(i).and_downcast::<ArtistObject>() else {
                continue;
            };
            let artist = artist.artist();
            if artist.meta.spotify_id.as_deref() == Some(spotify_id) {
                return Some(artist.id);
            }
            if by_name.is_none() && aliases.identity(&artist.name) == wanted {
                by_name = Some(artist.id);
            }
        }
        by_name
    }

    /// The album artists of an album: (artist id, name).
    pub fn album_artists(&self, album_id: i64) -> Vec<(i64, String)> {
        self.db()
            .and_then(|db| db.album_artists(album_id).ok())
            .unwrap_or_default()
    }

    /// The library's copy of an album on Spotify, if it has one (§9.8).
    pub fn local_album_for(&self, album: &jewelcase_core::db::SpotifyAlbum) -> Option<AlbumObject> {
        use jewelcase_core::matching::{AlbumFacts, artist_keys, barcode_key, same_album};
        let remote_artists: Vec<String> = album.artist.split(", ").map(str::to_owned).collect();
        let remote = AlbumFacts {
            title: &album.name,
            artists: &remote_artists,
            year: album.year,
            tracks: album.total_tracks,
            barcode: album.upc.as_deref(),
        };
        let index = self.album_index();
        let aliases = self.aliases().clone();
        // Only albums by the same artist (or with the same barcode) are
        // compared, not all of them for every Spotify album.
        let mut candidates: Vec<usize> = artist_keys(&remote_artists, &aliases)
            .iter()
            .filter_map(|key| index.by_artist.get(key))
            .flatten()
            .copied()
            .collect();
        if let Some(code) = album.upc.as_ref().filter(|c| !barcode_key(c).is_empty()) {
            candidates.extend(
                index
                    .by_barcode
                    .get(&barcode_key(code))
                    .into_iter()
                    .flatten(),
            );
        }
        candidates.sort_unstable();
        candidates.dedup();
        candidates.into_iter().find_map(|i| {
            let entry = &index.entries[i];
            let local = AlbumFacts {
                title: &entry.title,
                artists: std::slice::from_ref(&entry.artist),
                year: entry.year,
                tracks: entry.tracks,
                barcode: entry.barcode.as_deref(),
            };
            same_album(&local, &remote, &aliases).then(|| entry.group.clone())
        })
    }

    /// The library's albums indexed for `local_album_for` (per reload).
    fn album_index(&self) -> std::rc::Rc<AlbumIndex> {
        use jewelcase_core::matching::{artist_keys, barcode_key};
        if let Some(index) = self.imp().album_index.borrow().as_ref() {
            return index.clone();
        }
        let aliases = self.aliases().clone();
        let barcodes = self.imp().album_barcodes.borrow().clone();
        let mut index = AlbumIndex::default();
        let store = self.albums();
        for group in
            (0..store.n_items()).filter_map(|i| store.item(i).and_downcast::<AlbumObject>())
        {
            for version in group.versions() {
                let position = index.entries.len();
                let barcode = barcodes.get(&version.id).cloned();
                for key in artist_keys(std::slice::from_ref(&version.artist_credit), &aliases) {
                    index.by_artist.entry(key).or_default().push(position);
                }
                if let Some(code) = barcode.as_ref().filter(|c| !barcode_key(c).is_empty()) {
                    index
                        .by_barcode
                        .entry(barcode_key(code))
                        .or_default()
                        .push(position);
                }
                index.entries.push(AlbumEntry {
                    group: group.clone(),
                    title: version.title.clone(),
                    artist: version.artist_credit.clone(),
                    year: version.year,
                    tracks: version.track_count,
                    barcode,
                });
            }
        }
        let index = std::rc::Rc::new(index);
        self.imp().album_index.replace(Some(index.clone()));
        index
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
        Providers {
            musicbrainz: true,
            wikidata: settings.boolean("metadata-wikidata"),
            deezer: settings.boolean("metadata-deezer"),
        }
    }

    /// The Spotify account for portraits, when connected and allowed.
    fn portrait_spotify(&self) -> metadata::SpotifyClient {
        if !self.settings().boolean("metadata-spotify") {
            return None;
        }
        let spotify = JewelcaseApplication::get().spotify();
        spotify.is_connected().then(|| spotify.client()).flatten()
    }

    fn send_job(&self, job: Job) {
        if let Some(metadata) = self.imp().metadata.get() {
            metadata.0.send(job);
        }
    }

    /// ISRCs from MusicBrainz for albums whose songs have none, so Spotify
    /// entries match them surely (§9.5, step 2). With online lookups on;
    /// one request per album, asked again after half a year. Spotify
    /// entries are matched again afterwards.
    pub fn lookup_isrcs(&self) {
        const AGAIN: i64 = 180 * 24 * 3600;
        if !self.online_enabled() || self.imp().isrc_lookup.get() {
            return;
        }
        let releases = self
            .db()
            .and_then(|db| db.releases_without_isrcs(metadata::now() - AGAIN).ok())
            .unwrap_or_default();
        if releases.is_empty() {
            return;
        }
        self.imp().isrc_lookup.set(true);
        tracing::info!(
            "Looking up ISRCs of {} albums on MusicBrainz",
            releases.len()
        );
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = library)]
            self,
            async move {
                let mut found = 0;
                // One at a time: the worker's other jobs (an open artist
                // page) go in between.
                for release in releases {
                    let (reply, answer) = async_channel::bounded(1);
                    library.send_job(Job::ReleaseIsrcs { release, reply });
                    match answer.recv().await {
                        Ok(Ok(count)) => found += count,
                        Ok(Err(err)) => {
                            // Offline or MusicBrainz busy: the rest waits
                            // for the next sync.
                            tracing::info!("Cannot look up ISRCs: {err}");
                            break;
                        }
                        Err(_) => break,
                    }
                }
                library.imp().isrc_lookup.set(false);
                tracing::info!("MusicBrainz gave {found} ISRCs");
                if found > 0 {
                    library.imp().match_index.replace(None);
                    library.rematch_spotify();
                    library.reload_playlists();
                }
            }
        ));
    }

    /// Related artists from ListenBrainz for the library's artists (by
    /// MusicBrainz ID), for smart shuffle's "same artist" (§7). With
    /// online lookups and that criterion on; one artist at a time (one
    /// request a second at most), each again after 90 days; after
    /// ListenBrainz asks to wait, an hour at least. Smart shuffle uses
    /// them once they're in.
    pub fn lookup_related_artists(&self) {
        const AGAIN: i64 = 90 * 24 * 3600;
        const BUSY_PAUSE: std::time::Duration = std::time::Duration::from_secs(3600);
        let imp = self.imp();
        if !self.online_enabled()
            || !self.settings().boolean("mix-artist")
            || imp.related_lookup.get()
            || imp
                .related_paused
                .get()
                .is_some_and(|until| std::time::Instant::now() < until)
        {
            return;
        }
        let artists = self
            .db()
            .and_then(|db| db.artists_without_related(metadata::now() - AGAIN).ok())
            .unwrap_or_default();
        if artists.is_empty() {
            return;
        }
        imp.related_lookup.set(true);
        tracing::info!(
            "Asking ListenBrainz for artists related to {}",
            artists.len()
        );
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = library)]
            self,
            async move {
                let mut found = 0;
                for mbid in artists {
                    let (reply, answer) = async_channel::bounded(1);
                    library.send_job(Job::RelatedArtists { mbid, reply });
                    match answer.recv().await {
                        Ok(Ok(count)) => found += count,
                        Ok(Err(metadata::RelatedError::Busy(wait))) => {
                            tracing::info!("ListenBrainz asks to wait; not asking for a while");
                            library
                                .imp()
                                .related_paused
                                .set(Some(std::time::Instant::now() + wait.max(BUSY_PAUSE)));
                            break;
                        }
                        Ok(Err(metadata::RelatedError::Other(err))) => {
                            tracing::info!("Cannot ask ListenBrainz: {err}");
                            break;
                        }
                        Err(_) => break,
                    }
                }
                library.imp().related_lookup.set(false);
                tracing::info!("ListenBrainz gave {found} related artists");
                if found > 0 {
                    JewelcaseApplication::get().player().similarity_changed();
                }
            }
        ));
    }

    /// Looks the artist up (lazily, when shown) unless that was done recently.
    pub fn request_lookup(&self, artist: &ArtistObject) {
        self.request_lookup_with(artist, false);
    }

    /// The artist whose page is open: before the lookups tiles queued.
    pub fn request_lookup_now(&self, artist: &ArtistObject) {
        self.request_lookup_with(artist, true);
    }

    fn request_lookup_with(&self, artist: &ArtistObject, urgent: bool) {
        let meta = &artist.artist().meta;
        let online = self.online_enabled();
        let age = metadata::now() - meta.looked_up.unwrap_or(0);
        let due = match meta.lookup_state.as_deref() {
            None => true,
            Some("offline") => online,
            // A source couldn't answer ("spotify-later": an old name of
            // the same state).
            Some("partial" | "spotify-later") => online && age > metadata::PARTIAL_AGAIN,
            // (Lookups from before Spotify and Deezer had portraits were
            // made due once by migration 019.)
            Some("done") => age > metadata::REFRESH_DONE,
            Some(_) => online && age > metadata::REFRESH_FAILED,
        };
        // Queued already (by its tile): an urgent request goes ahead of it.
        let new = self.imp().lookups.borrow_mut().insert(artist.id());
        if due && (new || urgent) {
            self.send_job(Job::Lookup {
                artist_id: artist.id(),
                online,
                providers: self.providers(),
                spotify: self.portrait_spotify(),
                language: metadata::language(),
                urgent,
                again: false,
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
            spotify: self.portrait_spotify(),
            language: metadata::language(),
            urgent: true,
            again: true,
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
            spotify: self.portrait_spotify(),
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

    // Playlists (CONCEPT.md §4.1, §9.5) -------------------------------------

    pub fn playlists(&self) -> &gio::ListStore {
        self.imp().playlists.get().expect("set in constructed()")
    }

    pub fn reload_playlists(&self) {
        let playlists = match self.db().map(|db| db.playlists()) {
            Some(Ok(playlists)) => playlists,
            Some(Err(err)) => {
                tracing::warn!("Cannot load playlists: {err}");
                return;
            }
            None => return,
        };
        let covers = config::cover_cache();
        let mut missing_mosaics = Vec::new();
        let objects: Vec<PlaylistObject> = playlists
            .into_iter()
            .map(|playlist| {
                // Spotify playlists show Spotify's cover (its own image, or
                // its mosaic of the whole playlist), not a mosaic of the
                // few songs that are in the library.
                let spotify = (playlist.source == "spotify")
                    .then(|| playlist.spotify_cover_key.clone())
                    .flatten();
                let cover = playlist.custom_cover_key.clone().or(spotify).or_else(|| {
                    match playlist.cover_keys.len() {
                        // Not loaded yet: Spotify's, if there is one.
                        0 => playlist.spotify_cover_key.clone(),
                        1 => playlist.cover_keys.first().cloned(),
                        _ => {
                            let mosaic = covers.mosaic_key(&playlist.cover_keys);
                            if covers.has(&mosaic) {
                                Some(mosaic)
                            } else {
                                missing_mosaics.push(playlist.cover_keys.clone());
                                playlist.cover_keys.first().cloned()
                            }
                        }
                    }
                });
                PlaylistObject::new(playlist, cover)
            })
            .collect();
        let store = self.playlists();
        store.splice(0, store.n_items(), &objects);
        self.emit_by_name::<()>("playlists-changed", &[]);

        if !missing_mosaics.is_empty() {
            glib::spawn_future_local(glib::clone!(
                #[weak(rename_to = library)]
                self,
                async move {
                    let made = gio::spawn_blocking(move || {
                        missing_mosaics
                            .iter()
                            .filter(|keys| covers.mosaic(keys).is_ok())
                            .count()
                    })
                    .await
                    .unwrap_or(0);
                    if made > 0 {
                        library.reload_playlists();
                    }
                }
            ));
        }
    }

    /// New or removed songs: Spotify entries are matched again (§9.5).
    fn rematch_spotify(&self) {
        let linked = self
            .db()
            .and_then(|db| db.linked_playlists().ok())
            .is_some_and(|l| !l.is_empty());
        if !linked {
            return;
        }
        let index = self.match_index();
        let Some(mut db) = self.db() else { return };
        match db.rematch_spotify_items(&index) {
            Ok(0) => {}
            Ok(changed) => tracing::info!("{changed} Spotify playlist entries matched again"),
            Err(err) => tracing::warn!("Cannot match Spotify entries: {err}"),
        }
    }

    pub fn playlist(&self, id: i64) -> Option<PlaylistObject> {
        let store = self.playlists();
        (0..store.n_items())
            .filter_map(|i| store.item(i).and_downcast::<PlaylistObject>())
            .find(|p| p.id() == id)
    }

    pub fn playlist_items(&self, id: i64) -> Vec<PlaylistItem> {
        self.db()
            .and_then(|db| db.playlist_items(id).ok())
            .unwrap_or_default()
    }

    fn playlist_op(&self, op: impl FnOnce(&mut Database, i64) -> jewelcase_core::db::Result<()>) {
        self.playlist_op_quiet(op);
        self.reload_playlists();
    }

    /// A playlist change without reloading the playlists: for edits that
    /// also queue Spotify changes and reload once afterwards (with "1
    /// change waiting to be sent" in it).
    fn playlist_op_quiet(
        &self,
        op: impl FnOnce(&mut Database, i64) -> jewelcase_core::db::Result<()>,
    ) {
        let result = match self.db() {
            Some(mut db) => op(&mut db, metadata::now()),
            None => return,
        };
        if let Err(err) = result {
            tracing::warn!("Playlist change failed: {err}");
            self.emit_by_name::<()>("message", &[&gettext("The playlist couldn't be changed")]);
        }
    }

    /// Creates a playlist, optionally with tracks; returns its ID.
    pub fn create_playlist(&self, name: &str, tracks: &[i64]) -> Option<i64> {
        let mut created = None;
        self.playlist_op(|db, now| {
            let id = db.create_playlist(name, now)?;
            created = Some(id);
            // Library songs, and Spotify songs (negative IDs, §9.4).
            let (local, streams): (Vec<i64>, Vec<i64>) = tracks.iter().partition(|t| **t > 0);
            if !local.is_empty() {
                db.add_to_playlist(id, &local, None, now)?;
            }
            if !streams.is_empty() {
                db.add_streams_to_playlist(id, &streams, now)?;
            }
            Ok(())
        });
        created
    }

    /// The playlists that have this song, local or Spotify ones, as itself
    /// or as its Spotify version.
    pub fn playlists_containing(&self, track: &TrackObject) -> Vec<i64> {
        let device_uri = JewelcaseApplication::get()
            .spotify()
            .device_song_uri(track.id());
        // A device's stand-in ID means nothing to the database.
        let id = if device_uri.is_some() { 0 } else { track.id() };
        self.db()
            .and_then(|db| db.playlists_containing(id, device_uri.as_deref()).ok())
            .unwrap_or_default()
    }

    /// The playlist's Spotify link, when edits go to Spotify too (§9.6).
    fn spotify_link(&self, id: i64) -> Option<LinkedPlaylist> {
        self.db()?.linked_playlist(id).ok().flatten()
    }

    /// Others' Spotify playlists can't be changed; says so.
    fn refuse_read_only(&self, id: i64) -> bool {
        let read_only = self.spotify_link(id).is_some_and(|p| !p.editable);
        if read_only {
            self.emit_by_name::<()>(
                "message",
                &[&gettext("Only its owner can change this Spotify playlist")],
            );
        }
        read_only
    }

    /// While a playlist's changes are on their way to Spotify, it waits
    /// for Spotify's answer before taking new ones; says so.
    fn refuse_while_sending(&self, id: i64) -> bool {
        let sending = JewelcaseApplication::get().spotify().is_sending(id);
        if sending {
            let name = self
                .playlist(id)
                .map(|p| p.playlist().name.clone())
                .unwrap_or_default();
            self.emit_by_name::<()>(
                "message",
                // Translators: {} is a playlist name.
                &[
                    &gettext("“{}” is being sent to Spotify; try again in a moment")
                        .replace("{}", &name),
                ],
            );
        }
        sending
    }

    /// An addition at `position` of a linked playlist, as it is now.
    fn spotify_add(&self, id: i64, uris: Vec<String>, position: u32) -> crate::spotify::Op {
        let current = self
            .db()
            .and_then(|db| db.spotify_uris(id).ok())
            .unwrap_or_default();
        crate::spotify::Op::add(uris, Some(position), &current)
    }

    fn queue_spotify_op(&self, id: i64, op: crate::spotify::Op) {
        if let Some(mut db) = self.db()
            && let Err(err) = db.queue_spotify_op(id, &op.to_json(), metadata::now())
        {
            tracing::warn!("Cannot queue a Spotify change: {err}");
        }
        // The caller reloads the playlists once ("1 change waiting").
        JewelcaseApplication::get().spotify().changes_queued();
    }

    /// `announce`: confirm with a toast (not needed where the list shows it).
    pub fn add_to_playlist(&self, id: i64, tracks: &[i64], announce: bool) {
        if self.refuse_read_only(id) || self.refuse_while_sending(id) {
            return;
        }
        // Library songs, and Spotify songs (negative IDs, §9.4).
        let (local, streams): (Vec<i64>, Vec<i64>) = tracks.iter().partition(|t| **t > 0);
        let mut added_streams = Vec::new();
        self.playlist_op_quiet(|db, now| {
            if !local.is_empty() {
                db.add_to_playlist(id, &local, None, now)?;
            }
            if !streams.is_empty() {
                added_streams = db.add_streams_to_playlist(id, &streams, now)?;
            }
            Ok(())
        });
        if self.spotify_link(id).is_some() {
            // Spotify songs go to Spotify right away…
            for item in added_streams {
                let found = self.db().and_then(|db| {
                    Some((db.item_uri(item).ok()??, db.remote_position(item).ok()??))
                });
                if let Some((uri, position)) = found {
                    let op = self.spotify_add(id, vec![uri], position);
                    self.queue_spotify_op(id, op);
                }
            }
            // …library songs once their Spotify versions are found.
            if !local.is_empty() {
                JewelcaseApplication::get().spotify().resolve_soon(id, true);
            }
        }
        self.reload_playlists();
        if !announce {
            return;
        }
        let name = self
            .playlist(id)
            .map(|p| p.playlist().name.clone())
            .unwrap_or_default();
        // Translators: {} is a playlist name.
        self.emit_by_name::<()>("message", &[&gettext("Added to “{}”").replace("{}", &name)]);
    }

    pub fn rename_playlist(&self, id: i64, name: &str) {
        if self.refuse_read_only(id) || self.refuse_while_sending(id) {
            self.reload_playlists();
            return;
        }
        self.playlist_op_quiet(|db, now| db.rename_playlist(id, name, now));
        if self.spotify_link(id).is_some() {
            self.queue_spotify_op(
                id,
                crate::spotify::Op::Rename {
                    name: name.to_owned(),
                },
            );
        }
        self.reload_playlists();
    }

    pub fn delete_playlist(&self, id: i64) {
        self.playlist_op(|db, _| db.delete_playlist(id));
    }

    pub fn remove_playlist_item(&self, id: i64, item: i64) {
        if self.refuse_read_only(id) || self.refuse_while_sending(id) {
            return;
        }
        let uri = self
            .spotify_link(id)
            .and_then(|_| self.db()?.item_uri(item).ok().flatten());
        self.playlist_op_quiet(|db, now| db.remove_playlist_item(id, item, now));
        if let Some(uri) = uri {
            // Spotify removes every occurrence; the others go back in.
            let restore = self
                .db()
                .and_then(|db| db.spotify_uris(id).ok())
                .unwrap_or_default()
                .into_iter()
                .enumerate()
                .filter(|(_, u)| *u == uri)
                .map(|(position, u)| (u, position as u32))
                .collect();
            self.queue_spotify_op(
                id,
                crate::spotify::Op::Remove {
                    uris: vec![uri],
                    restore,
                },
            );
        }
        self.reload_playlists();
    }

    /// Removes these entries ("Add to Playlist…" for songs the playlist
    /// has already) at once: Spotify gets one removal per song.
    pub fn remove_playlist_items(&self, id: i64, items: &[i64]) {
        if items.is_empty() || self.refuse_read_only(id) || self.refuse_while_sending(id) {
            return;
        }
        let uris: Vec<String> = match self.spotify_link(id) {
            Some(_) => {
                let mut uris: Vec<String> = self
                    .db()
                    .map(|db| {
                        items
                            .iter()
                            .filter_map(|item| db.item_uri(*item).ok().flatten())
                            .collect()
                    })
                    .unwrap_or_default();
                uris.sort_unstable();
                uris.dedup();
                uris
            }
            None => Vec::new(),
        };
        self.playlist_op_quiet(|db, now| {
            for item in items {
                db.remove_playlist_item(id, *item, now)?;
            }
            Ok(())
        });
        if !uris.is_empty() {
            // Spotify removes every occurrence; entries left go back in.
            let current = self
                .db()
                .and_then(|db| db.spotify_uris(id).ok())
                .unwrap_or_default();
            let restore = current
                .into_iter()
                .enumerate()
                .filter(|(_, u)| uris.contains(u))
                .map(|(position, u)| (u, position as u32))
                .collect();
            self.queue_spotify_op(id, crate::spotify::Op::Remove { uris, restore });
        }
        self.reload_playlists();
        let name = self
            .playlist(id)
            .map(|p| p.playlist().name.clone())
            .unwrap_or_default();
        // Translators: {} is a playlist name.
        self.emit_by_name::<()>(
            "message",
            &[&gettext("Removed from “{}”").replace("{}", &name)],
        );
    }

    pub fn move_playlist_item(&self, id: i64, from: u32, to: u32) {
        if self.refuse_read_only(id) || self.refuse_while_sending(id) {
            self.reload_playlists();
            return;
        }
        // Where Spotify has it, before and after.
        let moved = self.spotify_link(id).and_then(|_| {
            let db = self.db()?;
            let (item, uri) = db.item_at(id, from).ok().flatten()?;
            uri?;
            Some((item, db.remote_position(item).ok().flatten()?))
        });
        self.playlist_op_quiet(|db, now| db.move_playlist_item(id, from, to, now));
        if let Some((item, start)) = moved
            && let Some(end) = self
                .db()
                .and_then(|db| db.remote_position(item).ok().flatten())
            && start != end
        {
            let current = self
                .db()
                .and_then(|db| db.spotify_uris(id).ok())
                .unwrap_or_default();
            self.queue_spotify_op(id, crate::spotify::Op::moved(start, end, &current));
        }
        self.reload_playlists();
    }

    /// A Spotify entry plays this library song (or, `None`, whatever
    /// matching finds) (§9.5).
    pub fn choose_local_match(&self, item: i64, track: Option<i64>) {
        self.playlist_op(|db, _| db.choose_local_match(item, track));
    }

    /// "Always Play from Spotify".
    pub fn pin_spotify(&self, item: i64) {
        self.playlist_op(|db, _| db.pin_spotify(item));
    }

    /// The suggested Spotify track is right: the entry goes to Spotify.
    pub fn accept_spotify_suggestion(&self, id: i64, item: i64, uri: &str) {
        if self.refuse_read_only(id) || self.refuse_while_sending(id) {
            return;
        }
        let linked = self.spotify_link(id).is_some();
        let mut position = None;
        self.playlist_op_quiet(|db, _| {
            // Where it goes on Spotify first: if that can't be read,
            // nothing is changed (not the entry without its add).
            let at = db.remote_position(item)?;
            db.set_item_spotify(item, uri, true)?;
            position = at;
            Ok(())
        });
        if linked && let Some(position) = position {
            let op = self.spotify_add(id, vec![uri.to_owned()], position);
            self.queue_spotify_op(id, op);
        }
        self.reload_playlists();
    }

    /// Drops a suggestion; for Spotify suggestions, the song counts as
    /// not on Spotify from now on.
    pub fn dismiss_suggestion(&self, item: i64, track: Option<i64>, spotify: bool) {
        self.playlist_op(|db, now| {
            db.dismiss_suggestion(item)?;
            if spotify && let Some(track) = track {
                db.set_spotify_version(
                    track,
                    &jewelcase_core::db::SpotifyVersion {
                        uri: None,
                        confidence: None,
                        title: None,
                        artist: None,
                        checked: now,
                    },
                )?;
            }
            Ok(())
        });
    }

    pub fn reset_playlist_cover(&self, id: i64) {
        self.playlist_op(|db, now| db.set_playlist_cover(id, None, now));
    }

    /// Uses an image file as the playlist's cover (decoded by glycin).
    pub async fn set_playlist_cover(&self, id: i64, data: Vec<u8>) -> Result<(), String> {
        if self.refuse_read_only(id) || self.refuse_while_sending(id) {
            return Ok(());
        }
        let key = jewelcase_core::library::covers::cover_key('c', &data);
        let job_key = key.clone();
        gio::spawn_blocking(move || {
            let covers = config::cover_cache();
            if covers.has(&job_key) {
                Ok(())
            } else {
                covers.generate(&job_key, data).map_err(|e| e.to_string())
            }
        })
        .await
        .map_err(|_| "the cover job failed".to_owned())??;
        // Asked again: sending may have started while the image decoded.
        if self.refuse_while_sending(id) {
            return Ok(());
        }
        self.playlist_op_quiet(|db, now| db.set_playlist_cover(id, Some(&key), now));
        // Spotify gets it too, for playlists that are synced and editable.
        if self.spotify_link(id).is_some_and(|p| p.editable) {
            self.queue_spotify_op(id, crate::spotify::Op::Cover { key });
        }
        self.reload_playlists();
        Ok(())
    }

    /// Imports an M3U/M3U8/XSPF file as a new playlist. Returns its ID and
    /// how many entries matched library tracks, of how many.
    pub async fn import_playlist(&self, path: PathBuf) -> Result<(i64, usize, usize), String> {
        let (name, entries) = gio::spawn_blocking(move || playlist_files::read(&path))
            .await
            .map_err(|_| "the import failed".to_owned())?
            .map_err(|e| e.to_string())?;
        let total = entries.len();
        let mut db = self.db().ok_or("no database")?;
        let now = metadata::now();
        let id = db.create_playlist(&name, now).map_err(|e| e.to_string())?;
        let matched = db
            .import_entries(id, &entries, now)
            .map_err(|e| e.to_string())?;
        drop(db);
        self.reload_playlists();
        Ok((id, matched, total))
    }

    /// Writes a playlist as M3U8 or XSPF (by the file's extension).
    pub async fn export_playlist(&self, id: i64, path: PathBuf) -> Result<(), String> {
        let name = self
            .playlist(id)
            .map(|p| p.playlist().name.clone())
            .unwrap_or_default();
        let entries: Vec<PlaylistFileEntry> = self
            .playlist_items(id)
            .into_iter()
            .map(|item| PlaylistFileEntry {
                location: item.track.as_ref().map(|t| t.record.path.clone()),
                title: item.title,
                artist: item.artist,
                album: item.album,
                duration_ms: item.duration_ms,
            })
            .collect();
        gio::spawn_blocking(move || {
            let text = match Format::from_path(&path) {
                Format::Xspf => playlist_files::write_xspf(&name, &entries),
                Format::M3u8 => playlist_files::write_m3u8(&entries),
            };
            std::fs::write(&path, text).map_err(|e| e.to_string())
        })
        .await
        .map_err(|_| "the export failed".to_owned())?
    }

    /// Full-text search (FTS5, §4.1).
    pub fn search(&self, query: &str, limit: usize) -> SearchResults {
        self.db()
            .and_then(|db| db.search(query, limit).ok())
            .unwrap_or_default()
    }

    fn folders(&self) -> Vec<PathBuf> {
        self.imp()
            .settings
            .get()
            .map(library_folders::load)
            .unwrap_or_default()
    }

    /// Starts an incremental scan, or queues one if a scan is running.
    /// "Rescan Library": everything regrouped, also if no file changed.
    pub fn rescan_all(&self) {
        self.imp().force_rescan.set(true);
        self.rescan();
    }

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
        let force = imp.force_rescan.take();
        let lyrembed_override = self.settings().string("lyrembed-path").to_string();
        let (tx, rx) = async_channel::unbounded();
        std::thread::spawn(move || {
            // Not just PATH: from the GNOME menu, ~/.local/bin is often
            // missing there (§10.0).
            let lyrembed = jewelcase_tools::find_program(
                "lyrembed",
                Some(std::path::Path::new(&lyrembed_override)),
            )
            .or_else(|| jewelcase_tools::find_program("lyrembed", None));
            // lyrembed's service when it's set up (also from a Flatpak),
            // else the program.
            let text = crate::tools::lyrembed_aliases_over_dbus()
                .or_else(|| lyrembed.as_deref().and_then(Aliases::list_from_program));
            let aliases = Aliases::from_text_cached(text, &config::aliases_cache_path());
            let _ = tx.send_blocking(ScanMessage::Aliases(aliases.clone()));
            let result = Database::open(&config::database_path())
                .map_err(|e| e.to_string())
                .and_then(|mut db| {
                    let covers = config::cover_cache();
                    let progress = |p| {
                        let _ = tx.send_blocking(ScanMessage::Progress(p));
                    };
                    scanner::scan(&mut db, &folders, &aliases, &covers, force, &progress)
                        .map_err(|e| e.to_string())
                });
            let _ = tx.send_blocking(ScanMessage::Done(result));
        });

        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = library)]
            self,
            async move {
                let mut reloaded = false;
                let mut done = false;
                while let Ok(message) = rx.recv().await {
                    match message {
                        ScanMessage::Aliases(aliases) => {
                            let old = library.imp().aliases.replace(aliases);
                            if old != *library.aliases() {
                                // Matched with the new ones from now on.
                                library.imp().match_index.replace(None);
                                library.imp().album_index.replace(None);
                            }
                        }
                        ScanMessage::Progress(ScanProgress::AlbumsReady) => {
                            library.reload_in_background();
                            reloaded = true;
                        }
                        ScanMessage::Progress(progress) => library.show_progress(progress),
                        ScanMessage::Done(result) => {
                            done = true;
                            library.scan_done(result, reloaded);
                            break;
                        }
                    }
                }
                // The scan thread ended without a result (it panicked):
                // not left "scanning" for good, which would hold back
                // every later scan.
                if !done {
                    library.scan_done(Err("the scan stopped unexpectedly".into()), reloaded);
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

    /// `reloaded`: the regrouped albums are shown already.
    fn scan_done(&self, result: Result<ScanSummary, String>, reloaded: bool) {
        let imp = self.imp();
        // Read the library again only if the scan changed something (or
        // failed): most scans find nothing new.
        let changed = match &result {
            Ok(summary) => (summary.regrouped && !reloaded) || summary.covers_made > 0,
            Err(_) => true,
        };
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
        if changed {
            self.reload_in_background();
        }
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
