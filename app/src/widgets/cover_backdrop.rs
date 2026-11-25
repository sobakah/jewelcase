//! The player's look in the side panels (CONCEPT.md §4.1, §4.2): the
//! cover, blurred, behind the top of an album, playlist or artist panel,
//! fading out into the panel's own background further down. Over it a
//! disc's sheen, as on Jewelcase Mobile's pages: fine grooves centred on
//! the cover like a CD's data side, and the "bow tie" of light a disc
//! throws, two opposite wedges in the cover's colours with a little of the
//! rainbow.

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
/// The grooves: one every `GROOVE_STEP` px, brighter and dimmer in bands
/// `BAND_STEP` px wide, from `GROOVE_ALPHA.0` to `.1` (of the text colour).
const GROOVE_STEP: f32 = 3.0;
const BAND_STEP: f32 = 37.0;
const GROOVE_ALPHA: (f32, f32) = (0.03, 0.08);
/// The wedges' turn (degrees, clockwise from the top: one wedge on the
/// right going down, the other on the left going up, as on the phone) and
/// their colours' opacity, from the inner edge to the outer.
const WEDGE_TURN: f32 = 60.0;
const WEDGE_ALPHA: [f32; 4] = [0.20, 0.36, 0.30, 0.18];
/// How much of a wedge's colour is the cover's (the rest the rainbow's),
/// and how bright a cover's colour is as light: on a dark page at least
/// `LIFTED_VALUE`; on a light one within `LIGHT_PAGE_VALUE`, else it
/// wouldn't show.
const COVER_SHARE: f32 = 0.7;
const LIFTED_VALUE: f32 = 0.92;
const LIGHT_PAGE_VALUE: (f32, f32) = (0.55, 0.75);

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
        /// The cover followed, for the sheen's centre.
        pub cover: glib::WeakRef<CoverImage>,
        /// The cover's main colours, for the sheen's wedges.
        pub palette: RefCell<Option<Vec<[u8; 3]>>>,
        /// Where the cover's centre and radius were last seen, as if the
        /// page were scrolled to the top (the sheen doesn't move with the
        /// scrolling, like the blurred cover).
        pub disc: std::cell::Cell<Option<(f32, f32, f32)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CoverBackdrop {
        const NAME: &'static str = "JewelcaseCoverBackdrop";
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
                    let groove = obj.color();
                    // Light text: a dark page.
                    let dark = groove.red() + groove.green() + groove.blue() > 1.5;
                    let palette = self.palette.borrow();
                    let sheen =
                        self.disc_place()
                            .zip(palette.as_ref())
                            .map(|((x, y, radius), palette)| Sheen {
                                center: graphene::Point::new(x, y),
                                radius,
                                hues: sheen_hues(self.tint.get(), palette, dark),
                                groove,
                            });
                    draw(
                        snapshot,
                        texture,
                        self.tint.get(),
                        sheen,
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

    impl CoverBackdrop {
        /// The cover's centre and radius here, as if scrolled to the top;
        /// the last place seen while the cover is out of the page (a
        /// playlist's top, scrolled far down, leaves its list).
        fn disc_place(&self) -> Option<(f32, f32, f32)> {
            let obj = self.obj();
            if let Some(cover) = self.cover.upgrade()
                && cover.is_mapped()
                && let Some(point) = cover.compute_point(
                    &*obj,
                    &graphene::Point::new(cover.width() as f32 / 2.0, cover.height() as f32 / 2.0),
                )
            {
                let scrolled = cover
                    .ancestor(gtk::ScrolledWindow::static_type())
                    .and_downcast::<gtk::ScrolledWindow>()
                    .map_or(0.0, |s| s.vadjustment().value() as f32);
                let radius = cover.width().min(cover.height()) as f32 / 2.0;
                self.disc
                    .set(Some((point.x(), point.y() + scrolled, radius)));
            }
            self.disc.get()
        }
    }
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
        imp.cover.set(Some(cover));
        imp.disc.set(None);
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
        let tint = cover.key().as_deref().and_then(crate::covers::color);
        imp.tint.set(tint);
        imp.palette.replace(texture.as_ref().map(texture_palette));
        imp.texture.replace(texture);
        self.queue_draw();
    }
}

