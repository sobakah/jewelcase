//! Drag and drop reordering for list rows (queue, playlists), animated:
//! the row under the pointer opens a gap where the dragged one would land,
//! and the dragged row fades while it's carried (styles in style.css).

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, glib};

thread_local! {
    /// The list a row is being dragged from: a drop only counts in that
    /// list (an index means nothing in another, the queue's in a playlist).
    static DRAGGED_FROM: RefCell<glib::WeakRef<gtk::Widget>> = RefCell::default();
}

/// The list `row` is in: a list box, or a list view (whose rows' content
/// carries the drag, see `attach_item`).
fn list_of(row: &gtk::Widget) -> Option<gtk::Widget> {
    row.ancestor(gtk::ListBox::static_type())
        .or_else(|| row.ancestor(gtk::ListView::static_type()))
}

/// Whether the drag in progress came from `row`'s list.
fn from_same_list(row: &gtk::Widget) -> bool {
    let from = DRAGGED_FROM.with_borrow(glib::WeakRef::upgrade);
    from.is_some() && from == list_of(row)
}

/// Makes `row` draggable and a drop target; `index()` is its place (asked
/// each time: rows may be kept when others come and go). `on_move(from,
/// to)` gets the final index of the moved row, as for "remove, then
/// insert". Alt+Up and Alt+Down move it too, for the keyboard; the focus
/// follows.
pub fn attach(
    row: &gtk::ListBoxRow,
    index: impl Fn() -> u32 + 'static,
    on_move: impl Fn(u32, u32) + 'static,
) {
    let on_move = Rc::new(on_move);
    let index: Rc<dyn Fn() -> u32> = Rc::new(index);
    keyboard(row, index.clone(), on_move.clone());
    let last = row.downgrade();
    drag_and_drop(
        row.upcast_ref(),
        index,
        Rc::new(move || {
            last.upgrade()
                .is_some_and(|row| row.next_sibling().is_none())
        }),
        on_move,
    );
}

/// `attach` for a row of a list view: `item` is the row's content (the
/// list view's own row widgets are reused for other items). `is_last()`:
/// it's the list's last row. For the keyboard, see `attach_list_keys`.
pub fn attach_item(
    item: &gtk::Widget,
    index: impl Fn() -> u32 + 'static,
    is_last: impl Fn() -> bool + 'static,
    on_move: impl Fn(u32, u32) + 'static,
) {
    drag_and_drop(item, Rc::new(index), Rc::new(is_last), Rc::new(on_move));
}

