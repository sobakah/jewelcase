//! Finds audio files in the library folders and keeps the database in sync
//! (CONCEPT.md §3.1).
//!
//! - Symlinks are skipped, whether they point to files or folders.
//! - Only new or changed files (by modification time and size) are read.
//! - Tracks inside a library folder that is currently missing (an unmounted
//!   drive) are kept.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use super::artists::{Aliases, build_artists};
use super::covers::{self, CoverCache};
use super::grouping::{album_folder, group_albums};
use super::tags;
use crate::db::{Database, DbError, ScannedFile};

/// File extensions the scanner reads (lofty-supported formats).
const AUDIO_EXTENSIONS: &[&str] = &[
    "flac", "m4a", "mp4", "aac", "alac", "mp3", "ogg", "oga", "opus", "wav", "aif", "aiff", "wv",
    "ape", "mpc", "spx",
];

/// Tracks are written to the database in batches of this size.
const BATCH_SIZE: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanProgress {
    /// Reading tags of new or changed files.
    Reading { done: usize, total: usize },
    /// The albums are in the database; covers may still be missing.
    AlbumsReady,
    /// Creating cover thumbnails.
    Covers { done: usize, total: usize },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanSummary {
    /// Audio files in the library folders.
    pub files: usize,
    /// New or changed files that were read.
    pub read: usize,
    pub removed: usize,
    /// Files found at a new path (a renamed or moved folder): their songs
    /// kept their ID.
    pub moved: usize,
    /// Files whose tags couldn't be read.
    pub failed: usize,
    pub symlinks_skipped: usize,
    pub albums: usize,
    pub artists: usize,
    /// Albums, artists and search were rebuilt (something changed).
    pub regrouped: bool,
    /// Album cover thumbnails made.
    pub covers_made: usize,
}

/// Raise when grouping albums or artists changes, so the next scan
/// regroups even if no file changed.
const GROUPING_VERSION: u32 = 2;
const FINGERPRINT_KEY: &str = "library-grouped-with";

struct FoundFile {
    path: PathBuf,
    mtime: i64,
    size: i64,
    /// Change time: also changes when tags are written with the old
    /// modification time kept.
    ctime: i64,
}

/// Brings the database up to date with `folders` and regroups the albums
/// when something changed (always with `force`: "Rescan Library").
pub fn scan(
    db: &mut Database,
    folders: &[PathBuf],
    aliases: &Aliases,
    covers: &CoverCache,
    force: bool,
    progress: &(dyn Fn(ScanProgress) + Sync),
) -> Result<ScanSummary, DbError> {
    let mut summary = ScanSummary::default();
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    // Folders whose songs are kept though none were found: a missing or
    // empty library folder (an unmounted drive leaves an empty mount
    // point) and folders that couldn't be read. Their songs come back
    // with the drive, history and all.
    let mut unavailable: Vec<PathBuf> = Vec::new();
    for folder in folders {
        let empty = fs::read_dir(folder).map_or(true, |mut entries| entries.next().is_none());
        if empty {
            unavailable.push(folder.clone());
            continue;
        }
        walk(
            folder,
            &mut found,
            &mut seen,
            &mut summary.symlinks_skipped,
            &mut unavailable,
        );
    }
    summary.files = found.len();

    let stamps = db.file_stamps()?;
    let ctimes = db.file_ctimes()?;
    // Change times to record: new, changed, or not recorded yet (the
    // first scan with them, which doesn't read the files for it).
    let new_ctimes: Vec<(PathBuf, i64)> = found
        .iter()
        .filter(|f| ctimes.get(&f.path).copied().flatten() != Some(f.ctime))
        .map(|f| (f.path.clone(), f.ctime))
        .collect();
    let mut changed: Vec<FoundFile> = found
        .into_iter()
        .filter(|f| {
            stamps.get(&f.path) != Some(&(f.mtime, f.size))
                || ctimes
                    .get(&f.path)
                    .copied()
                    .flatten()
                    .is_some_and(|ctime| ctime != f.ctime)
        })
        .collect();
    let mut removed: Vec<PathBuf> = stamps
        .keys()
        .filter(|path| !seen.contains(*path))
        .filter(|path| !unavailable.iter().any(|f| path.starts_with(f)))
        .cloned()
        .collect();

    summary.moved = find_moves(db, &mut changed, &mut removed, &stamps)?;
    let mut unread = Vec::new();
    // Folder images hashed while reading aren't read again for the
    // folder covers below.
    let folder_covers = FolderCovers::default();
    summary.read = read_files(db, &changed, progress, &mut unread, &folder_covers)?;
    summary.failed = unread.len();
    summary.removed = removed.len();
    db.delete_tracks(&removed)?;
    // A file that couldn't be read keeps its old change time, so the next
    // scan tries again (a change of mtime or size would anyway).
    let new_ctimes: Vec<(PathBuf, i64)> = new_ctimes
        .into_iter()
        .filter(|(path, _)| !unread.contains(path))
        .collect();
    db.set_ctimes(&new_ctimes)?;
    let covers_changed = update_folder_covers(db, &seen, &folder_covers)?;

    // Nothing changed (most scans: every start, a watcher event for a
    // file that isn't music): albums, artists and search stay as they are.
    let fingerprint = format!("{GROUPING_VERSION}:{}", aliases.fingerprint());
    let regroup = force
        || summary.read > 0
        || summary.removed > 0
        || summary.moved > 0
        || covers_changed
        || db.app_state(FINGERPRINT_KEY)?.as_deref() != Some(fingerprint.as_str());
    if regroup {
        let tracks = db.all_tracks()?;
        let albums = group_albums(&tracks);
        let artists = build_artists(&albums, &tracks, aliases);
        summary.albums = albums.len();
        summary.artists = artists.len();
        db.replace_library(&albums, &artists)?;
        db.relink_playlists()?;
        db.rebuild_search(aliases)?;
        db.set_app_state(FINGERPRINT_KEY, &fingerprint)?;
        summary.regrouped = true;
        progress(ScanProgress::AlbumsReady);
    }

    summary.covers_made = make_covers(db, covers, progress)?;
    clean_covers(db, covers)?;
    Ok(summary)
}

/// Gone paths that turn up elsewhere (a renamed or moved folder, cdport
/// sorting files) are moved in the database instead of deleted and added
/// again, so their songs keep their ID and with it the play history, the
/// queue, playlists and Spotify matches. Same file name, size and
/// modification time (kept by a rename) is a move; otherwise the audio
/// fingerprint decides, where the analysis made one. Moved files that
/// don't look unchanged are read again. Returns how many moved.
fn find_moves(
    db: &mut Database,
    changed: &mut Vec<FoundFile>,
    removed: &mut Vec<PathBuf>,
    stamps: &HashMap<PathBuf, (i64, i64)>,
) -> Result<usize, DbError> {
    if removed.is_empty() {
        return Ok(0);
    }
    let keys = db.content_keys(removed)?;
    let mut by_size: HashMap<i64, Vec<usize>> = HashMap::new();
    for (i, path) in removed.iter().enumerate() {
        if let Some((_, size)) = stamps.get(path) {
            by_size.entry(*size).or_default().push(i);
        }
    }
    let mut taken = vec![false; removed.len()];
    let mut moves = Vec::new();
    let mut unchanged = HashSet::new();
    for (i, file) in changed.iter().enumerate() {
        // Changed in place, not new here.
        if stamps.contains_key(&file.path) {
            continue;
        }
        let Some(candidates) = by_size.get(&file.size) else {
            continue;
        };
        let free: Vec<usize> = candidates.iter().copied().filter(|r| !taken[*r]).collect();
        let renamed = free.iter().copied().find(|r| {
            stamps[&removed[*r]].0 == file.mtime && removed[*r].file_name() == file.path.file_name()
        });
        let found = renamed.or_else(|| {
            let fingerprinted: Vec<usize> = free
                .iter()
                .copied()
                .filter(|r| keys.contains_key(&removed[*r]))
                .collect();
            if fingerprinted.is_empty() {
                return None;
            }
            let key = crate::analysis::content_key(&file.path).ok()?;
            fingerprinted
                .into_iter()
                .find(|r| keys[&removed[*r]] == key)
        });
        if let Some(r) = found {
            taken[r] = true;
            moves.push((removed[r].clone(), file.path.clone()));
            if renamed.is_some() {
                unchanged.insert(i);
            }
        }
    }
    if moves.is_empty() {
        return Ok(0);
    }
    db.move_tracks(&moves)?;
    let mut index = 0;
    changed.retain(|_| {
        let keep = !unchanged.contains(&index);
        index += 1;
        keep
    });
    let mut index = 0;
    removed.retain(|_| {
        let keep = !taken[index];
        index += 1;
        keep
    });
    tracing::info!(
        "{} files moved; their songs keep their history",
        moves.len()
    );
    Ok(moves.len())
}

/// About once a week: cached covers nothing refers to any more (replaced
/// covers, removed albums, Spotify songs no longer listed) are removed.
fn clean_covers(db: &mut Database, covers: &CoverCache) -> Result<(), DbError> {
    const KEY: &str = "cover-cache-cleaned";
    const EVERY_S: i64 = 7 * 24 * 3600;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let last: i64 = db.app_state(KEY)?.and_then(|v| v.parse().ok()).unwrap_or(0);
    if now - last < EVERY_S {
        return Ok(());
    }
    let mut keep = db.cover_keys_in_use()?;
    // Playlist mosaics of the covers they show now (a playlist's songs
    // changed: its old mosaic goes).
    for playlist in db.playlists()? {
        if playlist.cover_keys.len() > 1 {
            keep.insert(covers.mosaic_key(&playlist.cover_keys));
        }
    }
    // Album covers, Spotify images, mosaics, and playlist covers the
    // user replaced.
    let removed = covers.remove_unused(&keep, &['e', 'f', 's', 'm', 'c']);
    if removed > 0 {
        tracing::info!("Removed {removed} unused covers from the cache");
    }
    db.set_app_state(KEY, &now.to_string())
}

fn walk(
    dir: &Path,
    found: &mut Vec<FoundFile>,
    seen: &mut HashSet<PathBuf>,
    symlinks: &mut usize,
    unreadable: &mut Vec<PathBuf>,
) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            tracing::warn!("Cannot read {}: {err}", dir.display());
            unreadable.push(dir.to_path_buf());
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            tracing::debug!("Skipping non-UTF-8 name in {}", dir.display());
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            *symlinks += 1;
            tracing::debug!("Skipping symlink {}", path.display());
        } else if file_type.is_dir() {
            walk(&path, found, seen, symlinks, unreadable);
        } else if file_type.is_file() && is_audio(&path) {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs() as i64);
            if seen.insert(path.clone()) {
                found.push(FoundFile {
                    path,
                    mtime,
                    size: meta.len() as i64,
                    ctime: std::os::unix::fs::MetadataExt::ctime(&meta),
                });
            }
        }
    }
}

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXTENSIONS.contains(&e.to_lowercase().as_str()))
}

