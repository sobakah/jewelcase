//! The bottom bar of the library window (CONCEPT.md §4.1). Clicking it opens
//! the full player. Its controls use `player.*` actions, which don't exist
//! until playback lands in Phase 1, so they show as insensitive for now.

use adw::subclass::prelude::*;
use gtk::glib;

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/sobakah/Gatefold/ui/mini_player.ui")]
    pub struct MiniPlayer {}

    #[glib::object_subclass]
    impl ObjectSubclass for MiniPlayer {
        const NAME: &'static str = "GatefoldMiniPlayer";
        type Type = super::MiniPlayer;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MiniPlayer {}
    impl WidgetImpl for MiniPlayer {}
    impl BinImpl for MiniPlayer {}
}

glib::wrapper! {
    pub struct MiniPlayer(ObjectSubclass<imp::MiniPlayer>)
        @extends gtk::Widget, adw::Bin,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}
