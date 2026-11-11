//! A square cover. It takes the width it's given, up to its natural size
//! when capped (grid tiles keep the size set in Preferences, CONCEPT.md
//! §4.1), and stays square inside any allocation.

use std::cell::{Cell, RefCell};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};
use jewelcase_core::library::covers::CoverSize;

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
        /// Never larger than the natural size (grid tiles in wider cells).
        #[property(get, set = Self::set_capped)]
        pub capped: Cell<bool>,
        pub picture: gtk::Picture,
        pub placeholder: gtk::Image,
        /// The cache key shown by `set_cover`.
        pub key: RefCell<Option<String>>,
        /// Drawn in a CD jewel case (a hidden extra: window.rs).
        #[property(get, set = Self::set_case)]
        pub case: Cell<bool>,
    }

    impl Default for CoverImage {
        fn default() -> Self {
            Self {
                min_size: Cell::new(48),
                natural_size: Cell::new(180),
                kind: RefCell::new("grid".into()),
                shrinkable: Cell::new(false),
                capped: Cell::new(false),
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
                key: RefCell::default(),
                case: Cell::new(false),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CoverImage {
        const NAME: &'static str = "JewelcaseCoverImage";
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
            let for_size = if self.capped.get() && for_size >= 0 {
                for_size.min(natural)
            } else {
                for_size
            };
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
            let mut side = width.min(height);
            if self.capped.get() {
                side = side.min(self.natural_size.get().max(self.min_size.get()));
            }
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
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            self.parent_snapshot(snapshot);
            if self.case.get()
                && self.picture.is_visible()
                && let Some(bounds) = self.picture.compute_bounds(&*self.obj())
            {
                let dark = adw::StyleManager::default().is_dark();
                super::draw_case(snapshot, &bounds, dark);
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

        fn set_capped(&self, capped: bool) {
            self.capped.set(capped);
            self.obj().queue_resize();
        }

        fn set_case(&self, case: bool) {
            if self.case.replace(case) != case {
                self.obj().queue_draw();
            }
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
        self.show(texture.as_ref(), texture.as_ref().and(key));
    }

    /// The cache key of the cover shown (`None` for other images).
    pub fn key(&self) -> Option<String> {
        self.imp().key.borrow().clone()
    }

    /// The picture showing the cover (empty while the placeholder shows).
    pub fn picture(&self) -> gtk::Picture {
        self.imp().picture.clone()
    }

    /// Shows an image (e.g. an artist portrait), or the placeholder.
    pub fn set_texture(&self, texture: Option<&gtk::gdk::Texture>) {
        self.show(texture, None);
    }

    /// `key`: the cover cache key of `texture`, if it's a cover.
    fn show(&self, texture: Option<&gtk::gdk::Texture>, key: Option<&str>) {
        let imp = self.imp();
        // Before the picture changes: the backdrop asks for it then.
        imp.key.replace(key.map(str::to_owned));
        imp.picture.set_paintable(texture);
        imp.picture.set_visible(texture.is_some());
        imp.placeholder.set_visible(texture.is_none());
    }

    /// The icon shown without an image (default: a music folder).
    pub fn set_placeholder_icon(&self, icon_name: &str) {
        self.imp().placeholder.set_icon_name(Some(icon_name));
    }

    /// The accessible label, e.g. "Cover of 25".
    pub fn set_label(&self, label: &str) {
        self.update_property(&[gtk::accessible::Property::Label(label)]);
    }
}

/// A CD jewel case over the cover in `bounds`, like the app icon's: the
/// hinge on the left, the clear lid's frame around the booklet, a glare
/// across it and the case's edge below.
fn draw_case(snapshot: &gtk::Snapshot, bounds: &graphene::Rect, dark: bool) {
    let (x, y, side) = (bounds.x(), bounds.y(), bounds.width());
    let u = |part: f32| part * side;
    let rgba = |hex: u32, alpha: f32| {
        gdk::RGBA::new(
            ((hex >> 16) & 0xff) as f32 / 255.0,
            ((hex >> 8) & 0xff) as f32 / 255.0,
            (hex & 0xff) as f32 / 255.0,
            alpha,
        )
    };
    // GNOME's palette: the icon's light greys, smoked in the dark.
    let (frame, hinge, edge, shine) = if dark {
        (
            rgba(0x3d3846, 0.94),
            rgba(0x5e5c64, 1.0),
            rgba(0x241f31, 1.0),
            0.25,
        )
    } else {
        (
            rgba(0xdeddda, 0.94),
            rgba(0x9a9996, 1.0),
            rgba(0x5e5c64, 1.0),
            0.6,
        )
    };
    let outer = gsk::RoundedRect::from_rect(*bounds, u(0.035));
    let hinge_width = u(0.09);
    let edge_height = u(0.03);
    // The booklet behind the lid.
    let booklet = graphene::Rect::new(
        x + u(0.12),
        y + u(0.04),
        side - u(0.12) - u(0.04),
        side - u(0.04) - u(0.065),
    );

    // The lid's frame: the case without the booklet.
    let builder = gsk::PathBuilder::new();
    builder.add_rounded_rect(&outer);
    builder.add_rect(&booklet);
    snapshot.append_fill(&builder.to_path(), gsk::FillRule::EvenOdd, &frame);

    // The hinge and the edge, inside the case's rounded corners.
    snapshot.push_rounded_clip(&outer);
    snapshot.append_color(&hinge, &graphene::Rect::new(x, y, hinge_width, side));
    snapshot.append_color(
        &edge,
        &graphene::Rect::new(x, y + side - edge_height, side, edge_height),
    );
    snapshot.pop();
    // The hinge's ridges.
    let ridges = gsk::PathBuilder::new();
    for at in [0.14, 0.2, 0.8, 0.86] {
        ridges.move_to(x + u(0.025), y + u(at));
        ridges.line_to(x + hinge_width - u(0.025), y + u(at));
    }
    let stroke = gsk::Stroke::new(u(0.018).max(1.0));
    stroke.set_line_cap(gsk::LineCap::Round);
    snapshot.append_stroke(&ridges.to_path(), &stroke, &rgba(0xf6f5f4, 0.9));

    // The booklet's edge under the lid.
    let rim = gsk::PathBuilder::new();
    rim.add_rect(&booklet);
    snapshot.append_stroke(
        &rim.to_path(),
        &gsk::Stroke::new(1.0),
        &rgba(0x000000, 0.25),
    );

    // A glare across the lid's top left.
    let (bx, by, bw, bh) = (booklet.x(), booklet.y(), booklet.width(), booklet.height());
    let glare = gsk::PathBuilder::new();
    glare.move_to(bx, by);
    glare.line_to(bx + 0.6 * bw, by);
    glare.line_to(bx, by + 0.7 * bh);
    glare.close();
    snapshot.push_fill(&glare.to_path(), gsk::FillRule::Winding);
    snapshot.append_linear_gradient(
        &booklet,
        &graphene::Point::new(bx, by),
        &graphene::Point::new(bx + 0.32 * bw, by + 0.37 * bh),
        &[
            gsk::ColorStop::new(0.0, rgba(0xffffff, 0.5)),
            gsk::ColorStop::new(1.0, rgba(0xffffff, 0.0)),
        ],
    );
    snapshot.pop();

    // The lid's bright outline.
    let inset = graphene::Rect::new(x + 0.5, y + 0.5, side - 1.0, side - 1.0);
    let outline = gsk::PathBuilder::new();
    outline.add_rounded_rect(&gsk::RoundedRect::from_rect(inset, u(0.035) - 0.5));
    snapshot.append_stroke(
        &outline.to_path(),
        &gsk::Stroke::new(1.0),
        &rgba(0xffffff, shine),
    );
}
