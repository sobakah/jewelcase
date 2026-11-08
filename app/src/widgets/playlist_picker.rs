//! "Add to Playlist…" and the name dialog for new or renamed playlists.

use std::collections::HashMap;

use adw::prelude::*;
use gettextrs::{gettext, ngettext};
use gtk::glib;

use crate::application::GatefoldApplication;
use crate::objects::PlaylistObject;

/// Asks for a playlist name; `None` if cancelled or empty.
pub async fn ask_name(
    parent: &impl IsA<gtk::Widget>,
    heading: &str,
    initial: &str,
    confirm: &str,
) -> Option<String> {
    ask_name_with_option(parent, heading, initial, confirm, None)
        .await
        .map(|(name, _)| name)
}

/// Like `ask_name`, with a check box (`option`: its label and default).
pub async fn ask_name_with_option(
    parent: &impl IsA<gtk::Widget>,
    heading: &str,
    initial: &str,
    confirm: &str,
    option: Option<(&str, bool)>,
) -> Option<(String, bool)> {
    let entry = gtk::Entry::builder()
        .text(initial)
        .activates_default(true)
        .build();
    let check = option.map(|(label, active)| {
        gtk::CheckButton::builder()
            .label(label)
            .active(active)
            .build()
    });
    let extra = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .build();
    extra.append(&entry);
    if let Some(check) = &check {
        extra.append(check);
    }
    let dialog = adw::AlertDialog::builder()
        .heading(heading)
        .extra_child(&extra)
        .close_response("cancel")
        .default_response("ok")
        .build();
    dialog.add_responses(&[("cancel", &gettext("_Cancel")), ("ok", confirm)]);
    dialog.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    let update = glib::clone!(
        #[weak]
        dialog,
        move |entry: &gtk::Entry| {
            dialog.set_response_enabled("ok", !entry.text().trim().is_empty());
        }
    );
    update(&entry);
    entry.connect_changed(update);
    entry.grab_focus();
    let response = dialog.choose_future(Some(parent)).await;
    let name = entry.text().trim().to_owned();
    let checked = check.is_some_and(|c| c.is_active());
    (response == "ok" && !name.is_empty()).then_some((name, checked))
}

/// Saves `tracks` as a new playlist `name`; if one with that name exists,
/// asks whether to add them to it instead (or create another anyway).
pub async fn create_or_add(parent: &impl IsA<gtk::Widget>, name: &str, tracks: &[i64]) {
    let library = GatefoldApplication::get().library();
    let existing = {
        let store = library.playlists();
        (0..store.n_items())
            .filter_map(|i| store.item(i).and_downcast::<PlaylistObject>())
            .find(|p| p.playlist().name.trim().to_lowercase() == name.trim().to_lowercase())
    };
    let Some(existing) = existing else {
        library.create_playlist(name, tracks);
        return;
    };
    let dialog = adw::AlertDialog::builder()
        // Translators: {} is a playlist name.
        .heading(gettext("“{}” Already Exists").replace("{}", &existing.playlist().name))
        .body(gettext(
            "Add the songs to that playlist, or create another playlist with the same name?",
        ))
        .close_response("cancel")
        .default_response("add")
        .build();
    dialog.add_responses(&[
        ("cancel", &gettext("_Cancel")),
        ("new", &gettext("Create _New")),
        ("add", &gettext("_Add to Playlist")),
    ]);
    dialog.set_response_appearance("add", adw::ResponseAppearance::Suggested);
    match dialog.choose_future(Some(parent)).await.as_str() {
        "add" => library.add_to_playlist(existing.id(), tracks, true),
        "new" => {
            library.create_playlist(name, tracks);
        }
        _ => {}
    }
}