/// The sheen over the backdrop: centred on the cover (`radius` its half
/// side), the wedges in `hues`, the grooves in `groove` (the text colour).
struct Sheen {
    center: graphene::Point,
    radius: f32,
    hues: [gdk::RGBA; 4],
    groove: gdk::RGBA,
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
fn draw(
    snapshot: &gtk::Snapshot,
    texture: &gdk::Texture,
    tint: Option<[u8; 3]>,
    sheen: Option<Sheen>,
    size: Size,
) {
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
    if let Some(sheen) = sheen {
        draw_sheen(snapshot, &area, &sheen);
    }
    snapshot.pop();
    snapshot.pop();
}

/// The grooves and the bow tie of light over `area`, strongest near the
/// cover and fading away from it (the backdrop's fade ends them too).
fn draw_sheen(snapshot: &gtk::Snapshot, area: &graphene::Rect, sheen: &Sheen) {
    let Sheen {
        center,
        radius,
        hues,
        groove,
    } = sheen;
    let corner = |x: f32, y: f32| (x - center.x()).hypot(y - center.y());
    let outer = [
        corner(area.x(), area.y()),
        corner(area.x() + area.width(), area.y()),
        corner(area.x(), area.y() + area.height()),
        corner(area.x() + area.width(), area.y() + area.height()),
    ]
    .into_iter()
    .fold(0.0, f32::max)
        + 8.0;
    if outer <= *radius {
        return;
    }
    let clear = |color: &gdk::RGBA| gdk::RGBA::new(color.red(), color.green(), color.blue(), 0.0);
    let with_alpha = |color: &gdk::RGBA, alpha: f32| {
        gdk::RGBA::new(color.red(), color.green(), color.blue(), alpha)
    };
    // The light fades with the distance from the disc.
    snapshot.push_mask(gsk::MaskMode::Alpha);
    snapshot.append_radial_gradient(
        area,
        center,
        outer,
        outer,
        0.0,
        1.0,
        &[
            gsk::ColorStop::new(0.0, gdk::RGBA::BLACK),
            gsk::ColorStop::new(0.35, gdk::RGBA::BLACK),
            gsk::ColorStop::new(1.0, gdk::RGBA::new(0.0, 0.0, 0.0, 0.15)),
        ],
    );
    snapshot.pop();

    // The grooves: thin rings, brighter and dimmer in bands as a disc's
    // tracks are.
    snapshot.push_mask(gsk::MaskMode::Alpha);
    let low = GROOVE_ALPHA.0 / GROOVE_ALPHA.1;
    snapshot.append_repeating_radial_gradient(
        area,
        center,
        BAND_STEP,
        BAND_STEP,
        0.0,
        1.0,
        &[
            gsk::ColorStop::new(0.0, gdk::RGBA::new(0.0, 0.0, 0.0, low)),
            gsk::ColorStop::new(0.5, gdk::RGBA::BLACK),
            gsk::ColorStop::new(1.0, gdk::RGBA::new(0.0, 0.0, 0.0, low)),
        ],
    );
    snapshot.pop();
    let line = with_alpha(groove, GROOVE_ALPHA.1);
    snapshot.append_repeating_radial_gradient(
        area,
        center,
        GROOVE_STEP,
        GROOVE_STEP,
        0.0,
        1.0,
        &[
            gsk::ColorStop::new(0.0, clear(groove)),
            gsk::ColorStop::new(0.3, clear(groove)),
            gsk::ColorStop::new(0.5, line),
            gsk::ColorStop::new(0.7, clear(groove)),
            gsk::ColorStop::new(1.0, clear(groove)),
        ],
    );
    snapshot.pop();

    // The bow tie: two opposite wedges.
    let [a, b, c, d] = hues;
    let none = clear(a);
    let wedge = |from: f32| {
        [
            gsk::ColorStop::new(from, none),
            gsk::ColorStop::new(from + 0.04, *a),
            gsk::ColorStop::new(from + 0.09, *b),
            gsk::ColorStop::new(from + 0.14, *c),
            gsk::ColorStop::new(from + 0.19, *d),
            gsk::ColorStop::new(from + 0.24, none),
        ]
    };
    let mut stops = vec![gsk::ColorStop::new(0.0, none)];
    stops.extend(wedge(0.0).into_iter().skip(1));
    stops.extend(wedge(0.5));
    stops.push(gsk::ColorStop::new(1.0, none));
    snapshot.append_conic_gradient(area, center, WEDGE_TURN, &stops);
    snapshot.pop();
}

/// A texture's main colours (`cover_palette`), from a smaller copy.
fn texture_palette(texture: &gdk::Texture) -> Vec<[u8; 3]> {
    let mut downloader = gdk::TextureDownloader::new(texture);
    downloader.set_format(gdk::MemoryFormat::R8g8b8a8);
    let (bytes, stride) = downloader.download_bytes();
    let (width, height) = (texture.width() as usize, texture.height() as usize);
    // About 100 × 100 pixels are enough.
    let step = (width.max(height) / 100).max(1);
    let mut pixels = Vec::with_capacity((width / step + 1) * (height / step + 1));
    for y in (0..height).step_by(step) {
        for x in (0..width).step_by(step) {
            let at = y * stride + x * 4;
            if let Some(p) = bytes.get(at..at + 4) {
                pixels.push([p[0], p[1], p[2], p[3]]);
            }
        }
    }
    cover_palette(&pixels)
}

/// A cover's main colours, the most common first, at most four (as
/// Jewelcase Mobile's `coverPalette`): pixels go into buckets, greys by
/// lightness, colours by hue and lightness; a bucket's colour is its
/// pixels' average, and one with less than 4 % of the picture isn't
/// counted.
fn cover_palette(pixels: &[[u8; 4]]) -> Vec<[u8; 3]> {
    const GREYS: usize = 3;
    const HUES: usize = 12;
    let mut buckets: std::collections::HashMap<i32, (usize, [u64; 3])> = Default::default();
    let mut counted = 0;
    for &[r, g, b, a] in pixels {
        if a < 128 {
            continue;
        }
        let (h, s, v) = hsv([r, g, b]);
        let key = if s < 0.18 || v < 0.12 {
            -1 - ((v * GREYS as f32) as i32).min(GREYS as i32 - 1)
        } else {
            let hue = ((h / (360.0 / HUES as f32)) as i32).clamp(0, HUES as i32 - 1);
            hue * 2 + i32::from(v >= 0.6)
        };
        let bucket = buckets.entry(key).or_default();
        bucket.0 += 1;
        for (sum, c) in bucket.1.iter_mut().zip([r, g, b]) {
            *sum += u64::from(c);
        }
        counted += 1;
    }
    let mut found: Vec<_> = buckets
        .into_values()
        .filter(|(count, _)| *count as f32 >= counted as f32 * 0.04 && *count > 0)
        .collect();
    found.sort_by_key(|(count, _)| std::cmp::Reverse(*count));
    found
        .into_iter()
        .take(4)
        .map(|(count, sums)| sums.map(|sum| (sum / count as u64) as u8))
        .collect()
}

/// The four wedge colours. A disc's rainbow is a spread of hues, here
/// around the cover's dominant colour; the cover's main colours, as light
/// on the page (`dark`: lifted to light tones), make most of each, so a
/// grey cover shines silvery with a hint of colour and a colourful one in
/// its own colours.
fn sheen_hues(dominant: Option<[u8; 3]>, palette: &[[u8; 3]], dark: bool) -> [gdk::RGBA; 4] {
    let base = dominant.or_else(|| palette.first().copied());
    let (hue, saturation, _) = base.map_or((210.0, 0.0, 0.0), hsv);
    let saturation = saturation.max(0.55);
    let rainbow = [-50.0, 0.0, 130.0, 70.0]
        .map(|shift: f32| rgb((hue + shift).rem_euclid(360.0), saturation, 1.0));
    let mut hues = [gdk::RGBA::BLACK; 4];
    for (i, colour) in rainbow.into_iter().enumerate() {
        let mixed = match palette.get(i % palette.len().max(1)) {
            Some(cover) => {
                let (h, s, v) = hsv(*cover);
                let v = if dark {
                    v.max(LIFTED_VALUE)
                } else {
                    v.clamp(LIGHT_PAGE_VALUE.0, LIGHT_PAGE_VALUE.1)
                };
                let lifted = rgb(h, s, v);
                [0, 1, 2].map(|c| colour[c] + (lifted[c] - colour[c]) * COVER_SHARE)
            }
            None => colour,
        };
        hues[i] = gdk::RGBA::new(mixed[0], mixed[1], mixed[2], WEDGE_ALPHA[i]);
    }
    hues
}

/// Hue (0–360), saturation and value (0–1) of an RGB colour.
fn hsv([r, g, b]: [u8; 3]) -> (f32, f32, f32) {
    let (r, g, b) = (
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
    );
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let hue = if delta == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let saturation = if max == 0.0 { 0.0 } else { delta / max };
    (hue, saturation, max)
}

/// An RGB colour (0–1 each) from hue, saturation and value.
fn rgb(hue: f32, saturation: f32, value: f32) -> [f32; 3] {
    let c = value * saturation;
    let x = c * (1.0 - ((hue / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = value - c;
    let (r, g, b) = match (hue / 60.0) as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r + m, g + m, b + m]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_covers_main_colours_come_most_common_first() {
        let red = [200, 30, 30, 255];
        let grey = [128, 128, 128, 255];
        let mut pixels = vec![red; 60];
        pixels.extend(vec![grey; 38]);
        // Too few to count, and transparent ones not at all.
        pixels.extend(vec![[20, 200, 40, 255]; 2]);
        pixels.extend(vec![[20, 40, 200, 0]; 50]);
        assert_eq!(cover_palette(&pixels), vec![[200, 30, 30], [128, 128, 128]]);
        assert!(cover_palette(&[]).is_empty());
    }

    /// The owner on the phone, 2026-11-25: a white, black or grey cover
    /// showed only a rainbow; the cover's colours should count for more.
    #[test]
    fn a_grey_cover_shines_mostly_grey() {
        let rainbow = sheen_hues(Some([128, 128, 128]), &[], true);
        let grey = sheen_hues(Some([128, 128, 128]), &[[128, 128, 128]], true);
        let spread = |c: &gdk::RGBA| {
            c.red().max(c.green()).max(c.blue()) - c.red().min(c.green()).min(c.blue())
        };
        for (plain, cover) in rainbow.iter().zip(&grey) {
            assert!(spread(cover) < spread(plain) * 0.5);
            assert_eq!(plain.alpha(), cover.alpha());
        }
        // A colourful cover's light is in its colour: here red.
        let red = sheen_hues(Some([200, 30, 30]), &[[200, 30, 30]], true);
        assert!(red.iter().all(|c| c.red() > c.blue()));
        // On a light page a white cover's light is darker than white, or it
        // wouldn't show.
        let white = sheen_hues(Some([250, 250, 250]), &[[250, 250, 250]], false);
        assert!(
            white
                .iter()
                .all(|c| c.red().min(c.green()).min(c.blue()) < 0.85)
        );
    }

    #[test]
    fn hsv_goes_there_and_back() {
        for colour in [[255, 0, 0], [12, 200, 90], [128, 128, 128], [30, 60, 240]] {
            let (h, s, v) = hsv(colour);
            let back = rgb(h, s, v).map(|c| (c * 255.0).round() as u8);
            assert_eq!(back, colour);
        }
    }
}
