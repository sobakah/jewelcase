//! "Identify Artist…": pick the right MusicBrainz artist when the automatic
//! lookup wasn't sure (CONCEPT.md §8).

use adw::prelude::*;
use gettextrs::gettext;
use gtk::glib;

use crate::application::GatefoldApplication;
use crate::objects::ArtistObject;

pub fn show(parent: &impl IsA<gtk::Widget>, artist: &ArtistObject) {
    let library = GatefoldApplication::get().library();
    let artist_id = artist.id();

    let entry = gtk::SearchEntry::builder()
        .text(&artist.artist().name)
        .hexpand(true)
        .build();
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .valign(gtk::Align::Start)
        .css_classes(["boxed-list"])
        .build();
    let status = adw::StatusPage::builder().vexpand(true).build();
    let stack = gtk::Stack::new();
    stack.add_named(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list)
            .build(),
        Some("results"),
    );
    stack.add_named(&status, Some("status"));
    stack.add_named(
        &adw::Spinner::builder()
            .width_request(32)
            .height_request(32)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build(),
        Some("loading"),
    );

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_start(12)
        .margin_end(12)
        .margin_bottom(12)
        .build();
    content.append(&entry);
    content.append(&stack);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&content));
    let dialog = adw::Dialog::builder()
        .title(gettext("Identify Artist"))
        .content_width(460)
        .content_height(560)
        .child(&toolbar)
        .build();

    if !library.online_enabled() {
        status.set_icon_name(Some("network-offline-symbolic"));
        status.set_title(&gettext("Online Lookups Are Off"));
        status.set_description(Some(&gettext(
            "Turn on “Look Up Artists Online” in Preferences to identify artists.",
        )));
        stack.set_visible_child_name("status");
        entry.set_sensitive(false);
        dialog.present(Some(parent));
        return;
    }

    // Only the latest search shows (an earlier answer can come later).
    let searches = std::rc::Rc::new(std::cell::Cell::new(0u64));
    let search = glib::clone!(
        #[weak]
        list,
        #[weak]
        stack,
        #[weak]
        status,
        #[weak]
        dialog,
        move |name: String| {
            stack.set_visible_child_name("loading");
            searches.set(searches.get() + 1);
            let this = searches.get();
            let searches = searches.clone();
            glib::spawn_future_local(async move {
                let library = GatefoldApplication::get().library();
                let candidates = library.search_artists(&name).await;
                if searches.get() != this {
                    return;
                }
                list.remove_all();
                match candidates {
                    Ok(candidates) if !candidates.is_empty() => {
                        for candidate in candidates {
                            let details: Vec<String> = [
                                candidate.disambiguation.clone(),
                                candidate.kind.clone(),
                                candidate.country.clone(),
                            ]
                            .into_iter()
                            .flatten()
                            .collect();
                            let row = {
                                let row = adw::ActionRow::builder()
                                    .use_markup(false)
                                    .activatable(true)
                                    .build();
                                row.set_title(&candidate.name);
                                row.set_subtitle(&details.join(" · "));
                                row
                            };
                            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
                            let mbid = candidate.mbid.clone();
                            row.connect_activated(glib::clone!(
                                #[weak]
                                dialog,
                                move |_| {
                                    GatefoldApplication::get()
                                        .library()
                                        .lookup_again(artist_id, Some(&mbid));
                                    dialog.close();
                                }
                            ));
                            list.append(&row);
                        }
                        stack.set_visible_child_name("results");
                    }
                    Ok(_) => {
                        status.set_icon_name(Some("edit-find-symbolic"));
                        status.set_title(&gettext("No Artists Found"));
                        status.set_description(None);
                        stack.set_visible_child_name("status");
                    }
                    Err(err) => {
                        tracing::warn!("Artist search failed: {err}");
                        status.set_icon_name(Some("network-error-symbolic"));
                        status.set_title(&gettext("Search Failed"));
                        status.set_description(Some(&gettext("Check the network connection")));
                        stack.set_visible_child_name("status");
                    }
                }
            });
        }
    );
    entry.connect_activate(glib::clone!(
        #[strong]
        search,
        move |entry| search(entry.text().to_string())
    ));
    search(artist.artist().name.clone());
    dialog.present(Some(parent));
}
