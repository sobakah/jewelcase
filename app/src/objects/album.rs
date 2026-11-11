use std::cell::OnceCell;

use gtk::glib;
use gtk::subclass::prelude::*;
use jewelcase_core::db::Album;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct AlbumObject {
        /// The editions of this album; the first is the preferred one (§3.4).
        pub versions: OnceCell<Vec<Album>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AlbumObject {
        const NAME: &'static str = "JewelcaseAlbumObject";
        type Type = super::AlbumObject;
    }

    impl ObjectImpl for AlbumObject {}
}

glib::wrapper! {
    /// One album group in the library grid: an album and its merged editions.
    pub struct AlbumObject(ObjectSubclass<imp::AlbumObject>);
}

impl AlbumObject {
    pub fn new(versions: Vec<Album>) -> Self {
        assert!(!versions.is_empty(), "an album has at least one version");
        let obj: Self = glib::Object::new();
        obj.imp().versions.set(versions).expect("set once");
        obj
    }

    pub fn versions(&self) -> &[Album] {
        self.imp().versions.get().expect("set in new()")
    }

    /// The preferred version, shown in the grid.
    pub fn album(&self) -> &Album {
        &self.versions()[0]
    }

    pub fn group_key(&self) -> &str {
        &self.album().group_key
    }
}
