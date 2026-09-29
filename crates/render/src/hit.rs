//! Screen-to-page hit testing and the inverse of the page layout transform.
//!
//! The renderer knows how to place a page *on* screen: [`PageBox`] gives the
//! page's position in document layout space and the scale converts points to
//! pixels. Nothing in the codebase could answer the opposite question — "which
//! page is under this pointer, and where on that page is it?" — which is what
//! form-field overlays, annotation dragging and text selection all need.
//!
//! This module owns both directions, so the PDF y-flip and the rotation
//! transposition live in exactly one place.
//!
//! # Coordinate spaces
//!
//! * **PDF space** — points, y *up*, origin at the page's bottom-left. This is
//!   what the engine reports for glyph boxes and annotation rects.
//! * **Layout space** — CSS pixels, y *down*, origin at the top-left of the
//!   scrollable content. [`PageBox`] coordinates are in this space.
//! * **Screen space** — layout space plus the canvas origin. egui pointers are
//!   reported here. The shell converts by subtracting `ui.min_rect().min`;
//!   this module never sees it.
//!
//! Rotation is applied *before* the flip: a quarter-turn swaps the page's width
//! and height, so the transform depends on the unrotated `PageGeometry`.

use pdfreader_core::{PageGeometry, Rect, Rotation};

/// A page positioned in document layout space.
///
/// Width and height are CSS pixels at the current zoom, already accounting for
/// rotation — i.e. a quarter-turned page reports a transposed size.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PageBox {
    /// Zero-based page index.
    pub index: u32,
    /// Left edge in layout space.
    pub x: f32,
    /// Top edge in layout space.
    pub y: f32,
    /// Width in CSS pixels.
    pub w: f32,
    /// Height in CSS pixels.
    pub h: f32,
}

impl PageBox {
    /// Build a box from a position and size.
    pub const fn new(index: u32, x: f32, y: f32, w: f32, h: f32) -> Self {
        Self {
            index,
            x,
            y,
            w,
            h,
        }
    }

    /// Whether a layout-space point falls inside the page.
    pub const fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }
}

/// A rectangle in layout space: CSS pixels, y down, origin top-left.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct ScreenRect {
    /// Left edge in layout space.
    pub x: f32,
    /// Top edge in layout space.
    pub y: f32,
    /// Width in CSS pixels.
    pub w: f32,
    /// Height in CSS pixels.
    pub h: f32,
}

impl ScreenRect {
    /// Build from a position and size.
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// Whether a layout-space point falls inside.
    pub const fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }
}

/// One page plus everything needed to convert between its spaces.
///
/// Built from a [`PageBox`] during layout; the canvas keeps a `Vec` of these for
/// the pages currently on screen and feeds pointer positions to it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PageSpace {
    /// Zero-based page index.
    pub index: u32,
    /// Top-left corner of the page in layout space, in CSS pixels.
    pub origin: (f32, f32),
    /// On-screen size in CSS pixels, after rotation.
    pub size: (f32, f32),
    /// Intrinsic, unrotated page size.
    pub geom: PageGeometry,
    /// Rotation baked into the display.
    pub rotation: Rotation,
    /// CSS pixels per PDF point.
    pub zoom: f32,
}

impl PageSpace {
    /// Build a page space from its layout box and geometry.
    pub const fn new(box_: PageBox, geom: PageGeometry, rotation: Rotation, zoom: f32) -> Self {
        Self {
            index: box_.index,
            origin: (box_.x, box_.y),
            size: (box_.w, box_.h),
            geom,
            rotation,
            zoom,
        }
    }

    /// The layout box this page space was built from.
    pub const fn box_(self) -> PageBox {
        PageBox {
            index: self.index,
            x: self.origin.0,
            y: self.origin.1,
            w: self.size.0,
            h: self.size.1,
        }
    }

