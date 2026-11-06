//! A drag handle on the inner edge of a split view's sidebar (the album
//! and playlist panels), so the panel can be made wider or narrower. The
//! width is shared by all panels and remembered.

use adw::prelude::*;
use gtk::{gdk, gio, glib};

const KEY: &str = "panel-width";
const MIN_WIDTH: i32 = 280;
/// Room the content keeps next to the panel.
const MIN_CONTENT: i32 = 300;

/// Wraps the sidebar of `split` (whose sidebar is at the end) with a handle.
pub fn attach(split: &adw::OverlaySplitView, settings: &gio::Settings) {
    let Some(sidebar) = split.sidebar() else {
        return;
    };
    split.set_sidebar(None::<&gtk::Widget>);
    let handle = gtk::Box::builder()
        .width_request(8)
        .halign(gtk::Align::Start)
        .css_classes(["panel-resize-handle"])
        .cursor(&gdk::Cursor::from_name("col-resize", None).expect("a standard cursor"))
        .build();
    handle.update_property(&[gtk::accessible::Property::Label(&gettextrs::gettext(
        "Resize Panel",
    ))]);
    let overlay = gtk::Overlay::builder().child(&sidebar).build();
    overlay.add_overlay(&handle);
    split.set_sidebar(Some(&overlay));
    // Overlaid (collapsed) panels take the whole width; nothing to resize.
    split
        .bind_property("collapsed", &handle, "visible")
        .invert_boolean()
        .sync_create()
        .build();

    let apply = glib::clone!(
        #[weak]
        split,
        move |width: i32| {
            if width > 0 {
                split.set_min_sidebar_width(f64::from(width));
                split.set_max_sidebar_width(f64::from(width));
            } else {
                // Automatic, as in the templates.
                split.set_min_sidebar_width(320.0);
                split.set_max_sidebar_width(440.0);
            }
        }
    );
    apply(settings.int(KEY));
    let on_change = apply.clone();
    settings.connect_changed(Some(KEY), move |settings, key| on_change(settings.int(key)));

    let drag = gtk::GestureDrag::new();
    let start = std::rc::Rc::new(std::cell::Cell::new(0));
    drag.connect_drag_begin(glib::clone!(
        #[weak]
        overlay,
        #[strong]
        start,
        move |_, _, _| start.set(overlay.width())
    ));
    let clamp = glib::clone!(
        #[weak]
        split,
        #[upgrade_or]
        MIN_WIDTH,
        move |width: f64| {
            let max = (split.width() - MIN_CONTENT).max(MIN_WIDTH);
            (width as i32).clamp(MIN_WIDTH, max)
        }
    );
    let update_clamp = clamp.clone();
    drag.connect_drag_update(glib::clone!(
        #[strong]
        start,
        move |_, dx, _| {
            // The handle is on the panel's left edge: dragging left widens it.
            apply(update_clamp(f64::from(start.get()) - dx));
        }
    ));
    drag.connect_drag_end(glib::clone!(
        #[strong]
        settings,
        move |_, dx, _| {
            let width = clamp(f64::from(start.get()) - dx);
            let _ = settings.set_int(KEY, width);
        }
    ));
    handle.add_controller(drag);
}
