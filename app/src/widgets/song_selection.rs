//! Choosing several songs in the album and playlist panels (as Jewelcase
//! Mobile's long press): Ctrl+click chooses a song or drops it, Shift+click
//! the songs from the last one chosen; while songs are chosen a plain click
//! does the same as Ctrl+click instead of playing. A bar under the panel's
//! header then plays them next, queues them, adds them to a playlist or
//! removes them from it; Esc or its close button ends the choosing.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gettextrs::{gettext, ngettext};
use gtk::prelude::*;
use gtk::{gdk, glib};

/// Which songs are chosen, by key (a track's ID in an album, an entry's
/// in a playlist), without GTK.
#[derive(Debug, Default)]
pub struct Choice {
    chosen: Vec<i64>,
    /// The last song clicked, where a Shift+click's range starts.
    anchor: Option<i64>,
    /// Every song's key, in the order shown.
    order: Vec<i64>,
}

impl Choice {
    pub fn is_active(&self) -> bool {
        !self.chosen.is_empty()
    }

    pub fn contains(&self, key: i64) -> bool {
        self.chosen.contains(&key)
    }

    /// The chosen songs in the order shown.
    pub fn chosen(&self) -> Vec<i64> {
        self.order
            .iter()
            .copied()
            .filter(|key| self.chosen.contains(key))
            .collect()
    }

    /// The songs shown now; chosen ones no longer there are dropped.
    pub fn set_order(&mut self, order: Vec<i64>) {
        self.chosen.retain(|key| order.contains(key));
        if self.anchor.is_some_and(|a| !order.contains(&a)) {
            self.anchor = None;
        }
        self.order = order;
    }

    pub fn clear(&mut self) {
        self.chosen.clear();
        self.anchor = None;
    }

    /// A click on a song with these keys held; whether the choosing took
    /// it (else the row does what a click does: play, or nothing).
    pub fn click(&mut self, key: i64, ctrl: bool, shift: bool) -> bool {
        if shift
            && let Some(anchor) = self.anchor
            && let (Some(from), Some(to)) = (
                self.order.iter().position(|k| *k == anchor),
                self.order.iter().position(|k| *k == key),
            )
        {
            let range = &self.order[from.min(to)..=from.max(to)];
            if !ctrl {
                self.chosen.clear();
            }
            for key in range {
                if !self.chosen.contains(key) {
                    self.chosen.push(*key);
                }
            }
            return true;
        }
        if !(ctrl || shift || self.is_active()) {
            return false;
        }
        if let Some(at) = self.chosen.iter().position(|k| *k == key) {
            self.chosen.remove(at);
        } else {
            self.chosen.push(key);
        }
        self.anchor = Some(key);
        true
    }
}

/// What the bar's buttons do with the chosen songs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    PlayNext,
    AddToQueue,
    AddToPlaylist,
    Remove,
}

/// What a panel does with the chosen songs (keys in order).
type ActCallback = Box<dyn Fn(Act, Vec<i64>)>;

struct Inner {
    choice: RefCell<Choice>,
    bar: gtk::Revealer,
    count: gtk::Label,
    remove: gtk::Button,
    playlist: gtk::Button,
    /// The rows shown, by key, to mark the chosen ones (a playlist's rows
    /// come and go as they scroll by).
    rows: RefCell<HashMap<i64, Vec<glib::WeakRef<gtk::Widget>>>>,
    act: RefCell<Option<ActCallback>>,
    removable: Cell<bool>,
}

/// A panel's choosing: its bar and its rows.
#[derive(Clone)]
pub struct SongSelection(Rc<Inner>);

