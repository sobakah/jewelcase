//! A drag handle on the inner edge of a sliding panel (the album
//! and playlist panels, the artist page beside or over the artist grid,
//! and the album on an artist page), so the panel can be made wider or
//! narrower. The width is remembered: one for the album and playlist
//! panels, one for artist pages.
//!
//! As the window narrows, the content beside a panel gives up its room
//! first, down to two columns of covers; then the panel gets narrower.
//! An artist page and the album on it get narrower at the same rate,
//! until the page's content or the album can't; then the other alone,
//! and when neither can, the album goes over the page. Worked out in each
//! layout of the panel view (`SlideView::set_width_rule`), never a frame
//! behind the window. A panel is never wider than it was set.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::widgets::slide_view::{Panel, Room};

/// The album and playlist panels' width (shared; also the album on an
/// artist page).
const KEY: &str = "panel-width";
const MIN_WIDTH: i32 = 280;
/// Automatic album and playlist panels: this part of the room, within
/// the two below (as in the templates).
const AUTO_FRACTION: f64 = 0.38;
const AUTO_MIN: i32 = 320;
const AUTO_MAX: i32 = 440;
/// Artist pages.
const ARTIST_KEY: &str = "artist-panel-width";
/// Over the grid (narrow windows).
const ARTIST_MIN_WIDTH: i32 = 360;
/// Below this window width an artist page takes all of it (no handle).
const ARTIST_FULL_BELOW: i32 = 600;
/// Beside the grid, the page is never narrower (its portrait and text,
/// two columns of albums). The grid keeps room for two columns of
/// artists; narrower windows than both have the page over the grid
/// (window.blp).
const ARTIST_BESIDE_MIN: i32 = 560;
/// Room content other than a grid keeps next to the panel (the
/// playlists), and next to a panel over it.
const MIN_CONTENT: i32 = 300;
/// On an artist page, the page keeps this beside an album (its portrait
/// and name, two columns of albums); with less, the album goes over it.
const PAGE_CONTENT_MIN: i32 = 480;
/// One key press (Left, Right).
const KEY_STEP: i32 = 20;

/// Which panel: what it keeps room for, and how it gets narrower.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    /// Beside a grid of covers with this padding, or (None) other content.
    Panel { grid_padding: Option<i32> },
    /// An artist page, beside the artist grid (covers with this padding).
    Artist { grid_padding: i32 },
    /// An album on an artist page.
    AlbumOnPage,
}

/// The playlist panel.
pub fn attach(split: &crate::widgets::SlideView, settings: &gio::Settings) {
    attach_panel(split, settings, Kind::Panel { grid_padding: None });
}

/// The album panel beside the album grid, whose covers have
/// `grid_padding` around them.
pub fn attach_beside_grid(
    split: &crate::widgets::SlideView,
    settings: &gio::Settings,
    grid_padding: i32,
) {
    let grid_padding = Some(grid_padding);
    attach_panel(split, settings, Kind::Panel { grid_padding });
}

/// The artist page beside the artist grid: half the window until its edge
/// is dragged, at least `ARTIST_BESIDE_MIN`. Over the grid (narrow
/// windows): three quarters of the window, all of it below
/// `ARTIST_FULL_BELOW`.
pub fn attach_artist(
    split: &crate::widgets::SlideView,
    settings: &gio::Settings,
    grid_padding: i32,
) {
    attach_panel(split, settings, Kind::Artist { grid_padding });
}

/// The album panel on an artist page.
pub fn attach_album_on_page(split: &crate::widgets::SlideView, settings: &gio::Settings) {
    attach_panel(split, settings, Kind::AlbumOnPage);
}

/// The widths set (0: automatic): the panel's and, for an album on an
/// artist page, the page's.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Set {
    width: i32,
    page: i32,
}

impl Set {
    /// An album on a page narrower than set is narrower by as much.
    fn page_scale(self, kind: Kind, space: i32) -> f64 {
        if kind == Kind::AlbumOnPage && self.page > 0 && space < self.page {
            f64::from(space) / f64::from(self.page)
        } else {
            1.0
        }
    }
}

