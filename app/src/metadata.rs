//! Artist lookups on a background thread (CONCEPT.md §8).
//!
//! Portrait order: the user's own choice (stored separately, always wins),
//! a local `artist.*`/`folder.*` in the artist's folder, Spotify (when
//! connected), Deezer, then Wikimedia Commons. Downloaded images are
//! decoded in glycin's sandbox like covers.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use jewelcase_core::db::{ArtistMeta, Database};
use jewelcase_core::library::covers::{self, CoverCache};
use jewelcase_core::library::grouping::album_folder;
use jewelcase_metadata::{
    ArtistQuery, Candidate, Client, Identity, Links, PortraitCandidate, Providers,
};

use crate::config;

/// Lookups are repeated after this long (seconds).
pub const REFRESH_DONE: i64 = 30 * 24 * 3600;
/// Failed or unclear lookups are retried after this long.
pub const REFRESH_FAILED: i64 = 7 * 24 * 3600;
/// New releases are those of the last 30 days.
pub const NEW_RELEASES_KEPT_S: i64 = 30 * 24 * 3600;
/// A lookup that a portrait source couldn't answer (Spotify's or
/// Deezer's rate limit, offline) is repeated after this long.
pub const PARTIAL_AGAIN: i64 = 24 * 3600;
/// Spotify is asked for portraits at most this often: after an upgrade
/// many artists are looked up, and Spotify blocks apps that ask a lot.
const SPOTIFY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// The Spotify account, for portraits (`None`: not connected, or turned
/// off in Preferences).
pub type SpotifyClient = Option<Arc<jewelcase_spotify::Client>>;

const LOCAL_PORTRAIT_NAMES: &[&str] = &["artist", "folder"];
const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];

pub enum Job {
    /// Local portrait, plus the online lookup if `online`. `urgent`: an
    /// open artist page or "Look Up Again" (before tiles' lookups);
    /// `again`: run even if this artist was looked up this session.
    Lookup {
        artist_id: i64,
        online: bool,
        providers: Providers,
        spotify: SpotifyClient,
        language: String,
        urgent: bool,
        again: bool,
    },
    /// MusicBrainz search for "Identify Artist…".
    Search {
        name: String,
        reply: async_channel::Sender<Result<Vec<Candidate>, String>>,
    },
    /// Portraits from every provider, downloaded and cached, for "Choose
    /// from Online Sources…": (portrait key, candidate).
    Candidates {
        artist_id: i64,
        providers: Providers,
        spotify: SpotifyClient,
        reply: async_channel::Sender<Result<Vec<(String, PortraitCandidate)>, String>>,
    },
    /// ISRCs of an album's songs from MusicBrainz, for matching Spotify
    /// entries (§9.5, step 2); replies how many were found.
    ReleaseIsrcs {
        release: String,
        reply: async_channel::Sender<Result<usize, String>>,
    },
    /// Related artists from ListenBrainz, for smart shuffle (§7); replies
    /// how many, or how long to wait.
    RelatedArtists {
        mbid: String,
        reply: async_channel::Sender<Result<usize, RelatedError>>,
    },
    /// A followed Spotify artist's recent releases from Deezer (§9.8):
    /// replies how many, or why Deezer can't be asked now.
    ArtistReleases {
        spotify_id: String,
        name: String,
        reply: async_channel::Sender<Result<usize, String>>,
    },
    /// After the artists: forgets releases of artists no longer followed
    /// and reads the barcodes of new ones.
    ReleaseBarcodes {
        reply: async_channel::Sender<Result<(), String>>,
    },
    /// An image file the user picked as the portrait.
    SetCustom {
        artist_id: i64,
        data: Vec<u8>,
        reply: async_channel::Sender<Result<(), String>>,
    },
}

/// An error of one artist's (or release's) Deezer request: `Ok(0)` when
/// it's about that one only (unexpected data), else `Err`, which stops the
/// check (offline, Deezer's quota).
fn about_artist(err: jewelcase_metadata::Error) -> Result<usize, String> {
    match err {
        jewelcase_metadata::Error::Format(what) => {
            tracing::info!("Unexpected answer from {what}");
            Ok(0)
        }
        err => Err(err.to_string()),
    }
}

