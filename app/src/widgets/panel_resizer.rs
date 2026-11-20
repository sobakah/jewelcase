//! A drag handle on the inner edge of a sliding panel (the album
//! and playlist panels, and the artist page beside or over the artist
//! grid), so the panel can be made wider or narrower. The width is
//! remembered: one for the album and playlist panels, one for artist pages.

use adw::prelude::*;
use gtk::{gdk, gio, glib};

/// The album and playlist panels' width (shared).
const KEY: &str = "panel-width";
const MIN_WIDTH: i32 = 280;
/// Artist pages.
const ARTIST_KEY: &str = "artist-panel-width";
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
    // An artist page over the grid is narrower than the window (apply).
    if !artist {
        split
            .bind_property("collapsed", &handle, "visible")
            .invert_boolean()
            .sync_create()
            .build();
    }

    let apply = glib::clone!(
        #[weak]
        split,
        #[weak]
        handle,
        move |width: i32| {
            if artist {
                let room = split.width();
                if room <= 0 {
                    return;
                }
                let (narrowest, widest) = limits(&split, min_width, artist);
                let full = split.collapsed() && room < ARTIST_FULL_BELOW;
                handle.set_visible(!full);
                let panel = if full {
                    room
                } else if width > 0 {
                    width.clamp(narrowest, widest)
                } else if split.collapsed() {
                    room * 3 / 4
                } else {
                    (room / 2).clamp(narrowest, widest)
                };
                split.set_min_sidebar_width(f64::from(panel.min(min_width)));
                split.set_max_sidebar_width(f64::from(panel));
                split.set_min_sidebar_width(f64::from(panel));
            } else if width > 0 {
                // Never more than there's room for (an artist page's own
                // panel is narrower than the window's; windows shrink).
                let room = split.width();
                let width = if room > 0 {
                    width.min((room - MIN_CONTENT).max(min_width))
                } else {
                    width
                };
                split.set_min_sidebar_width(f64::from(width));
                split.set_max_sidebar_width(f64::from(width));
            } else {
                // Automatic, as in the templates.
                split.set_min_sidebar_width(320.0);
                split.set_max_sidebar_width(440.0);
            }
        }
    );
    apply(settings.int(setting));
    let on_change = apply.clone();
    let id = settings.connect_changed(Some(setting), move |settings, _| {
        on_change(settings.int(setting))
    });
    crate::widgets::disconnect_with(split, settings, id);
    // Beside the grid or over it: other limits.
    let on_collapse = apply.clone();
    let collapse_settings = settings.clone();
    split.connect_collapsed_notify(move |_| {
        let apply = on_collapse.clone();
        let settings = collapse_settings.clone();
        glib::idle_add_local_once(move || apply(settings.int(setting)));
    });
    // Again once the room is known, and when the window's width changes.
    let on_map = apply.clone();
    let on_map_settings = settings.clone();
    split.connect_map(move |_| {
        let apply = on_map.clone();
        let settings = on_map_settings.clone();
        glib::idle_add_local_once(move || apply(settings.int(setting)));
    });
    let on_resize = apply.clone();
    let resize_settings = settings.clone();
    split.connect_realize(move |split| {
        let Some(surface) = split.native().and_then(|n| n.surface()) else {
            return;
        };
        let apply = on_resize.clone();
        let settings = resize_settings.clone();
        let id = surface.connect_width_notify(move |_| {
            let apply = apply.clone();
            let settings = settings.clone();
            glib::idle_add_local_once(move || apply(settings.int(setting)));
        });
        crate::widgets::disconnect_with(split, &surface, id);
    });

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
        move |gesture, _, _| {
            if let Some(width) = width_at(gesture)
                && wanted.get() != Some(width)
            {
                wanted.set(Some(width));
                apply(width);
            }
        }
    ));
    drag.connect_drag_end(glib::clone!(
        #[strong]
        settings,
        #[weak]
        split,
        move |_, _, _| {
            if let Some(width) = wanted.take() {
                let _ = settings.set_int(setting, width);
            }
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
            let _ = settings.set_int(setting, width);
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
