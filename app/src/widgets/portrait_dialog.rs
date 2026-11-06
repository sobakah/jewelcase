//! "Choose From Online Sources…": a grid of portrait candidates from every
//! provider (CONCEPT.md §8).

use adw::prelude::*;
use gatefold_core::library::covers::CoverSize;
use gettextrs::gettext;
use gtk::glib;

use crate::application::GatefoldApplication;
use crate::covers;

pub fn show(parent: &impl IsA<gtk::Widget>, artist_id: i64) {
    let library = GatefoldApplication::get().library();
    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .min_children_per_line(2)
        .max_children_per_line(4)
        .column_spacing(12)
        .row_spacing(12)
        .valign(gtk::Align::Start)
        .build();
    let status = adw::StatusPage::builder().vexpand(true).build();
    let stack = gtk::Stack::new();
    stack.add_named(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&flow)
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
    stack.set_visible_child_name("loading");
    stack.set_margin_start(12);
    stack.set_margin_end(12);
    stack.set_margin_bottom(12);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&stack));
    let dialog = adw::Dialog::builder()
        .title(gettext("Choose Artist Image"))
        .content_width(560)
        .content_height(520)
        .child(&toolbar)
        .build();

    let show_status = glib::clone!(
        #[weak]
        stack,
        #[weak]
        status,
        move |icon: &str, title: String, description: Option<String>| {
            status.set_icon_name(Some(icon));
            status.set_title(&title);
            status.set_description(description.as_deref());
            stack.set_visible_child_name("status");
        }
    );

    if !library.online_enabled() {
        show_status(
            "network-offline-symbolic",
            gettext("Online Lookups Are Off"),
            Some(gettext("Turn on “Look Up Artists Online” in Preferences.")),
        );
        dialog.present(Some(parent));
        return;
    }

    glib::spawn_future_local(glib::clone!(
        #[weak]
        flow,
        #[weak]
        stack,
        #[weak]
        dialog,
        async move {
            let library = GatefoldApplication::get().library();
            match library.portrait_candidates(artist_id).await {
                Ok(candidates) if !candidates.is_empty() => {
                    for (key, candidate) in &candidates {
                        let avatar = adw::Avatar::builder().size(120).build();
                        let texture = covers::portrait(key, CoverSize::Grid);
                        avatar.set_custom_image(texture.as_ref());
                        let tooltip = candidate
                            .attribution
                            .clone()
                            .unwrap_or_else(|| candidate.source.to_owned());
                        avatar.set_tooltip_text(Some(&tooltip));
                        flow.append(&avatar);
                    }
                    flow.connect_child_activated(move |_, child| {
                        if let Some((key, _)) = candidates.get(child.index() as usize) {
                            GatefoldApplication::get()
                                .library()
                                .choose_portrait(artist_id, Some(key));
                            dialog.close();
                        }
                    });
                    stack.set_visible_child_name("results");
                }
                Ok(_) => show_status(
                    "image-missing-symbolic",
                    gettext("No Images Found"),
                    Some(gettext(
                        "Online sources need the artist's MusicBrainz ID. Adding a fanart.tv API key in Preferences finds more images.",
                    )),
                ),
                Err(err) => {
                    tracing::warn!("Portrait candidates failed: {err}");
                    show_status(
                        "network-error-symbolic",
                        gettext("Images Couldn't Be Loaded"),
                        Some(gettext("Check the network connection")),
                    );
                }
            }
        }
    ));
    dialog.present(Some(parent));
}