fn automatic(space: i32) -> i32 {
    ((f64::from(space) * AUTO_FRACTION).round() as i32).clamp(AUTO_MIN, AUTO_MAX)
}

/// Where the panel goes in `room`, the content keeping `keep` beside it.
fn layout(kind: Kind, set: Set, room: Room, keep: i32) -> Panel {
    let space = room.width;
    let keep = keep.max(room.content_min);
    let over = |width| Panel { width, over: true };
    let beside = |width| Panel { width, over: false };
    match kind {
        Kind::Artist { .. } if room.collapsed && space < ARTIST_FULL_BELOW => over(space),
        Kind::Artist { .. } if room.collapsed => over(
            (if set.width > 0 {
                set.width
            } else {
                space * 3 / 4
            })
            .clamp(
                ARTIST_MIN_WIDTH,
                (space - MIN_CONTENT).max(ARTIST_MIN_WIDTH),
            ),
        ),
        Kind::Artist { .. } => beside(
            (if set.width > 0 { set.width } else { space / 2 })
                .min(space - keep)
                .max(ARTIST_BESIDE_MIN),
        ),
        Kind::Panel { .. } if room.collapsed => over(if set.width > 0 {
            set.width
                .clamp(MIN_WIDTH, (space - MIN_CONTENT).max(MIN_WIDTH))
        } else {
            AUTO_MAX.min(space)
        }),
        Kind::Panel { .. } => beside(
            (if set.width > 0 {
                set.width
            } else {
                automatic(space)
            })
            .min(space - keep)
            .max(MIN_WIDTH),
        ),
        Kind::AlbumOnPage => {
            let wanted = if set.width > 0 {
                (f64::from(set.width) * set.page_scale(kind, space)).round() as i32
            } else {
                automatic(space)
            };
            let width = wanted.min(space - keep);
            if room.collapsed || width < MIN_WIDTH {
                // As wide as it was beside the page, not wider.
                over(wanted.max(MIN_WIDTH).min(space))
            } else {
                beside(width)
            }
        }
    }
}

/// The room the content keeps beside the panel: two columns of a grid's
/// covers, or a fixed minimum.
fn keep(kind: Kind) -> i32 {
    let grid = |padding| super::sectioned_grid::min_width(super::tile_size(), padding);
    match kind {
        Kind::Panel {
            grid_padding: Some(padding),
        }
        | Kind::Artist {
            grid_padding: padding,
        } => grid(padding),
        Kind::Panel { grid_padding: None } => MIN_CONTENT,
        Kind::AlbumOnPage => PAGE_CONTENT_MIN,
    }
}

/// The widths set, and the width being dragged.
#[derive(Default)]
struct Width {
    set: Cell<Set>,
    dragged: Cell<Option<i32>>,
}

impl Width {
    fn for_room(&self, kind: Kind, room: Room) -> Panel {
        let keep = keep(kind);
        match self.dragged.get() {
            Some(width) => Panel {
                width,
                over: room.collapsed
                    || (kind == Kind::AlbumOnPage
                        && room.width - width < keep.max(room.content_min)),
            },
            None => layout(kind, self.set.get(), room, keep),
        }
    }

    /// The width to keep for one the panel was given in `space`: for an
    /// album on a narrower page than set, as on the page as set.
    fn to_keep(&self, kind: Kind, width: i32, space: i32) -> i32 {
        (f64::from(width) / self.set.get().page_scale(kind, space)).round() as i32
    }
}

fn content_min(split: &crate::widgets::SlideView) -> i32 {
    split
        .content()
        .map_or(0, |c| c.measure(gtk::Orientation::Horizontal, -1).0)
}

