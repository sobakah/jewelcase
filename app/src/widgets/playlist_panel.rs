//! A playlist in the side panel (CONCEPT.md §4.1, §9.5).

use std::cell::RefCell;
use std::sync::OnceLock;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gatefold_core::db::PlaylistItem;
use gettextrs::{gettext, ngettext};
use gtk::glib::subclass::Signal;
use gtk::{gio, glib};

use super::playlist_picker::ask_name;
use super::{CoverBackdrop, CoverImage};
use crate::application::GatefoldApplication;
use crate::format;
use crate::objects::{PlaylistObject, TrackObject};

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/sobakah/Gatefold/ui/playlist_panel.ui")]
    pub struct PlaylistPanel {
        #[template_child]
        pub cover: TemplateChild<CoverImage>,
        #[template_child]
        pub backdrop: TemplateChild<CoverBackdrop>,
        #[template_child]
        pub name: TemplateChild<gtk::Label>,
        #[template_child]
        pub name_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub name_entry: TemplateChild<gtk::Entry>,
        /// The name may be edited here (own playlists, not linked).
        pub renamable: std::cell::Cell<bool>,
        #[template_child]
        pub add_search: TemplateChild<gtk::SearchEntry>,
        #[template_child]
        pub add_results: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub meta: TemplateChild<gtk::Label>,
        #[template_child]
        pub availability: TemplateChild<gtk::Label>,
        #[template_child]
        pub source: TemplateChild<gtk::Label>,
        #[template_child]
        pub hidden: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub empty: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub items: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub arrange_banner: TemplateChild<adw::Banner>,
        /// A Spotify playlist's songs can be dragged (see `arrange`).
        pub arranging: std::cell::Cell<bool>,
        /// The rows are to be built again (they can be dragged now, or
        /// no longer).
        pub rebuild: std::cell::Cell<bool>,
        pub playlist: RefCell<Option<PlaylistObject>>,
        pub entries: RefCell<Vec<PlaylistItem>>,
        /// Rows are built a batch per idle moment (playlists can have
        /// thousands of songs); a newer refresh stops an older fill.
        pub fill_generation: std::cell::Cell<u64>,
        /// Covers of Spotify-only songs, by image URL, filled in as they
        /// download.
        pub spotify_covers:
            RefCell<std::collections::HashMap<String, Vec<glib::WeakRef<CoverImage>>>>,
        /// How the playlist is listed: (by, reversed); see `Sort`.
        pub sort: RefCell<(String, bool)>,
        /// The "sort.by" and "sort.reversed" actions, to show the sort.
        pub sort_actions: std::cell::OnceCell<(gio::SimpleAction, gio::SimpleAction)>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PlaylistPanel {
        const NAME: &'static str = "GatefoldPlaylistPanel";
        type Type = super::PlaylistPanel;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            CoverImage::ensure_type();
            CoverBackdrop::ensure_type();
            klass.bind_template();
            klass.install_action("playlist.close", None, |panel, _, _| {
                panel.finish_arranging();
                panel.emit_by_name::<()>("close", &[]);
            });
            klass.install_action_async("playlist.rename-spotify", None, |panel, _, _| async move {
                panel.rename_on_spotify().await;
            });
            klass.install_action("playlist.arrange", None, |panel, _, _| {
                panel.arrange();
            });
            klass.install_action("playlist.play", None, |panel, _, _| panel.play(None, false));
            klass.install_action("playlist.play-next", None, |panel, _, _| {
                let tracks = panel.queueable();
                if !tracks.is_empty() {
                    GatefoldApplication::get().player().play_next(tracks);
                }
            });
            klass.install_action("playlist.add-to-queue", None, |panel, _, _| {
                let tracks = panel.queueable();
                if !tracks.is_empty() {
                    GatefoldApplication::get().player().add_to_queue(tracks);
                }
            });
            klass.install_action("playlist.add-to-playlist", None, |panel, _, _| {
                let ids: Vec<i64> = entry_track_ids(&panel.imp().entries.borrow())
                    .into_iter()
                    .flatten()
                    .collect();
                if !ids.is_empty() {
                    super::super::playlist_picker::show(panel, ids);
                }
            });
            klass.install_action("playlist.shuffle", None, |panel, _, _| {
                panel.play(None, true)
            });
            klass.install_action_async("playlist.rename", None, |panel, _, _| async move {
                panel.rename().await;
            });
            klass.install_action_async("playlist.change-cover", None, |panel, _, _| async move {
                panel.change_cover().await;
            });
            klass.install_action("playlist.reset-cover", None, |panel, _, _| {
                if let Some(id) = panel.playlist_id() {
                    GatefoldApplication::get()
                        .library()
                        .reset_playlist_cover(id);
                }
            });
            klass.install_action_async("playlist.export", None, |panel, _, _| async move {
                panel.export().await;
            });
            klass.install_action_async("playlist.delete", None, |panel, _, _| async move {
                panel.delete().await;
            });
            klass.install_action_async("playlist.publish", None, |panel, _, _| async move {
                let Some(id) = panel.playlist_id() else {
                    return;
                };
                GatefoldApplication::get()
                    .spotify()
                    .publish(id, &panel)
                    .await;
            });
            klass.install_action("playlist.refresh", None, |panel, _, _| {
                if let Some(id) = panel.playlist_id() {
                    GatefoldApplication::get().spotify().refresh_playlist(id);
                }
            });
            klass.install_action("playlist.open-spotify", None, |panel, _, _| {
                if let Some(id) = panel.spotify_id() {
                    crate::spotify::open_on_spotify("playlist", &id);
                }
            });
            klass.install_action("playlist.duplicate", None, |panel, _, _| {
                panel.duplicate();
            });
            klass.install_action_async("playlist.unlink", None, |panel, _, _| async move {
                panel.unlink().await;
            });
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for PlaylistPanel {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| vec![Signal::builder("close").build()])
        }

        fn constructed(&self) {
            self.parent_constructed();
            self.backdrop.follow(&self.cover);
            let obj = self.obj();
            self.items.connect_row_activated(glib::clone!(
                #[weak]
                obj,
                move |_, row| obj.activate_entry(row.index() as usize)
            ));
            // Renaming by clicking the name.
            let click = gtk::GestureClick::new();
            click.connect_released(glib::clone!(
                #[weak]
                obj,
                move |_, _, _, _| obj.start_renaming()
            ));
            self.name.add_controller(click);
            self.name_entry.connect_activate(glib::clone!(
                #[weak]
                obj,
                move |_| obj.finish_renaming(true)
            ));
            let focus = gtk::EventControllerFocus::new();
            focus.connect_leave(glib::clone!(
                #[weak]
                obj,
                move |_| obj.finish_renaming(true)
            ));
            self.name_entry.add_controller(focus);
            let keys = gtk::EventControllerKey::new();
            keys.connect_key_pressed(glib::clone!(
                #[weak]
                obj,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |_, key, _, _| {
                    if key == gtk::gdk::Key::Escape {
                        obj.finish_renaming(false);
                        glib::Propagation::Stop
                    } else {
                        glib::Propagation::Proceed
                    }
                }
            ));
            self.name_entry.add_controller(keys);
            self.add_search.connect_search_changed(glib::clone!(
                #[weak]
                obj,
                move |entry| obj.search_songs(&entry.text())
            ));
            self.add_search
                .connect_stop_search(|entry| entry.set_text(""));
            self.arrange_banner.connect_button_clicked(glib::clone!(
                #[weak]
                obj,
                move |_| obj.finish_arranging()
            ));
            // Hidden (another view, the window closing): the new order goes.
            obj.connect_unmap(|obj| obj.finish_arranging());
            // Locked while its changes are being sent.
            GatefoldApplication::get()
                .spotify()
                .connect_sending_notify(glib::clone!(
                    #[weak]
                    obj,
                    move |_| obj.refresh()
                ));
            obj.setup_sort();
            GatefoldApplication::get().library().connect_local(
                "playlists-changed",
                false,
                glib::clone!(
                    #[weak]
                    obj,
                    #[upgrade_or]
                    None,
                    move |_| {
                        obj.refresh();
                        None
                    }
                ),
            );
        }
    }

    impl WidgetImpl for PlaylistPanel {}
    impl BinImpl for PlaylistPanel {}
}

