//! The full player, shown in the expanded bottom sheet (CONCEPT.md §4.3).
//! Phase 0 has the sleek layout only; lyrics and queue panes come in Phase 3.

use adw::subclass::prelude::*;
use gtk::glib;

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/sobakah/Gatefold/ui/player_view.ui")]
    pub struct PlayerView {}

    #[glib::object_subclass]
    impl ObjectSubclass for PlayerView {
        const NAME: &'static str = "GatefoldPlayerView";
        type Type = super::PlayerView;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for PlayerView {}
    impl WidgetImpl for PlayerView {}
    impl BinImpl for PlayerView {}
}

glib::wrapper! {
    pub struct PlayerView(ObjectSubclass<imp::PlayerView>)
        @extends gtk::Widget, adw::Bin,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}
