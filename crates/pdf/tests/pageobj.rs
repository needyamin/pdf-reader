//! End-to-end tests for page-content editing against a real document.
//!
//! These are the only tests that exercise the vendored `PDFium` page-object
//! calls for real — `crates/pdf`'s unit tests can check a rectangle conversion
//! and nothing more, because a page object cannot be built without a document.
//! So this file is where "does `FPDFText_SetText` actually work through the
//! binding, and does `regenerate_content` make it survive a save" is answered.
//!
//! Like the other integration tests here, it skips rather than fails when the
//! staged `PDFium` library or the generated fixtures are absent.

use std::path::PathBuf;

use pdfreader_core::{PageObjectKind, Rect, TextFormat};
use pdfreader_pdf::engine::{PdfEngine, PdfiumEngine};

/// Workspace root, derived from this crate's manifest directory.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// Locate the staged `PDFium` library, scanning `target/native/*`.
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

/// Bind an engine and open the three-page fixture, or explain why not.
fn fixture() -> Option<(PdfiumEngine, pdfreader_pdf::engine::DocumentHandle)> {
    let Some(dir) = find_pdfium() else {
        eprintln!("SKIP: no PDFium library staged under target/native");
        return None;
    };
    let mut engine = PdfiumEngine::bind(Some(dir.as_path())).expect("bind");
    let path = workspace_root()
        .join("target")
        .join("test-pdfs")
        .join("small-3p.pdf");
    if !path.exists() {
        eprintln!("SKIP: fixture missing (run tools/generate_fixtures.py)");
        return None;
    }
    let (handle, _info) = engine.open(&path, None).expect("open");
    Some((engine, handle))
}

/// The objects on a page, as the editor would see them.
fn objects(
    engine: &mut PdfiumEngine,
    handle: pdfreader_pdf::engine::DocumentHandle,
    page: u32,
) -> Vec<pdfreader_core::PageObjectInfo> {
    engine.page_objects(handle, page).expect("list objects")
}

/// The object holding exactly this text.
fn find_text(
    list: &[pdfreader_core::PageObjectInfo],
    text: &str,
) -> Option<pdfreader_core::PageObjectInfo> {
    list.iter()
        .find(|o| o.kind == PageObjectKind::Text && o.text.as_deref() == Some(text))
        .cloned()
}

