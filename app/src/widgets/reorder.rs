//! Drag and drop reordering for list rows (queue, playlists), animated:
//! the row under the pointer opens a gap where the dragged one would land,
//! and the dragged row fades while it's carried (styles in style.css).

use adw::prelude::*;
use gtk::{gdk, glib};

/// Makes `row` (at `index`) draggable and a drop target. `on_move(from, to)`
/// gets the final index of the moved row, as for "remove, then insert".
/// Alt+Up and Alt+Down move it too, for the keyboard; the focus follows.
pub fn attach(row: &gtk::ListBoxRow, index: u32, on_move: impl Fn(u32, u32) + 'static) {
    let on_move = std::rc::Rc::new(on_move);
    keyboard(row, index, on_move.clone());
    let on_move = move |from, to| on_move(from, to);
    let drag = gtk::DragSource::builder()
        .actions(gdk::DragAction::MOVE)
        .content(&gdk::ContentProvider::for_value(&index.to_value()))
        .build();
    drag.connect_drag_begin(glib::clone!(
        #[weak]
        row,
        move |source, _| {
            let paintable = gtk::WidgetPaintable::new(Some(&row));
            source.set_icon(Some(&paintable), 24, 24);
            row.add_css_class("dragging");
        }
    ));
    drag.connect_drag_end(glib::clone!(
        #[weak]
        row,
        move |_, _, _| {
            row.remove_css_class("dragging");
            hold_sheet(&row, false);
        }
    ));
    row.add_controller(drag);

    let drop = gtk::DropTarget::new(u32::static_type(), gdk::DragAction::MOVE);
    // The gap opens above the row under the pointer; only the last row
    // also offers "after it", in its lower quarter. One direction per row
    // keeps rows from flipping back and forth as the pointer moves.
    let after = |row: &gtk::ListBoxRow, y: f64| {
        row.next_sibling().is_none() && y > f64::from(row.height()) * 0.75
    };
    let update_gap = glib::clone!(
        #[weak]
        row,
        #[upgrade_or]
        gdk::DragAction::empty(),
        move |y: f64| {
            set_gap(&row, Some(!after(&row, y)));
            gdk::DragAction::MOVE
        }
    );
    let on_enter = update_gap.clone();
    drop.connect_enter(move |_, _, y| on_enter(y));
    drop.connect_motion(move |_, _, y| update_gap(y));
    drop.connect_leave(glib::clone!(
        #[weak]
        row,
        move |_| set_gap(&row, None)
    ));
    drop.connect_drop(glib::clone!(
        #[weak]
        row,
        #[upgrade_or]
        false,
        move |_, value, _, y| {
            let place_after = after(&row, y);
            set_gap(&row, None);
            let Ok(from) = value.get::<u32>() else {
                return false;
            };
            let mut to = if place_after { index + 1 } else { index };
            if from < to {
                to -= 1;
            }
            if from != to {
                on_move(from, to);
            }
            true
        }
    ));
    row.add_controller(drop);
}

/// Opens a gap above (`Some(true)`) or below the row, or closes it.
fn set_gap(row: &gtk::ListBoxRow, gap: Option<bool>) {
    row.remove_css_class("drop-before");
    row.remove_css_class("drop-after");
    match gap {
        Some(true) => row.add_css_class("drop-before"),
        Some(false) => row.add_css_class("drop-after"),
        None => {}
    }
}

/// While a row is pressed or dragged inside the player sheet, the sheet
/// mustn't take the drag as "swipe down to close".
pub fn hold_sheet(widget: &impl IsA<gtk::Widget>, hold: bool) {
    if let Some(sheet) = widget
        .ancestor(adw::BottomSheet::static_type())
        .and_downcast::<adw::BottomSheet>()
    {
        sheet.set_can_close(!hold);
    }
}

/// Holds the sheet from the press on a row of `list` until the release
/// (or the end of a drag, see `attach`).
pub fn guard_sheet(list: &gtk::ListBox) {
    let legacy = gtk::EventControllerLegacy::new();
    legacy.set_propagation_phase(gtk::PropagationPhase::Capture);
    legacy.connect_event(|controller, event| {
        let widget = controller.widget();
        match event.event_type() {
            gdk::EventType::ButtonPress | gdk::EventType::TouchBegin => {
                if let Some(widget) = &widget {
                    hold_sheet(widget, true);
                }
            }
            gdk::EventType::ButtonRelease
            | gdk::EventType::TouchEnd
            | gdk::EventType::TouchCancel => {
                if let Some(widget) = &widget {
                    hold_sheet(widget, false);
                }
            }
            _ => {}
        }
        glib::Propagation::Proceed
    });
    list.add_controller(legacy);
}

/// Alt+Up/Down: the row one place up or down, then focused again (the
/// list is rebuilt).
fn keyboard(row: &gtk::ListBoxRow, index: u32, on_move: std::rc::Rc<dyn Fn(u32, u32)>) {
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed(glib::clone!(
        #[weak]
        row,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |_, key, _, modifiers| {
            if !modifiers.contains(gdk::ModifierType::ALT_MASK) {
                return glib::Propagation::Proceed;
            }
            let last = row
                .parent()
                .and_downcast::<gtk::ListBox>()
                .map_or(0, |list| {
                    let mut count = 0;
                    while list.row_at_index(count).is_some() {
                        count += 1;
                    }
                    count.max(1) - 1
                });
            let to = match key {
                gdk::Key::Up if index > 0 => index - 1,
                gdk::Key::Down if (index as i32) < last => index + 1,
                gdk::Key::Up | gdk::Key::Down => return glib::Propagation::Stop,
                _ => return glib::Propagation::Proceed,
            };
            let list = row.parent().and_downcast::<gtk::ListBox>();
            // Not past the queue's current song.
            let fixed = list
                .as_ref()
                .and_then(|l| l.row_at_index(to as i32))
                .is_some_and(|r| r.has_css_class("now-playing"));
            if fixed {
                return glib::Propagation::Stop;
            }
            on_move(index, to);
            if let Some(list) = list {
                glib::idle_add_local_once(move || {
                    if let Some(moved) = list.row_at_index(to as i32) {
                        moved.grab_focus();
                    }
                });
            }
            glib::Propagation::Stop
        }
    ));
    row.add_controller(keys);
}
