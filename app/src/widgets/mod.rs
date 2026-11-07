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
pub use spotify_tile::{ShelfItem, tile_content};
pub use track_row::TrackRow;

/// Disconnects a handler `owner` put on a long-lived `source` (the
/// library, player, settings) once `owner` is gone, so pages and dialogs
/// opened again and again don't leave handlers running for nothing.
pub fn disconnect_with(
    owner: &impl gtk::glib::object::IsA<gtk::glib::Object>,
    source: &impl gtk::glib::object::IsA<gtk::glib::Object>,
    id: gtk::glib::SignalHandlerId,
) {
    use gtk::glib::prelude::*;
    let source = source.clone().upcast::<gtk::glib::Object>();
    let _ = owner.add_weak_ref_notify_local(move || source.disconnect(id));
}

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
    line.append(&spotify_icon());
    line.append(&label);
    line.upcast()
}

/// The small Spotify icon for songs that play from Spotify.
pub fn spotify_icon() -> gtk::Image {
    gtk::Image::builder()
        .icon_name("gatefold-spotify-symbolic")
        .pixel_size(12)
        .valign(gtk::Align::Center)
        .tooltip_text(gettextrs::gettext("Plays from Spotify"))
        .build()
}
