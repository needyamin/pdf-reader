//! End-to-end smoke test: the exact engine-call sequence the UI produces
//! while a user annotates a document, asserted after every single step.
//!
//! The shell runs this pipeline for every command: mutate → re-list →
//! reconcile. So the smoke test does too — nothing here is allowed to assume
//! the list is right; it is re-read and checked after every operation, the
//! same way the app reconciles after every command.
//!
//! Covered: repeated application of every tool, kinds combined on one page,
//! interleaved move/resize/recolour/rename, deletion preserving earlier
//! marks, and the whole document saved to bytes, reopened, and re-verified —
//! the "reopen the project and everything is still there" requirement.

use std::path::PathBuf;

use pdfreader_core::{AnnotationKind, NewAnnotation, Rect, Rotation};
use pdfreader_pdf::engine::{PdfEngine, PdfiumEngine};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

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

/// Every annotation with a rectangle the test placed, matched with tolerance.
fn find_rect(list: &[pdfreader_core::AnnotationInfo], rect: Rect) -> bool {
    list.iter().any(|a| {
        (a.rect.min_x - rect.min_x).abs() < 0.5
            && (a.rect.min_y - rect.min_y).abs() < 0.5
            && (a.rect.width() - rect.width()).abs() < 0.5
            && (a.rect.height() - rect.height()).abs() < 0.5
    })
}

#[test]
fn full_annotation_lifecycle_smoke() {
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
    let baseline = engine.annotations(handle).expect("list").len();

    // --- 1. Every tool, applied TWICE in a row, list verified after each ---
    let placements: Vec<(NewAnnotation, Rect)> = vec![
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
            NewAnnotation::Square(Rect::from_xywh(40.0, 480.0, 120.0, 60.0)),
            Rect::from_xywh(40.0, 480.0, 120.0, 60.0),
        ),
        (
            NewAnnotation::StickyNote((220.0, 700.0), "first".into()),
            Rect::from_xywh(210.0, 690.0, 20.0, 20.0),
        ),
        (
            NewAnnotation::FreeText(Rect::from_xywh(200.0, 520.0, 120.0, 30.0), "txt".into()),
            Rect::from_xywh(200.0, 520.0, 120.0, 30.0),
        ),
    ];

    let mut created: Vec<pdfreader_core::AnnotationId> = Vec::new();
    let mut expected_rects: Vec<Rect> = Vec::new();
    for (new, rect) in &placements {
        for _ in 0..2 {
            let id = engine.add_annotation(handle, 0, new.clone()).expect("add");
            created.push(id);
            expected_rects.push(*rect);
            let list = engine.annotations(handle).expect("list");
            assert_eq!(
                list.len(),
                baseline + expected_rects.len(),
                "list must grow by exactly one after each add"
            );
            assert!(
                find_rect(&list, *rect),
                "the just-created mark must appear at {rect:?}"
            );
        }
    }
    assert_eq!(created.len(), 14, "seven tools applied twice each");

    // --- 2. Interleaved edits: move, recolour, rename — list after each ---
    for (index, id) in created.iter().take(6).enumerate() {
        let moved = Rect::from_xywh(
            320.0 + index as f32 * 10.0,
            350.0,
            90.0,
            12.0 + index as f32 * 2.0,
        );
        assert!(
            engine.update_annotation_bounds(handle, *id, moved).expect("move"),
            "the id must resolve right after creation"
        );
        let list = engine.annotations(handle).expect("list");
        assert!(
            find_rect(&list, moved),
            "the moved mark must report its new bounds"
        );

        let color = [(100 + index as u8) * 2, 80, 160, 200];
        assert!(
            engine.set_annotation_color(handle, *id, color).expect("recolor"),
            "recolor must apply"
        );
        let list = engine.annotations(handle).expect("list");
        let found = list.iter().find(|a| a.id == *id).expect("still listed");
        assert_eq!(found.color, Some(color), "the colour must round-trip");
    }

    // Rename the first note (placements[5] → created[10]); the text must
    // come back. Note: create_annotation must return the NOTE's id, not its
    // auto-created popup's — this assertion is what caught that bug.
    let note_id = created[10];
    engine
        .set_annotation_contents(handle, note_id, "renamed note")
        .expect("contents");
    let list = engine.annotations(handle).expect("list");
    let note = list.iter().find(|a| a.id == note_id).expect("note listed");
    assert_eq!(note.contents.as_deref(), Some("renamed note"));

    // --- 3. Delete one of each duplicated pair; the other must survive ---
    //
    // Ids are POSITIONAL: after a removal, every annotation after it shifts
    // down an index. The UI handles this by re-listing after every mutation,
    // so the test does the same — it re-derives the id from the rectangle
    // immediately before each delete instead of trusting an old one.
    let same = |a: Rect, b: Rect| {
        (a.min_x - b.min_x).abs() < 0.5
            && (a.min_y - b.min_y).abs() < 0.5
            && (a.width() - b.width()).abs() < 0.5
            && (a.height() - b.height()).abs() < 0.5
    };
    let count_at = |list: &[pdfreader_core::AnnotationInfo], rect: Rect| {
        list.iter().filter(|a| same(a.rect, rect)).count()
    };
    // (The first six marks were just moved, so the delete check uses the
    // unmoved square pair.)
    let sq_rect = placements[4].1;
    let ft_rect = placements[6].1;
    let before = engine.annotations(handle).expect("list");
    assert_eq!(count_at(&before, sq_rect), 2, "two squares were placed");
    assert_eq!(count_at(&before, ft_rect), 2, "two typewriters were placed");

    for rect in [sq_rect, ft_rect] {
        let list = engine.annotations(handle).expect("list");
        let id = list
            .iter()
            .find(|a| same(a.rect, rect))
            .expect("a duplicate is still present")
            .id;
        assert!(engine.delete_annotation(handle, id).expect("delete"));
        let after = engine.annotations(handle).expect("list");
        assert_eq!(
            count_at(&after, rect),
            1,
            "exactly one of the pair survives its sibling's deletion"
        );
    }
    let after_delete = engine.annotations(handle).expect("list");
    assert_eq!(after_delete.len(), baseline + 12);

    // --- 4. Save → reopen → everything still there (persistence) ---
    let bytes = engine.save_to_bytes(handle, false).expect("save");
    let out = std::env::temp_dir().join("pdfreader-smoke-annotated.pdf");
    std::fs::write(&out, &bytes).expect("write saved bytes");

    let (reopen, _info) = engine.open(&out, None).expect("reopen");
    let reopened = engine.annotations(reopen).expect("list");
    assert_eq!(
        reopened.len(),
        after_delete.len(),
        "every surviving annotation must persist across save/reopen"
    );
    for info in &after_delete {
        let found = reopened
            .iter()
            .find(|a| same(a.rect, info.rect) && a.kind == info.kind);
        assert!(
            found.is_some(),
            "{:?} at {:?} must survive the reopen",
            info.kind,
            info.rect
        );
    }
    // The renamed note keeps its text through the round-trip.
    let note = reopened.iter().find(|a| {
        a.kind == AnnotationKind::StickyNote && a.contents == Some("renamed note".into())
    });
    assert!(note.is_some(), "the renamed note must survive the reopen");

    // Render one page with everything on it — the rasterizer must not choke
    // on the heavily annotated page.
    let pixels = engine
        .render_page(reopen, 0, Rotation::None, 1.0)
        .expect("render annotated page");
    assert!(pixels.width > 0 && pixels.height > 0);

    std::fs::remove_file(&out).ok();
}

