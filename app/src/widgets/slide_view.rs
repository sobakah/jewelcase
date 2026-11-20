//! A content with a panel that slides in at its end: the album and
//! playlist panels, the artist page over the artist grid and the album
//! panel on an artist page (CONCEPT.md §4.1, §4.2). Like
//! AdwOverlaySplitView (beside the content, or over it when `collapsed`),
//! but the panel moves like a record sleeve pushed in: a spring that runs
//! a little past its place and settles back, with a shadow on its edge.
//! Every panel uses it, so all of them move alike.
//!
//! Collapsed, the content is dimmed behind the panel; a click on it or
//! Escape closes the panel. Collapsing hides the panel (unless
//! `keep-open`), and uncollapsing brings back one that was open.
//!
//! With `min-content-width`, the panel goes over the content by itself
//! whenever the content wouldn't keep that much room beside it (an
//! album on an artist page): decided in each layout, so a panel never
//! squeezes or covers the content while the window is resized.
//! `set_width_rule` lets the panel's width follow the window's in the
//! same layout (panel_resizer.rs).

use std::cell::{Cell, OnceCell, RefCell};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, glib};

/// Sliding in: slower than libadwaita's panels and a little bouncy (it
/// runs about 2 % past its place).
fn open_spring() -> adw::SpringParams {
    adw::SpringParams::new(0.78, 1.0, 280.0)
}

/// Sliding out: as unhurried, without a bounce (one would only flash the
/// panel's edge once more).
fn close_spring() -> adw::SpringParams {
    adw::SpringParams::new(1.0, 1.0, 380.0)
}

/// What a panel's width is worked out from, in each layout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Room {
    /// The panel view's width.
    pub width: i32,
    /// The window's.
    pub window: i32,
    /// The panel goes over the content.
    pub over: bool,
}

pub type WidthRule = Box<dyn Fn(Room) -> i32>;

mod imp {
    use super::*;