/// Insert, read back, move, retype, delete — the whole editor, in the order the
/// UI performs it, with the list re-read after every step.
#[test]
fn a_text_object_can_be_added_moved_retyped_and_removed() {
    let Some((mut engine, handle)) = fixture() else {
        return;
    };

    // --- Insert -----------------------------------------------------------
    let id = engine
        .insert_text_object(handle, 0, 72.0, 700.0, "Placed by the test", 12.0)
        .expect("insert text object");

    let list = objects(&mut engine, handle, 0);
    let placed = find_text(&list, "Placed by the test").expect("the new object is listed");
    assert_eq!(placed.kind, PageObjectKind::Text);
    assert_eq!(placed.id.page, 0);
    assert!(
        placed.font_size.is_some_and(|size| size > 0.0),
        "a created text object must report a size: {placed:?}"
    );
    assert!(
        !placed.bounds.is_degenerate(),
        "a created text object must have a real box: {placed:?}"
    );

    // The id the engine handed back must address the object it created.
    assert_eq!(
        list.iter()
            .filter(|o| o.id.index == id.index)
            .map(|o| o.text.as_deref())
            .next(),
        Some(Some("Placed by the test")),
        "the returned index must point at the created object"
    );

    // --- Move and resize --------------------------------------------------
    let target = Rect::from_xywh(200.0, 400.0, 150.0, 30.0);
    assert!(
        engine
            .set_page_object_bounds(handle, id, target)
            .expect("move"),
        "moving a live object must report success"
    );
    let moved = find_text(&objects(&mut engine, handle, 0), "Placed by the test")
        .expect("still there after the move");
    assert!(
        (moved.bounds.min_x - target.min_x).abs() < 1.0
            && (moved.bounds.min_y - target.min_y).abs() < 1.0
            && (moved.bounds.width() - target.width()).abs() < 2.0,
        "the box should follow the transform: wanted {target:?}, got {:?}",
        moved.bounds
    );

    // --- Retype -----------------------------------------------------------
    assert!(
        engine
            .set_page_object_text(
                handle,
                id,
                "Placed by the test",
                "Retyped by the test",
                moved.layout_bounds,
                moved.font_size.unwrap_or(12.0),
                TextFormat::default(),
                TextFormat::default(),
            )
            .expect("set text"),
        "retyping a text object must report success"
    );
    let list = objects(&mut engine, handle, 0);
    let retyped = find_text(&list, "Retyped by the test").expect("the new string is listed");
    assert!(
        (retyped.font_size.unwrap_or_default() - moved.font_size.unwrap_or_default()).abs() < 0.1,
        "a retype must keep the PDF's font size: before {moved:?}, after {retyped:?}"
    );
    assert!(
        (retyped.bounds.min_x - moved.bounds.min_x).abs() < 2.0
            && (retyped.bounds.min_y - moved.bounds.min_y).abs() < 2.0,
        "a retype must not move the run: before {:?}, after {:?}",
        moved.bounds,
        retyped.bounds
    );

    // Undo is the same write with the previous string. The font size stays.
    assert!(
        engine
            .set_page_object_text(
                handle,
                id,
                "Retyped by the test",
                "Placed by the test",
                moved.layout_bounds,
                moved.font_size.unwrap_or(12.0),
                TextFormat::default(),
                TextFormat::default(),
            )
            .expect("undo the retype"),
        "undoing a retype must report success"
    );
    let undone = find_text(&objects(&mut engine, handle, 0), "Placed by the test")
        .expect("undo restores the original words");
    assert!(
        (undone.font_size.unwrap_or_default() - moved.font_size.unwrap_or_default()).abs() < 0.1,
        "undo must restore the original font size too: {undone:?}"
    );
    assert!(
        engine
            .set_page_object_text(
                handle,
                id,
                "Placed by the test",
                "Retyped by the test",
                moved.layout_bounds,
                moved.font_size.unwrap_or(12.0),
                TextFormat::default(),
                TextFormat::default(),
            )
            .expect("redo the retype"),
        "redoing a retype must report success"
    );
    let list = objects(&mut engine, handle, 0);
    assert!(
        find_text(&list, "Retyped by the test").is_some(),
        "the new string must be what the list reports: {list:?}"
    );
    assert!(
        find_text(&list, "Placed by the test").is_none(),
        "the old string must be gone, not merely shadowed"
    );

    // --- Survives a save --------------------------------------------------
    // This is the step that catches a missing `regenerate_content`: the change
    // would show in the list and in a render, but never reach the file.
    let bytes = engine.save_to_bytes(handle, false).expect("save");
    let reopened = std::env::temp_dir().join("pdfreader-pageobj-roundtrip.pdf");
    std::fs::write(&reopened, &bytes).expect("write the saved document");
    let (other, _) = engine.open(&reopened, None).expect("reopen the saved file");
    let list = objects(&mut engine, other, 0);
    assert!(
        find_text(&list, "Retyped by the test").is_some(),
        "a retyped object must survive being saved and reopened: {list:?}"
    );
    engine.close(other);
    let _ = std::fs::remove_file(&reopened);

    // --- Delete -----------------------------------------------------------
    assert!(
        engine.delete_page_object(handle, id).expect("delete"),
        "deleting a live object must report success"
    );
    let list = objects(&mut engine, handle, 0);
    assert!(
        find_text(&list, "Retyped by the test").is_none(),
        "the deleted object must be gone from the list: {list:?}"
    );
}