fn workers() -> usize {
    thread::available_parallelism().map_or(2, |n| n.get().min(8))
}

/// Reads tags on worker threads and writes them in batches. Returns how
/// many files were read successfully.
fn read_files(
    db: &mut Database,
    files: &[FoundFile],
    progress: &(dyn Fn(ScanProgress) + Sync),
    failed: &mut Vec<PathBuf>,
    folder_covers: &FolderCovers,
) -> Result<usize, DbError> {
    let total = files.len();
    if total == 0 {
        return Ok(0);
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let next = AtomicUsize::new(0);
    let (tx, rx) = mpsc::channel();

    thread::scope(|scope| {
        for _ in 0..workers().min(total) {
            let tx = tx.clone();
            let next = &next;
            scope.spawn(move || {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(file) = files.get(index) else { break };
                    if tx.send(read_file(file, folder_covers)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);

        let mut batch = Vec::with_capacity(BATCH_SIZE);
        let mut read = 0;
        for (done, result) in rx.into_iter().enumerate() {
            match result {
                Ok(file) => {
                    batch.push(file);
                    read += 1;
                }
                Err((path, err)) => {
                    failed.push(path.clone());
                    tracing::warn!("Skipping {}: {err}", path.display());
                }
            }
            if batch.len() >= BATCH_SIZE {
                db.upsert_tracks(&batch, now)?;
                batch.clear();
            }
            progress(ScanProgress::Reading {
                done: done + 1,
                total,
            });
        }
        db.upsert_tracks(&batch, now)?;
        Ok(read)
    })
}

type FolderCovers = Mutex<HashMap<PathBuf, Option<String>>>;

fn read_file(
    file: &FoundFile,
    folder_covers: &FolderCovers,
) -> Result<ScannedFile, (PathBuf, tags::TagError)> {
    let (tags, picture) = tags::read_tags(&file.path).map_err(|err| (file.path.clone(), err))?;
    let cover_key = match picture {
        Some(data) => Some(covers::cover_key('e', &data)),
        None => folder_cover_key(&file.path, folder_covers),
    };
    Ok(ScannedFile {
        path: file.path.clone(),
        mtime: file.mtime,
        size: file.size,
        tags,
        cover_key,
    })
}

/// The cover image in the track's folder, or in the album folder above a
/// disc subfolder.
fn folder_cover(track: &Path) -> Option<PathBuf> {
    let parent = track.parent()?;
    covers::find_folder_cover(parent).or_else(|| {
        let album = album_folder(track);
        (album != parent)
            .then(|| covers::find_folder_cover(album))
            .flatten()
    })
}

fn folder_cover_key(track: &Path, cache: &FolderCovers) -> Option<String> {
    let parent = track.parent()?.to_path_buf();
    if let Some(key) = cache.lock().ok()?.get(&parent) {
        return key.clone();
    }
    let key = folder_cover(track)
        .and_then(|path| fs::read(path).ok())
        .map(|data| covers::cover_key('f', &data));
    cache.lock().ok()?.insert(parent, key.clone());
    key
}

/// A folder image (cover.jpg …) added, replaced or removed since the last
/// scan: the songs there without an embedded picture get the new cover.
/// (Only changed audio files are read, so this isn't noticed otherwise.)
/// Whether songs' covers changed.
fn update_folder_covers(
    db: &mut Database,
    found: &HashSet<PathBuf>,
    cache: &FolderCovers,
) -> Result<bool, DbError> {
    let stamp = |path: &Path| -> Option<(i64, i64)> {
        let meta = fs::metadata(path).ok()?;
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs() as i64);
        Some((mtime, meta.len() as i64))
    };
    // The folders songs take covers from: their own and the album folder
    // above a disc folder.
    let mut folders: HashSet<PathBuf> = HashSet::new();
    for path in found {
        if let Some(parent) = path.parent() {
            folders.insert(parent.to_path_buf());
            folders.insert(album_folder(path).to_path_buf());
        }
    }
    let old = db.folder_cover_stamps()?;
    let mut now = HashMap::new();
    let mut changed: HashSet<PathBuf> = HashSet::new();
    for folder in folders {
        let current = covers::find_folder_cover(&folder)
            .and_then(|image| stamp(&image).map(|(m, s)| (image, m, s)));
        if current.as_ref() != old.get(&folder) {
            changed.insert(folder.clone());
        }
        if let Some(current) = current {
            now.insert(folder, current);
        }
    }
    let mut updated = false;
    if !changed.is_empty() {
        // A song takes its cover from its own folder or the album folder
        // (folder_cover), so only those two can matter.
        let affected = |path: &Path| {
            path.parent().is_some_and(|p| changed.contains(p))
                || changed.contains(album_folder(path))
        };
        let updates: Vec<(i64, Option<String>)> = db
            .all_tracks()?
            .into_iter()
            .filter(|t| t.cover_key.as_deref().is_none_or(|k| k.starts_with('f')))
            .filter(|t| affected(&t.path))
            .filter_map(|t| {
                let key = folder_cover_key(&t.path, cache);
                (key != t.cover_key).then_some((t.id, key))
            })
            .collect();
        if !updates.is_empty() {
            tracing::info!("{} songs take a changed folder cover", updates.len());
            db.set_track_cover_keys(&updates)?;
            updated = true;
        }
    }
    db.replace_folder_cover_stamps(&now)?;
    Ok(updated)
}

/// Creates thumbnails for album covers that aren't cached yet.
/// Returns how many it made.
fn make_covers(
    db: &Database,
    covers: &CoverCache,
    progress: &(dyn Fn(ScanProgress) + Sync),
) -> Result<usize, DbError> {
    let missing: Vec<(String, PathBuf)> = db
        .album_cover_sources()?
        .into_iter()
        // Covers made before colors existed get theirs from the thumbnail.
        .filter(|(key, _)| !(covers.has(key) && covers.ensure_color(key)))
        .collect();
    let total = missing.len();
    if total == 0 {
        return Ok(0);
    }
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    thread::scope(|scope| {
        for _ in 0..workers().min(total).min(4) {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some((key, track)) = missing.get(index) else {
                        break;
                    };
                    if let Err(err) = make_cover(covers, key, track) {
                        tracing::warn!("No cover for {}: {err}", track.display());
                    }
                    let done = done.fetch_add(1, Ordering::Relaxed) + 1;
                    progress(ScanProgress::Covers { done, total });
                }
            });
        }
    });
    Ok(total)
}

