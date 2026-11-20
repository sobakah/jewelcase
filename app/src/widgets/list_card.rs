//! A boxed list's card behind the rows of a list view. A list view builds
//! only the rows on screen, so it can't be a boxed list (whose card is the
//! list itself) and a row's own shadow would fall on its neighbours. Here
//! the card is libadwaita's `.card`, drawn behind the scrolled child and
//! laid out from the first and last rows, wherever they are: above the
//! top or below the bottom while they're scrolled away.

use std::cell::Cell;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{glib, graphene, gsk};

/// How far past the edges the card reaches when its first or last row is
/// scrolled away (more than its shadow and its rounded corners).
const BEYOND: f32 = 48.0;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct ListCard {
        pub card: gtk::Box,
        /// The list view the rows are in: the card is as wide.
        pub list: glib::WeakRef<gtk::Widget>,
        /// The first and last rows, while they're built.
        pub first: glib::WeakRef<gtk::Widget>,
        pub last: glib::WeakRef<gtk::Widget>,
        /// There are rows (else no card).
        pub shown: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ListCard {
        const NAME: &'static str = "JewelcaseListCard";
        type Type = super::ListCard;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for ListCard {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            // Behind the child: the first child is drawn first.
            self.card.add_css_class("card");
            self.card.set_can_target(false);
            self.card.set_parent(&*obj);
            self.card.set_child_visible(false);
            // Its shadow ends at the edges, like the scrolled rows.
            obj.set_overflow(gtk::Overflow::Hidden);
            // Laid out here (a bin's layout would give the card all of it,
            // and with a layout manager, size_allocate isn't called).
            obj.set_layout_manager(None::<gtk::LayoutManager>);
        }

        fn dispose(&self) {
            self.card.unparent();
        }
    }

    impl WidgetImpl for ListCard {
        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            self.obj()
                .child()
                .map_or((0, 0, -1, -1), |child| child.measure(orientation, for_size))
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            // The child first: the rows are where they are now.
            if let Some(child) = self.obj().child() {
                child.allocate(width, height, baseline, None);
            }
            match self.card_bounds(height as f32) {
                Some(bounds) => {
                    self.card.set_child_visible(true);
                    self.card.measure(gtk::Orientation::Vertical, -1);
                    self.card.allocate(
                        bounds.width().round() as i32,
                        bounds.height().round() as i32,
                        -1,
                        Some(gsk::Transform::new().translate(&graphene::Point::new(
                            bounds.x().round(),
                            bounds.y().round(),
                        ))),
                    );
                }
                None => self.card.set_child_visible(false),
            }
        }
    }

    impl BinImpl for ListCard {}

    impl ListCard {
        /// Where the card goes: as wide as the list, from the first row's
        /// top to the last row's bottom.
        fn card_bounds(&self, height: f32) -> Option<graphene::Rect> {
            if !self.shown.get() {
                return None;
            }
            let obj = self.obj();
            let list = self.list.upgrade()?.compute_bounds(&*obj)?;
            let bounds = |row: &glib::WeakRef<gtk::Widget>| {
                row.upgrade().and_then(|row| row.compute_bounds(&*obj))
            };
            let top = bounds(&self.first).map_or(-BEYOND, |b| b.y().max(-BEYOND));
            let bottom = bounds(&self.last).map_or(height + BEYOND, |b| {
                (b.y() + b.height()).min(height + BEYOND)
            });
            (bottom > top).then(|| graphene::Rect::new(list.x(), top, list.width(), bottom - top))
        }
    }
}

glib::wrapper! {
    pub struct ListCard(ObjectSubclass<imp::ListCard>)
        @extends gtk::Widget, adw::Bin,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ListCard {
    /// The card follows `list`'s rows; `scroller` scrolls them.
    pub fn follow(&self, list: &impl IsA<gtk::Widget>, scroller: &gtk::ScrolledWindow) {
        self.imp().list.set(Some(list.upcast_ref()));
        // Scrolling moves the rows without laying this out again.
        scroller.vadjustment().connect_value_changed(glib::clone!(
            #[weak(rename_to = card)]
            self,
            move |_| card.queue_allocate()
        ));
    }

    /// Whether there are rows at all (else no card).
    pub fn set_shown(&self, shown: bool) {
        if self.imp().shown.replace(shown) != shown {
            self.queue_allocate();
        }
    }

    /// `row` is the first or the last row (or neither, any more). The card
    /// reaches from the first to the last; while one isn't built (scrolled
    /// away), past that edge.
    pub fn set_row(&self, row: &gtk::Widget, first: bool, last: bool) {
        let imp = self.imp();
        let mut changed = false;
        for (end, is) in [(&imp.first, first), (&imp.last, last)] {
            let was = end.upgrade().as_ref() == Some(row);
            if is && !was {
                end.set(Some(row));
                changed = true;
            } else if !is && was {
                end.set(None);
                changed = true;
            }
        }
        if changed {
            self.queue_allocate();
        }
    }
}
