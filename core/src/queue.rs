//! The play queue (CONCEPT.md §6, §7).
//!
//! The queue is separate from the album it was started from; that album is
//! kept as its context. Items added with "Play next" / "Add to queue" form a
//! "Next up" block right after the current track.

use rand::Rng;
use rand::seq::SliceRandom;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// From the album the queue was started with.
    Context,
    /// Added with "Play next" or "Add to queue".
    User,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Context => "context",
            Self::User => "user",
        }
    }

    pub fn from_str_lossy(value: &str) -> Self {
        match value {
            "user" => Self::User,
            _ => Self::Context,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RepeatMode {
    #[default]
    Off,
    /// Repeat the whole queue.
    All,
    One,
}

impl RepeatMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::All => "all",
            Self::One => "one",
        }
    }

    pub fn from_str_lossy(value: &str) -> Self {
        match value {
            "all" => Self::All,
            "one" => Self::One,
            _ => Self::Off,
        }
    }

    /// Off → All → One → Off, as the repeat button cycles.
    pub fn cycle(self) -> Self {
        match self {
            Self::Off => Self::All,
            Self::All => Self::One,
            Self::One => Self::Off,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueItem {
    /// Unique within this queue, so the same track can be queued twice.
    pub uid: u64,
    pub track_id: i64,
    pub origin: Origin,
}

/// What is saved to the database (§6.2 persistence).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueSnapshot {
    pub items: Vec<SnapshotItem>,
    pub current: Option<usize>,
    pub position_ms: u64,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub context_album: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotItem {
    pub track_id: i64,
    pub origin: Origin,
    /// Position in the order to restore when shuffle is turned off.
    pub unshuffled_position: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct Queue {
    items: Vec<QueueItem>,
    /// The order before shuffling, as uids. `Some` while shuffled.
    unshuffled: Option<Vec<u64>>,
    current: Option<usize>,
    repeat: RepeatMode,
    context_album: Option<i64>,
    next_uid: u64,
}

impl Queue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn items(&self) -> &[QueueItem] {
        &self.items
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn current_index(&self) -> Option<usize> {
        self.current
    }

    pub fn current(&self) -> Option<&QueueItem> {
        self.current.and_then(|i| self.items.get(i))
    }

    pub fn is_shuffled(&self) -> bool {
        self.unshuffled.is_some()
    }

    pub fn repeat(&self) -> RepeatMode {
        self.repeat
    }

    pub fn set_repeat(&mut self, repeat: RepeatMode) {
        self.repeat = repeat;
    }

    pub fn context_album(&self) -> Option<i64> {
        self.context_album
    }

    fn new_item(&mut self, track_id: i64, origin: Origin) -> QueueItem {
        self.next_uid += 1;
        QueueItem {
            uid: self.next_uid,
            track_id,
            origin,
        }
    }

    /// Replaces the queue with `tracks` (an album) and makes `start` current.
    /// With `shuffle` and no `start`, a random track goes first.
    pub fn play_context(
        &mut self,
        tracks: &[i64],
        start: Option<usize>,
        context_album: Option<i64>,
        shuffle: bool,
        rng: &mut impl Rng,
    ) {
        self.items = tracks
            .iter()
            .map(|&id| self.new_item(id, Origin::Context))
            .collect();
        self.unshuffled = None;
        self.context_album = context_album;
        self.current = match start {
            Some(index) if index < self.items.len() => Some(index),
            _ if self.items.is_empty() => None,
            _ if shuffle => Some(rng.random_range(0..self.items.len())),
            _ => Some(0),
        };
        if shuffle {
            self.set_shuffle(true, rng);
        }
    }

    /// Inserts tracks right after the current one.
    pub fn play_next(&mut self, tracks: &[i64]) {
        let position = self.current.map_or(0, |c| c + 1);
        let unshuffled_position = self.unshuffled_position_after_current(false);
        self.insert(tracks, position, unshuffled_position);
    }

    /// Appends tracks to the "Next up" block, before the rest of the context.
    pub fn add(&mut self, tracks: &[i64]) {
        let position = match self.current {
            Some(current) => user_block_end(&self.items, current + 1),
            None => self.items.len(),
        };
        let unshuffled_position = self.unshuffled_position_after_current(true);
        self.insert(tracks, position, unshuffled_position);
    }

    fn unshuffled_position_after_current(&self, after_user_block: bool) -> Option<usize> {
        let order = self.unshuffled.as_ref()?;
        let Some(current) = self.current() else {
            return Some(if after_user_block { order.len() } else { 0 });
        };
        let start = order.iter().position(|&uid| uid == current.uid)? + 1;
        if !after_user_block {
            return Some(start);
        }
        let origin = |uid: u64| self.items.iter().find(|i| i.uid == uid).map(|i| i.origin);
        Some(
            start
                + order[start..]
                    .iter()
                    .take_while(|&&uid| origin(uid) == Some(Origin::User))
                    .count(),
        )
    }

    fn insert(&mut self, tracks: &[i64], position: usize, unshuffled_position: Option<usize>) {
        let new: Vec<QueueItem> = tracks
            .iter()
            .map(|&id| self.new_item(id, Origin::User))
            .collect();
        if let (Some(order), Some(at)) = (self.unshuffled.as_mut(), unshuffled_position) {
            let at = at.min(order.len());
            order.splice(at..at, new.iter().map(|i| i.uid));
        }
        let position = position.min(self.items.len());
        self.items.splice(position..position, new.iter().copied());
        match self.current {
            Some(current) if current >= position => self.current = Some(current + new.len()),
            None if !new.is_empty() => self.current = Some(position),
            _ => {}
        }
    }

    /// The index that plays after the current one. `auto` is true when the
    /// current track ended by itself (repeat-one repeats only then).
    pub fn peek_next(&self, auto: bool) -> Option<usize> {
        if self.items.is_empty() {
            return None;
        }
        let Some(current) = self.current else {
            return Some(0);
        };
        if auto && self.repeat == RepeatMode::One {
            return Some(current);
        }
        if current + 1 < self.items.len() {
            Some(current + 1)
        } else if self.repeat != RepeatMode::Off {
            Some(0)
        } else {
            None
        }
    }

    pub fn advance(&mut self, auto: bool) -> Option<QueueItem> {
        let next = self.peek_next(auto)?;
        self.current = Some(next);
        self.current().copied()
    }

    pub fn peek_previous(&self) -> Option<usize> {
        let current = self.current?;
        if current > 0 {
            Some(current - 1)
        } else if self.repeat != RepeatMode::Off && !self.items.is_empty() {
            Some(self.items.len() - 1)
        } else {
            None
        }
    }

    pub fn go_previous(&mut self) -> Option<QueueItem> {
        let previous = self.peek_previous()?;
        self.current = Some(previous);
        self.current().copied()
    }

    pub fn jump(&mut self, index: usize) -> Option<QueueItem> {
        if index >= self.items.len() {
            return None;
        }
        self.current = Some(index);
        self.current().copied()
    }

    /// Makes the item with `uid` current; false if it's no longer queued.
    pub fn jump_to_uid(&mut self, uid: u64) -> bool {
        match self.items.iter().position(|i| i.uid == uid) {
            Some(index) => {
                self.current = Some(index);
                true
            }
            None => false,
        }
    }

    /// Shuffles the tracks after the current one (keeping the "Next up"
    /// block in order), or restores the original order.
    pub fn set_shuffle(&mut self, shuffle: bool, rng: &mut impl Rng) {
        if shuffle == self.is_shuffled() {
            return;
        }
        if shuffle {
            self.unshuffled = Some(self.items.iter().map(|i| i.uid).collect());
            let start = self.current.map_or(0, |c| c + 1);
            let start = user_block_end(&self.items, start);
            self.items[start..].shuffle(rng);
        } else if let Some(order) = self.unshuffled.take() {
            let current_uid = self.current().map(|i| i.uid);
            let mut items = Vec::with_capacity(self.items.len());
            for uid in order {
                if let Some(item) = self.items.iter().find(|i| i.uid == uid) {
                    items.push(*item);
                }
            }
            for item in &self.items {
                if !items.iter().any(|i| i.uid == item.uid) {
                    items.push(*item);
                }
            }
            self.items = items;
            self.current = current_uid.and_then(|uid| self.items.iter().position(|i| i.uid == uid));
        }
    }

    /// Removes an upcoming or past item. The current item can't be removed.
    pub fn remove(&mut self, index: usize) -> bool {
        if index >= self.items.len() || Some(index) == self.current {
            return false;
        }
        let item = self.items.remove(index);
        if let Some(order) = self.unshuffled.as_mut() {
            order.retain(|&uid| uid != item.uid);
        }
        if let Some(current) = self.current
            && index < current
        {
            self.current = Some(current - 1);
        }
        true
    }

    /// Moves an upcoming item to another upcoming position (drag & drop).
    /// Items dropped into or right after the "Next up" block become part of
    /// it; items dropped among the album's tracks become context again.
    /// Returns false for invalid moves (the current item stays put).
    pub fn move_item(&mut self, from: usize, to: usize) -> bool {
        let first_upcoming = self.current.map_or(0, |c| c + 1);
        let len = self.items.len();
        if from == to || from < first_upcoming || to < first_upcoming || from >= len || to >= len {
            return false;
        }
        let mut item = self.items.remove(from);
        // The "Next up" block as it is without the moved item.
        let block_end = user_block_end(&self.items, first_upcoming);
        item.origin = if to <= block_end {
            Origin::User
        } else {
            Origin::Context
        };
        self.items.insert(to, item);

        if let Some(order) = self.unshuffled.as_mut() {
            // Follow the item before the new position, so turning shuffle
            // off keeps the move.
            order.retain(|&uid| uid != item.uid);
            let before = self.items[to - 1].uid;
            let at = order
                .iter()
                .position(|&uid| uid == before)
                .map_or(order.len(), |p| p + 1);
            order.insert(at, item.uid);
        }
        true
    }

    pub fn clear_upcoming(&mut self) {
        let keep = self.current.map_or(0, |c| c + 1);
        let removed: Vec<u64> = self.items.drain(keep..).map(|i| i.uid).collect();
        if let Some(order) = self.unshuffled.as_mut() {
            order.retain(|uid| !removed.contains(uid));
        }
    }

    pub fn snapshot(&self, position_ms: u64) -> QueueSnapshot {
        let unshuffled_position = |uid: u64| {
            self.unshuffled
                .as_ref()
                .and_then(|order| order.iter().position(|&u| u == uid))
        };
        QueueSnapshot {
            items: self
                .items
                .iter()
                .map(|item| SnapshotItem {
                    track_id: item.track_id,
                    origin: item.origin,
                    unshuffled_position: unshuffled_position(item.uid),
                })
                .collect(),
            current: self.current,
            position_ms,
            shuffle: self.is_shuffled(),
            repeat: self.repeat,
            context_album: self.context_album,
        }
    }

    /// Rebuilds a queue, skipping tracks for which `exists` is false.
    /// Returns the queue and how many items were skipped.
    pub fn restore(snapshot: &QueueSnapshot, exists: impl Fn(i64) -> bool) -> (Self, usize) {
        let mut queue = Self {
            repeat: snapshot.repeat,
            context_album: snapshot.context_album,
            ..Self::default()
        };
        let mut skipped = 0;
        let mut current = None;
        let mut unshuffled: Vec<(usize, u64)> = Vec::new();
        for (index, saved) in snapshot.items.iter().enumerate() {
            if !exists(saved.track_id) {
                skipped += 1;
                continue;
            }
            let item = queue.new_item(saved.track_id, saved.origin);
            if snapshot.current.is_some_and(|c| index >= c) && current.is_none() {
                current = Some(queue.items.len());
            }
            if let Some(position) = saved.unshuffled_position {
                unshuffled.push((position, item.uid));
            }
            queue.items.push(item);
        }
        queue.current = current.or_else(|| (!queue.items.is_empty()).then_some(0));
        if snapshot.shuffle {
            unshuffled.sort_unstable();
            queue.unshuffled = Some(unshuffled.into_iter().map(|(_, uid)| uid).collect());
        }
        (queue, skipped)
    }
}

/// The index after the run of `User` items starting at `start`.
fn user_block_end(items: &[QueueItem], start: usize) -> usize {
    let start = start.min(items.len());
    start
        + items[start..]
            .iter()
            .take_while(|i| i.origin == Origin::User)
            .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    fn rng() -> StdRng {
        StdRng::seed_from_u64(7)
    }

    fn ids(queue: &Queue) -> Vec<i64> {
        queue.items().iter().map(|i| i.track_id).collect()
    }

    fn album(start: Option<usize>) -> Queue {
        let mut queue = Queue::new();
        queue.play_context(&[1, 2, 3, 4, 5], start, Some(9), false, &mut rng());
        queue
    }

    #[test]
    fn play_next_and_add_form_next_up() {
        let mut queue = album(Some(1));
        queue.add(&[10]);
        queue.add(&[11]);
        queue.play_next(&[12]);
        assert_eq!(ids(&queue), [1, 2, 12, 10, 11, 3, 4, 5]);
        assert_eq!(queue.current().unwrap().track_id, 2);
    }

    #[test]
    fn advancing_and_repeat() {
        let mut queue = album(Some(4));
        assert_eq!(queue.peek_next(true), None);
        queue.set_repeat(RepeatMode::All);
        assert_eq!(queue.peek_next(true), Some(0));
        queue.set_repeat(RepeatMode::One);
        assert_eq!(queue.peek_next(true), Some(4));
        assert_eq!(queue.peek_next(false), Some(0));
        queue.set_repeat(RepeatMode::Off);
        assert_eq!(queue.go_previous().unwrap().track_id, 4);
        assert_eq!(queue.advance(true).unwrap().track_id, 5);
    }

    #[test]
    fn shuffle_keeps_current_and_restores_order() {
        let mut queue = Queue::new();
        let tracks: Vec<i64> = (1..=20).collect();
        queue.play_context(&tracks, Some(3), None, false, &mut rng());
        queue.play_next(&[100]);
        queue.set_shuffle(true, &mut rng());

        assert_eq!(queue.current().unwrap().track_id, 4);
        assert_eq!(&ids(&queue)[..5], [1, 2, 3, 4, 100]);
        let mut sorted = ids(&queue);
        sorted.sort_unstable();
        let mut expected = tracks.clone();
        expected.push(100);
        assert_eq!(sorted, expected);
        assert_ne!(ids(&queue)[5..], tracks[4..]);

        queue.add(&[200]);
        queue.advance(false);
        queue.set_shuffle(false, &mut rng());
        let mut expected = vec![1, 2, 3, 4, 100, 200];
        expected.extend(5..=20);
        assert_eq!(ids(&queue), expected);
    }

    #[test]
    fn shuffled_start_picks_random_first_track() {
        let mut queue = Queue::new();
        queue.play_context(&[1, 2, 3, 4], None, None, true, &mut rng());
        assert!(queue.is_shuffled());
        assert_eq!(queue.items().len(), 4);
        assert!(queue.current().is_some());
    }

    #[test]
    fn remove_and_clear() {
        let mut queue = album(Some(2));
        assert!(!queue.remove(2));
        assert!(queue.remove(0));
        assert_eq!(queue.current().unwrap().track_id, 3);
        queue.clear_upcoming();
        assert_eq!(ids(&queue), [2, 3]);
    }

    #[test]
    fn moving_items_updates_next_up() {
        let mut queue = album(Some(0));
        queue.add(&[10]);
        // [1*, 10u, 2, 3, 4, 5]
        assert!(queue.move_item(4, 1));
        assert_eq!(ids(&queue), [1, 4, 10, 2, 3, 5]);
        assert_eq!(queue.items()[1].origin, Origin::User);
        // Back among the album tracks: context again.
        assert!(queue.move_item(1, 4));
        assert_eq!(ids(&queue), [1, 10, 2, 3, 4, 5]);
        assert_eq!(queue.items()[4].origin, Origin::Context);
        // The current item can't move, and nothing moves before it.
        assert!(!queue.move_item(0, 2));
        assert!(!queue.move_item(3, 0));
    }

    #[test]
    fn moves_survive_turning_shuffle_off() {
        let mut queue = album(Some(0));
        queue.set_shuffle(true, &mut rng());
        let last = queue.items().len() - 1;
        let moved = queue.items()[last].track_id;
        assert!(queue.move_item(last, 1));
        queue.set_shuffle(false, &mut rng());
        assert_eq!(ids(&queue)[1], moved);
        assert_eq!(queue.items().len(), 5);
    }

    #[test]
    fn adding_to_an_empty_queue_sets_current() {
        let mut queue = Queue::new();
        queue.add(&[1, 2]);
        assert_eq!(queue.current().unwrap().track_id, 1);
    }

    #[test]
    fn snapshot_round_trip_skips_missing_tracks() {
        let mut queue = Queue::new();
        queue.play_context(&[1, 2, 3, 4, 5, 6], Some(2), Some(9), true, &mut rng());
        queue.set_repeat(RepeatMode::All);
        let snapshot = queue.snapshot(500);

        let (restored, skipped) = Queue::restore(&snapshot, |_| true);
        assert_eq!(skipped, 0);
        assert_eq!(restored.snapshot(500), snapshot);

        let current_track = queue.current().unwrap().track_id;
        let (mut restored, skipped) = Queue::restore(&snapshot, |id| id != current_track);
        assert_eq!(skipped, 1);
        assert_eq!(restored.items().len(), 5);
        assert!(restored.current().is_some());
        restored.set_shuffle(false, &mut rng());
        let mut expected = vec![1, 2, 3, 4, 5, 6];
        expected.retain(|&id| id != current_track);
        assert_eq!(ids(&restored), expected);
    }
}
