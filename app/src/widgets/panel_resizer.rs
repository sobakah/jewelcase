//! A drag handle on the inner edge of a sliding panel (the album
//! and playlist panels, the artist page beside or over the artist grid,
//! and the album on an artist page), so the panel can be made wider or
//! narrower. The width is remembered: one for the album and playlist
//! panels, one for artist pages.
//!
//! As the window narrows, the content beside a panel gives up its room
//! first, down to two columns of covers; then the panel gets narrower.
//! An artist page and the album on it get narrower at the same rate,
//! until the page's content or the album can't; then the other alone.
//! When neither can, the page (with the album beside its content) goes
//! over the artist grid. What was opened last stays in sight longest
//! (closing it leads back): when an album doesn't fit beside the page's
//! content any more, the page takes the whole window and the album lies
//! over it, as wide; an album or playlist panel without room beside its
//! grid lies over the grid, the whole window wide. Worked out in each
//! layout of the panel view (`SlideView::set_width_rule`), never a frame
//! behind the window. A panel is never wider than it was set (an artist
//! page grows to keep an album beside its content), except over what it
//! covers.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::widgets::slide_view::{Panel, Place, Room};

/// The album and playlist panels' width (shared; also the album on an
/// artist page).
const KEY: &str = "panel-width";
/// Beside other content: narrower, an album's song titles were cut to a
/// few letters (then it goes over what it covers, all of the window).
const MIN_WIDTH: i32 = 360;
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
/// two columns of albums); with an album open, the page's content and
/// the album at their narrowest. The grid keeps room for two columns of
/// artists, or the page goes over it.
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

/// Whether an album is open on the artist page.
type AlbumOpen = Rc<dyn Fn() -> bool>;

/// The album or playlist panel beside the album or playlist grid, whose
/// covers have `grid_padding` around them.
pub fn attach_beside_grid(
    split: &crate::widgets::SlideView,
    settings: &gio::Settings,
    grid_padding: i32,
) {
    let grid_padding = Some(grid_padding);
    attach_panel(split, settings, Kind::Panel { grid_padding }, None);
}

/// The artist page beside the artist grid: half the window until its edge
/// is dragged, at least `ARTIST_BESIDE_MIN`. Over the grid (narrow
/// windows): at its narrowest, all of the window below
/// `ARTIST_FULL_BELOW`. `album_open` tells whether an album is open on
/// the page (it needs more room).
pub fn attach_artist(
    split: &crate::widgets::SlideView,
    settings: &gio::Settings,
    grid_padding: i32,
    album_open: impl Fn() -> bool + 'static,
) {
    let album_open: AlbumOpen = Rc::new(album_open);
    attach_panel(
        split,
        settings,
        Kind::Artist { grid_padding },
        Some(album_open),
    );
}

