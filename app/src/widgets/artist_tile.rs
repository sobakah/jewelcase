//! A tile in the artist grid: portrait (a rounded square, like album
//! covers) and name (CONCEPT.md §4.1).

use adw::prelude::*;
use gatefold_core::library::covers::CoverSize;
use gtk::glib;
use gtk::subclass::prelude::*;

use super::CoverImage;
use crate::covers;
use crate::objects::ArtistObject;

mod imp {
    use super::*;

    #[derive(Debug)]
    pub struct ArtistTile {
        pub portrait: CoverImage,
        pub name: gtk::Label,
    }

    impl Default for ArtistTile {
        fn default() -> Self {
            Self {
                portrait: super::super::tile_cover(),
                // Narrow on its own: the portrait decides the tile's width.
                name: gtk::Label::builder()
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .width_chars(1)
                    .max_width_chars(1)
                    .css_classes(["album-title", "artist-name"])
                    .build(),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ArtistTile {
        const NAME: &'static str = "GatefoldArtistTile";
        type Type = super::ArtistTile;
        type ParentType = gtk::Box;
    }

    impl ObjectImpl for ArtistTile {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_orientation(gtk::Orientation::Vertical);
            obj.set_spacing(3);
            // The portrait's size, also in a wider cell.
            obj.set_halign(gtk::Align::Center);
            obj.add_css_class("artist-tile");
            self.portrait
                .set_placeholder_icon("avatar-default-symbolic");
            self.portrait.set_margin_bottom(6);
            obj.append(&self.portrait);
            obj.append(&self.name);
        }
    }

    impl WidgetImpl for ArtistTile {}
    impl BoxImpl for ArtistTile {}
}

glib::wrapper! {
    pub struct ArtistTile(ObjectSubclass<imp::ArtistTile>)
        @extends gtk::Widget, gtk::Box,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl ArtistTile {
    pub fn new() -> Self {
        glib::Object::new()
    }

    pub fn bind(&self, artist: &ArtistObject) {
        let imp = self.imp();
        let info = artist.artist();
        imp.name.set_label(&info.name);
        let portrait = info
            .portrait_key
            .as_deref()
            .and_then(|key| covers::portrait(key, CoverSize::Grid));
        imp.portrait.set_texture(portrait.as_ref());
        imp.portrait.set_label(&info.name);
        self.set_tooltip_text(Some(&info.name));
    }
}

impl Default for ArtistTile {
    fn default() -> Self {
        Self::new()
    }
}
