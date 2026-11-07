//! Saved albums, followed artists and discographies from Spotify
//! (CONCEPT.md §9.8). They're shown apart from the library: a shelf above
//! the album and artist grids, an "On Spotify" part on artist pages, and
//! album panels whose songs play from the library when it has them and
//! stream otherwise.

use std::collections::HashMap;

use gatefold_core::db::{SpotifyAlbum, SpotifyArtist, StreamInfo};
use gatefold_core::matching::{Confidence, RemoteTrack};
use gatefold_spotify::Error;
use gettextrs::gettext;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::Spotify;
use crate::application::GatefoldApplication;
use crate::objects::TrackObject;

/// Discographies are read again after this long.
const DISCOGRAPHY_AGE_S: i64 = 24 * 3600;

#[derive(Default)]
pub struct LibraryState {
    /// Spotify refused to show saved albums or followed artists: the login
    /// predates those permissions; connecting again grants them.
    pub needs_permission: bool,
    /// Songs of albums opened this session.
    albums: HashMap<String, Vec<gatefold_spotify::AlbumTrack>>,
    /// Spotify IDs found by name for library artists (`None`: not found).
    artist_ids: HashMap<String, Option<String>>,
    /// Where the pages of songs played this session are, by URI.
    links: HashMap<String, TrackLinks>,
}

/// A Spotify song's album and artists, for the player's links.
#[derive(Debug, Clone, Default)]
pub struct TrackLinks {
    pub album_id: Option<String>,
    /// (Spotify ID, name), the main artist first.
    pub artists: Vec<(String, String)>,
}

pub fn album_from(album: &gatefold_spotify::Album) -> SpotifyAlbum {
    SpotifyAlbum {
        spotify_id: album.id.clone(),
        name: album.name.clone(),
        artist: album.artist_line(),
        artist_ids: album
            .artists
            .iter()
            .filter_map(|(id, _)| id.clone())
            .collect(),
        album_type: album.album_type.clone(),
        year: album.year,
        total_tracks: album.total_tracks,
        image_url: album.image_url.clone(),
        upc: album.upc.clone(),
    }
}

fn artist_from(artist: &gatefold_spotify::Artist) -> SpotifyArtist {
    SpotifyArtist {
        spotify_id: artist.id.clone(),
        name: artist.name.clone(),
        image_url: artist.image_url.clone(),
        genres: artist.genres.clone(),
    }
}

fn now() -> i64 {
    crate::metadata::now()
}

impl Spotify {
    /// Saved albums and followed artists need a newer login.
    pub fn needs_library_permission(&self) -> bool {
        self.imp().library.borrow().needs_permission
    }

    /// Reads the saved albums and followed artists (with each sync).
    pub(super) async fn sync_library(&self) {
        let albums = self.call(|client| client.saved_albums()).await;
        let artists = self.call(|client| client.followed_artists()).await;
        if !self.is_connected() {
            return;
        }
        let denied = |err: Option<&Error>| matches!(err, Some(Error::Status { status: 403, .. }));
        let needs_permission = denied(albums.as_ref().err()) || denied(artists.as_ref().err());
        let library = GatefoldApplication::get().library();
        let mut urls = Vec::new();
        if let Some(mut db) = library.db() {
            match &albums {
                Ok(albums) => {
                    let albums: Vec<SpotifyAlbum> = albums.iter().map(album_from).collect();
                    urls.extend(albums.iter().filter_map(|a| a.image_url.clone()));
                    if let Err(err) = db.replace_saved_albums(&albums) {
                        tracing::warn!("Cannot store the saved Spotify albums: {err}");
                    }
                }
                Err(err) => tracing::info!("Cannot read the saved Spotify albums: {err}"),
            }
            match &artists {
                Ok(artists) => {
                    let artists: Vec<SpotifyArtist> = artists.iter().map(artist_from).collect();
                    urls.extend(artists.iter().filter_map(|a| a.image_url.clone()));
                    if let Err(err) = db.replace_followed_artists(&artists) {
                        tracing::warn!("Cannot store the followed Spotify artists: {err}");
                    }
                }
                Err(err) => tracing::info!("Cannot read the followed Spotify artists: {err}"),
            }
        }
        self.imp().library.borrow_mut().needs_permission = needs_permission;
        self.emit_by_name::<()>("library-changed", &[]);
        // Only covers not downloaded yet: each batch that arrives shows the
        // sections again, which is wasted work for covers already there.
        let covers = gatefold_core::library::covers::CoverCache::new(crate::config::cover_dir());
        urls.retain(|url| !covers.has(&super::image_cover_key(url)));
        let spotify = self.downgrade();
        super::fetch_covers_in_batches(urls, move |_| {
            if let Some(spotify) = spotify.upgrade() {
                spotify.emit_by_name::<()>("library-changed", &[]);
            }
        });
    }