fn make_cover(covers: &CoverCache, key: &str, track: &Path) -> Result<(), String> {
    let data = if key.starts_with('e') {
        tags::read_front_cover(track).map_err(|e| e.to_string())?
    } else {
        folder_cover(track).and_then(|path| fs::read(path).ok())
    };
    let data = data.ok_or("the cover is gone")?;
    covers.generate(key, data).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn walk_skips_symlinks_hidden_and_non_audio() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("Adele/25")).unwrap();
        fs::create_dir_all(root.join(".hidden")).unwrap();
        fs::create_dir_all(root.join("elsewhere")).unwrap();
        fs::write(root.join("Adele/25/01 Hello.flac"), b"").unwrap();
        fs::write(root.join("Adele/25/cover.jpg"), b"").unwrap();
        fs::write(root.join(".hidden/x.mp3"), b"").unwrap();
        fs::write(root.join("elsewhere/y.mp3"), b"").unwrap();
        symlink(root.join("elsewhere"), root.join("Adele/GDrive")).unwrap();
        symlink(root.join("elsewhere/y.mp3"), root.join("Adele/link.mp3")).unwrap();

        let mut found = Vec::new();
        let mut symlinks = 0;
        walk(
            &root.join("Adele"),
            &mut found,
            &mut HashSet::new(),
            &mut symlinks,
            &mut Vec::new(),
        );
        let paths: Vec<_> = found.iter().map(|f| f.path.clone()).collect();
        assert_eq!(paths, [root.join("Adele/25/01 Hello.flac")]);
        assert_eq!(symlinks, 2);
    }

    #[test]
    fn a_scan_without_changes_doesnt_regroup() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut db = Database::open_in_memory().unwrap();
        let covers = CoverCache::new(dir.path().join("covers"));
        let music = dir.path().join("Musik");
        fs::create_dir_all(&music).unwrap();
        fs::write(music.join("broken.flac"), b"nope").unwrap();
        let scan_now = |db: &mut Database, force| {
            scan(
                db,
                std::slice::from_ref(&music),
                &Aliases::default(),
                &covers,
                force,
                &|_| {},
            )
            .unwrap()
        };
        // The first time, the library is grouped.
        assert!(scan_now(&mut db, false).regrouped);
        assert!(!scan_now(&mut db, false).regrouped);
        // "Rescan Library" always does.
        assert!(scan_now(&mut db, true).regrouped);
        // Other aliases (lyrembed's list changed): grouped again.
        let aliases = Aliases::parse_lyrembed("  BTS = Bangtan Boys\n");
        let summary = scan(
            &mut db,
            std::slice::from_ref(&music),
            &aliases,
            &covers,
            false,
            &|_| {},
        )
        .unwrap();
        assert!(summary.regrouped);
    }

    #[test]
    fn a_new_or_replaced_folder_cover_reaches_the_songs() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut db = Database::open_in_memory().unwrap();
        let album = dir.path().join("Adele/25");
        fs::create_dir_all(&album).unwrap();
        let song = album.join("01.flac");
        let embedded = album.join("02.flac");
        let file = |path: &Path, cover_key: Option<&str>| ScannedFile {
            path: path.to_path_buf(),
            mtime: 0,
            size: 0,
            tags: tags::TrackTags::default(),
            cover_key: cover_key.map(str::to_owned),
        };
        db.upsert_tracks(&[file(&song, None), file(&embedded, Some("eown"))], 0)
            .unwrap();
        let seen: HashSet<PathBuf> = [song.clone(), embedded.clone()].into();
        let key_of = |db: &Database, path: &Path| {
            db.all_tracks()
                .unwrap()
                .into_iter()
                .find(|t| t.path == path)
                .unwrap()
                .cover_key
        };

        fs::write(album.join("cover.jpg"), b"first").unwrap();
        update_folder_covers(&mut db, &seen, &FolderCovers::default()).unwrap();
        let first = key_of(&db, &song);
        assert_eq!(first, Some(covers::cover_key('f', b"first")));
        // A song with its own picture keeps it.
        assert_eq!(key_of(&db, &embedded).as_deref(), Some("eown"));

        fs::write(album.join("cover.jpg"), b"second, longer").unwrap();
        update_folder_covers(&mut db, &seen, &FolderCovers::default()).unwrap();
        assert_eq!(
            key_of(&db, &song),
            Some(covers::cover_key('f', b"second, longer"))
        );

        fs::remove_file(album.join("cover.jpg")).unwrap();
        update_folder_covers(&mut db, &seen, &FolderCovers::default()).unwrap();
        assert_eq!(key_of(&db, &song), None);
    }

    #[test]
    fn a_renamed_folder_keeps_its_songs_and_a_new_ctime_is_read_again() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut db = Database::open_in_memory().unwrap();
        let covers = CoverCache::new(dir.path().join("covers"));
        let music = dir.path().join("Musik");
        let old = music.join("Old Name");
        fs::create_dir_all(&old).unwrap();
        // Not decodable: reading it would fail, so a move must not read it.
        fs::write(old.join("a.flac"), b"abc").unwrap();
        let mtime = fs::metadata(old.join("a.flac"))
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        db.upsert_tracks(
            &[ScannedFile {
                path: old.join("a.flac"),
                mtime,
                size: 3,
                tags: tags::TrackTags::default(),
                cover_key: None,
            }],
            0,
        )
        .unwrap();
        let id = db.all_tracks().unwrap()[0].id;
        let scan_now = |db: &mut Database| {
            scan(
                db,
                std::slice::from_ref(&music),
                &Aliases::default(),
                &covers,
                false,
                &|_| {},
            )
            .unwrap()
        };
        // Records the change time, without reading.
        let summary = scan_now(&mut db);
        assert_eq!((summary.read, summary.failed), (0, 0));

        let new = music.join("New Name");
        fs::rename(&old, &new).unwrap();
        let summary = scan_now(&mut db);
        assert_eq!((summary.moved, summary.removed, summary.failed), (1, 0, 0));
        let tracks = db.all_tracks().unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!((tracks[0].id, &tracks[0].path), (id, &new.join("a.flac")));

        // Tags written with the old time kept: the change time differs.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        fs::set_permissions(
            new.join("a.flac"),
            std::os::unix::fs::PermissionsExt::from_mode(0o600),
        )
        .unwrap();
        let summary = scan_now(&mut db);
        assert_eq!(summary.read + summary.failed, 1, "read again");
        // It couldn't be read: the next scan tries again.
        let summary = scan_now(&mut db);
        assert_eq!(summary.failed, 1, "tried again");
    }

    #[test]
    fn scan_keeps_tracks_of_missing_folders_and_drops_deleted_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut db = Database::open_in_memory().unwrap();
        let covers = CoverCache::new(dir.path().join("covers"));
        let music = dir.path().join("Musik");
        fs::create_dir_all(&music).unwrap();
        // Not decodable: the file fails to read and isn't stored.
        fs::write(music.join("broken.flac"), b"nope").unwrap();

        let summary = scan(
            &mut db,
            std::slice::from_ref(&music),
            &Aliases::default(),
            &covers,
            false,
            &|_| {},
        )
        .unwrap();
        assert_eq!((summary.files, summary.read, summary.failed), (1, 0, 1));

        let unmounted = dir.path().join("Drive");
        let stored = ScannedFile {
            path: unmounted.join("a.flac"),
            mtime: 0,
            size: 0,
            tags: tags::TrackTags::default(),
            cover_key: None,
        };
        let gone = ScannedFile {
            path: music.join("gone.flac"),
            ..stored.clone()
        };
        // An empty mount point (the drive isn't mounted) and a folder
        // that can't be read keep their songs too.
        let mount_point = dir.path().join("NAS");
        fs::create_dir_all(&mount_point).unwrap();
        let on_nas = ScannedFile {
            path: mount_point.join("b.flac"),
            ..stored.clone()
        };
        let locked = music.join("Locked");
        fs::create_dir_all(&locked).unwrap();
        let in_locked = ScannedFile {
            path: locked.join("c.flac"),
            ..stored.clone()
        };
        fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o000)).unwrap();
        // Root (as in a CI container) reads it anyway; that case is left out.
        let can_lock = fs::read_dir(&locked).is_err();
        let mut tracks = vec![stored, gone, on_nas];
        if can_lock {
            tracks.push(in_locked);
        }
        db.upsert_tracks(&tracks, 0).unwrap();
        let summary = scan(
            &mut db,
            &[music, unmounted.clone(), mount_point.clone()],
            &Aliases::default(),
            &covers,
            false,
            &|_| {},
        )
        .unwrap();
        fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        assert_eq!(summary.removed, 1);
        assert!(summary.regrouped);
        let mut left: Vec<_> = db
            .all_tracks()
            .unwrap()
            .into_iter()
            .map(|t| t.path)
            .collect();
        left.sort();
        let mut expected = vec![unmounted.join("a.flac"), mount_point.join("b.flac")];
        if can_lock {
            expected.push(locked.join("c.flac"));
        }
        expected.sort();
        assert_eq!(left, expected);
    }
}
