//! A spectrum visualizer (CONCEPT.md §4.3): bars in the cover color that
//! follow the local song as it's heard, rising at once and falling back
//! gently. A still, faint strip for Spotify songs (Soloist plays them
//! itself), while paused and when nothing plays.
//!
//! It asks the player for bars once a frame, only while it's on screen
//! and the music plays; the player measures the spectrum only while a
//! visualizer is on screen. Preferences → Playback turns it off, and so
//! do GNOME's animations being off.

use std::cell::{Cell, RefCell};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};

use crate::application::GatefoldApplication;
use crate::config::APP_ID;

/// How fast a bar falls back (its full height per second).
const FALL_PER_S: f32 = 1.8;
/// The height of a silent bar, as part of the widget's height.
const REST: f32 = 0.12;

mod imp {
    use super::*;

    #[derive(Debug, glib::Properties)]
    #[properties(wrapper_type = super::Visualizer)]
    pub struct Visualizer {
        /// How many bars.
        #[property(get, set = Self::set_bars, minimum = 1, maximum = 128, default = 32)]
        pub bars: Cell<u32>,
        /// Bar width as part of each bar's slot (the rest is the gap).
        #[property(get, set, minimum = 0.1, maximum = 1.0, default = 0.6)]
        pub fill: Cell<f64>,
        /// Above 0: as many bars as fit (each slot this wide, in pixels),
        /// from `min-bars` to `max-bars`, centered in the space given.
        #[property(get, set, minimum = 0.0, maximum = 64.0, default = 0.0)]
        pub slot: Cell<f64>,
        #[property(get, set, minimum = 1, maximum = 128, default = 4)]
        pub min_bars: Cell<u32>,
        #[property(get, set, minimum = 1, maximum = 128, default = 24)]
        pub max_bars: Cell<u32>,
        pub levels: RefCell<Vec<f32>>,
        pub tick: RefCell<Option<gtk::TickCallbackId>>,
        pub last_frame: Cell<i64>,
        pub using: Cell<bool>,
        pub handlers: RefCell<Vec<(glib::Object, glib::SignalHandlerId)>>,
    }

