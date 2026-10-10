//! Tiling must agree with a full-page render, pixel for pixel.
//!
//! Tiles are what the screen shows, and they are produced by rendering one
//! region at a time. That region is chosen through the transform matrix — the
//! mechanism PDFium leaves available once form-data rendering is off — so an
//! error in the matrix means every tile shows the wrong part of the page while
//! still looking plausible in isolation. Comparing a tile against the matching
//! crop of a full-page render is what catches that.
//!
//! Fixtures live under `target/test-pdfs`, produced by
//! `tools/generate_fixtures.py`, so the test skips cleanly when they are absent.

use std::path::PathBuf;
use std::sync::Mutex;

/// Serialises these tests against each other.
///
/// `Pdfium::bind_to_library` loads the platform library and builds a bindings
/// table, and the test harness runs a binary's tests on parallel threads.
/// Binding from two threads at once has been seen to fail outright — roughly one
/// run in twenty — so only one test binds or renders at a time. The cost is
/// nothing: the whole file runs in a few hundredths of a second.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// Take the serialisation lock, ignoring poisoning.
///
/// A test that panicked while holding it has already reported its own failure;
/// failing the others too would only hide which one broke.
fn one_at_a_time() -> std::sync::MutexGuard<'static, ()> {
    ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

use pdfreader_core::Rotation;
use pdfreader_pdf::TilePixels;
use pdfreader_pdf::engine::{PdfEngine, PdfiumEngine, TileRequest};

/// Workspace root, derived from this crate's manifest directory.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// Locate the staged PDFium library, scanning `target/native/*`.
fn find_pdfium() -> Option<PathBuf> {
    let native = workspace_root().join("target").join("native");
    let entries = std::fs::read_dir(&native).ok()?;
    for entry in entries.flatten() {
        let dir = entry.path();
        if dir.join("pdfium.dll").exists()
            || dir.join("libpdfium.so").exists()
            || dir.join("libpdfium.dylib").exists()
        {
            return Some(dir);
        }
    }
    None
}

/// Skip a test with a clear message when its fixture is missing.
fn require_fixture(name: &str) -> PathBuf {
    let path = workspace_root().join("target").join("test-pdfs").join(name);
    if !path.exists() {
        eprintln!(
            "SKIP: fixture {} not found (run: python3 tools/generate_fixtures.py target/test-pdfs)",
            path.display()
        );
        std::process::exit(0);
    }
    path
}

/// Open the fixture, skipping when either PDFium or the fixture is absent.
fn open(name: &str) -> Option<(PdfiumEngine, pdfreader_pdf::DocumentHandle)> {
    let dir = find_pdfium()?;
    let mut engine = match PdfiumEngine::bind(Some(dir.as_path())) {
        Ok(engine) => engine,
        Err(error) => {
            eprintln!("SKIP: could not bind PDFium: {error}");
            return None;
        }
    };
    let path = require_fixture(name);
    let (handle, _info) = engine
        .open(&path, None)
        .unwrap_or_else(|error| panic!("{name} should open: {error}"));
    Some((engine, handle))
}

/// How far one pixel is from another, summed over the colour channels.
///
/// Tiles and the full page are rasterized by different PDFium entry points —
/// the full page through PDFium's origin offset, a tile through the transform
/// matrix — so the two antialias glyph edges slightly differently. Comparing
/// the *mean* difference rather than demanding identical bytes absorbs that
/// while still being far too sensitive to hide a tile showing the wrong
/// region: a shifted tile differs by tens per channel on every pixel, where
/// antialiasing moves a few percent of pixels by a little.
fn pixel_delta(a: &TilePixels, ax: u32, ay: u32, b: &TilePixels, bx: u32, by: u32) -> u64 {
    let index = |tile: &TilePixels, x: u32, y: u32| ((y * tile.width + x) * 4) as usize;
    let (ia, ib) = (index(a, ax, ay), index(b, bx, by));
    if ia + 3 >= a.data.len() || ib + 3 >= b.data.len() {
        return 0;
    }
    (0..3)
        .map(|channel| a.data[ia + channel].abs_diff(b.data[ib + channel]) as u64)
        .sum()
}

