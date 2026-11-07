//! Tiles for Spotify's albums and artists (CONCEPT.md §9.8): cover, title
//! and a dim line, like the library's tiles.

use adw::prelude::*;
use gtk::glib;

use super::CoverImage;

/// What a tile shows.
#[derive(Debug, Clone, Default)]
pub struct ShelfItem {
    pub cover_key: Option<String>,
    pub title: String,
    pub subtitle: String,
    /// Artists show a person when there's no picture, and their name bold.
    pub artist: bool,
}

/// The tile's content, and its cover (to show it once downloaded).
pub fn tile_content(item: &ShelfItem, natural: i32) -> (gtk::Box, CoverImage) {
    let cover: CoverImage = glib::Object::builder()
        .property("min-size", 120)
        .property("natural-size", natural)
        .build();
    if item.artist {
        cover.set_placeholder_icon("avatar-default-symbolic");
    }
    cover.set_cover(item.cover_key.as_deref());
    cover.set_label(&item.title);
    cover.set_margin_bottom(6);
    let label = |text: &str, classes: &[&str]| {
        gtk::Label::builder()
            .label(text)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .width_chars(1)
            .max_width_chars(1)
            .css_classes(classes)
            .build()
    };
    let content = gtk::Box::new(gtk::Orientation::Vertical, 3);
    content.add_css_class(if item.artist {
        "artist-tile"
    } else {
        "album-tile"
    });
    content.append(&cover);
    content.append(&label(
        &item.title,
        if item.artist {
            &["artist-name"]
        } else {
            &["album-title"]
        },
    ));
    if !item.subtitle.is_empty() {
        content.append(&label(&item.subtitle, &["dim-label"]));
    }
    (content, cover)
}

/// A clickable tile, for grids built by hand (artist pages).
pub fn sized_tile(item: &ShelfItem, natural: i32) -> (gtk::Button, CoverImage) {
    let (content, cover) = tile_content(item, natural);
    let button = gtk::Button::builder()
        .child(&content)
        .tooltip_text(&item.title)
        .css_classes(["flat", "grid-tile"])
        .build();
    (button, cover)
}
