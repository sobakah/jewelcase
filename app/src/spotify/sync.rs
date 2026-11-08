//! Spotify playlists both ways (CONCEPT.md §9.5, §9.6): reading them into
//! the database, sending local edits through the outbox, finding library
//! songs on Spotify, and publishing local playlists.
//!
//! One job runs at a time; requests made meanwhile are merged into the
//! next run.

use std::collections::HashMap;
use std::time::Duration;

use adw::prelude::*;
use gatefold_core::db::{LinkedPlaylist, RemoteEntry, SpotifyPlaylistInfo, SpotifyVersion, Track};
use gatefold_core::matching::{self, Confidence, LocalTrack, RemoteTrack};
use gatefold_spotify::{Client, Error, PlaylistEntry};
use gettextrs::{gettext, ngettext};
use gtk::glib;
use gtk::subclass::prelude::*;
use serde_json::{Value, json};

use super::Spotify;
use crate::application::GatefoldApplication;
use crate::metadata::now;

/// Local edits wait this long after the last one, so several edits in a
/// row (drags, removes, a bulk add) go out together, and a playlist is
/// rarely locked while it's being changed.
const FLUSH_DELAY: Duration = Duration::from_secs(4);
/// A playlist just created on Spotify may not be in Spotify's list yet.
const NEW_PLAYLIST_GRACE: Duration = Duration::from_secs(600);
/// After a network error, waiting changes are tried again this much later.
const RETRY_DELAY_S: u32 = 60;
/// "Not on Spotify" is believed for this long, then searched again.
const NOT_FOUND_TTL_S: i64 = 30 * 24 * 3600;
/// Pause between searches, to stay below Spotify's rate limit.
const SEARCH_PAUSE: Duration = Duration::from_millis(250);
/// A search waits this long at most when Spotify asks to wait.
const MAX_SEARCH_WAIT: Duration = Duration::from_secs(30);

#[derive(Default)]
pub struct SyncState {
    running: bool,
    /// Requests for the next run.
    full: bool,
    flush: bool,
    playlists: Vec<i64>,
    /// Playlists whose library songs need their Spotify versions, and
    /// whether to say when some aren't on Spotify.
    resolve: Vec<(i64, bool)>,
    flush_timer: Option<glib::SourceId>,
    retry_timer: Option<glib::SourceId>,
    /// A full sync / these playlists' searches are to be tried again
    /// (one retry each, not one per failure).
    full_retry: bool,
    resolve_retry: std::collections::HashSet<i64>,
    /// Playlists whose changes wait until the user is done (reordering).
    held: std::collections::HashSet<i64>,
    /// Local playlists being created on Spotify ("Create on Spotify").
    publishing: std::collections::HashSet<i64>,
    /// Creations waiting for their turn: (playlist, entries found).
    create: Vec<Creation>,
    /// Spotify playlists created in this session, and when.
    created: Vec<(String, std::time::Instant)>,
}

/// "Create on Spotify", once the user said yes.
struct Creation {
    id: i64,
    name: String,
    /// Entries and the Spotify songs found for them.
    found: Vec<(i64, String)>,
}

/// How long to wait before trying again: what Spotify asked for (it can be
/// hours after many searches), at least a minute.
fn retry_delay(err: &Error) -> u32 {
    match err {
        Error::RateLimited(wait) => (wait.as_secs().min(6 * 3600) as u32).max(RETRY_DELAY_S),
        _ => RETRY_DELAY_S,
    }
}

/// A change waiting in the outbox; positions are Spotify's (§9.6).
///
/// Positions only hold for the snapshot they were made for, so adds and
/// moves also name the entry they follow: when the playlist changed
/// elsewhere meanwhile, that's where they go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    Add {
        uris: Vec<String>,
        position: Option<u32>,
        /// The Spotify entry before it (`None`: at the start, or old ops).
        after: Option<String>,
        /// Sent, but the answer got lost (timeout, reset): checked before
        /// sending it again, so it isn't added twice.
        sent: bool,
    },
    /// Spotify removes every occurrence; `restore` puts the others back
    /// (URI and final position, ascending).
    Remove {
        uris: Vec<String>,
        restore: Vec<(String, u32)>,
    },
    Move {
        start: u32,
        before: u32,
        length: u32,
        /// The song moved and the entry it follows afterwards (`None`: at
        /// the start).
        uri: Option<String>,
        after: Option<String>,
    },
    Rename {
        name: String,
    },
    Replace {
        uris: Vec<String>,
    },
}

/// What to do with a change, given what Spotify has now.
enum Placed {
    Send(Op),
    /// Nothing to send: it's there already, or its song is gone.
    Done,
}

impl Op {
    /// Adds `uris` at `position` of `current`, the Spotify entries as they
    /// are with the addition.
    pub fn add(uris: Vec<String>, position: Option<u32>, current: &[String]) -> Self {
        let after = position
            .and_then(|p| p.checked_sub(1))
            .and_then(|p| current.get(p as usize).cloned());
        Self::Add {
            uris,
            position,
            after,
            sent: false,
        }
    }

    /// The entry at `start` moved to `end`; `current` is the Spotify
    /// entries after the move.
    pub fn moved(start: u32, end: u32, current: &[String]) -> Self {
        Self::Move {
            start,
            // Spotify counts the place before the move.
            before: if end > start { end + 1 } else { end },
            length: 1,
            uri: current.get(end as usize).cloned(),
            after: end
                .checked_sub(1)
                .and_then(|p| current.get(p as usize).cloned()),
        }
    }

    pub fn to_json(&self) -> String {
        match self {
            Self::Add {
                uris,
                position,
                after,
                sent,
            } => {
                json!({"op": "add", "uris": uris, "position": position, "after": after, "sent": sent})
            }
            Self::Remove { uris, restore } => {
                json!({"op": "remove", "uris": uris, "restore": restore})
            }
            Self::Move {
                start,
                before,
                length,
                uri,
                after,
            } => json!({
                "op": "move", "start": start, "before": before, "length": length,
                "uri": uri, "after": after,
            }),
            Self::Rename { name } => json!({"op": "rename", "name": name}),
            Self::Replace { uris } => json!({"op": "replace", "uris": uris}),
        }
        .to_string()
    }