/// A stale id must be refused rather than acting on whatever shifted into its
/// slot. Ids are positional, so this is the failure that would silently corrupt
/// an unrelated object.
#[test]
fn a_stale_or_mismatched_id_is_refused_rather_than_acted_on() {
    let Some((mut engine, handle)) = fixture() else {
        return;
    };

    let before = objects(&mut engine, handle, 0);
    let far_away = pdfreader_core::PageObjectId {
        page: 0,
        index: u32::try_from(before.len() + 50).unwrap_or(u32::MAX),
    };

    assert!(
        !engine
            .set_page_object_bounds(handle, far_away, Rect::from_xywh(0.0, 0.0, 10.0, 10.0))
            .expect("an out-of-range move is not an error"),
        "a move past the end of the list must be refused"
    );
    assert!(
        !engine
            .delete_page_object(handle, far_away)
            .expect("an out-of-range delete is not an error"),
        "a delete past the end of the list must be refused"
    );
    assert!(
        !engine
            .set_page_object_text(
                handle,
                far_away,
                "gone",
                "nope",
                Rect::from_xywh(0.0, 0.0, 10.0, 10.0),
                12.0,
                TextFormat::default(),
                TextFormat::default(),
            )
            .expect("an out-of-range retype is not an error"),
        "a retype past the end of the list must be refused"
    );

    // Nothing changed.
    let after = objects(&mut engine, handle, 0);
    assert_eq!(
        before.len(),
        after.len(),
        "a refused operation must not touch the page"
    );

    // Retyping something that is not text must be refused too. Insert a text
    // object, then ask for the index of a non-text object if the fixture has
    // one; otherwise this half is vacuous but harmless.
    let id = engine
        .insert_text_object(handle, 0, 10.0, 10.0, "x", 10.0)
        .expect("insert");
    let inserted = engine
        .page_objects(handle, 0)
        .expect("list")
        .into_iter()
        .find(|object| object.id == id)
        .expect("inserted object");
    assert!(
        engine
            .set_page_object_text(
                handle,
                id,
                "x",
                "y",
                inserted.layout_bounds,
                inserted.font_size.unwrap_or(10.0),
                TextFormat::default(),
                TextFormat::default(),
            )
            .expect("retyping a text object works"),
        "the sanity half of this test must pass, or the refusals above mean nothing"
    );
    engine.delete_page_object(handle, id).expect("clean up");
}

/// Undo of a delete re-creates the object, which renumbers the page. The redo
/// has to find it again by what it *is*, not by the index it used to have.
///
/// The test arranges for the old index to hold a *different* object before the
/// redo runs, so an implementation that trusted the index would delete the
/// wrong thing — which is exactly the corruption this re-match exists to stop.
#[test]
fn a_redone_delete_finds_the_object_by_its_record() {
    let Some((mut engine, handle)) = fixture() else {
        return;
    };

    engine
        .insert_text_object(handle, 0, 100.0, 500.0, "Find me again", 11.0)
        .expect("insert");
    let info = find_text(&objects(&mut engine, handle, 0), "Find me again").expect("listed");

    // Delete it, as the user would, then take the slot it left with something
    // else — so the recorded index now points at a different object.
    assert!(engine.delete_page_object(handle, info.id).expect("delete"));
    engine
        .insert_text_object(handle, 0, 300.0, 300.0, "Innocent bystander", 11.0)
        .expect("insert the bystander");
    let bystander = find_text(&objects(&mut engine, handle, 0), "Innocent bystander")
        .expect("the bystander is listed");
    assert_eq!(
        bystander.id.index, info.id.index,
        "the fixture must reuse the vacated slot, or this test proves nothing"
    );

    // Undo: the engine re-creates the deleted object, at a fresh index.
    engine
        .insert_text_object(
            handle,
            info.id.page,
            info.bounds.min_x,
            info.bounds.min_y,
            info.text.as_deref().unwrap_or_default(),
            info.font_size.unwrap_or(12.0),
        )
        .expect("re-create on undo");
    let restored =
        find_text(&objects(&mut engine, handle, 0), "Find me again").expect("back after the undo");
    assert_ne!(
        restored.id.index, info.id.index,
        "the re-created object must have moved, or there is nothing to re-match"
    );

    // Redo: delete by record. The recorded index now holds the bystander, so an
    // index-based delete would remove the wrong object.
    assert!(
        engine
            .delete_page_object_matching(handle, &info)
            .expect("delete by record"),
        "the record must still identify the object"
    );
    let list = objects(&mut engine, handle, 0);
    assert!(
        find_text(&list, "Find me again").is_none(),
        "the re-matched delete must remove the right object: {list:?}"
    );
    assert!(
        find_text(&list, "Innocent bystander").is_some(),
        "and must leave the object that took its index alone: {list:?}"
    );

    // A record that matches nothing is refused, not an error.
    assert!(
        !engine
            .delete_page_object_matching(handle, &info)
            .expect("a record with no match is not an error"),
        "the second attempt must report that there was nothing to remove"
    );
}

