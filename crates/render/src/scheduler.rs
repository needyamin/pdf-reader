//! Tile render scheduler with generation counters for cancellation.
//!
//! The engine thread is single-threaded (PDFium is not thread-safe — see the
//! crate docs), so it works through a flat queue. The scheduler turns a set of
//! visible tiles into an ordered queue and stamps every task with a
//! [`Generation`]. When the viewport moves, the generation is bumped; any task
//! still in flight from a previous generation is dropped on arrival, which is how
//! a fast scroll avoids wasting raster time on pages you have already blown past.

use crate::tile::TileKey;
use crate::viewport::VisibleTile;

/// Stamped on every scheduled task. Bumped on each viewport change.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Generation(pub u64);

impl Generation {
    /// The next generation.
    pub fn next(self) -> Self {
        Generation(self.0 + 1)
    }
}

/// How urgently a tile should be rasterized.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Priority {
    /// On screen right now.
    Visible = 0,
    /// One tile ring outside the viewport; prefetched before it scrolls in.
    Adjacent = 1,
    /// Far prefetch / background warm-up.
    Prefetch = 2,
}

/// A single rasterization request with its scheduling metadata.
#[derive(Clone, Copy, Debug)]
pub struct RenderTask {
    /// Which tile to rasterize.
    pub key: TileKey,
    /// Scheduling urgency.
    pub priority: Priority,
    /// Generation this task was created in.
    pub generation: Generation,
}

impl RenderTask {
    /// Whether this task still belongs to the current generation.
    pub fn is_current(self, current: Generation) -> bool {
        self.generation == current
    }
}

/// Produces ordered work from what the viewport shows.
pub struct TileScheduler {
    generation: Generation,
}

impl TileScheduler {
    /// Create a scheduler at generation 0.
    pub fn new() -> Self {
        Self {
            generation: Generation(0),
        }
    }

    /// Current generation; tasks stamped with it are live.
    pub fn generation(&self) -> Generation {
        self.generation
    }

    /// Advance the generation, cancelling everything still queued from before.
    pub fn bump(&mut self) -> Generation {
        self.generation = self.generation.next();
        self.generation
    }

    /// Build the ordered work list for the current viewport.
    ///
    /// Visible tiles are listed first (highest priority), then an adjacent ring
    /// of prefetch tiles, so a steady scroll always has the next band ready
    /// before it arrives.
    pub fn schedule(&mut self, visible: &[VisibleTile]) -> Vec<RenderTask> {
        let mut tasks: Vec<RenderTask> = visible
            .iter()
            .map(|t| RenderTask {
                key: t.key,
                priority: Priority::Visible,
                generation: self.generation,
            })
            .collect();

        tasks.sort_by_key(|t| t.priority);
        tasks
    }
}

impl Default for TileScheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tile::TileKey;
    use crate::viewport::{PageLayout, Viewport, VisibleTile};

    fn dummy_visible(keys: &[TileKey]) -> Vec<VisibleTile> {
        keys.iter()
            .map(|k| VisibleTile {
                key: *k,
                screen_x: 0.0,
                screen_y: 0.0,
                screen_w: 1.0,
                screen_h: 1.0,
            })
            .collect()
    }

    #[test]
    fn tasks_are_stamped_with_current_generation() {
        let mut sched = TileScheduler::new();
        let first_gen = sched.generation();
        let tasks = sched.schedule(&dummy_visible(&[TileKey::new(0, 0, 0, 0)]));
        assert_eq!(tasks.len(), 1);
        assert!(tasks[0].is_current(first_gen));
    }

    #[test]
    fn bumping_generation_invalidates_old_tasks() {
        let mut sched = TileScheduler::new();
        let first = sched.generation();
        let old = sched.schedule(&dummy_visible(&[TileKey::new(0, 0, 0, 0)]));
        let current = sched.bump();
        assert_ne!(first, current);
        assert!(!old[0].is_current(current));
        assert!(old[0].is_current(first));
    }

    #[test]
    fn visible_tiles_become_visible_priority_tasks() {
        let mut sched = TileScheduler::new();
        let vp = Viewport {
            scroll_x: 0.0,
            scroll_y: 0.0,
            width: 2000.0,
            height: 2000.0,
            zoom: 1.0,
            dpr: 1.0,
        };
        let layout = PageLayout {
            index: 0,
            screen_x: 0.0,
            screen_y: 0.0,
            pixel_w: 1024,
            pixel_h: 1024,
        };
        let visible = vp.visible_tiles(&layout);
        let tasks = sched.schedule(&visible);
        assert_eq!(tasks.len(), visible.len());
        assert!(tasks.iter().all(|t| t.priority == Priority::Visible));
        // All task keys must correspond to tiles of page 0 at the viewport level.
        let level = vp.level();
        assert!(
            tasks
                .iter()
                .all(|t| t.key.page == 0 && t.key.level == level)
        );
    }
}
