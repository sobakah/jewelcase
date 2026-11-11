use std::cell::{OnceCell, RefCell};

use gtk::glib;
use gtk::subclass::prelude::*;
use jewelcase_core::db::Playlist;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct PlaylistObject {
        pub playlist: OnceCell<Playlist>,
        /// The cover to show: custom, mosaic, or a single album cover.
        pub cover_key: RefCell<Option<String>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PlaylistObject {
        const NAME: &'static str = "JewelcasePlaylistObject";
        type Type = super::PlaylistObject;
    }

    impl ObjectImpl for PlaylistObject {}
}

glib::wrapper! {
    pub struct PlaylistObject(ObjectSubclass<imp::PlaylistObject>);
}

impl PlaylistObject {
    pub fn new(playlist: Playlist, cover_key: Option<String>) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().playlist.set(playlist).expect("set once");
        obj.imp().cover_key.replace(cover_key);
        obj
    }

    pub fn playlist(&self) -> &Playlist {
        self.imp().playlist.get().expect("set in new()")
    }

    pub fn id(&self) -> i64 {
        self.playlist().id
    }

    pub fn cover_key(&self) -> Option<String> {
        self.imp().cover_key.borrow().clone()
    }
}