fn sort_job(
    job: Job,
    urgent: &mut std::collections::VecDeque<Job>,
    background: &mut std::collections::VecDeque<Job>,
) {
    match &job {
        Job::Lookup { urgent: false, .. }
        | Job::ReleaseIsrcs { .. }
        | Job::RelatedArtists { .. }
        | Job::ArtistReleases { .. }
        | Job::ReleaseBarcodes { .. } => {
            background.push_back(job);
        }
        _ => urgent.push_back(job),
    }
}

/// Why related artists couldn't be asked for.
#[derive(Debug)]
pub enum RelatedError {
    /// ListenBrainz asks to wait (this long, if it said).
    Busy(std::time::Duration),
    Other(String),
}

/// Sent to the main thread when an artist's data changed.
pub struct ArtistChanged(pub i64);

pub struct MetadataService {
    jobs: async_channel::Sender<Job>,
}

impl MetadataService {
    pub fn start(updates: async_channel::Sender<ArtistChanged>) -> Self {
        let (jobs, queue) = async_channel::unbounded::<Job>();
        std::thread::Builder::new()
            .name("metadata".into())
            .spawn(move || {
                let db = match Database::open(&config::database_path()) {
                    Ok(db) => db,
                    Err(err) => {
                        tracing::error!("Metadata worker can't open the database: {err}");
                        return;
                    }
                };
                let mut worker = Worker {
                    db,
                    client: Client::new(config::VERSION),
                    portraits: CoverCache::new(config::portrait_dir()),
                    updates,
                    last_spotify: None,
                };
                worker.clean_portraits();
                // Dialogs and the open artist page go before the lookups
                // that artist tiles queue (one at a time, each a few
                // seconds with MusicBrainz' rate limit).
                let mut urgent = std::collections::VecDeque::new();
                let mut background = std::collections::VecDeque::new();
                // Artists looked up this session, and whether online.
                let mut looked_up: std::collections::HashMap<i64, bool> =
                    std::collections::HashMap::new();
                loop {
                    if urgent.is_empty() && background.is_empty() {
                        match queue.recv_blocking() {
                            Ok(job) => sort_job(job, &mut urgent, &mut background),
                            Err(_) => break,
                        }
                    }
                    while let Ok(job) = queue.try_recv() {
                        sort_job(job, &mut urgent, &mut background);
                    }
                    let Some(job) = urgent.pop_front().or_else(|| background.pop_front()) else {
                        continue;
                    };
                    // Done already (an urgent one before this queued one).
                    // One looked up offline is looked up again once online
                    // metadata is on.
                    if let Job::Lookup {
                        artist_id,
                        online,
                        again,
                        ..
                    } = &job
                        && !again
                        && looked_up
                            .get(artist_id)
                            .is_some_and(|was_online| *was_online || !online)
                    {
                        continue;
                    }
                    if let Job::Lookup {
                        artist_id, online, ..
                    } = &job
                    {
                        let was_online = looked_up.get(artist_id).copied().unwrap_or(false);
                        looked_up.insert(*artist_id, was_online || *online);
                    }
                    worker.run(job);
                }
            })
            .expect("can start a thread");
        Self { jobs }
    }

    pub fn send(&self, job: Job) {
        let _ = self.jobs.send_blocking(job);
    }
}

struct Worker {
    db: Database,
    client: Client,
    portraits: CoverCache,
    updates: async_channel::Sender<ArtistChanged>,
    /// When Spotify was last asked (see SPOTIFY_INTERVAL).
    last_spotify: Option<std::time::Instant>,
}

impl Worker {
    /// About once a week: portraits no artist shows any more (replaced,
    /// and the candidates of "Choose from Online Sources…").
    fn clean_portraits(&mut self) {
        const KEY: &str = "portrait-cache-cleaned";
        const EVERY_S: i64 = 7 * 24 * 3600;
        let last: i64 = self
            .db
            .app_state(KEY)
            .ok()
            .flatten()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        if now() - last < EVERY_S {
            return;
        }
        let Ok(keep) = self.db.portrait_keys_in_use() else {
            return;
        };
        let removed = self.portraits.remove_unused(&keep, &['o', 'l', 'c']);
        if removed > 0 {
            tracing::info!("Removed {removed} unused portraits from the cache");
        }
        let repeated = self.portraits.remove_repeated_sizes();
        if repeated > 0 {
            tracing::info!("Removed {repeated} portraits that repeated a smaller size");
        }
        let _ = self.db.set_app_state(KEY, &now().to_string());
    }

