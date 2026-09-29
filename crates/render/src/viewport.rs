//! Viewport math: which tiles are on screen and where they land.
//!
//! The renderer is deliberately unaware of whole-document layout. The app supplies
//! a [`PageLayout`] — one page positioned in screen space at the current scale —
//! and this module answers "which tiles of this page intersect the viewport, and
//! what rectangle should they be drawn into?"

use crate::tile::{PixelRect, TileKey, tile_grid, tile_pixel_rect};
use crate::zoom::{TILE_SIZE, zoom_level};

/// A single page positioned in screen (viewport) space at the current scale.
///
/// `pixel_w`/`pixel_h` already include the current `zoom × dpr`, i.e. they are
/// the page's size in device pixels at the active ladder level.
#[derive(Clone, Copy, Debug)]
pub struct PageLayout {
    /// Zero-based page index.
    pub index: u32,
    /// Left edge of the page in viewport coordinates.
    pub screen_x: f32,
    /// Top edge of the page in viewport coordinates.
    pub screen_y: f32,
    /// Rendered page width, in device pixels.
    pub pixel_w: u32,
    /// Rendered page height, in device pixels.
    pub pixel_h: u32,
}

/// The visible portion of the document, in viewport coordinates.
#[derive(Clone, Copy, Debug)]
pub struct Viewport {
    /// Horizontal scroll offset (viewport left edge in document space).
    pub scroll_x: f32,
    /// Vertical scroll offset (viewport top edge in document space).
    pub scroll_y: f32,
    /// Viewport width, in device pixels.
    pub width: f32,
    /// Viewport height, in device pixels.
    pub height: f32,
    /// User zoom factor (PDF points → CSS px).
    pub zoom: f32,
    /// Device pixel ratio.
    pub dpr: f32,
}

/// A visible tile plus the rectangle it should be composited into.
#[derive(Clone, Copy, Debug)]
pub struct VisibleTile {
    /// Tile identity.
    pub key: TileKey,
    /// Destination left edge in viewport coordinates.
    pub screen_x: f32,
    /// Destination top edge in viewport coordinates.
    pub screen_y: f32,
    /// Destination width in viewport coordinates.
    pub screen_w: f32,
    /// Destination height in viewport coordinates.
    pub screen_h: f32,
}

impl Viewport {
    /// Ladder level for the current zoom/dpr.
    pub fn level(&self) -> i32 {
        zoom_level(self.zoom, self.dpr)
    }

    /// Tiles of `layout` that intersect the viewport, with their screen rects.
    ///
    /// `pixel_w`/`pixel_h` on the layout are expected to be the page size at this
    /// same level, so tiles line up with the page image.
    pub fn visible_tiles(&self, layout: &PageLayout) -> Vec<VisibleTile> {
        let level = self.level();
        let (cols, rows) = tile_grid(layout.pixel_w, layout.pixel_h, TILE_SIZE);

        let mut out = Vec::with_capacity((cols * rows) as usize);

        for row in 0..rows {
            for col in 0..cols {
                let key = TileKey::new(layout.index, level, col, row);
                let rect: PixelRect =
                    tile_pixel_rect(key, layout.pixel_w, layout.pixel_h, TILE_SIZE);

                // Screen position of the tile's top-left, relative to the
                // viewport's scroll origin.
                let screen_x = layout.screen_x + rect.x as f32 - self.scroll_x;
                let screen_y = layout.screen_y + rect.y as f32 - self.scroll_y;

                // Cull tiles fully outside the viewport.
                if screen_x + rect.w as f32 <= 0.0
                    || screen_y + rect.h as f32 <= 0.0
                    || screen_x >= self.width
                    || screen_y >= self.height
                {
                    continue;
                }

                out.push(VisibleTile {
                    key,
                    screen_x,
                    screen_y,
                    screen_w: rect.w as f32,
                    screen_h: rect.h as f32,
                });
            }
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(pixel_w: u32, pixel_h: u32) -> PageLayout {
        PageLayout {
            index: 0,
            screen_x: 0.0,
            screen_y: 0.0,
            pixel_w,
            pixel_h,
        }
    }

    #[test]
    fn all_tiles_visible_when_page_fits() {
        let vp = Viewport {
            scroll_x: 0.0,
            scroll_y: 0.0,
            width: 2000.0,
            height: 2000.0,
            zoom: 1.0,
            dpr: 1.0,
        };
        let tiles = vp.visible_tiles(&layout(1024, 1024));
        // 1024/512 = 2×2 grid.
        assert_eq!(tiles.len(), 4);
    }

    #[test]
    fn offscreen_page_yields_no_tiles() {
        let vp = Viewport {
            scroll_x: 0.0,
            scroll_y: 5000.0, // scrolled well past a page at origin
            width: 2000.0,
            height: 2000.0,
            zoom: 1.0,
            dpr: 1.0,
        };
        let tiles = vp.visible_tiles(&layout(1024, 1024));
        assert!(tiles.is_empty());
    }

    #[test]
    fn only_overlapping_tiles_returned() {
        // Viewport is 600×600 at scroll origin; the page is offset 100px
        // down/right. A tile's screen position is 100 + tile_origin, so:
        //   - column 1 starts at x = 100 + 512 = 612 ≥ 600  → culled
        //   - row 1 starts at y    = 100 + 512 = 612 ≥ 600  → culled
        // Only the top-left tile (col 0, row 0) survives culling.
        let vp = Viewport {
            scroll_x: 0.0,
            scroll_y: 0.0,
            width: 600.0,
            height: 600.0,
            zoom: 1.0,
            dpr: 1.0,
        };
        let l = PageLayout {
            index: 0,
            screen_x: 100.0,
            screen_y: 100.0,
            pixel_w: 1500,
            pixel_h: 900,
        };
        let tiles = vp.visible_tiles(&l);
        assert_eq!(tiles.len(), 1);
        let t = &tiles[0];
        assert_eq!((t.key.col, t.key.row), (0, 0));
        // The returned tile must be inside the viewport.
        assert!(t.screen_x < 600.0);
        assert!(t.screen_y < 600.0);
    }

    #[test]
    fn screen_rect_matches_tile_origin() {
        let vp = Viewport {
            scroll_x: 50.0,
            scroll_y: 50.0,
            width: 2000.0,
            height: 2000.0,
            zoom: 1.0,
            dpr: 1.0,
        };
        let l = PageLayout {
            index: 2,
            screen_x: 0.0,
            screen_y: 0.0,
            pixel_w: 1024,
            pixel_h: 1024,
        };
        let tiles = vp.visible_tiles(&l);
        let second_row = tiles
            .iter()
            .find(|t| t.key.row == 1 && t.key.col == 0)
            .unwrap();
        // Scrolled by 50, so the second row (y = 512) appears at 512 − 50 = 462.
        assert!((second_row.screen_y - 462.0).abs() < 1e-3);
        assert_eq!(second_row.key.page, 2);
    }
}