/// Non-white pixels inside a window of the rendered page.
///
/// The fixture page has text on it, so the mark's own ink has to be isolated by
/// a window and compared against a baseline render of the same window — the
/// page content is identical in both, so any difference is the mark.
fn window_ink(pixels: &pdfreader_pdf::engine::TilePixels, x0: usize, x1: usize, y0: usize, y1: usize) -> usize {
    let width = pixels.width as usize;
    let mut ink = 0;
    for (index, pixel) in pixels.data.chunks_exact(4).enumerate() {
        let x = index % width;
        let y = index / width;
        if x < x0 || x >= x1 || y < y0 || y >= y1 {
            continue;
        }
        if !(pixel[0] > 240 && pixel[1] > 240 && pixel[2] > 240) {
            ink += 1;
        }
    }
    ink
}

/// The user-facing half of the vertical-resize contract, driven through the
/// engine exactly as the canvas drives it: dragging a top or bottom handle on
/// an underline must make the *rendered mark* thicker, and the change must
/// survive save → reopen.
///
/// Pdfium draws these three subtypes as a hard-coded 1pt line, so before the
/// appearance stream was supplied this test could not have passed: the ink
/// height was identical no matter how far the handle was dragged.
#[test]
fn vertical_resize_thickens_a_line_markup_and_survives_reopen() {
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

    // The window the mark lives in: the band it spans horizontally, plus enough
    // height for it to grow into. PDF space is y-up, the bitmap y-down.
    let (x0, x1) = (85, 235);
    let (y0, y1) = (300, 460);
    let baseline = window_ink(
        &engine.render_page(handle, 0, Rotation::None, 1.0).expect("render"),
        x0,
        x1,
        y0,
        y1,
    );

    // A band the height of a text line — what the underline tool drags over.
    let id = engine
        .add_annotation(
            handle,
            0,
            NewAnnotation::Underline(Rect::from_xywh(80.0, 400.0, 160.0, 14.0)),
        )
        .expect("add");

    // As drawn, the mark must stay a thin underline rather than filling the
    // band it was dragged over: a 1.4pt line over 160pt is ~230px of ink.
    let drawn = window_ink(
        &engine.render_page(handle, 0, Rotation::None, 1.0).expect("render"),
        x0,
        x1,
        y0,
        y1,
    );
    assert!(
        drawn < baseline + 1200,
        "a freshly drawn underline must be as thin as the preview, \
         got {}px of ink over a {baseline}px baseline",
        drawn - baseline
    );

    // Now pull the top handle a long way up.
    let grown = Rect::from_xywh(80.0, 400.0, 160.0, 114.0);
    assert!(
        engine
            .update_annotation_bounds(handle, id, grown)
            .expect("resize"),
        "the resize must apply"
    );
    let listed = engine.annotations(handle).expect("list");
    assert!(find_rect(&listed, grown), "the new bounds must be reported");
    let thick = window_ink(
        &engine.render_page(handle, 0, Rotation::None, 1.0).expect("render"),
        x0,
        x1,
        y0,
        y1,
    );
    assert!(
        thick > baseline + 5000,
        "dragging the top handle up must thicken the mark \
         ({}px of ink over a {baseline}px baseline)",
        thick - baseline
    );

    // The colour must still round-trip through the list after that resize.
    let color = [12, 34, 56, 255];
    assert!(
        engine
            .set_annotation_color(handle, id, color)
            .expect("recolour"),
        "recolour must apply"
    );
    let listed = engine.annotations(handle).expect("list");
    let found = listed.iter().find(|a| a.id == id).expect("still listed");
    assert_eq!(found.color, Some(color), "the colour must round-trip");

    // Save, reopen, and check the thickness is still there.
    let bytes = engine.save_to_bytes(handle, false).expect("save");
    let out = std::env::temp_dir().join("pdfreader-smoke-thickness.pdf");
    std::fs::write(&out, &bytes).expect("write");
    let (reopen, _i) = engine.open(&out, None).expect("reopen");

    let reopened = engine.annotations(reopen).expect("list");
    let mark = reopened
        .iter()
        .find(|a| a.kind == AnnotationKind::Underline)
        .expect("the underline must survive the reopen");
    assert_eq!(mark.color, Some(color), "the colour must survive the reopen");
    assert!(
        (mark.rect.height() - 114.0).abs() < 1.0,
        "the resized bounds must survive the reopen, got {:?}",
        mark.rect
    );
    let persisted = window_ink(
        &engine.render_page(reopen, 0, Rotation::None, 1.0).expect("render"),
        x0,
        x1,
        y0,
        y1,
    );
    assert!(
        persisted > baseline + 5000,
        "the thickened mark must still be thick after save and reopen \
         ({}px of ink over a {baseline}px baseline)",
        persisted - baseline
    );

    std::fs::remove_file(&out).ok();
}