    /// An artist's albums, EPs and singles on Spotify: read again after a
    /// day; when Spotify can't be asked, what was read before, else why
    /// not (for the user).
    pub async fn discography(&self, artist_id: &str) -> Result<Vec<SpotifyAlbum>, String> {
        let library = GatefoldApplication::get().library();
        let cached = library
            .db()
            .and_then(|db| db.discography(artist_id).ok().flatten());
        if let Some((albums, read)) = &cached
            && now() - read < DISCOGRAPHY_AGE_S
        {
            return Ok(albums.clone());
        }
        if !self.is_connected() {
            return cached
                .map(|(albums, _)| albums)
                .ok_or_else(|| gettext("Not connected to Spotify; connect in Preferences."));
        }
        let id = artist_id.to_owned();
        match self.call(move |client| client.artist_albums(&id)).await {
            Ok(albums) => {
                let albums: Vec<SpotifyAlbum> = albums.iter().map(album_from).collect();
                if let Some(mut db) = library.db()
                    && let Err(err) = db.store_discography(artist_id, &albums, now())
                {
                    tracing::warn!("Cannot store a discography: {err}");
                }
                Ok(albums)
            }
            Err(err) => {
                tracing::warn!("Cannot read the releases of {artist_id}: {err}");
                cached
                    .map(|(albums, _)| albums)
                    .ok_or_else(|| super::answer_error(&err))
            }
        }
    }

    /// A Spotify artist: stored, else asked for (a page for an artist that
    /// isn't followed).
    pub async fn spotify_artist(&self, artist_id: &str) -> Option<SpotifyArtist> {
        let library = GatefoldApplication::get().library();
        if let Some(artist) = library
            .db()
            .and_then(|db| db.spotify_artist(artist_id).ok().flatten())
        {
            return Some(artist);
        }
        let id = artist_id.to_owned();
        let artist = artist_from(&self.call(move |client| client.artist(&id)).await.ok()?);
        if let Some(mut db) = library.db() {
            let _ = db.store_spotify_artist(&artist);
        }
        Some(artist)
    }

    /// The Spotify ID of a library artist: from the online lookup
    /// (Wikidata), else a search by name (remembered for the session).
    pub async fn artist_id_for(&self, name: &str, known: Option<&str>) -> Option<String> {
        if let Some(id) = known {
            return Some(id.to_owned());
        }
        if let Some(found) = self.imp().library.borrow().artist_ids.get(name) {
            return found.clone();
        }
        if !self.is_connected() {
            return None;
        }
        let query = name.to_owned();
        let found = match self.call(move |client| client.search_artists(&query)).await {
            Ok(artists) => {
                let aliases = GatefoldApplication::get().library().aliases().clone();
                let wanted = aliases.identity(name).to_lowercase();
                artists
                    .into_iter()
                    .find(|a| aliases.identity(&a.name).to_lowercase() == wanted)
                    .map(|a| a.id)
            }
            // Asked again next time.
            Err(err) => {
                tracing::info!("Cannot search Spotify for the artist {name}: {err}");
                return None;
            }
        };
        self.imp()
            .library
            .borrow_mut()
            .artist_ids
            .insert(name.to_owned(), found.clone());
        found
    }

