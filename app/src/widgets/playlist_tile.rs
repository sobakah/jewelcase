//! A tile in the playlist grid: cover (custom or a 2×2 mosaic), name and
//! song count (CONCEPT.md §4.1).

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::CoverImage;
use crate::format;
use crate::objects::PlaylistObject;

mod imp {
    use super::*;

    #[derive(Debug)]
    pub struct PlaylistTile {
        pub cover: CoverImage,
        pub name: gtk::Label,
        pub count: gtk::Label,
    }

    impl Default for PlaylistTile {
        fn default() -> Self {
            let label = |classes: &[&str]| {
                gtk::Label::builder()
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .css_classes(classes)
                    .build()
            };
            Self {
                cover: glib::Object::builder()
                    .property("min-size", 120)
                    .property("natural-size", 180)
                    .build(),
                name: label(&["album-title"]),
                count: label(&["dim-label"]),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PlaylistTile {
        const NAME: &'static str = "GatefoldPlaylistTile";
        type Type = super::PlaylistTile;
        type ParentType = gtk::Box;
    }

    impl ObjectImpl for PlaylistTile {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_orientation(gtk::Orientation::Vertical);
            obj.set_spacing(3);
            obj.add_css_class("album-tile");
            self.cover.set_margin_bottom(6);
            obj.append(&self.cover);
            obj.append(&self.name);
            obj.append(&self.count);
        }
    }

    impl WidgetImpl for PlaylistTile {}
    impl BoxImpl for PlaylistTile {}
}

glib::wrapper! {
    pub struct PlaylistTile(ObjectSubclass<imp::PlaylistTile>)
        @extends gtk::Widget, gtk::Box,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl PlaylistTile {
    pub fn new() -> Self {
        glib::Object::new()
    }

    pub fn bind(&self, playlist: &PlaylistObject) {
        let imp = self.imp();
        let info = playlist.playlist();
        imp.cover.set_cover(playlist.cover_key().as_deref());
        imp.name.set_label(&info.name);
        imp.count.set_label(&format::song_count(info.item_count));
        self.set_tooltip_text(Some(&info.name));
    }
}

impl Default for PlaylistTile {
    fn default() -> Self {
        Self::new()
    }
}
