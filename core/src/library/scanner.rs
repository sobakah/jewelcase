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
    /// Files whose tags couldn't be read.
    pub failed: usize,
    pub symlinks_skipped: usize,
    pub albums: usize,
    pub artists: usize,
}

struct FoundFile {
    path: PathBuf,
    mtime: i64,
    size: i64,
}

/// Brings the database up to date with `folders` and regroups the albums.
pub fn scan(
    db: &mut Database,
    folders: &[PathBuf],
    aliases: &Aliases,
    covers: &CoverCache,
    progress: &(dyn Fn(ScanProgress) + Sync),
) -> Result<ScanSummary, DbError> {
    let mut summary = ScanSummary::default();
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    for folder in folders {
        walk(folder, &mut found, &mut seen, &mut summary.symlinks_skipped);
    }
    summary.files = found.len();

    let stamps = db.file_stamps()?;
    let changed: Vec<FoundFile> = found
        .into_iter()
        .filter(|f| stamps.get(&f.path) != Some(&(f.mtime, f.size)))
        .collect();
    let missing_folders: Vec<&PathBuf> = folders.iter().filter(|f| !f.is_dir()).collect();
    let removed: Vec<PathBuf> = stamps
        .into_keys()
        .filter(|path| !seen.contains(path))
        .filter(|path| !missing_folders.iter().any(|f| path.starts_with(f)))
        .collect();

    summary.read = read_files(db, &changed, progress, &mut summary.failed)?;
    summary.removed = removed.len();
    db.delete_tracks(&removed)?;

    let tracks = db.all_tracks()?;
    let albums = group_albums(&tracks);
    let artists = build_artists(&albums, &tracks, aliases);
    summary.albums = albums.len();
    summary.artists = artists.len();
    db.replace_library(&albums, &artists)?;
    progress(ScanProgress::AlbumsReady);

    make_covers(db, covers, progress)?;
    Ok(summary)
}

fn walk(dir: &Path, found: &mut Vec<FoundFile>, seen: &mut HashSet<PathBuf>, symlinks: &mut usize) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            tracing::warn!("Cannot read {}: {err}", dir.display());
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
            walk(&path, found, seen, symlinks);
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
    failed: &mut usize,
) -> Result<usize, DbError> {
    let total = files.len();
    if total == 0 {
        return Ok(0);
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let next = AtomicUsize::new(0);
    let folder_covers = Mutex::new(HashMap::new());
    let (tx, rx) = mpsc::channel();

    thread::scope(|scope| {
        for _ in 0..workers().min(total) {
            let tx = tx.clone();
            let (next, folder_covers) = (&next, &folder_covers);
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
                    *failed += 1;
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

/// Creates thumbnails for album covers that aren't cached yet.
fn make_covers(
    db: &Database,
    covers: &CoverCache,
    progress: &(dyn Fn(ScanProgress) + Sync),
) -> Result<(), DbError> {
    let missing: Vec<(String, PathBuf)> = db
        .album_cover_sources()?
        .into_iter()
        .filter(|(key, _)| !covers.has(key))
        .collect();
    let total = missing.len();
    if total == 0 {
        return Ok(());
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
    Ok(())
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
        );
        let paths: Vec<_> = found.iter().map(|f| f.path.clone()).collect();
        assert_eq!(paths, [root.join("Adele/25/01 Hello.flac")]);
        assert_eq!(symlinks, 2);
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
        db.upsert_tracks(&[stored, gone], 0).unwrap();
        let summary = scan(
            &mut db,
            &[music, unmounted.clone()],
            &Aliases::default(),
            &covers,
            &|_| {},
        )
        .unwrap();
        assert_eq!(summary.removed, 1);
        let left: Vec<_> = db
            .all_tracks()
            .unwrap()
            .into_iter()
            .map(|t| t.path)
            .collect();
        assert_eq!(left, [unmounted.join("a.flac")]);
    }
}
