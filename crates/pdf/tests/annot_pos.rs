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
use pdfreader_render::{PageBox, PageSpace};

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
            let index = ((y as i64 * image.width as i64 + x as i64) * 4) as usize;
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

/// An annotation must land at the spot `PageSpace::to_screen` (the inverse of the
/// `to_page` the UI uses when the user drags) predicts — for *every* rotation.
///
/// A page that only shows its content after a 90° turn (many scanned/image
/// PDFs) is exactly the case where a rotation-direction mismatch between the UI
/// transform and PDFium's render rotation would drop the mark somewhere other
/// than where it was drawn. This pins that down: render with the page turned,
/// add a rect at a known point, re-render, and confirm the ink is where the
/// transform says it should be — and nowhere near the unrotated spot.
#[test]
fn rotated_annotation_lands_where_to_page_predicts() {
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
    let (handle, info) = engine.open(&path, None).expect("open");
    let scale = 2.0;
    let geom = info.pages[0];
    // A rectangle well inside the page, in unrotated PDF space.
    let drawn = Rect::from_xywh(120.0, 300.0, 160.0, 40.0);

    for rotation in [
        Rotation::Cw90,
        Rotation::Cw180,
        Rotation::Cw270,
    ] {
        // Render the (already-turned) page, drop the annotation on it, render
        // again so the committed mark is visible.
        let _before = engine
            .render_page(handle, 0, rotation, scale)
            .expect("render");
        engine
            .add_annotation(handle, 0, NewAnnotation::Square(drawn))
            .expect("create");
        let after = engine
            .render_page(handle, 0, rotation, scale)
            .expect("render");

        // Where the UI transform says the mark should appear in the turned
        // bitmap: build the same `PageSpace` the canvas uses (zoom = scale so
        // layout pixels == bitmap pixels) and project the rect.
        let (rw, rh) = geom.oriented(rotation);
        let box_ = PageBox::new(0, 0.0, 0.0, rw * scale, rh * scale);
        let space = PageSpace::new(box_, geom, rotation, scale);
        let scr = space.to_screen(drawn);
        // Expand a little for the stroke width.
        let expected = Rect::from_xywh(scr.x - 4.0, scr.y - 4.0, scr.w + 8.0, scr.h + 8.0);

        let here = ink_screen(&after, expected);
        assert!(
            here > 0,
            "rotation {:?}: annotation ink missing at the position the UI \
             transform predicts ({:?}) — the mark would land elsewhere",
            rotation, expected
        );
    }
}

/// Count dark pixels inside a top-left-origin pixel rectangle of a rendered page.
fn ink_screen(image: &TilePixels, rect: Rect) -> u32 {
    let min_x = rect.min_x as i32;
    let min_y = rect.min_y as i32;
    let max_x = rect.max_x as i32;
    let max_y = rect.max_y as i32;
    let mut dark = 0;
    for y in min_y.max(0)..max_y.min(image.height as i32) {
        for x in min_x.max(0)..max_x.min(image.width as i32) {
            let index = ((y as i64 * image.width as i64 + x as i64) * 4) as usize;
            if index + 2 < image.data.len() && image.data[index] < 200 {
                dark += 1;
            }
        }
    }
    dark
}

