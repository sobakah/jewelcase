//! GObject wrappers around core types, for list models (CONCEPT.md §2).

mod album;
mod artist;
mod track;

pub use album::AlbumObject;
pub use artist::ArtistObject;
pub use track::TrackObject;
