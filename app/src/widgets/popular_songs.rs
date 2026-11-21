//! An artist page's Popular section (CONCEPT.md §4.2): the artist's songs
//! you played most, and (optional) their most popular songs on Deezer
//! beside them, which is all it shows when you haven't played the artist.
//! Spotify has no artist top tracks for apps in Development Mode (§9.1).
//!
//! Deezer's songs are matched with the library; one that isn't there is
//! found on Spotify only when it's played (its ISRC from Deezer, then one
//! search), so a page costs no Spotify requests.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gettextrs::{gettext, ngettext};
use gtk::glib;
use jewelcase_core::matching::{Confidence, RemoteTrack};
use jewelcase_metadata::DeezerTrack;

use super::CoverImage;
use crate::application::JewelcaseApplication;
use crate::config::APP_ID;
use crate::format;
use crate::objects::TrackObject;

/// The `popular-songs` setting's values, in the order of Preferences' row.
pub const POPULAR_CHOICES: [&str; 3] = ["off", "local", "deezer"];

/// Songs in each column.
const SHOWN: usize = 5;

/// What an artist page needs for its Popular section.
pub struct Artist {
    /// A library artist (their songs and albums); `None` on a Spotify
    /// artist's page (only streamed songs, by name).
    pub id: Option<i64>,
    pub name: String,
    /// Where their Deezer artist is kept: their Spotify ID, else "name:"
    /// and the name.
    pub deezer_key: String,
    /// Their album titles, to tell artists of one name apart on Deezer.
    pub titles: Vec<String>,
}

impl Artist {
    pub fn deezer_key(spotify_id: Option<&str>, name: &str) -> String {
        spotify_id.map_or_else(|| format!("name:{}", name.to_lowercase()), str::to_owned)
    }
}

/// The section, or `None` when it's off or (without Deezer) the artist
/// hasn't been played. Deezer's column comes when Deezer answers; until
/// then (or without an answer) the section is hidden if it has nothing
/// else; one taken off the page meanwhile isn't filled.
pub fn section(artist: Artist) -> Option<gtk::Widget> {
    let settings = gtk::gio::Settings::new(APP_ID);
    let mode = settings.string("popular-songs");
    if mode == "off" {
        return None;
    }
    let library = JewelcaseApplication::get().library();
    let played = library.most_played(artist.id, &artist.name, SHOWN);
    let deezer = mode == "deezer" && library.online_enabled();
    if played.is_empty() && !deezer {
        return None;
    }
    let section = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .visible(!played.is_empty())
        .build();
    section.append(
        &gtk::Label::builder()
            .label(gettext("Popular"))
            .xalign(0.0)
            .css_classes(["title-4"])
            .build(),
    );
    // Side by side where there's room, else one above the other.
    let columns = Columns::default();
    section.append(&columns);
    if !played.is_empty() {
        columns.append(&played_column(&artist.name, played));
    }
    if deezer {
        let section = section.downgrade();
        let columns = columns.downgrade();
        let name = artist.name.clone();
        glib::spawn_future_local(async move {
            let found = JewelcaseApplication::get()
                .library()
                .popular_songs(artist.deezer_key, artist.name, artist.titles)
                .await;
            let songs = match found {
                Ok(songs) => songs,
                Err(err) => {
                    tracing::info!("Cannot read {name}'s popular songs from Deezer: {err}");
                    return;
                }
            };
            if songs.is_empty() {
                return;
            }
            if let (Some(section), Some(columns)) = (section.upgrade(), columns.upgrade()) {
                columns.append(&deezer_column(songs));
                section.set_visible(true);
            }
        });
    }
    Some(section.upcast())
}

/// A column: its heading and a boxed list of songs.
fn column(title: &str, list: &gtk::ListBox) -> gtk::Widget {
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .valign(gtk::Align::Start)
        .build();
    column.append(
        &gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .css_classes(["dim-label", "caption-heading"])
            .build(),
    );
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("boxed-list");
    column.append(list);
    column.upcast()
}

