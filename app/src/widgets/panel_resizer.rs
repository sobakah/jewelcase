//! A drag handle on the inner edge of a sliding panel (the album
//! and playlist panels, and the artist page beside or over the artist
//! grid), so the panel can be made wider or narrower. The width is
//! remembered: one for the album and playlist panels, one for artist pages.
//!
//! A width is kept on windows at least as wide as the one it was set on,
//! and scaled down with narrower ones, so the layout stays alike (never
//! below the panel's minimum). Worked out in each layout of the panel
//! view, from the window's width then (`SlideView::set_width_rule`).

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::widgets::slide_view::Room;

/// The album and playlist panels' width (shared), and the window's width
/// when it was set (0: set before 0.9.1, taken as on a full screen).
const KEY: &str = "panel-width";
const KEY_WINDOW: &str = "panel-width-window";
const MIN_WIDTH: i32 = 280;
/// Automatic album and playlist panels: this part of the room, within
/// the two below (as in the templates).
const AUTO_FRACTION: f64 = 0.38;
const AUTO_MIN: i32 = 320;
const AUTO_MAX: i32 = 440;
/// Artist pages.
const ARTIST_KEY: &str = "artist-panel-width";
const ARTIST_KEY_WINDOW: &str = "artist-panel-width-window";
/// Over the grid (narrow windows).
const ARTIST_MIN_WIDTH: i32 = 360;
/// Below this window width an artist page takes all of it (no handle).
const ARTIST_FULL_BELOW: i32 = 600;
/// Beside the grid, the page is never narrower (its portrait and text,
/// two columns of albums), and the grid keeps room for two columns of
/// artists. Narrower windows have the page over the grid (window.blp).
const ARTIST_BESIDE_MIN: i32 = 560;
const ARTIST_GRID_MIN: i32 = 400;
/// Room the content keeps next to the panel.
const MIN_CONTENT: i32 = 300;
/// One key press (Left, Right).
const KEY_STEP: i32 = 20;

/// On an artist page, an album's panel goes over the page when the page
/// would keep less than this beside it (its portrait and name, two
/// columns of albums).
pub const ARTIST_PAGE_CONTENT_MIN: i32 = 480;

/// Wraps the sidebar of `split` (whose sidebar is at the end) with a handle.
pub fn attach(split: &crate::widgets::SlideView, settings: &gio::Settings) {
    attach_panel(split, settings, KEY, MIN_WIDTH, false);
}

/// The artist page beside the artist grid: half the window until its edge
/// is dragged, at least `ARTIST_BESIDE_MIN`. Over the grid (narrow
/// windows): three quarters of the window, all of it below
/// `ARTIST_FULL_BELOW`.
pub fn attach_artist(split: &crate::widgets::SlideView, settings: &gio::Settings) {
    attach_panel(split, settings, ARTIST_KEY, ARTIST_MIN_WIDTH, true);
}

/// A width set on a window `set_on` wide, on a window `window` wide:
/// scaled down on a narrower one, as set on a wider one.
fn scaled(width: i32, set_on: i32, window: i32) -> i32 {
    if set_on > 0 && window > 0 && window < set_on {
        (i64::from(width) * i64::from(window) / i64::from(set_on)) as i32
    } else {
        width
    }
}

/// The panel's width in `room`, from the width set (0: automatic) on a
/// window `set_on` wide.
fn panel_width(artist: bool, width: i32, set_on: i32, room: Room) -> i32 {
    let set = (width > 0).then(|| scaled(width, set_on, room.window));
    let space = room.width;
    if artist {
        if room.over && space < ARTIST_FULL_BELOW {
            space
        } else if room.over {
            set.unwrap_or(space * 3 / 4).clamp(
                ARTIST_MIN_WIDTH,
                (space - MIN_CONTENT).max(ARTIST_MIN_WIDTH),
            )
        } else {
            set.unwrap_or(space / 2).clamp(
                ARTIST_BESIDE_MIN,
                (space - ARTIST_GRID_MIN).max(ARTIST_BESIDE_MIN),
            )
        }
    } else {
        let widest = (space - MIN_CONTENT).max(MIN_WIDTH);
        match set {
            Some(set) => set.clamp(MIN_WIDTH, widest),
            None if room.over => AUTO_MAX.min(space),
            None => ((f64::from(space) * AUTO_FRACTION).round() as i32).clamp(AUTO_MIN, AUTO_MAX),
        }
    }
}

/// The width set, the window's width then, and the width being dragged.
#[derive(Default)]
struct Width {
    set: Cell<i32>,
    set_on: Cell<i32>,
    /// For widths set before the window's was kept: the screen's.
    screen: Cell<i32>,
    dragged: Cell<Option<i32>>,
}

impl Width {
    fn for_room(&self, artist: bool, room: Room) -> i32 {
        if let Some(width) = self.dragged.get() {
            return width;
        }
        let set_on = match self.set_on.get() {
            0 => self.screen.get(),
            set_on => set_on,
        };
        panel_width(artist, self.set.get(), set_on, room)
    }
}

