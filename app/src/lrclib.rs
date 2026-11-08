//! Lyrics for Spotify songs that aren't in the library, from LRCLIB
//! (CONCEPT.md §5): shown only, never written anywhere but Gatefold's
//! cache. Gentle on LRCLIB's servers:
//!
//! - only for the song that plays, and only while the lyrics are shown
//!   (no prefetching, nothing for the library)
//! - every answer is kept, "not found" too (asked again after 30 days)
//! - after LRCLIB asks to wait (429) or fails (5xx), nothing is asked
//!   for an hour at least (`Retry-After` if longer); offline, 10 minutes
//! - one request at a time (a second one without the album if the first
//!   found nothing), with Gatefold's User-Agent

use std::cell::{Cell, RefCell};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use gatefold_core::db::OnlineLyricsRow;
use gatefold_lyrics::LyricsState;
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::application::GatefoldApplication;
use crate::config;
use crate::metadata::now;
use crate::objects::TrackObject;

/// "Not found" is believed this long.
const MISS_AGAIN_S: i64 = 30 * 24 * 3600;
/// After a 429 or 5xx, at least this long without asking.
const BUSY_PAUSE: Duration = Duration::from_secs(3600);
/// After a network error.
const OFFLINE_PAUSE: Duration = Duration::from_secs(600);
/// How often a song waiting for another song's request looks again.
const ASKING_POLL: Duration = Duration::from_millis(200);

thread_local! {
    static PAUSED_UNTIL: Cell<Option<Instant>> = const { Cell::new(None) };
    /// The song being asked for.
    static ASKING: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn client() -> &'static gatefold_metadata::Client {
    static CLIENT: OnceLock<gatefold_metadata::Client> = OnceLock::new();
    CLIENT.get_or_init(|| gatefold_metadata::Client::new(config::VERSION))
}

/// Online lookups and this source are on (Preferences).
pub fn enabled() -> bool {
    let settings = GatefoldApplication::get().player().settings().clone();
    settings.boolean("online-metadata") && settings.boolean("lyrics-lrclib")
}

/// What LRCLIB knows about a Spotify song (`LyricsState::None`: nothing).
/// `None`: it wasn't asked (turned off, waiting after an error, no longer
/// wanted, not a Spotify song). While another song is being asked for,
/// this waits for it, as long as `still_wanted` says so (the song may be
/// skipped meanwhile).
pub async fn lyrics(track: &TrackObject, still_wanted: impl Fn() -> bool) -> Option<LyricsState> {
    if !enabled() || !track.is_stand_in() {
        return None;
    }
    let library = GatefoldApplication::get().library();
    let uri = GatefoldApplication::get()
        .spotify()
        .stand_in_uri(track.id())?;
    if let Some(known) = cached(&uri) {
        return Some(known);
    }
    // One request at a time.
    while ASKING.with_borrow(Option::is_some) {
        glib::timeout_future(ASKING_POLL).await;
        if !still_wanted() {
            return None;
        }
    }
    // The request before may have been for this song.
    if let Some(known) = cached(&uri) {
        return Some(known);
    }
    if PAUSED_UNTIL
        .get()
        .is_some_and(|until| Instant::now() < until)
    {
        return None;
    }
    ASKING.set(Some(uri.clone()));
    let title = track.title();
    let artist = track.artist().unwrap_or_default();
    let album = track
        .track()
        .album_title
        .clone()
        .or_else(|| track.track().record.tags.album.clone());
    let seconds = track.duration_ms().div_ceil(1000);
    let answer = gio::spawn_blocking(move || {
        // Spotify's album names often differ ("(2011 Remaster)"): once
        // more without it.
        match client().lrclib(&title, &artist, album.as_deref(), seconds) {
            Ok(None) if album.is_some() => client().lrclib(&title, &artist, None, seconds),
            result => result,
        }
    })
    .await;
    ASKING.set(None);
    let row = match answer {
        Ok(Ok(Some(found))) => OnlineLyricsRow {
            checked: now(),
            found: true,
            synced: found.synced,
            plain: found.plain,
            instrumental: found.instrumental,
        },
        Ok(Ok(None)) => OnlineLyricsRow {
            checked: now(),
            ..OnlineLyricsRow::default()
        },
        Ok(Err(gatefold_metadata::Error::Busy(wait))) => {
            tracing::info!("LRCLIB asks to wait; not asking for a while");
            PAUSED_UNTIL.set(Some(Instant::now() + wait.max(BUSY_PAUSE)));
            return None;
        }
        Ok(Err(err)) => {
            tracing::info!("Cannot ask LRCLIB: {err}");
            PAUSED_UNTIL.set(Some(Instant::now() + OFFLINE_PAUSE));
            return None;
        }
        Err(_) => return None,
    };
    if let Some(mut db) = library.db()
        && let Err(err) = db.set_online_lyrics(&uri, &row)
    {
        tracing::warn!("Cannot keep the lyrics: {err}");
    }
    Some(state(&row))
}

/// The cached answer, while it holds ("not found" for 30 days).
fn cached(uri: &str) -> Option<LyricsState> {
    let library = GatefoldApplication::get().library();
    let row = library.db()?.online_lyrics(uri).ok()??;
    if row.found {
        Some(state(&row))
    } else if now() - row.checked < MISS_AGAIN_S {
        Some(LyricsState::None)
    } else {
        None
    }
}

fn state(row: &OnlineLyricsRow) -> LyricsState {
    if !row.found {
        return LyricsState::None;
    }
    gatefold_lyrics::from_online(
        row.synced.as_deref(),
        row.plain.as_deref(),
        row.instrumental,
    )
}
