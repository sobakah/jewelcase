//! The album side panel: cover, title, edition, actions and the tracklist
//! grouped by disc (CONCEPT.md §4.1, §3.4).

use std::cell::RefCell;
use std::sync::OnceLock;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use gtk::glib::subclass::Signal;

use super::album_tile::artist_or_unknown;
use super::{CoverImage, TrackRow};
use crate::application::GatefoldApplication;
use crate::format;
use crate::objects::{AlbumObject, TrackObject};

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/sobakah/Gatefold/ui/album_panel.ui")]
    pub struct AlbumPanel {
        #[template_child]
        pub cover: TemplateChild<CoverImage>,
        #[template_child]
        pub title: TemplateChild<gtk::Label>,
        #[template_child]
        pub edition: TemplateChild<gtk::Label>,
        #[template_child]
        pub artist: TemplateChild<gtk::Label>,
        #[template_child]
        pub meta: TemplateChild<gtk::Label>,
        #[template_child]
        pub tracks: TemplateChild<gtk::Box>,
        #[template_child]
        pub versions_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub versions_label: TemplateChild<gtk::Label>,
        #[template_child]
        pub versions: TemplateChild<gtk::DropDown>,

        pub album: RefCell<Option<AlbumObject>>,
        /// Index into the album's versions.
        pub version: RefCell<usize>,
        pub track_objects: RefCell<Vec<TrackObject>>,
        pub rows: RefCell<Vec<TrackRow>>,
        pub versions_handler: RefCell<Option<glib::SignalHandlerId>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AlbumPanel {
        const NAME: &'static str = "GatefoldAlbumPanel";
        type Type = super::AlbumPanel;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            CoverImage::ensure_type();
            klass.bind_template();
            klass.install_action("panel.close", None, |panel, _, _| {
                panel.emit_by_name::<()>("close", &[]);
            });
            klass.install_action("panel.play", None, |panel, _, _| panel.play(None, false));
            klass.install_action("panel.shuffle", None, |panel, _, _| panel.play(None, true));
            klass.install_action("panel.play-next", None, |panel, _, _| {
                let tracks = panel.imp().track_objects.borrow().clone();
                GatefoldApplication::get().player().play_next(tracks);
            });
            klass.install_action("panel.add-to-queue", None, |panel, _, _| {
                let tracks = panel.imp().track_objects.borrow().clone();
                GatefoldApplication::get().player().add_to_queue(tracks);
            });
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for AlbumPanel {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| vec![Signal::builder("close").build()])
        }

        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            let player = GatefoldApplication::get().player();
            player.connect_current_track_notify(glib::clone!(
                #[weak]
                obj,
                move |_| obj.update_playing()
            ));
        }
    }

    impl WidgetImpl for AlbumPanel {}
    impl BinImpl for AlbumPanel {}
}