/// A replacement that cannot fit legibly is applied anyway, at the size the
/// user was looking at: refusing edits made per-glyph-run PDFs (ink boxes a few
/// points wide) impossible to retype. The text changes, the size does not, and
/// neighbouring objects keep their geometry — overflow is the user's call.
#[test]
fn an_overflowing_text_edit_is_applied_at_the_original_size() {
    let Some((mut engine, handle)) = fixture() else {
        return;
    };

    let before = objects(&mut engine, handle, 0);
    let target = before
        .iter()
        .find(|object| object.kind == PageObjectKind::Text && object.text.is_some())
        .cloned()
        .expect("fixture has a text run");
    let original = target.text.clone().expect("text run has content");
    let impossible = "W".repeat(5_000);

    assert!(
        engine
            .set_page_object_text(
                handle,
                target.id,
                &original,
                &impossible,
                target.layout_bounds,
                target.font_size.unwrap_or(12.0),
                TextFormat::default(),
                TextFormat::default(),
            )
            .expect("an overflowing retype must never be refused"),
        "the edit must report success"
    );

    let after = objects(&mut engine, handle, 0);
    let edited = after
        .iter()
        .find(|object| object.id == target.id)
        .expect("the edited run is still there");
    assert_eq!(
        edited.text.as_deref().map(str::len),
        Some(impossible.len()),
        "the user's words must be in the document"
    );
    assert!(
        (edited.font_size.unwrap_or_default() - target.font_size.unwrap_or_default()).abs() < 0.1,
        "the original size must be kept — not shrunk to something illegible: {edited:?} vs {target:?}"
    );

    for old in before.iter().filter(|object| object.id != target.id) {
        let neighbour = after
            .iter()
            .find(|object| object.id == old.id)
            .expect("a rejected edit leaves neighbours present");
        assert_eq!(neighbour.text, old.text, "neighbour text must not change");
        assert!(
            (neighbour.bounds.min_x - old.bounds.min_x).abs() < 0.5
                && (neighbour.bounds.min_y - old.bounds.min_y).abs() < 0.5
                && (neighbour.bounds.max_x - old.bounds.max_x).abs() < 0.5
                && (neighbour.bounds.max_y - old.bounds.max_y).abs() < 0.5,
            "neighbour geometry must not change: {old:?} vs {neighbour:?}"
        );
    }
}

