//! A content with a panel that slides in at its end: the album and
//! playlist panels, the artist page over the artist grid and the album
//! panel on an artist page (CONCEPT.md §4.1, §4.2). Like
//! AdwOverlaySplitView (beside the content, or over it when `collapsed`),
//! but the panel moves like a record sleeve pushed in: a spring that runs
//! a little past its place and settles back, with a shadow on its edge.
//! Every panel uses it, so all of them move alike.
//!
//! Collapsed, the content is dimmed behind the panel; a click on it or
//! Escape closes the panel. Collapsing hides the panel, and uncollapsing
//! brings back one that was open.

use std::cell::{Cell, OnceCell};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, glib};

/// Sliding in: slower than libadwaita's panels and a little bouncy (it
/// runs about 4 % past its place).
fn open_spring() -> adw::SpringParams {
    adw::SpringParams::new(0.72, 1.0, 280.0)
}

/// Sliding out: as unhurried, without a bounce (one would only flash the
/// panel's edge once more).
fn close_spring() -> adw::SpringParams {
    adw::SpringParams::new(1.0, 1.0, 380.0)
}

mod imp {
    use super::*;

    #[derive(Debug, glib::Properties)]
    #[properties(wrapper_type = super::SlideView)]
    pub struct SlideView {
        #[property(get, set = Self::set_show_sidebar)]
        pub show_sidebar: Cell<bool>,
        /// The panel over the content instead of beside it.
        #[property(get, set = Self::set_collapsed)]
        pub collapsed: Cell<bool>,
        #[property(get, set = Self::set_min_sidebar_width, default = 320.0)]
        pub min_sidebar_width: Cell<f64>,
        #[property(get, set = Self::set_max_sidebar_width, default = 440.0)]
        pub max_sidebar_width: Cell<f64>,
        /// Beside the content: this part of the width, within the two above.
        #[property(get, set = Self::set_sidebar_width_fraction, default = 0.38)]
        pub sidebar_width_fraction: Cell<f64>,
        #[property(get = Self::content, set = Self::set_content, nullable)]
        pub content: std::marker::PhantomData<Option<gtk::Widget>>,
        #[property(get = Self::sidebar, set = Self::set_sidebar, nullable)]
        pub sidebar: std::marker::PhantomData<Option<gtk::Widget>>,

        pub content_bin: adw::Bin,
        pub dimming: OnceCell<gtk::Widget>,
        pub pane: adw::Bin,
        /// 0 hidden, 1 in place; above 1 while it bounces.
        pub progress: Cell<f64>,
        pub animation: OnceCell<adw::SpringAnimation>,
        /// Open when it collapsed (to open again when it uncollapses).
        pub open_before_collapse: Cell<bool>,
    }

