//! A playlist in the side panel (CONCEPT.md §4.1, §9.5).

use std::cell::RefCell;
use std::sync::OnceLock;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gatefold_core::db::PlaylistItem;
use gettextrs::{gettext, ngettext};
use gtk::glib::subclass::Signal;
use gtk::{gio, glib};

use super::CoverImage;
use super::playlist_picker::ask_name;
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
        pub name: TemplateChild<gtk::EditableLabel>,
        #[template_child]
        pub add_search: TemplateChild<gtk::SearchEntry>,
        #[template_child]
        pub add_results: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub meta: TemplateChild<gtk::Label>,
        #[template_child]
        pub availability: TemplateChild<gtk::Label>,
        #[template_child]
        pub empty: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub items: TemplateChild<gtk::ListBox>,
        pub playlist: RefCell<Option<PlaylistObject>>,
        pub entries: RefCell<Vec<PlaylistItem>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PlaylistPanel {
        const NAME: &'static str = "GatefoldPlaylistPanel";
        type Type = super::PlaylistPanel;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            CoverImage::ensure_type();
            klass.bind_template();
            klass.install_action("playlist.close", None, |panel, _, _| {
                panel.emit_by_name::<()>("close", &[]);
            });
            klass.install_action("playlist.play", None, |panel, _, _| panel.play(None, false));
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
            let obj = self.obj();
            self.items.connect_row_activated(glib::clone!(
                #[weak]
                obj,
                move |_, row| obj.play(Some(row.index() as usize), false)
            ));
            // Renaming: commit when editing ends (Enter or focus leaves).
            self.name.connect_editing_notify(glib::clone!(
                #[weak]
                obj,
                move |label| {
                    if !label.is_editing() {
                        obj.commit_name();
                    }
                }
            ));
            self.add_search.connect_search_changed(glib::clone!(
                #[weak]
                obj,
                move |entry| obj.search_songs(&entry.text())
            ));
            self.add_search
                .connect_stop_search(|entry| entry.set_text(""));
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
        imp.playlist.replace(Some(playlist.clone()));
        if changed {
            imp.add_search.set_text("");
        }
        self.refresh();
        // A new, empty playlist: go straight to finding songs.
        if changed && playlist.playlist().item_count == 0 {
            imp.add_search.grab_focus();
        }
    }

    /// Types `text` into the "Add Songs" field (screenshot hook).
    pub fn search_for_songs(&self, text: &str) {
        self.imp().add_search.set_text(text);
    }

    fn commit_name(&self) {
        let imp = self.imp();
        let Some(playlist) = imp.playlist.borrow().clone() else {
            return;
        };
        let name = imp.name.text().trim().to_owned();
        if name.is_empty() || name == playlist.playlist().name {
            imp.name.set_text(&playlist.playlist().name);
            return;
        }
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
        if !imp.name.is_editing() {
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
        imp.playlist.replace(Some(playlist));

        let entries = library.playlist_items(id);
        imp.empty.set_visible(entries.is_empty());
        imp.items.set_visible(!entries.is_empty());
        imp.items.remove_all();
        for entry in &entries {
            imp.items.append(&self.row(id, entry));
        }
        imp.entries.replace(entries);
    }

    fn row(&self, playlist_id: i64, entry: &PlaylistItem) -> gtk::ListBoxRow {
        let track = entry.track.clone().map(TrackObject::new);
        let cover: CoverImage = glib::Object::builder()
            .property("min-size", 40)
            .property("natural-size", 40)
            .build();
        cover.set_cover(track.as_ref().and_then(|t| t.cover_key()));
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
        line.append(
            &gtk::Image::builder()
                .icon_name("list-drag-handle-symbolic")
                .css_classes(["dim-label", "drag-handle"])
                .build(),
        );
        line.append(&cover);
        line.append(&text);
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
        let remove = gtk::Button::builder()
            .icon_name("list-remove-symbolic")
            .tooltip_text(gettext("Remove from Playlist"))
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular"])
            .build();
        let item_id = entry.id;
        remove.connect_clicked(move |_| {
            GatefoldApplication::get()
                .library()
                .remove_playlist_item(playlist_id, item_id);
        });
        line.append(&remove);

        let row = gtk::ListBoxRow::builder().child(&line).build();
        if track.is_none() {
            // Not in the library (any more): shown, but can't play.
            row.add_css_class("unavailable");
            row.set_activatable(false);
            row.set_tooltip_text(Some(&gettext("This song isn't in your library")));
        }

        // Drag and drop to reorder, animated.
        super::reorder::attach(&row, entry.position, move |from, to| {
            GatefoldApplication::get()
                .library()
                .move_playlist_item(playlist_id, from, to);
        });
        row
    }

    /// Plays the entries that are in the library, from `start` (a row).
    fn play(&self, start: Option<usize>, shuffle: bool) {
        let imp = self.imp();
        let entries = imp.entries.borrow();
        let playable: Vec<(usize, TrackObject)> = entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| Some((i, TrackObject::new(e.track.clone()?))))
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
            .body(gettext("The songs stay in your library."))
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