/// A song's row: cover, title and a line under it, and `end` (the ⋮ menu
/// goes after it, in the returned line).
fn song_row(
    cover_key: Option<&str>,
    title: &str,
    streamed: bool,
    subtitle: Option<&str>,
    end: &str,
) -> (gtk::ListBoxRow, gtk::Box, CoverImage) {
    let cover: CoverImage = glib::Object::builder()
        .property("min-size", 40)
        .property("natural-size", 40)
        .build();
    cover.set_cover(cover_key);
    let line = gtk::Box::builder()
        .spacing(12)
        .margin_start(6)
        .margin_end(12)
        .margin_top(9)
        .margin_bottom(9)
        .build();
    line.append(&cover);
    line.append(&super::song_lines(title, streamed, subtitle));
    line.append(
        &gtk::Label::builder()
            .label(end)
            .css_classes(["dim-label", "numeric"])
            .build(),
    );
    let row = gtk::ListBoxRow::builder().child(&line).build();
    super::tooltip_when_cut(&row);
    (row, line, cover)
}

/// "Your Most Played": a click plays them from there.
fn played_column(name: &str, played: Vec<(TrackObject, u32)>) -> gtk::Widget {
    let list = gtk::ListBox::new();
    for (track, plays) in &played {
        // Translators: how often you played a song, on an artist page.
        let plays_text = ngettext("{} play", "{} plays", *plays).replace("{}", &plays.to_string());
        let (row, line, _) = song_row(
            track.cover_key(),
            &track.title(),
            track.is_stand_in(),
            track.track().album_title.as_deref(),
            &plays_text,
        );
        // The queue's song menu (as for Recently Played).
        line.append(&super::queue_view::song_menu(
            &row,
            track,
            super::queue_view::SongRow::Played,
        ));
        list.append(&row);
    }
    let tracks: Vec<TrackObject> = played.into_iter().map(|(track, _)| track).collect();
    let name = name.to_owned();
    list.connect_row_activated(move |_, row| {
        JewelcaseApplication::get().player().play_tracks(
            tracks.clone(),
            Some(row.index() as usize),
            false,
            Some(name.clone()),
        );
    });
    column(&gettext("Your Most Played"), &list)
}

/// A Deezer song: the library's copy if it has one, else (once found) its
/// Spotify version to stream.
struct DeezerSong {
    song: DeezerTrack,
    track: RefCell<Option<TrackObject>>,
    row: gtk::ListBoxRow,
    line: gtk::Box,
    /// Its ⋮ button: the queue's song menu once the song is known, else
    /// one that finds it on Spotify first.
    menu: RefCell<Option<gtk::MenuButton>>,
}

impl DeezerSong {
    /// The library's copy, or the Spotify version (found once: its ISRC
    /// from Deezer, then a search); `None` with a message if not found.
    async fn track(self: &Rc<Self>) -> Option<TrackObject> {
        if let Some(track) = self.track.borrow().clone() {
            return Some(track);
        }
        let app = JewelcaseApplication::get();
        let song = &self.song;
        let isrc = app.library().deezer_isrc(song.id).await.ok().flatten();
        let found = app
            .spotify()
            .find_song(
                song.title.clone(),
                song.artist.clone(),
                song.album.clone(),
                song.duration_ms,
                isrc,
            )
            .await;
        match &found {
            Some(track) => {
                self.track.replace(Some(track.clone()));
                self.set_menu(Some(track));
            }
            None => app.spotify().message(
                // Translators: {} is a song title.
                &gettext("“{}” wasn’t found on Spotify").replace("{}", &song.title),
            ),
        }
        found
    }

    /// Its ⋮ button for `track` (known), or one that finds it first.
    fn set_menu(self: &Rc<Self>, track: Option<&TrackObject>) {
        if let Some(old) = self.menu.take() {
            self.line.remove(&old);
        }
        let button = match track {
            Some(track) => {
                super::queue_view::song_menu(&self.row, track, super::queue_view::SongRow::Played)
            }
            None => self.finding_menu(),
        };
        self.line.append(&button);
        self.menu.replace(Some(button));
    }