    fn run(&mut self, job: Job) {
        match job {
            Job::Lookup {
                artist_id,
                online,
                providers,
                spotify,
                language,
                ..
            } => {
                if let Err(err) = self.lookup(artist_id, online, &providers, &spotify, language) {
                    tracing::warn!("Lookup of artist {artist_id} failed: {err}");
                }
                let _ = self.updates.send_blocking(ArtistChanged(artist_id));
            }
            Job::Search { name, reply } => {
                let result = self.client.search_artists(&name).map_err(|e| e.to_string());
                let _ = reply.send_blocking(result);
            }
            Job::Candidates {
                artist_id,
                providers,
                spotify,
                reply,
            } => {
                let _ = reply.send_blocking(self.candidates(artist_id, &providers, &spotify));
            }
            Job::ReleaseIsrcs { release, reply } => {
                let result = match self.client.release_isrcs(&release) {
                    Ok(isrcs) => self
                        .db
                        .set_release_isrcs(&release, &isrcs, now())
                        .map(|()| isrcs.len())
                        .map_err(|e| e.to_string()),
                    // Gone from MusicBrainz (merged), or an ID it refuses:
                    // not asked again soon, and no reason to stop the rest.
                    Err(err) if err.is_refused() => self
                        .db
                        .set_release_isrcs(&release, &[], now())
                        .map(|()| 0)
                        .map_err(|e| e.to_string()),
                    Err(err) => Err(err.to_string()),
                };
                let _ = reply.send_blocking(result);
            }
            Job::RelatedArtists { mbid, reply } => {
                let result = match self.client.similar_artists(&mbid) {
                    Ok(related) => self
                        .db
                        .set_related_artists(&mbid, &related, now())
                        .map(|()| related.len())
                        .map_err(|e| RelatedError::Other(e.to_string())),
                    Err(jewelcase_metadata::Error::Busy(wait)) => Err(RelatedError::Busy(wait)),
                    Err(err) => Err(RelatedError::Other(err.to_string())),
                };
                let _ = reply.send_blocking(result);
            }
            Job::ArtistReleases {
                spotify_id,
                name,
                reply,
            } => {
                let _ = reply.send_blocking(self.artist_releases(&spotify_id, &name));
            }
            Job::ReleaseBarcodes { reply } => {
                let _ = reply.send_blocking(self.release_barcodes());
            }
            Job::SetCustom {
                artist_id,
                data,
                reply,
            } => {
                let result = self.set_custom(artist_id, data);
                let _ = reply.send_blocking(result);
                let _ = self.updates.send_blocking(ArtistChanged(artist_id));
            }
        }
    }