/// The album panel on an artist page.
pub fn attach_album_on_page(split: &crate::widgets::SlideView, settings: &gio::Settings) {
    attach_panel(split, settings, Kind::AlbumOnPage, None);
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

/// The narrowest an artist page is beside the grid: with an album open,
/// room for the page's content and the album.
fn page_min(album_open: bool) -> i32 {
    if album_open {
        ARTIST_BESIDE_MIN.max(PAGE_CONTENT_MIN + MIN_WIDTH)
    } else {
        ARTIST_BESIDE_MIN
    }
}

/// Where the panel goes in `room`, the content keeping `keep` beside it;
/// `album_open`: an album is open on the artist page.
fn layout(kind: Kind, set: Set, room: Room, keep: i32, album_open: bool) -> Panel {
    let space = room.width;
    let keep = keep.max(room.content_min);
    let over = |width| Panel {
        width,
        place: Place::Over,
    };
    let beside = |width| Panel {
        width,
        place: Place::Beside,
    };
    match kind {
        Kind::Artist { .. } => {
            let wanted = if set.width > 0 { set.width } else { space / 2 };
            let need = page_min(album_open);
            if !room.collapsed && space - keep >= need {
                // Never wider than set, unless the album needs it.
                beside(wanted.max(need).min(space - keep))
            } else if space < ARTIST_FULL_BELOW || space < need {
                // All of the window; with an album that doesn't fit
                // beside the content, it lies over the page, as wide.
                over(space)
            } else {
                // Over the grid, at its narrowest (it doesn't jump wider:
                // the grid goes under it), with the album beside its
                // content.
                over(need)
            }
        }
        Kind::Panel { .. } | Kind::AlbumOnPage => {
            let wanted = if set.width > 0 {
                (f64::from(set.width) * set.page_scale(kind, space)).round() as i32
            } else {
                automatic(space)
            };
            // At its minimum, the page's content gives up its room alone.
            let width = wanted.max(MIN_WIDTH).min(space - keep);
            if room.collapsed || width < MIN_WIDTH {
                // No room beside the grid or the page's content: over it,
                // all of the view (narrower, titles were cut short).
                over(space)
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
    fn for_room(&self, kind: Kind, room: Room, album_open: bool) -> Panel {
        self.for_room_keeping(kind, room, keep(kind), album_open)
    }

    /// As `for_room`, the content keeping `keep` beside the panel.
    fn for_room_keeping(&self, kind: Kind, room: Room, keep: i32, album_open: bool) -> Panel {
        let panel = layout(kind, self.set.get(), room, keep, album_open);
        match self.dragged.get() {
            // As wide as dragged while beside the content, within what the
            // rule allows there (the window may change during the drag);
            // over it, as the rule says (all of the window).
            Some(width) if panel.place == Place::Beside => {
                let narrowest = narrowest_beside(kind, album_open);
                let widest = (room.width - keep.max(room.content_min)).max(narrowest);
                Panel {
                    width: width.clamp(narrowest, widest),
                    ..panel
                }
            }
            _ => panel,
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

/// The narrowest a panel is beside the content: an artist page with an
/// album open has room for both.
fn narrowest_beside(kind: Kind, album_open: bool) -> i32 {
    match kind {
        Kind::Artist { .. } => page_min(album_open),
        _ => MIN_WIDTH,
    }
}

/// The narrowest and widest the panel may be dragged now.
fn limits(split: &crate::widgets::SlideView, kind: Kind, album_open: bool) -> (i32, i32) {
    let room = split.width();
    let keep = keep(kind).max(content_min(split));
    let narrowest = match kind {
        Kind::Artist { .. } if split.over() => ARTIST_MIN_WIDTH,
        _ => narrowest_beside(kind, album_open),
    };
    let keep = if split.over() { MIN_CONTENT } else { keep };
    (narrowest, (room - keep).max(narrowest))
}

fn attach_panel(
    split: &crate::widgets::SlideView,
    settings: &gio::Settings,
    kind: Kind,
    album_open: Option<AlbumOpen>,
) {
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

    let open_now: Rc<dyn Fn() -> bool> = {
        let album_open = album_open.clone();
        Rc::new(move || album_open.as_ref().is_some_and(|open| open()))
    };
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
        // Each goes over what it covers when there's no room beside it.
        true,
        glib::clone!(
            #[strong]
            width,
            move |room| width.for_room(kind, room, album_open.as_ref().is_some_and(|open| open())),
        ),
    );
    // Over the content a panel is the whole view, or (an artist page over
    // the grid) at its narrowest: no handle.
    split
        .bind_property("over", &handle, "visible")
        .invert_boolean()
        .sync_create()
        .build();

    let open_now_keys = open_now.clone();
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
            let (narrowest, widest) = limits(&split, kind, open_now());
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
            let (narrowest, widest) = limits(&split, kind, open_now_keys());
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

    fn beside(width: i32) -> Panel {
        Panel {
            width,
            place: Place::Beside,
        }
    }

    fn over(width: i32) -> Panel {
        Panel {
            width,
            place: Place::Over,
        }
    }

    const PAGE: Kind = Kind::Artist { grid_padding: 24 };
    const ALBUM: Kind = Kind::AlbumOnPage;
    /// Two columns of artists at 160 px covers.
    const GRID_MIN: i32 = 392;

    #[test]
    fn two_columns_of_artists_are_the_grids_minimum() {
        assert_eq!(super::super::sectioned_grid::min_width(160, 24), GRID_MIN);
    }

    /// As the window narrows, the artist grid gives up its room first,
    /// down to two columns, then the page gets narrower, down to its
    /// minimum; then it goes over the grid. Never wider than set.
    #[test]
    fn the_grid_gives_up_its_room_before_the_page() {
        let page =
            |set_width, window| layout(PAGE, set(set_width, 0), room(window), GRID_MIN, false);
        assert_eq!(page(1556, 2560), beside(1556));
        assert_eq!(page(1556, 1280), beside(1280 - GRID_MIN));
        assert_eq!(page(1556, 952), beside(ARTIST_BESIDE_MIN));
        // Narrower: over the grid, as narrow as it was.
        assert_eq!(page(1556, 940), over(ARTIST_BESIDE_MIN));
        assert_eq!(page(700, 2560), beside(700));
    }

    /// With an album open, the page needs room for its content and the
    /// album, so it goes over the grid sooner (as wide as both need);
    /// a page set narrower grows to keep the album beside its content.
    #[test]
    fn a_page_with_an_album_goes_over_the_grid_sooner() {
        let page =
            |set_width, window| layout(PAGE, set(set_width, 0), room(window), GRID_MIN, true);
        let need = PAGE_CONTENT_MIN + MIN_WIDTH;
        assert_eq!(page(1556, 1280), beside(1280 - GRID_MIN));
        assert_eq!(page(1556, GRID_MIN + need), beside(need));
        assert_eq!(page(1556, GRID_MIN + need - 1), over(need));
        assert_eq!(page(1556, 900), over(need));
        // Too narrow for both: all of the window (the album over it).
        assert_eq!(page(1556, 800), over(800));
        // Set narrower than both need.
        assert_eq!(page(600, 2560), beside(need));
    }

    /// An album on an artist page narrower than set is narrower at the
    /// same rate as the page; once the page's content is at its minimum,
    /// the album alone; when the album can't, it lies over the page, as
    /// wide: what was opened last stays in sight (it went out of sight,
    /// and no album could be opened on a narrow window).
    #[test]
    fn an_album_shrinks_with_its_page_then_lies_over_it() {
        let album = |page| layout(ALBUM, set(647, 1556), room(page), PAGE_CONTENT_MIN, false);
        assert_eq!(album(1556), beside(647));
        // 1100/1556 of both widths (457 + 643).
        assert_eq!(album(1100), beside(457));
        assert_eq!(album(887), beside(369));
        // 358 by the rate: the album keeps its minimum, the page's
        // content alone gets narrower, down to its 480.
        assert_eq!(album(860), beside(MIN_WIDTH));
        assert_eq!(album(840), beside(MIN_WIDTH));
        assert_eq!(album(839), over(839));
        assert_eq!(album(400), over(400));
        // A page content wider than its minimum keeps that instead.
        let wide_content = Room {
            content_min: 600,
            ..room(900)
        };
        let panel = layout(ALBUM, set(647, 1556), wide_content, PAGE_CONTENT_MIN, false);
        assert_eq!(panel, over(900));
        let wide_content = Room {
            content_min: 600,
            ..room(1000)
        };
        let panel = layout(ALBUM, set(647, 1556), wide_content, PAGE_CONTENT_MIN, false);
        assert_eq!(panel, beside(400));
    }

    /// A dragged edge sets the width only beside the content, within what
    /// the rule allows there: an artist page kept room for its album, and
    /// a panel the window put over the grid stayed as wide as dragged.
    #[test]
    fn a_dragged_width_stays_within_the_rule() {
        let width = Width::default();
        width.set.set(set(1556, 0));
        width.dragged.set(Some(560));
        // An album open: not narrower than the page's content and it.
        let page = width.for_room_keeping(PAGE, room(2560), GRID_MIN, true);
        assert_eq!(page, beside(PAGE_CONTENT_MIN + MIN_WIDTH));
        // Over the grid: as the rule says.
        let panel = Kind::Panel {
            grid_padding: Some(18),
        };
        width.set.set(set(700, 0));
        width.dragged.set(Some(500));
        assert_eq!(
            width.for_room_keeping(panel, room(600), 380, false),
            over(600)
        );
        assert_eq!(
            width.for_room_keeping(panel, room(1000), 380, false),
            beside(500)
        );
    }

    /// The album and playlist panels: the grid keeps two columns, then
    /// the panel gets narrower down to its minimum; automatic panels are
    /// a part of the room. Without room for both, the panel lies over the
    /// grid, the whole window wide: it was hidden, and opened again it
    /// was too narrow for its titles.
    #[test]
    fn a_panel_beside_a_grid_leaves_it_two_columns_then_covers_it() {
        let panel = |set_width, window| {
            let kind = Kind::Panel {
                grid_padding: Some(18),
            };
            layout(kind, set(set_width, 0), room(window), 380, false)
        };
        assert_eq!(panel(700, 2400), beside(700));
        assert_eq!(panel(700, 1000), beside(620));
        assert_eq!(panel(700, 740), beside(MIN_WIDTH));
        assert_eq!(panel(700, 739), over(739));
        assert_eq!(panel(0, 1000), beside(380));
        assert_eq!(panel(0, 500), over(500));
    }
}
