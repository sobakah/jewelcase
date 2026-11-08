//! The music library: folders, tags, albums and the scanner (CONCEPT.md §3).

pub mod artists;
pub mod covers;
pub mod flac;
pub mod folders;
pub mod grouping;
pub mod scanner;
pub mod tags;
pub mod titles;
pub mod watch;

use std::path::PathBuf;

use tags::TrackTags;

/// A track file as stored in the library database.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackRecord {
    pub id: i64,
    pub path: PathBuf,
    pub tags: TrackTags,
    /// Cache key of the track's cover (embedded or from the album folder).
    pub cover_key: Option<String>,
    /// When the file was first seen (Unix seconds).
    pub added: i64,
}