    /// Play Next, Add to Queue and Add to Playlist… for a song not found
    /// yet: each finds it on Spotify first.
    fn finding_menu(self: &Rc<Self>) -> gtk::MenuButton {
        let actions = gtk::gio::SimpleActionGroup::new();
        let menu = gtk::gio::Menu::new();
        for (name, label) in [
            ("play-next", gettext("Play _Next")),
            ("add-to-queue", gettext("Add to _Queue")),
            ("add-to-playlist", gettext("Add to _Playlist…")),
        ] {
            let action = gtk::gio::SimpleAction::new(name, None);
            let entry = Rc::downgrade(self);
            action.connect_activate(move |_, _| {
                let Some(entry) = entry.upgrade() else {
                    return;
                };
                glib::spawn_future_local(async move {
                    let Some(track) = entry.track().await else {
                        return;
                    };
                    let player = JewelcaseApplication::get().player();
                    match name {
                        "play-next" => player.play_next(vec![track]),
                        "add-to-queue" => player.add_to_queue(vec![track]),
                        _ => super::playlist_picker::show(&entry.row, vec![track.id()]),
                    }
                });
            });
            actions.add_action(&action);
            menu.append(Some(&label), Some(&format!("found.{name}")));
        }
        self.row.insert_action_group("found", Some(&actions));
        let button = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .valign(gtk::Align::Center)
            .menu_model(&menu)
            .css_classes(["flat", "circular"])
            .build();
        super::set_accessible_label(&button, &gettext("More"));
        button
    }
}

/// "Popular on Deezer": library songs play from the library; the others
/// from Spotify, looked up when clicked.
fn deezer_column(songs: Vec<DeezerTrack>) -> gtk::Widget {
    let library = JewelcaseApplication::get().library();
    let index = library.match_index();
    let spotify = JewelcaseApplication::get().spotify();
    let list = gtk::ListBox::new();
    let mut entries = Vec::new();
    let mut downloads = Vec::new();
    for song in songs {
        let local = index
            .find(&RemoteTrack {
                title: song.title.clone(),
                artists: vec![song.artist.clone()],
                album: song.album.clone(),
                duration_ms: song.duration_ms,
                isrc: None,
            })
            .filter(|(_, confidence)| *confidence == Confidence::Sure)
            .and_then(|(id, _)| library.tracks(&[id]).remove(&id))
            .map(TrackObject::new);
        let streamable = local.is_some() || spotify.is_connected();
        let cover_key = local
            .as_ref()
            .and_then(|t| t.cover_key().map(str::to_owned))
            .or_else(|| {
                song.cover_url
                    .as_deref()
                    .map(crate::spotify::image_cover_key)
            });
        let subtitle = if streamable {
            song.album.clone()
        } else {
            Some(gettext("Not in your library; connect Spotify to play it"))
        };
        let (row, line, cover) = song_row(
            cover_key.as_deref(),
            &song.title,
            local.is_none() && streamable,
            subtitle.as_deref(),
            &format::duration(song.duration_ms),
        );
        row.set_activatable(streamable);
        if local.is_none()
            && let Some(url) = song.cover_url.clone()
        {
            downloads.push((url, cover, cover_key));
        }
        list.append(&row);
        let entry = Rc::new(DeezerSong {
            song,
            track: RefCell::new(local.clone()),
            row,
            line,
            menu: RefCell::new(None),
        });
        if streamable {
            entry.set_menu(local.as_ref());
        }
        entries.push(entry);
    }
    for (url, cover, key) in downloads {
        crate::spotify::fetch_covers(vec![url], move || cover.set_cover(key.as_deref()));
    }
    list.connect_row_activated(move |_, row| {
        let Some(entry) = entries.get(row.index() as usize).cloned() else {
            return;
        };
        glib::spawn_future_local(async move {
            if let Some(track) = entry.track().await {
                JewelcaseApplication::get().player().play_now(track);
            }
        });
    });
    column(&gettext("Popular on Deezer"), &list)
}

/// Narrower than this, the columns go one above the other.
const COLUMN_MIN: i32 = 300;
const COLUMN_SPACING: i32 = 18;

mod columns_imp {
    use gtk::subclass::prelude::*;

    use super::*;

