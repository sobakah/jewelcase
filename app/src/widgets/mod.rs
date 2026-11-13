mod album_panel;
mod album_tile;
mod artist_page;
mod artist_tile;
mod cover_backdrop;
mod cover_image;
mod device_popover;
mod identify_dialog;
mod lyrics_view;
mod mini_player;
pub mod new_releases;
pub mod panel_resizer;
mod player_view;
mod playlist_panel;
pub mod playlist_picker;
mod playlist_tile;
mod portrait_dialog;
mod queue_view;
pub mod reorder;
mod search_view;
mod sectioned_grid;
mod shuffle_options;
mod slide_view;
mod spotify_tile;
pub mod statistics;
mod track_info;
mod track_row;
mod visualizer;

pub use album_panel::AlbumPanel;
pub use album_tile::AlbumTile;
pub use artist_page::ArtistPage;
pub use artist_tile::ArtistTile;
pub use cover_backdrop::CoverBackdrop;
pub use cover_image::CoverImage;
pub use lyrics_view::LyricsView;
pub use mini_player::MiniPlayer;
pub use player_view::PlayerView;
pub use playlist_panel::{DEFAULT_SORTS as DEFAULT_PLAYLIST_SORTS, PlaylistPanel};
pub use playlist_tile::PlaylistTile;
pub use queue_view::QueueView;
pub use search_view::SearchView;
pub use sectioned_grid::SectionedGrid;
pub use slide_view::SlideView;
pub use spotify_tile::{ShelfItem, tile_content};
pub use track_row::TrackRow;
pub use visualizer::Visualizer;

thread_local! {
    static TILE_SETTINGS: gtk::gio::Settings =
        gtk::gio::Settings::new(crate::config::APP_ID);
}

/// The settings the grids' cover size comes from.
fn tile_settings() -> gtk::gio::Settings {
    TILE_SETTINGS.with(Clone::clone)
}

/// The cover size in the grids (Preferences › Appearance, CONCEPT.md §4.1).
pub fn tile_size() -> i32 {
    use gtk::prelude::*;
    tile_settings().int("tile-size")
}

/// Calls `changed` on `owner` when the cover size in the grids changes,
/// while `owner` lives.
pub fn connect_tile_size<O: gtk::glib::object::IsA<gtk::glib::Object>>(
    owner: &O,
    changed: impl Fn(&O) + 'static,
) {
    use gtk::prelude::*;
    let settings = tile_settings();
    let weak = owner.downgrade();
    let id = settings.connect_changed(Some("tile-size"), move |_, _| {
        if let Some(owner) = weak.upgrade() {
            changed(&owner);
        }
    });
    disconnect_with(owner, &settings, id);
}

/// A grid tile's cover: as large as set in Preferences, never larger (a
/// wider cell leaves room around it), smaller only where the window is
/// too narrow for two columns.
pub fn tile_cover() -> CoverImage {
    use gtk::prelude::*;
    let cover: CoverImage = gtk::glib::Object::builder()
        .property("min-size", 120)
        .property("capped", true)
        .build();
    tile_settings()
        .bind("tile-size", &cover, "natural-size")
        .get()
        .build();
    cover
}

/// An album's cover goes into a jewel case while the hidden extra is on
/// (window.rs).
pub fn album_case(cover: &CoverImage) {
    use gtk::prelude::*;
    tile_settings()
        .bind("jewel-cases", cover, "case")
        .get()
        .build();
}

/// The name an icon-only button is announced by (what its tooltip said
/// before tooltips were kept for less common icons).
pub fn set_accessible_label(widget: &impl gtk::prelude::IsA<gtk::Accessible>, label: &str) {
    use gtk::prelude::AccessibleExtManual;
    widget.update_property(&[gtk::accessible::Property::Label(label)]);
}

/// A tile's tooltip: its texts in full, only while one is cut off ("…").
/// Taken from the labels that can shorten, when asked, so a reused tile
/// needs nothing more.
pub fn tooltip_when_cut(widget: &impl gtk::prelude::IsA<gtk::Widget>) {
    use gtk::prelude::*;
    fn collect(widget: &gtk::Widget, texts: &mut Vec<String>, cut: &mut bool) {
        let mut child = widget.first_child();
        while let Some(current) = child {
            if current.is_visible() {
                match current.downcast_ref::<gtk::Label>() {
                    Some(label) => {
                        if label.ellipsize() != gtk::pango::EllipsizeMode::None
                            && !label.text().is_empty()
                        {
                            *cut |= label.layout().is_ellipsized();
                            texts.push(label.text().to_string());
                        }
                    }
                    None => collect(&current, texts, cut),
                }
            }
            child = current.next_sibling();
        }
    }
    widget.set_has_tooltip(true);
    widget.connect_query_tooltip(|widget, _, _, _, tooltip| {
        let mut texts = Vec::new();
        let mut cut = false;
        collect(widget.upcast_ref(), &mut texts, &mut cut);
        if !cut {
            return false;
        }
        tooltip.set_text(Some(&texts.join(" — ")));
        true
    });
}

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