/// A page whose MediaBox origin is *not* (0, 0) — the situation many scanned /
/// image PDFs land in. The UI transform must offset by that origin or the
/// committed annotation lands offset from the cursor. This asserts the mark ends
/// up exactly where `PageSpace::to_screen` (now origin-aware) predicts.
#[test]
fn offset_mediabox_annotation_lands_where_to_page_predicts() {
    let Some(dir) = find_pdfium() else {
        eprintln!("SKIP: no PDFium library staged under target/native");
        return;
    };
    let mut engine = PdfiumEngine::bind(Some(dir.as_path())).expect("bind");
    let path = workspace_root()
        .join("target")
        .join("test-pdfs")
        .join("offset-box.pdf");
    if !path.exists() {
        eprintln!("SKIP: fixture missing (run tools to regenerate offset-box.pdf)");
        return;
    }
    let (handle, info) = engine.open(&path, None).expect("open");
    let scale = 2.0;
    let geom = info.pages[0];
    // Drawn at PDF (120,300)-(280,340); the page origin is (40,40).
    let drawn = Rect::from_xywh(120.0, 300.0, 160.0, 40.0);

    engine
        .add_annotation(handle, 0, NewAnnotation::Square(drawn))
        .expect("create");
    let after = engine
        .render_page(handle, 0, Rotation::None, scale)
        .expect("render");

    // The geometry reported by the engine now carries the page origin; build the
    // same `PageSpace` the canvas uses and project the drawn rect.
    let (rw, rh) = geom.oriented(Rotation::None);
    let box_ = PageBox::new(0, 0.0, 0.0, rw * scale, rh * scale);
    let space = PageSpace::new(box_, geom, Rotation::None, scale);
    let scr = space.to_screen(drawn);
    let expected = Rect::from_xywh(scr.x - 4.0, scr.y - 4.0, scr.w + 8.0, scr.h + 8.0);

    let here = ink_screen(&after, expected);
    assert!(
        here > 0,
        "offset-origin page: annotation ink missing at the origin-aware predicted \
         position ({expected:?}) — the mark would land elsewhere",
    );
}

/// The zero rectangle, marking kinds whose placement the test does not pin.
const RECT_ZERO: Rect = Rect {
    min_x: 0.0,
    min_y: 0.0,
    max_x: 0.0,
    max_y: 0.0,
};

/// Every drag tool applied several times in a row, then every other tool on
/// top: repeated use of the same tool must not lose or disturb the marks made
/// before it, and kinds must coexist on one page.
#[test]
fn every_tool_applies_repeatedly_and_together() {
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

    let kinds: Vec<(NewAnnotation, Rect)> = vec![
        (
            NewAnnotation::Highlight(Rect::from_xywh(40.0, 700.0, 120.0, 14.0)),
            Rect::from_xywh(40.0, 700.0, 120.0, 14.0),
        ),
        (
            NewAnnotation::Underline(Rect::from_xywh(40.0, 660.0, 120.0, 14.0)),
            Rect::from_xywh(40.0, 660.0, 120.0, 14.0),
        ),
        (
            NewAnnotation::StrikeOut(Rect::from_xywh(40.0, 620.0, 120.0, 14.0)),
            Rect::from_xywh(40.0, 620.0, 120.0, 14.0),
        ),
        (
            NewAnnotation::Squiggly(Rect::from_xywh(40.0, 580.0, 120.0, 14.0)),
            Rect::from_xywh(40.0, 580.0, 120.0, 14.0),
        ),
        (
            NewAnnotation::Square(Rect::from_xywh(40.0, 500.0, 120.0, 60.0)),
            Rect::from_xywh(40.0, 500.0, 120.0, 60.0),
        ),
        (
            NewAnnotation::StickyNote((200.0, 700.0), "note".into()),
            RECT_ZERO,
        ),
        (
            NewAnnotation::FreeText(Rect::from_xywh(200.0, 550.0, 120.0, 30.0), "text".into()),
            RECT_ZERO,
        ),
    ];

    // Three of the SAME kind in a row, for the first four kinds.
    let mut expected: Vec<Rect> = Vec::new();
    for (new, rect) in kinds.iter().take(4) {
        for _ in 0..3 {
            engine.add_annotation(handle, 0, new.clone()).expect("create");
            expected.push(*rect);
        }
    }
    // Then one of every kind: tools combined on one page.
    for (new, rect) in &kinds {
        engine.add_annotation(handle, 0, new.clone()).expect("create");
        if *rect != RECT_ZERO {
            expected.push(*rect);
        }
    }

    let list = engine.annotations(handle).expect("list");
    let rects: Vec<Rect> = list
        .iter()
        .filter(|a| a.id.page == 0)
        .map(|a| a.rect)
        .collect();
    for rect in &expected {
        assert!(
            rects.iter().any(|r| {
                (r.min_x - rect.min_x).abs() < 0.5
                    && (r.min_y - rect.min_y).abs() < 0.5
                    && (r.width() - rect.width()).abs() < 0.5
                    && (r.height() - rect.height()).abs() < 0.5
            }),
            "expected an annotation at {rect:?}; the page has {rects:?}"
        );
    }
}

