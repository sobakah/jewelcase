//! Loads cover and portrait thumbnails made by gatefold_core's CoverCache.
//!
//! The thumbnails are our own JPEGs, so GDK's built-in loader reads them.
//! Recently used textures are cached; covers repeat a lot in the grid.

use std::cell::RefCell;
use std::collections::HashMap;

use gatefold_core::library::covers::{CoverCache, CoverSize};
use gtk::gdk;

use crate::config;

/// Textures kept in memory.
const CACHE_LIMIT: usize = 400;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    Cover,
    Portrait,
}

thread_local! {
    static CACHE: RefCell<HashMap<(Kind, String, CoverSize), gdk::Texture>> = RefCell::default();
    static COVERS: CoverCache = CoverCache::new(config::cover_dir());
    static PORTRAITS: CoverCache = CoverCache::new(config::portrait_dir());
}

fn path(kind: Kind, key: &str, size: CoverSize) -> std::path::PathBuf {
    match kind {
        Kind::Cover => COVERS.with(|c| c.path(key, size)),
        Kind::Portrait => PORTRAITS.with(|c| c.path(key, size)),
    }
}

fn load(kind: Kind, key: &str, size: CoverSize) -> Option<gdk::Texture> {
    let cache_key = (kind, key.to_owned(), size);
    if let Some(texture) = CACHE.with_borrow(|cache| cache.get(&cache_key).cloned()) {
        return Some(texture);
    }
    let path = path(kind, key, size);
    let texture = match gdk::Texture::from_filename(&path) {
        Ok(texture) => texture,
        Err(err) => {
            tracing::debug!("No thumbnail {}: {err}", path.display());
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

pub fn texture(key: &str, size: CoverSize) -> Option<gdk::Texture> {
    load(Kind::Cover, key, size)
}

pub fn portrait(key: &str, size: CoverSize) -> Option<gdk::Texture> {
    load(Kind::Portrait, key, size)
}

/// The cover's dominant color, if the scanner computed it.
pub fn color(key: &str) -> Option<[u8; 3]> {
    COVERS.with(|covers| covers.color(key))
}

/// A `file://` URI of a cover thumbnail, for MPRIS.
pub fn uri(key: &str, size: CoverSize) -> Option<String> {
    let path = path(Kind::Cover, key, size);
    path.is_file()
        .then(|| gtk::glib::filename_to_uri(&path, None).ok())
        .flatten()
        .map(|uri| uri.to_string())
}

/// Forgets cached textures, e.g. after a rescan replaced thumbnails.
pub fn clear() {
    CACHE.with_borrow_mut(HashMap::clear);
}
