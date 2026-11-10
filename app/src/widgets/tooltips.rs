//! Tooltips that wait. GTK shows one once the pointer rests on a widget
//! for half a second, and the next ones almost at once while the pointer
//! moves on ("browse mode"); neither can be set. Here a tooltip waits until
//! the pointer has rested on its widget for `DELAY`, every time: the
//! widgets under the pointer are held back (`has-tooltip` off) and let go,
//! and shown the usual way, once it's still long enough.
//!
//! [`wait_for_rest`] goes on every surface with tooltips: the window
//! (dialogs are inside it) and popovers, which have their own.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;

/// How long the pointer rests before a tooltip shows (GTK: 500 ms).
const DELAY: Duration = Duration::from_millis(1000);

#[derive(Default)]
struct Held {
    /// The widgets with a tooltip under the pointer, innermost first.
    widgets: Vec<glib::WeakRef<gtk::Widget>>,
    /// Their tooltip shows: moving on within them keeps it.
    shown: bool,
    timer: Option<glib::SourceId>,
}

/// Tooltips in `root` (a window or popover) wait for the pointer to rest.
pub fn wait_for_rest(root: &impl IsA<gtk::Widget>) {
    let root = root.as_ref();
    let held: Rc<RefCell<Held>> = Rc::default();
    let motion = gtk::EventControllerMotion::new();
    // Before the widgets under the pointer, so GTK never sees them first.
    motion.set_propagation_phase(gtk::PropagationPhase::Capture);
    let on_motion = {
        let held = held.clone();
        let root = root.downgrade();
        move |_: &gtk::EventControllerMotion, x: f64, y: f64| {
            if let Some(root) = root.upgrade() {
                moved(&root, &held, x, y);
            }
        }
    };
    motion.connect_enter(on_motion.clone());
    motion.connect_motion(on_motion);
    motion.connect_leave(move |_| release(&mut held.borrow_mut()));
    root.add_controller(motion);
}

fn moved(root: &gtk::Widget, held: &Rc<RefCell<Held>>, x: f64, y: f64) {
    let mut state = held.borrow_mut();
    let was: Vec<gtk::Widget> = state.widgets.iter().filter_map(|w| w.upgrade()).collect();
    // As GTK looks for a tooltip: the widget under the pointer (also an
    // insensitive one) and its parents.
    let mut chain = Vec::new();
    let mut next = root.pick(
        x,
        y,
        gtk::PickFlags::INSENSITIVE | gtk::PickFlags::NON_TARGETABLE,
    );
    while let Some(widget) = next {
        if widget.has_tooltip() || was.contains(&widget) {
            chain.push(widget.clone());
        }
        if widget == *root {
            break;
        }
        next = widget.parent();
    }
    if chain == was {
        if state.shown {
            return;
        }
        // Still moving over them: the rest starts again.
    } else {
        release(&mut state);
        for widget in &chain {
            widget.set_has_tooltip(false);
        }
        state.widgets = chain.iter().map(|w| w.downgrade()).collect();
    }
    if let Some(timer) = state.timer.take() {
        timer.remove();
    }
    if chain.is_empty() {
        return;
    }
    let held = held.clone();
    state.timer = Some(glib::timeout_add_local_once(DELAY, move || {
        let widgets: Vec<gtk::Widget> = {
            let mut state = held.borrow_mut();
            state.timer = None;
            state.shown = true;
            state.widgets.iter().filter_map(|w| w.upgrade()).collect()
        };
        for widget in &widgets {
            widget.set_has_tooltip(true);
        }
        if let Some(innermost) = widgets.first() {
            innermost.trigger_tooltip_query();
        }
    }));
}

/// The held widgets get their tooltips back (the pointer left them).
fn release(state: &mut Held) {
    if let Some(timer) = state.timer.take() {
        timer.remove();
    }
    for widget in state.widgets.drain(..).filter_map(|w| w.upgrade()) {
        widget.set_has_tooltip(true);
    }
    state.shown = false;
}