    fn lookup(
        &mut self,
        artist_id: i64,
        online: bool,
        providers: &Providers,
        spotify: &SpotifyClient,
        language: String,
    ) -> Result<(), String> {
        let artist = self
            .db
            .artist(artist_id)
            .map_err(|e| e.to_string())?
            .ok_or("the artist is gone")?;
        let mut meta = ArtistMeta {
            custom_portrait_key: artist.meta.custom_portrait_key.clone(),
            ..ArtistMeta::default()
        };

        let paths = self
            .db
            .artist_track_paths(artist_id)
            .map_err(|e| e.to_string())?;
        if let Some(path) = local_portrait(&paths, &artist.name)
            && let Ok(data) = fs::read(&path)
        {
            let key = covers::cover_key('l', &data);
            if self.portraits.has(&key) || self.portraits.generate(&key, data).is_ok() {
                meta.portrait_key = Some(key);
                meta.portrait_source = Some("local".into());
            }
        }

        if !online {
            meta.lookup_state = Some("offline".into());
        } else {
            let query = ArtistQuery {
                name: artist.name.clone(),
                mbid: artist.mbid.clone(),
                release_mbids: self
                    .db
                    .artist_release_mbids(artist_id)
                    .map_err(|e| e.to_string())?,
                album_titles: self.album_titles(artist_id),
                language,
            };
            match self.client.lookup(&query, providers) {
                Ok(mut info) => {
                    meta.lookup_state = Some(
                        match &info.identity {
                            Identity::Found(_) => "done",
                            Identity::Ambiguous(_) => "ambiguous",
                            Identity::NotFound => "not-found",
                        }
                        .into(),
                    );
                    if let Identity::Found(mbid) = info.identity {
                        meta.lookup_mbid = Some(mbid);
                    }
                    // The ID found earlier stays unless Wikidata has one.
                    info.links.spotify_id = info
                        .links
                        .spotify_id
                        .take()
                        .or(artist.meta.spotify_id.clone());
                    // Spotify's picture first (by that ID, else by name).
                    let mut spotify_missing = false;
                    if let Some(client) = spotify {
                        let known = info.links.spotify_id.clone();
                        match self.spotify_portrait(client, known.as_deref(), &artist.name) {
                            Ok(Some((id, candidate))) => {
                                info.links.spotify_id.get_or_insert(id);
                                info.portraits.insert(0, candidate);
                            }
                            Ok(None) => {}
                            Err(()) => spotify_missing = true,
                        }
                    }
                    // A source couldn't answer (too many requests,
                    // offline): done otherwise, asked again in a day.
                    let partial = spotify_missing || info.incomplete;
                    if partial && meta.lookup_state.as_deref() == Some("done") {
                        meta.lookup_state = Some("partial".into());
                    }
                    meta.wikidata_id = info.links.wikidata_id;
                    meta.spotify_id = info.links.spotify_id;
                    meta.website = info.links.website;
                    meta.wikipedia_url = info.links.wikipedia_url;
                    meta.bio = info.bio;
                    // Spotify's photo from before stays while Spotify can't
                    // be asked, rather than a lesser one for a day.
                    let keep_spotify = spotify_missing
                        && artist.meta.portrait_source.as_deref() == Some("spotify");
                    if meta.portrait_key.is_none() && !keep_spotify {
                        for candidate in &info.portraits {
                            if let Some(key) = self.fetch_portrait(candidate) {
                                meta.portrait_key = Some(key);
                                meta.portrait_source = Some(candidate.source.into());
                                meta.portrait_attribution = candidate.attribution.clone();
                                break;
                            }
                        }
                    }
                    // Nothing new while a source couldn't answer: the
                    // photo from before stays.
                    if meta.portrait_key.is_none()
                        && partial
                        && artist.meta.portrait_source.as_deref() != Some("local")
                    {
                        meta.portrait_key.clone_from(&artist.meta.portrait_key);
                        meta.portrait_source
                            .clone_from(&artist.meta.portrait_source);
                        meta.portrait_attribution
                            .clone_from(&artist.meta.portrait_attribution);
                    }
                }
                Err(err) => {
                    tracing::info!("Online lookup of {} failed: {err}", artist.name);
                    // Keep what an earlier lookup found, plus a local portrait.
                    let mut earlier = artist.meta.clone();
                    if meta.portrait_key.is_some() {
                        earlier.portrait_key = meta.portrait_key.take();
                        earlier.portrait_source = meta.portrait_source.take();
                        earlier.portrait_attribution = None;
                    }
                    earlier.lookup_state = Some("error".into());
                    meta = earlier;
                }
            }
        }
        meta.looked_up = Some(now());
        self.db
            .set_artist_meta(artist_id, &meta)
            .map_err(|e| e.to_string())
    }

    /// Downloads and caches a portrait; returns its key.
    fn fetch_portrait(&self, candidate: &PortraitCandidate) -> Option<String> {
        let data = match self.client.download(&candidate.url) {
            Ok(data) => data,
            Err(err) => {
                tracing::info!("Portrait download failed: {err}");
                return None;
            }
        };
        let key = covers::cover_key('o', &data);
        if !self.portraits.has(&key)
            && let Err(err) = self.portraits.generate(&key, data)
        {
            tracing::info!("Portrait can't be used: {err}");
            return None;
        }
        Some(key)
    }