    impl Default for Visualizer {
        fn default() -> Self {
            Self {
                bars: Cell::new(32),
                fill: Cell::new(0.6),
                slot: Cell::new(0.0),
                min_bars: Cell::new(4),
                max_bars: Cell::new(24),
                levels: RefCell::new(vec![0.0; 32]),
                tick: RefCell::default(),
                last_frame: Cell::new(0),
                using: Cell::new(false),
                handlers: RefCell::default(),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Visualizer {
        const NAME: &'static str = "GatefoldVisualizer";
        type Type = super::Visualizer;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("visualizer");
            klass.set_accessible_role(gtk::AccessibleRole::Presentation);
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for Visualizer {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.add_css_class("visualizer");
            let player = GatefoldApplication::get().player();
            let playing = player.connect_playing_notify(glib::clone!(
                #[weak]
                obj,
                move |_| obj.update_running()
            ));
            let settings = gtk::gio::Settings::new(APP_ID);
            let setting = settings.connect_changed(
                Some("visualizer"),
                glib::clone!(
                    #[weak]
                    obj,
                    move |_, _| obj.update_visible()
                ),
            );
            let track = player.connect_current_track_notify(glib::clone!(
                #[weak]
                obj,
                move |_| obj.update_running()
            ));
            let mut handlers = vec![
                (player.clone().upcast(), playing),
                (player.upcast(), track),
                (settings.upcast(), setting),
            ];
            if let Some(gtk_settings) = gtk::Settings::default() {
                let animations = gtk_settings.connect_gtk_enable_animations_notify(glib::clone!(
                    #[weak]
                    obj,
                    move |_| obj.update_visible()
                ));
                handlers.push((gtk_settings.upcast(), animations));
            }
            self.handlers.replace(handlers);
            obj.update_visible();
        }

        fn dispose(&self) {
            for (object, handler) in self.handlers.take() {
                object.disconnect(handler);
            }
            self.obj().stop();
        }
    }

    impl WidgetImpl for Visualizer {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let slot = self.slot.get();
            if orientation == gtk::Orientation::Horizontal && slot > 0.0 {
                let min = (self.min_bars.get() as f64 * slot).ceil() as i32;
                let natural = (self.max_bars.get() as f64 * slot).ceil() as i32;
                (min, natural, -1, -1)
            } else {
                (0, 0, -1, -1)
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.parent_size_allocate(width, height, baseline);
            let slot = self.slot.get();
            if slot > 0.0 {
                let fits = (width as f64 / slot) as u32;
                let bars = fits.clamp(
                    self.min_bars.get(),
                    self.max_bars.get().max(self.min_bars.get()),
                );
                if bars != self.bars.get() {
                    self.set_bars(bars);
                }
            }
        }

        fn map(&self) {
            self.parent_map();
            self.obj().update_running();
        }

        fn unmap(&self) {
            self.obj().stop();
            self.parent_unmap();
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let (width, height) = (obj.width() as f32, obj.height() as f32);
            let levels = self.levels.borrow();
            let count = levels.len().max(1) as f32;
            if width <= 0.0 || height <= 0.0 {
                return;
            }
            let color = obj.color();
            // Fixed slots are centered; otherwise the bars fill the width.
            let (slot, start) = match self.slot.get() as f32 {
                fixed if fixed > 0.0 => (fixed, (width - fixed * count) / 2.0),
                _ => (width / count, 0.0),
            };
            let bar = (slot * self.fill.get() as f32).max(1.0);
            // Fitted bars grow taller with their number (wider space).
            let full = if self.slot.get() > 0.0 {
                height.min(16.0 + 1.2 * count)
            } else {
                height
            };
            for (i, &level) in levels.iter().enumerate() {
                let h = (REST + (1.0 - REST) * level) * full;
                let x = start + i as f32 * slot + (slot - bar) / 2.0;
                let y = (height - h) / 2.0;
                let rect = graphene::Rect::new(x, y, bar, h);
                let radius = bar.min(h) / 2.0;
                let corner = graphene::Size::new(radius, radius);
                let rounded = gsk::RoundedRect::new(rect, corner, corner, corner, corner);
                // Faint at rest, full when loud.
                let alpha = color.alpha() * (0.35 + 0.65 * level);
                let color = gdk::RGBA::new(color.red(), color.green(), color.blue(), alpha);
                snapshot.push_rounded_clip(&rounded);
                snapshot.append_color(&color, &rect);
                snapshot.pop();
            }
        }
    }

    impl Visualizer {
        fn set_bars(&self, bars: u32) {
            self.bars.set(bars);
            self.levels.replace(vec![0.0; bars as usize]);
            self.obj().queue_draw();
        }
    }
}

glib::wrapper! {
    pub struct Visualizer(ObjectSubclass<imp::Visualizer>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Visualizer {
    /// Shown with the setting on and GNOME's animations on.
    fn update_visible(&self) {
        let setting = gtk::gio::Settings::new(APP_ID).boolean("visualizer");
        let animations = gtk::Settings::default().is_none_or(|s| s.is_gtk_enable_animations());
        self.set_visible(setting && animations);
    }

    /// Ticks while on screen and playing (and until the bars have
    /// fallen back after a pause).
    fn update_running(&self) {
        let imp = self.imp();
        if !self.is_mapped() {
            return;
        }
        if !imp.using.replace(true) {
            GatefoldApplication::get().player().use_spectrum(true);
        }
        if imp.tick.borrow().is_some() {
            return;
        }
        imp.last_frame.set(0);
        let id = self.add_tick_callback(|obj, clock| obj.step(clock.frame_time()));
        imp.tick.replace(Some(id));
    }

    fn stop(&self) {
        let imp = self.imp();
        if let Some(id) = imp.tick.take() {
            id.remove();
        }
        if imp.using.replace(false) {
            GatefoldApplication::get().player().use_spectrum(false);
        }
        imp.levels.borrow_mut().fill(0.0);
    }

    fn step(&self, frame_time: i64) -> glib::ControlFlow {
        let imp = self.imp();
        let last = imp.last_frame.replace(frame_time);
        let dt = if last == 0 {
            0.0
        } else {
            ((frame_time - last) as f32 / 1_000_000.0).min(0.1)
        };
        let player = GatefoldApplication::get().player();
        let count = imp.bars.get() as usize;
        let targets = player.spectrum_bars(count);
        let mut moving = false;
        {
            let mut levels = imp.levels.borrow_mut();
            for (i, level) in levels.iter_mut().enumerate() {
                let target = targets
                    .as_ref()
                    .and_then(|t| t.get(i))
                    .copied()
                    .unwrap_or(0.0);
                let fallen = (*level - FALL_PER_S * dt).max(0.0);
                *level = target.max(fallen);
                moving |= *level > 0.0;
            }
        }
        self.queue_draw();
        // Paused, or a Spotify song, and settled: no more frames until
        // it plays again or the song changes.
        if !moving && (!player.playing() || targets.is_none() && player.plays_from_spotify()) {
            imp.tick.take();
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    }
}
