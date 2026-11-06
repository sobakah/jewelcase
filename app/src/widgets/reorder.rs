//! Drag and drop reordering for list rows (queue, playlists), animated:
//! the row under the pointer opens a gap where the dragged one would land,
//! and the dragged row fades while it's carried (styles in style.css).

use adw::prelude::*;
use gtk::{gdk, glib};

/// Makes `row` (at `index`) draggable and a drop target. `on_move(from, to)`
/// gets the final index of the moved row, as for "remove, then insert".
pub fn attach(row: &gtk::ListBoxRow, index: u32, on_move: impl Fn(u32, u32) + 'static) {
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
    let update_gap = glib::clone!(
        #[weak]
        row,
        #[upgrade_or]
        gdk::DragAction::empty(),
        move |y: f64| {
            let before = y < f64::from(row.height()) / 2.0;
            set_gap(&row, Some(before));
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
            set_gap(&row, None);
            let Ok(from) = value.get::<u32>() else {
                return false;
            };
            let before = y < f64::from(row.height()) / 2.0;
            let mut to = if before { index } else { index + 1 };
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
