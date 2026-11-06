//! Loads cover thumbnails made by the scanner (gatefold_core::library::covers).
//!
//! The thumbnails are our own JPEGs, so GDK's built-in loader reads them.
//! Recently used textures are cached; covers repeat a lot in the grid.

use std::cell::RefCell;
use std::collections::HashMap;

use gatefold_core::library::covers::{CoverCache, CoverSize};
use gtk::gdk;

use crate::config;

/// Textures kept in memory, per size.
const CACHE_LIMIT: usize = 400;

thread_local! {
    static CACHE: RefCell<HashMap<(String, CoverSize), gdk::Texture>> = RefCell::default();
    static COVERS: CoverCache = CoverCache::new(config::cover_dir());
}

pub fn texture(key: &str, size: CoverSize) -> Option<gdk::Texture> {
    let cache_key = (key.to_owned(), size);
    if let Some(texture) = CACHE.with_borrow(|cache| cache.get(&cache_key).cloned()) {
        return Some(texture);
    }
    let path = COVERS.with(|covers| covers.path(key, size));
    let texture = match gdk::Texture::from_filename(&path) {
        Ok(texture) => texture,
        Err(err) => {
            tracing::debug!("No cover thumbnail {}: {err}", path.display());
            return None;
        }
    };
    CACHE.with_borrow_mut(|cache| {
        if cache.len() >= CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(cache_key, texture.clone());
    });
    Some(texture)
}

/// A `file://` URI of a thumbnail, for MPRIS.
pub fn uri(key: &str, size: CoverSize) -> Option<String> {
    let path = COVERS.with(|covers| covers.path(key, size));
    path.is_file()
        .then(|| gtk::glib::filename_to_uri(&path, None).ok())
        .flatten()
        .map(|uri| uri.to_string())
}

/// Forgets cached textures, e.g. after a rescan replaced thumbnails.
pub fn clear() {
    CACHE.with_borrow_mut(HashMap::clear);
}
