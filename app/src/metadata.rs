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

use gatefold_core::db::{ArtistMeta, Database};
use gatefold_core::library::covers::{self, CoverCache};
use gatefold_core::library::grouping::album_folder;
use gatefold_metadata::{
    ArtistQuery, Candidate, Client, Identity, Links, PortraitCandidate, Providers,
};

use crate::config;

/// Lookups are repeated after this long (seconds).
pub const REFRESH_DONE: i64 = 30 * 24 * 3600;
/// Failed or unclear lookups are retried after this long.
pub const REFRESH_FAILED: i64 = 7 * 24 * 3600;
/// A lookup whose Spotify part couldn't be done (too many requests,
/// offline) is repeated after this long.
pub const SPOTIFY_LATER: i64 = 24 * 3600;

/// The Spotify account, for portraits (`None`: not connected, or turned
/// off in Preferences).
pub type SpotifyClient = Option<Arc<gatefold_spotify::Client>>;

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
    /// An image file the user picked as the portrait.
    SetCustom {
        artist_id: i64,
        data: Vec<u8>,
        reply: async_channel::Sender<Result<(), String>>,
    },
}

fn sort_job(
    job: Job,
    urgent: &mut std::collections::VecDeque<Job>,
    background: &mut std::collections::VecDeque<Job>,
) {
    match &job {
        Job::Lookup { urgent: false, .. } => background.push_back(job),
        _ => urgent.push_back(job),
    }
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
                };
                // Dialogs and the open artist page go before the lookups
                // that artist tiles queue (one at a time, each a few
                // seconds with MusicBrainz' rate limit).
                let mut urgent = std::collections::VecDeque::new();
                let mut background = std::collections::VecDeque::new();
                let mut looked_up = std::collections::HashSet::new();
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
                    if let Job::Lookup {
                        artist_id, again, ..
                    } = &job
                        && !looked_up.insert(*artist_id)
                        && !again
                    {
                        // Done already (an urgent one before this queued one).
                        continue;
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
}

impl Worker {
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
                    // Spotify's picture first (by the ID Wikidata gave,
                    // else by name).
                    let known = info
                        .links
                        .spotify_id
                        .clone()
                        .or(artist.meta.spotify_id.clone());
                    if let Some(client) = spotify {
                        match self.spotify_portrait(client, known.as_deref(), &artist.name) {
                            Ok(Some((id, candidate))) => {
                                info.links.spotify_id.get_or_insert(id);
                                info.portraits.insert(0, candidate);
                            }
                            Ok(None) => {}
                            // Too many requests, offline: done otherwise,
                            // Spotify is asked again in a day (see
                            // SPOTIFY_LATER).
                            Err(()) => {
                                if meta.lookup_state.as_deref() == Some("done") {
                                    meta.lookup_state = Some("spotify-later".into());
                                }
                            }
                        }
                    }
                    meta.wikidata_id = info.links.wikidata_id;
                    meta.spotify_id = info.links.spotify_id;
                    meta.website = info.links.website;
                    meta.wikipedia_url = info.links.wikipedia_url;
                    meta.bio = info.bio;
                    if meta.portrait_key.is_none() {
                        for candidate in &info.portraits {
                            if let Some(key) = self.fetch_portrait(candidate) {
                                meta.portrait_key = Some(key);
                                meta.portrait_source = Some(candidate.source.into());
                                meta.portrait_attribution = candidate.attribution.clone();
                                break;
                            }
                        }
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
        &self,
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
        candidates.extend(self.client.portrait_candidates(&query, &links, providers));
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
        &self,
        client: &gatefold_spotify::Client,
        known: Option<&str>,
        name: &str,
    ) -> Result<Option<(String, PortraitCandidate)>, ()> {
        let failed = |what: &str, err: gatefold_spotify::Error| {
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
                _ => match client.artist(id) {
                    Ok(artist) => Some((artist.id, artist.image_url)),
                    Err(err) => return failed(&format!("artist {id}"), err),
                },
            },
            None => {
                let wanted = name.trim().to_lowercase();
                match client.search_artists(name) {
                    Ok(artists) => artists
                        .into_iter()
                        .find(|a| a.name.trim().to_lowercase() == wanted)
                        .map(|a| (a.id, a.image_url)),
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