    fn candidates(
        &mut self,
        artist_id: i64,
        providers: &Providers,
        spotify: &SpotifyClient,
    ) -> Result<Vec<(String, PortraitCandidate)>, String> {
        let artist = self
            .db
            .artist(artist_id)
            .map_err(|e| e.to_string())?
            .ok_or("the artist is gone")?;
        let query = ArtistQuery {
            name: artist.name.clone(),
            mbid: artist.mbid.clone(),
            album_titles: self.album_titles(artist_id),
            ..ArtistQuery::default()
        };
        let links = Links {
            wikidata_id: artist.meta.wikidata_id.clone(),
            ..Links::default()
        };
        let mut candidates = Vec::new();
        if let Some(client) = spotify
            && let Ok(Some((_, candidate))) =
                self.spotify_portrait(client, artist.meta.spotify_id.as_deref(), &artist.name)
        {
            candidates.push(candidate);
        }
        candidates.extend(self.client.portrait_candidates(&query, &links, providers).0);
        Ok(candidates
            .into_iter()
            .filter_map(|candidate| Some((self.fetch_portrait(&candidate)?, candidate)))
            .collect())
    }

    /// The artist's album titles, to tell artists of the same name apart.
    fn album_titles(&self, artist_id: i64) -> Vec<String> {
        self.db
            .artist_albums(artist_id)
            .unwrap_or_default()
            .into_iter()
            .map(|(album, _)| album.title)
            .collect()
    }

    /// The artist's picture on Spotify, with their Spotify ID: by `known`
    /// ID (from Wikidata, or stored for followed artists), else the
    /// search result of exactly that name. `Err` when Spotify couldn't be
    /// asked for now (too many requests, offline), to try again later.
    fn spotify_portrait(
        &mut self,
        client: &jewelcase_spotify::Client,
        known: Option<&str>,
        name: &str,
    ) -> Result<Option<(String, PortraitCandidate)>, ()> {
        let failed = |what: &str, err: jewelcase_spotify::Error| {
            tracing::info!("Spotify {what}: {err}");
            if err.is_temporary() {
                Err(())
            } else {
                Ok(None)
            }
        };
        let found = match known {
            // Followed artists are stored with their picture.
            Some(id) => match self.db.spotify_artist(id).ok().flatten() {
                Some(stored) if stored.image_url.is_some() => {
                    Some((id.to_owned(), stored.image_url))
                }
                _ => match self.pace_spotify(|| client.artist(id)) {
                    Ok(artist) => Some((artist.id, artist.image_url)),
                    Err(err) => return failed(&format!("artist {id}"), err),
                },
            },
            None => {
                let wanted = name.trim().to_lowercase();
                match self.pace_spotify(|| client.search_artists(name)) {
                    // Only when it's the one artist of that name: of
                    // several, the photo could be someone else's.
                    Ok(artists) => {
                        let mut same = artists
                            .into_iter()
                            .filter(|a| a.name.trim().to_lowercase() == wanted);
                        match (same.next(), same.next()) {
                            (Some(only), None) => Some((only.id, only.image_url)),
                            _ => None,
                        }
                    }
                    Err(err) => return failed(&format!("search for the artist {name}"), err),
                }
            }
        };
        Ok(found.and_then(|(id, url)| {
            Some((
                id,
                PortraitCandidate {
                    url: url?,
                    source: "spotify",
                    attribution: Some("Spotify".to_owned()),
                },
            ))
        }))
    }

    /// Runs a Spotify request, at least SPOTIFY_INTERVAL after the last.
    fn pace_spotify<T>(&mut self, request: impl FnOnce() -> T) -> T {
        if let Some(last) = self.last_spotify {
            let wait = SPOTIFY_INTERVAL.saturating_sub(last.elapsed());
            std::thread::sleep(wait);
        }
        let result = request();
        self.last_spotify = Some(std::time::Instant::now());
        result
    }

