//! The player's look in the side panels (CONCEPT.md §4.1, §4.2): the
//! cover, blurred, behind the top of an album, playlist or artist panel,
//! fading out into the panel's own background further down.

use std::cell::RefCell;

use adw::subclass::prelude::*;
use gtk::prelude::*;
use gtk::{gdk, glib, graphene, gsk};

use super::CoverImage;

/// The player's backdrop (`.player-backdrop` in style.css, `.player-tint`
/// in theme.rs): its opacity, blur and tint, top to bottom.
const OPACITY: f32 = 0.35;
const BLUR: f32 = 60.0;
const TINT: [(f32, f32); 3] = [(0.0, 0.55), (0.55, 0.18), (1.0, 0.05)];
/// The backdrop's height, relative to the panel's width, and at most.
const HEIGHT_PER_WIDTH: f32 = 1.4;
const MAX_HEIGHT: f32 = 760.0;
/// Where the fade starts, as part of the backdrop's height.
const FADE_FROM: f32 = 0.45;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct CoverBackdrop {
        pub texture: RefCell<Option<gdk::Texture>>,
        /// The cover's dominant color (from the scan), as the player's
        /// tint: a blurred cover of many colors alone turns grey.
        pub tint: std::cell::Cell<Option<[u8; 3]>>,
        /// A height other than the default (see `set_max_height`).
        pub max_height: std::cell::Cell<Option<f32>>,
        pub followed: RefCell<Option<(glib::WeakRef<gtk::Picture>, glib::SignalHandlerId)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CoverBackdrop {
        const NAME: &'static str = "GatefoldCoverBackdrop";
        type Type = super::CoverBackdrop;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for CoverBackdrop {
        fn dispose(&self) {
            if let Some((picture, handler)) = self.followed.take()
                && let Some(picture) = picture.upgrade()
            {
                picture.disconnect(handler);
            }
        }
    }

    impl WidgetImpl for CoverBackdrop {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            if let Some(texture) = self.texture.borrow().as_ref() {
                let max_height = self.max_height.get().unwrap_or(MAX_HEIGHT);
                let width = obj.width() as f32;
                let full_height = obj.height() as f32;
                let height = (width * HEIGHT_PER_WIDTH).min(max_height).min(full_height);
                // The player shows the cover as large as the window.
                let window = obj
                    .root()
                    .map(|root| (root.width() as f32, root.height() as f32))
                    .unwrap_or((width, full_height));
                if width > 0.0 && height > 0.0 {
                    draw(
                        snapshot,
                        texture,
                        self.tint.get(),
                        Size {
                            width,
                            height,
                            full_height,
                            window,
                        },
                    );
                }
            }
            self.parent_snapshot(snapshot);
        }
    }

    impl BinImpl for CoverBackdrop {}
}

