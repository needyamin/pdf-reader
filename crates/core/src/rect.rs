//! Axis-aligned rectangles in PDF page space.
//!
//! PDF page space has its origin at the bottom-left, unlike screen space. This
//! type is deliberately in `core` so that the engine, the renderer and the
//! domain state all agree on which way up a rectangle is — the flip to screen
//! space happens in exactly one place, `pdfreader_render::hit`.
//!
//! Keeping it here rather than in the engine crate is what lets form fields and
//! annotations live in [`crate::store::AppState`]: they carry rectangles, and
//! `core` cannot depend on the engine that produces them.

use serde::{Deserialize, Serialize};

/// A rectangle, in PDF points, with `min <= max` on both axes.
#[derive(Clone, Copy, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct Rect {
    /// Left edge.
    pub min_x: f32,
    /// Bottom edge in PDF space.
    pub min_y: f32,
    /// Right edge.
    pub max_x: f32,
    /// Top edge in PDF space.
    pub max_y: f32,
}

impl Rect {
    /// Build a rect from left, bottom, width and height.
    ///
    /// Negative extents are normalised, so a rect dragged from bottom-right to
    /// top-left still describes the same region.
    pub fn from_xywh(x: f32, y: f32, width: f32, height: f32) -> Self {
        let (dx, dy) = (width.abs(), height.abs());
        Self {
            min_x: if width < 0.0 { x - dx } else { x },
            min_y: if height < 0.0 { y - dy } else { y },
            max_x: if width < 0.0 { x } else { x + dx },
            max_y: if height < 0.0 { y } else { y + dy },
        }
    }

    /// Build a rect from two corners, normalising the order.
    pub fn from_corners(a: (f32, f32), b: (f32, f32)) -> Self {
        Self {
            min_x: a.0.min(b.0),
            min_y: a.1.min(b.1),
            max_x: a.0.max(b.0),
            max_y: a.1.max(b.1),
        }
    }

    /// Width in points.
    pub const fn width(self) -> f32 {
        self.max_x - self.min_x
    }

    /// Height in points.
    pub const fn height(self) -> f32 {
        self.max_y - self.min_y
    }

    /// Area in square points.
    pub const fn area(self) -> f32 {
        self.width() * self.height()
    }

    /// Whether the rect has no extent on either axis.
    ///
    /// Glyph bounding boxes are routinely degenerate, so callers that turn
    /// rects into screen geometry should check this rather than assume.
    pub const fn is_degenerate(self) -> bool {
        self.width() <= 0.0 || self.height() <= 0.0
    }

    /// Whether the rect contains a point.
    pub const fn contains(self, x: f32, y: f32) -> bool {
        x >= self.min_x && x <= self.max_x && y >= self.min_y && y <= self.max_y
    }

    /// Whether two rects overlap. Touching edges do not count.
    pub const fn intersects(self, other: Self) -> bool {
        self.min_x < other.max_x
            && other.min_x < self.max_x
            && self.min_y < other.max_y
            && other.min_y < self.max_y
    }

    /// The overlapping region of two rects, if any.
    pub fn intersection(self, other: Self) -> Option<Self> {
        let r = Self {
            min_x: self.min_x.max(other.min_x),
            min_y: self.min_y.max(other.min_y),
            max_x: self.max_x.min(other.max_x),
            max_y: self.max_y.min(other.max_y),
        };
        (!r.is_degenerate()).then_some(r)
    }

    /// The smallest rect containing both.
    pub fn union(self, other: Self) -> Self {
        Self {
            min_x: self.min_x.min(other.min_x),
            min_y: self.min_y.min(other.min_y),
            max_x: self.max_x.max(other.max_x),
            max_y: self.max_y.max(other.max_y),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_xywh_normalises_negative_extents() {
        let r = Rect::from_xywh(100.0, 100.0, -40.0, -20.0);
        assert_eq!((r.min_x, r.max_x), (60.0, 100.0));
        assert_eq!((r.min_y, r.max_y), (80.0, 100.0));
        assert_eq!(r.width(), 40.0);
        assert_eq!(r.height(), 20.0);
    }

    #[test]
    fn degenerate_rects_are_detected() {
        assert!(Rect::from_xywh(0.0, 0.0, 0.0, 10.0).is_degenerate());
        assert!(Rect::from_xywh(0.0, 0.0, 10.0, -0.0).is_degenerate());
        assert!(!Rect::from_xywh(0.0, 0.0, 1.0, 1.0).is_degenerate());
    }

    #[test]
    fn intersection_rejects_touching_edges() {
        let a = Rect::from_xywh(0.0, 0.0, 10.0, 10.0);
        let b = Rect::from_xywh(10.0, 0.0, 10.0, 10.0);
        assert!(a.intersection(b).is_none());

        let c = Rect::from_xywh(5.0, 5.0, 10.0, 10.0);
        assert_eq!(c.intersection(a), Some(Rect::from_xywh(5.0, 5.0, 5.0, 5.0)));
    }

    #[test]
    fn union_spans_both_rects() {
        let a = Rect::from_xywh(0.0, 0.0, 10.0, 10.0);
        let b = Rect::from_xywh(20.0, 20.0, 5.0, 5.0);
        assert_eq!(a.union(b), Rect::from_xywh(0.0, 0.0, 25.0, 25.0));
    }

    #[test]
    fn containment_and_overlap_agree() {
        let outer = Rect::from_xywh(0.0, 0.0, 100.0, 100.0);
        assert!(outer.contains(50.0, 50.0));
        assert!(!outer.contains(101.0, 50.0));
        assert!(outer.intersects(Rect::from_xywh(50.0, 50.0, 10.0, 10.0)));
        assert!(!outer.intersects(Rect::from_xywh(200.0, 200.0, 10.0, 10.0)));
    }
}