    /// A followed artist's recent releases on Deezer (§9.8). Their Deezer
    /// artist is looked for once (again after a week when not found).
    /// `Err` for what isn't about this artist (offline, Deezer's quota).
    fn artist_releases(&mut self, spotify_id: &str, name: &str) -> Result<usize, String> {
        use jewelcase_core::db::NewRelease;
        let now = now();
        let stored = self
            .db
            .deezer_artist(spotify_id)
            .map_err(|e| e.to_string())?;
        let deezer_id = match stored {
            Some((Some(id), _)) => id as u64,
            Some((None, matched)) if now - matched < REFRESH_FAILED => return Ok(0),
            _ => {
                let known = self
                    .db
                    .known_albums(spotify_id, name)
                    .map_err(|e| e.to_string())?;
                let id = match self.client.deezer_artist(name, &known.upcs, &known.titles) {
                    Ok(id) => id,
                    Err(err) => return about_artist(err),
                };
                if id.is_none() {
                    tracing::info!("{name} isn't on Deezer, or not clearly");
                }
                self.db
                    .set_deezer_artist(spotify_id, id.map(|i| i as i64), now)
                    .map_err(|e| e.to_string())?;
                match id {
                    Some(id) => id,
                    None => return Ok(0),
                }
            }
        };
        let releases = match self.client.deezer_releases(deezer_id) {
            Ok(releases) => releases,
            Err(err) => return about_artist(err),
        };
        let today = jewelcase_metadata::date_string(now);
        let since = jewelcase_metadata::date_string(now - NEW_RELEASES_KEPT_S);
        let recent: Vec<NewRelease> =
            jewelcase_metadata::recent_releases(&releases, &since, &today)
                .into_iter()
                .map(|r| NewRelease {
                    deezer_id: r.id as i64,
                    artist_spotify_id: spotify_id.to_owned(),
                    artist: name.to_owned(),
                    title: r.title,
                    kind: r.kind,
                    release_date: r.date,
                    image_url: r.cover_url,
                    upc: None,
                    spotify_album_id: None,
                    seen: false,
                })
                .collect();
        self.db
            .replace_new_releases(spotify_id, &recent)
            .map_err(|e| e.to_string())?;
        Ok(recent.len())
    }

    /// Forgets releases of artists no longer followed, and reads the
    /// barcodes of the others (Deezer's album lists have none), to find
    /// them on Spotify.
    fn release_barcodes(&mut self) -> Result<(), String> {
        self.db
            .remove_unfollowed_releases()
            .map_err(|e| e.to_string())?;
        let since = jewelcase_metadata::date_string(now() - NEW_RELEASES_KEPT_S);
        let releases = self.db.new_releases(&since).map_err(|e| e.to_string())?;
        for release in releases.iter().filter(|r| r.upc.is_none()) {
            match self.client.deezer_upc(release.deezer_id as u64) {
                Ok(Some(upc)) => self
                    .db
                    .set_new_release_upc(release.deezer_id, &upc)
                    .map_err(|e| e.to_string())?,
                Ok(None) => {}
                Err(err) => about_artist(err).map(|_| ())?,
            }
        }
        Ok(())
    }

    fn set_custom(&mut self, artist_id: i64, data: Vec<u8>) -> Result<(), String> {
        let key = covers::cover_key('c', &data);
        if !self.portraits.has(&key) {
            self.portraits
                .generate(&key, data)
                .map_err(|e| e.to_string())?;
        }
        self.db
            .set_custom_portrait(artist_id, Some(&key))
            .map_err(|e| e.to_string())
    }
}

/// `artist.*` or `folder.*` in the artist's folder: the folder above the
/// album folders, if it's named like the artist (Kodi/Jellyfin layout).
fn local_portrait(track_paths: &[PathBuf], artist: &str) -> Option<PathBuf> {
    let mut folders: Vec<&Path> = track_paths
        .iter()
        .filter_map(|path| album_folder(path).parent())
        .filter(|dir| {
            dir.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.eq_ignore_ascii_case(artist))
        })
        .collect();
    folders.sort();
    folders.dedup();
    folders.into_iter().find_map(find_image)
}

fn find_image(dir: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    let mut best: Option<(usize, PathBuf)> = None;
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let (Some(stem), Some(ext)) = (
            path.file_stem().and_then(|s| s.to_str()),
            path.extension().and_then(|s| s.to_str()),
        ) else {
            continue;
        };
        if !IMAGE_EXTENSIONS.contains(&ext.to_lowercase().as_str()) {
            continue;
        }
        if let Some(rank) = LOCAL_PORTRAIT_NAMES
            .iter()
            .position(|n| n.eq_ignore_ascii_case(stem))
            && best.as_ref().is_none_or(|(r, _)| rank < *r)
        {
            best = Some((rank, path));
        }
    }
    best.map(|(_, path)| path)
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// The UI language for Wikipedia links, e.g. "de".
pub fn language() -> String {
    gtk::glib::language_names()
        .iter()
        .map(|l| l.split(['_', '.', '@']).next().unwrap_or("").to_owned())
        .find(|l| l.len() == 2 || l.len() == 3)
        .unwrap_or_else(|| "en".into())
}