/// A song's title and the line under it (the artist), with the Spotify
/// icon before the title for songs that play from Spotify (§9.4): the
/// line under starts under the title, not under the icon (like the mini
/// player).
pub fn song_lines(title: &str, from_spotify: bool, subtitle: Option<&str>) -> gtk::Widget {
    use gtk::prelude::*;
    let grid = gtk::Grid::builder()
        .column_spacing(6)
        .valign(gtk::Align::Center)
        .hexpand(true)
        .build();
    if from_spotify {
        grid.attach(&spotify_icon(), 0, 0, 1, 1);
    }
    let label = gtk::Label::builder()
        .label(title)
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    grid.attach(&label, 1, 0, 1, 1);
    if let Some(subtitle) = subtitle {
        grid.attach(
            &gtk::Label::builder()
                .label(subtitle)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(["dim-label", "caption"])
                .build(),
            1,
            1,
            1,
            1,
        );
    }
    grid.upcast()
}

/// The Spotify icon's size in lists and the mini player (pixels). Spotify's
/// guidelines ask for 21 at least; the owner chose a smaller one, in line
/// with the other icons (§9.4).
pub const SPOTIFY_ICON_SIZE: i32 = 16;

/// Spotify's icon for content from Spotify, in the colours Spotify's
/// guidelines ask (data/resources/icons/README.md).
pub fn spotify_logo(tooltip: &str) -> gtk::Image {
    gtk::Image::builder()
        .icon_name("jewelcase-spotify-symbolic")
        .pixel_size(SPOTIFY_ICON_SIZE)
        .valign(gtk::Align::Center)
        .tooltip_text(tooltip)
        .css_classes(["spotify-logo"])
        .build()
}

/// The Spotify icon for songs that play from Spotify.
pub fn spotify_icon() -> gtk::Image {
    spotify_logo(&gettextrs::gettext("Plays from Spotify"))
}

/// The artist or album page of a Spotify song (`uri`): the library's when
/// the library has them, else Spotify's. Spotify is asked which they are.
pub fn open_song_page(uri: String, artist: bool) {
    use gettextrs::gettext;
    use gtk::prelude::*;

    use crate::application::JewelcaseApplication;
    use crate::window::JewelcaseWindow;

    gtk::glib::spawn_future_local(async move {
        let app = JewelcaseApplication::get();
        let spotify = app.spotify();
        let unanswered = || {
            app.library()
                .emit_by_name::<()>("message", &[&gettext("Spotify can’t be asked right now")]);
        };
        let Some(links) = spotify.track_links(&uri).await else {
            return unanswered();
        };
        let Some(window) = app.active_window().and_downcast::<JewelcaseWindow>() else {
            return;
        };
        if artist {
            let Some((id, name)) = links.artists.first() else {
                return;
            };
            match app.library().artist_for_spotify(id, name) {
                Some(local) => {
                    let _ = WidgetExt::activate_action(
                        &window,
                        "win.show-artist",
                        Some(&local.to_variant()),
                    );
                }
                None => window.show_spotify_artist(id),
            }
            return;
        }
        let Some(album_id) = links.album_id else {
            return;
        };
        let Some(album) = spotify.album_by_id(&album_id).await else {
            return unanswered();
        };
        match app.library().local_album_for(&album) {
            Some(local) => {
                let _ = WidgetExt::activate_action(
                    &window,
                    "win.show-album",
                    Some(&local.album().id.to_variant()),
                );
            }
            None => window.show_spotify_album(&album),
        }
    });
}

#[cfg(test)]
mod tests {
    /// The panels and the player have no ×: a header bar that hides the
    /// title buttons on the right hides them on the left too, where a
    /// button layout such as `close:` puts the ×.
    #[test]
    fn no_close_button_on_either_side() {
        let ui = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../data/resources/ui");
        let mut one_sided = Vec::new();
        for entry in std::fs::read_dir(&ui).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "blp") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let start = text.matches("show-start-title-buttons: false;").count();
            let end = text.matches("show-end-title-buttons: false;").count();
            if start != end {
                one_sided.push(path.file_name().unwrap().to_string_lossy().into_owned());
            }
        }
        assert!(
            one_sided.is_empty(),
            "one side's title buttons hidden: {one_sided:?}"
        );
    }
}
