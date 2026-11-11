//! Saved albums, followed artists and discographies from Spotify
//! (CONCEPT.md §9.8). They're shown apart from the library: a shelf above
//! the album and artist grids, an "On Spotify" part on artist pages, and
//! album panels whose songs play from the library when it has them and
//! stream otherwise.

use std::collections::HashMap;

use gettextrs::gettext;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use jewelcase_core::db::{SpotifyAlbum, SpotifyArtist, StreamInfo};
use jewelcase_core::matching::{Confidence, RemoteTrack};
use jewelcase_spotify::Error;

use super::Spotify;
use crate::application::JewelcaseApplication;
use crate::objects::TrackObject;

/// Discographies are read again after this long: a week, as Spotify
/// counts requests to artists over a day, and a prolific artist takes
/// several (10 releases each).
const DISCOGRAPHY_AGE_S: i64 = 7 * 24 * 3600;

/// An artist's releases on Spotify, and when Spotify said so.
pub struct Discography {
    pub albums: Vec<SpotifyAlbum>,
    /// When they were read (Unix seconds).
    pub read: i64,
    /// Spotify couldn't be asked anew (why); these are the stored ones.
    pub problem: Option<String>,
}

/// Something that can be in the user's Spotify library: a saved album,
/// a followed artist.
#[derive(Debug, Clone)]
pub enum LibraryItem {
    Album(SpotifyAlbum),
    Artist { id: String, name: String },
}

impl LibraryItem {
    fn uri(&self) -> String {
        match self {
            Self::Album(album) => format!("spotify:album:{}", album.spotify_id),
            Self::Artist { id, .. } => format!("spotify:artist:{id}"),
        }
    }

    fn done_text(&self, on: bool) -> String {
        match (self, on) {
            // Translators: {} is an album title.
            (Self::Album(album), true) => {
                gettext("Saved “{}” on Spotify").replace("{}", &album.name)
            }
            (Self::Album(album), false) => {
                gettext("Removed “{}” from your Spotify albums").replace("{}", &album.name)
            }
            // Translators: {} is an artist name.
            (Self::Artist { name, .. }, true) => {
                gettext("Following {} on Spotify").replace("{}", name)
            }
            (Self::Artist { name, .. }, false) => {
                gettext("Unfollowed {} on Spotify").replace("{}", name)
            }
        }
    }
}

