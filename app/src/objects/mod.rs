//! GObject wrappers around core types, for list models (CONCEPT.md §2).

mod album;
mod artist;
mod playlist;
mod queue_entry;
mod track;

pub use album::AlbumObject;
pub use artist::ArtistObject;
pub use playlist::PlaylistObject;
pub use queue_entry::QueueEntry;
pub use track::TrackObject;