glib::wrapper! {
    pub struct PlaylistPanel(ObjectSubclass<imp::PlaylistPanel>)
        @extends gtk::Widget, adw::Bin,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl PlaylistPanel {
    pub fn playlist_id(&self) -> Option<i64> {
        self.imp()
            .playlist
            .borrow()
            .as_ref()
            .map(PlaylistObject::id)
    }

    pub fn set_playlist(&self, playlist: &PlaylistObject) {
        let imp = self.imp();
        let changed = self.playlist_id() != Some(playlist.id());
        if changed {
            self.finish_arranging();
            // Another playlist: its rows from scratch (in batches).
            imp.rebuild.set(true);
        }
        imp.playlist.replace(Some(playlist.clone()));
        if changed {
            imp.add_search.set_text("");
        }
        self.refresh();
        // Spotify's version may be newer.
        if changed && playlist.playlist().spotify_id.is_some() {
            GatefoldApplication::get()
                .spotify()
                .refresh_playlist(playlist.id());
            self.fetch_spotify_covers();
        }
        // A new, empty playlist: go straight to finding songs.
        if changed && playlist.playlist().item_count == 0 && playlist.playlist().editable {
            imp.add_search.grab_focus();
        }
    }

    /// Covers of the Spotify-only songs, in the background; the rows show
    /// them once they're here.
    fn fetch_spotify_covers(&self) {
        let urls: Vec<String> = self
            .imp()
            .entries
            .borrow()
            .iter()
            .filter(|e| e.track.is_none())
            .filter_map(|e| e.image_url.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let panel = self.downgrade();
        crate::spotify::fetch_covers_in_batches(urls, move |done| {
            let Some(panel) = panel.upgrade() else { return };
            let covers = panel.imp().spotify_covers.borrow();
            for url in done {
                let key = crate::spotify::image_cover_key(&url);
                for cover in covers.get(&url).into_iter().flatten() {
                    if let Some(cover) = cover.upgrade() {
                        cover.set_cover(Some(&key));
                    }
                }
            }
        });
    }

    fn spotify_id(&self) -> Option<String> {
        self.imp()
            .playlist
            .borrow()
            .as_ref()
            .and_then(|p| p.playlist().spotify_id.clone())
    }

    /// Types `text` into the "Add Songs" field (screenshot hook).
    pub fn search_for_songs(&self, text: &str) {
        self.imp().add_search.set_text(text);
    }

    fn is_renaming(&self) -> bool {
        self.imp().name_stack.visible_child_name().as_deref() == Some("entry")
    }

    /// The name turns into a text field.
    fn start_renaming(&self) {
        let imp = self.imp();
        if !imp.renamable.get() || self.is_renaming() {
            return;
        }
        imp.name_entry.set_text(&imp.name.text());
        imp.name_stack.set_visible_child_name("entry");
        imp.name_entry.grab_focus();
    }

    /// Back to the name; `save`: rename to what was typed.
    fn finish_renaming(&self, save: bool) {
        let imp = self.imp();
        if !self.is_renaming() {
            return;
        }
        imp.name_stack.set_visible_child_name("label");
        let Some(playlist) = imp.playlist.borrow().clone() else {
            return;
        };
        let name = imp.name_entry.text().trim().to_owned();
        if !save || name.is_empty() || name == playlist.playlist().name {
            return;
        }
        imp.name.set_text(&name);
        GatefoldApplication::get()
            .library()
            .rename_playlist(playlist.id(), &name);
    }

    /// Library songs matching `query`, each with a button to add it.
    fn search_songs(&self, query: &str) {
        let imp = self.imp();
        imp.add_results.remove_all();
        let query = query.trim();
        let Some(playlist_id) = self.playlist_id().filter(|_| !query.is_empty()) else {
            imp.add_results.set_visible(false);
            return;
        };
        let library = GatefoldApplication::get().library();
        let ids = library.search(query, 8).tracks;
        let tracks = library.tracks(&ids);
        for id in &ids {
            let Some(track) = tracks.get(id).cloned().map(TrackObject::new) else {
                continue;
            };
            let row = adw::ActionRow::builder()
                .use_markup(false)
                .title(track.title())
                .subtitle(
                    [track.artist(), track.track().album_title.clone()]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" — "),
                )
                .build();
            let cover: CoverImage = glib::Object::builder()
                .property("min-size", 40)
                .property("natural-size", 40)
                .build();
            cover.set_cover(track.cover_key());
            cover.set_valign(gtk::Align::Center);
            row.add_prefix(&cover);
            let add = gtk::Button::builder()
                .icon_name("list-add-symbolic")
                .tooltip_text(gettext("Add to Playlist"))
                .valign(gtk::Align::Center)
                .css_classes(["flat", "circular"])
                .build();
            let track_id = track.id();
            add.connect_clicked(move |button| {
                GatefoldApplication::get().library().add_to_playlist(
                    playlist_id,
                    &[track_id],
                    false,
                );
                // Added: show it, and don't add it twice by accident.
                button.set_icon_name("object-select-symbolic");
                button.set_sensitive(false);
            });
            row.add_suffix(&add);
            row.set_activatable_widget(Some(&add));
            imp.add_results.append(&row);
        }
        imp.add_results
            .set_visible(imp.add_results.first_child().is_some());
    }

    /// Shows the current state of the playlist, or closes when it's gone.
    fn refresh(&self) {
        let imp = self.imp();
        let Some(id) = self.playlist_id() else { return };
        let library = GatefoldApplication::get().library();
        let Some(playlist) = library.playlist(id) else {
            imp.playlist.replace(None);
            self.emit_by_name::<()>("close", &[]);
            return;
        };
        let info = playlist.playlist();
        imp.cover.set_cover(playlist.cover_key().as_deref());
        if !self.is_renaming() {
            imp.name.set_text(&info.name);
        }
        imp.meta.set_label(&format!(
            "{} · {}",
            format::song_count(info.item_count),
            format::length(info.duration_ms)
        ));
        // "38 of 50 songs play from your library" (§9.5).
        let partial = info.local_count < info.item_count;
        imp.availability.set_visible(partial);
        imp.availability.set_label(
            &ngettext(
                "{local} of {total} song plays from your library",
                "{local} of {total} songs play from your library",
                info.item_count,
            )
            .replace("{local}", &info.local_count.to_string())
            .replace("{total}", &info.item_count.to_string()),
        );
        // Spotify doesn't say what a followed playlist holds.
        imp.meta.set_visible(info.readable);
        let sending = GatefoldApplication::get().spotify().is_sending(id);
        imp.source.set_label(&source_line(info, sending));
        imp.source.set_visible(info.spotify_id.is_some());
        let editable = info.editable;
        let connected = GatefoldApplication::get().spotify().is_connected();
        let linked = info.spotify_id.is_some();
        let mirror = info.source == "spotify";
        // Spotify playlists are renamed and reordered from the menu only,
        // so they aren't changed by accident.
        let renamable = editable && !linked;
        imp.renamable.set(renamable);
        imp.name.set_cursor_from_name(renamable.then_some("text"));
        if !renamable {
            self.finish_renaming(false);
        }
        imp.name
            .set_tooltip_text(renamable.then(|| gettext("Rename")).as_deref());
        imp.add_search.set_visible(editable);
        imp.add_search.set_sensitive(!sending);
        imp.arrange_banner.set_button_label(Some(&if sending {
            gettext("Sending…")
        } else {
            gettext("_Done")
        }));
        self.action_set_enabled("playlist.rename", renamable);
        self.action_set_enabled("playlist.rename-spotify", editable && linked);
        self.action_set_enabled(
            "playlist.arrange",
            editable && linked && info.readable && !imp.arranging.get(),
        );
        // Mirrors: a new cover goes to Spotify too (owned ones); Spotify
        // has no "back to automatic", so there's no reset for them.
        self.action_set_enabled("playlist.change-cover", !mirror || editable);
        self.action_set_enabled("playlist.reset-cover", !mirror);
        self.action_set_enabled("playlist.publish", connected && !linked);
        self.action_set_enabled("playlist.refresh", connected && linked);
        self.action_set_enabled("playlist.open-spotify", linked);
        self.action_set_enabled("playlist.duplicate", mirror && info.readable);
        self.action_set_enabled("playlist.unlink", linked && !mirror);
        self.action_set_enabled("playlist.delete", !mirror);
        let same_playlist = imp.playlist.borrow().as_ref().map(|p| p.id()) == Some(id);
        // What every row shows (remove buttons, "only in Gatefold").
        let kind = |p: &gatefold_core::db::Playlist| (p.editable, p.spotify_id.is_some());
        let kind_changed = imp
            .playlist
            .borrow()
            .as_ref()
            .is_some_and(|old| kind(old.playlist()) != kind(info));
        imp.playlist.replace(Some(playlist.clone()));

        let sort = library
            .db()
            .and_then(|db| db.playlist_sort(id).ok())
            .unwrap_or_else(|| ("custom".to_owned(), false));
        self.show_sort(&sort);
        let mut entries = library.playlist_items(id);
        sort_entries(&mut entries, &sort.0, sort.1);
        let sort_changed = *imp.sort.borrow() != sort;
        imp.sort.replace(sort);
        let hidden = !info.readable;
        imp.hidden.set_visible(hidden);
        imp.empty.set_visible(entries.is_empty() && !hidden);
        imp.items.set_visible(!entries.is_empty());
        // Unchanged (another playlist changed, a sync): keep the rows.
        let rebuild = imp.rebuild.take() || kind_changed;
        let same = !sort_changed
            && !rebuild
            && *imp.entries.borrow() == entries
            && imp.items.first_child().is_some();
        if same {
            return;
        }
        // The same playlist changed (a song added, removed or moved): only
        // those rows are made, the others stay (and so does the scroll
        // position). Not while the rows are still being filled in.
        let filled = imp
            .items
            .row_at_index(imp.entries.borrow().len() as i32 - 1)
            .is_some();
        if same_playlist && !sort_changed && !rebuild && filled && !entries.is_empty() {
            self.update_rows(&playlist, entries);
            return;
        }
        // The same playlist changed (a song removed or moved): it stays
        // where it was scrolled to, instead of jumping to the top.
        let scroller = imp
            .items
            .ancestor(gtk::ScrolledWindow::static_type())
            .and_downcast::<gtk::ScrolledWindow>();
        let keep_at = scroller
            .as_ref()
            .filter(|_| same_playlist)
            .map(|s| s.vadjustment())
            .map(|a| (a.value(), a.page_size()))
            .filter(|(value, _)| *value > 0.0);
        imp.items.remove_all();
        imp.spotify_covers.borrow_mut().clear();
        imp.entries.replace(entries.clone());
        let generation = imp.fill_generation.get() + 1;
        imp.fill_generation.set(generation);
        // Enough rows at once to reach the old place (rows are about 50 px).
        let first = keep_at.map_or(0, |(value, page)| ((value + page) / 40.0) as usize + 20);
        self.fill_rows(playlist, entries, 0, generation, first);
        if let (Some((value, _)), Some(scroller)) = (keep_at, scroller) {
            glib::idle_add_local_once(move || {
                let adjustment = scroller.vadjustment();
                adjustment.set_value(value.min(adjustment.upper() - adjustment.page_size()));
            });
        }
    }

    /// Rows for `entries`, keeping the rows of entries that didn't change
    /// (wherever they are now).
    fn update_rows(&self, playlist: &PlaylistObject, entries: Vec<PlaylistItem>) {
        let imp = self.imp();
        let old = imp.entries.replace(entries.clone());
        let mut kept: std::collections::HashMap<i64, (PlaylistItem, gtk::ListBoxRow)> =
            std::collections::HashMap::new();
        for (index, entry) in old.into_iter().enumerate() {
            if let Some(row) = imp.items.row_at_index(index as i32) {
                kept.insert(entry.id, (entry, row));
            }
        }
        // Stop a fill still running (it can't be, see refresh; to be sure).
        imp.fill_generation.set(imp.fill_generation.get() + 1);
        imp.items.remove_all();
        let mut made = 0;
        for entry in &entries {
            let row = match kept.remove(&entry.id) {
                Some((old, row)) if same_but_place(&old, entry) => row,
                _ => {
                    made += 1;
                    self.row(playlist, entry)
                }
            };
            imp.items.append(&row);
        }
        tracing::debug!("Playlist rows: {made} made, {} kept", entries.len() - made);
    }

    /// Appends rows from `from`, a batch now and the rest in later idle
    /// moments, so a big playlist doesn't freeze the window.
    /// `at_least`: rows to add in the first batch (to reach a place).
    fn fill_rows(
        &self,
        playlist: PlaylistObject,
        entries: Vec<PlaylistItem>,
        from: usize,
        generation: u64,
        at_least: usize,
    ) {
        const BATCH: usize = 60;
        let imp = self.imp();
        if imp.fill_generation.get() != generation {
            return;
        }
        let end = (from + BATCH.max(at_least)).min(entries.len());
        for entry in &entries[from..end] {
            imp.items.append(&self.row(&playlist, entry));
        }
        if end < entries.len() {
            glib::idle_add_local_once(glib::clone!(
                #[weak(rename_to = panel)]
                self,
                move || panel.fill_rows(playlist, entries, end, generation, 0)
            ));
        }
    }

    fn row(&self, playlist: &PlaylistObject, entry: &PlaylistItem) -> gtk::ListBoxRow {
        let info = playlist.playlist();
        let playlist_id = info.id;
        let editable = info.editable;
        let linked = info.spotify_id.is_some();
        let track = entry.track.clone().map(TrackObject::new);
        let cover: CoverImage = glib::Object::builder()
            .property("min-size", 40)
            .property("natural-size", 40)
            .build();
        // Spotify-only songs: Spotify's cover, once downloaded.
        let spotify_cover = entry
            .image_url
            .as_deref()
            .map(crate::spotify::image_cover_key);
        if track.is_none()
            && let Some(url) = &entry.image_url
        {
            self.imp()
                .spotify_covers
                .borrow_mut()
                .entry(url.clone())
                .or_default()
                .push(cover.downgrade());
        }
        cover.set_cover(
            track
                .as_ref()
                .and_then(|t| t.cover_key().map(str::to_owned))
                .or(spotify_cover)
                .as_deref(),
        );
        let title = track
            .as_ref()
            .map(TrackObject::title)
            .or_else(|| entry.title.clone())
            .unwrap_or_else(|| gettext("Unknown Song"));
        let subtitle = [
            track
                .as_ref()
                .and_then(TrackObject::artist)
                .or(entry.artist.clone()),
            entry.album.clone(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" — ");
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .valign(gtk::Align::Center)
            .hexpand(true)
            .build();
        text.append(
            &gtk::Label::builder()
                .label(&title)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build(),
        );
        if !subtitle.is_empty() {
            text.append(
                &gtk::Label::builder()
                    .label(&subtitle)
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
            .margin_top(6)
            .margin_bottom(6)
            .build();
        // Spotify playlists only after choosing "Change Order on Spotify".
        // Sorted, the order can't be changed by dragging.
        let reorderable =
            editable && (!linked || self.imp().arranging.get()) && self.in_own_order();
        if reorderable {
            line.append(
                &gtk::Image::builder()
                    .icon_name("list-drag-handle-symbolic")
                    .css_classes(["dim-label", "drag-handle"])
                    .build(),
            );
        }
        line.append(&cover);
        line.append(&text);
        // Where it plays from (§9.5): nothing for the library, the normal
        // case.
        let (icon, tooltip) = match entry.match_state.as_str() {
            "spotify_only" | "user_pinned_spotify" if entry.spotify_uri.is_some() => (
                Some("gatefold-spotify-symbolic"),
                Some(if entry.match_state == "user_pinned_spotify" {
                    gettext("Plays from Spotify")
                } else {
                    gettext("Only on Spotify")
                }),
            ),
            _ if linked && entry.spotify_uri.is_none() && track.is_some() => (
                Some("computer-symbolic"),
                Some(gettext("Only in Gatefold; not on Spotify")),
            ),
            _ => (None, None),
        };
        if let Some(icon) = icon {
            line.append(
                &gtk::Image::builder()
                    .icon_name(icon)
                    .tooltip_text(tooltip.unwrap_or_default())
                    .css_classes(["dim-label"])
                    .build(),
            );
        }
        let duration = track
            .as_ref()
            .map(TrackObject::duration_ms)
            .or(entry.duration_ms);
        if let Some(duration) = duration {
            line.append(
                &gtk::Label::builder()
                    .label(format::duration(duration))
                    .css_classes(["dim-label", "numeric"])
                    .build(),
            );
        }
        // Like an album's songs: to the queue, and more in the menu.
        let entry_ref = entry.clone();
        if has_song(entry) {
            let queue = gtk::Button::builder()
                .icon_name("list-add-symbolic")
                .tooltip_text(gettext("Add to Queue"))
                .valign(gtk::Align::Center)
                .css_classes(["flat", "circular", "row-action"])
                .build();
            queue.connect_clicked(move |_| {
                if let Some(track) = queueable_entry(&entry_ref) {
                    GatefoldApplication::get()
                        .player()
                        .add_to_queue(vec![track]);
                }
            });
            line.append(&queue);
        }
        line.append(&entry_menu(entry, track.as_ref()));
        let item_id = entry.id;
        if editable {
            let remove = gtk::Button::builder()
                .icon_name("list-remove-symbolic")
                .tooltip_text(gettext("Remove from Playlist"))
                .valign(gtk::Align::Center)
                .css_classes(["flat", "circular"])
                .build();
            remove.connect_clicked(move |_| {
                GatefoldApplication::get()
                    .library()
                    .remove_playlist_item(playlist_id, item_id);
            });
            line.append(&remove);
        }

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        content.append(&line);
        if let Some(question) = suggestion(playlist_id, entry, editable) {
            content.append(&question);
        }
        let row = gtk::ListBoxRow::builder()
            .child(&content)
            .css_classes(["track-row"])
            .build();
        let remote = GatefoldApplication::get().player().is_remote() && linked;
        let streamable = streamable(entry);
        if track.is_none() && !((remote || streamable) && entry.spotify_uri.is_some()) {
            // Not in the library: shown, but can't play here. Spotify songs
            // say how to play them when clicked.
            line.add_css_class("unavailable");
            if entry.spotify_uri.is_none() {
                row.set_activatable(false);
                row.set_tooltip_text(Some(&gettext("This song isn't in your library")));
            }
        }

        // Drag and drop to reorder, animated.
        if reorderable {
            let place = row.downgrade();
            let index = move || place.upgrade().map_or(0, |row| row.index().max(0) as u32);
            super::reorder::attach(&row, index, move |from, to| {
                GatefoldApplication::get()
                    .library()
                    .move_playlist_item(playlist_id, from, to);
            });
        }
        row
    }

    /// A row was clicked: plays from it, or says how a Spotify-only song
    /// can play.
    fn activate_entry(&self, index: usize) {
        let only_on_spotify = self
            .imp()
            .entries
            .borrow()
            .get(index)
            .is_some_and(|e| e.track.is_none() && e.spotify_uri.is_some());
        let app = GatefoldApplication::get();
        if only_on_spotify && !app.player().is_remote() && !app.spotify().can_stream() {
            app.library()
                .emit_by_name::<()>("message", &[&only_on_spotify_hint()]);
            return;
        }
        self.play(Some(index), false);
    }

    /// Plays the entries that are in the library, from `start` (a row).
    /// With a Spotify device in control, the Spotify playlist plays there.
    fn play(&self, start: Option<usize>, shuffle: bool) {
        let imp = self.imp();
        let app = GatefoldApplication::get();
        // A followed playlist: its songs aren't known here, but Soloist can
        // play it whole (§9.4).
        let hidden = imp
            .playlist
            .borrow()
            .as_ref()
            .is_some_and(|p| !p.playlist().readable);
        if hidden && !app.player().is_remote() {
            let started = self.spotify_id().is_some_and(|id| {
                app.spotify()
                    .soloist_play_context(&format!("spotify:playlist:{id}"))
            });
            if !started {
                app.library().emit_by_name::<()>(
                    "message",
                    &[&gettext(
                        "Spotify doesn’t show this playlist’s songs; it can play in Soloist (Preferences → Spotify → Streaming)",
                    )],
                );
            }
            return;
        }
        if app.player().is_remote()
            && let Some(spotify_id) = self.spotify_id()
        {
            let entries = imp.entries.borrow();
            let offset = start
                .map(|row| {
                    entries[..row.min(entries.len())]
                        .iter()
                        .filter(|e| e.spotify_uri.is_some())
                        .count() as u32
                })
                .unwrap_or(0);
            // Sorted: the songs in this order, not Spotify's playlist.
            let order = (!self.in_own_order()).then(|| {
                entries
                    .iter()
                    .filter_map(|e| e.spotify_uri.clone())
                    .collect::<Vec<_>>()
            });
            let spotify = app.spotify();
            glib::spawn_future_local(async move {
                if shuffle {
                    let _ = spotify.call(|c| c.set_shuffle(true)).await;
                }
                spotify
                    .play_playlist_remote(&spotify_id, offset, order)
                    .await;
            });
            return;
        }
        let entries = imp.entries.borrow();
        // Spotify-only songs stream when streaming is on (§9.4).
        let mut streamed = std::collections::HashMap::new();
        if app.spotify().can_stream()
            && let Some(mut db) = app.library().db()
        {
            for entry in entries
                .iter()
                .filter(|e| e.track.is_none() && streamable(e))
            {
                if let Ok(Some(id)) = db.stream_track_for_item(entry.id) {
                    streamed.insert(entry.id, id);
                }
            }
        }
        let stand_ins = app
            .library()
            .tracks(&streamed.values().copied().collect::<Vec<_>>());
        if !stand_ins.is_empty() {
            app.spotify()
                .fetch_stream_covers(stand_ins.keys().copied().collect());
        }
        let skipped = entries
            .iter()
            .filter(|e| {
                e.track.is_none() && e.spotify_uri.is_some() && !streamed.contains_key(&e.id)
            })
            .count();
        if skipped > 0 {
            app.library().emit_by_name::<()>(
                "message",
                &[&ngettext(
                    "{} song is only on Spotify and is skipped",
                    "{} songs are only on Spotify and are skipped",
                    skipped as u32,
                )
                .replace("{}", &skipped.to_string())],
            );
        }
        let playable: Vec<(usize, TrackObject)> = entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| {
                let track = e.track.clone().or_else(|| {
                    let id = streamed.get(&e.id)?;
                    stand_ins.get(id).cloned()
                })?;
                Some((i, TrackObject::new(track)))
            })
            .collect();
        let start = start.and_then(|row| playable.iter().position(|(i, _)| *i == row));
        let name = imp
            .playlist
            .borrow()
            .as_ref()
            .map(|p| p.playlist().name.clone());
        let tracks = playable.into_iter().map(|(_, t)| t).collect();
        GatefoldApplication::get()
            .player()
            .play_tracks(tracks, start, shuffle, name);
    }

    /// The sort menu's actions ("sort.by", "sort.reversed").
    fn setup_sort(&self) {
        let group = gio::SimpleActionGroup::new();
        let by = gio::SimpleAction::new_stateful(
            "by",
            Some(glib::VariantTy::STRING),
            &"custom".to_variant(),
        );
        by.connect_activate(glib::clone!(
            #[weak(rename_to = panel)]
            self,
            move |_, param| {
                if let Some(by) = param.and_then(|p| p.get::<String>()) {
                    let reversed = panel.imp().sort.borrow().1;
                    panel.set_sort(&by, reversed);
                }
            }
        ));
        let reversed = gio::SimpleAction::new_stateful("reversed", None, &false.to_variant());
        reversed.connect_activate(glib::clone!(
            #[weak(rename_to = panel)]
            self,
            move |_, _| {
                let (by, reversed) = panel.imp().sort.borrow().clone();
                panel.set_sort(&by, !reversed);
            }
        ));
        group.add_action(&by);
        group.add_action(&reversed);
        self.insert_action_group("sort", Some(&group));
        let _ = self.imp().sort_actions.set((by, reversed));
    }

    /// Lists the playlist sorted (remembered per playlist).
    fn set_sort(&self, by: &str, reversed: bool) {
        let Some(id) = self.playlist_id() else { return };
        let library = GatefoldApplication::get().library();
        if let Some(mut db) = library.db()
            && let Err(err) = db.set_playlist_sort(id, by, reversed)
        {
            tracing::warn!("Cannot keep the playlist's sort: {err}");
        }
        self.refresh();
    }

    fn show_sort(&self, (by, reversed): &(String, bool)) {
        if let Some((by_action, reversed_action)) = self.imp().sort_actions.get() {
            by_action.set_state(&by.to_variant());
            reversed_action.set_state(&reversed.to_variant());
        }
    }

    /// In the playlist's own order (not sorted, not reversed): songs can
    /// be dragged.
    fn in_own_order(&self) -> bool {
        let sort = self.imp().sort.borrow();
        sort.0 == "custom" && !sort.1
    }

    /// The entries that can go to the queue, in order: library songs, and
    /// Spotify songs when they can play (streaming, or a Spotify device).
    fn queueable(&self) -> Vec<TrackObject> {
        let app = GatefoldApplication::get();
        let spotify_plays = app.player().is_remote() || app.spotify().can_stream();
        let entries = self.imp().entries.borrow().clone();
        let wanted: Vec<PlaylistItem> = entries
            .iter()
            .filter(|e| e.track.is_some() || spotify_plays)
            .cloned()
            .collect();
        let tracks = entry_objects(&wanted);
        let skipped = entries
            .iter()
            .filter(|e| e.track.is_none() && e.spotify_uri.is_some())
            .count();
        if !spotify_plays && skipped > 0 {
            app.library().emit_by_name::<()>(
                "message",
                &[&ngettext(
                    "{} song is only on Spotify and is skipped",
                    "{} songs are only on Spotify and are skipped",
                    skipped as u32,
                )
                .replace("{}", &skipped.to_string())],
            );
        }
        tracks
    }

    /// "Change Order on Spotify": the songs can be dragged until Done;
    /// the new order goes to Spotify then, in one go.
    fn arrange(&self) {
        let imp = self.imp();
        let Some(id) = self.playlist_id() else { return };
        if imp.arranging.replace(true) {
            return;
        }
        GatefoldApplication::get().spotify().hold_changes(id, true);
        imp.arrange_banner.set_revealed(true);
        imp.rebuild.set(true);
        // Dragging works in the playlist's own order.
        if !self.in_own_order() {
            self.set_sort("custom", false);
        } else {
            self.refresh();
        }
    }

    fn finish_arranging(&self) {
        let imp = self.imp();
        if !imp.arranging.replace(false) {
            return;
        }
        imp.arrange_banner.set_revealed(false);
        if let Some(id) = self.playlist_id() {
            GatefoldApplication::get().spotify().hold_changes(id, false);
        }
        imp.rebuild.set(true);
        self.refresh();
    }

    /// Spotify playlists are renamed here, not by clicking the name.
    async fn rename_on_spotify(&self) {
        let Some(playlist) = self.imp().playlist.borrow().clone() else {
            return;
        };
        if let Some(name) = ask_name(
            self,
            &gettext("Rename on Spotify"),
            &playlist.playlist().name,
            &gettext("_Rename"),
        )
        .await
        {
            GatefoldApplication::get()
                .library()
                .rename_playlist(playlist.id(), &name);
        }
    }

    async fn rename(&self) {
        let Some(playlist) = self.imp().playlist.borrow().clone() else {
            return;
        };
        if let Some(name) = ask_name(
            self,
            &gettext("Rename Playlist"),
            &playlist.playlist().name,
            &gettext("_Rename"),
        )
        .await
        {
            GatefoldApplication::get()
                .library()
                .rename_playlist(playlist.id(), &name);
        }
    }

    async fn change_cover(&self) {
        let Some(id) = self.playlist_id() else { return };
        let filter = gtk::FileFilter::new();
        filter.set_name(Some(&gettext("Images")));
        filter.add_mime_type("image/*");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title(gettext("Choose Playlist Cover"))
            .modal(true)
            .filters(&filters)
            .build();
        let window = self.root().and_downcast::<gtk::Window>();
        let Ok(file) = dialog.open_future(window.as_ref()).await else {
            return;
        };
        let library = GatefoldApplication::get().library();
        let result = match file.load_bytes_future().await {
            Ok((bytes, _)) => library.set_playlist_cover(id, bytes.to_vec()).await,
            Err(err) => Err(err.to_string()),
        };
        if let Err(err) = result {
            tracing::warn!("Cannot use the cover: {err}");
            library.emit_by_name::<()>("message", &[&gettext("This image can't be used")]);
        }
    }

    async fn export(&self) {
        let Some(playlist) = self.imp().playlist.borrow().clone() else {
            return;
        };
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        for (name, pattern) in [("M3U8", "*.m3u8"), ("XSPF", "*.xspf")] {
            let filter = gtk::FileFilter::new();
            filter.set_name(Some(name));
            filter.add_pattern(pattern);
            filters.append(&filter);
        }
        let dialog = gtk::FileDialog::builder()
            .title(gettext("Export Playlist"))
            .modal(true)
            .filters(&filters)
            .initial_name(format!(
                "{}.m3u8",
                playlist.playlist().name.replace('/', "-")
            ))
            .build();
        let window = self.root().and_downcast::<gtk::Window>();
        let Ok(file) = dialog.save_future(window.as_ref()).await else {
            return;
        };
        let Some(path) = file.path() else { return };
        let library = GatefoldApplication::get().library();
        let message = match library.export_playlist(playlist.id(), path).await {
            Ok(()) => gettext("Playlist exported"),
            Err(err) => {
                tracing::warn!("Export failed: {err}");
                gettext("The playlist couldn't be exported")
            }
        };
        library.emit_by_name::<()>("message", &[&message]);
    }

    async fn delete(&self) {
        let Some(playlist) = self.imp().playlist.borrow().clone() else {
            return;
        };
        let dialog = adw::AlertDialog::builder()
            // Translators: {} is a playlist name.
            .heading(gettext("Delete “{}”?").replace("{}", &playlist.playlist().name))
            .body(if playlist.playlist().spotify_id.is_some() {
                gettext("The songs stay in your library, and the playlist stays on Spotify.")
            } else {
                gettext("The songs stay in your library.")
            })
            .close_response("cancel")
            .default_response("cancel")
            .build();
        dialog.add_responses(&[
            ("cancel", &gettext("_Cancel")),
            ("delete", &gettext("_Delete")),
        ]);
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        if dialog.choose_future(Some(self)).await == "delete" {
            GatefoldApplication::get()
                .library()
                .delete_playlist(playlist.id());
        }
    }
}

impl PlaylistPanel {
    /// "Duplicate as Local Playlist": a Spotify playlist's songs as a new
    /// local playlist (library songs only).
    fn duplicate(&self) {
        let imp = self.imp();
        let Some(playlist) = imp.playlist.borrow().clone() else {
            return;
        };
        let tracks: Vec<i64> = imp
            .entries
            .borrow()
            .iter()
            .filter_map(|e| e.track.as_ref().map(|t| t.record.id))
            .collect();
        let library = GatefoldApplication::get().library();
        // Translators: {} is a playlist name.
        let name = gettext("{} (Copy)").replace("{}", &playlist.playlist().name);
        if let Some(id) = library.create_playlist(&name, &tracks) {
            if let Some(copy) = library.playlist(id) {
                self.set_playlist(&copy);
            }
            // Translators: {} is a playlist name.
            library.emit_by_name::<()>("message", &[&gettext("Created “{}”").replace("{}", &name)]);
        }
    }

    async fn unlink(&self) {
        let Some(id) = self.playlist_id() else { return };
        let dialog = adw::AlertDialog::builder()
            .heading(gettext("Stop Syncing with Spotify?"))
            .body(gettext(
                "The playlist stays here and on Spotify, but changes are no longer copied between them.",
            ))
            .close_response("cancel")
            .default_response("cancel")
            .build();
        dialog.add_responses(&[
            ("cancel", &gettext("_Cancel")),
            ("unlink", &gettext("_Stop Syncing")),
        ]);
        if dialog.choose_future(Some(self)).await == "unlink" {
            let library = GatefoldApplication::get().library();
            if let Some(mut db) = library.db() {
                let _ = db.unlink_playlist(id);
            }
            library.reload_playlists();
        }
    }
}

/// Sorts entries for the view: by "title", "artist", "album", "added"
/// or "duration" ("custom": the playlist's order), ties in the playlist's
/// order; `reversed` turns the result around.
fn sort_entries(entries: &mut [PlaylistItem], by: &str, reversed: bool) {
    let text = |value: Option<String>| value.unwrap_or_default().to_lowercase();
    let title = |e: &PlaylistItem| {
        text(
            e.track
                .as_ref()
                .and_then(|t| t.record.tags.title.clone())
                .or(e.title.clone()),
        )
    };
    let artist = |e: &PlaylistItem| {
        text(
            e.track
                .as_ref()
                .and_then(|t| t.record.tags.artist.clone().or(t.album_artist.clone()))
                .or(e.artist.clone()),
        )
    };
    let album = |e: &PlaylistItem| {
        text(
            e.track
                .as_ref()
                .and_then(|t| t.album_title.clone())
                .or(e.album.clone()),
        )
    };
    // Songs of one album in album order.
    let place = |e: &PlaylistItem| {
        e.track.as_ref().map_or((0, 0), |t| {
            (
                t.album_disc.or(t.record.tags.disc).unwrap_or(1),
                t.record.tags.number.unwrap_or(0),
            )
        })
    };
    let duration = |e: &PlaylistItem| {
        e.track
            .as_ref()
            .map(|t| t.record.tags.duration_ms)
            .or(e.duration_ms)
            .unwrap_or(0)
    };
    match by {
        "title" => entries.sort_by_cached_key(|e| (title(e), e.position)),
        "artist" => {
            entries.sort_by_cached_key(|e| (artist(e), album(e), place(e), e.position));
        }
        "album" => entries.sort_by_cached_key(|e| (album(e), place(e), e.position)),
        // Unknown dates (entries from before Gatefold kept them) first.
        "added" => entries.sort_by_key(|e| (e.added.unwrap_or(0), e.position)),
        "duration" => entries.sort_by_key(|e| (duration(e), e.position)),
        _ => entries.sort_by_key(|e| e.position),
    }
    if reversed {
        entries.reverse();
    }
}

/// The track ID an entry adds to a playlist: the library song, or the
/// Spotify song (as one to stream, a negative ID).
fn entry_track_id(entry: &PlaylistItem) -> Option<i64> {
    entry_track_ids(std::slice::from_ref(entry)).pop().flatten()
}

/// An entry as a song for the queue: the library song, else the Spotify
/// song to stream.
fn entry_object(entry: &PlaylistItem) -> Option<TrackObject> {
    entry_objects(std::slice::from_ref(entry)).pop()
}

/// `entry_track_id` for many entries at once (one database use), in
/// order; `None` for entries without a song.
fn entry_track_ids(entries: &[PlaylistItem]) -> Vec<Option<i64>> {
    let library = GatefoldApplication::get().library();
    let mut db = library.db();
    entries
        .iter()
        .map(|entry| {
            if let Some(track) = &entry.track {
                return Some(track.record.id);
            }
            entry
                .spotify_uri
                .as_deref()
                .filter(|uri| uri.starts_with("spotify:track:"))?;
            db.as_mut()?.stream_track_for_item(entry.id).ok()?
        })
        .collect()
}

/// `entry_object` for many entries at once: Spotify songs are read and
/// their covers fetched in one go, not one by one.
fn entry_objects(entries: &[PlaylistItem]) -> Vec<TrackObject> {
    let ids = entry_track_ids(entries);
    let streams: Vec<i64> = entries
        .iter()
        .zip(&ids)
        .filter(|(e, _)| e.track.is_none())
        .filter_map(|(_, id)| *id)
        .collect();
    let app = GatefoldApplication::get();
    let mut found = app.library().tracks(&streams);
    if !streams.is_empty() {
        app.spotify().fetch_stream_covers(streams);
    }
    entries
        .iter()
        .zip(ids)
        .filter_map(|(entry, id)| match &entry.track {
            Some(track) => Some(TrackObject::new(track.clone())),
            None => found.remove(&id?).map(TrackObject::new),
        })
        .collect()
}

/// One entry for the queue, or a hint when it's a Spotify song that
/// can't play here.
fn queueable_entry(entry: &PlaylistItem) -> Option<TrackObject> {
    let app = GatefoldApplication::get();
    if entry.track.is_none() && !app.player().is_remote() && !app.spotify().can_stream() {
        app.library()
            .emit_by_name::<()>("message", &[&only_on_spotify_hint()]);
        return None;
    }
    entry_object(entry)
}

/// How a song that is only on Spotify can play.
fn only_on_spotify_hint() -> String {
    if crate::spotify::Spotify::streaming_built() {
        gettext(
            "This song is only on Spotify; turn on streaming in Preferences, or choose a device under Play On…",
        )
    } else {
        gettext("This song is only on Spotify; choose a device under Play On… to play it there")
    }
}

/// The same entry, maybe at another place.
fn same_but_place(old: &PlaylistItem, new: &PlaylistItem) -> bool {
    PlaylistItem {
        position: new.position,
        ..old.clone()
    } == *new
}

/// "Spotify playlist by Max · 2 changes waiting".
fn source_line(info: &gatefold_core::db::Playlist, sending: bool) -> String {
    let mut parts = Vec::new();
    let owner = info.spotify_owner.clone().unwrap_or_default();
    parts.push(match (info.source.as_str(), info.editable) {
        // Translators: {} is a Spotify user name.
        ("spotify", true) => gettext("Spotify playlist by {}").replace("{}", &owner),
        // Translators: {} is a Spotify user name.
        ("spotify", false) => gettext("Spotify playlist by {}, read-only").replace("{}", &owner),
        _ => gettext("Also on Spotify"),
    });
    if sending {
        parts.push(gettext("sending changes to Spotify…"));
    } else if info.pending_changes > 0 {
        parts.push(
            ngettext(
                "{} change waiting to be sent",
                "{} changes waiting to be sent",
                info.pending_changes,
            )
            .replace("{}", &info.pending_changes.to_string()),
        );
    }
    parts.join(" · ")
}

/// A Spotify song that can stream (§9.4), streaming being on.
fn streamable(entry: &PlaylistItem) -> bool {
    entry
        .spotify_uri
        .as_deref()
        .is_some_and(|uri| uri.starts_with("spotify:track:"))
        && entry.match_state != "unavailable"
        && GatefoldApplication::get().spotify().can_stream()
}

/// What Yes or No does.
type Answer = Box<dyn Fn()>;

/// "Is this “Butterfly” from your library?" for likely matches (§9.5),
/// both ways.
fn suggestion(playlist_id: i64, entry: &PlaylistItem, editable: bool) -> Option<gtk::Box> {
    let item = entry.id;
    let (text, accept, reject): (String, Answer, Answer) =
        if let Some(local) = &entry.suggested_track {
            let local = TrackObject::new(local.clone());
            let track_id = local.id();
            let label = [
                Some(local.title()),
                local.artist(),
                local.track().album_title.clone(),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" — ");
            (
                // Translators: {} is "Song — Artist — Album" from the library.
                gettext("Play “{}” from your library?").replace("{}", &label),
                Box::new(move || {
                    GatefoldApplication::get()
                        .library()
                        .choose_local_match(item, Some(track_id));
                }),
                Box::new(move || {
                    GatefoldApplication::get()
                        .library()
                        .dismiss_suggestion(item, None, false);
                }),
            )
        } else {
            let (uri, title, artist) = entry
                .suggested_spotify
                .as_deref()
                .and_then(crate::spotify::parse_suggestion)
                .filter(|_| editable)?;
            let track_id = entry.track.as_ref().map(|t| t.record.id);
            (
                // Translators: {} is "Song — Artist" on Spotify.
                gettext("On Spotify as “{}”?").replace("{}", &format!("{title} — {artist}")),
                Box::new(move || {
                    GatefoldApplication::get()
                        .library()
                        .accept_spotify_suggestion(playlist_id, item, &uri);
                }),
                Box::new(move || {
                    GatefoldApplication::get()
                        .library()
                        .dismiss_suggestion(item, track_id, true);
                }),
            )
        };
    let line = gtk::Box::builder()
        .spacing(6)
        .margin_start(12)
        .margin_end(6)
        .margin_bottom(6)
        .build();
    line.append(
        &gtk::Label::builder()
            .label(&text)
            .xalign(0.0)
            .wrap(true)
            .hexpand(true)
            .css_classes(["caption"])
            .build(),
    );
    let yes = gtk::Button::builder()
        .label(gettext("Yes"))
        .valign(gtk::Align::Center)
        .css_classes(["flat", "small"])
        .build();
    yes.connect_clicked(move |_| accept());
    let no = gtk::Button::builder()
        .label(gettext("No"))
        .valign(gtk::Align::Center)
        .css_classes(["flat", "small"])
        .build();
    no.connect_clicked(move |_| reject());
    line.append(&yes);
    line.append(&no);
    Some(line)
}

/// A song to play or add: in the library, or a Spotify song.
fn has_song(entry: &PlaylistItem) -> bool {
    entry.track.is_some()
        || (entry.title.is_some()
            && entry
                .spotify_uri
                .as_deref()
                .is_some_and(|uri| uri.starts_with("spotify:track:")))
}

/// The menu of an entry: the queue and playlists as for an album's songs,
/// which version plays (Spotify entries), and where to find it.
fn entry_menu(entry: &PlaylistItem, track: Option<&TrackObject>) -> gtk::MenuButton {
    let item = entry.id;
    let actions = gio::SimpleActionGroup::new();
    let menu = gio::Menu::new();
    let button = gtk::MenuButton::builder()
        .icon_name("view-more-symbolic")
        .tooltip_text(gettext("More"))
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular"])
        .build();

    if has_song(entry) {
        let queue = gio::Menu::new();
        let play_next = gio::SimpleAction::new("play-next", None);
        let for_next = entry.clone();
        play_next.connect_activate(move |_, _| {
            if let Some(track) = queueable_entry(&for_next) {
                GatefoldApplication::get().player().play_next(vec![track]);
            }
        });
        actions.add_action(&play_next);
        queue.append(Some(&gettext("Play _Next")), Some("entry.play-next"));
        let add = gio::SimpleAction::new("add-to-playlist", None);
        let for_playlist = entry.clone();
        let anchor = button.downgrade();
        add.connect_activate(move |_, _| {
            if let (Some(id), Some(anchor)) = (entry_track_id(&for_playlist), anchor.upgrade()) {
                super::playlist_picker::show(&anchor, vec![id]);
            }
        });
        actions.add_action(&add);
        queue.append(
            Some(&gettext("Add to _Playlist…")),
            Some("entry.add-to-playlist"),
        );
        menu.append_section(None, &queue);
    }

    if entry.spotify_uri.is_some() {
        let versions = gio::Menu::new();
        let choose = gio::SimpleAction::new("use-local", None);
        let query = [entry.title.clone(), entry.artist.clone()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        choose.connect_activate(move |_, _| {
            let query = query.clone();
            glib::spawn_future_local(async move {
                let window = GatefoldApplication::get().active_window();
                if let Some(track) = choose_library_song(window.as_ref(), &query).await {
                    GatefoldApplication::get()
                        .library()
                        .choose_local_match(item, Some(track));
                }
            });
        });
        actions.add_action(&choose);
        versions.append(
            Some(&gettext("_Use Song from Library…")),
            Some("entry.use-local"),
        );
        if entry.match_state == "user_pinned_spotify" || entry.match_state == "user_matched" {
            let auto = gio::SimpleAction::new("auto", None);
            auto.connect_activate(move |_, _| {
                GatefoldApplication::get()
                    .library()
                    .choose_local_match(item, None);
            });
            actions.add_action(&auto);
            versions.append(Some(&gettext("_Match Automatically")), Some("entry.auto"));
        } else if track.is_some() {
            let pin = gio::SimpleAction::new("pin", None);
            pin.connect_activate(move |_, _| {
                GatefoldApplication::get().library().pin_spotify(item)
            });
            actions.add_action(&pin);
            versions.append(
                Some(&gettext("Always Play from _Spotify")),
                Some("entry.pin"),
            );
        }
        menu.append_section(None, &versions);
    }

    // Where it's from: the library's pages, else Spotify's.
    let pages = gio::Menu::new();
    let song_uri = entry
        .spotify_uri
        .clone()
        .filter(|uri| uri.starts_with("spotify:track:"));
    let album_id = track.and_then(|t| t.track().album_id);
    if let Some(album) = album_id {
        pages.append(
            Some(&gettext("Go to _Album")),
            Some(&gio::Action::print_detailed_name(
                "win.show-album",
                Some(&album.to_variant()),
            )),
        );
        let artist = GatefoldApplication::get()
            .library()
            .album_artists(album)
            .first()
            .map(|(id, _)| *id);
        if let Some(artist) = artist {
            pages.append(
                Some(&gettext("Go to A_rtist")),
                Some(&gio::Action::print_detailed_name(
                    "win.show-artist",
                    Some(&artist.to_variant()),
                )),
            );
        }
    } else if let Some(uri) = &song_uri {
        for (name, label, artist) in [
            ("album", gettext("Go to _Album"), false),
            ("artist", gettext("Go to A_rtist"), true),
        ] {
            let action = gio::SimpleAction::new(name, None);
            let uri = uri.clone();
            action.connect_activate(move |_, _| super::open_song_page(uri.clone(), artist));
            actions.add_action(&action);
            pages.append(Some(&label), Some(&format!("entry.{name}")));
        }
    }
    if let Some(id) = song_uri
        .as_deref()
        .and_then(|u| u.strip_prefix("spotify:track:"))
    {
        let id = id.to_owned();
        let open = gio::SimpleAction::new("open", None);
        open.connect_activate(move |_, _| crate::spotify::open_on_spotify("track", &id));
        actions.add_action(&open);
        pages.append(Some(&gettext("_Open in Spotify")), Some("entry.open"));
    }
    menu.append_section(None, &pages);
    button.set_menu_model(Some(&menu));
    button.insert_action_group("entry", Some(&actions));
    button
}

/// Lets the user pick a library song (for a Spotify entry).
async fn choose_library_song(parent: Option<&gtk::Window>, query: &str) -> Option<i64> {
    let dialog = adw::AlertDialog::builder()
        .heading(gettext("Use Song from Library"))
        .close_response("cancel")
        .build();
    dialog.add_responses(&[("cancel", &gettext("_Cancel"))]);
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .width_request(320)
        .build();
    let search = gtk::SearchEntry::builder().text(query).build();
    let results = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    content.append(&search);
    content.append(
        &gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(320)
            .child(&results)
            .build(),
    );
    dialog.set_extra_child(Some(&content));
    let chosen = std::rc::Rc::new(std::cell::Cell::new(None));
    let fill = glib::clone!(
        #[weak]
        results,
        #[weak]
        dialog,
        #[strong]
        chosen,
        move |text: &str| {
            results.remove_all();
            let library = GatefoldApplication::get().library();
            let ids = library.search(text, 10).tracks;
            let tracks = library.tracks(&ids);
            for id in ids {
                let Some(track) = tracks.get(&id).cloned().map(TrackObject::new) else {
                    continue;
                };
                let row = adw::ActionRow::builder()
                    .use_markup(false)
                    .title(track.title())
                    .subtitle(
                        [track.artist(), track.track().album_title.clone()]
                            .into_iter()
                            .flatten()
                            .collect::<Vec<_>>()
                            .join(" — "),
                    )
                    .activatable(true)
                    .build();
                row.connect_activated(glib::clone!(
                    #[weak]
                    dialog,
                    #[strong]
                    chosen,
                    move |_| {
                        chosen.set(Some(id));
                        dialog.close();
                    }
                ));
                results.append(&row);
            }
        }
    );
    fill(query);
    search.connect_search_changed(move |entry| fill(&entry.text()));
    dialog.choose_future(parent).await;
    chosen.get()
}