fn drag_and_drop(
    row: &gtk::Widget,
    index: Rc<dyn Fn() -> u32>,
    is_last: Rc<dyn Fn() -> bool>,
    on_move: Rc<dyn Fn(u32, u32)>,
) {
    let drag = gtk::DragSource::builder()
        .actions(gdk::DragAction::MOVE)
        .build();
    let drag_index = index.clone();
    drag.connect_prepare(move |_, _, _| {
        Some(gdk::ContentProvider::for_value(&drag_index().to_value()))
    });
    drag.connect_drag_begin(glib::clone!(
        #[weak]
        row,
        move |source, _| {
            let paintable = gtk::WidgetPaintable::new(Some(&row));
            source.set_icon(Some(&paintable), 24, 24);
            row.add_css_class("dragging");
            let list = list_of(&row);
            DRAGGED_FROM.with_borrow_mut(|from| from.set(list.as_ref()));
        }
    ));
    drag.connect_drag_end(glib::clone!(
        #[weak]
        row,
        move |_, _, _| {
            row.remove_css_class("dragging");
            hold_sheet(&row, false);
            DRAGGED_FROM.with_borrow_mut(|from| from.set(None));
        }
    ));
    row.add_controller(drag);

    let drop = gtk::DropTarget::new(u32::static_type(), gdk::DragAction::MOVE);
    // The gap opens above the row under the pointer; only the last row
    // also offers "after it", in its lower quarter. One direction per row
    // keeps rows from flipping back and forth as the pointer moves.
    let after = move |row: &gtk::Widget, y: f64| is_last() && y > f64::from(row.height()) * 0.75;
    let after = Rc::new(after);
    let gap_after = after.clone();
    let update_gap = glib::clone!(
        #[weak]
        row,
        #[upgrade_or]
        gdk::DragAction::empty(),
        move |y: f64| {
            if !from_same_list(&row) {
                return gdk::DragAction::empty();
            }
            set_gap(&row, Some(!gap_after(&row, y)));
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
            if !from_same_list(&row) {
                return false;
            }
            let Ok(from) = value.get::<u32>() else {
                return false;
            };
            let index = index();
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
fn set_gap(row: &gtk::Widget, gap: Option<bool>) {
    row.remove_css_class("drop-before");
    row.remove_css_class("drop-after");
    match gap {
        Some(true) => row.add_css_class("drop-before"),
        Some(false) => row.add_css_class("drop-after"),
        None => {}
    }
}

thread_local! {
    /// The sheet held by `hold_sheet`, so it's let go also when the row
    /// that held it has left the window (a moved queue row is rebuilt).
    static HELD_SHEET: RefCell<glib::WeakRef<adw::BottomSheet>> = RefCell::default();
}

/// While a row is pressed or dragged inside the player sheet, the sheet
/// mustn't take the drag as "swipe down to close".
pub fn hold_sheet(widget: &impl IsA<gtk::Widget>, hold: bool) {
    if !hold {
        release_sheet();
        return;
    }
    if let Some(sheet) = widget
        .ancestor(adw::BottomSheet::static_type())
        .and_downcast::<adw::BottomSheet>()
    {
        sheet.set_can_close(false);
        HELD_SHEET.with_borrow_mut(|held| held.set(Some(&sheet)));
    }
}

/// Lets go of a held sheet. Every press in the window does this first
/// (window.rs): a release that went elsewhere (a menu) doesn't leave the
/// sheet unable to close.
pub fn release_sheet() {
    if let Some(sheet) = HELD_SHEET.with_borrow_mut(|held| {
        let sheet = held.upgrade();
        held.set(None);
        sheet
    }) {
        sheet.set_can_close(true);
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
fn keyboard(
    row: &gtk::ListBoxRow,
    index: std::rc::Rc<dyn Fn() -> u32>,
    on_move: std::rc::Rc<dyn Fn(u32, u32)>,
) {
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
            let index = index();
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

/// Alt+Up/Down in a list view, whose focus is on its own row widgets:
/// `focused()` gives the focused row's index and the number of rows (or
/// `None`: not a row that moves); `focus(index)` focuses a row again once
/// the list has changed.
pub fn attach_list_keys(
    list: &gtk::ListView,
    focused: impl Fn() -> Option<(u32, u32)> + 'static,
    on_move: impl Fn(u32, u32) + 'static,
    focus: impl Fn(u32) + 'static,
) {
    let focus = Rc::new(focus);
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed(move |_, key, _, modifiers| {
        if !modifiers.contains(gdk::ModifierType::ALT_MASK)
            || !matches!(key, gdk::Key::Up | gdk::Key::Down)
        {
            return glib::Propagation::Proceed;
        }
        let Some((index, count)) = focused() else {
            return glib::Propagation::Proceed;
        };
        let to = match key {
            gdk::Key::Up if index > 0 => index - 1,
            gdk::Key::Down if index + 1 < count => index + 1,
            _ => return glib::Propagation::Stop,
        };
        on_move(index, to);
        let focus = focus.clone();
        glib::idle_add_local_once(move || focus(to));
        glib::Propagation::Stop
    });
    list.add_controller(keys);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A queue row moved by a drag is rebuilt: the row that held the sheet
    /// has left the window when the drag ends, and the sheet must still be
    /// let go (it couldn't close on a click beside it until a restart).
    #[test]
    fn a_sheet_held_by_a_row_that_left_is_let_go() {
        if adw::init().is_err() {
            eprintln!("no display: skipped");
            return;
        }
        let sheet = adw::BottomSheet::new();
        let list = gtk::ListBox::new();
        let row = gtk::ListBoxRow::new();
        list.append(&row);
        sheet.set_sheet(Some(&list));

        hold_sheet(&row, true);
        assert!(!sheet.can_close());
        list.remove(&row);
        hold_sheet(&row, false);
        assert!(sheet.can_close());

        // A release that never came: the next press lets go.
        let other = gtk::ListBoxRow::new();
        list.append(&other);
        hold_sheet(&other, true);
        release_sheet();
        assert!(sheet.can_close());
    }
}