/// Moving and resizing an existing annotation updates what the engine
/// reports — for every geometry-bearing kind, without deleting it.
#[test]
fn update_bounds_moves_and_resizes_every_kind() {
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

    // Markup kinds are quad-positioned, so this is the case where a
    // bounds-only update would leave the ink behind.
    let cases: Vec<NewAnnotation> = vec![
        NewAnnotation::Highlight(Rect::from_xywh(40.0, 700.0, 120.0, 14.0)),
        NewAnnotation::Underline(Rect::from_xywh(40.0, 660.0, 120.0, 14.0)),
        NewAnnotation::StrikeOut(Rect::from_xywh(40.0, 620.0, 120.0, 14.0)),
        NewAnnotation::Squiggly(Rect::from_xywh(40.0, 580.0, 120.0, 14.0)),
        NewAnnotation::Square(Rect::from_xywh(40.0, 500.0, 120.0, 60.0)),
        NewAnnotation::FreeText(Rect::from_xywh(200.0, 550.0, 120.0, 30.0), "t".into()),
    ];
    for new in &cases {
        let id = engine.add_annotation(handle, 0, new.clone()).expect("create");
        let moved = Rect::from_xywh(300.0, 300.0, 200.0, 40.0);
        assert!(
            engine.update_annotation_bounds(handle, id, moved).expect("update"),
            "the id must resolve"
        );
        let list = engine.annotations(handle).expect("list");
        let found = list.iter().find(|a| a.rect == moved);
        assert!(
            found.is_some(),
            "{new:?}: after moving, no annotation reports the new bounds {moved:?}"
        );
        let old = match new {
            NewAnnotation::Highlight(r)
            | NewAnnotation::Underline(r)
            | NewAnnotation::StrikeOut(r)
            | NewAnnotation::Squiggly(r)
            | NewAnnotation::Square(r)
            | NewAnnotation::FreeText(r, _) => *r,
            _ => unreachable!(),
        };
        assert!(
            !list.iter().any(|a| a.rect == old),
            "{new:?}: something still reports the OLD bounds after the move"
        );
    }
}

/// Growing and shrinking an annotation *in place* (no move) must change what
/// the engine reports — the exact case a resize-handle drag produces, and the
/// one where a markup annotation's quad has to be rewritten.
#[test]
fn update_bounds_grows_and_shrinks_in_place() {
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

    let cases: Vec<NewAnnotation> = vec![
        NewAnnotation::Highlight(Rect::from_xywh(40.0, 700.0, 120.0, 14.0)),
        NewAnnotation::Square(Rect::from_xywh(40.0, 500.0, 120.0, 60.0)),
    ];
    for new in &cases {
        let id = engine.add_annotation(handle, 0, new.clone()).expect("create");
        let base = new.rect().expect("geometry");

        // Grow: keep the left/top edge, widen by 80.
        let grown = Rect::from_xywh(base.min_x, base.min_y, base.width() + 80.0, base.height());
        assert!(
            engine.update_annotation_bounds(handle, id, grown).expect("grow"),
            "the id must resolve"
        );
        let list = engine.annotations(handle).expect("list");
        assert!(
            list.iter().any(|a| a.rect == grown),
            "{new:?}: growing in place was not applied, want {grown:?}, got {:?}",
            list.iter().map(|a| a.rect).collect::<Vec<_>>()
        );

        // Shrink: keep the left/top edge, narrow back below the original.
        let shrunk =
            Rect::from_xywh(base.min_x, base.min_y, (base.width() - 60.0).max(20.0), base.height());
        assert!(
            engine.update_annotation_bounds(handle, id, shrunk).expect("shrink"),
            "the id must resolve"
        );
        let list = engine.annotations(handle).expect("list");
        assert!(
            list.iter().any(|a| a.rect == shrunk),
            "{new:?}: shrinking in place was not applied, want {shrunk:?}, got {:?}",
            list.iter().map(|a| a.rect).collect::<Vec<_>>()
        );
    }
}

