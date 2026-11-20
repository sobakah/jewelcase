//! The album and artist grids' tiles gliding to their new places when the
//! number of columns changes (a panel opens beside the grid, the window is
//! resized), instead of jumping there in one frame.
//!
//! When the columns change, each tile's place on screen is noted, with an
//! image of it. Then, while the grid's rows are already laid out for the
//! new columns, every tile is drawn on its way from where it was to where
//! it is now. Tiles that weren't on screen before come from where they
//! would have been (off screen, mostly below); tiles that are no longer
//! built (they moved off screen) are drawn from their image on the way to
//! their new place. Only the drawing moves: clicks go to the new places.

use std::collections::{HashMap, HashSet};

use gtk::{gdk, graphene};

/// What a tile shows, to follow it from one layout to the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TileKey {
    /// A library item.
    Local(u32),
    /// A section's item.
    Section(usize, u32),
    /// A section's header (a whole row).
    Header(usize),
}

impl TileKey {
    /// The run of tiles it's in (the library, or a section) and its index
    /// there; none for a header.
    pub fn place(self) -> Option<(usize, u32)> {
        match self {
            Self::Local(index) => Some((0, index)),
            Self::Section(section, index) => Some((section + 1, index)),
            Self::Header(_) => None,
        }
    }
}

/// The grid's layout: its columns' count and width, and the rows' height.
#[derive(Debug, Clone, Copy, Default)]
pub struct Lattice {
    pub columns: u32,
    pub column: f32,
    pub pitch: f32,
}

impl Lattice {
    /// Where the tile at `index` is, from where the one at `anchor_index`
    /// is (both in the same run).
    pub fn place(&self, anchor: graphene::Point, anchor_index: u32, index: u32) -> graphene::Point {
        let columns = self.columns.max(1);
        let rows = (index / columns) as f32 - (anchor_index / columns) as f32;
        let across = (index % columns) as f32 - (anchor_index % columns) as f32;
        graphene::Point::new(
            anchor.x() + across * self.column,
            anchor.y() + rows * self.pitch,
        )
    }
}

/// A tile as it was drawn when the columns changed.
#[derive(Debug, Clone)]
struct Before {
    at: graphene::Point,
    image: Option<gdk::Paintable>,
    size: (f32, f32),
}

/// A tile built now: where it's laid out (in the grid's coordinates).
#[derive(Debug, Clone, Copy)]
pub struct Built {
    pub key: TileKey,
    pub at: graphene::Point,
}

#[derive(Debug, Default)]
pub struct Reflow {
    before: HashMap<TileKey, Before>,
    /// For tiles not on screen before: one noted tile of each run (the
    /// first), and the layout then.
    anchors: HashMap<usize, (u32, graphene::Point)>,
    old: Lattice,
    /// 0 where the tiles were, 1 where they are; while a reflow runs.
    progress: Option<f64>,
}

impl Reflow {
    pub fn running(&self) -> bool {
        self.progress.is_some()
    }

    fn ease(&self) -> f32 {
        self.progress.unwrap_or(1.0) as f32
    }

    /// Notes where the tiles are drawn now (`built`, with an image and a
    /// size each, and those still on their way out from a reflow before)
    /// and starts a new reflow from there; `old` is the layout they're in.
    pub fn start(
        &mut self,
        built: &[(Built, Option<gdk::Paintable>, (f32, f32))],
        old: Lattice,
        new: Lattice,
    ) {
        let mut before = HashMap::new();
        let keys: HashSet<TileKey> = built.iter().map(|(b, _, _)| b.key).collect();
        // Still leaving from the reflow before: from where they are now.
        if self.running() {
            let anchors = anchors(built.iter().map(|(b, _, _)| *b));
            for (key, gone) in &self.before {
                if keys.contains(key) {
                    continue;
                }
                if let (Some(at), _) = self.leaving(*key, gone, &anchors, new) {
                    before.insert(
                        *key,
                        Before {
                            at,
                            image: gone.image.clone(),
                            size: gone.size,
                        },
                    );
                }
            }
        }
        for (tile, image, size) in built {
            let (at, _) = self.drawn(tile.key, tile.at);
            before.insert(
                tile.key,
                Before {
                    at,
                    image: image.clone(),
                    size: *size,
                },
            );
        }
        let mut anchors: HashMap<usize, (u32, graphene::Point)> = HashMap::new();
        for (key, tile) in &before {
            if let Some((run, index)) = key.place() {
                let first = anchors.entry(run).or_insert((index, tile.at));
                if index < first.0 {
                    *first = (index, tile.at);
                }
            }
        }
        self.before = before;
        self.anchors = anchors;
        self.old = old;
        self.progress = Some(0.0);
    }

    pub fn set_progress(&mut self, progress: f64) {
        if self.running() {
            self.progress = Some(progress);
        }
    }

    pub fn finish(&mut self) {
        self.before.clear();
        self.anchors.clear();
        self.progress = None;
    }

    /// Where the tile `key`, laid out at `at`, is drawn now, and how
    /// opaque (one coming from nowhere fades in).
    pub fn drawn(&self, key: TileKey, at: graphene::Point) -> (graphene::Point, f32) {
        if !self.running() {
            return (at, 1.0);
        }
        let from = self.before.get(&key).map(|b| b.at).or_else(|| {
            let (run, index) = key.place()?;
            let (anchor_index, anchor) = *self.anchors.get(&run)?;
            Some(self.old.place(anchor, anchor_index, index))
        });
        let e = self.ease();
        match from {
            Some(from) => (lerp(from, at, e), 1.0),
            None => (at, e),
        }
    }

