//! *Change Cover…* of a playlist: the covers of the albums in it, or an
//! image file (as Jewelcase Mobile's cover sheet). *Use Automatic Cover*
//! stays in the playlist's menu.

use adw::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use jewelcase_core::db::PlaylistItem;

use super::CoverImage;
use crate::application::JewelcaseApplication;

/// At most this many albums' covers are offered.
const MAX_COVERS: usize = 60;

/// The covers of the albums among a playlist's songs, each once, in the
/// playlist's order: (cover key, album title). A library song brings its
/// album's cover; a Spotify song Spotify's, once downloaded (`cached`).
pub fn album_covers(
    entries: &[PlaylistItem],
    cached: impl Fn(&str) -> bool,
) -> Vec<(String, String)> {
    let mut covers: Vec<(String, String)> = Vec::new();
    for entry in entries {
        let found = match &entry.track {
            Some(track) => track
                .album_cover_key
                .clone()
                .or_else(|| track.record.cover_key.clone())
                .map(|key| {
                    (
                        key,
                        track
                            .album_title
                            .clone()
                            .or_else(|| track.record.tags.album.clone()),
                    )
                }),
            None => entry
                .image_url
                .as_deref()
                .map(crate::spotify::image_cover_key)
                .filter(|key| cached(key))
                .map(|key| (key, entry.album.clone())),
        };
        let Some((key, album)) = found else { continue };
        if covers.iter().any(|(k, _)| *k == key) {
            continue;
        }
        covers.push((key, album.unwrap_or_default()));
        if covers.len() == MAX_COVERS {
            break;
        }
    }
    covers
}

/// `choose_file` opens the file chooser (the dialog is closed first).
pub fn show(
    parent: &impl IsA<gtk::Widget>,
    playlist_id: i64,
    covers: Vec<(String, String)>,
    choose_file: impl Fn() + 'static,
) {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_start(12)
        .margin_end(12)
        .margin_bottom(18)
        .build();
    content.append(
        &gtk::Label::builder()
            .label(gettext("Albums in This Playlist"))
            .xalign(0.0)
            .margin_start(6)
            .css_classes(["heading"])
            .build(),
    );
    if covers.is_empty() {
        content.append(
            &gtk::Label::builder()
                .label(gettext(
                    "Songs from your library or from Spotify bring their album’s cover.",
                ))
                .xalign(0.0)
                .wrap(true)
                .margin_start(6)
                .css_classes(["dim-label"])
                .build(),
        );
    }
    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .min_children_per_line(2)
        .max_children_per_line(4)
        .column_spacing(12)
        .row_spacing(12)
        .valign(gtk::Align::Start)
        .visible(!covers.is_empty())
        .build();
    for (key, album) in &covers {
        let cover: CoverImage = glib::Object::builder()
            .property("min-size", 110)
            .property("natural-size", 110)
            .build();
        cover.set_cover(Some(key));
        if !album.is_empty() {
            cover.set_tooltip_text(Some(album));
            super::set_accessible_label(&cover, album);
        }
        let child = gtk::FlowBoxChild::builder()
            .child(&cover)
            .css_classes(["cover-choice"])
            .build();
        flow.append(&child);
    }
    content.append(&flow);
    let file = gtk::Button::builder()
        .label(gettext("Choose _Image File…"))
        .use_underline(true)
        .halign(gtk::Align::Center)
        .margin_top(6)
        .css_classes(["pill"])
        .build();
    content.append(&file);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .child(&content)
            .build(),
    ));
    let dialog = adw::Dialog::builder()
        .title(gettext("Choose Playlist Cover"))
        .content_width(560)
        .child(&toolbar)
        .build();
    flow.connect_child_activated(glib::clone!(
        #[weak]
        dialog,
        move |_, child| {
            if let Some((key, _)) = covers.get(child.index() as usize) {
                JewelcaseApplication::get()
                    .library()
                    .use_playlist_cover(playlist_id, key);
                dialog.close();
            }
        }
    ));
    file.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| {
            dialog.close();
            choose_file();
        }
    ));
    dialog.present(Some(parent));
}

#[cfg(test)]
mod tests {
    use super::*;
    use jewelcase_core::db::Track;

    fn entry(track: Option<Track>, image_url: Option<&str>, album: &str) -> PlaylistItem {
        PlaylistItem {
            id: 0,
            position: 0,
            track,
            title: None,
            artist: None,
            album: Some(album.into()),
            duration_ms: None,
            match_state: "local".into(),
            spotify_uri: None,
            suggested_track: None,
            suggested_spotify: None,
            image_url: image_url.map(str::to_owned),
            added: None,
        }
    }

    fn track(cover: &str, album: &str) -> Track {
        Track {
            record: jewelcase_core::library::TrackRecord {
                id: 1,
                path: "/music/song.flac".into(),
                tags: Default::default(),
                cover_key: None,
                added: 0,
            },
            album_id: Some(1),
            album_title: Some(album.into()),
            album_artist: None,
            album_cover_key: Some(cover.into()),
            album_disc: None,
        }
    }

    #[test]
    fn each_album_is_offered_once_in_the_playlists_order() {
        let spotify = "https://i.scdn.co/image/abc";
        let downloaded = crate::spotify::image_cover_key(spotify);
        let entries = [
            entry(Some(track("cb", "B")), None, ""),
            entry(Some(track("ca", "A")), None, ""),
            entry(Some(track("cb", "B")), None, ""),
            // A Spotify song: its cover once downloaded; another isn't yet.
            entry(None, Some(spotify), "S"),
            entry(None, Some("https://i.scdn.co/image/later"), "L"),
            entry(None, None, "None"),
        ];
        let covers = album_covers(&entries, |key| key == downloaded);
        assert_eq!(
            covers,
            vec![
                ("cb".to_owned(), "B".to_owned()),
                ("ca".to_owned(), "A".to_owned()),
                (downloaded, "S".to_owned()),
            ]
        );
    }
}