#[derive(Default)]
pub struct LibraryState {
    /// Saves and follows on their way (by URI).
    pending: std::collections::HashSet<String>,
    /// Spotify refused to show saved albums or followed artists: the login
    /// predates those permissions; connecting again grants them.
    pub needs_permission: bool,
    /// Songs of albums opened this session.
    albums: HashMap<String, Vec<jewelcase_spotify::AlbumTrack>>,
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

pub fn album_from(album: &jewelcase_spotify::Album) -> SpotifyAlbum {
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

fn artist_from(artist: &jewelcase_spotify::Artist) -> SpotifyArtist {
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
        let library = JewelcaseApplication::get().library();
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
        let covers = jewelcase_core::library::covers::CoverCache::new(crate::config::cover_dir());
        urls.retain(|url| !covers.has(&super::image_cover_key(url)));
        let spotify = self.downgrade();
        super::fetch_covers_in_batches(urls, move |_| {
            if let Some(spotify) = spotify.upgrade() {
                spotify.emit_by_name::<()>("library-changed", &[]);
            }
        });
    }

    /// An artist's releases on Spotify: the stored ones while younger than
    /// a week, else asked for anew; `refresh`: asked for anew in any case
    /// (the page's Refresh button). Stored ones also stand in when Spotify
    /// can't be asked.
    pub async fn discography(&self, artist_id: &str, refresh: bool) -> Result<Discography, String> {
        let library = JewelcaseApplication::get().library();
        let cached = library
            .db()
            .and_then(|db| db.discography(artist_id).ok().flatten());
        if let Some((albums, read)) = &cached
            && !refresh
            && now() - read < DISCOGRAPHY_AGE_S
        {
            return Ok(Discography {
                albums: albums.clone(),
                read: *read,
                problem: None,
            });
        }
        let stored = |problem: String| match &cached {
            Some((albums, read)) => Ok(Discography {
                albums: albums.clone(),
                read: *read,
                problem: Some(problem),
            }),
            None => Err(problem),
        };
        if !self.is_connected() {
            return stored(gettext("Not connected to Spotify; connect in Preferences."));
        }
        let id = artist_id.to_owned();
        match self.call(move |client| client.artist_albums(&id)).await {
            Ok(albums) => {
                let albums: Vec<SpotifyAlbum> = albums.iter().map(album_from).collect();
                let read = now();
                if let Some(mut db) = library.db()
                    && let Err(err) = db.store_discography(artist_id, &albums, read)
                {
                    tracing::warn!("Cannot store a discography: {err}");
                }
                Ok(Discography {
                    albums,
                    read,
                    problem: None,
                })
            }
            Err(err) => {
                tracing::warn!("Cannot read the releases of {artist_id}: {err}");
                stored(super::answer_error(&err))
            }
        }
    }

    /// A Spotify artist: stored, else asked for (a page for an artist that
    /// isn't followed).
    pub async fn spotify_artist(&self, artist_id: &str) -> Option<SpotifyArtist> {
        let library = JewelcaseApplication::get().library();
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
    pub async fn artist_id_for(
        &self,
        name: &str,
        known: Option<&str>,
    ) -> Result<Option<String>, String> {
        if let Some(id) = known {
            return Ok(Some(id.to_owned()));
        }
        if let Some(found) = self.imp().library.borrow().artist_ids.get(name) {
            return Ok(found.clone());
        }
        if !self.is_connected() {
            return Ok(None);
        }
        let query = name.to_owned();
        let found = match self.call(move |client| client.search_artists(&query)).await {
            Ok(artists) => {
                let aliases = JewelcaseApplication::get().library().aliases().clone();
                let wanted = aliases.identity(name).to_lowercase();
                artists
                    .into_iter()
                    .find(|a| aliases.identity(&a.name).to_lowercase() == wanted)
                    .map(|a| a.id)
            }
            // Asked again next time.
            Err(err) => {
                tracing::info!("Cannot search Spotify for the artist {name}: {err}");
                return Err(super::answer_error(&err));
            }
        };
        self.imp()
            .library
            .borrow_mut()
            .artist_ids
            .insert(name.to_owned(), found.clone());
        Ok(found)
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
        let library = JewelcaseApplication::get().library();
        let index = library.match_index();
        let mut db = library.db()?;
        let ids: Vec<(i64, &jewelcase_spotify::AlbumTrack)> = tracks
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
                .zip(&track.artists)
                .filter_map(|(id, name)| Some((id.clone()?, name.clone())))
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

    /// Saved or followed, as Jewelcase last knew.
    pub fn in_library(&self, item: &LibraryItem) -> bool {
        JewelcaseApplication::get()
            .library()
            .db()
            .and_then(|db| match item {
                LibraryItem::Album(album) => db.is_album_saved(&album.spotify_id).ok(),
                LibraryItem::Artist { id, .. } => db.is_artist_followed(id).ok(),
            })
            .unwrap_or(false)
    }

    /// A save or follow on its way to Spotify (its button waits).
    pub fn library_pending(&self, item: &LibraryItem) -> bool {
        self.imp().library.borrow().pending.contains(&item.uri())
    }

    /// Asks Spotify whether the item is saved or followed (it may have
    /// changed in the Spotify app since the last sync).
    pub async fn check_in_library(&self, item: LibraryItem) {
        let uri = item.uri();
        let Ok(on) = self.call(move |client| client.library_contains(&uri)).await else {
            return;
        };
        if on != self.in_library(&item) {
            self.store_in_library(&item, on);
            self.emit_by_name::<()>("library-changed", &[]);
        }
    }

    /// Saves an album or follows an artist, or the other way round; says
    /// so with Undo (which `undo` leaves out, being the undo).
    pub async fn set_in_library(&self, item: LibraryItem, on: bool, undo: bool) {
        let uri = item.uri();
        if !self.imp().library.borrow_mut().pending.insert(uri.clone()) {
            return;
        }
        self.emit_by_name::<()>("library-changed", &[]);
        let call_uri = uri.clone();
        let result = self
            .call(move |client| {
                if on {
                    client.save_to_library(&call_uri)
                } else {
                    client.remove_from_library(&call_uri)
                }
            })
            .await;
        self.imp().library.borrow_mut().pending.remove(&uri);
        if let Err(err) = result {
            tracing::warn!("Cannot change {uri} in the Spotify library: {err}");
            self.emit_by_name::<()>("library-changed", &[]);
            self.library_error(&err);
            return;
        }
        self.store_in_library(&item, on);
        self.emit_by_name::<()>("library-changed", &[]);
        let text = item.done_text(on);
        if !undo {
            self.message(&text);
            return;
        }
        let spotify = self.clone();
        crate::window::undo_toast(&text, move || {
            let spotify = spotify.clone();
            let item = item.clone();
            glib::spawn_future_local(async move { spotify.set_in_library(item, !on, false).await });
        });
    }

    fn store_in_library(&self, item: &LibraryItem, on: bool) {
        let library = JewelcaseApplication::get().library();
        let Some(mut db) = library.db() else {
            return;
        };
        let stored = match item {
            LibraryItem::Album(album) => db.set_album_saved(album, on),
            LibraryItem::Artist { id, name } => db.set_artist_followed(id, name, on),
        };
        if let Err(err) = stored {
            tracing::warn!("Cannot store a change of the Spotify library: {err}");
        }
    }

    /// Spotify albums by Spotify ID, from the database.
    pub fn stored_album(&self, album_id: &str) -> Option<SpotifyAlbum> {
        JewelcaseApplication::get()
            .library()
            .db()
            .and_then(|db| db.spotify_album(album_id).ok().flatten())
    }

    pub fn saved_albums(&self) -> Vec<SpotifyAlbum> {
        JewelcaseApplication::get()
            .library()
            .db()
            .and_then(|db| db.saved_albums().ok())
            .unwrap_or_default()
    }

    pub fn followed_artists(&self) -> Vec<SpotifyArtist> {
        JewelcaseApplication::get()
            .library()
            .db()
            .and_then(|db| db.followed_artists().ok())
            .unwrap_or_default()
    }
}

impl Spotify {
    /// Why saving or following didn't work, for the user; a login from
    /// before Jewelcase asked for the permission can connect again.
    fn library_error(&self, err: &Error) {
        if matches!(err, Error::Status { status: 403, .. }) {
            let spotify = self.clone();
            crate::window::button_toast(
                &gettext("Spotify needs you to connect again to allow this"),
                &gettext("_Connect Again"),
                10,
                move || {
                    let spotify = spotify.clone();
                    glib::spawn_future_local(async move { spotify.connect().await });
                },
            );
        } else {
            self.message(&super::sync::answer_error(err));
        }
    }
}
