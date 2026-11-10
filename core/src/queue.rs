//! The play queue (CONCEPT.md §6, §7).
//!
//! The queue is separate from the album it was started from; that album is
//! kept as its context. Items added with "Play next" / "Add to queue" form a
//! "Next up" block right after the current track.

use std::collections::HashMap;

use rand::Rng;
use rand::seq::SliceRandom;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// From the album the queue was started with.
    Context,
    /// Added with "Play next" or "Add to queue".
    User,
    /// Mixed in by smart shuffle, or added to keep the music going (§7).
    /// Moves and plays like the album's songs.
    Suggested,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Context => "context",
            Self::User => "user",
            Self::Suggested => "suggested",
        }
    }

    pub fn from_str_lossy(value: &str) -> Self {
        match value {
            "user" => Self::User,
            "suggested" => Self::Suggested,
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

    /// Takes out the upcoming suggested songs (mixing off, or redone).
    pub fn remove_upcoming_suggestions(&mut self) {
        let first_upcoming = self.current.map_or(0, |c| c + 1);
        let removed: Vec<u64> = self.items[first_upcoming.min(self.items.len())..]
            .iter()
            .filter(|i| i.origin == Origin::Suggested)
            .map(|i| i.uid)
            .collect();
        self.items.retain(|i| !removed.contains(&i.uid));
        if let Some(order) = self.unshuffled.as_mut() {
            order.retain(|uid| !removed.contains(uid));
        }
    }

    /// Mixes suggestions into the whole upcoming queue: one after every
    /// `every` album songs (the current one counts), from `pick(seed)` for
    /// the song before it, never inside the "Next up" block (one due there
    /// comes after it). Returns how many were added.
    pub fn mix_throughout(
        &mut self,
        every: usize,
        mut pick: impl FnMut(i64) -> Option<i64>,
    ) -> usize {
        let every = every.max(1);
        let Some(current) = self.current else {
            return 0;
        };
        let mut count = usize::from(self.items[current].origin == Origin::Context);
        let mut added = 0;
        let mut index = current + 1;
        while index < self.items.len() {
            let item = self.items[index];
            match item.origin {
                Origin::Suggested => count = 0,
                Origin::Context => count += 1,
                Origin::User => {}
            }
            let inside_block = item.origin == Origin::User
                && self
                    .items
                    .get(index + 1)
                    .is_some_and(|i| i.origin == Origin::User);
            if count >= every && !inside_block {
                count = 0;
                if let Some(track_id) = pick(item.track_id) {
                    let new = self.new_item(track_id, Origin::Suggested);
                    self.items.insert(index + 1, new);
                    if let Some(order) = self.unshuffled.as_mut() {
                        let at = order
                            .iter()
                            .position(|&uid| uid == item.uid)
                            .map_or(order.len(), |p| p + 1);
                        order.insert(at, new.uid);
                    }
                    added += 1;
                    index += 1;
                }
            }
            index += 1;
        }
        added
    }

    /// Adds a suggested song at the end ("keep playing similar music").
    pub fn append_suggestion(&mut self, track_id: i64) {
        let unshuffled_position = self.unshuffled.as_ref().map(Vec::len);
        let end = self.items.len();
        self.insert_as(&[track_id], Origin::Suggested, end, unshuffled_position);
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
        self.insert_as(tracks, Origin::User, position, unshuffled_position);
    }

    fn insert_as(
        &mut self,
        tracks: &[i64],
        origin: Origin,
        position: usize,
        unshuffled_position: Option<usize>,
    ) {
        let new: Vec<QueueItem> = tracks.iter().map(|&id| self.new_item(id, origin)).collect();
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

    /// The item with `uid`, handed to GStreamer for gapless playback, is
    /// audible now: it becomes current. When the queue changed since the
    /// hand-over ("Play next", shuffle, a move, repeat one), it plays now
    /// all the same, so it moves to right after the current one instead of
    /// the queue jumping there and skipping what lies between. False if
    /// it's no longer queued.
    pub fn start_handed_over(&mut self, uid: u64) -> bool {
        let Some(index) = self.items.iter().position(|i| i.uid == uid) else {
            return false;
        };
        let Some(current) = self.current else {
            self.current = Some(index);
            return true;
        };
        if index == current || self.peek_next(false) == Some(index) {
            self.current = Some(index);
            return true;
        }
        let item = self.items.remove(index);
        let current = if index < current {
            current - 1
        } else {
            current
        };
        let at = current + 1;
        self.items.insert(at, item);
        self.current = Some(at);
        if let Some(order) = self.unshuffled.as_mut() {
            // Follow the item before it, as for a move.
            order.retain(|&u| u != uid);
            let before = self.items[current].uid;
            let at = order
                .iter()
                .position(|&u| u == before)
                .map_or(order.len(), |p| p + 1);
            order.insert(at, uid);
        }
        true
    }

    /// Shuffles the tracks after the current one (keeping the "Next up"
    /// block in order), or restores the original order.
    pub fn set_shuffle(&mut self, shuffle: bool, rng: &mut impl Rng) {
        self.set_shuffle_by(shuffle, None::<fn(i64) -> Option<i64>>, rng);
    }

    /// Like `set_shuffle`; with `album_of`, shuffles whole albums: their
    /// order is random, each plays in order ("Shuffle albums", §7).
    pub fn set_shuffle_by(
        &mut self,
        shuffle: bool,
        album_of: Option<impl Fn(i64) -> Option<i64>>,
        rng: &mut impl Rng,
    ) {
        if shuffle == self.is_shuffled() {
            return;
        }
        if shuffle {
            self.unshuffled = Some(self.items.iter().map(|i| i.uid).collect());
            let start = self.current.map_or(0, |c| c + 1);
            let start = user_block_end(&self.items, start);
            match album_of {
                None => self.items[start..].shuffle(rng),
                Some(album_of) => {
                    let playing = self.current().and_then(|i| album_of(i.track_id));
                    // Runs of the same album, in first-seen order.
                    let mut groups: Vec<(Option<i64>, Vec<QueueItem>)> = Vec::new();
                    for item in self.items.drain(start..) {
                        let album = album_of(item.track_id);
                        match groups
                            .iter_mut()
                            .find(|(a, _)| album.is_some() && *a == album)
                        {
                            Some((_, items)) => items.push(item),
                            None => groups.push((album, vec![item])),
                        }
                    }
                    // The album that's playing finishes first.
                    let first = match groups.first() {
                        Some((album, _)) if album.is_some() && *album == playing => {
                            Some(groups.remove(0))
                        }
                        _ => None,
                    };
                    groups.shuffle(rng);
                    self.items
                        .extend(first.into_iter().chain(groups).flat_map(|(_, items)| items));
                }
            }
        } else if let Some(order) = self.unshuffled.take() {
            let current_uid = self.current().map(|i| i.uid);
            let mut by_uid: HashMap<u64, QueueItem> =
                self.items.iter().map(|i| (i.uid, *i)).collect();
            let mut items = Vec::with_capacity(self.items.len());
            for uid in order {
                if let Some(item) = by_uid.remove(&uid) {
                    items.push(item);
                }
            }
            // Not in the order (shouldn't happen): at the end, as they were.
            items.extend(self.items.iter().filter(|i| by_uid.contains_key(&i.uid)));
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
    /// An item stays in its section: songs added with "Play next" / "Add
    /// to queue" move within the "Next up" block, the album's songs within
    /// the rest, so an album stays together. A drop beyond the section's
    /// edge lands at that edge. Returns false for invalid moves (the
    /// current item stays put) and moves that change nothing.
    pub fn move_item(&mut self, from: usize, to: usize) -> bool {
        let first_upcoming = self.current.map_or(0, |c| c + 1);
        let len = self.items.len();
        if from == to || from < first_upcoming || to < first_upcoming || from >= len || to >= len {
            return false;
        }
        let item = self.items.remove(from);
        // The "Next up" block as it is without the moved item.
        let block_end = user_block_end(&self.items, first_upcoming);
        let to = match item.origin {
            Origin::User => to.clamp(first_upcoming, block_end),
            Origin::Context | Origin::Suggested => to.max(block_end),
        };
        self.items.insert(to, item);
        if to == from {
            return false;
        }

        if let Some(order) = self.unshuffled.as_mut() {
            // Follow the item before the new position, so turning shuffle
            // off keeps the move.
            order.retain(|&uid| uid != item.uid);
            let at = match to.checked_sub(1).map(|i| self.items[i].uid) {
                Some(before) => order
                    .iter()
                    .position(|&uid| uid == before)
                    .map_or(order.len(), |p| p + 1),
                None => 0,
            };
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
        let positions: HashMap<u64, usize> = self
            .unshuffled
            .iter()
            .flatten()
            .enumerate()
            .map(|(position, &uid)| (uid, position))
            .collect();
        let unshuffled_position = |uid: u64| positions.get(&uid).copied();
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
    fn moved_items_stay_in_their_section() {
        let mut queue = album(Some(0));
        queue.add(&[10, 11]);
        // [1*, 10u, 11u, 2, 3, 4, 5]
        // An album track dragged to the top lands at the top of the album.
        assert!(queue.move_item(5, 1));
        assert_eq!(ids(&queue), [1, 10, 11, 4, 2, 3, 5]);
        assert_eq!(queue.items()[3].origin, Origin::Context);
        // Down and back up again: still album, no "Next up" entry.
        assert!(queue.move_item(3, 6));
        assert!(queue.move_item(6, 1));
        assert_eq!(ids(&queue), [1, 10, 11, 4, 2, 3, 5]);
        assert!(
            queue.items()[3..]
                .iter()
                .all(|i| i.origin == Origin::Context)
        );
        // A "Next up" song dragged among the album stays at the block's end.
        assert!(queue.move_item(1, 5));
        assert_eq!(ids(&queue), [1, 11, 10, 4, 2, 3, 5]);
        assert_eq!(queue.items()[2].origin, Origin::User);
        // Dropping where it already is changes nothing.
        assert!(!queue.move_item(2, 6));
        // The current item can't move, and nothing moves before it.
        assert!(!queue.move_item(0, 2));
        assert!(!queue.move_item(3, 0));
    }

    #[test]
    fn shuffling_albums_keeps_each_album_in_order() {
        let mut queue = Queue::new();
        // Three albums of three tracks: 1x, 2x, 3x.
        let tracks = [11, 12, 13, 21, 22, 23, 31, 32, 33];
        queue.play_context(&tracks, Some(0), None, false, &mut rng());
        queue.set_shuffle_by(true, Some(|id: i64| Some(id / 10)), &mut rng());
        let order = ids(&queue);
        assert_eq!(order[0], 11);
        // Each album's tracks stay together and in order.
        for album in [1, 2, 3] {
            let positions: Vec<usize> = order
                .iter()
                .enumerate()
                .filter(|(_, id)| **id / 10 == album)
                .map(|(i, _)| i)
                .collect();
            assert!(positions.windows(2).all(|w| w[1] == w[0] + 1), "{order:?}");
        }
        queue.set_shuffle(false, &mut rng());
        assert_eq!(ids(&queue), tracks);
    }

    #[test]
    fn suggestions_mix_in_after_next_up_and_at_the_end() {
        let mut queue = album(Some(0));
        queue.add(&[10, 11]);
        // Due right after the current song, but "Next up" stays together.
        let mut picks = vec![99];
        queue.mix_throughout(1, |_| picks.pop());
        assert_eq!(ids(&queue), [1, 10, 11, 99, 2, 3, 4, 5]);
        assert_eq!(queue.items()[3].origin, Origin::Suggested);
        queue.append_suggestion(98);
        assert_eq!(*ids(&queue).last().unwrap(), 98);
        // Suggestions move like album songs, never into "Next up".
        assert!(!queue.move_item(3, 1));
        assert_eq!(ids(&queue)[1..4], [10, 11, 99]);
        // A new "Add to queue" song still joins the block.
        queue.add(&[12]);
        assert_eq!(ids(&queue)[1..5], [10, 11, 12, 99]);
    }

    #[test]
    fn a_song_handed_over_after_the_queue_changed_plays_without_skipping() {
        // [1*, 2, 3, 4, 5]: 2 was handed over, then "Play next" 10.
        let mut queue = album(Some(0));
        let handed = queue.items()[1].uid;
        queue.play_next(&[10]);
        assert!(queue.start_handed_over(handed));
        assert_eq!(ids(&queue), [1, 2, 10, 3, 4, 5]);
        assert_eq!(queue.current_index(), Some(1));

        // Shuffle turned on meanwhile: the handed-over song plays next
        // and the rest stays upcoming.
        let mut queue = album(Some(0));
        let handed = queue.items()[1].uid;
        queue.set_shuffle(true, &mut rng());
        assert!(queue.start_handed_over(handed));
        assert_eq!(queue.current_index(), Some(1));
        assert_eq!(queue.current().unwrap().track_id, 2);
        let mut upcoming = ids(&queue)[2..].to_vec();
        upcoming.sort();
        assert_eq!(upcoming, [3, 4, 5]);
        queue.set_shuffle(false, &mut rng());
        assert_eq!(ids(&queue), [1, 2, 3, 4, 5]);
        assert_eq!(queue.current_index(), Some(1));

        // Unchanged, and with repeat all from the end: the usual next one.
        let mut queue = album(Some(4));
        queue.set_repeat(RepeatMode::All);
        let first = queue.items()[0].uid;
        assert!(queue.start_handed_over(first));
        assert_eq!(ids(&queue), [1, 2, 3, 4, 5]);
        assert_eq!(queue.current_index(), Some(0));
        assert!(!queue.start_handed_over(12345));
    }

    #[test]
    fn mixing_throughout_fills_the_queue_and_can_be_undone() {
        let mut queue = Queue::new();
        let tracks: Vec<i64> = (1..=10).collect();
        queue.play_context(&tracks, Some(0), None, false, &mut rng());
        let mut next = 100;
        let added = queue.mix_throughout(3, |_seed| {
            next += 1;
            Some(next)
        });
        // The current song counts: after 3, 6 and 9.
        assert_eq!(added, 3);
        assert_eq!(ids(&queue), [1, 2, 3, 101, 4, 5, 6, 102, 7, 8, 9, 103, 10]);
        // Redone, the same positions; nothing doubles up.
        queue.remove_upcoming_suggestions();
        assert_eq!(ids(&queue), tracks);
        assert_eq!(queue.mix_throughout(3, |_| Some(200)), 3);
        queue.remove_upcoming_suggestions();
        queue.set_shuffle(true, &mut rng());
        queue.mix_throughout(2, |_| Some(300));
        queue.remove_upcoming_suggestions();
        queue.set_shuffle(false, &mut rng());
        assert_eq!(ids(&queue), tracks);
    }

    #[test]
    fn without_next_up_album_tracks_reorder_freely() {
        let mut queue = album(Some(0));
        // [1*, 2, 3, 4, 5]
        assert!(queue.move_item(4, 1));
        assert_eq!(ids(&queue), [1, 5, 2, 3, 4]);
        assert!(queue.items().iter().all(|i| i.origin == Origin::Context));
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
