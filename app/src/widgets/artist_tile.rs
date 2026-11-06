//! A tile in the artist grid: circular portrait and name (CONCEPT.md §4.1).

use adw::prelude::*;
use gatefold_core::library::covers::CoverSize;
use gtk::glib;
use gtk::subclass::prelude::*;

use crate::covers;
use crate::objects::ArtistObject;

mod imp {
    use super::*;

    #[derive(Debug)]
    pub struct ArtistTile {
        pub avatar: adw::Avatar,
        pub name: gtk::Label,
    }

    impl Default for ArtistTile {
        fn default() -> Self {
            Self {
                avatar: adw::Avatar::builder()
                    .size(140)
                    .show_initials(true)
                    .halign(gtk::Align::Center)
                    .build(),
                name: gtk::Label::builder()
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .justify(gtk::Justification::Center)
                    .css_classes(["artist-name"])
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
            obj.set_spacing(9);
            obj.add_css_class("artist-tile");
            obj.append(&self.avatar);
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
        imp.avatar.set_text(Some(&info.name));
        let portrait = info
            .portrait_key
            .as_deref()
            .and_then(|key| covers::portrait(key, CoverSize::Grid));
        imp.avatar.set_custom_image(portrait.as_ref());
        self.set_tooltip_text(Some(&info.name));
    }
}

impl Default for ArtistTile {
    fn default() -> Self {
        Self::new()
    }
}