/// Total ink coverage inside a window: each pixel's darkness, 0 (white) to 255.
///
/// Unlike a count of non-white pixels this is linear in a stroke's width, so a
/// hairline and a 1.4pt line are distinguishable even though antialiasing
/// leaves both with a row of lightly touched pixels. Channel order is not
/// assumed — the average is taken — so BGRA and RGBA measure the same.
fn window_darkness(
    pixels: &pdfreader_pdf::engine::TilePixels,
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
) -> u64 {
    let width = pixels.width as usize;
    let mut darkness = 0u64;
    for (index, pixel) in pixels.data.chunks_exact(4).enumerate() {
        let x = index % width;
        let y = index / width;
        if x < x0 || x >= x1 || y < y0 || y >= y1 {
            continue;
        }
        let mean = (u32::from(pixel[0]) + u32::from(pixel[1]) + u32::from(pixel[2])) / 3;
        darkness += u64::from(255 - mean);
    }
    darkness
}

/// Render page 0 and measure the ink inside `window`, in the 4× pixel space
/// the window is given in.
///
/// A stroke thinner than a device pixel is widened by the rasterizer so it
/// stays visible, which flattens the difference between weights at 1:1. At 4×
/// the 1.4pt default covers ~5.6px and a hairline ~0.8px, so the two weights
/// separate cleanly.
fn page_darkness(
    engine: &mut PdfiumEngine,
    handle: pdfreader_pdf::DocumentHandle,
    window: (usize, usize, usize, usize),
) -> u64 {
    let pixels = engine
        .render_page(handle, 0, Rotation::None, 4.0)
        .expect("render");
    window_darkness(&pixels, window.0, window.1, window.2, window.3)
}

