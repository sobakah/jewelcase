//! A drag handle on the inner edge of a split view's sidebar (the album
//! and playlist panels, and the artist page over the artist grid), so the
//! panel can be made wider or narrower. The width is remembered: one for
//! the album and playlist panels, one for artist pages.

use adw::prelude::*;
use gtk::subclass::prelude::ObjectSubclassIsExt;
use gtk::{gdk, gio, glib};

/// The album and playlist panels' width (shared).
const KEY: &str = "panel-width";
const MIN_WIDTH: i32 = 280;
/// Artist pages over the artist grid.
const ARTIST_KEY: &str = "artist-panel-width";
const ARTIST_MIN_WIDTH: i32 = 360;
/// Below this window width an artist page takes all of it (no handle).
const ARTIST_FULL_BELOW: i32 = 600;
/// Room the content keeps next to the panel.
const MIN_CONTENT: i32 = 300;
/// One key press (Left, Right).
const KEY_STEP: i32 = 20;

/// Wraps the sidebar of `split` (whose sidebar is at the end) with a handle.
pub fn attach(split: &adw::OverlaySplitView, settings: &gio::Settings) {
    attach_panel(split, settings, KEY, MIN_WIDTH, false);
}

/// The artist page over the artist grid (a split view that always
/// overlays): three quarters of the window until its edge is dragged, all
/// of it on narrow windows.
pub fn attach_artist_overlay(split: &adw::OverlaySplitView, settings: &gio::Settings) {
    attach_panel(split, settings, ARTIST_KEY, ARTIST_MIN_WIDTH, true);
}

fn attach_panel(
    split: &adw::OverlaySplitView,
    settings: &gio::Settings,
    setting: &'static str,
    min_width: i32,
    overlays: bool,
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
    split.set_sidebar(Some(&overlay));
    let stable = StableContent::wrap(split);
    // Overlaid (collapsed) panels take the whole width; nothing to resize.
    // The artist overlay always overlays: there it's the window width.
    if !overlays {
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
            if overlays {
                let room = split.width();
                if room <= 0 {
                    return;
                }
                let full = room < ARTIST_FULL_BELOW;
                handle.set_visible(!full);
                let panel = if full {
                    room
                } else if width > 0 {
                    width.clamp(min_width, (room - MIN_CONTENT).max(min_width))
                } else {
                    room * 3 / 4
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
    // While dragging, the grid behind follows the edge live.
    let begin_stable = stable.clone();
    let cancel_stable = stable.clone();
    drag.connect_drag_begin(glib::clone!(
        #[strong]
        grab,
        #[strong]
        wanted,
        move |_, x, _| {
            grab.set(x);
            wanted.set(None);
            if let Some(stable) = &begin_stable {
                stable.set_dragging(true);
            }
        }
    ));
    // The pointer is measured in the split view, which stays put. The
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
            let max = (split.width() - MIN_CONTENT).max(min_width);
            Some((width as i32).clamp(min_width, max))
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
        move |_, _, _| {
            if let Some(width) = wanted.take() {
                let _ = settings.set_int(setting, width);
            }
            if let Some(stable) = &stable {
                stable.set_dragging(false);
            }
        }
    ));
    drag.connect_cancel(move |_, _| {
        if let Some(stable) = &cancel_stable {
            stable.set_dragging(false);
        }
    });
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
            let now = split.sidebar().map_or(0, |s| s.width());
            let max = (split.width() - MIN_CONTENT).max(min_width);
            let width = (now + change).clamp(min_width, max);
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

mod stable_imp {
    use std::cell::{Cell, RefCell};

    use adw::prelude::*;
    use gtk::glib;
    use gtk::subclass::prelude::*;

    #[derive(Debug, Default)]
    pub struct StableContent {
        pub child: RefCell<Option<gtk::Widget>>,
        pub split: glib::WeakRef<adw::OverlaySplitView>,
        /// The content width while the panel is shown.
        pub narrow: Cell<i32>,
        /// While the panel's edge is dragged (not sliding in or out).
        pub dragging: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for StableContent {
        const NAME: &'static str = "GatefoldStableContent";
        type Type = super::StableContent;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for StableContent {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().set_overflow(gtk::Overflow::Hidden);
        }

        fn dispose(&self) {
            if let Some(child) = self.child.take() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for StableContent {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            self.child
                .borrow()
                .as_ref()
                .map_or(gtk::SizeRequestMode::ConstantSize, |c| c.request_mode())
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            self.child
                .borrow()
                .as_ref()
                .map_or((0, 0, -1, -1), |c| c.measure(orientation, for_size))
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            let Some(child) = self.child.borrow().clone() else {
                return;
            };
            let child_width = match self.split.upgrade() {
                // The width the split view gives, in step with the panel.
                Some(split) if !split.is_collapsed() && self.dragging.get() => width,
                Some(split) if !split.is_collapsed() => {
                    let full = split.width();
                    if split.shows_sidebar() {
                        // Opening (or open): the final width right away.
                        let sidebar = split.sidebar().map_or(0, |s| s.width());
                        let narrow = if sidebar > 0 { full - sidebar } else { width };
                        self.narrow.set(narrow);
                        narrow.min(full)
                    } else if width < full && self.narrow.get() > 0 {
                        // Closing: stay narrow until the panel is gone.
                        self.narrow.get().max(width)
                    } else {
                        width
                    }
                }
                _ => width,
            };
            let (min, _, _, _) = child.measure(gtk::Orientation::Horizontal, -1);
            child.allocate(child_width.max(min), height, baseline, None);
        }
    }
}

glib::wrapper! {
    /// A split view's content that changes width once when the panel opens
    /// or closes, not on every frame of the slide: a grid behind the panel
    /// would otherwise reflow its columns all the way and jump around.
    pub struct StableContent(ObjectSubclass<stable_imp::StableContent>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl StableContent {
    /// While the panel's edge is dragged, the content follows the width
    /// the split view gives it.
    fn set_dragging(&self, dragging: bool) {
        self.imp().dragging.set(dragging);
        self.queue_allocate();
    }

    fn wrap(split: &adw::OverlaySplitView) -> Option<Self> {
        let content = split.content()?;
        split.set_content(None::<&gtk::Widget>);
        let stable: Self = glib::Object::new();
        let imp = stable.imp();
        content.set_parent(&stable);
        imp.child.replace(Some(content));
        imp.split.set(Some(split));
        split.set_content(Some(&stable));
        for property in ["show-sidebar", "collapsed"] {
            split.connect_notify_local(
                Some(property),
                glib::clone!(
                    #[weak]
                    stable,
                    move |_, _| stable.queue_allocate()
                ),
            );
        }
        Some(stable)
    }
}
