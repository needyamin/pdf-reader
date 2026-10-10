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
