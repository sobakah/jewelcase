//! The feed of new releases under the bell in the header bar (CONCEPT.md
//! §9.8): the followed artists' albums, EPs and singles of the last 30
//! days, newest first; the ones not seen before marked with a dot.

use adw::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use jewelcase_core::db::NewRelease;

use super::CoverImage;

/// When a release came out ("today", "3 days ago"), from its date
/// (YYYY-MM-DD); the date itself if it can't be read.
pub fn release_day(date: &str) -> String {
    let parts: Vec<i32> = date.split('-').filter_map(|p| p.parse().ok()).collect();
    match parts[..] {
        [year, month, day] => glib::DateTime::from_local(year, month, day, 12, 0, 0.0).map_or_else(
            |_| date.to_owned(),
            |d| crate::format::days_ago(d.to_unix()),
        ),
        _ => date.to_owned(),
    }
}

/// The small dot of something new (style.css: .new-dot).
pub fn new_dot() -> gtk::Box {
    gtk::Box::builder()
        .valign(gtk::Align::Center)
        .can_target(false)
        .css_classes(["new-dot"])
        .build()
}

/// Fills the popover with `releases`; `open` opens one (after the
/// popover closed).
pub fn fill(popover: &gtk::Popover, releases: &[NewRelease], open: impl Fn(NewRelease) + 'static) {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .width_request(360)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(6)
        .margin_end(6)
        .build();
    content.append(
        &gtk::Label::builder()
            .label(gettext("New Releases"))
            .xalign(0.0)
            .margin_start(6)
            .css_classes(["heading"])
            .build(),
    );
    if releases.is_empty() {
        content.append(
            &gtk::Label::builder()
                .label(gettext(
                    "Nothing new from the artists you follow in the last 30 days",
                ))
                .wrap(true)
                .xalign(0.0)
                .margin_start(6)
                .margin_end(6)
                .margin_bottom(6)
                .css_classes(["dim-label"])
                .build(),
        );
        popover.set_child(Some(&content));
        return;
    }
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    for release in releases {
        list.append(&row(release));
    }
    let releases = releases.to_vec();
    list.connect_row_activated(glib::clone!(
        #[weak]
        popover,
        move |_, row| {
            if let Some(release) = releases.get(row.index() as usize) {
                popover.popdown();
                open(release.clone());
            }
        }
    ));
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .max_content_height(440)
        .child(&list)
        .build();
    content.append(&scroller);
    popover.set_child(Some(&content));
}

fn row(release: &NewRelease) -> adw::ActionRow {
    // Texts after building: in the builder they'd be read as markup.
    let row = adw::ActionRow::builder()
        .use_markup(false)
        .activatable(true)
        .build();
    row.set_title(&release.title);
    // Translators: an artist, and when they released it ("today",
    // "3 days ago").
    row.set_subtitle(
        &gettext("{artist} · {when}")
            .replace("{artist}", &release.artist)
            .replace("{when}", &release_day(&release.release_date)),
    );
    let cover: CoverImage = glib::Object::builder()
        .property("min-size", 40)
        .property("natural-size", 40)
        .build();
    cover.set_cover(release.cover_key().as_deref());
    row.add_prefix(&cover);
    if !release.seen {
        let dot = new_dot();
        dot.update_property(&[gtk::accessible::Property::Label(&gettext("New"))]);
        row.add_suffix(&dot);
    }
    row
}
