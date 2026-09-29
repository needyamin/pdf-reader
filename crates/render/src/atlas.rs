//! Atlas slot allocator.
//!
//! The GPU atlas is an `ATLAS_SIZE × ATLAS_SIZE` `Bgra8Unorm` texture holding
//! `ATLAS_SLOTS` fixed `TILE_SIZE` slots in a `16 × 16` grid. A fixed grid has
//! no fragmentation and a trivial free list, which is exactly what the plan
//! calls for; the actual `wgpu` texture is created and uploaded elsewhere. This
//! module owns only the bookkeeping.

use std::collections::HashSet;

use crate::zoom::{ATLAS_SIZE, ATLAS_SLOTS, TILE_SIZE};

/// Handle to an allocated atlas slot.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Slot(u32);

impl Slot {
    /// Column of the slot within the atlas grid.
    pub fn col(self) -> u32 {
        self.0 % (ATLAS_SIZE / TILE_SIZE)
    }

    /// Row of the slot within the atlas grid.
    pub fn row(self) -> u32 {
        self.0 / (ATLAS_SIZE / TILE_SIZE)
    }

    /// Top-left pixel of the slot within the atlas texture.
    pub fn origin(self) -> (u32, u32) {
        (self.col() * TILE_SIZE, self.row() * TILE_SIZE)
    }
}

/// Fixed-grid allocator over the atlas.
pub struct TileAtlas {
    free: Vec<u32>,
    used: HashSet<u32>,
}

impl TileAtlas {
    /// Create an allocator with every slot free.
    pub fn new() -> Self {
        let mut free: Vec<u32> = (0..ATLAS_SLOTS).collect();
        free.reverse(); // hand out low indices first
        Self {
            free,
            used: HashSet::new(),
        }
    }

    /// Total number of slots.
    pub fn capacity(&self) -> u32 {
        ATLAS_SLOTS
    }

    /// Number of currently allocated slots.
    pub fn used_count(&self) -> usize {
        self.used.len()
    }

    /// Allocate a slot, or `None` if the atlas is full.
    pub fn alloc(&mut self) -> Option<Slot> {
        let idx = self.free.pop()?;
        self.used.insert(idx);
        Some(Slot(idx))
    }

    /// Return a slot to the free list.
    pub fn free(&mut self, slot: Slot) {
        if self.used.remove(&slot.0) {
            self.free.push(slot.0);
        }
    }
}

impl Default for TileAtlas {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_is_256() {
        let atlas = TileAtlas::new();
        assert_eq!(atlas.capacity(), 256);
        assert_eq!(atlas.used_count(), 0);
    }

    #[test]
    fn alloc_returns_distinct_slots() {
        let mut atlas = TileAtlas::new();
        let a = atlas.alloc().unwrap();
        let b = atlas.alloc().unwrap();
        assert_ne!(a, b);
        assert_eq!(atlas.used_count(), 2);
    }

    #[test]
    fn free_returns_slot_to_pool() {
        let mut atlas = TileAtlas::new();
        let a = atlas.alloc().unwrap();
        atlas.free(a);
        assert_eq!(atlas.used_count(), 0);
        // The slot is reusable.
        let again = atlas.alloc().unwrap();
        assert_eq!(again, a);
    }

    #[test]
    fn exhausts_and_recovers() {
        let mut atlas = TileAtlas::new();
        let mut slots = Vec::new();
        for _ in 0..256 {
            slots.push(atlas.alloc().expect("slot"));
        }
        assert!(atlas.alloc().is_none(), "atlas should be full");

        // Free half; allocation should succeed again.
        for s in slots.drain(..128) {
            atlas.free(s);
        }
        assert_eq!(atlas.used_count(), 128);
        assert!(atlas.alloc().is_some());
    }

    #[test]
    fn slot_geometry_is_in_bounds() {
        let slot = Slot(255);
        let (x, y) = slot.origin();
        // Last slot is the bottom-right corner of a 16×16 grid.
        assert_eq!(slot.col(), 15);
        assert_eq!(slot.row(), 15);
        assert_eq!((x, y), (15 * TILE_SIZE, 15 * TILE_SIZE));
        assert!(x + TILE_SIZE <= ATLAS_SIZE);
        assert!(y + TILE_SIZE <= ATLAS_SIZE);
    }
}
