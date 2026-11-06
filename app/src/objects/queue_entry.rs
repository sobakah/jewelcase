use std::cell::OnceCell;

use gatefold_core::queue::Origin;
use gtk::glib;
use gtk::subclass::prelude::*;

use super::TrackObject;

#[derive(Debug)]
pub struct Entry {
    /// Position relative to the current track (0 = now playing).
    pub offset: usize,
    pub track: TrackObject,
    pub origin: Origin,
}

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct QueueEntry {
        pub entry: OnceCell<Entry>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for QueueEntry {
        const NAME: &'static str = "GatefoldQueueEntry";
        type Type = super::QueueEntry;
    }

    impl ObjectImpl for QueueEntry {}
}

glib::wrapper! {
    /// One row of the queue view (CONCEPT.md §6.2).
    pub struct QueueEntry(ObjectSubclass<imp::QueueEntry>);
}

impl QueueEntry {
    pub fn new(offset: usize, track: TrackObject, origin: Origin) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp()
            .entry
            .set(Entry {
                offset,
                track,
                origin,
            })
            .expect("set once");
        obj
    }

    pub fn entry(&self) -> &Entry {
        self.imp().entry.get().expect("set in new()")
    }
}
