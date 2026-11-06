//! A drag handle on the inner edge of a split view's sidebar (the album
//! and playlist panels), so the panel can be made wider or narrower. The
//! width is shared by all panels and remembered.

use adw::prelude::*;
use gtk::subclass::prelude::ObjectSubclassIsExt;
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
    StableContent::wrap(split);
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
    fn wrap(split: &adw::OverlaySplitView) {
        let Some(content) = split.content() else {
            return;
        };
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
    }
}
