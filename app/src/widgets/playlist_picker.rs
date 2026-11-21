//! "Add to Playlist…" and the name dialog for new or renamed playlists.

use std::collections::HashMap;

use adw::prelude::*;
use gettextrs::{gettext, ngettext};
use gtk::glib;

use crate::application::JewelcaseApplication;
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
    let library = JewelcaseApplication::get().library();
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

/// The setting that keeps the list's order (see `sorting::compare_picker`).
const SORT_KEY: &str = "playlist-picker-sort";

/// Lets the user pick a playlist (or create one) for `tracks`: a popover
/// at `parent` (the button or row it was chosen from), closed by a click
/// beside it like Jewelcase's other menus (a dialog stayed open).
pub fn show(parent: &impl IsA<gtk::Widget>, tracks: Vec<i64>) {
    if tracks.is_empty() {
        return;
    }
    // New Playlist's questions: over the window (the popover is closed by
    // then, and a row it pointed at may be gone).
    let Some(window) = parent.root().and_upcast::<gtk::Widget>() else {
        return;
    };
    let library = JewelcaseApplication::get().library();
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
    let playlists: Vec<PlaylistObject> = {
        let store = library.playlists();
        (0..store.n_items())
            .filter_map(|i| store.item(i).and_downcast::<PlaylistObject>())
            .collect()
    };
    // Each playlist's row, made once; the sort only orders them.
    let mut rows: Vec<(i64, adw::ActionRow)> = Vec::new();
    // What they're sorted by: has some of the songs, name, last change.
    let mut keys: Vec<(bool, String, i64)> = Vec::new();
    for playlist in &playlists {
        let info = playlist.playlist();
        let row = adw::ActionRow::builder()
            .use_markup(false)
            .activatable(true)
            .build();
        row.set_title(&info.name);
        let count = crate::format::song_count(info.item_count);
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
        keys.push((n > 0, info.name.clone(), info.modified));
        rows.push((playlist.id(), row));
    }
    let rows = std::rc::Rc::new(rows);

    // The playlists that have some of the songs first (choosing one that
    // has them all removes them), each group in the chosen order.
    let arrange = {
        let list = list.clone();
        let rows = rows.clone();
        move |sort: &str| {
            let key = |i: usize| crate::sorting::PickerKey {
                has: keys[i].0,
                name: &keys[i].1,
                changed: keys[i].2,
            };
            let mut order: Vec<usize> = (0..rows.len()).collect();
            order.sort_by(|&a, &b| crate::sorting::compare_picker(sort, &key(a), &key(b)));
            for (_, row) in rows.iter() {
                if row.parent().is_some() {
                    list.remove(row);
                }
            }
            for i in order {
                list.append(&rows[i].1);
            }
        }
    };
    let settings = gtk::gio::Settings::new(crate::config::APP_ID);
    let sort = settings.string(SORT_KEY);
    arrange(&sort);
    let (by, reversed) = match sort.strip_suffix("-reversed") {
        Some(by) => (by.to_owned(), true),
        None => (sort.to_string(), false),
    };
    // The sort menu, as the playlist panel's: by what, and reversed.
    let by_action =
        gtk::gio::SimpleAction::new_stateful("by", Some(glib::VariantTy::STRING), &by.to_variant());
    let reversed_action =
        gtk::gio::SimpleAction::new_stateful("reversed", None, &reversed.to_variant());
    let apply = {
        let settings = settings.clone();
        let by_action = by_action.clone();
        let reversed_action = reversed_action.clone();
        move || {
            let by = by_action
                .state()
                .and_then(|s| s.get::<String>())
                .unwrap_or_default();
            let reversed = reversed_action
                .state()
                .and_then(|s| s.get::<bool>())
                .unwrap_or(false);
            let sort = if reversed {
                format!("{by}-reversed")
            } else {
                by
            };
            // Remembered for the next time.
            let _ = settings.set_string(SORT_KEY, &sort);
            arrange(&sort);
        }
    };
    let apply = std::rc::Rc::new(apply);
    by_action.connect_activate({
        let apply = apply.clone();
        move |action, target| {
            if let Some(target) = target {
                action.set_state(target);
                apply();
            }
        }
    });
    reversed_action.connect_activate(move |action, _| {
        let reversed = action
            .state()
            .and_then(|s| s.get::<bool>())
            .unwrap_or(false);
        action.set_state(&(!reversed).to_variant());
        apply();
    });
    let actions = gtk::gio::SimpleActionGroup::new();
    actions.add_action(&by_action);
    actions.add_action(&reversed_action);
    let sort_menu = gtk::gio::Menu::new();
    let by_section = gtk::gio::Menu::new();
    for (label, target) in [
        (gettext("Last _Changed"), "changed"),
        (gettext("_Name"), "name"),
    ] {
        let item = gtk::gio::MenuItem::new(Some(&label), None);
        item.set_action_and_target_value(Some("picker.by"), Some(&target.to_variant()));
        by_section.append_item(&item);
    }
    sort_menu.append_section(None, &by_section);
    let reverse_section = gtk::gio::Menu::new();
    reverse_section.append(Some(&gettext("_Reverse Order")), Some("picker.reversed"));
    sort_menu.append_section(None, &reverse_section);
    let sort_button = gtk::MenuButton::builder()
        .icon_name("view-sort-descending-symbolic")
        .tooltip_text(gettext("Sort"))
        .menu_model(&sort_menu)
        .css_classes(["flat", "circular"])
        .build();

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(6)
        .margin_end(6)
        .build();
    content.insert_action_group("picker", Some(&actions));
    let title = gtk::CenterBox::new();
    title.set_center_widget(Some(
        &gtk::Label::builder()
            .label(gettext("Add to Playlist"))
            .css_classes(["heading"])
            .build(),
    ));
    title.set_end_widget(Some(&sort_button));
    content.append(&title);
    content.append(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(420)
            .min_content_width(320)
            .child(&list)
            .build(),
    );
    let popover = gtk::Popover::builder().child(&content).build();
    popover.set_parent(parent);
    popover.connect_closed(|popover| {
        // Not while it's still closing.
        let popover = popover.clone();
        glib::idle_add_local_once(move || popover.unparent());
    });

    list.connect_row_activated(glib::clone!(
        #[weak]
        popover,
        move |_, row| {
            let library = JewelcaseApplication::get().library();
            match row.index() {
                0 => {
                    popover.popdown();
                    let tracks = tracks.clone();
                    let window = window.clone();
                    glib::spawn_future_local(async move {
                        let name =
                            ask_name(&window, &gettext("New Playlist"), "", &gettext("_Create"))
                                .await;
                        if let Some(name) = name {
                            create_or_add(&window, &name, &tracks).await;
                        }
                    });
                }
                _ => {
                    if let Some(id) = rows
                        .iter()
                        .find(|(_, r)| r.upcast_ref::<gtk::ListBoxRow>() == row)
                        .map(|(id, _)| *id)
                    {
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
                    popover.popdown();
                }
            }
        }
    ));
    popover.popup();
}