impl SongSelection {
    pub fn new() -> Self {
        let bar_box = gtk::Box::builder()
            .spacing(6)
            .margin_start(12)
            .margin_end(12)
            .margin_top(6)
            .margin_bottom(6)
            .css_classes(["toolbar", "selection-bar"])
            .build();
        let close = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular"])
            .build();
        super::set_accessible_label(&close, &gettext("Cancel"));
        let count = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["heading"])
            .build();
        let button = |icon: &str, tooltip: String| {
            gtk::Button::builder()
                .icon_name(icon)
                .tooltip_text(tooltip)
                .valign(gtk::Align::Center)
                .css_classes(["flat", "circular"])
                .build()
        };
        let playlist = button("view-list-bullet-symbolic", gettext("Add to Playlist…"));
        let queue = button("list-add-symbolic", gettext("Add to Queue"));
        let next = button("jewelcase-play-next-symbolic", gettext("Play Next"));
        let remove = button("list-remove-symbolic", gettext("Remove from Playlist"));
        bar_box.append(&close);
        bar_box.append(&count);
        bar_box.append(&playlist);
        bar_box.append(&queue);
        bar_box.append(&next);
        bar_box.append(&remove);
        let bar = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .child(&bar_box)
            .build();
        let selection = Self(Rc::new(Inner {
            choice: RefCell::default(),
            bar,
            count,
            remove,
            playlist,
            rows: RefCell::default(),
            act: RefCell::default(),
            removable: Cell::new(false),
        }));
        let weak = Rc::downgrade(&selection.0);
        close.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                Self(inner).clear();
            }
        });
        for (button, act) in [
            (&selection.0.playlist, Act::AddToPlaylist),
            (&queue, Act::AddToQueue),
            (&next, Act::PlayNext),
            (&selection.0.remove, Act::Remove),
        ] {
            let weak = Rc::downgrade(&selection.0);
            button.connect_clicked(move |_| {
                if let Some(inner) = weak.upgrade() {
                    Self(inner).act(act);
                }
            });
        }
        selection.update();
        selection
    }

    /// The bar, to go under the panel's header.
    pub fn bar(&self) -> &gtk::Revealer {
        &self.0.bar
    }

    /// What the bar's buttons do (with the chosen keys, in order); the
    /// choosing ends after an act.
    pub fn connect_act(&self, act: impl Fn(Act, Vec<i64>) + 'static) {
        self.0.act.replace(Some(Box::new(act)));
    }

    /// Whether the songs can be removed here (an editable playlist).
    pub fn set_removable(&self, removable: bool) {
        self.0.removable.set(removable);
        self.update();
    }

    pub fn is_active(&self) -> bool {
        self.0.choice.borrow().is_active()
    }

    /// The songs shown now, in order (after the list changed).
    pub fn set_order(&self, order: Vec<i64>) {
        self.0.choice.borrow_mut().set_order(order);
        self.0
            .rows
            .borrow_mut()
            .retain(|_, rows| rows.iter().any(|r| r.upgrade().is_some()));
        self.update();
    }

    pub fn clear(&self) {
        self.0.choice.borrow_mut().clear();
        self.update();
    }

    fn act(&self, act: Act) {
        let chosen = self.0.choice.borrow().chosen();
        if chosen.is_empty() {
            return;
        }
        // Taken out while it runs: it may change the list, and so this.
        let callback = self.0.act.take();
        if let Some(callback) = &callback {
            callback(act, chosen);
        }
        if self.0.act.borrow().is_none() {
            self.0.act.replace(callback);
        }
        self.clear();
    }

    /// A song's row: clicks on it choose while choosing (or with Ctrl or
    /// Shift held), and it shows whether it's chosen. Clicks on its
    /// buttons stay theirs.
    pub fn attach(&self, row: &impl IsA<gtk::Widget>, key: i64) {
        let row = row.upcast_ref::<gtk::Widget>();
        {
            let mut rows = self.0.rows.borrow_mut();
            let list = rows.entry(key).or_default();
            list.retain(|r| r.upgrade().is_some());
            list.push(row.downgrade());
        }
        self.mark(row, self.0.choice.borrow().contains(key));
        let click = gtk::GestureClick::builder()
            .button(gdk::BUTTON_PRIMARY)
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        let weak = Rc::downgrade(&self.0);
        let row_ref = row.downgrade();
        click.connect_pressed(move |gesture, n_press, x, y| {
            let (Some(inner), Some(row)) = (weak.upgrade(), row_ref.upgrade()) else {
                return;
            };
            let selection = Self(inner);
            let state = gesture.current_event_state();
            let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
            let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
            if !(ctrl || shift || selection.is_active()) || on_button(&row, x, y) {
                return;
            }
            // The second click of a double-click doesn't play either.
            if n_press == 1 && !selection.0.choice.borrow_mut().click(key, ctrl, shift) {
                return;
            }
            gesture.set_state(gtk::EventSequenceState::Claimed);
            selection.update();
        });
        row.add_controller(click);
    }

    /// Esc ends the choosing, wherever the focus is in the window (a
    /// click on a row leaves it where it was), while the panel shows.
    /// Esc keeps its other uses otherwise: dialogs and the search come
    /// first, and without songs chosen it isn't taken.
    pub fn escape_controller(&self) -> gtk::ShortcutController {
        let weak = Rc::downgrade(&self.0);
        let action = gtk::CallbackAction::new(move |panel, _| match weak.upgrade() {
            Some(inner) if panel.is_mapped() && Self(inner.clone()).is_active() => {
                Self(inner).clear();
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
        let controller = gtk::ShortcutController::new();
        controller.set_scope(gtk::ShortcutScope::Global);
        controller.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("Escape"),
            Some(action),
        ));
        controller
    }

    fn mark(&self, row: &gtk::Widget, chosen: bool) {
        if chosen {
            row.add_css_class("chosen");
        } else {
            row.remove_css_class("chosen");
        }
        row.update_state(&[gtk::accessible::State::Selected(Some(chosen))]);
    }

    /// The bar and the rows' marks, after a change.
    fn update(&self) {
        let choice = self.0.choice.borrow();
        let count = choice.chosen().len() as u32;
        self.0.count.set_label(
            &ngettext("{} Selected", "{} Selected", count).replace("{}", &count.to_string()),
        );
        self.0.bar.set_reveal_child(count > 0);
        self.0.remove.set_visible(self.0.removable.get());
        for (key, rows) in self.0.rows.borrow().iter() {
            let chosen = choice.contains(*key);
            for row in rows.iter().filter_map(glib::WeakRef::upgrade) {
                self.mark(&row, chosen);
            }
        }
    }
}