    /// Convert a layout-space point to unrotated PDF space.
    ///
    /// `p` is in layout space: for an egui pointer, subtract the canvas origin
    /// (`ui.min_rect().min`) first. Returns `(x, y)` in points with y up. The
    /// result can fall outside the page when the pointer is on the margin.
    pub fn to_page(self, p: (f32, f32)) -> (f32, f32) {
        // Points measured within the *rotated* image, top-left origin.
        let u = (p.0 - self.origin.0) / self.zoom;
        let v = (p.1 - self.origin.1) / self.zoom;
        let (w, h) = (self.geom.width_pt, self.geom.height_pt);

        // Undo the rotation, keeping `v` measured from the top.
        let (ux, vy) = match self.rotation.quarter_turns() {
            0 => (u, v),
            1 => (v, h - u),
            2 => (w - u, h - v),
            _ => (w - v, u),
        };

        // Flip into PDF's bottom-left origin.
        (ux, h - vy)
    }

    /// Convert an unrotated PDF-space point back to layout space.
    ///
    /// Inverse of [`Self::to_page`].
    pub fn to_screen_point(self, p: (f32, f32)) -> (f32, f32) {
        let (w, h) = (self.geom.width_pt, self.geom.height_pt);
        let (x, y) = p;

        // Rotate into the displayed image, measuring `v` from the top.
        let (u, v) = match self.rotation.quarter_turns() {
            0 => (x, h - y),
            1 => (y, x),
            2 => (w - x, y),
            _ => (h - y, w - x),
        };

        (
            self.origin.0 + u * self.zoom,
            self.origin.1 + v * self.zoom,
        )
    }

    /// Convert a PDF-space rect to the axis-aligned layout-space rect that
    /// bounds it.
    ///
    /// A rect is axis-aligned in PDF space, so under a quarter turn it is still
    /// axis-aligned on screen and the bounding box is exact rather than loose.
    pub fn to_screen(self, rect: Rect) -> ScreenRect {
        let corners = [
            self.to_screen_point((rect.min_x, rect.min_y)),
            self.to_screen_point((rect.max_x, rect.min_y)),
            self.to_screen_point((rect.min_x, rect.max_y)),
            self.to_screen_point((rect.max_x, rect.max_y)),
        ];

        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for (x, y) in corners {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }

        ScreenRect {
            x: min_x,
            y: min_y,
            w: max_x - min_x,
            h: max_y - min_y,
        }
    }

    /// Whether a layout-space point is over this page.
    pub const fn contains(self, p: (f32, f32)) -> bool {
        p.0 >= self.origin.0
            && p.0 <= self.origin.0 + self.size.0
            && p.1 >= self.origin.1
            && p.1 <= self.origin.1 + self.size.1
    }
}