    fn parse(text: &str) -> Option<Self> {
        let value: Value = serde_json::from_str(text).ok()?;
        let uris = || -> Vec<String> {
            value["uris"]
                .as_array()
                .map(|uris| {
                    uris.iter()
                        .filter_map(|u| u.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default()
        };
        let number = |key: &str| value[key].as_u64().map(|n| n as u32);
        let text = |key: &str| value[key].as_str().map(str::to_owned);
        Some(match value["op"].as_str()? {
            "add" => Self::Add {
                uris: uris(),
                position: number("position"),
                after: text("after"),
                sent: value["sent"].as_bool().unwrap_or(false),
            },
            "remove" => Self::Remove {
                uris: uris(),
                restore: value["restore"]
                    .as_array()
                    .map(|list| {
                        list.iter()
                            .filter_map(|pair| {
                                Some((pair[0].as_str()?.to_owned(), pair[1].as_u64()? as u32))
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            },
            "move" => Self::Move {
                start: number("start")?,
                before: number("before")?,
                length: number("length").unwrap_or(1),
                uri: text("uri"),
                after: text("after"),
            },
            "rename" => Self::Rename {
                name: value["name"].as_str()?.to_owned(),
            },
            "replace" => Self::Replace { uris: uris() },
            _ => return None,
        })
    }

    /// Depends on where Spotify's entries are.
    fn is_positional(&self) -> bool {
        match self {
            Self::Add { position, .. } => position.is_some(),
            Self::Remove { restore, .. } => !restore.is_empty(),
            Self::Move { .. } => true,
            Self::Rename { .. } | Self::Replace { .. } => false,
        }
    }

    /// The change for `current` (Spotify's entries now, which may differ
    /// from the snapshot the change was made for).
    fn place(self, current: &[String]) -> Placed {
        match self {
            Self::Add {
                uris,
                position,
                after,
                sent,
            } => {
                let at = locate(current, position, after.as_deref());
                let there = current.get(at..at + uris.len()) == Some(uris.as_slice())
                    || (position.is_none() && current.ends_with(&uris));
                if sent && there {
                    return Placed::Done;
                }
                Placed::Send(Self::Add {
                    uris,
                    position: position.map(|_| at as u32),
                    after,
                    sent: false,
                })
            }
            Self::Remove { uris, restore } => {
                // Spotify removes by URI; the copies put back go where
                // they fit now.
                let left = current.iter().filter(|u| !uris.contains(u)).count();
                let restore = restore
                    .into_iter()
                    .enumerate()
                    .map(|(i, (uri, p))| (uri, p.min((left + i) as u32)))
                    .collect();
                Placed::Send(Self::Remove { uris, restore })
            }
            Self::Move {
                start,
                before,
                length: 1,
                uri: Some(uri),
                after,
            } => {
                let Some(from) = nearest(current, &uri, start as usize) else {
                    // Removed elsewhere.
                    return Placed::Done;
                };
                let mut rest = current.to_vec();
                rest.remove(from);
                let wanted = if before > start { before - 1 } else { before } as usize;
                let to = match &after {
                    None => 0,
                    Some(after) => nearest(&rest, after, wanted.saturating_sub(1))
                        .map_or(wanted.min(rest.len()), |i| i + 1),
                };
                if to == from {
                    return Placed::Done;
                }
                Placed::Send(Self::Move {
                    start: from as u32,
                    before: if to > from { to + 1 } else { to } as u32,
                    length: 1,
                    uri: Some(uri),
                    after,
                })
            }
            op => Placed::Send(op),
        }
    }

    /// `current` as Spotify has it after this change.
    fn record(&self, current: &mut Vec<String>) {
        match self {
            Self::Add { uris, position, .. } => {
                let at = position.map_or(current.len(), |p| (p as usize).min(current.len()));
                current.splice(at..at, uris.iter().cloned());
            }
            Self::Remove { uris, restore } => {
                current.retain(|u| !uris.contains(u));
                for (uri, p) in restore {
                    let at = (*p as usize).min(current.len());
                    current.insert(at, uri.clone());
                }
            }
            Self::Move {
                start,
                before,
                length,
                ..
            } => {
                let (start, length) = (*start as usize, *length as usize);
                if start + length > current.len() {
                    return;
                }
                let moved: Vec<String> = current.drain(start..start + length).collect();
                let before = *before as usize;
                let at = if before > start {
                    before - length
                } else {
                    before
                };
                let at = at.min(current.len());
                current.splice(at..at, moved);
            }
            Self::Replace { uris } => *current = uris.clone(),
            Self::Rename { .. } => {}
        }
    }

    /// Sends the change; the new snapshot, if the call makes one.
    fn apply(
        &self,
        client: &Client,
        id: &str,
        snapshot: Option<&str>,
    ) -> gatefold_spotify::Result<Option<String>> {
        match self {
            Self::Add { uris, position, .. } if !uris.is_empty() => {
                client.add_items(id, uris, *position).map(Some)
            }
            Self::Remove { uris, restore } if !uris.is_empty() => {
                let mut snapshot = client.remove_items(id, uris, snapshot)?;
                for (uri, position) in restore {
                    snapshot = client.add_items(id, std::slice::from_ref(uri), Some(*position))?;
                }
                Ok(Some(snapshot))
            }
            Self::Move {
                start,
                before,
                length,
                ..
            } => client
                .reorder(id, *start, *before, *length, snapshot)
                .map(Some),
            Self::Rename { name } => client.rename_playlist(id, name).map(|()| None),
            Self::Replace { uris } => client.replace_items(id, uris).map(Some),
            _ => Ok(None),
        }
    }
}

/// Where an addition goes in `current`: after the entry it followed (the
/// copy nearest its old place), else at its old place, else at the end.
fn locate(current: &[String], position: Option<u32>, after: Option<&str>) -> usize {
    match position {
        None => current.len(),
        Some(0) => 0,
        Some(p) => after
            .and_then(|after| nearest(current, after, p as usize - 1))
            .map_or((p as usize).min(current.len()), |i| i + 1),
    }
}

/// The occurrence of `uri` closest to `near`.
fn nearest(list: &[String], uri: &str, near: usize) -> Option<usize> {
    list.iter()
        .enumerate()
        .filter(|(_, u)| *u == uri)
        .map(|(i, _)| i)
        .min_by_key(|i| i.abs_diff(near))
}

/// What a job does.
enum Job {
    Full,
    Flush,
    Playlist(i64),
    Resolve(i64, bool),
    Create(Creation),
}

fn remote_track(track: &gatefold_spotify::Track) -> RemoteTrack {
    RemoteTrack {
        title: track.name.clone(),
        artists: track.artists.clone(),
        album: track.album.clone(),
        duration_ms: track.duration_ms,
        isrc: track.isrc.clone(),
    }
}

fn remote_entries(entries: &[PlaylistEntry]) -> Vec<RemoteEntry> {
    entries
        .iter()
        .map(|entry| RemoteEntry {
            uri: entry.uri.clone(),
            image_url: entry.track.as_ref().and_then(|t| t.image_url.clone()),
            track: entry.track.as_ref().map(remote_track),
            added: entry.added_at,
        })
        .collect()
}

/// A library song, as matching sees it.
pub fn local_track(track: &Track) -> LocalTrack {
    let tags = &track.record.tags;
    LocalTrack {
        track_id: track.record.id,
        title: tags.title.clone().unwrap_or_default(),
        artist: tags.artist.clone(),
        album_artist: track.album_artist.clone().or(tags.album_artist.clone()),
        album: track.album_title.clone().or(tags.album.clone()),
        duration_ms: tags.duration_ms,
        isrc: tags.isrc.clone(),
        format: tags.format.clone(),
    }
}

/// (URI, title, artist) of a stored suggestion.
pub fn parse_suggestion(text: &str) -> Option<(String, String, String)> {
    let value: Value = serde_json::from_str(text).ok()?;
    Some((
        value["uri"].as_str()?.to_owned(),
        value["title"].as_str().unwrap_or_default().to_owned(),
        value["artist"].as_str().unwrap_or_default().to_owned(),
    ))
}

impl Spotify {
    fn set_syncing(&self, syncing: bool) {
        if self.imp().syncing.replace(syncing) != syncing {
            self.notify_syncing();
        }
    }

    /// Reads the user's playlists (after connecting, on start, "Refresh").
    pub fn sync_soon(&self) {
        self.imp().sync.borrow_mut().full = true;
        self.run_jobs();
    }

    /// A playlist was opened: send its changes and read it if it changed.
    pub fn refresh_playlist(&self, id: i64) {
        let mut state = self.imp().sync.borrow_mut();
        if !state.playlists.contains(&id) {
            state.playlists.push(id);
        }
        drop(state);
        self.run_jobs();
    }

    /// Library songs were added to a linked playlist: find them on
    /// Spotify (one search at a time), then add them there.
    pub fn resolve_soon(&self, id: i64, announce: bool) {
        let mut state = self.imp().sync.borrow_mut();
        match state.resolve.iter_mut().find(|(p, _)| *p == id) {
            Some(entry) => entry.1 |= announce,
            None => state.resolve.push((id, announce)),
        }
        drop(state);
        self.run_jobs();
    }

    /// Local edits are in the outbox: send them shortly.
    pub fn changes_queued(&self) {
        let mut state = self.imp().sync.borrow_mut();
        if let Some(timer) = state.flush_timer.take() {
            timer.remove();
        }
        state.flush_timer = Some(glib::timeout_add_local_once(
            FLUSH_DELAY,
            glib::clone!(
                #[weak(rename_to = spotify)]
                self,
                move || {
                    let mut state = spotify.imp().sync.borrow_mut();
                    state.flush_timer = None;
                    state.flush = true;
                    drop(state);
                    spotify.run_jobs();
                }
            ),
        ));
    }

    /// While the user reorders a playlist, its changes wait; released,
    /// they go out shortly.
    pub fn hold_changes(&self, id: i64, hold: bool) {
        let mut state = self.imp().sync.borrow_mut();
        let changed = if hold {
            state.held.insert(id)
        } else {
            state.held.remove(&id)
        };
        drop(state);
        if changed && !hold {
            self.changes_queued();
        }
    }

    /// Changes to this playlist are on their way to Spotify: no new ones
    /// until Spotify confirmed them.
    pub fn is_sending(&self, id: i64) -> bool {
        self.sending() == id
    }

    fn set_sending(&self, id: i64) {
        if self.imp().sending.replace(id) != id {
            self.notify_sending();
        }
    }

    fn run_jobs(&self) {
        if !self.is_connected() || self.imp().sync.borrow().running {
            return;
        }
        self.imp().sync.borrow_mut().running = true;
        self.set_syncing(true);
        glib::spawn_future_local(glib::clone!(
            #[weak(rename_to = spotify)]
            self,
            async move {
                loop {
                    let job = {
                        let mut state = spotify.imp().sync.borrow_mut();
                        if !state.create.is_empty() {
                            Some(Job::Create(state.create.remove(0)))
                        } else if std::mem::take(&mut state.full) {
                            state.flush = false;
                            state.playlists.clear();
                            Some(Job::Full)
                        } else if std::mem::take(&mut state.flush) {
                            Some(Job::Flush)
                        } else if !state.playlists.is_empty() {
                            Some(Job::Playlist(state.playlists.remove(0)))
                        } else if !state.resolve.is_empty() {
                            let (id, announce) = state.resolve.remove(0);
                            Some(Job::Resolve(id, announce))
                        } else {
                            None
                        }
                    };
                    let Some(job) = job else { break };
                    if !spotify.is_connected() {
                        break;
                    }
                    match job {
                        Job::Full => spotify.sync_all().await,
                        Job::Flush => spotify.flush_all().await,
                        Job::Playlist(id) => spotify.sync_playlist(id).await,
                        Job::Resolve(id, announce) => {
                            spotify.resolve_entries(id, announce).await;
                        }
                        Job::Create(creation) => spotify.create(creation).await,
                    }
                }
                spotify.imp().sync.borrow_mut().running = false;
                spotify.set_syncing(false);
            }
        ));
    }

    /// Searches failed for a temporary reason: try the playlist again later.
    fn retry_resolve_later(&self, id: i64, err: &Error) {
        if !self.imp().sync.borrow_mut().resolve_retry.insert(id) {
            return;
        }
        glib::timeout_add_seconds_local_once(
            retry_delay(err),
            glib::clone!(
                #[weak(rename_to = spotify)]
                self,
                move || {
                    spotify.imp().sync.borrow_mut().resolve_retry.remove(&id);
                    spotify.resolve_soon(id, false);
                }
            ),
        );
    }

    /// Reading playlists stopped (offline, too many requests): try again.
    fn retry_later_full(&self, err: &Error) {
        if std::mem::replace(&mut self.imp().sync.borrow_mut().full_retry, true) {
            return;
        }
        glib::timeout_add_seconds_local_once(
            retry_delay(err),
            glib::clone!(
                #[weak(rename_to = spotify)]
                self,
                move || {
                    spotify.imp().sync.borrow_mut().full_retry = false;
                    spotify.sync_soon();
                }
            ),
        );
    }

    fn retry_later(&self, err: &Error) {
        let mut state = self.imp().sync.borrow_mut();
        if state.retry_timer.is_some() {
            return;
        }
        state.retry_timer = Some(glib::timeout_add_seconds_local_once(
            retry_delay(err),
            glib::clone!(
                #[weak(rename_to = spotify)]
                self,
                move || {
                    spotify.imp().sync.borrow_mut().retry_timer = None;
                    spotify.imp().sync.borrow_mut().flush = true;
                    spotify.run_jobs();
                }
            ),
        ));
    }

    /// Everything: waiting changes, the list, changed playlists, covers,
    /// and library songs added to linked playlists.
    async fn sync_all(&self) {
        self.flush_all().await;
        let list = match self.call(|client| client.my_playlists()).await {
            Ok(list) => list,
            Err(err) => {
                tracing::warn!("Cannot read the Spotify playlists: {err}");
                // The whole sync again (playlists, saved albums, artists),
                // not only the waiting changes.
                if err.is_temporary() {
                    self.retry_later_full(&err);
                }
                return;
            }
        };
        // Disconnected while asking: nothing of that account comes back.
        if !self.is_connected() {
            return;
        }
        let user = self.user_id().unwrap_or_default();
        let show_mirrors = self.settings().boolean("spotify-playlists");
        let library = GatefoldApplication::get().library();
        let mut to_read = Vec::new();
        {
            let Some(mut db) = library.db() else { return };
            let linked: HashMap<String, LinkedPlaylist> = db
                .linked_playlists()
                .unwrap_or_default()
                .into_iter()
                .map(|p| (p.spotify_id.clone(), p))
                .collect();
            let mut keep = Vec::new();
            for summary in &list {
                let existing = linked.get(&summary.id);
                let published = existing.is_some_and(|p| p.source == "local");
                if !show_mirrors && !published {
                    continue;
                }
                let editable = summary.editable_by(&user);
                let info = SpotifyPlaylistInfo {
                    spotify_id: summary.id.clone(),
                    name: summary.name.clone(),
                    owner: summary.owner_name.clone(),
                    editable,
                    readable: editable || summary.total.is_some(),
                    image_url: summary.image_url.clone(),
                };
                match db.upsert_spotify_playlist(&info, now()) {
                    Ok(id) => {
                        keep.push(summary.id.clone());
                        // Changes still waiting (held, or Spotify was
                        // unreachable): read after they're sent, else
                        // reading would undo them here.
                        let waiting = existing.is_some_and(|p| p.pending_changes > 0);
                        let stale = !waiting
                            && existing.is_none_or(|p| {
                                p.snapshot_id.as_deref() != Some(&summary.snapshot_id)
                            });
                        // Spotify refused before: don't ask again.
                        let readable = existing.map_or(info.readable, |p| p.readable || editable);
                        if readable && stale {
                            to_read.push((id, summary.id.clone(), summary.snapshot_id.clone()));
                        }
                    }
                    Err(err) => tracing::warn!("Cannot store a Spotify playlist: {err}"),
                }
            }
            // Just created here: Spotify's list may not have it yet.
            {
                let mut state = self.imp().sync.borrow_mut();
                state
                    .created
                    .retain(|(_, when)| when.elapsed() < NEW_PLAYLIST_GRACE);
                keep.extend(state.created.iter().map(|(id, _)| id.clone()));
            }
            if let Err(err) = db.forget_spotify_playlists(&keep) {
                tracing::warn!("Cannot remove old Spotify playlists: {err}");
            }
        }
        library.reload_playlists();
        for (id, spotify_id, snapshot) in to_read {
            if !self.is_connected() {
                return;
            }
            match self
                .read_items(id, &spotify_id, Some(snapshot.clone()), false)
                .await
            {
                Ok(_) => {}
                // Development Mode: only own and collaborative playlists
                // show their entries (§9.5).
                Err(Error::Status { status: 403, .. }) => {
                    tracing::info!("Spotify doesn't show the songs of playlist {spotify_id}");
                    if let Some(mut db) = library.db() {
                        let _ = db.mark_unreadable(id, &snapshot);
                    }
                }
                Err(err) => {
                    tracing::warn!("Cannot read a Spotify playlist: {err}");
                    // Too many requests or offline: the rest waits.
                    if err.is_temporary() {
                        self.retry_later_full(&err);
                        break;
                    }
                }
            }
        }
        library.reload_playlists();
        self.load_covers().await;
        let editable: Vec<i64> = library
            .db()
            .and_then(|db| db.linked_playlists().ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|p| p.editable)
            .map(|p| p.id)
            .collect();
        for id in editable {
            self.resolve_entries(id, false).await;
        }
        self.sync_library().await;
    }

    /// One playlist: its changes, then a re-read if Spotify's changed.
    async fn sync_playlist(&self, id: i64) {
        self.flush(id).await;
        let library = GatefoldApplication::get().library();
        let Some(linked) = library
            .db()
            .and_then(|db| db.linked_playlist(id).ok().flatten())
        else {
            return;
        };
        if !linked.readable {
            return;
        }
        let spotify_id = linked.spotify_id.clone();
        let summary = match self.call(move |client| client.playlist(&spotify_id)).await {
            Ok(summary) => summary,
            Err(err) => {
                tracing::info!("Cannot check a Spotify playlist: {err}");
                return;
            }
        };
        let waiting = library
            .db()
            .is_some_and(|db| !db.spotify_ops(id).unwrap_or_default().is_empty());
        if !waiting
            && linked.snapshot_id.as_deref() != Some(&summary.snapshot_id)
            && let Err(err) = self
                .read_items(id, &linked.spotify_id, Some(summary.snapshot_id), false)
                .await
        {
            tracing::warn!("Cannot read a Spotify playlist: {err}");
        }
        if linked.editable {
            self.resolve_entries(id, false).await;
        }
    }

    /// Reads a playlist's entries into its mirror. Returns whether the
    /// result differs from what the mirror expected.
    async fn read_items(
        &self,
        id: i64,
        spotify_id: &str,
        snapshot: Option<String>,
        compare: bool,
    ) -> gatefold_spotify::Result<bool> {
        let sid = spotify_id.to_owned();
        let (entries, snapshot) = self
            .call(move |client| {
                let entries = client.playlist_items(&sid)?;
                let snapshot = match snapshot {
                    Some(snapshot) => snapshot,
                    None => client.playlist(&sid)?.snapshot_id,
                };
                Ok((entries, snapshot))
            })
            .await?;
        let library = GatefoldApplication::get().library();
        let index = library.match_index();
        let Some(mut db) = library.db() else {
            return Ok(false);
        };
        let remote: Vec<String> = entries.iter().filter_map(|e| e.uri.clone()).collect();
        let differs = compare && db.spotify_uris(id).ok().as_ref() != Some(&remote);
        if compare && !differs {
            let _ = db.set_snapshot(id, &snapshot);
            return Ok(false);
        }
        if let Err(err) =
            db.set_spotify_items(id, &remote_entries(&entries), &index, &snapshot, now())
        {
            tracing::warn!("Cannot store a Spotify playlist: {err}");
        }
        drop(db);
        library.reload_playlists();
        Ok(differs)
    }

    async fn flush_all(&self) {
        let ids = GatefoldApplication::get()
            .library()
            .db()
            .and_then(|db| db.playlists_with_ops().ok())
            .unwrap_or_default();
        for id in ids {
            self.flush(id).await;
        }
    }

    /// Sends a playlist's waiting changes, then checks the result against
    /// Spotify: if someone changed it elsewhere meanwhile, Spotify's version
    /// wins and changes Spotify refused are applied again by URI (§9.6).
    /// The playlist can't be edited until that's done.
    async fn flush(&self, id: i64) {
        if self.imp().sync.borrow().held.contains(&id) {
            return;
        }
        self.set_sending(id);
        self.send_changes(id).await;
        self.set_sending(0);
    }

    async fn send_changes(&self, id: i64) {
        let library = GatefoldApplication::get().library();
        let (linked, ops) = {
            let Some(db) = library.db() else { return };
            let Some(linked) = db.linked_playlist(id).ok().flatten() else {
                return;
            };
            (linked, db.spotify_ops(id).unwrap_or_default())
        };
        let Some(last_op) = ops.last().map(|(op_id, _)| *op_id) else {
            return;
        };
        let drop_ops = |op_ids: &[i64]| {
            if let Some(mut db) = library.db() {
                for op_id in op_ids {
                    let _ = db.remove_spotify_op(*op_id);
                }
            }
        };
        let mut snapshot = linked.snapshot_id.clone();
        let mut refused = Vec::new();
        let mut parsed = Vec::new();
        for (op_id, text) in ops {
            match Op::parse(&text) {
                Some(op) => parsed.push((vec![op_id], op)),
                None => drop_ops(&[op_id]),
            }
        }
        let merged = merge_adds(parsed);
        // Positions hold for the snapshot they were made for. If Spotify's
        // playlist changed since (elsewhere, or by an addition whose
        // answer got lost), its entries are read and the changes placed
        // by the songs they follow.
        let positional = merged.iter().any(|(_, op)| op.is_positional());
        let unsure = merged
            .iter()
            .any(|(_, op)| matches!(op, Op::Add { sent: true, .. }));
        let mut current: Option<Vec<String>> = None;
        if positional || unsure {
            let (sid, known) = (linked.spotify_id.clone(), snapshot.clone());
            let checked = self
                .call(move |client| {
                    let now = client.playlist(&sid)?.snapshot_id;
                    if !unsure && known.as_deref() == Some(now.as_str()) {
                        return Ok((now, None));
                    }
                    let entries = client.playlist_items(&sid)?;
                    Ok((
                        now,
                        Some(entries.into_iter().filter_map(|e| e.uri).collect()),
                    ))
                })
                .await;
            match checked {
                Ok((now, entries)) => {
                    if entries.is_some() {
                        tracing::info!(
                            "Spotify playlist {} changed; placing changes by song",
                            linked.spotify_id
                        );
                    }
                    snapshot = Some(now);
                    current = entries;
                }
                Err(Error::LoggedOut) => return,
                Err(err) if err.is_temporary() || !refused_change(&err) => {
                    tracing::info!("Spotify is unreachable, changes wait: {err}");
                    self.retry_later(&err);
                    return;
                }
                // Refused (gone?): the next full sync sorts it out.
                Err(err) => tracing::warn!("Cannot check a Spotify playlist: {err}"),
            }
        }
        for (op_ids, op) in merged {
            let op = match &current {
                Some(entries) => match op.place(entries) {
                    Placed::Send(op) => op,
                    Placed::Done => {
                        drop_ops(&op_ids);
                        continue;
                    }
                },
                None => op,
            };
            let (sid, snap, job) = (linked.spotify_id.clone(), snapshot.clone(), op.clone());
            match self
                .call(move |client| job.apply(client, &sid, snap.as_deref()))
                .await
            {
                Ok(new_snapshot) => {
                    if new_snapshot.is_some() {
                        snapshot = new_snapshot;
                    }
                    if let Some(entries) = &mut current {
                        op.record(entries);
                    }
                    drop_ops(&op_ids);
                }
                Err(Error::LoggedOut) => return,
                // Only Spotify refusing this change drops it; a problem
                // with the login (e.g. renewing it failed) keeps it waiting.
                Err(err) if err.is_temporary() || !refused_change(&err) => {
                    tracing::info!("Spotify is unreachable, changes wait: {err}");
                    if let Some(mut db) = library.db() {
                        if let Some(snapshot) = &snapshot {
                            let _ = db.set_snapshot(id, snapshot);
                        }
                        // It may have arrived without the answer: checked
                        // before it's sent again.
                        if let Op::Add {
                            uris,
                            position,
                            after,
                            ..
                        } = &op
                            && answer_lost(&err)
                            && let Some((first, rest)) = op_ids.split_first()
                        {
                            let unsure = Op::Add {
                                uris: uris.clone(),
                                position: *position,
                                after: after.clone(),
                                sent: true,
                            };
                            let _ = db.update_spotify_op(*first, &unsure.to_json());
                            for op_id in rest {
                                let _ = db.remove_spotify_op(*op_id);
                            }
                        }
                    }
                    library.reload_playlists();
                    self.retry_later(&err);
                    return;
                }
                Err(err) => {
                    tracing::warn!("Spotify refused a playlist change: {err}");
                    refused.push(op);
                    drop_ops(&op_ids);
                }
            }
        }
        if !refused.is_empty() {
            self.reapply_by_uri(&linked.spotify_id, refused).await;
        }
        // Newer changes came in meanwhile: comparing now would take them
        // for changes made elsewhere. They're sent (and compared) next.
        let newer = library.db().is_some_and(|db| {
            db.spotify_ops(id)
                .unwrap_or_default()
                .iter()
                .any(|(op_id, _)| *op_id > last_op)
        });
        if newer {
            if let (Some(mut db), Some(snapshot)) = (library.db(), &snapshot) {
                let _ = db.set_snapshot(id, snapshot);
            }
            library.reload_playlists();
            return;
        }
        match self.read_items(id, &linked.spotify_id, None, true).await {
            Ok(true) => {
                let name = library
                    .playlist(id)
                    .map(|p| p.playlist().name.clone())
                    .unwrap_or_default();
                // Translators: {} is a playlist name.
                self.message(&gettext("“{}” was changed elsewhere — merged").replace("{}", &name));
            }
            Ok(false) => library.reload_playlists(),
            Err(err) => tracing::info!("Cannot check a playlist after changing it: {err}"),
        }
    }

    /// Changes Spotify refused by position (it changed meanwhile), applied
    /// to what it has now.
    async fn reapply_by_uri(&self, spotify_id: &str, ops: Vec<Op>) {
        let sid = spotify_id.to_owned();
        let result = self
            .call(move |client| {
                let current: Vec<String> = client
                    .playlist_items(&sid)?
                    .into_iter()
                    .filter_map(|e| e.uri)
                    .collect();
                for op in ops {
                    match op {
                        Op::Add { uris, .. } => {
                            let missing: Vec<String> =
                                uris.into_iter().filter(|u| !current.contains(u)).collect();
                            if !missing.is_empty() {
                                client.add_items(&sid, &missing, None)?;
                            }
                        }
                        // Spotify removes every copy of a song; the copies the
                        // change kept (`restore`) are put back, also when it
                        // was the putting back that failed.
                        Op::Remove { uris, restore } => {
                            let mut seen = std::collections::HashSet::new();
                            for uri in uris.iter().filter(|u| seen.insert(u.as_str())) {
                                let present = current.iter().filter(|u| *u == uri).count();
                                let keep = restore.iter().filter(|(u, _)| u == uri).count();
                                let one = std::slice::from_ref(uri);
                                if present > keep {
                                    client.remove_items(&sid, one, None)?;
                                    for _ in 0..keep {
                                        client.add_items(&sid, one, None)?;
                                    }
                                } else {
                                    for _ in present..keep {
                                        client.add_items(&sid, one, None)?;
                                    }
                                }
                            }
                        }
                        Op::Rename { name } => client.rename_playlist(&sid, &name)?,
                        // The order changed elsewhere; theirs stays.
                        Op::Move { .. } | Op::Replace { .. } => {}
                    }
                }
                Ok(())
            })
            .await;
        if let Err(err) = result {
            tracing::warn!("Cannot apply playlist changes again: {err}");
        }
    }

    /// Spotify playlists' covers, downloaded when Spotify's image changed.
    async fn load_covers(&self) {
        let library = GatefoldApplication::get().library();
        let wanted: Vec<(i64, String, String)> = library
            .db()
            .and_then(|db| db.linked_playlists().ok())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|p| {
                let url = p.image_url?;
                let key = gatefold_core::library::covers::cover_key('s', url.as_bytes());
                (p.cover_key.as_deref() != Some(&key)).then_some((p.id, url, key))
            })
            .collect();
        if wanted.is_empty() {
            return;
        }
        for (id, url, key) in wanted {
            let job_key = key.clone();
            let made = gtk::gio::spawn_blocking(move || {
                let covers =
                    gatefold_core::library::covers::CoverCache::new(crate::config::cover_dir());
                if covers.has(&job_key) {
                    return true;
                }
                match gatefold_spotify::download_image(&url) {
                    Ok(data) => covers.generate(&job_key, data).is_ok(),
                    Err(err) => {
                        tracing::info!("Cannot load a playlist cover: {err}");
                        false
                    }
                }
            })
            .await
            .unwrap_or(false);
            if made && let Some(mut db) = library.db() {
                let _ = db.set_spotify_cover(id, Some(&key));
            }
        }
        library.reload_playlists();
    }

    /// The Spotify version of a library song: from the cache, another
    /// playlist, or a search (ISRC first, then title and artist).
    /// An error means Spotify couldn't be asked (offline, too many
    /// requests); nothing is remembered then, so it's tried again later.
    pub async fn find_version(&self, track_id: i64) -> gatefold_spotify::Result<SpotifyVersion> {
        let library = GatefoldApplication::get().library();
        // A Spotify song already (one to stream).
        if track_id < 0 {
            let uri = library
                .db()
                .and_then(|db| db.stream_uri(track_id).ok().flatten());
            return Ok(SpotifyVersion {
                uri,
                confidence: Some(Confidence::Sure),
                title: None,
                artist: None,
                checked: now(),
            });
        }
        let not_found = || SpotifyVersion {
            uri: None,
            confidence: None,
            title: None,
            artist: None,
            checked: now(),
        };
        {
            let Some(db) = library.db() else {
                return Ok(not_found());
            };
            if let Some(version) = db.spotify_version(track_id).ok().flatten()
                && (version.uri.is_some() || now() - version.checked < NOT_FOUND_TTL_S)
            {
                return Ok(version);
            }
            if let Some(uri) = db.known_uri(track_id).ok().flatten() {
                return Ok(SpotifyVersion {
                    uri: Some(uri),
                    confidence: Some(Confidence::Sure),
                    title: None,
                    artist: None,
                    checked: now(),
                });
            }
        }
        let Some(track) = library.tracks(&[track_id]).remove(&track_id) else {
            return Ok(not_found());
        };
        let local = local_track(&track);
        let isrc = local.isrc.clone();
        let queries = search_queries(&local);
        let title = local.title.clone();
        let aliases = library.aliases().clone();
        let wanted = local.clone();
        let results = self
            .call(move |client| {
                // Too many requests: wait as long as Spotify asks (on this
                // worker thread), once.
                let search = |query: &str, limit: u32| match client.search_tracks(query, limit) {
                    Err(Error::RateLimited(wait)) if wait <= MAX_SEARCH_WAIT => {
                        tracing::info!("Spotify asks to wait {wait:?} before searching");
                        std::thread::sleep(wait);
                        client.search_tracks(query, limit)
                    }
                    result => result,
                };
                // From the most exact query to the most forgiving; stop at
                // a sure match.
                let mut results = Vec::new();
                let isrc_query = isrc.map(|isrc| (format!("isrc:{isrc}"), 5));
                for (query, limit) in isrc_query
                    .into_iter()
                    .chain(queries.into_iter().map(|q| (q, 10)))
                {
                    let found = search(&query, limit)?;
                    tracing::debug!("Spotify search {query:?}: {} results", found.len());
                    std::thread::sleep(SEARCH_PAUSE);
                    let sure = found.iter().any(|t| {
                        matching::compare(&wanted, &remote_track(t), &aliases)
                            == Some(Confidence::Sure)
                    });
                    results.extend(found);
                    if sure {
                        break;
                    }
                }
                Ok(results)
            })
            .await;
        let results = match results {
            Ok(results) => results,
            Err(err) => {
                tracing::warn!("Cannot search Spotify for “{title}”: {err}");
                return Err(err);
            }
        };
        tracing::debug!("Spotify search for “{title}”: {} results", results.len());
        let aliases = library.aliases().clone();
        let best = results
            .iter()
            .filter(|t| t.is_playable && !t.is_local)
            .filter_map(|t| {
                let confidence = matching::compare(&local, &remote_track(t), &aliases)?;
                Some((
                    confidence,
                    std::cmp::Reverse(t.duration_ms.abs_diff(local.duration_ms)),
                    t,
                ))
            })
            .max_by_key(|(confidence, closeness, _)| (*confidence, *closeness))
            .map(|(confidence, _, t)| (confidence, t.clone()));
        let version = SpotifyVersion {
            uri: best.as_ref().map(|(_, t)| t.uri.clone()),
            confidence: best.as_ref().map(|(c, _)| *c),
            title: best.as_ref().map(|(_, t)| t.name.clone()),
            artist: best.as_ref().map(|(_, t)| t.artist_line()),
            checked: now(),
        };
        if let Some(mut db) = library.db() {
            let _ = db.set_spotify_version(track_id, &version);
        }
        Ok(version)
    }

    /// Library songs in a linked playlist get their Spotify version: sure
    /// ones are added on Spotify, likely ones are suggested. Returns how
    /// many aren't on Spotify.
    async fn resolve_entries(&self, id: i64, announce: bool) -> usize {
        let library = GatefoldApplication::get().library();
        let entries = library
            .db()
            .and_then(|db| db.items_without_uri(id).ok())
            .unwrap_or_default();
        let mut added = 0;
        let mut missing = 0;
        let mut failed = None;
        for (item, track) in entries {
            let version = match self.find_version(track).await {
                Ok(version) => version,
                // A song that isn't there is an empty result. An error
                // about this song's query: the next one. Any other error
                // (offline, too many requests, Spotify refusing searches)
                // holds for the other songs too, and asking for each of
                // them anyway can get Gatefold blocked for hours.
                Err(err) => {
                    let stop = !err.is_about_request();
                    failed = Some(err);
                    if stop {
                        break;
                    }
                    continue;
                }
            };
            let Some(mut db) = library.db() else { break };
            match (version.uri, version.confidence) {
                (Some(uri), Some(Confidence::Sure)) => {
                    if db.set_item_spotify(item, &uri).is_ok()
                        && let Ok(Some(position)) = db.remote_position(item)
                    {
                        let current = db.spotify_uris(id).unwrap_or_default();
                        let op = Op::add(vec![uri], Some(position), &current);
                        let _ = db.queue_spotify_op(id, &op.to_json(), now());
                        added += 1;
                    }
                }
                (Some(uri), Some(Confidence::Likely)) => {
                    let suggestion = json!({
                        "uri": uri,
                        "title": version.title,
                        "artist": version.artist,
                    })
                    .to_string();
                    let _ = db.set_item_suggestion(item, Some(&suggestion));
                }
                _ => missing += 1,
            }
        }
        library.reload_playlists();
        if added > 0 {
            self.changes_queued();
        }
        if let Some(err) = failed {
            if announce {
                self.message(&search_error(&err));
            }
            if err.is_temporary() {
                self.retry_resolve_later(id, &err);
            }
        } else if announce && missing > 0 {
            self.message(
                &ngettext(
                    "{} song isn't on Spotify; it stays in Gatefold only",
                    "{} songs aren't on Spotify; they stay in Gatefold only",
                    missing as u32,
                )
                .replace("{}", &missing.to_string()),
            );
        }
        missing
    }

    /// "Create on Spotify" (§9.6): finds the songs, shows what will be
    /// missing, then creates the Spotify playlist and links it.
    pub async fn publish(&self, id: i64, parent: &impl IsA<gtk::Widget>) {
        // Once at a time per playlist (two quick clicks).
        if !self.imp().sync.borrow_mut().publishing.insert(id) {
            return;
        }
        if !self.ask_to_publish(id, parent).await {
            self.imp().sync.borrow_mut().publishing.remove(&id);
        }
    }

    /// Finds the songs and asks; true when the creation is queued.
    async fn ask_to_publish(&self, id: i64, parent: &impl IsA<gtk::Widget>) -> bool {
        let library = GatefoldApplication::get().library();
        let Some(playlist) = library.playlist(id) else {
            return false;
        };
        if playlist.playlist().spotify_id.is_some() {
            return false;
        }
        let name = playlist.playlist().name.clone();
        let entries = library
            .db()
            .and_then(|db| db.items_without_uri(id).ok())
            .unwrap_or_default();
        self.message(&gettext("Finding the songs on Spotify…"));
        self.set_syncing(true);
        let mut sure = Vec::new();
        let mut likely = Vec::new();
        let mut missing = Vec::new();
        let mut unchecked = Vec::new();
        let mut failure = None;
        for (item, track) in &entries {
            match self.find_version(*track).await {
                Ok(SpotifyVersion {
                    uri: Some(uri),
                    confidence: Some(Confidence::Sure),
                    ..
                }) => sure.push((*item, uri)),
                Ok(SpotifyVersion {
                    uri: Some(uri),
                    title,
                    artist,
                    ..
                }) => likely.push((*item, *track, uri, title, artist)),
                Ok(_) => missing.push(*track),
                Err(err) => {
                    unchecked.push(*track);
                    // Unless it's about this song's query, the rest waits
                    // (see resolve_entries).
                    let stop = !err.is_about_request();
                    failure = Some(err);
                    if !stop {
                        continue;
                    }
                    for (_, rest) in entries.iter().skip_while(|(i, _)| i != item).skip(1) {
                        unchecked.push(*rest);
                    }
                    break;
                }
            }
        }
        self.set_syncing(self.imp().sync.borrow().running);
        let tracks = library.tracks(&entries.iter().map(|(_, t)| *t).collect::<Vec<_>>());
        let title_of = |track: i64| {
            tracks
                .get(&track)
                .map(|t| {
                    let title = t.record.tags.title.clone().unwrap_or_default();
                    match t
                        .record
                        .tags
                        .artist
                        .as_deref()
                        .or(t.album_artist.as_deref())
                    {
                        Some(artist) => format!("{title} — {artist}"),
                        None => title,
                    }
                })
                .unwrap_or_default()
        };

        let dialog = adw::AlertDialog::builder()
            // Translators: {} is a playlist name.
            .heading(gettext("Create “{}” on Spotify?").replace("{}", &name))
            .body(
                ngettext(
                    "{found} of {total} song was found on Spotify.",
                    "{found} of {total} songs were found on Spotify.",
                    entries.len() as u32,
                )
                .replace("{found}", &sure.len().to_string())
                .replace("{total}", &entries.len().to_string()),
            )
            .close_response("cancel")
            .default_response("create")
            .build();
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .build();
        let mut checks = Vec::new();
        if !likely.is_empty() {
            content.append(&small_heading(&gettext("Probably the Same Song")));
            let list = boxed_list();
            for (_, track, _, title, artist) in &likely {
                let check = gtk::CheckButton::builder()
                    .valign(gtk::Align::Center)
                    .build();
                let row = adw::ActionRow::builder()
                    .use_markup(false)
                    .title(title_of(*track))
                    .subtitle(
                        // Translators: a Spotify search result, "Song — Artist".
                        gettext("On Spotify: {}").replace(
                            "{}",
                            &[title.clone(), artist.clone()]
                                .into_iter()
                                .flatten()
                                .collect::<Vec<_>>()
                                .join(" — "),
                        ),
                    )
                    .activatable_widget(&check)
                    .build();
                row.add_prefix(&check);
                list.append(&row);
                checks.push(check);
            }
            content.append(&list);
        }
        if let Some(err) = &failure {
            content.append(&small_heading(&gettext("Not Checked Yet")));
            content.append(
                &gtk::Label::builder()
                    .label(format!(
                        "{} {}",
                        search_error(err),
                        gettext("These songs stay in Gatefold for now and are added on Spotify once they're found.")
                    ))
                    .wrap(true)
                    .xalign(0.0)
                    .css_classes(["dim-label", "caption"])
                    .build(),
            );
            let list = boxed_list();
            for track in &unchecked {
                list.append(
                    &adw::ActionRow::builder()
                        .use_markup(false)
                        .title(title_of(*track))
                        .build(),
                );
            }
            content.append(&list);
        }
        if !missing.is_empty() {
            content.append(&small_heading(&gettext(
                "Not on Spotify — Stay in Gatefold Only",
            )));
            let list = boxed_list();
            for track in &missing {
                list.append(
                    &adw::ActionRow::builder()
                        .use_markup(false)
                        .title(title_of(*track))
                        .build(),
                );
            }
            content.append(&list);
        }
        if content.first_child().is_some() {
            let scrolled = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .propagate_natural_height(true)
                .max_content_height(320)
                .child(&content)
                .build();
            dialog.set_extra_child(Some(&scrolled));
        }
        dialog.add_responses(&[
            ("cancel", &gettext("_Cancel")),
            ("create", &gettext("C_reate")),
        ]);
        dialog.set_response_appearance("create", adw::ResponseAppearance::Suggested);
        // Development aid: headless tests can't click.
        let ask =
            !(cfg!(debug_assertions) && std::env::var_os("GATEFOLD_SCREENSHOT_PUBLISH").is_some());
        if ask && dialog.choose_future(Some(parent)).await != "create" {
            return false;
        }
        for (check, (item, _, uri, _, _)) in checks.iter().zip(&likely) {
            if check.is_active() {
                sure.push((*item, uri.clone()));
            }
        }
        // Created between the sync's other jobs: a sync that read the
        // list of playlists before can't unlink it.
        self.imp().sync.borrow_mut().create.push(Creation {
            id,
            name,
            found: sure,
        });
        self.run_jobs();
        true
    }

    /// Creates the Spotify playlist and links it (a sync job).
    async fn create(&self, creation: Creation) {
        let Creation { id, name, found } = creation;
        self.created_playlist(id, &name, &found).await;
        self.imp().sync.borrow_mut().publishing.remove(&id);
    }

    async fn created_playlist(&self, id: i64, name: &str, sure: &[(i64, String)]) {
        let library = GatefoldApplication::get().library();
        // Linked meanwhile, or deleted.
        if library
            .playlist(id)
            .is_none_or(|p| p.playlist().spotify_id.is_some())
        {
            return;
        }
        let playlist_name = name.to_owned();
        let created = self
            .call(move |client| client.create_playlist(&playlist_name, "Made with Gatefold"))
            .await;
        let summary = match created {
            Ok(summary) => summary,
            Err(err) => {
                tracing::warn!("Cannot create the Spotify playlist: {err}");
                self.message(&gettext("The playlist couldn't be created on Spotify"));
                return;
            }
        };
        self.imp()
            .sync
            .borrow_mut()
            .created
            .push((summary.id.clone(), std::time::Instant::now()));
        {
            let Some(mut db) = library.db() else { return };
            let owner = self.imp().account_name.borrow().clone();
            if let Err(err) =
                db.link_playlist(id, &summary.id, &summary.snapshot_id, owner.as_deref())
            {
                tracing::warn!("Cannot link the playlist: {err}");
                return;
            }
            for (item, uri) in sure {
                let _ = db.set_item_spotify(*item, uri);
            }
            let uris = db.spotify_uris(id).unwrap_or_default();
            let _ = db.queue_spotify_op(id, &Op::Replace { uris }.to_json(), now());
        }
        library.reload_playlists();
        // Translators: {} is a playlist name.
        self.message(&gettext("Created “{}” on Spotify").replace("{}", name));
        // Sent next (this runs as a job).
        self.imp().sync.borrow_mut().flush = true;
    }
}

/// Songs added one after another (each queued on its own) go as one call
/// of up to 100: at the end, or at positions that follow each other.
fn merge_adds(ops: Vec<(Vec<i64>, Op)>) -> Vec<(Vec<i64>, Op)> {
    const MAX_URIS: usize = 100;
    let mut merged: Vec<(Vec<i64>, Op)> = Vec::new();
    for (ids, op) in ops {
        if let (
            Some((
                last_ids,
                Op::Add {
                    uris: last,
                    position: last_position,
                    sent: false,
                    ..
                },
            )),
            Op::Add {
                uris,
                position,
                sent: false,
                ..
            },
        ) = (merged.last_mut(), &op)
        {
            let follows = match (*last_position, *position) {
                (None, None) => true,
                (Some(a), Some(b)) => b as usize == a as usize + last.len(),
                _ => false,
            };
            if follows && last.len() + uris.len() <= MAX_URIS {
                last.extend(uris.iter().cloned());
                last_ids.extend(ids);
                continue;
            }
        }
        merged.push((ids, op));
    }
    merged
}

/// The request may have arrived though its answer didn't (a timeout, a
/// reset, Spotify failing on its side).
fn answer_lost(err: &Error) -> bool {
    matches!(err, Error::Http(_)) || err.status().is_some_and(|s| s >= 500)
}

/// Spotify said no to the change itself (not to the login, not busy).
fn refused_change(err: &Error) -> bool {
    matches!(
        err,
        Error::Status {
            status: 400..=499,
            ..
        }
    ) && !matches!(
        err,
        Error::Status {
            status: 401 | 429,
            ..
        }
    )
}

/// Why Spotify couldn't be searched, for the user.
pub fn search_error(err: &Error) -> String {
    request_error(err).unwrap_or_else(|| {
        // Translators: {} is Spotify's own error message.
        gettext("Spotify couldn't be searched ({}).").replace("{}", &err.to_string())
    })
}

/// Why Spotify didn't answer, for the user (an artist's releases, an
/// album).
pub fn answer_error(err: &Error) -> String {
    request_error(err).unwrap_or_else(|| {
        // Translators: {} is Spotify's own error message.
        gettext("Spotify said no ({}).").replace("{}", &err.to_string())
    })
}

/// The reasons that read the same for every request; `None` for
/// Spotify's own error messages.
fn request_error(err: &Error) -> Option<String> {
    Some(match err {
        // Spotify limits how much an app may search; after many searches
        // (matching a large library) it can ask to wait for hours.
        Error::RateLimited(wait) if wait.as_secs() >= 90 => {
            // Translators: {} is a time, e.g. "3 h" or "20 min".
            gettext("Spotify asks Gatefold to wait {} (too many requests).")
                .replace("{}", &wait_text(*wait))
        }
        Error::RateLimited(_) => {
            gettext("Spotify is busy (too many requests); Gatefold tries again later.")
        }
        Error::LoggedOut => gettext("Not connected to Spotify; connect in Preferences."),
        Error::Http(_) => gettext("Spotify can't be reached; Gatefold tries again later."),
        _ => return None,
    })
}

/// "3 h", "20 min".
fn wait_text(wait: std::time::Duration) -> String {
    let minutes = wait.as_secs().div_ceil(60);
    if minutes >= 90 {
        // Translators: hours, e.g. "3 h".
        gettext("{} h").replace("{}", &minutes.div_ceil(60).to_string())
    } else {
        // Translators: minutes, e.g. "20 min".
        gettext("{} min").replace("{}", &minutes.to_string())
    }
}

/// Searches for a library song, most exact first: title and artist as
/// fields, then as plain words (forgiving with punctuation and
/// spellings), then the title alone.
fn search_queries(local: &LocalTrack) -> Vec<String> {
    let title = matching::search_title(&local.title);
    let artists: Vec<String> = [local.artist.as_deref(), local.album_artist.as_deref()]
        .into_iter()
        .flatten()
        .filter_map(|a| {
            gatefold_core::library::artists::track_artist_names(a)
                .into_iter()
                .next()
        })
        .fold(Vec::new(), |mut list, name| {
            if !list.iter().any(|n: &String| n.eq_ignore_ascii_case(&name)) {
                list.push(name);
            }
            list
        });
    let artist = artists.first().cloned().unwrap_or_default();
    let mut queries = vec![
        format!("track:{} artist:{}", quote(&title), quote(&artist)),
        format!("{title} {artist}"),
    ];
    // "TOMORROW X TOGETHER" on the album, "Yeonjun" on the track.
    if let Some(other) = artists.get(1) {
        queries.push(format!("{title} {other}"));
    }
    queries.push(title);
    queries.dedup();
    queries
}

/// A field value for Spotify's search: quoted when it has spaces.
fn quote(value: &str) -> String {
    let value = value.replace('"', "");
    if value.contains(' ') {
        format!("\"{value}\"")
    } else {
        value
    }
}

fn small_heading(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .css_classes(["heading"])
        .build()
}

fn boxed_list() -> gtk::ListBox {
    gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn songs_added_one_after_another_go_together() {
        let add = |uri: &str, position| Op::Add {
            uris: vec![uri.to_owned()],
            position,
            after: None,
            sent: false,
        };
        let merged = merge_adds(vec![
            (vec![1], add("a", None)),
            (vec![2], add("b", None)),
            (vec![3], add("c", Some(5))),
            (vec![4], add("d", Some(6))),
            (vec![5], add("e", Some(9))),
            (vec![6], Op::Rename { name: "x".into() }),
            (vec![7], add("f", None)),
        ]);
        let shapes: Vec<(Vec<i64>, Vec<String>)> = merged
            .into_iter()
            .map(|(ids, op)| match op {
                Op::Add { uris, .. } => (ids, uris),
                _ => (ids, Vec::new()),
            })
            .collect();
        assert_eq!(
            shapes,
            [
                (vec![1, 2], vec!["a".to_owned(), "b".to_owned()]),
                (vec![3, 4], vec!["c".to_owned(), "d".to_owned()]),
                (vec![5], vec!["e".to_owned()]),
                (vec![6], vec![]),
                (vec![7], vec!["f".to_owned()]),
            ]
        );
    }

    #[test]
    fn only_spotify_refusing_a_change_drops_it() {
        let status = |status| Error::Status {
            status,
            message: String::new(),
            reason: None,
        };
        assert!(refused_change(&status(400)));
        assert!(refused_change(&status(403)));
        assert!(!refused_change(&status(401)));
        assert!(!refused_change(&status(429)));
        assert!(!refused_change(&status(503)));
        assert!(!refused_change(&Error::Auth("server_error".into())));
    }

    #[test]
    fn ops_round_trip() {
        for op in [
            Op::Add {
                uris: vec!["spotify:track:a".into()],
                position: Some(3),
                after: Some("spotify:track:z".into()),
                sent: true,
            },
            Op::Add {
                uris: vec!["spotify:track:a".into()],
                position: None,
                after: None,
                sent: false,
            },
            Op::Remove {
                uris: vec!["spotify:track:b".into()],
                restore: vec![("spotify:track:b".into(), 4)],
            },
            Op::Move {
                start: 1,
                before: 5,
                length: 1,
                uri: Some("spotify:track:c".into()),
                after: None,
            },
            Op::Rename { name: "New".into() },
            Op::Replace { uris: vec![] },
        ] {
            assert_eq!(Op::parse(&op.to_json()), Some(op));
        }
    }

    fn list(names: &str) -> Vec<String> {
        names.split_whitespace().map(str::to_owned).collect()
    }

    fn placed(op: Op, current: &[String]) -> Option<Op> {
        match op.place(current) {
            Placed::Send(op) => Some(op),
            Placed::Done => None,
        }
    }

    #[test]
    fn an_addition_follows_its_song_when_the_playlist_changed() {
        // Made for "a b c" (x after b); meanwhile "z" was put first.
        let op = Op::add(list("x"), Some(2), &list("a b x c"));
        let current = list("z a b c");
        let op = placed(op, &current).unwrap();
        let mut result = current.clone();
        op.record(&mut result);
        assert_eq!(result, list("z a b x c"));
        // The song it followed is gone: its old place.
        let op = Op::add(list("x"), Some(2), &list("a b x c"));
        let mut result = list("a c");
        placed(op, &result).unwrap().record(&mut result);
        assert_eq!(result, list("a c x"));
    }

    #[test]
    fn an_addition_whose_answer_got_lost_is_not_sent_twice() {
        let unsure = |position| Op::Add {
            uris: list("x"),
            position,
            after: Some("b".into()),
            sent: true,
        };
        assert!(placed(unsure(Some(2)), &list("a b x c")).is_none());
        assert!(placed(unsure(None), &list("a b c x")).is_none());
        // Not there: sent again.
        assert!(placed(unsure(Some(2)), &list("a b c")).is_some());
    }

    #[test]
    fn a_move_follows_its_songs_when_the_playlist_changed() {
        // "a b c d": d moved after a, made for that list.
        let op = Op::moved(3, 1, &list("a d b c"));
        // Meanwhile "z" came first: d still goes after a.
        let current = list("z a b c d");
        let op = placed(op, &current).unwrap();
        let mut result = current.clone();
        op.record(&mut result);
        assert_eq!(result, list("z a d b c"));
        // Moved down: a after c.
        let op = Op::moved(0, 2, &list("b c a d"));
        let current = list("a b c d e");
        let mut result = current.clone();
        placed(op, &current).unwrap().record(&mut result);
        assert_eq!(result, list("b c a d e"));
        // The song is gone, or already there: nothing to send.
        assert!(placed(Op::moved(3, 1, &list("a d b c")), &list("a b c")).is_none());
        assert!(placed(Op::moved(3, 1, &list("a d b c")), &list("a d b c")).is_none());
    }

    #[test]
    fn recording_a_move_matches_spotifys_reorder() {
        let mut current = list("a b c d");
        // Spotify: range_start 0, insert_before 3 → "b c a d".
        Op::Move {
            start: 0,
            before: 3,
            length: 1,
            uri: None,
            after: None,
        }
        .record(&mut current);
        assert_eq!(current, list("b c a d"));
        Op::Move {
            start: 3,
            before: 0,
            length: 1,
            uri: None,
            after: None,
        }
        .record(&mut current);
        assert_eq!(current, list("d b c a"));
    }
}