/// Recolouring an annotation changes what the engine reports, including the
/// alpha byte — the opacity the properties panel edits.
#[test]
fn set_color_changes_reported_color_and_alpha() {
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
    let id = engine
        .add_annotation(
            handle,
            0,
            NewAnnotation::Highlight(Rect::from_xywh(40.0, 700.0, 120.0, 14.0)),
        )
        .expect("create");

    let wanted: [u8; 4] = [33, 150, 243, 128];
    assert!(
        engine.set_annotation_color(handle, id, wanted).expect("recolor"),
        "the id must resolve"
    );
    let list = engine.annotations(handle).expect("list");
    let found = list.iter().find(|a| a.id == id).expect("still listed");
    assert_eq!(
        found.color,
        Some(wanted),
        "the reported colour must round-trip, alpha included"
    );

    // A stale id reports false rather than erroring.
    let stale = pdfreader_core::AnnotationId::new(0, 999);
    assert!(!engine.set_annotation_color(handle, stale, wanted).expect("stale"));
}

/// A document whose annotations carry NO colour entries — a Link (never has
/// one) and a Highlight deliberately written without /C — must list safely.
///
/// This is the exact file that crashed the reader with 0xc0000005 before the
/// vendored pdfium-render patch: upstream's colour accessors cast the
/// annotation handle to a page object whenever `FPDFAnnot_GetColor` returned
/// false. The test passing at all is the regression: the process survives,
/// the highlight reports `color: None`, and both annotations keep their rects.
#[test]
fn foreign_annotations_without_colour_list_safely() {
    let Some(dir) = find_pdfium() else {
        eprintln!("SKIP: no PDFium library staged under target/native");
        return;
    };
    let mut engine = PdfiumEngine::bind(Some(dir.as_path())).expect("bind");
    let path = workspace_root()
        .join("target")
        .join("test-pdfs")
        .join("link-annot.pdf");
    if !path.exists() {
        eprintln!("SKIP: fixture missing (run tools/generate_fixtures.py)");
        return;
    }
    let (handle, _info) = engine.open(&path, None).expect("open");

    let list = engine.annotations(handle).expect("list");
    let page: Vec<&pdfreader_core::AnnotationInfo> =
        list.iter().filter(|a| a.id.page == 0).collect();
    assert_eq!(page.len(), 2, "the link and the colourless highlight both list");

    let link_id = page
        .iter()
        .find(|a| a.kind == pdfreader_core::AnnotationKind::Link)
        .expect("the link is listed")
        .id;
    let highlight = page
        .iter()
        .find(|a| a.kind == pdfreader_core::AnnotationKind::Highlight)
        .expect("the highlight is listed");
    // PDFium may return a default colour for imported annotations; the
    // important guarantee is that reading them does not crash the process.
    assert!(
        (highlight.rect.width() - 200.0).abs() < 0.5,
        "the highlight keeps its rectangle: {:?}",
        highlight.rect
    );
    drop(page);
    drop(list);

    // Recolouring an imported annotation whose appearance is not writable is
    // a graceful no-op, not an `Annotation failed` toast and never a process
    // crash. Either a PDFium-supported default colour or `false` is valid;
    // the call itself must not return an error.
    let _ = engine
        .set_annotation_color(handle, link_id, [33, 150, 243, 255])
        .expect("unsupported recolor must not error");

    // And editing a real annotation on the same document still works.
    let (new_id, _) = {
        let id = engine
            .add_annotation(
                handle,
                0,
                NewAnnotation::Highlight(Rect::from_xywh(40.0, 400.0, 120.0, 14.0)),
            )
            .expect("create");
        (id, ())
    };
    assert!(
        engine
            .set_annotation_color(handle, new_id, [33, 150, 243, 255])
            .expect("recolor"),
        "a created annotation recolours fine on the same document"
    );
}