    /// One or two columns, side by side when each gets `COLUMN_MIN`, else
    /// stacked; each as wide as there is room. (A flow box went by the
    /// columns' natural width, which long titles make wide: stacked on
    /// pages with room for both.)
    #[derive(Default)]
    pub struct Columns;

    #[glib::object_subclass]
    impl ObjectSubclass for Columns {
        const NAME: &'static str = "JewelcasePopularColumns";
        type Type = super::Columns;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Columns {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl Columns {
        fn children(&self) -> Vec<gtk::Widget> {
            let mut children = Vec::new();
            let mut child = self.obj().first_child();
            while let Some(widget) = child {
                child = widget.next_sibling();
                children.push(widget);
            }
            children
        }
    }

    impl WidgetImpl for Columns {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let children = self.children();
            if orientation == gtk::Orientation::Horizontal {
                let min = children
                    .iter()
                    .map(|c| c.measure(orientation, -1).0)
                    .max()
                    .unwrap_or(0);
                let natural: i32 = children.iter().map(|c| c.measure(orientation, -1).1).sum();
                return (min, natural.max(min), -1, -1);
            }
            let heights = |width: i32| {
                children
                    .iter()
                    .map(|c| {
                        let (min, natural, _, _) = c.measure(orientation, width);
                        (min, natural)
                    })
                    .collect::<Vec<_>>()
            };
            match super::side_by_side(children.len(), for_size) {
                // The taller column.
                Some(width) => {
                    let heights = heights(width);
                    let min = heights.iter().map(|h| h.0).max().unwrap_or(0);
                    let natural = heights.iter().map(|h| h.1).max().unwrap_or(0);
                    (min, natural, -1, -1)
                }
                // One above the other.
                None => {
                    let heights = heights(for_size);
                    let gaps = COLUMN_SPACING * (heights.len() as i32 - 1).max(0);
                    let min = heights.iter().map(|h| h.0).sum::<i32>() + gaps;
                    let natural = heights.iter().map(|h| h.1).sum::<i32>() + gaps;
                    (min, natural, -1, -1)
                }
            }
        }

        fn size_allocate(&self, width: i32, _height: i32, _baseline: i32) {
            let children = self.children();
            match super::side_by_side(children.len(), width) {
                Some(column) => {
                    for (i, child) in children.iter().enumerate() {
                        let height = child.measure(gtk::Orientation::Vertical, column).0;
                        let x = i as i32 * (column + COLUMN_SPACING);
                        child.allocate(
                            column,
                            height,
                            -1,
                            Some(
                                gtk::gsk::Transform::new()
                                    .translate(&gtk::graphene::Point::new(x as f32, 0.0)),
                            ),
                        );
                    }
                }
                None => {
                    let mut y = 0;
                    for child in &children {
                        let height = child.measure(gtk::Orientation::Vertical, width).0;
                        child.allocate(
                            width,
                            height,
                            -1,
                            Some(
                                gtk::gsk::Transform::new()
                                    .translate(&gtk::graphene::Point::new(0.0, y as f32)),
                            ),
                        );
                        y += height + COLUMN_SPACING;
                    }
                }
            }
        }
    }
}

glib::wrapper! {
    pub struct Columns(ObjectSubclass<columns_imp::Columns>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for Columns {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl Columns {
    fn append(&self, child: &impl IsA<gtk::Widget>) {
        child.set_parent(self);
        self.queue_resize();
    }
}

/// Each column's width when `count` columns go side by side in `width`;
/// `None` when they go one above the other (one column, or too narrow).
fn side_by_side(count: usize, width: i32) -> Option<i32> {
    if count < 2 || width < 0 {
        return None;
    }
    let count = count as i32;
    let column = (width - COLUMN_SPACING * (count - 1)) / count;
    (column >= COLUMN_MIN).then_some(column)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two columns side by side when each gets 300 px, else one above the
    /// other; one column is never split.
    #[test]
    fn columns_go_side_by_side_when_both_have_room() {
        assert_eq!(side_by_side(2, 700), Some(341));
        assert_eq!(side_by_side(2, 617), None);
        assert_eq!(side_by_side(2, 618), Some(300));
        assert_eq!(side_by_side(1, 2000), None);
        assert_eq!(side_by_side(2, -1), None);
    }
}