/// Lets the user pick a playlist (or create one) for `tracks`.
pub fn show(parent: &impl IsA<gtk::Widget>, tracks: Vec<i64>) {
    if tracks.is_empty() {
        return;
    }
    let library = GatefoldApplication::get().library();
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .valign(gtk::Align::Start)
        .css_classes(["boxed-list"])
        .build();
    let new_row = adw::ActionRow::builder()
        .title(gettext("New Playlist…"))
        .activatable(true)
        .build();
    new_row.add_prefix(&gtk::Image::from_icon_name("list-add-symbolic"));
    list.append(&new_row);
    // The songs' entries in each playlist (as themselves, or as their
    // Spotify versions), and which songs those are.
    let mut entries: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut has: HashMap<i64, Vec<i64>> = HashMap::new();
    if let Some(db) = library.db() {
        for track in &tracks {
            for (playlist, entry) in db.song_entries(*track, None).unwrap_or_default() {
                entries.entry(playlist).or_default().push(entry);
                let songs = has.entry(playlist).or_default();
                if !songs.contains(track) {
                    songs.push(*track);
                }
            }
        }
    }
    let has_all = |playlist: i64| has.get(&playlist).is_some_and(|s| s.len() >= tracks.len());
    // The playlists that have them come first: choosing one removes them.
    let mut playlists: Vec<PlaylistObject> = {
        let store = library.playlists();
        (0..store.n_items())
            .filter_map(|i| store.item(i).and_downcast::<PlaylistObject>())
            .collect()
    };
    playlists.sort_by_key(|p| !has_all(p.id()));
    for playlist in &playlists {
        let row = adw::ActionRow::builder()
            .use_markup(false)
            .activatable(true)
            .build();
        row.set_title(&playlist.playlist().name);
        let count = crate::format::song_count(playlist.playlist().item_count);
        let n = has.get(&playlist.id()).map_or(0, Vec::len);
        let subtitle = if n == 0 {
            count
        } else if has_all(playlist.id()) {
            let (note, tooltip) = if tracks.len() == 1 {
                (
                    gettext("Has this song"),
                    gettext("Remove the Song From This Playlist"),
                )
            } else {
                (
                    gettext("Has all of these songs"),
                    gettext("Remove the Songs From This Playlist"),
                )
            };
            row.add_suffix(
                &gtk::Image::builder()
                    .icon_name("object-select-symbolic")
                    .css_classes(["accent"])
                    .build(),
            );
            row.set_tooltip_text(Some(&tooltip));
            format!("{note} · {count}")
        } else {
            // Translators: {} is a number of songs, e.g. "Has 3 of these songs".
            let note = ngettext("Has {} of these songs", "Has {} of these songs", n as u32)
                .replace("{}", &n.to_string());
            row.set_tooltip_text(Some(&gettext("Add the Songs It Doesn't Have Yet")));
            format!("{note} · {count}")
        };
        row.set_subtitle(&subtitle);
        list.append(&row);
    }

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .child(
                &adw::Clamp::builder()
                    .margin_start(12)
                    .margin_end(12)
                    .margin_bottom(12)
                    .child(&list)
                    .build(),
            )
            .build(),
    ));
    let dialog = adw::Dialog::builder()
        .title(gettext("Add to Playlist"))
        .content_width(400)
        .child(&toolbar)
        .build();

    list.connect_row_activated(glib::clone!(
        #[weak]
        dialog,
        move |_, row| {
            let library = GatefoldApplication::get().library();
            match row.index() {
                0 => {
                    let tracks = tracks.clone();
                    glib::spawn_future_local(async move {
                        let name =
                            ask_name(&dialog, &gettext("New Playlist"), "", &gettext("_Create"))
                                .await;
                        if let Some(name) = name {
                            create_or_add(&dialog, &name, &tracks).await;
                            dialog.close();
                        }
                    });
                }
                index => {
                    if let Some(playlist) = playlists.get(index as usize - 1) {
                        let id = playlist.id();
                        let had = has.get(&id).cloned().unwrap_or_default();
                        if had.len() >= tracks.len() {
                            // All there: chosen again, they're removed.
                            let items = entries.get(&id).cloned().unwrap_or_default();
                            library.remove_playlist_items(id, &items);
                        } else {
                            // Only those it doesn't have yet.
                            let missing: Vec<i64> = tracks
                                .iter()
                                .copied()
                                .filter(|t| !had.contains(t))
                                .collect();
                            library.add_to_playlist(id, &missing, true);
                        }
                    }
                    dialog.close();
                }
            }
        }
    ));
    dialog.present(Some(parent));
}
