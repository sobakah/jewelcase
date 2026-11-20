//! A bin that shows its child only when it gets the child's minimum
//! width, and asks for none itself: the header bar's view switcher, which
//! would otherwise hold the window wider than the screen allows (the
//! header bar keeps its title in the middle, so the switcher needs room
//! for the wider side twice). `fits` tells whether the child is shown;
//! the window shows the switcher bar at the bottom instead.

use std::cell::Cell;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;

mod imp {
    use super::*;

    #[derive(Debug, glib::Properties)]
    #[properties(wrapper_type = super::FitBin)]
    pub struct FitBin {
        /// The child got its minimum width in the last layout; notified
        /// after the layout.
        #[property(get)]
        pub fits: Cell<bool>,
    }

    impl Default for FitBin {
        fn default() -> Self {
            Self {
                fits: Cell::new(true),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FitBin {
        const NAME: &'static str = "JewelcaseFitBin";
        type Type = super::FitBin;
        type ParentType = adw::Bin;
    }

    #[glib::derived_properties]
    impl ObjectImpl for FitBin {
        fn constructed(&self) {
            self.parent_constructed();
            // Its own measure and size_allocate (below): with adw::Bin's
            // layout manager, GTK wouldn't call them.
            self.obj().set_layout_manager(None::<gtk::LayoutManager>);
        }
    }

    impl WidgetImpl for FitBin {
        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let Some(child) = self.obj().child() else {
                return (0, 0, -1, -1);
            };
            let (min, natural, _, _) = child.measure(orientation, for_size);
            match orientation {
                gtk::Orientation::Horizontal => (0, natural, -1, -1),
                _ => (min, natural, -1, -1),
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            let Some(child) = self.obj().child() else {
                return;
            };
            let (min, _, _, _) = child.measure(gtk::Orientation::Horizontal, -1);
            let fits = width >= min;
            // Laid out at its minimum when it doesn't fit, and neither
            // drawn (snapshot) nor clickable.
            child.allocate(width.max(min), height, baseline, None);
            child.set_can_target(fits);
            if self.fits.replace(fits) != fits {
                // Not during the layout (the window reveals the switcher
                // bar, which changes it).
                let obj = self.obj().downgrade();
                glib::idle_add_local_once(move || {
                    if let Some(obj) = obj.upgrade() {
                        obj.notify_fits();
                    }
                });
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            if self.fits.get()
                && let Some(child) = self.obj().child()
            {
                self.obj().snapshot_child(&child, snapshot);
            }
        }
    }

    impl BinImpl for FitBin {}
}

glib::wrapper! {
    pub struct FitBin(ObjectSubclass<imp::FitBin>)
        @extends gtk::Widget, adw::Bin,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for FitBin {
    fn default() -> Self {
        glib::Object::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bin asks for no width of its own and shows its child only with
    /// its minimum: the header bar's view switcher held the window wider
    /// than it was (adw::Bin's layout manager measured the child instead).
    #[test]
    fn a_child_that_does_not_fit_is_hidden() {
        crate::gtk_test::run(|| {
            let child = gtk::Box::builder().width_request(300).build();
            let bin = FitBin::default();
            bin.set_child(Some(&child));
            assert_eq!(bin.measure(gtk::Orientation::Horizontal, -1).0, 0);
            bin.allocate(200, 40, -1, None);
            assert!(!bin.fits());
            bin.allocate(320, 40, -1, None);
            assert!(bin.fits());
        });
    }
}