glib::wrapper! {
    pub struct CoverBackdrop(ObjectSubclass<imp::CoverBackdrop>)
        @extends gtk::Widget, adw::Bin,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl CoverBackdrop {
    /// At most `max_height` px tall (wide panels like the artist page: a
    /// lower band).
    pub fn set_max_height(&self, max_height: f32) {
        self.imp().max_height.set(Some(max_height));
        self.queue_draw();
    }

    /// Shows what `cover` shows, also when it changes later (covers that
    /// arrive after a download).
    pub fn follow(&self, cover: &CoverImage) {
        let picture = cover.picture();
        let imp = self.imp();
        if let Some((old, handler)) = imp.followed.take()
            && let Some(old) = old.upgrade()
        {
            old.disconnect(handler);
        }
        let handler = picture.connect_paintable_notify(glib::clone!(
            #[weak(rename_to = backdrop)]
            self,
            #[weak]
            cover,
            move |picture| backdrop.show_picture(picture, &cover)
        ));
        self.show_picture(&picture, cover);
        imp.followed.replace(Some((picture.downgrade(), handler)));
    }

    fn show_picture(&self, picture: &gtk::Picture, cover: &CoverImage) {
        let texture = picture.paintable().and_downcast::<gdk::Texture>();
        let imp = self.imp();
        imp.tint
            .set(cover.key().as_deref().and_then(crate::covers::color));
        imp.texture.replace(texture);
        self.queue_draw();
    }
}

/// Where the backdrop is drawn: `width` × `height` at the top of a widget
/// `full_height` tall, in a window of the size `window`.
struct Size {
    width: f32,
    height: f32,
    full_height: f32,
    window: (f32, f32),
}

/// The blurred, slightly more saturated cover over `width` × `height`,
/// faded out from `FADE_FROM` down: the player's backdrop, at the same
/// size (the window's) and centered on the widget.
fn draw(snapshot: &gtk::Snapshot, texture: &gdk::Texture, tint: Option<[u8; 3]>, size: Size) {
    let Size {
        width,
        height,
        full_height,
        window,
    } = size;
    let area = graphene::Rect::new(0.0, 0.0, width, height);
    snapshot.push_clip(&area);
    snapshot.push_mask(gsk::MaskMode::Alpha);
    // The mask: opaque at the top, transparent at the bottom.
    snapshot.append_linear_gradient(
        &area,
        &graphene::Point::new(0.0, height * FADE_FROM),
        &graphene::Point::new(0.0, height),
        &[
            gsk::ColorStop::new(0.0, gdk::RGBA::new(0.0, 0.0, 0.0, 1.0)),
            gsk::ColorStop::new(1.0, gdk::RGBA::new(0.0, 0.0, 0.0, 0.0)),
        ],
    );
    snapshot.pop();
    snapshot.push_opacity(f64::from(OPACITY));
    snapshot.push_color_matrix(&saturation(1.3), &graphene::Vec4::zero());
    snapshot.push_blur(f64::from(BLUR));
    // Square like the cover, as large as in the player (which fills the
    // window), and at least so large that the blur doesn't fade at the
    // widget's edges.
    let side = window
        .0
        .max(window.1)
        .max(width + 4.0 * BLUR)
        .max(full_height + 4.0 * BLUR);
    snapshot.append_texture(
        texture,
        &graphene::Rect::new((width - side) / 2.0, (full_height - side) / 2.0, side, side),
    );
    snapshot.pop();
    snapshot.pop();
    snapshot.pop();
    // The tint, like the player's: strongest at the top.
    if let Some([r, g, b]) = tint {
        let color = |alpha: f32| {
            gdk::RGBA::new(
                f32::from(r) / 255.0,
                f32::from(g) / 255.0,
                f32::from(b) / 255.0,
                alpha,
            )
        };
        snapshot.append_linear_gradient(
            &area,
            &graphene::Point::new(0.0, 0.0),
            &graphene::Point::new(0.0, height),
            &TINT.map(|(at, alpha)| gsk::ColorStop::new(at, color(alpha))),
        );
    }
    snapshot.pop();
    snapshot.pop();
}

/// A color matrix that changes saturation by `amount` (1: unchanged).
fn saturation(amount: f32) -> graphene::Matrix {
    // Rec. 709 luma weights.
    let (r, g, b) = (0.2126, 0.7152, 0.0722);
    let s = amount;
    let row = |luma: [f32; 3], own: usize| -> [f32; 4] {
        let mut row = [0.0; 4];
        for (i, l) in luma.iter().enumerate() {
            row[i] = l * (1.0 - s) + if i == own { s } else { 0.0 };
        }
        row
    };
    let luma = [r, g, b];
    let (red, green, blue) = (row(luma, 0), row(luma, 1), row(luma, 2));
    // graphene matrices are row-major with vectors multiplied from the
    // left, so the rows here are the columns of the usual matrix.
    graphene::Matrix::from_float([
        red[0], green[0], blue[0], 0.0, //
        red[1], green[1], blue[1], 0.0, //
        red[2], green[2], blue[2], 0.0, //
        0.0, 0.0, 0.0, 1.0,
    ])
}
