//! A tile in the album grid: cover, album title and album artist, one line
//! each (CONCEPT.md §4.1).

use gettextrs::gettext;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::CoverImage;
use crate::objects::AlbumObject;

mod imp {
    use super::*;

    #[derive(Debug)]
    pub struct AlbumTile {
        pub cover: CoverImage,
        pub title: gtk::Label,
        pub artist: gtk::Label,
    }

    impl Default for AlbumTile {
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
                title: label(&["album-title"]),
                artist: label(&["dim-label"]),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AlbumTile {
        const NAME: &'static str = "GatefoldAlbumTile";
        type Type = super::AlbumTile;
        type ParentType = gtk::Box;
    }

    impl ObjectImpl for AlbumTile {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_orientation(gtk::Orientation::Vertical);
            obj.set_spacing(3);
            obj.add_css_class("album-tile");
            self.cover.set_margin_bottom(6);
            obj.append(&self.cover);
            obj.append(&self.title);
            obj.append(&self.artist);
        }
    }

    impl WidgetImpl for AlbumTile {}
    impl BoxImpl for AlbumTile {}
}

glib::wrapper! {
    pub struct AlbumTile(ObjectSubclass<imp::AlbumTile>)
        @extends gtk::Widget, gtk::Box,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl AlbumTile {
    pub fn new() -> Self {
        glib::Object::new()
    }

    pub fn bind(&self, album: &AlbumObject) {
        let imp = self.imp();
        let info = album.album();
        let artist = artist_or_unknown(&info.artist_credit);
        imp.title.set_label(&info.title);
        imp.artist.set_label(&artist);
        imp.cover.set_cover(info.cover_key.as_deref());
        // Translators: accessible label of an album cover; {} is the album title.
        imp.cover
            .set_label(&gettext("Cover of {}").replace("{}", &info.title));
        self.set_tooltip_text(Some(&format!("{} — {artist}", info.title)));
    }
}

impl Default for AlbumTile {
    fn default() -> Self {
        Self::new()
    }
}

impl AlbumTile {
    /// Cover and title only, for the artist page (CONCEPT.md §4.2).
    pub fn bind_title_only(&self, album: &AlbumObject) {
        self.bind(album);
        self.imp().artist.set_visible(false);
        self.set_tooltip_text(Some(&album.album().title));
    }
}

pub fn artist_or_unknown(credit: &str) -> String {
    if credit.is_empty() {
        gettext("Unknown Artist")
    } else {
        credit.to_owned()
    }
}