/// Bring to front / send to back re-order the paint order — the panel's
/// professional stacking tools — and undo's exact-index restore puts the run
/// precisely where it was. Records are matched, not ids: a restack renumbers
/// the page.
#[test]
fn a_page_object_can_be_brought_to_front_and_sent_to_back() {
    let Some((mut engine, handle)) = fixture() else {
        return;
    };

    // Two runs, far apart, so the record match is unambiguous.
    let a = engine
        .insert_text_object(handle, 0, 72.0, 700.0, "Order A", 12.0)
        .expect("insert A");
    let _ = engine
        .insert_text_object(handle, 0, 300.0, 400.0, "Order B", 12.0)
        .expect("insert B");
    let list = objects(&mut engine, handle, 0);
    let record_a = list
        .iter()
        .find(|o| o.id == a)
        .cloned()
        .expect("A is listed");

    // Bring A to front: its index must land above B's.
    assert!(
        engine
            .restack_page_object(handle, &record_a, pdfreader_core::RestackTarget::Front)
            .expect("bring to front"),
        "bring to front must report success"
    );
    let after = objects(&mut engine, handle, 0);
    let a_now = find_text(&after, "Order A").expect("A survived the restack");
    let b_now = find_text(&after, "Order B").expect("B survived the restack");
    assert!(
        a_now.id.index > b_now.id.index,
        "A must now paint above B: A={:?} B={:?}",
        a_now.id,
        b_now.id
    );

    // Send A to back: below B's index.
    assert!(
        engine
            .restack_page_object(handle, &record_a, pdfreader_core::RestackTarget::Back)
            .expect("send to back"),
        "send to back must report success"
    );
    let after = objects(&mut engine, handle, 0);
    let a_now = find_text(&after, "Order A").expect("A survived the restack");
    let b_now = find_text(&after, "Order B").expect("B survived the restack");
    assert!(
        a_now.id.index < b_now.id.index,
        "A must now paint below B: A={:?} B={:?}",
        a_now.id,
        b_now.id
    );

    // Undo's exact-index restore puts A precisely where it started.
    assert!(
        engine
            .restack_page_object(
                handle,
                &record_a,
                pdfreader_core::RestackTarget::At(record_a.id.index)
            )
            .expect("restore exact index"),
        "an exact-index restack must report success"
    );
    let after = objects(&mut engine, handle, 0);
    let a_now = find_text(&after, "Order A").expect("A survived the restore");
    assert_eq!(
        a_now.id.index, record_a.id.index,
        "undo must restore the exact original index"
    );

    // Clean up with the CURRENT ids (the restacks renumbered the page).
    let a_now = find_text(&objects(&mut engine, handle, 0), "Order A").expect("A current id");
    let b_now = find_text(&objects(&mut engine, handle, 0), "Order B").expect("B current id");
    engine
        .delete_page_object(handle, a_now.id)
        .expect("delete A");
    engine
        .delete_page_object(handle, b_now.id)
        .expect("delete B");
}

/// Every page's object list must be readable, including pages with nothing the
/// editor can change — an empty list is an answer, not a failure.
#[test]
fn every_page_reports_a_list() {
    let Some((mut engine, handle)) = fixture() else {
        return;
    };
    for page in 0..3 {
        let list = objects(&mut engine, handle, page);
        assert!(
            list.iter().all(|o| o.id.page == page),
            "page {page}: an object must carry the page it came from"
        );
        assert!(
            list.iter()
                .all(|o| matches!(o.kind, PageObjectKind::Text | PageObjectKind::Image)),
            "page {page}: only text and images are offered for editing"
        );
    }
}

/// The saved bytes must be a real PDF, not an empty buffer.
#[test]
fn a_saved_document_is_written_back_intact() {
    let Some((mut engine, handle)) = fixture() else {
        return;
    };
    let bytes = engine.save_to_bytes(handle, false).expect("save");
    assert!(bytes.starts_with(b"%PDF"), "a saved document must be a PDF");
    assert!(
        bytes.len() > 1000,
        "a three-page document must not save to {} bytes",
        bytes.len()
    );
}