impl Default for SongSelection {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for SongSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SongSelection")
            .field(&self.0.choice.borrow())
            .finish()
    }
}

/// Whether (x, y) in `row` is on one of its buttons (or a button's child).
fn on_button(row: &gtk::Widget, x: f64, y: f64) -> bool {
    let mut widget = row.pick(x, y, gtk::PickFlags::DEFAULT);
    while let Some(current) = widget {
        if current == *row {
            return false;
        }
        if current.is::<gtk::Button>() || current.is::<gtk::MenuButton>() {
            return true;
        }
        widget = current.parent();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice() -> Choice {
        let mut choice = Choice::default();
        choice.set_order(vec![10, 20, 30, 40, 50]);
        choice
    }

    #[test]
    fn a_plain_click_chooses_only_while_choosing() {
        let mut choice = choice();
        assert!(!choice.click(20, false, false));
        assert!(!choice.is_active());
        assert!(choice.click(20, true, false));
        // Now a plain click chooses, and drops again.
        assert!(choice.click(40, false, false));
        assert_eq!(choice.chosen(), vec![20, 40]);
        assert!(choice.click(20, false, false));
        assert_eq!(choice.chosen(), vec![40]);
    }

    #[test]
    fn shift_chooses_the_songs_from_the_last_one_clicked() {
        let mut choice = choice();
        choice.click(40, true, false);
        assert!(choice.click(20, false, true));
        assert_eq!(choice.chosen(), vec![20, 30, 40]);
        // Without Ctrl the range replaces what was chosen; with it, adds.
        choice.click(50, true, false);
        choice.click(40, false, true);
        assert_eq!(choice.chosen(), vec![40, 50]);
        choice.click(10, true, false);
        choice.click(20, true, true);
        assert_eq!(choice.chosen(), vec![10, 20, 40, 50]);
        // Shift with nothing clicked yet chooses the one song.
        let mut fresh = self::choice();
        assert!(fresh.click(30, false, true));
        assert_eq!(fresh.chosen(), vec![30]);
    }

    #[test]
    fn songs_no_longer_shown_are_dropped() {
        let mut choice = choice();
        choice.click(20, true, false);
        choice.click(30, true, false);
        choice.set_order(vec![10, 30]);
        assert_eq!(choice.chosen(), vec![30]);
        // The anchor went with its song: Shift chooses just the one.
        choice.set_order(vec![10, 20]);
        assert!(!choice.is_active());
        choice.click(10, false, true);
        assert_eq!(choice.chosen(), vec![10]);
    }
}