    /// The tiles noted before that aren't built now, each with its image
    /// and where to draw it (with its opacity), on the way to where it
    /// would be in `new`, laid out from the `built` tiles.
    pub fn leaving_tiles(
        &self,
        built: &[Built],
        new: Lattice,
    ) -> Vec<(gdk::Paintable, graphene::Rect, f32)> {
        if !self.running() {
            return Vec::new();
        }
        // Rows keep their height.
        let new = Lattice {
            pitch: self.old.pitch,
            ..new
        };
        let keys: HashSet<TileKey> = built.iter().map(|b| b.key).collect();
        let anchors = anchors(built.iter().copied());
        self.before
            .iter()
            .filter(|(key, _)| !keys.contains(key))
            .filter_map(|(key, gone)| {
                let image = gone.image.clone()?;
                let (at, opacity) = self.leaving(*key, gone, &anchors, new);
                let at = at?;
                let rect = graphene::Rect::new(at.x(), at.y(), gone.size.0, gone.size.1);
                Some((image, rect, opacity))
            })
            .collect()
    }

    /// Where a tile no longer built is drawn now: on its way to its place
    /// in `new` (from a built tile of its run), or fading where it was.
    fn leaving(
        &self,
        key: TileKey,
        gone: &Before,
        anchors: &HashMap<usize, (u32, graphene::Point)>,
        new: Lattice,
    ) -> (Option<graphene::Point>, f32) {
        let e = self.ease();
        let to = key.place().and_then(|(run, index)| {
            let (anchor_index, anchor) = *anchors.get(&run)?;
            Some(new.place(anchor, anchor_index, index))
        });
        match to {
            Some(to) => (Some(lerp(gone.at, to, e)), 1.0),
            None => (Some(gone.at), 1.0 - e),
        }
    }
}

/// One built tile of each run: where the others are laid out from.
fn anchors(built: impl Iterator<Item = Built>) -> HashMap<usize, (u32, graphene::Point)> {
    let mut anchors = HashMap::new();
    for tile in built {
        if let Some((run, index)) = tile.key.place() {
            anchors.entry(run).or_insert((index, tile.at));
        }
    }
    anchors
}

fn lerp(from: graphene::Point, to: graphene::Point, e: f32) -> graphene::Point {
    graphene::Point::new(
        from.x() + (to.x() - from.x()) * e,
        from.y() + (to.y() - from.y()) * e,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(x: f32, y: f32) -> graphene::Point {
        graphene::Point::new(x, y)
    }

    fn built(key: TileKey, x: f32, y: f32) -> (Built, Option<gdk::Paintable>, (f32, f32)) {
        (
            Built {
                key,
                at: point(x, y),
            },
            None,
            (100.0, 140.0),
        )
    }

    const OLD: Lattice = Lattice {
        columns: 4,
        column: 100.0,
        pitch: 140.0,
    };
    const NEW: Lattice = Lattice {
        columns: 2,
        column: 100.0,
        pitch: 140.0,
    };

    /// A tile starts where it was drawn and ends where it's laid out; one
    /// that wasn't on screen comes from where it would have been.
    #[test]
    fn tiles_move_from_their_old_place_to_the_new() {
        let mut reflow = Reflow::default();
        // Four columns: items 0..4 in the first row.
        let tiles: Vec<_> = (0..4)
            .map(|i| built(TileKey::Local(i), i as f32 * 100.0, 0.0))
            .collect();
        reflow.start(&tiles, OLD, NEW);
        // Item 3 now at the start of the second row of two columns.
        let new = point(100.0, 140.0);
        assert_eq!(reflow.drawn(TileKey::Local(3), new).0, point(300.0, 0.0));
        reflow.set_progress(0.5);
        assert_eq!(reflow.drawn(TileKey::Local(3), new).0, point(200.0, 70.0));
        reflow.set_progress(1.0);
        assert_eq!(reflow.drawn(TileKey::Local(3), new).0, new);
        // Item 6 wasn't built: it was in the second row, third column.
        reflow.set_progress(0.0);
        assert_eq!(
            reflow.drawn(TileKey::Local(6), point(0.0, 420.0)).0,
            point(200.0, 140.0)
        );
        // A header wasn't on screen: it fades in.
        assert_eq!(reflow.drawn(TileKey::Header(0), point(0.0, 0.0)).1, 0.0);
        reflow.finish();
        assert_eq!(
            reflow.drawn(TileKey::Local(3), new),
            (new, 1.0),
            "after it, tiles are where they're laid out"
        );
    }

    /// A reflow that starts while one runs starts where the tiles are
    /// drawn, not where they're laid out.
    #[test]
    fn a_new_reflow_starts_where_the_tiles_are() {
        let mut reflow = Reflow::default();
        reflow.start(&[built(TileKey::Local(1), 100.0, 0.0)], OLD, NEW);
        reflow.set_progress(0.5);
        // Laid out at (300, 0) now, half way there from (100, 0).
        reflow.start(&[built(TileKey::Local(1), 300.0, 0.0)], NEW, OLD);
        assert_eq!(
            reflow.drawn(TileKey::Local(1), point(0.0, 0.0)).0,
            point(200.0, 0.0)
        );
    }
}
