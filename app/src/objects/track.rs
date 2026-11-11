use std::cell::OnceCell;

use gtk::glib;
use gtk::subclass::prelude::*;
use jewelcase_core::db::Track;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct TrackObject {
        pub track: OnceCell<Track>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TrackObject {
        const NAME: &'static str = "JewelcaseTrackObject";
        type Type = super::TrackObject;
    }

    impl ObjectImpl for TrackObject {}
}

glib::wrapper! {
    pub struct TrackObject(ObjectSubclass<imp::TrackObject>);
}

impl TrackObject {
    pub fn new(track: Track) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().track.set(track).expect("set once");
        obj
    }

    pub fn track(&self) -> &Track {
        self.imp().track.get().expect("set in new()")
    }

    pub fn id(&self) -> i64 {
        self.track().record.id
    }

    /// A Spotify song that isn't in the library (remote mode): no file, no
    /// lyrics, no details.
    pub fn is_stand_in(&self) -> bool {
        self.id() < 0
    }

    pub fn title(&self) -> String {
        let record = &self.track().record;
        record.tags.title.clone().unwrap_or_else(|| {
            record
                .path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
    }

    /// The track artist, falling back to the album artist.
    pub fn artist(&self) -> Option<String> {
        let track = self.track();
        track
            .record
            .tags
            .artist
            .clone()
            .or_else(|| track.album_artist.clone().filter(|a| !a.is_empty()))
    }

    /// The disc as shown in the album panel.
    pub fn disc(&self) -> u32 {
        let track = self.track();
        track.album_disc.or(track.record.tags.disc).unwrap_or(1)
    }

    pub fn duration_ms(&self) -> u64 {
        self.track().record.tags.duration_ms
    }

    /// The album's cover, falling back to the track's own.
    pub fn cover_key(&self) -> Option<&str> {
        let track = self.track();
        track
            .album_cover_key
            .as_deref()
            .or(track.record.cover_key.as_deref())
    }
}
