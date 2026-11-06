//! "Add to Playlist…" and the name dialog for new or renamed playlists.

use adw::prelude::*;
use gettextrs::gettext;
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
    let entry = gtk::Entry::builder()
        .text(initial)
        .activates_default(true)
        .build();
    let dialog = adw::AlertDialog::builder()
        .heading(heading)
        .extra_child(&entry)
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
    (response == "ok" && !name.is_empty()).then_some(name)
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
    let playlists: Vec<PlaylistObject> = {
        let store = library.playlists();
        (0..store.n_items())
            .filter_map(|i| store.item(i).and_downcast::<PlaylistObject>())
            .collect()
    };
    for playlist in &playlists {
        let row = adw::ActionRow::builder()
            .title(&playlist.playlist().name)
            .subtitle(crate::format::song_count(playlist.playlist().item_count))
            .use_markup(false)
            .activatable(true)
            .build();
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
                            GatefoldApplication::get()
                                .library()
                                .create_playlist(&name, &tracks);
                            dialog.close();
                        }
                    });
                }
                index => {
                    if let Some(playlist) = playlists.get(index as usize - 1) {
                        library.add_to_playlist(playlist.id(), &tracks, true);
                    }
                    dialog.close();
                }
            }
        }
    ));
    dialog.present(Some(parent));
}
