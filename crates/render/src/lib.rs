//! Page layout, tile ladder, atlas allocation and render scheduling.
//!
//! This crate is the math and bookkeeping half of the rendering engine. It is
//! deliberately GPU- and I/O-free: it decides *which* tiles to rasterize, *where*
//! they go on screen, and *which* atlas slot holds them. The `wgpu` texture, the
//! PDFium raster calls and the egui paint callback live in the application crate
//! that wires this to real hardware.

pub mod atlas;
pub mod hit;
pub mod scheduler;
pub mod tile;
pub mod viewport;
pub mod zoom;

pub use atlas::{Slot, TileAtlas};
pub use hit::{PageBox, PageSpace, ScreenRect, hit_page};
pub use scheduler::{Generation, Priority, RenderTask, TileScheduler};
pub use tile::{PixelRect, TileKey, page_tiles, tile_grid, tile_pixel_rect};
pub use viewport::{PageLayout, Viewport, VisibleTile};
pub use zoom::{
    ATLAS_SIZE, ATLAS_SLOTS, MAX_ZOOM, MIN_ZOOM, TILE_SIZE, clamp_zoom, level_scale, nearest_level,
    zoom_level,
};