    /// An album's songs, ready to play: library songs where the library has
    /// them, Spotify songs to stream otherwise. `None` if Spotify can't be
    /// asked.
    pub async fn album_tracks(&self, album_id: &str) -> Option<Vec<TrackObject>> {
        let cached = self.imp().library.borrow().albums.get(album_id).cloned();
        let tracks = match cached {
            Some(tracks) => tracks,
            None => {
                let id = album_id.to_owned();
                match self.call(move |client| client.album(&id)).await {
                    Ok((_, tracks)) => {
                        self.imp()
                            .library
                            .borrow_mut()
                            .albums
                            .insert(album_id.to_owned(), tracks.clone());
                        tracks
                    }
                    Err(err) => {
                        tracing::info!("Cannot read the Spotify album {album_id}: {err}");
                        return None;
                    }
                }
            }
        };
        let library = GatefoldApplication::get().library();
        let index = library.match_index();
        let mut db = library.db()?;
        let ids: Vec<(i64, &gatefold_spotify::AlbumTrack)> = tracks
            .iter()
            .filter_map(|entry| {
                let track = &entry.track;
                let local = index
                    .find(&RemoteTrack {
                        title: track.name.clone(),
                        artists: track.artists.clone(),
                        album: track.album.clone(),
                        duration_ms: track.duration_ms,
                        isrc: track.isrc.clone(),
                    })
                    .filter(|(_, confidence)| *confidence == Confidence::Sure)
                    .map(|(id, _)| id);
                let id = local.or_else(|| {
                    db.stream_track_id(&StreamInfo {
                        uri: track.uri.clone(),
                        title: track.name.clone(),
                        artist: Some(track.artist_line()),
                        album: track.album.clone(),
                        duration_ms: track.duration_ms,
                        isrc: track.isrc.clone(),
                        image_url: track.image_url.clone(),
                    })
                    .ok()
                })?;
                Some((id, entry))
            })
            .collect();
        drop(db);
        let mut found = library.tracks(&ids.iter().map(|(id, _)| *id).collect::<Vec<_>>());
        Some(
            ids.into_iter()
                .filter_map(|(id, entry)| {
                    let mut track = found
                        .remove(&id)
                        .or_else(|| library.tracks(&[id]).remove(&id))?;
                    // Streamed songs show their place on the album.
                    if id < 0 {
                        track.record.tags.number = Some(entry.number);
                        track.record.tags.disc = Some(entry.disc);
                    }
                    Some(TrackObject::new(track))
                })
                .collect(),
        )
    }

    /// Where a Spotify song's album and artist pages are; asked once per
    /// session. `None` if Spotify can't be asked.
    pub async fn track_links(&self, uri: &str) -> Option<TrackLinks> {
        if let Some(links) = self.imp().library.borrow().links.get(uri) {
            return Some(links.clone());
        }
        let id = uri.strip_prefix("spotify:track:")?.to_owned();
        let track = match self.call(move |client| client.track(&id)).await {
            Ok(track) => track,
            Err(err) => {
                tracing::warn!("Cannot read the Spotify song {uri}: {err}");
                return None;
            }
        };
        let links = TrackLinks {
            album_id: track.album_id.clone(),
            artists: track
                .artist_ids
                .iter()
                .cloned()
                .zip(track.artists.iter().cloned())
                .collect(),
        };
        self.imp()
            .library
            .borrow_mut()
            .links
            .insert(uri.to_owned(), links.clone());
        Some(links)
    }

    /// A Spotify album by ID: stored (saved albums, discographies), else
    /// asked for; its songs are kept for playing it.
    pub async fn album_by_id(&self, album_id: &str) -> Option<SpotifyAlbum> {
        if let Some(album) = self.stored_album(album_id) {
            return Some(album);
        }
        let id = album_id.to_owned();
        match self.call(move |client| client.album(&id)).await {
            Ok((album, tracks)) => {
                self.imp()
                    .library
                    .borrow_mut()
                    .albums
                    .insert(album_id.to_owned(), tracks);
                Some(album_from(&album))
            }
            Err(err) => {
                tracing::warn!("Cannot read the Spotify album {album_id}: {err}");
                None
            }
        }
    }

    /// Spotify albums by Spotify ID, from the database.
    pub fn stored_album(&self, album_id: &str) -> Option<SpotifyAlbum> {
        GatefoldApplication::get()
            .library()
            .db()
            .and_then(|db| db.spotify_album(album_id).ok().flatten())
    }

    pub fn saved_albums(&self) -> Vec<SpotifyAlbum> {
        GatefoldApplication::get()
            .library()
            .db()
            .and_then(|db| db.saved_albums().ok())
            .unwrap_or_default()
    }

    pub fn followed_artists(&self) -> Vec<SpotifyArtist> {
        GatefoldApplication::get()
            .library()
            .db()
            .and_then(|db| db.followed_artists().ok())
            .unwrap_or_default()
    }
}