    #[derive(glib::Properties)]
    #[properties(wrapper_type = super::SlideView)]
    pub struct SlideView {
        #[property(get, set = Self::set_show_sidebar)]
        pub show_sidebar: Cell<bool>,
        /// The panel over the content instead of beside it.
        #[property(get, set = Self::set_collapsed)]
        pub collapsed: Cell<bool>,
        /// An open panel stays open when it collapses, over the content
        /// (else it's hidden, as libadwaita's are).
        #[property(get, set)]
        pub keep_open: Cell<bool>,
        /// The panel's edge is being dragged: a grid in the content keeps
        /// its layout until the drag ends (sectioned_grid.rs).
        #[property(get, set)]
        pub resizing: Cell<bool>,
        /// Beside the panel, the content keeps at least this much room,
        /// else the panel goes over it (0: only when `collapsed`).
        #[property(get, set = Self::set_min_content_width)]
        pub min_content_width: Cell<i32>,
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
        /// The panel's width from the room (else the properties above).
        pub width_rule: RefCell<Option<WidthRule>>,
        /// Over the content in the last layout (collapsed, or no room).
        pub over: Cell<bool>,
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
                keep_open: Cell::new(false),
                resizing: Cell::new(false),
                min_content_width: Cell::new(0),
                min_sidebar_width: Cell::new(320.0),
                max_sidebar_width: Cell::new(440.0),
                sidebar_width_fraction: Cell::new(0.38),
                content: Default::default(),
                sidebar: Default::default(),
                content_bin: adw::Bin::new(),
                dimming: OnceCell::new(),
                pane: adw::Bin::new(),
                width_rule: RefCell::new(None),
                over: Cell::new(false),
                progress: Cell::new(0.0),
                animation: OnceCell::new(),
                open_before_collapse: Cell::new(false),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SlideView {
        const NAME: &'static str = "JewelcaseSlideView";
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
                    if obj.imp().over.get() {
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
                    if key == gdk::Key::Escape && obj.imp().over.get() && obj.show_sidebar() {
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
            let horizontal = orientation == gtk::Orientation::Horizontal;
            // A panel that can go over the content needs no room beside it.
            let can_go_over = self.collapsed.get() || self.min_content_width.get() > 0;
            let beside = horizontal && !can_go_over && self.show_sidebar.get();
            if beside {
                (content_min + pane_min, content_nat + pane_nat, -1, -1)
            } else if horizontal && can_go_over {
                (content_min, content_nat.max(pane_nat), -1, -1)
            } else {
                (content_min.max(pane_min), content_nat.max(pane_nat), -1, -1)
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            let progress = self.progress.get();
            let (content_min, _, _, _) = self.content_bin.measure(gtk::Orientation::Horizontal, -1);
            let over = self.goes_over(width, content_min);
            if self.over.replace(over) != over {
                // Not during the layout: new style classes would undo it
                // (drawn "without a current allocation").
                let obj = self.obj().downgrade();
                glib::idle_add_local_once(move || {
                    if let Some(obj) = obj.upgrade() {
                        obj.imp().update_styles();
                    }
                });
            }
            let panel = self.sidebar_width_for(width);
            // The content takes its final width once, when the panel
            // starts to move (a grid behind it would reflow its columns on
            // every frame): narrow as it starts opening, full as it starts
            // closing, under the panel sliding away (staying narrow until
            // the end left an empty strip behind the panel).
            let content_width = if !over && self.show_sidebar.get() {
                (width - panel).max(content_min)
            } else {
                width.max(content_min)
            };
            self.content_bin
                .allocate(content_width, height, baseline, None);

            if let Some(dimming) = self.dimming.get() {
                let dim = over && progress > 0.0;
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

        fn set_min_content_width(&self, width: i32) {
            self.min_content_width.set(width);
            self.obj().queue_resize();
        }

        /// The window's width (the panel view's own outside one).
        fn window_width(&self, width: i32) -> i32 {
            self.obj()
                .root()
                .map(|root| root.width())
                .filter(|w| *w > 0)
                .unwrap_or(width)
        }

        /// The panel's width out of `width`, beside the content or over it.
        fn width_for(&self, width: i32, over: bool) -> i32 {
            let (pane_min, _, _, _) = self.pane.measure(gtk::Orientation::Horizontal, -1);
            let wanted = if let Some(rule) = &*self.width_rule.borrow() {
                rule(Room {
                    width,
                    window: self.window_width(width),
                    over,
                })
            } else {
                let min = self.min_sidebar_width.get();
                let max = self.max_sidebar_width.get().max(min);
                let wanted = if over {
                    max
                } else {
                    (f64::from(width) * self.sidebar_width_fraction.get()).clamp(min, max)
                };
                wanted.round() as i32
            };
            wanted.max(pane_min).min(width.max(pane_min))
        }

        /// Over the content: collapsed, or the content wouldn't keep
        /// `min-content-width` (nor its own minimum) beside the panel.
        fn goes_over(&self, width: i32, content_min: i32) -> bool {
            let keep = self.min_content_width.get();
            self.collapsed.get()
                || (keep > 0 && width - self.width_for(width, false) < keep.max(content_min))
        }

        /// The panel's width in place, out of `width`.
        pub fn sidebar_width_for(&self, width: i32) -> i32 {
            self.width_for(width, self.over.get())
        }

        fn set_show_sidebar(&self, show: bool) {
            if self.show_sidebar.replace(show) == show {
                return;
            }
            // Opened or closed while collapsed: that's what uncollapsing
            // keeps, not the state from before.
            if self.collapsed.get() {
                self.open_before_collapse.set(false);
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
            if collapsed && self.keep_open.get() {
                self.open_before_collapse.set(false);
            } else if collapsed {
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
            if collapsed || self.min_content_width.get() == 0 {
                self.over.set(collapsed);
            }
            self.update_styles();
            obj.queue_resize();
        }

        /// The same style classes as libadwaita's split view, so panels
        /// look as they did: beside the content a sidebar, over it an
        /// overlay.
        fn update_styles(&self) {
            let collapsed = self.over.get();
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

    /// The panel's width from the room, in each layout (instead of
    /// `min-sidebar-width`, `max-sidebar-width` and the fraction).
    pub fn set_width_rule(&self, rule: impl Fn(Room) -> i32 + 'static) {
        self.imp().width_rule.replace(Some(Box::new(rule)));
        self.queue_resize();
    }

    /// The panel is over the content (collapsed, or no room beside it).
    pub fn is_over(&self) -> bool {
        self.imp().over.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A panel collapsing (the window narrows) hides, as libadwaita's do,
    /// and comes back when it uncollapses; an artist page (`keep-open`)
    /// stays open, over the grid.
    #[test]
    fn collapsing_hides_the_panel_unless_it_keeps_open() {
        crate::gtk_test::run(|| {
            let view = SlideView::default();
            view.set_sidebar(Some(
                gtk::Label::new(Some("panel")).upcast_ref::<gtk::Widget>(),
            ));
            view.set_show_sidebar(true);
            view.set_collapsed(true);
            assert!(!view.show_sidebar());
            view.set_collapsed(false);
            assert!(view.show_sidebar());

            let artist = SlideView::default();
            artist.set_keep_open(true);
            artist.set_sidebar(Some(
                gtk::Label::new(Some("page")).upcast_ref::<gtk::Widget>(),
            ));
            artist.set_show_sidebar(true);
            artist.set_collapsed(true);
            assert!(artist.show_sidebar());
            artist.set_collapsed(false);
            assert!(artist.show_sidebar());
        });
    }

    /// With `min-content-width` (an album on an artist page), the panel
    /// goes over the content when the content wouldn't keep that much
    /// beside it, in the same layout, instead of covering part of it
    /// while it's laid out beside (the window widening).
    #[test]
    fn a_panel_goes_over_content_without_room_beside_it() {
        crate::gtk_test::run(|| {
            let view = SlideView::default();
            let content = gtk::Label::new(Some("page"));
            view.set_content(Some(content.upcast_ref::<gtk::Widget>()));
            view.set_sidebar(Some(
                gtk::Label::new(Some("album")).upcast_ref::<gtk::Widget>(),
            ));
            view.set_min_content_width(480);
            view.set_show_sidebar(true);
            let _ = view.measure(gtk::Orientation::Horizontal, -1);
            // 0.38 of 700 is under the panel's 320; 380 left.
            view.allocate(700, 600, -1, None);
            assert!(view.is_over());
            assert_eq!(content.width(), 700);
            // 0.38 of 900: 342; 558 left.
            view.allocate(900, 600, -1, None);
            assert!(!view.is_over());
            assert_eq!(content.width(), 900 - 342);
        });
    }
}