/// Find the page under a layout-space point.
///
/// Returns the last match so that the frontmost page wins when pages overlap.
/// `pages` is expected in draw order.
pub fn hit_page(pages: &[PageSpace], p: (f32, f32)) -> Option<PageSpace> {
    pages.iter().rev().copied().find(|page| page.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    const A4: PageGeometry = PageGeometry {
        width_pt: 595.28,
        height_pt: 841.89,
    };

    fn page(rotation: Rotation) -> PageSpace {
        // Laid out at (100, 50) at 2x zoom, so an A4 page is 1190.56 x 1683.78
        // CSS pixels (or transposed under a quarter turn).
        let (w, h) = A4.oriented(rotation);
        PageSpace::new(
            PageBox::new(0, 100.0, 50.0, w * 2.0, h * 2.0),
            A4,
            rotation,
            2.0,
        )
    }

    fn all_rotations() -> [Rotation; 4] {
        [
            Rotation::None,
            Rotation::Cw90,
            Rotation::Cw180,
            Rotation::Cw270,
        ]
    }

    /// The whole point of centralising the transform: every PDF point must
    /// survive a round trip, at every rotation.
    #[test]
    fn page_point_round_trips_at_every_rotation() {
        for rotation in all_rotations() {
            let space = page(rotation);
            for (x, y) in [
                (0.0, 0.0),
                (A4.width_pt, A4.height_pt),
                (123.4, 567.8),
                (A4.width_pt, 0.0),
                (0.0, A4.height_pt),
            ] {
                let screen = space.to_screen_point((x, y));
                let back = space.to_page(screen);
                assert!(
                    (back.0 - x).abs() < 1e-3 && (back.1 - y).abs() < 1e-3,
                    "rotation {:?}: ({x}, {y}) -> {screen:?} -> {back:?}",
                    rotation
                );
            }
        }
    }

    /// Corner mapping is the part that is easy to get wrong, so pin it down
    /// explicitly rather than only relying on round trips.
    #[test]
    fn corners_land_where_rotation_says_they_should() {
        // No rotation: PDF bottom-left is the bottom-left on screen.
        let space = page(Rotation::None);
        let bl = space.to_screen_point((0.0, 0.0));
        assert!((bl.0 - 100.0).abs() < 1e-3);
        // A4 is 841.89pt tall; at 2x that is 1683.78px below y = 50.
        assert!((bl.1 - (50.0 + 841.89 * 2.0)).abs() < 1e-3);

        // Quarter turn clockwise: the PDF bottom-left goes to the screen
        // top-left, and the page is transposed (H wide, W tall).
        let space = page(Rotation::Cw90);
        let bl = space.to_screen_point((0.0, 0.0));
        assert!((bl.0 - 100.0).abs() < 1e-3);
        assert!((bl.1 - 50.0).abs() < 1e-3);
        let tr = space.to_screen_point((A4.width_pt, A4.height_pt));
        assert!((tr.0 - (100.0 + 841.89 * 2.0)).abs() < 1e-3);
        assert!((tr.1 - (50.0 + 595.28 * 2.0)).abs() < 1e-3);

        // Half turn: both corners invert.
        let space = page(Rotation::Cw180);
        let bl = space.to_screen_point((0.0, 0.0));
        assert!((bl.0 - (100.0 + 595.28 * 2.0)).abs() < 1e-3);
        assert!((bl.1 - 50.0).abs() < 1e-3);

        // Three-quarter turn: the PDF top-right goes to the screen top-left.
        let space = page(Rotation::Cw270);
        let tr = space.to_screen_point((A4.width_pt, A4.height_pt));
        assert!((tr.0 - 100.0).abs() < 1e-3);
        assert!((tr.1 - 50.0).abs() < 1e-3);
    }

    /// A rect stays the same size in points regardless of rotation, but its
    /// screen footprint is transposed under a quarter turn.
    #[test]
    fn rect_bounds_are_transposed_under_a_quarter_turn() {
        let rect = Rect::from_xywh(10.0, 20.0, 100.0, 50.0);

        let flat = page(Rotation::None).to_screen(rect);
        assert!((flat.w - 200.0).abs() < 1e-3);
        assert!((flat.h - 100.0).abs() < 1e-3);

        let turned = page(Rotation::Cw90).to_screen(rect);
        assert!((turned.w - 100.0).abs() < 1e-3);
        assert!((turned.h - 200.0).abs() < 1e-3);
    }

    /// The rect's own area must agree with the point transform: the top-left of
    /// the screen bounds is the transformed top-left of the PDF rect.
    #[test]
    fn rect_and_point_transforms_agree() {
        for rotation in all_rotations() {
            let space = page(rotation);
            let rect = Rect::from_xywh(40.0, 60.0, 200.0, 80.0);
            let screen = space.to_screen(rect);
            // Under rotation the "top-left" of the bounds is whichever corner
            // rotation put there, so compare via the inverse instead: every
            // corner of the screen bounds must map back inside the rect.
            for corner in [
                (screen.x, screen.y),
                (screen.x + screen.w, screen.y),
                (screen.x, screen.y + screen.h),
                (screen.x + screen.w, screen.y + screen.h),
            ] {
                let (px, py) = space.to_page(corner);
                assert!(
                    px >= rect.min_x - 1e-2
                        && px <= rect.max_x + 1e-2
                        && py >= rect.min_y - 1e-2
                        && py <= rect.max_y + 1e-2,
                    "rotation {:?}: corner {corner:?} -> ({px}, {py})",
                    rotation
                );
            }
        }
    }

    #[test]
    fn hit_page_finds_the_page_under_a_point() {
        let pages = vec![page(Rotation::None)];
        assert!(hit_page(&pages, (200.0, 200.0)).is_some());
        // Outside the page's box entirely.
        assert!(hit_page(&pages, (0.0, 0.0)).is_none());
        assert!(hit_page(&pages, (5000.0, 5000.0)).is_none());
    }

    #[test]
    fn hit_page_prefers_the_frontmost_page() {
        let mut front = page(Rotation::None);
        front.index = 1;
        let pages = vec![page(Rotation::None), front];
        // Overlapping boxes: the later (frontmost) one must win.
        assert_eq!(hit_page(&pages, (200.0, 200.0)).unwrap().index, 1);
    }

}
