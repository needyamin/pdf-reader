//! Document-level domain types.
//!
//! Everything here is pure data with no dependency on the PDF engine or the UI,
//! so it can be unit tested, serialized for session restore, and reasoned about
//! in isolation.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Identity of an open document. Stable for the lifetime of the process.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct DocumentId(u64);

impl DocumentId {
    /// Wrap a raw counter value into an id.
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// Return the raw counter value.
    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// Page rotation, expressed in clockwise quarter turns.
///
/// Rotation lives in the domain model (rather than being a render-time flag)
/// because it has to be baked into layout, tile cache keys, save and export.
/// Treating it as a display-only transform is what caused the Electron version
/// to lose annotations on rotate.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub enum Rotation {
    /// No rotation.
    #[default]
    None,
    /// 90 degrees clockwise.
    Cw90,
    /// 180 degrees.
    Cw180,
    /// 270 degrees clockwise (90 counter-clockwise).
    Cw270,
}

impl Rotation {
    /// Number of clockwise quarter turns, 0..=3.
    pub const fn quarter_turns(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Cw90 => 1,
            Self::Cw180 => 2,
            Self::Cw270 => 3,
        }
    }

    /// Build a rotation from a possibly out-of-range quarter-turn count.
    pub fn from_quarter_turns(turns: i32) -> Self {
        match turns.rem_euclid(4) {
            0 => Self::None,
            1 => Self::Cw90,
            2 => Self::Cw180,
            _ => Self::Cw270,
        }
    }

    /// Rotate 90 degrees clockwise.
    pub fn rotate_cw(self) -> Self {
        Self::from_quarter_turns(i32::from(self.quarter_turns()) + 1)
    }

    /// Rotate 90 degrees counter-clockwise.
    pub fn rotate_ccw(self) -> Self {
        Self::from_quarter_turns(i32::from(self.quarter_turns()) + 3)
    }

    /// Whether the rotation swaps width and height.
    pub const fn is_transposed(self) -> bool {
        self.quarter_turns() & 1 == 1
    }

    /// Rotation in degrees, clockwise positive: 0, 90, 180 or 270.
    ///
    /// Returned as `u16` because 270 does not fit in the `u8` that
    /// [`Self::quarter_turns`] returns — computing degrees from quarter turns
    /// in a `u8` overflows (and panics in debug builds) at 270 degrees.
    pub const fn degrees(self) -> u16 {
        match self {
            Self::None => 0,
            Self::Cw90 => 90,
            Self::Cw180 => 180,
            Self::Cw270 => 270,
        }
    }

    /// Rotation in radians, clockwise positive.
    pub fn radians(self) -> f32 {
        std::f32::consts::FRAC_PI_2 * f32::from(self.quarter_turns())
    }
}

/// Intrinsic page size in PDF points (1/72 of an inch), before rotation or zoom.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct PageGeometry {
    /// Unrotated width in points.
    pub width_pt: f32,
    /// Unrotated height in points.
    pub height_pt: f32,
}

impl PageGeometry {
    /// US Letter, 8.5 x 11 inches.
    pub const LETTER: Self = Self {
        width_pt: 612.0,
        height_pt: 792.0,
    };

    /// A4, 210 x 297 mm.
    pub const A4: Self = Self {
        width_pt: 595.28,
        height_pt: 841.89,
    };

    /// Width and height after applying a rotation.
    ///
    /// Returns `(width, height)` in points, swapping the pair when the rotation
    /// is a quarter or three-quarter turn.
    pub fn oriented(self, rotation: Rotation) -> (f32, f32) {
        if rotation.is_transposed() {
            (self.height_pt, self.width_pt)
        } else {
            (self.width_pt, self.height_pt)
        }
    }
}

/// A node in the document outline (bookmarks tree).
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct OutlineNode {
    /// Display title.
    pub title: String,
    /// Zero-based destination page, when the destination resolves to a page.
    pub page: Option<u32>,
    /// Nested children.
    pub children: Vec<OutlineNode>,
}

/// The document outline. Empty when the PDF has none.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct Outline {
    /// Top-level nodes.
    pub root: Vec<OutlineNode>,
}

impl Outline {
    /// Whether the document has any outline entries.
    pub fn is_empty(&self) -> bool {
        self.root.is_empty()
    }

    /// Total number of nodes at every level.
    ///
    /// Recursion is depth-capped: outlines built by the engine are already
    /// capped at ingestion, this is defense in depth against any other source.
    pub fn len(&self) -> usize {
        const MAX_DEPTH: usize = 64;
        fn count(nodes: &[OutlineNode], depth: usize) -> usize {
            if depth >= MAX_DEPTH {
                return nodes.len();
            }
            nodes
                .iter()
                .map(|n| 1 + count(&n.children, depth + 1))
                .sum()
        }
        count(&self.root, 0)
    }
}

/// An opened document and the metadata needed to lay it out.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Document {
    /// Process-unique id.
    pub id: DocumentId,
    /// Path on disk.
    pub path: PathBuf,
    /// Display name, usually the file stem.
    pub title: String,
    /// One entry per page, in page order.
    pub pages: Vec<PageGeometry>,
    /// Whether the file required a password to open.
    pub encrypted: bool,
    /// Bookmarks tree, if any.
    pub outline: Outline,
}

impl Document {
    /// Number of pages.
    pub fn page_count(&self) -> u32 {
        self.pages.len() as u32
    }

    /// Geometry of a page, or `None` when out of range.
    pub fn page(&self, index: u32) -> Option<PageGeometry> {
        self.pages.get(index as usize).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::Rotation;

    /// 270 degrees used to be computed as `quarter_turns() * 90` in a `u8`,
    /// which overflows at 270 and panics in debug builds. `degrees()` must
    /// return the full value for every rotation.
    #[test]
    fn rotation_degrees_cover_every_turn_without_overflow() {
        assert_eq!(Rotation::None.degrees(), 0);
        assert_eq!(Rotation::Cw90.degrees(), 90);
        assert_eq!(Rotation::Cw180.degrees(), 180);
        assert_eq!(Rotation::Cw270.degrees(), 270);
    }

    #[test]
    fn rotating_four_times_returns_to_the_start() {
        let mut rotation = Rotation::None;
        for expected in [90, 180, 270, 0] {
            rotation = rotation.rotate_cw();
            assert_eq!(rotation.degrees(), expected);
        }
    }
}