    impl Default for SlideView {
        fn default() -> Self {
            Self {
                show_sidebar: Cell::new(false),
                collapsed: Cell::new(false),
                min_sidebar_width: Cell::new(320.0),
                max_sidebar_width: Cell::new(440.0),
                sidebar_width_fraction: Cell::new(0.38),
                content: Default::default(),
                sidebar: Default::default(),
                content_bin: adw::Bin::new(),
                dimming: OnceCell::new(),
                pane: adw::Bin::new(),
                progress: Cell::new(0.0),
                animation: OnceCell::new(),
                open_before_collapse: Cell::new(false),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SlideView {
        const NAME: &'static str = "GatefoldSlideView";
        type Type = super::SlideView;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("slide-view");
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for SlideView {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_overflow(gtk::Overflow::Hidden);
            self.content_bin.set_parent(&*obj);
            let dimming = glib::Object::builder::<adw::Bin>()
                .property("css-name", "dimming")
                .build()
                .upcast::<gtk::Widget>();
            dimming.set_child_visible(false);
            dimming.set_parent(&*obj);
            // A click beside an overlaid panel closes it.
            let click = gtk::GestureClick::new();
            click.connect_released(glib::clone!(
                #[weak]
                obj,
                move |_, _, _, _| {
                    if obj.collapsed() {
                        obj.set_show_sidebar(false);
                    }
                }
            ));
            dimming.add_controller(click);
            self.dimming.set(dimming).expect("constructed once");
            self.pane.add_css_class("slide-pane");
            self.pane.set_child_visible(false);
            self.pane.set_parent(&*obj);
            // Escape closes an overlaid panel (an inner one first).
            let keys = gtk::EventControllerKey::new();
            keys.connect_key_pressed(glib::clone!(
                #[weak]
                obj,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |_, key, _, _| {
                    if key == gdk::Key::Escape && obj.collapsed() && obj.show_sidebar() {
                        obj.set_show_sidebar(false);
                        glib::Propagation::Stop
                    } else {
                        glib::Propagation::Proceed
                    }
                }
            ));
            obj.add_controller(keys);

            let target = adw::CallbackAnimationTarget::new(glib::clone!(
                #[weak]
                obj,
                move |value| obj.imp().set_progress(value)
            ));
            let animation = adw::SpringAnimation::new(&*obj, 0.0, 1.0, open_spring(), target);
            self.animation.set(animation).expect("constructed once");
            self.update_styles();
        }

        fn dispose(&self) {
            self.content_bin.unparent();
            if let Some(dimming) = self.dimming.get() {
                dimming.unparent();
            }
            self.pane.unparent();
        }
    }

    impl WidgetImpl for SlideView {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::ConstantSize
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let (content_min, content_nat, _, _) = self.content_bin.measure(orientation, -1);
            let (pane_min, pane_nat, _, _) = self.pane.measure(orientation, -1);
            let _ = for_size; // Children are measured without a size.
            let beside = orientation == gtk::Orientation::Horizontal
                && !self.collapsed.get()
                && self.show_sidebar.get();
            if beside {
                (content_min + pane_min, content_nat + pane_nat, -1, -1)
            } else if orientation == gtk::Orientation::Horizontal && self.collapsed.get() {
                (content_min, content_nat.max(pane_nat), -1, -1)
            } else {
                (content_min.max(pane_min), content_nat.max(pane_nat), -1, -1)
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            let progress = self.progress.get();
            let panel = self.sidebar_width_for(width);
            // The content keeps its final width while the panel slides
            // (a grid behind it would reflow its columns on every frame):
            // narrow from the start of opening to the end of closing.
            let (content_min, _, _, _) = self.content_bin.measure(gtk::Orientation::Horizontal, -1);
            let content_width = if !self.collapsed.get() && (self.show_sidebar.get() || progress > 0.0) {
                (width - panel).max(content_min)
            } else {
                width.max(content_min)
            };
            self.content_bin
                .allocate(content_width, height, baseline, None);

            if let Some(dimming) = self.dimming.get() {
                let dim = self.collapsed.get() && progress > 0.0;
                dimming.set_child_visible(dim);
                dimming.set_opacity(progress.clamp(0.0, 1.0));
                // Only an open panel's dimming takes clicks (not one
                // sliding out).
                dimming.set_can_target(dim && self.show_sidebar.get());
                dimming.allocate(width, height, -1, None);
            }

            // In from the end. While it bounces past its place, the panel
            // is drawn a little wider to stay against the edge (laid out
            // wider, an artist page could change its number of columns
            // for a moment).
            let shown = f64::from(panel) * progress;
            let x = f64::from(width) - shown;
            let stretch = (shown / f64::from(panel.max(1))).max(1.0);
            let transform = gtk::gsk::Transform::new()
                .translate(&gtk::graphene::Point::new(x.round() as f32, 0.0))
                .scale(stretch as f32, 1.0);
            self.pane.allocate(panel, height, -1, Some(transform));
        }
    }

    impl SlideView {
        fn content(&self) -> Option<gtk::Widget> {
            self.content_bin.child()
        }

        fn set_content(&self, content: Option<&gtk::Widget>) {
            self.content_bin.set_child(content);
        }

        fn sidebar(&self) -> Option<gtk::Widget> {
            self.pane.child()
        }

        fn set_sidebar(&self, sidebar: Option<&gtk::Widget>) {
            self.pane.set_child(sidebar);
        }

        fn set_min_sidebar_width(&self, width: f64) {
            self.min_sidebar_width.set(width);
            self.obj().queue_resize();
        }

        fn set_max_sidebar_width(&self, width: f64) {
            self.max_sidebar_width.set(width);
            self.obj().queue_resize();
        }

        fn set_sidebar_width_fraction(&self, fraction: f64) {
            self.sidebar_width_fraction.set(fraction);
            self.obj().queue_resize();
        }

        /// The panel's width in place, out of `width`.
        pub fn sidebar_width_for(&self, width: i32) -> i32 {
            let (pane_min, _, _, _) = self.pane.measure(gtk::Orientation::Horizontal, -1);
            let min = self.min_sidebar_width.get();
            let max = self.max_sidebar_width.get().max(min);
            let wanted = if self.collapsed.get() {
                max
            } else {
                (f64::from(width) * self.sidebar_width_fraction.get()).clamp(min, max)
            };
            (wanted.round() as i32).max(pane_min).min(width.max(pane_min))
        }

        fn set_show_sidebar(&self, show: bool) {
            if self.show_sidebar.replace(show) == show {
                return;
            }
            self.animate_to(if show { 1.0 } else { 0.0 });
            self.obj().queue_resize();
        }

        fn set_collapsed(&self, collapsed: bool) {
            if self.collapsed.replace(collapsed) == collapsed {
                return;
            }
            let obj = self.obj();
            // Like libadwaita's: collapsing hides the panel (it would
            // cover the content); uncollapsing brings it back.
            if collapsed {
                let open = self.show_sidebar.get();
                self.open_before_collapse.set(open);
                if open {
                    self.show_sidebar.set(false);
                    self.jump_to(0.0);
                    obj.notify_show_sidebar();
                }
            } else if self.open_before_collapse.take() && !self.show_sidebar.get() {
                self.show_sidebar.set(true);
                self.jump_to(1.0);
                obj.notify_show_sidebar();
            }
            self.update_styles();
            obj.queue_resize();
        }

        /// The same style classes as libadwaita's split view, so panels
        /// look as they did: beside the content a sidebar, over it an
        /// overlay.
        fn update_styles(&self) {
            let collapsed = self.collapsed.get();
            for class in ["sidebar-pane", "end"] {
                if collapsed {
                    self.pane.remove_css_class(class);
                } else {
                    self.pane.add_css_class(class);
                }
            }
            if collapsed {
                self.pane.add_css_class("overlay-pane");
                self.content_bin.remove_css_class("content-pane");
            } else {
                self.pane.remove_css_class("overlay-pane");
                self.content_bin.add_css_class("content-pane");
            }
        }

        fn animation(&self) -> &adw::SpringAnimation {
            self.animation.get().expect("made in constructed")
        }

        /// Springs from where the panel is now, keeping its speed (when
        /// it's turned around halfway).
        fn animate_to(&self, to: f64) {
            let animation = self.animation();
            let velocity = if animation.state() == adw::AnimationState::Playing {
                animation.velocity()
            } else {
                0.0
            };
            animation.pause();
            animation.set_value_from(self.progress.get());
            animation.set_value_to(to);
            animation.set_initial_velocity(velocity);
            animation.set_spring_params(&if to > 0.5 {
                open_spring()
            } else {
                close_spring()
            });
            animation.play();
        }

        fn jump_to(&self, to: f64) {
            self.animation().pause();
            self.set_progress(to);
        }

        fn set_progress(&self, progress: f64) {
            self.progress.set(progress);
            self.pane.set_child_visible(progress > 0.0);
            self.obj().queue_allocate();
        }
    }

}

glib::wrapper! {
    pub struct SlideView(ObjectSubclass<imp::SlideView>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for SlideView {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl SlideView {
    /// The panel's width in place (not while it bounces).
    pub fn sidebar_width(&self) -> i32 {
        self.imp().sidebar_width_for(self.width())
    }
}