glib::wrapper! {
    pub struct AlbumPanel(ObjectSubclass<imp::AlbumPanel>)
        @extends gtk::Widget, adw::Bin,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl AlbumPanel {
    pub fn album(&self) -> Option<AlbumObject> {
        self.imp().album.borrow().clone()
    }

    pub fn set_album(&self, album: &AlbumObject) {
        let imp = self.imp();
        let same_group = imp
            .album
            .borrow()
            .as_ref()
            .is_some_and(|a| a.group_key() == album.group_key());
        let version = if same_group {
            (*imp.version.borrow()).min(album.versions().len() - 1)
        } else {
            0
        };
        imp.album.replace(Some(album.clone()));
        self.setup_versions(album, version);
        self.show_version(version);
    }

    fn setup_versions(&self, album: &AlbumObject, selected: usize) {
        let imp = self.imp();
        if let Some(handler) = imp.versions_handler.take() {
            imp.versions.disconnect(handler);
        }
        let versions = album.versions();
        imp.versions_box.set_visible(versions.len() > 1);
        if versions.len() < 2 {
            return;
        }
        let names: Vec<String> = versions
            .iter()
            .map(|v| v.edition.clone().unwrap_or_else(|| gettext("Standard")))
            .collect();
        // Translators: {} is a list of album editions, e.g. "L version, F version".
        imp.versions_label
            .set_label(&gettext("Versions: {}").replace("{}", &names.join(", ")));
        let model = gtk::StringList::new(&names.iter().map(String::as_str).collect::<Vec<_>>());
        imp.versions.set_model(Some(&model));
        imp.versions.set_selected(selected as u32);
        let handler = imp.versions.connect_selected_notify(glib::clone!(
            #[weak(rename_to = panel)]
            self,
            move |dropdown| panel.show_version(dropdown.selected() as usize)
        ));
        imp.versions_handler.replace(Some(handler));
    }

    fn show_version(&self, index: usize) {
        let imp = self.imp();
        let Some(album) = self.album() else { return };
        let Some(info) = album.versions().get(index) else {
            return;
        };
        imp.version.replace(index);

        imp.cover.set_cover(info.cover_key.as_deref());
        // Translators: accessible label of an album cover; {} is the album title.
        imp.cover
            .set_label(&gettext("Cover of {}").replace("{}", &info.title));
        imp.title.set_label(&info.title);
        imp.edition
            .set_label(info.edition.as_deref().unwrap_or_default());
        imp.edition.set_visible(info.edition.is_some());
        let artist = artist_or_unknown(&info.artist_credit);
        imp.artist.set_label(&artist);

        let mut meta = Vec::new();
        if let Some(year) = info.year {
            meta.push(year.to_string());
        }
        meta.push(release_type_name(info));
        meta.push(format::song_count(info.track_count));
        meta.push(format::length(info.duration_ms));
        imp.meta.set_label(&meta.join(" · "));

        let tracks = GatefoldApplication::get().library().album_tracks(info.id);
        self.fill_tracks(&tracks, &info.artist_credit);
        imp.track_objects.replace(tracks);
        self.update_playing();
    }

    fn fill_tracks(&self, tracks: &[TrackObject], album_artist: &str) {
        let imp = self.imp();
        while let Some(child) = imp.tracks.first_child() {
            imp.tracks.remove(&child);
        }
        let multi_disc = tracks
            .iter()
            .any(|t| t.track().record.tags.disc.unwrap_or(1) != 1);
        let mut rows = Vec::with_capacity(tracks.len());
        let mut current_disc = None;
        let mut list: Option<gtk::ListBox> = None;
        for track in tracks {
            let disc = track.track().record.tags.disc.unwrap_or(1);
            if list.is_none() || current_disc != Some(disc) {
                current_disc = Some(disc);
                if multi_disc {
                    // Translators: {} is a disc number.
                    let header = gtk::Label::builder()
                        .label(gettext("Disc {}").replace("{}", &disc.to_string()))
                        .xalign(0.0)
                        .css_classes(["heading"])
                        .build();
                    imp.tracks.append(&header);
                }
                let new_list = gtk::ListBox::builder()
                    .selection_mode(gtk::SelectionMode::None)
                    .activate_on_single_click(false)
                    .css_classes(["boxed-list"])
                    .build();
                new_list.connect_row_activated(glib::clone!(
                    #[weak(rename_to = panel)]
                    self,
                    move |_, row| {
                        if let Some(row) = row.downcast_ref::<TrackRow>() {
                            let id = row.track().id();
                            let index = panel
                                .imp()
                                .track_objects
                                .borrow()
                                .iter()
                                .position(|t| t.id() == id);
                            panel.play(index, false);
                        }
                    }
                ));
                imp.tracks.append(&new_list);
                list = Some(new_list);
            }
            let row = TrackRow::new(track, album_artist);
            row.set_tooltip_text(Some(&gettext("Double-click to play")));
            if let Some(list) = &list {
                list.append(&row);
            }
            rows.push(row);
        }
        imp.rows.replace(rows);
    }

    fn update_playing(&self) {
        let current = GatefoldApplication::get()
            .player()
            .current_track()
            .map(|t| t.id());
        for row in self.imp().rows.borrow().iter() {
            row.set_playing(Some(row.track().id()) == current);
        }
    }

    fn play(&self, start: Option<usize>, shuffle: bool) {
        let imp = self.imp();
        let Some(album) = self.album() else { return };
        let Some(info) = album.versions().get(*imp.version.borrow()) else {
            return;
        };
        GatefoldApplication::get()
            .player()
            .play_album(info.id, start, shuffle);
    }
}

fn release_type_name(album: &gatefold_core::db::Album) -> String {
    use gatefold_core::library::titles::ReleaseType;
    if album.soundtrack {
        return gettext("Soundtrack");
    }
    if album.compilation {
        return gettext("Compilation");
    }
    match album.release_type {
        ReleaseType::Album => gettext("Album"),
        ReleaseType::Ep => gettext("EP"),
        ReleaseType::Single => gettext("Single"),
    }
}
