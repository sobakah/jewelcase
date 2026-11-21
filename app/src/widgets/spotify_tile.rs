//! Tiles for Spotify's albums, artists and followed playlists (CONCEPT.md
//! §9.8): cover, title and a dim line, like the library's tiles.

use adw::prelude::*;

use super::CoverImage;

/// What a tile shows.
#[derive(Debug, Clone, Default)]
pub struct ShelfItem {
    pub cover_key: Option<String>,
    pub title: String,
    pub subtitle: String,
    pub kind: ShelfKind,
}

/// What kind of thing a tile shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ShelfKind {
    /// In a jewel case when those are on.
    #[default]
    Album,
    /// A person when there's no picture, and the name bold.
    Artist,
    /// As the library's playlist tiles.
    Playlist,
}

/// The tile's content, and its cover (to show it once downloaded).
pub fn tile_content(item: &ShelfItem) -> (gtk::Box, CoverImage) {
    let cover = super::tile_cover();
    match item.kind {
        ShelfKind::Artist => cover.set_placeholder_icon("avatar-default-symbolic"),
        ShelfKind::Album => super::album_case(&cover),
        ShelfKind::Playlist => {}
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
    // The cover's size, also in a wider cell.
    content.set_halign(gtk::Align::Center);
    let artist = item.kind == ShelfKind::Artist;
    content.add_css_class(if artist { "artist-tile" } else { "album-tile" });
    content.append(&cover);
    content.append(&label(
        &item.title,
        if artist {
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
