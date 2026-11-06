//! The queue pane of the player (CONCEPT.md §6.2): sections *Now Playing*,
//! *Next Up* (added by the user) and *Next From: <album>*, reordering by
//! drag and drop, removing by button or right-click, "Clear".

use adw::prelude::*;
use adw::subclass::prelude::*;
use gatefold_core::queue::Origin;
use gettextrs::{gettext, ngettext};
use gtk::{gdk, glib};

use super::CoverImage;
use crate::application::GatefoldApplication;
use crate::format;
use crate::objects::{QueueEntry, TrackObject};
use crate::player::Player;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct QueueView {
        pub list: gtk::ListBox,
        pub history: gtk::ListBox,
        pub history_box: gtk::Box,
        pub scroller: gtk::ScrolledWindow,
        /// The songs the history rows show, in row order.
        pub history_tracks: std::cell::RefCell<Vec<TrackObject>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for QueueView {
        const NAME: &'static str = "GatefoldQueueView";
        type Type = super::QueueView;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for QueueView {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup();
        }
    }

    impl WidgetImpl for QueueView {}
    impl BinImpl for QueueView {}
}

glib::wrapper! {
    pub struct QueueView(ObjectSubclass<imp::QueueView>)
        @extends gtk::Widget, adw::Bin,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl QueueView {
    fn setup(&self) {
        let imp = self.imp();
        let player = GatefoldApplication::get().player();
        self.add_css_class("queue-view");

        let header = gtk::Box::builder().spacing(6).build();
        header.append(
            &gtk::Label::builder()
                .label(gettext("Queue"))
                .xalign(0.0)
                .hexpand(true)
                .css_classes(["heading"])
                .build(),
        );
        let save = gtk::Button::builder()
            .label(gettext("_Save"))
            .use_underline(true)
            .tooltip_text(gettext("Save as Playlist"))
            .css_classes(["flat"])
            .build();
        save.connect_clicked(glib::clone!(
            #[weak]
            player,
            move |button| {
                let button = button.clone();
                glib::spawn_future_local(async move {
                    let history = player.history(HISTORY_SHOWN);
                    // Translators: {} is a number of songs.
                    let option = ngettext(
                        "Include the recently played song ({})",
                        "Include the recently played songs ({})",
                        history.len() as u32,
                    )
                    .replace("{}", &history.len().to_string());
                    let answer = super::playlist_picker::ask_name_with_option(
                        &button,
                        &gettext("Save Queue as Playlist"),
                        "",
                        &gettext("_Save"),
                        (!history.is_empty()).then_some((option.as_str(), false)),
                    )
                    .await;
                    if let Some((name, with_history)) = answer {
                        let mut ids: Vec<i64> = if with_history {
                            history.iter().map(TrackObject::id).collect()
                        } else {
                            Vec::new()
                        };
                        ids.extend(player.queued_track_ids());
                        super::playlist_picker::create_or_add(&button, &name, &ids).await;
                    }
                });
            }
        ));
        header.append(&save);
        header.append(
            &gtk::Button::builder()
                .label(gettext("_Clear"))
                .use_underline(true)
                .tooltip_text(gettext("Clear Upcoming Songs"))
                .action_name("player.clear-queue")
                .css_classes(["flat"])
                .build(),
        );

        imp.list.set_selection_mode(gtk::SelectionMode::None);
        imp.list.set_valign(gtk::Align::Start);
        imp.list.add_css_class("boxed-list");
        imp.list.add_css_class("reorderable");
        super::reorder::guard_sheet(&imp.list);
        imp.list.bind_model(
            Some(player.queue_model()),
            glib::clone!(
                #[weak]
                player,
                #[upgrade_or_panic]
                move |item| {
                    let entry = item.downcast_ref::<QueueEntry>().expect("queue entries");
                    queue_row(&player, entry)
                }
            ),
        );
        imp.list.set_header_func(glib::clone!(
            #[weak]
            player,
            move |row, before| section_header(&player, row, before)
        ));
        imp.list.connect_row_activated(glib::clone!(
            #[weak]
            player,
            move |_, row| {
                if row.index() > 0 {
                    player.jump_to_upcoming(row.index() as usize);
                }
            }
        ));

        // Recently played, above "Now Playing": scroll up to see it.
        imp.history.set_selection_mode(gtk::SelectionMode::None);
        imp.history.add_css_class("boxed-list");
        imp.history.connect_row_activated(glib::clone!(
            #[weak]
            player,
            #[weak(rename_to = view)]
            self,
            move |_, row| {
                let track = view
                    .imp()
                    .history_tracks
                    .borrow()
                    .get(row.index() as usize)
                    .cloned();
                if let Some(track) = track {
                    player.play_now(track);
                }
            }
        ));
        imp.history_box.set_orientation(gtk::Orientation::Vertical);
        imp.history_box.set_spacing(6);
        imp.history_box.append(
            &gtk::Label::builder()
                .label(gettext("Recently Played"))
                .xalign(0.0)
                .margin_start(12)
                .margin_top(6)
                .css_classes(["dim-label", "caption-heading"])
                .build(),
        );
        imp.history_box.append(&imp.history);
        let lists = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .valign(gtk::Align::Start)
            .build();
        lists.append(&imp.history_box);
        lists.append(&imp.list);
        imp.scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        imp.scroller.set_vexpand(true);
        imp.scroller.set_child(Some(&lists));
        let scroller = imp.scroller.clone();
        player.connect_local(
            "history-changed",
            false,
            glib::clone!(
                #[weak(rename_to = view)]
                self,
                #[upgrade_or]
                None,
                move |_| {
                    view.fill_history();
                    view.scroll_to_now_playing();
                    None
                }
            ),
        );
        // Opening the queue shows "Now Playing" at the top.
        self.connect_map(|view| {
            view.fill_history();
            view.scroll_to_now_playing();
        });
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();
        content.append(&header);
        content.append(&scroller);
        self.set_child(Some(&content));
    }

    fn fill_history(&self) {
        let imp = self.imp();
        let tracks = GatefoldApplication::get().player().history(HISTORY_SHOWN);
        imp.history.remove_all();
        for track in &tracks {
            imp.history.append(&history_row(track));
        }
        imp.history_box.set_visible(!tracks.is_empty());
        imp.history_tracks.replace(tracks);
    }

    fn scroll_to_now_playing(&self) {
        let view = self.downgrade();
        // After the lists are laid out.
        glib::idle_add_local_once(move || {
            let Some(view) = view.upgrade() else { return };
            let imp = view.imp();
            // "Now Playing" sits below the latest history, but never lower
            // than the middle of the view.
            let adjustment = imp.scroller.vadjustment();
            let history = if imp.history_box.is_visible() {
                f64::from(imp.history_box.height() + 12)
            } else {
                0.0
            };
            adjustment.set_value((history - adjustment.page_size() / 2.0).max(0.0));
        });
    }
}

/// History entries shown above "Now Playing".
const HISTORY_SHOWN: usize = 30;

fn history_row(track: &TrackObject) -> gtk::ListBoxRow {
    let cover: CoverImage = glib::Object::builder()
        .property("min-size", 40)
        .property("natural-size", 40)
        .build();
    cover.set_cover(track.cover_key());
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .valign(gtk::Align::Center)
        .hexpand(true)
        .build();
    text.append(
        &gtk::Label::builder()
            .label(track.title())
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build(),
    );
    if let Some(artist) = track.artist() {
        text.append(
            &gtk::Label::builder()
                .label(artist)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(["dim-label", "caption"])
                .build(),
        );
    }
    let line = gtk::Box::builder()
        .spacing(12)
        .margin_start(12)
        .margin_end(12)
        .margin_top(8)
        .margin_bottom(8)
        .build();
    line.append(&cover);
    line.append(&text);
    line.append(
        &gtk::Label::builder()
            .label(format::duration(track.duration_ms()))
            .css_classes(["dim-label", "numeric"])
            .build(),
    );
    let row = gtk::ListBoxRow::builder().child(&line).build();
    row.add_css_class("history-row");
    row.set_tooltip_text(Some(&gettext("Play Now")));
    row
}

/// The entry a row shows: rows are in model order.
fn entry_of(player: &Player, row: &gtk::ListBoxRow) -> Option<QueueEntry> {
    player
        .queue_model()
        .item(u32::try_from(row.index()).ok()?)
        .and_downcast()
}

fn section_header(player: &Player, row: &gtk::ListBoxRow, before: Option<&gtk::ListBoxRow>) {
    let section = |row: &gtk::ListBoxRow| -> u8 {
        match entry_of(player, row).map(|e| (e.entry().offset, e.entry().origin)) {
            Some((0, _)) => 0,
            Some((_, Origin::User)) => 1,
            _ => 2,
        }
    };
    let current = section(row);
    if before.is_some_and(|b| section(b) == current) {
        row.set_header(None::<&gtk::Widget>);
        return;
    }
    let label = match current {
        0 => gettext("Now Playing"),
        1 => gettext("Next Up"),
        _ => match player.context_title() {
            // Translators: {} is an album title.
            Some(title) => gettext("Next From: {}").replace("{}", &title),
            None => gettext("Next"),
        },
    };
    row.set_header(Some(
        &gtk::Label::builder()
            .label(label)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .margin_start(12)
            .margin_end(12)
            .margin_top(12)
            .margin_bottom(6)
            .css_classes(["dim-label", "caption-heading"])
            .build(),
    ));
}

fn queue_row(player: &Player, entry: &QueueEntry) -> gtk::Widget {
    let offset = entry.entry().offset;
    let track = &entry.entry().track;
    let cover: CoverImage = glib::Object::builder()
        .property("min-size", 40)
        .property("natural-size", 40)
        .build();
    cover.set_cover(track.cover_key());
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .valign(gtk::Align::Center)
        .hexpand(true)
        .build();
    text.append(
        &gtk::Label::builder()
            .label(track.title())
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build(),
    );
    if let Some(artist) = track.artist() {
        text.append(
            &gtk::Label::builder()
                .label(artist)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(["dim-label", "caption"])
                .build(),
        );
    }
    let line = gtk::Box::builder()
        .spacing(12)
        .margin_start(6)
        .margin_end(6)
        .margin_top(9)
        .margin_bottom(9)
        .build();
    if offset > 0 {
        line.append(
            &gtk::Image::builder()
                .icon_name("list-drag-handle-symbolic")
                .css_classes(["dim-label", "drag-handle"])
                .build(),
        );
    }
    line.append(&cover);
    line.append(&text);
    line.append(
        &gtk::Label::builder()
            .label(format::duration(track.duration_ms()))
            .css_classes(["dim-label", "numeric"])
            .build(),
    );
    let row = gtk::ListBoxRow::builder().child(&line).build();
    if offset == 0 {
        row.add_css_class("now-playing");
        row.set_activatable(false);
        return row.upcast();
    }

    let remove = gtk::Button::builder()
        .icon_name("list-remove-symbolic")
        .tooltip_text(gettext("Remove from Queue"))
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular"])
        .build();
    remove.connect_clicked(glib::clone!(
        #[weak]
        player,
        move |_| player.remove_upcoming(offset)
    ));
    line.append(&remove);

    // Right-click: remove.
    let menu = gtk::gio::Menu::new();
    menu.append(
        Some(&gettext("_Remove from Queue")),
        Some("queue-row.remove"),
    );
    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.set_parent(&row);
    popover.set_has_arrow(false);
    let actions = gtk::gio::SimpleActionGroup::new();
    let remove_action = gtk::gio::SimpleAction::new("remove", None);
    remove_action.connect_activate(glib::clone!(
        #[weak]
        player,
        move |_, _| player.remove_upcoming(offset)
    ));
    actions.add_action(&remove_action);
    row.insert_action_group("queue-row", Some(&actions));
    let click = gtk::GestureClick::builder()
        .button(gdk::BUTTON_SECONDARY)
        .build();
    click.connect_pressed(glib::clone!(
        #[weak]
        popover,
        move |_, _, x, y| {
            popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            popover.popup();
        }
    ));
    row.add_controller(click);
    row.connect_destroy(move |_| popover.unparent());

    // Drag and drop to reorder (§6.2), within the row's section.
    super::reorder::attach(
        &row,
        offset as u32,
        glib::clone!(
            #[weak]
            player,
            move |from, to| player.move_upcoming(from as usize, to as usize)
        ),
    );
    row.upcast()
}

impl Default for QueueView {
    fn default() -> Self {
        glib::Object::new()
    }
}