/// The width of the screen the window is on.
fn screen_width(widget: &impl IsA<gtk::Widget>) -> Option<i32> {
    let surface = widget.native()?.surface()?;
    let monitor = widget.display().monitor_at_surface(&surface)?;
    Some(monitor.geometry().width())
}

/// Keeps a width the panel was given: with the window's width now.
fn save(settings: &gio::Settings, keys: (&str, &str), width: i32, window: i32) {
    let _ = settings.set_int(keys.1, window);
    let _ = settings.set_int(keys.0, width);
}

fn window_width(widget: &impl IsA<gtk::Widget>) -> i32 {
    widget.root().map_or(0, |root| root.width())
}

/// The narrowest and widest the panel may be now.
fn limits(split: &crate::widgets::SlideView, min_width: i32, artist: bool) -> (i32, i32) {
    let room = split.width();
    if artist && !split.collapsed() {
        let max = (room - ARTIST_GRID_MIN).max(ARTIST_BESIDE_MIN);
        (ARTIST_BESIDE_MIN, max)
    } else {
        (min_width, (room - MIN_CONTENT).max(min_width))
    }
}

fn attach_panel(
    split: &crate::widgets::SlideView,
    settings: &gio::Settings,
    setting: &'static str,
    min_width: i32,
    artist: bool,
) {
    let Some(sidebar) = split.sidebar() else {
        return;
    };
    let window_key = if artist {
        ARTIST_KEY_WINDOW
    } else {
        KEY_WINDOW
    };
    let saved_as = (setting, window_key);
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
    if !artist {
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
            width.set.set(settings.int(setting));
            width.set_on.set(settings.int(window_key));
            split.queue_resize();
        }
    );
    read(settings);
    let id = settings.connect_changed(None, move |settings, key| {
        if key == setting || key == window_key {
            read(settings);
        }
    });
    crate::widgets::disconnect_with(split, settings, id);
    split.set_width_rule(glib::clone!(
        #[strong]
        width,
        move |room| width.for_room(artist, room)
    ));
    // The screen's width, for a width set before the window's was kept.
    let on_map = width.clone();
    split.connect_map(move |split| {
        if let Some(screen) = screen_width(split) {
            on_map.screen.set(screen);
            split.queue_resize();
        }
    });
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
            let (narrowest, widest) = limits(&split, min_width, artist);
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
        #[weak]
        split,
        move |_, _, _| {
            if let Some(dragged) = wanted.take() {
                save(&settings, saved_as, dragged, window_width(&split));
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
            let (narrowest, widest) = limits(&split, min_width, artist);
            let width = (now + change).clamp(narrowest, widest);
            save(&settings, saved_as, width, window_width(&split));
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

    fn room(width: i32, window: i32, over: bool) -> Room {
        Room {
            width,
            window,
            over,
        }
    }

    /// A width set on a full screen is narrower in proportion on a
    /// narrower window (the layout stays alike), never wider on a wider
    /// one, and never below the panel's minimum.
    #[test]
    fn a_panel_is_scaled_down_with_the_window_only() {
        // Album panel, 700 set on a 2400 window.
        assert_eq!(panel_width(false, 700, 2400, room(2400, 2400, false)), 700);
        assert_eq!(panel_width(false, 700, 2400, room(1200, 1200, false)), 350);
        assert_eq!(panel_width(false, 700, 2400, room(3000, 3000, false)), 700);
        assert_eq!(
            panel_width(false, 700, 2400, room(600, 600, false)),
            MIN_WIDTH
        );
        // Set on a half-wide window: as set on a full one.
        assert_eq!(panel_width(false, 500, 1200, room(2400, 2400, false)), 500);
        // On an artist page, by the window's width, not the page's.
        assert_eq!(panel_width(false, 700, 2400, room(1000, 1600, false)), 466);
        // Automatic: a part of the room, as before.
        assert_eq!(panel_width(false, 0, 0, room(1000, 1000, false)), 380);

        // Artist page, 1200 set on a 2400 window: half of a narrower one,
        // never under its minimum beside the grid.
        assert_eq!(panel_width(true, 1200, 2400, room(2400, 2400, false)), 1200);
        assert_eq!(panel_width(true, 1200, 2400, room(1600, 1600, false)), 800);
        assert_eq!(
            panel_width(true, 1200, 2400, room(1000, 1000, false)),
            ARTIST_BESIDE_MIN
        );
        // Over the grid of a very narrow window: all of it.
        assert_eq!(panel_width(true, 1200, 2400, room(500, 500, true)), 500);
    }

    /// The window's breakpoint that puts an artist page over the grid is
    /// where the page and the grid no longer fit side by side; in sp it
    /// moved with the text size (the grid got 240 px at 0.8).
    #[test]
    fn the_artist_breakpoint_is_where_both_fit() {
        let window = include_str!("../../../data/resources/ui/window.blp");
        let wanted = format!("max-width: {}px", ARTIST_BESIDE_MIN + ARTIST_GRID_MIN);
        assert!(window.contains(&wanted), "window.blp has no \"{wanted}\"");
    }
}