/// The narrowest and widest the panel may be dragged now.
fn limits(split: &crate::widgets::SlideView, kind: Kind) -> (i32, i32) {
    let room = split.width();
    let keep = keep(kind).max(content_min(split));
    let narrowest = match kind {
        Kind::Artist { .. } if split.collapsed() => ARTIST_MIN_WIDTH,
        Kind::Artist { .. } => ARTIST_BESIDE_MIN,
        _ => MIN_WIDTH,
    };
    let keep = if split.collapsed() { MIN_CONTENT } else { keep };
    (narrowest, (room - keep).max(narrowest))
}

fn attach_panel(split: &crate::widgets::SlideView, settings: &gio::Settings, kind: Kind) {
    let Some(sidebar) = split.sidebar() else {
        return;
    };
    let artist = matches!(kind, Kind::Artist { .. });
    let setting = if artist { ARTIST_KEY } else { KEY };
    let min_width = if artist { ARTIST_MIN_WIDTH } else { MIN_WIDTH };
    split.set_sidebar(None::<&gtk::Widget>);
    let handle = gtk::Box::builder()
        .width_request(8)
        .halign(gtk::Align::Start)
        .css_classes(["panel-resize-handle"])
        // Reachable with the keyboard: Left and Right change the width.
        .focusable(true)
        .accessible_role(gtk::AccessibleRole::Separator)
        .cursor(&gdk::Cursor::from_name("col-resize", None).expect("a standard cursor"))
        .build();
    handle.update_property(&[gtk::accessible::Property::Label(&gettextrs::gettext(
        "Resize Panel",
    ))]);
    let overlay = gtk::Overlay::builder().child(&sidebar).build();
    overlay.add_overlay(&handle);
    split.set_sidebar(Some(overlay.upcast_ref::<gtk::Widget>()));
    // Overlaid (collapsed) panels take the whole width; nothing to resize.
    // An artist page over the grid is narrower than the window (below).
    if matches!(kind, Kind::Panel { .. }) {
        split
            .bind_property("collapsed", &handle, "visible")
            .invert_boolean()
            .sync_create()
            .build();
    }

    let width = Rc::new(Width::default());
    let read = glib::clone!(
        #[strong]
        width,
        #[weak]
        split,
        move |settings: &gio::Settings| {
            width.set.set(Set {
                width: settings.int(setting),
                page: if kind == Kind::AlbumOnPage {
                    settings.int(ARTIST_KEY)
                } else {
                    0
                },
            });
            split.queue_resize();
        }
    );
    read(settings);
    let id = settings.connect_changed(None, move |settings, key| {
        if key == setting || key == ARTIST_KEY {
            read(settings);
        }
    });
    crate::widgets::disconnect_with(split, settings, id);
    split.set_width_rule(
        kind == Kind::AlbumOnPage,
        glib::clone!(
            #[strong]
            width,
            move |room| width.for_room(kind, room)
        ),
    );
    // An artist page over a very narrow window takes all of it: no handle.
    if artist {
        let update = glib::clone!(
            #[weak]
            split,
            #[weak]
            handle,
            move || {
                let room = split.width();
                handle.set_visible(!(split.collapsed() && room > 0 && room < ARTIST_FULL_BELOW));
            }
        );
        let on_collapse = update.clone();
        split.connect_collapsed_notify(move |_| {
            let update = on_collapse.clone();
            glib::idle_add_local_once(update);
        });
        split.connect_realize(move |split| {
            let Some(surface) = split.native().and_then(|n| n.surface()) else {
                return;
            };
            let update = update.clone();
            let id = surface.connect_width_notify(move |_| {
                let update = update.clone();
                glib::idle_add_local_once(update);
            });
            crate::widgets::disconnect_with(split, &surface, id);
        });
    }

    let drag = gtk::GestureDrag::new();
    // Where in the handle it was grabbed, and the width the drag asks for.
    let grab = std::rc::Rc::new(std::cell::Cell::new(0.0));
    let wanted = std::rc::Rc::new(std::cell::Cell::new(None::<i32>));
    drag.connect_drag_begin(glib::clone!(
        #[strong]
        grab,
        #[strong]
        wanted,
        #[weak]
        split,
        move |_, x, _| {
            grab.set(x);
            wanted.set(None);
            // The content holds still until the drag ends (sectioned_grid.rs).
            split.set_resizing(true);
        }
    ));
    // The pointer is measured in the panel view, which stays put. The
    // handle moves with the panel's edge: offsets relative to it changed
    // with every resize, and a held edge flipped between two widths on
    // alternate frames (the panel looked doubled).
    let width_at = glib::clone!(
        #[weak]
        split,
        #[weak]
        handle,
        #[strong]
        grab,
        #[upgrade_or]
        None,
        move |gesture: &gtk::GestureDrag| {
            let sequence = gesture.current_sequence();
            let (x, y) = gesture.point(sequence.as_ref())?;
            let point =
                handle.compute_point(&split, &gtk::graphene::Point::new(x as f32, y as f32))?;
            // The handle is on the panel's left edge: left widens it.
            let edge = f64::from(point.x()) - grab.get();
            let width = f64::from(split.width()) - edge;
            let (narrowest, widest) = limits(&split, kind);
            Some((width as i32).clamp(narrowest, widest))
        }
    );
    drag.connect_drag_update(glib::clone!(
        #[strong]
        wanted,
        #[strong]
        width,
        #[weak]
        split,
        move |gesture, _, _| {
            if let Some(dragged) = width_at(gesture)
                && wanted.get() != Some(dragged)
            {
                wanted.set(Some(dragged));
                width.dragged.set(Some(dragged));
                split.queue_resize();
            }
        }
    ));
    drag.connect_drag_end(glib::clone!(
        #[strong]
        settings,
        #[strong]
        width,
        #[weak]
        split,
        move |_, _, _| {
            if let Some(dragged) = wanted.take() {
                let _ = settings.set_int(setting, width.to_keep(kind, dragged, split.width()));
            }
            width.dragged.set(None);
            split.queue_resize();
            split.set_resizing(false);
        }
    ));
    handle.add_controller(drag);

    // Left widens (the handle is on the panel's left edge), Right narrows;
    // with Ctrl in bigger steps.
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed(glib::clone!(
        #[weak]
        split,
        #[strong]
        width,
        #[strong]
        settings,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |_, key, _, modifiers| {
            let step = if modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
                KEY_STEP * 4
            } else {
                KEY_STEP
            };
            let change = match key {
                gdk::Key::Left | gdk::Key::KP_Left => step,
                gdk::Key::Right | gdk::Key::KP_Right => -step,
                _ => return glib::Propagation::Proceed,
            };
            let now = split.sidebar_width();
            let (narrowest, widest) = limits(&split, kind);
            let now = (now + change).clamp(narrowest, widest);
            let _ = settings.set_int(setting, width.to_keep(kind, now, split.width()));
            glib::Propagation::Stop
        }
    ));
    handle.add_controller(keys);
    // Its value for assistive technology.
    let describe = glib::clone!(
        #[weak]
        handle,
        move |settings: &gio::Settings| {
            let width = settings.int(setting);
            handle.update_property(&[
                gtk::accessible::Property::ValueMin(f64::from(min_width)),
                gtk::accessible::Property::ValueNow(f64::from(width.max(0))),
            ]);
        }
    );
    describe(settings);
    let id = settings.connect_changed(Some(setting), move |settings, _| describe(settings));
    crate::widgets::disconnect_with(split, settings, id);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room(width: i32) -> Room {
        Room {
            width,
            content_min: 0,
            collapsed: false,
        }
    }

    fn set(width: i32, page: i32) -> Set {
        Set { width, page }
    }

    const GRID: Kind = Kind::Artist { grid_padding: 24 };

    /// The window's breakpoint that puts an artist page over the grid is
    /// where the page and the grid no longer fit side by side; in sp it
    /// moved with the text size (the grid got 240 px at 0.8).
    #[test]
    fn the_artist_breakpoint_is_where_both_fit() {
        // Two columns of artists at the default cover size.
        const ARTIST_GRID_MIN: i32 = 400;
        let window = include_str!("../../../data/resources/ui/window.blp");
        let wanted = format!("max-width: {}px", ARTIST_BESIDE_MIN + ARTIST_GRID_MIN);
        assert!(window.contains(&wanted), "window.blp has no \"{wanted}\"");
        // Two columns of artists at the default cover size (160) fit.
        assert!(super::super::sectioned_grid::min_width(160, 24) <= ARTIST_GRID_MIN);
    }

    /// As the window narrows, the artist grid gives up its room first,
    /// down to two columns (392 px at 160 px covers), then the page gets
    /// narrower; never wider than set. The page covered part of the grid
    /// below two columns, and the grid's minimum wasn't kept at all.
    #[test]
    fn the_grid_gives_up_its_room_before_the_page() {
        let keep = 392;
        // 1556 set: the whole of it on a full screen.
        assert_eq!(layout(GRID, set(1556, 0), room(2560), keep).width, 1556);
        // On half the screen, the grid keeps two columns.
        assert_eq!(
            layout(GRID, set(1556, 0), room(1280), keep).width,
            1280 - keep
        );
        // Never below its own minimum beside the grid.
        assert_eq!(
            layout(GRID, set(1556, 0), room(900), keep).width,
            ARTIST_BESIDE_MIN
        );
        // Never wider than set.
        assert_eq!(layout(GRID, set(700, 0), room(2560), keep).width, 700);
    }

    /// An album on an artist page narrower than set is narrower at the
    /// same rate as the page; once the page's content is at its minimum,
    /// the album alone, and when the album can't, it goes over the page.
    /// It went over the page (or vanished) while it still fitted, and
    /// stayed as wide as set while the page got narrower.
    #[test]
    fn an_album_shrinks_with_its_page_then_goes_over_it() {
        let album = Kind::AlbumOnPage;
        let keep = PAGE_CONTENT_MIN;
        // Page set to 1556, album to 647: on the whole page, as set.
        let full = layout(album, set(647, 1556), room(1556), keep);
        assert_eq!(
            full,
            Panel {
                width: 647,
                over: false
            }
        );
        // Page at 1100: both at 1100/1556 of their width (457 + 643).
        let narrower = layout(album, set(647, 1556), room(1100), keep);
        assert_eq!(
            narrower,
            Panel {
                width: 457,
                over: false
            }
        );
        // Page at 820: 341 by the rate, and the page keeps its 480.
        let at_min = layout(album, set(647, 1556), room(820), keep);
        assert_eq!(
            at_min,
            Panel {
                width: 340,
                over: false
            }
        );
        // Page at 760: the album alone gets narrower, to its minimum.
        let alone = layout(album, set(647, 1556), room(760), keep);
        assert_eq!(
            alone,
            Panel {
                width: MIN_WIDTH,
                over: false
            }
        );
        // Narrower: over the page, as wide as by the rate.
        let over = layout(album, set(647, 1556), room(700), keep);
        assert_eq!(
            over,
            Panel {
                width: 291,
                over: true
            }
        );
        // A page wider than its content's own minimum keeps that instead.
        let wide_content = Room {
            content_min: 600,
            ..room(900)
        };
        assert_eq!(layout(album, set(647, 1556), wide_content, keep).width, 300);
    }

    /// The album and playlist panels: the grid keeps two columns, then
    /// the panel gets narrower down to its minimum; automatic panels are
    /// a part of the room.
    #[test]
    fn a_panel_beside_a_grid_leaves_it_two_columns() {
        let panel = Kind::Panel {
            grid_padding: Some(18),
        };
        assert_eq!(layout(panel, set(700, 0), room(2400), 380).width, 700);
        assert_eq!(layout(panel, set(700, 0), room(1000), 380).width, 620);
        assert_eq!(layout(panel, set(700, 0), room(600), 380).width, MIN_WIDTH);
        assert_eq!(layout(panel, set(0, 0), room(1000), 380).width, 380);
    }
}
