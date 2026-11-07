mod album_panel;
mod album_tile;
mod artist_page;
mod artist_tile;
mod cover_image;
mod device_popover;
mod identify_dialog;
mod lyrics_view;
mod mini_player;
pub mod panel_resizer;
mod player_view;
mod playlist_panel;
pub mod playlist_picker;
mod playlist_tile;
mod portrait_dialog;
mod queue_view;
mod reorder;
mod search_view;
mod sectioned_grid;
mod shuffle_options;
mod spotify_tile;
mod track_info;
mod track_row;

pub use album_panel::AlbumPanel;
pub use album_tile::AlbumTile;
pub use artist_page::ArtistPage;
pub use artist_tile::ArtistTile;
pub use cover_image::CoverImage;
pub use lyrics_view::LyricsView;
pub use mini_player::MiniPlayer;
pub use player_view::PlayerView;
pub use playlist_panel::PlaylistPanel;
pub use playlist_tile::PlaylistTile;
pub use queue_view::QueueView;
pub use search_view::SearchView;
pub use sectioned_grid::SectionedGrid;
pub use spotify_tile::{ShelfItem, sized_tile, tile_content};
pub use track_row::TrackRow;

/// A song title, with the Spotify icon before it for songs that play from
/// Spotify (§9.4).
pub fn spotify_title(title: &str, from_spotify: bool) -> gtk::Widget {
    use gtk::prelude::*;
    let label = gtk::Label::builder()
        .label(title)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    if !from_spotify {
        return label.upcast();
    }
    let line = gtk::Box::builder().spacing(6).build();
    line.append(
        &gtk::Image::builder()
            .icon_name("gatefold-spotify-symbolic")
            .pixel_size(12)
            .valign(gtk::Align::Center)
            .tooltip_text(gettextrs::gettext("Plays from Spotify"))
            .build(),
    );
    line.append(&label);
    line.upcast()
}