/// Synthetic styling — bold, a smaller point size and a colour — is applied
/// through `apply_page_object_format`, read back from the object's render mode
/// and size, and survives being saved and reopened. Colour is not asserted here
/// because PDFium cannot report a run's colour back; the shell is the authority
/// for it. Italic has no synthetic form at all: shearing the object matrix
/// climbs the baseline across a run, so it is not offered (see `TextFormat`).
#[test]
fn text_styling_is_applied_and_survives_a_save() {
    let Some((mut engine, handle)) = fixture() else {
        return;
    };

    let target = objects(&mut engine, handle, 0)
        .into_iter()
        .find(|object| object.kind == PageObjectKind::Text && object.text.is_some())
        .expect("fixture has a text run");
    let text = target.text.clone().expect("text run has content");
    let layout = target.layout_bounds;
    let size = target.font_size.unwrap_or(12.0);

    // Bold, with an explicit (smaller) size and a colour the engine cannot read
    // back. Only bold and size are verifiable through a re-list. The panel
    // always sends the *full* current format with one field toggled, so the size
    // travels with every change — a format without it would mean "back to the
    // original size", which is not what a style toggle should do.
    let styled = TextFormat {
        bold: true,
        color: Some([200, 0, 0, 255]),
        size: Some(8.0),
    };
    assert!(
        engine
            .apply_page_object_format(
                handle,
                target.id,
                layout,
                size,
                TextFormat::default(),
                styled
            )
            .expect("apply bold"),
        "applying bold must report success"
    );
    let read = find_text(&objects(&mut engine, handle, 0), &text).expect("still listed");
    assert!(read.format.bold, "the run must report bold after styling");
    assert!(
        (read.format.size.unwrap_or(0.0) - 8.0).abs() < 0.5,
        "the smaller size must be applied: {:?}",
        read.format.size
    );

    // A size INCREASE must apply as asked: shrinking increases back into the
    // run's original ink area made the panel look broken ("font size does not
    // change"). Growing into a neighbour is the user's call.
    let bigger = TextFormat {
        size: Some(24.0),
        ..Default::default()
    };
    assert!(
        engine
            .apply_page_object_format(handle, target.id, layout, size, styled, bigger)
            .expect("apply larger size"),
        "applying a larger size must report success"
    );
    let grown = find_text(&objects(&mut engine, handle, 0), &text).expect("still listed");
    assert!(
        (grown.format.size.unwrap_or(0.0) - 24.0).abs() < 0.5,
        "a deliberate larger size must NOT be shrunk back: {:?}",
        grown.format.size
    );

    // Turning bold off again restores the plain render mode.
    let plain = TextFormat {
        size: Some(8.0),
        ..Default::default()
    };
    assert!(
        engine
            .apply_page_object_format(handle, target.id, layout, size, styled, plain)
            .expect("remove bold"),
        "bold must be removable"
    );
    assert!(
        !find_text(&objects(&mut engine, handle, 0), &text)
            .expect("still listed")
            .format
            .bold,
        "the run must report plain after bold is removed"
    );

    // Bold and size must survive a save and reopen. The last style applied is
    // the plain one, so re-apply bold for the round trip.
    assert!(
        engine
            .apply_page_object_format(handle, target.id, layout, size, plain, styled)
            .expect("re-apply bold"),
        "bold must reapply"
    );
    let bytes = engine.save_to_bytes(handle, false).expect("save");
    let reopened = std::env::temp_dir().join("pdfreader-format-roundtrip.pdf");
    std::fs::write(&reopened, &bytes).expect("write the saved document");
    let (other, _) = engine.open(&reopened, None).expect("reopen the saved file");
    let saved = find_text(&objects(&mut engine, other, 0), &text).expect("present after save");
    assert!(saved.format.bold, "bold must survive a save");
    assert!(
        (saved.format.size.unwrap_or(0.0) - 8.0).abs() < 0.5,
        "the size must survive a save: {:?}",
        saved.format.size
    );
    engine.close(other);
    let _ = std::fs::remove_file(&reopened);
}
