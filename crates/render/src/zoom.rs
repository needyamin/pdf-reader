//! Zoom ladder shared by the renderer and the UI.
//!
//! Tiles are produced on a geometric ladder so that scrolling and zooming reuse
//! cached bitmaps instead of re-rasterizing. Each step of two levels doubles the
//! scale, so the step size is √2 — zoom feels continuous while the set of
//! distinct cache levels stays small.

/// Edge length of every tile, in device pixels.
pub const TILE_SIZE: u32 = 512;
/// Edge length of the atlas texture, in device pixels (256 tile slots).
pub const ATLAS_SIZE: u32 = 8192;
/// Number of tile slots in the atlas: (ATLAS_SIZE / TILE_SIZE)².
pub const ATLAS_SLOTS: u32 = (ATLAS_SIZE / TILE_SIZE) * (ATLAS_SIZE / TILE_SIZE);

/// Minimum supported zoom factor (matches `core::MIN_ZOOM`).
pub const MIN_ZOOM: f32 = 0.1;
/// Maximum supported zoom factor (matches `core::MAX_ZOOM`).
pub const MAX_ZOOM: f32 = 32.0;

/// Clamp a zoom factor to the range the engine and atlas support.
pub fn clamp_zoom(zoom: f32) -> f32 {
    if !zoom.is_finite() || zoom <= 0.0 {
        return 1.0;
    }
    zoom.clamp(MIN_ZOOM, MAX_ZOOM)
}

/// Map a `(zoom × dpr)` product to the nearest ladder level.
///
/// Level 0 is exactly 1 device pixel per PDF point. Level `n` has scale
/// `2^(n/2)`, so the step between adjacent levels is √2.
pub fn zoom_level(zoom: f32, dpr: f32) -> i32 {
    let product = (clamp_zoom(zoom) * dpr.max(f32::MIN_POSITIVE)).max(f32::MIN_POSITIVE);
    (product.log2() * 2.0).round() as i32
}

/// Scale (device pixels per PDF point) for a ladder level.
pub fn level_scale(level: i32) -> f32 {
    // 2^(level/2): level +2 → 2×, level −2 → 0.5×, level 0 → 1×.
    2f32.powf(level as f32 / 2.0)
}

/// The ladder level whose scale is closest to `scale`, together with that
/// level's actual scale. Used to pick a cached level to show while a sharper
/// one is still rasterizing.
pub fn nearest_level(scale: f32) -> (i32, f32) {
    let level = (scale.max(f32::MIN_POSITIVE).log2() * 2.0).round() as i32;
    (level, level_scale(level))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_zero_is_one_to_one() {
        assert_eq!(zoom_level(1.0, 1.0), 0);
        assert!((level_scale(0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn ladder_is_sqrt2_step() {
        // A 2× zoom at dpr 1 should land on level 2.
        assert_eq!(zoom_level(2.0, 1.0), 2);
        assert!((level_scale(2) - 2.0).abs() < 1e-6);

        // √2 zoom should round to level 1.
        assert_eq!(zoom_level(2.0f32.sqrt(), 1.0), 1);
        assert!((level_scale(1) - 2.0f32.sqrt()).abs() < 1e-6);
    }

    #[test]
    fn dpr_shifts_level() {
        // 1× zoom at 2× dpr is the same device scale as 2× zoom at 1× dpr.
        assert_eq!(zoom_level(1.0, 2.0), zoom_level(2.0, 1.0));
    }

    #[test]
    fn zoom_is_clamped() {
        assert_eq!(clamp_zoom(0.01), MIN_ZOOM);
        assert_eq!(clamp_zoom(100.0), MAX_ZOOM);
        assert_eq!(clamp_zoom(-5.0), 1.0);
        assert_eq!(clamp_zoom(f32::NAN), 1.0);
    }

    #[test]
    fn atlas_has_256_slots() {
        assert_eq!(ATLAS_SLOTS, 256);
        assert_eq!(ATLAS_SIZE / TILE_SIZE, 16);
    }
}
