use std::cell::OnceCell;

use gtk::glib;
use gtk::subclass::prelude::*;
use jewelcase_core::db::Artist;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct ArtistObject {
        pub artist: OnceCell<Artist>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ArtistObject {
        const NAME: &'static str = "JewelcaseArtistObject";
        type Type = super::ArtistObject;
    }

    impl ObjectImpl for ArtistObject {}
}

glib::wrapper! {
    /// An album artist in the artist grid (CONCEPT.md §3.3).
    pub struct ArtistObject(ObjectSubclass<imp::ArtistObject>);
}

impl ArtistObject {
    pub fn new(artist: Artist) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().artist.set(artist).expect("set once");
        obj
    }

    pub fn artist(&self) -> &Artist {
        self.imp().artist.get().expect("set in new()")
    }

    pub fn id(&self) -> i64 {
        self.artist().id
    }
}
