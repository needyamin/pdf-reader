//! A drawn annotation must land on the rectangle it was given, and be visible.
//!
//! Both halves matter and neither is obvious from the code. The rectangle
//! travels from a pointer drag, through layout space, into unrotated PDF points,
//! and is then handed to PDFium — so a sign error puts the mark somewhere the
//! user did not draw. And a created annotation is only as visible as the
//! properties PDFium needs: a rectangle whose fill is set to *fully
//! transparent* renders neither its fill nor its border, so it appeared while
//! the drag was in progress (the canvas draws its own preview) and then vanished
//! the moment the re-rendered page arrived.
//!
//! Fixtures live under `target/test-pdfs`, produced by
//! `tools/generate_fixtures.py`, so the test skips cleanly when they are absent.

use std::path::PathBuf;

use pdfreader_core::{NewAnnotation, Rect, Rotation};
use pdfreader_pdf::engine::{PdfEngine, PdfiumEngine, TilePixels};

/// Workspace root, derived from this crate's manifest directory.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// Locate the staged PDFium library, scanning `target/native/*`.
fn find_pdfium() -> Option<PathBuf> {
    let native = workspace_root().join("target").join("native");
    for entry in std::fs::read_dir(&native).ok()?.flatten() {
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

/// Dark pixels inside a rectangle of a rendered page.
///
/// The raster is measured down from the top, the rectangle up from the bottom,
/// so the vertical axis flips.
fn ink(image: &TilePixels, rect: Rect, scale: f32) -> u32 {
    let height = image.height as f32;
    let y0 = ((height - rect.max_y * scale) as i32).max(0) as u32;
    let y1 = ((height - rect.min_y * scale) as i32).max(0) as u32;
    let x0 = ((rect.min_x * scale) as i32).max(0) as u32;
    let x1 = ((rect.max_x * scale) as i32).max(0) as u32;

    let mut dark = 0;
    for y in y0..y1.min(image.height) {
        for x in x0..x1.min(image.width) {
            let index = ((y * image.width + x) * 4) as usize;
            if index + 2 < image.data.len() && image.data[index] < 200 {
                dark += 1;
            }
        }
    }
    dark
}

/// Every tool that draws a rectangle must leave a mark inside it, and nothing
/// anywhere else.
#[test]
fn a_drawn_annotation_lands_on_its_rectangle_and_is_visible() {
    let Some(dir) = find_pdfium() else {
        eprintln!("SKIP: no PDFium library staged under target/native");
        return;
    };
    let mut engine = PdfiumEngine::bind(Some(dir.as_path())).expect("bind");
    let path = workspace_root()
        .join("target")
        .join("test-pdfs")
        .join("small-3p.pdf");
    if !path.exists() {
        eprintln!("SKIP: fixture missing (run tools/generate_fixtures.py)");
        return;
    }
    let (handle, _info) = engine.open(&path, None).expect("open");
    let scale = 2.0;

    // Drawn where the page is blank, with a band well away from it to catch a
    // mark landing anywhere other than where it was drawn.
    let drawn = Rect::from_xywh(120.0, 300.0, 160.0, 40.0);
    let elsewhere = Rect::from_xywh(120.0, 500.0, 160.0, 40.0);

    for (name, new) in [
        ("Underline", NewAnnotation::Underline(drawn)),
        ("StrikeOut", NewAnnotation::StrikeOut(drawn)),
        ("Squiggly", NewAnnotation::Squiggly(drawn)),
        ("Rectangle", NewAnnotation::Square(drawn)),
        ("Highlight", NewAnnotation::Highlight(drawn)),
    ] {
        let before = engine
            .render_page(handle, 0, Rotation::None, scale)
            .expect("render");
        engine.add_annotation(handle, 0, new).expect("create");
        let after = engine
            .render_page(handle, 0, Rotation::None, scale)
            .expect("render");

        let here = ink(&after, drawn, scale) as i32 - ink(&before, drawn, scale) as i32;
        let away = ink(&after, elsewhere, scale) as i32 - ink(&before, elsewhere, scale) as i32;

        assert!(
            here > 0,
            "{name} drew nothing inside the rectangle it was given — a created \
             annotation is only as visible as the properties PDFium needs"
        );
        assert_eq!(away, 0, "{name} drew outside the rectangle it was given");
    }
}