/// The "I can't make it very very thin, it locks suddenly" report, driven
/// through the engine exactly as the canvas drives it.
///
/// A line markup's height is its stroke weight, and the weight floor used to be
/// the weight the mark was drawn at, so every downward drag clamped straight
/// back and the stroke stopped dead. The mark is now *born* a hairline; what
/// this exercises is the other half — that a thickened mark can be dragged all
/// the way back down to a hairline, and that the hairline survives save →
/// reopen instead of reverting to Pdfium's own 1pt line.
#[test]
fn a_line_markup_can_be_thinned_back_to_a_hairline_and_survives_reopen() {
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

    // The window the mark lives in, in the 4× pixel space `page_darkness`
    // renders in. The mark's own ink is isolated against a baseline render of
    // the same window, because the page has text.
    let window = (340, 940, 1200, 1840);
    let baseline = page_darkness(&mut engine, handle, window);

    // A band the height of a text line — what the underline tool drags over.
    let band = 14.0;
    let id = engine
        .add_annotation(
            handle,
            0,
            NewAnnotation::Underline(Rect::from_xywh(80.0, 400.0, 160.0, band)),
        )
        .expect("add");
    let drawn = page_darkness(&mut engine, handle, window) - baseline;
    assert!(drawn > 0, "the freshly drawn underline must show ink");

    // Drag the top handle a long way up: the mark must get clearly thicker.
    let grown = Rect::from_xywh(80.0, 400.0, 160.0, band + 100.0);
    assert!(
        engine
            .update_annotation_bounds(handle, id, grown)
            .expect("grow"),
        "the thickening resize must apply"
    );
    let thick = page_darkness(&mut engine, handle, window) - baseline;
    assert!(
        thick > drawn * 5,
        "dragging the handle up must thicken the mark well past the weight it \
         was drawn at (drawn {drawn}, thick {thick})"
    );

    // Now drag it back down to the smallest box the canvas allows. The weight
    // falls by the same distance the box does, so it bottoms out at the
    // hairline floor — this is the step that used to lock.
    let thinned_rect = Rect::from_xywh(80.0, 400.0, 160.0, band / 2.0);
    assert!(
        engine
            .update_annotation_bounds(handle, id, thinned_rect)
            .expect("thin"),
        "the thinning resize must apply"
    );
    let thinned = page_darkness(&mut engine, handle, window) - baseline;
    assert!(
        thinned * 5 < thick,
        "dragging the handle back down must thin the mark again \
         (thick {thick}, thinned {thinned})"
    );

    // Save, reopen, and the hairline must still be a hairline — not restored to
    // Pdfium's own 1pt line by the round-trip.
    let bytes = engine.save_to_bytes(handle, false).expect("save");
    let out = std::env::temp_dir().join("pdfreader-smoke-hairline.pdf");
    std::fs::write(&out, &bytes).expect("write");
    let (reopen, _i) = engine.open(&out, None).expect("reopen");

    let reopened = engine.annotations(reopen).expect("list");
    let mark = reopened
        .iter()
        .find(|a| a.kind == AnnotationKind::Underline)
        .expect("the underline must survive the reopen");
    assert!(
        (mark.rect.height() - band / 2.0).abs() < 1.0,
        "the thinned bounds must survive the reopen, got {:?}",
        mark.rect
    );
    let persisted = page_darkness(&mut engine, reopen, window) - baseline;
    assert!(
        persisted * 5 < thick,
        "the thinned stroke must still be thin after save and reopen \
         (thick {thick}, persisted {persisted})"
    );

    std::fs::remove_file(&out).ok();
}

/// Minimal isolation: does a note rename survive save → reopen on its own?
#[test]
fn rename_survives_save_reopen_minimal() {
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
        eprintln!("SKIP: fixture missing");
        return;
    }
    let (handle, _info) = engine.open(&path, None).expect("open");
    let id = engine
        .add_annotation(
            handle,
            0,
            NewAnnotation::StickyNote((100.0, 400.0), "first".into()),
        )
        .expect("add");
    engine
        .set_annotation_contents(handle, id, "renamed note")
        .expect("rename");
    let live = engine.annotations(handle).expect("list");
    eprintln!("DEBUG live contents: {:?}", live.iter().find(|a| a.id == id).map(|a| &a.contents));

    let bytes = engine.save_to_bytes(handle, false).expect("save");
    let out = std::env::temp_dir().join("pdfreader-smoke-rename.pdf");
    std::fs::write(&out, &bytes).expect("write");
    let (reopen, _i) = engine.open(&out, None).expect("reopen");
    let after = engine.annotations(reopen).expect("list");
    eprintln!("DEBUG reopened contents: {:?}", after.iter().find(|a| a.kind == AnnotationKind::StickyNote).map(|a| &a.contents));
    let note = after
        .iter()
        .find(|a| a.kind == AnnotationKind::StickyNote)
        .expect("note persists");
    assert_eq!(note.contents.as_deref(), Some("renamed note"));
    std::fs::remove_file(&out).ok();
}