/// A tile must be the matching crop of the full-page render.
#[test]
fn a_tile_matches_the_full_page_render() {
    let _serial = one_at_a_time();
    let Some((mut engine, handle)) = open("small-3p.pdf") else {
        return;
    };

    // An awkward scale on purpose: a whole-number one could mask an offset that
    // happens to be a multiple of it.
    let scale = 2.5;
    let page = engine
        .render_page(handle, 0, Rotation::None, scale)
        .expect("full page render");

    // Tiles are 512 px square in the app; use a smaller size here so the test
    // exercises a tile that is neither the first row nor the first column.
    const TILE: u32 = 256;
    let columns = page.width.div_ceil(TILE);
    let rows = page.height.div_ceil(TILE);
    assert!(
        columns > 1 && rows > 1,
        "the fixture must need more than one tile"
    );

    let mut total_delta = 0u64;
    let mut compared = 0u64;

    for row in 0..rows {
        for column in 0..columns {
            let origin_x = column * TILE;
            let origin_y = row * TILE;
            let width = TILE.min(page.width - origin_x);
            let height = TILE.min(page.height - origin_y);

            let tile = engine
                .render_tile(
                    handle,
                    &TileRequest {
                        page: 0,
                        rotation: Rotation::None,
                        scale,
                        origin_x: origin_x as i32,
                        origin_y: origin_y as i32,
                        width,
                        height,
                    },
                )
                .expect("tile render");

            assert_eq!(tile.width, width);
            assert_eq!(tile.height, height);

            // Sample every 4th pixel: enough to catch a shifted region, cheap
            // enough to keep the test fast.
            for y in (0..height).step_by(4) {
                for x in (0..width).step_by(4) {
                    compared += 1;
                    total_delta += pixel_delta(&tile, x, y, &page, origin_x + x, origin_y + y);
                }
            }
        }
    }

    assert!(
        compared > 1000,
        "the comparison must actually cover the page"
    );

    // Measured at 1.8 per channel-summed pixel for a correct origin; shifting
    // the tile by even one pixel pushes it past 20. The threshold sits between
    // the two by a wide margin on both sides.
    let mean = total_delta as f64 / compared as f64;
    assert!(
        mean < 8.0,
        "tiles disagree with the full-page render (mean delta {mean:.1} per pixel over \
         {compared} sampled pixels) — the tile origin is being applied incorrectly"
    );
}

/// Dark pixels inside a rectangle of a rendered image.
///
/// `origin` is the image's top-left within the full page, in pixels — `(0, 0)`
/// for a full-page render. `page_height` is the *page's* height in pixels: the
/// rectangle is in PDF space, measured up from the page's bottom, while the
/// image is measured down from its top, so the flip needs the page, not the
/// image.
fn ink_in_rect(
    image: &TilePixels,
    rect: pdfreader_core::Rect,
    scale: f32,
    origin: (f32, f32),
    page_height: f32,
) -> u32 {
    let y0 = ((page_height - rect.max_y * scale) - origin.1).max(0.0) as u32;
    let y1 = ((page_height - rect.min_y * scale) - origin.1).max(0.0) as u32;
    let x0 = ((rect.min_x * scale) - origin.0).max(0.0) as u32;
    let x1 = ((rect.max_x * scale) - origin.0).max(0.0) as u32;

    let mut dark = 0;
    for y in y0..y1.min(image.height) {
        for x in x0..x1.min(image.width) {
            let index = ((y * image.width + x) * 4) as usize;
            if index + 2 < image.data.len() && image.data[index] < 128 {
                dark += 1;
            }
        }
    }
    dark
}

/// A screen tile must not carry form *values*.
///
/// The canvas draws every value itself, in the face, size and colour the user
/// chose, inside the field's own rectangle. PDFium drawing them as well put the
/// same value on screen twice — once where the form author placed it and once
/// where the canvas places it — so a committed field appeared to jump to the
/// top of its box. Full-page renders (print, export, thumbnails) must still
/// carry them, because nothing else paints values into those rasters.
#[test]
fn a_screen_tile_carries_no_form_values() {
    let _serial = one_at_a_time();
    let Some((mut engine, handle)) = open("forms.pdf") else {
        return;
    };
    let scale = 2.0;

    let form = engine.form_fields(handle).expect("form read");
    let field = form
        .fields
        .iter()
        .find(|f| f.name == "FullName")
        .expect("the fixture's text field");
    let (id, rect) = (field.id, field.rect);

    let page_render = |engine: &mut PdfiumEngine| {
        engine
            .render_page(handle, 0, Rotation::None, scale)
            .expect("page render")
    };
    let tile_render = |engine: &mut PdfiumEngine| {
        engine
            .render_tile(
                handle,
                &TileRequest {
                    page: 0,
                    rotation: Rotation::None,
                    scale,
                    origin_x: 0,
                    origin_y: 0,
                    width: 512,
                    height: 512,
                },
            )
            .expect("tile render")
    };

    let page_height = engine.page_size(handle, 0).expect("page size").1 * scale;
    let full_page = (0.0, 0.0);

    let before_tile = ink_in_rect(
        &tile_render(&mut engine),
        rect,
        scale,
        full_page,
        page_height,
    );
    let before_page = ink_in_rect(
        &page_render(&mut engine),
        rect,
        scale,
        full_page,
        page_height,
    );
    assert!(
        before_page > 0,
        "the fixture's field must have a visible value for this to prove anything"
    );

    // A value long enough to add plenty of ink wherever it is drawn.
    engine
        .set_field_value(
            handle,
            id,
            pdfreader_core::FieldValue::Text("WWWWWWWWWWWWWWWW".into()),
        )
        .expect("write the text field");

    let after_tile = ink_in_rect(
        &tile_render(&mut engine),
        rect,
        scale,
        full_page,
        page_height,
    );
    let after_page = ink_in_rect(
        &page_render(&mut engine),
        rect,
        scale,
        full_page,
        page_height,
    );

    assert_eq!(
        before_tile, after_tile,
        "a screen tile must not change when a value changes — the canvas draws values, \
         and a tile that carried them too would show each one twice \
         ({before_tile} -> {after_tile})"
    );
    assert!(
        after_page > before_page,
        "a full-page render must carry the value: print and export have no overlay \
         ({before_page} -> {after_page} dark pixels)"
    );
}
