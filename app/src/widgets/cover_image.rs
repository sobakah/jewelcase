//! A square cover. It takes the width it's given (so grid columns always
//! fill the row, CONCEPT.md §4.1) and stays square inside any allocation.

use std::cell::{Cell, RefCell};

use gatefold_core::library::covers::CoverSize;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::covers;

mod imp {
    use super::*;

    #[derive(Debug, glib::Properties)]
    #[properties(wrapper_type = super::CoverImage)]
    pub struct CoverImage {
        /// The smallest side length.
        #[property(get, set = Self::set_min_size)]
        pub min_size: Cell<i32>,
        /// The preferred side length.
        #[property(get, set = Self::set_natural_size)]
        pub natural_size: Cell<i32>,
        /// Thumbnail size: "grid", "panel" or "player".
        #[property(get, set)]
        pub kind: RefCell<String>,
        /// Whether the height may shrink below the width (the cover then
        /// stays square and centered). Off: height always equals width.
        #[property(get, set = Self::set_shrinkable)]
        pub shrinkable: Cell<bool>,
        pub picture: gtk::Picture,
        pub placeholder: gtk::Image,
    }

    impl Default for CoverImage {
        fn default() -> Self {
            Self {
                min_size: Cell::new(48),
                natural_size: Cell::new(180),
                kind: RefCell::new("grid".into()),
                shrinkable: Cell::new(false),
                picture: gtk::Picture::builder()
                    .content_fit(gtk::ContentFit::Cover)
                    .can_shrink(true)
                    .css_classes(["cover"])
                    .overflow(gtk::Overflow::Hidden)
                    .visible(false)
                    .build(),
                placeholder: gtk::Image::builder()
                    .icon_name("folder-music-symbolic")
                    .css_classes(["cover", "cover-placeholder"])
                    .overflow(gtk::Overflow::Hidden)
                    .build(),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CoverImage {
        const NAME: &'static str = "GatefoldCoverImage";
        type Type = super::CoverImage;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_accessible_role(gtk::AccessibleRole::Img);
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for CoverImage {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            self.picture.set_parent(&*obj);
            self.placeholder.set_parent(&*obj);
            self.update_placeholder_size();
        }

        fn dispose(&self) {
            self.picture.unparent();
            self.placeholder.unparent();
        }
    }

    impl WidgetImpl for CoverImage {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let min = self.min_size.get();
            let natural = self.natural_size.get().max(min);
            if min == natural {
                // A fixed size, like the mini player's cover.
                return (min, min, -1, -1);
            }
            match orientation {
                gtk::Orientation::Horizontal => (min, natural, -1, -1),
                _ if for_size >= 0 && self.shrinkable.get() => {
                    (min.min(for_size).max(0), for_size.max(min), -1, -1)
                }
                _ if for_size >= 0 => (for_size, for_size, -1, -1),
                _ => (min, natural, -1, -1),
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            let side = width.min(height);
            let x = (width - side) / 2;
            let y = (height - side) / 2;
            let transform = gtk::gsk::Transform::new()
                .translate(&gtk::graphene::Point::new(x as f32, y as f32));
            for child in [
                self.picture.upcast_ref::<gtk::Widget>(),
                self.placeholder.upcast_ref(),
            ] {
                child.allocate(side, side, baseline, Some(transform.clone()));
            }
        }
    }

    impl CoverImage {
        fn set_min_size(&self, size: i32) {
            self.min_size.set(size);
            self.update_placeholder_size();
            self.obj().queue_resize();
        }

        fn set_shrinkable(&self, shrinkable: bool) {
            self.shrinkable.set(shrinkable);
            self.obj().queue_resize();
        }

        fn set_natural_size(&self, size: i32) {
            self.natural_size.set(size);
            self.obj().queue_resize();
        }

        fn update_placeholder_size(&self) {
            self.placeholder
                .set_pixel_size((self.min_size.get() / 2).clamp(16, 128));
        }
    }
}

glib::wrapper! {
    pub struct CoverImage(ObjectSubclass<imp::CoverImage>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl CoverImage {
    /// Shows the cover with this cache key, or the placeholder.
    pub fn set_cover(&self, key: Option<&str>) {
        let imp = self.imp();
        let size = match imp.kind.borrow().as_str() {
            "panel" => CoverSize::Panel,
            "player" => CoverSize::Player,
            _ => CoverSize::Grid,
        };
        let texture = key.and_then(|key| covers::texture(key, size));
        imp.picture.set_paintable(texture.as_ref());
        imp.picture.set_visible(texture.is_some());
        imp.placeholder.set_visible(texture.is_none());
    }

    /// The accessible label, e.g. "Cover of 25".
    pub fn set_label(&self, label: &str) {
        self.update_property(&[gtk::accessible::Property::Label(label)]);
    }
}
