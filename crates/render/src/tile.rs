//! Tile keys and the decomposition of a page into a grid of tiles.
//!
//! A tile is always `TILE_SIZE` device pixels on its inner edges; tiles on the
//! right and bottom of a page are smaller so they never extend past the page
//! bounds. Tiles are addressed by `(page, level, col, row)` — the level makes
//! them cacheable across zoom changes.

/// Identifies one tile of one page at one ladder level.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TileKey {
    /// Zero-based page index.
    pub page: u32,
    /// Zoom-ladder level (see [`crate::zoom::zoom_level`]).
    pub level: i32,
    /// Column within the page's tile grid.
    pub col: u32,
    /// Row within the page's tile grid.
    pub row: u32,
}

impl TileKey {
    /// Construct a tile key.
    pub fn new(page: u32, level: i32, col: u32, row: u32) -> Self {
        Self {
            page,
            level,
            col,
            row,
        }
    }
}

/// An axis-aligned rectangle in page-pixel space.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PixelRect {
    /// Left edge, in page pixels.
    pub x: u32,
    /// Top edge, in page pixels.
    pub y: u32,
    /// Width, in page pixels.
    pub w: u32,
    /// Height, in page pixels.
    pub h: u32,
}

/// Number of tile columns and rows needed to cover a page of the given pixel
/// size.
pub fn tile_grid(page_w: u32, page_h: u32, tile_size: u32) -> (u32, u32) {
    let cols = page_w.div_ceil(tile_size);
    let rows = page_h.div_ceil(tile_size);
    (cols.max(1), rows.max(1))
}

/// Pixel rectangle of a tile within the full page image.
///
/// Inner tiles are full `tile_size` squares; edge tiles are clipped to the page
/// so they never read past the page bounds (which would expose garbage from the
/// scratch bitmap).
pub fn tile_pixel_rect(key: TileKey, page_w: u32, page_h: u32, tile_size: u32) -> PixelRect {
    let x = key.col * tile_size;
    let y = key.row * tile_size;
    let w = (page_w.saturating_sub(x)).min(tile_size);
    let h = (page_h.saturating_sub(y)).min(tile_size);
    PixelRect { x, y, w, h }
}

/// All tile keys covering a page, in row-major order.
pub fn page_tiles(page: u32, level: i32, page_w: u32, page_h: u32, tile_size: u32) -> Vec<TileKey> {
    let (cols, rows) = tile_grid(page_w, page_h, tile_size);
    let mut keys = Vec::with_capacity((cols * rows) as usize);
    for row in 0..rows {
        for col in 0..cols {
            keys.push(TileKey::new(page, level, col, row));
        }
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zoom::TILE_SIZE;

    #[test]
    fn grid_for_exact_multiple() {
        // 1024×1024 page, 512 tiles → exactly 2×2.
        assert_eq!(tile_grid(1024, 1024, 512), (2, 2));
    }

    #[test]
    fn grid_rounds_up_partial_tiles() {
        // 600×600 page → 2×2 tiles (the second column/row is partial).
        assert_eq!(tile_grid(600, 600, 512), (2, 2));
    }

    #[test]
    fn inner_tile_is_full_size() {
        let rect = tile_pixel_rect(TileKey::new(0, 0, 0, 0), 2000, 2000, TILE_SIZE);
        assert_eq!(
            rect,
            PixelRect {
                x: 0,
                y: 0,
                w: TILE_SIZE,
                h: TILE_SIZE
            }
        );
    }

    #[test]
    fn edge_tile_is_clipped_to_page() {
        // Bottom-right tile of a 600×600 page is only 88×88.
        let (cols, rows) = tile_grid(600, 600, TILE_SIZE);
        let key = TileKey::new(0, 0, cols - 1, rows - 1);
        let rect = tile_pixel_rect(key, 600, 600, TILE_SIZE);
        assert_eq!(rect.x, (cols - 1) * TILE_SIZE);
        assert_eq!(rect.y, (rows - 1) * TILE_SIZE);
        assert_eq!(rect.w, 600 - (cols - 1) * TILE_SIZE);
        assert_eq!(rect.h, 600 - (rows - 1) * TILE_SIZE);
    }

    #[test]
    fn page_tiles_count_matches_grid() {
        let (cols, rows) = tile_grid(1500, 900, TILE_SIZE);
        let keys = page_tiles(3, 1, 1500, 900, TILE_SIZE);
        assert_eq!(keys.len() as u32, cols * rows);
        assert!(keys.iter().all(|k| k.page == 3 && k.level == 1));
    }
}
