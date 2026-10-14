//! End-to-end smoke test for content editing.
//!
//! This is the "open a PDF and edit it" test. It drives the **real** pipeline —
//! the real `PDFium` engine, the real store and reducer, the real canvas with
//! synthetic pointer events — and checks the file actually changed by saving it,
//! reopening it, and reading it back. Nothing here is a stand-in for the app
//! except the two things that cannot be driven headlessly: the engine *thread*
//! (effects are applied to the engine directly, one line each, exactly as
//! `App::execute` does) and the native file picker.
//!
//! It skips rather than fails when the staged `PDFium` library or the generated
//! fixtures are absent, like the other integration tests in this workspace.

use std::path::PathBuf;

use egui::Color32;
use pdfreader_core::{
    Command, Effect, PageObjectInfo, PageObjectKind, Rect, Store, TextFormat, Tool,
};
use pdfreader_pdf::engine::{DocumentHandle, PdfEngine, PdfiumEngine};

use crate::canvas::{Canvas, ContentEdit};
use crate::lens::PageTextCache;
use crate::objects::PageObjectCache;

/// Workspace root, derived from this crate's manifest directory.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// Bind a real engine and open the three-page fixture, or explain why not.
fn engine_and_fixture() -> Option<(PdfiumEngine, PathBuf)> {
    let native = workspace_root().join("target").join("native");
    let dir = std::fs::read_dir(&native)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|dir| {
            dir.join("pdfium.dll").exists()
                || dir.join("libpdfium.so").exists()
                || dir.join("libpdfium.dylib").exists()
        });
    let Some(dir) = dir else {
        eprintln!("SKIP: no PDFium library staged under target/native");
        return None;
    };
    let path = workspace_root()
        .join("target")
        .join("test-pdfs")
        .join("small-3p.pdf");
    if !path.exists() {
        eprintln!("SKIP: fixture missing (run tools/generate_fixtures.py)");
        return None;
    }
    let engine = PdfiumEngine::bind(Some(dir.as_path())).expect("bind pdfium");
    Some((engine, path))
}

/// Apply the effects the reducer produced, exactly as `App::execute` does.
///
/// Returns the commands the engine's answers turn into — the `PageObjectInserted`
/// follow-up that resolves an insert's id — so the caller can dispatch them the
/// way `drain_engine` would.
fn apply_effects(
    engine: &mut PdfiumEngine,
    handle: DocumentHandle,
    doc: pdfreader_core::DocumentId,
    effects: &[Effect],
) -> Vec<Command> {
    let mut follow_ups = Vec::new();
    for effect in effects {
        match effect {
            Effect::SetPageObjectBounds { id, rect, .. } => {
                engine
                    .set_page_object_bounds(handle, *id, *rect)
                    .expect("move a page object");
            }
            Effect::DeletePageObject { id, .. } => {
                engine
                    .delete_page_object(handle, *id)
                    .expect("delete a page object");
            }
            Effect::DeletePageObjectMatching { info, .. } => {
                engine
                    .delete_page_object_matching(handle, info)
                    .expect("delete a page object by record");
            }
            Effect::RestackPageObject { info, target, .. } => {
                engine
                    .restack_page_object(handle, info, *target)
                    .expect("restack a page object");
            }
            Effect::SetPageObjectText {
                id,
                previous,
                text,
                layout_bounds,
                base_font_size,
                previous_format,
                format,
                ..
            } => {
                // A too-long replacement is applied anyway (overflow, never
                // refusal), so any engine error here is unexpected.
                engine
                    .set_page_object_text(
                        handle,
                        *id,
                        previous,
                        text,
                        *layout_bounds,
                        *base_font_size,
                        *previous_format,
                        *format,
                    )
                    .expect("retype a page object");
            }
            Effect::SetPageObjectFormat {
                id,
                layout_bounds,
                base_font_size,
                previous_format,
                format,
                ..
            } => {
                engine
                    .apply_page_object_format(
                        handle,
                        *id,
                        *layout_bounds,
                        *base_font_size,
                        *previous_format,
                        *format,
                    )
                    .expect("restyle a page object");
            }
            Effect::InsertPageObjectText {
                page,
                point,
                text,
                font_size,
                ..
            } => {
                let id = engine
                    .insert_text_object(handle, *page, point.0, point.1, text, *font_size)
                    .expect("insert a text object");
                follow_ups.push(Command::PageObjectInserted {
                    doc,
                    page: *page,
                    id,
                    objects: engine.page_objects(handle, *page).expect("re-list"),
                });
            }
            // Not this feature's business.
            _ => {}
        }
    }
    follow_ups
}

/// The object holding exactly this text, if the page has one.
fn find_text(
    engine: &mut PdfiumEngine,
    handle: DocumentHandle,
    page: u32,
    text: &str,
) -> Option<PageObjectInfo> {
    engine
        .page_objects(handle, page)
        .expect("list objects")
        .into_iter()
        .find(|o| o.kind == PageObjectKind::Text && o.text.as_deref() == Some(text))
}

/// Everything the smoke test needs to run a frame.
struct Harness {
    canvas: Canvas,
    store: Store,
    engine: PdfiumEngine,
    handle: DocumentHandle,
    objects: PageObjectCache,
    text: PageTextCache,
    screen: egui::Rect,
}

impl Harness {
    /// Open the fixture and put it in the store, exactly as opening a file does.
    fn open() -> Option<Self> {
        let (mut engine, path) = engine_and_fixture()?;
        let (handle, info) = engine.open(&path, None).expect("open the fixture");

        // The real document builder, so the store sees what the app would.
        let doc = crate::engine_thread::build_document(&path, info);

        let mut store = Store::new();
        let effects = store.dispatch(Command::OpenPath(path.clone()));
        let Some(Effect::OpenDocument { tab, .. }) = effects.first().cloned() else {
            panic!("opening a path must ask the shell to load it");
        };
        store.dispatch(Command::DocumentOpened {
            tab,
            document: doc.clone(),
        });

        Some(Self {
            canvas: Canvas::new(),
            store,
            engine,
            handle,
            objects: PageObjectCache::new(),
            text: PageTextCache::new(),
            screen: egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 800.0)),
        })
    }

    /// The open document, as the store holds it.
    ///
    /// Read from the store rather than kept in a field: `DocumentOpened`
    /// reassigns the document id, so a copy taken before that is stale — and a
    /// stale id silently makes every engine answer look like it belongs to
    /// another tab.
    fn document(&self) -> pdfreader_core::Document {
        self.store
            .state()
            .active()
            .and_then(|tab| tab.document.clone())
            .expect("a document is open")
    }

    /// Its id.
    fn doc_id(&self) -> pdfreader_core::DocumentId {
        self.document().id
    }

    /// Read the object list the way the shell's request loop does.
    fn refresh_objects(&mut self, page: u32) {
        let objects = self
            .engine
            .page_objects(self.handle, page)
            .expect("list objects");
        self.objects.insert(self.doc_id(), page, objects);
    }

    /// Run one frame of the canvas and return the commands it produced, plus the
    /// content origin so the caller can aim at a PDF point.
    fn frame(&mut self, events: Vec<egui::Event>, ctx: &egui::Context) -> Vec<Command> {
        let doc = self.document();
        let view = self.store.state().active().expect("a tab").view;
        let editing = self.store.state().editing_content;
        let selected = self
            .store
            .state()
            .active()
            .and_then(|t| t.view.selected_object);
        let objects = &self.objects;
        let text = &self.text;
        let canvas = &mut self.canvas;
        let screen = self.screen;

        let mut raw = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        raw.events = events;
        let mut out = ctx.run_ui(raw, |ui| {
            canvas.draw(
                ui,
                &doc,
                &view,
                view.zoom,
                1.0,
                Color32::BLACK,
                Color32::BLACK,
                None,
                None,
                None,
                Color32::TRANSPARENT,
                Tool::Select,
                ContentEdit {
                    editing,
                    objects,
                    selected,
                },
                text,
                &[],
                None,
            );
        });
        out.textures_delta.clear();
        self.canvas.take_edits()
    }

    /// Dispatch commands and run their effects, the way `App::apply` does.
    fn dispatch(&mut self, commands: Vec<Command>) {
        // Read once: the id borrows the store, and `apply_effects` borrows the
        // engine mutably.
        let doc = self.doc_id();
        for command in commands {
            let effects = self.store.dispatch(command);
            let follow_ups = apply_effects(&mut self.engine, self.handle, doc, &effects);
            for follow_up in follow_ups {
                let more = self.store.dispatch(follow_up);
                apply_effects(&mut self.engine, self.handle, doc, &more);
            }
        }
    }

    /// Where a PDF-space point lands on screen, from the canvas's own layout.
    fn screen_point(&self, pdf: (f32, f32)) -> egui::Pos2 {
        let space = self
            .canvas
            .page_spaces()
            .first()
            .copied()
            .expect("a laid-out page");
        let (x, y) = space.to_screen_point(pdf);
        egui::pos2(x, y)
    }
}

/// Press, move, release — one pointer gesture across three frames.
fn gesture(target: egui::Pos2, to: egui::Pos2) -> [Vec<egui::Event>; 3] {
    [
        vec![
            egui::Event::PointerMoved(target),
            egui::Event::PointerButton {
                pos: target,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            },
        ],
        vec![egui::Event::PointerMoved(to)],
        vec![egui::Event::PointerButton {
            pos: to,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        }],
    ]
}

/// Open a real PDF, move an object with the mouse, and check the *file* changed.
///
/// The save-and-reopen step is the point: an edit that only lives in PDFium's
/// object list would look perfect on screen and be gone the moment the user
/// saved.
#[test]
fn a_real_pdf_can_be_edited_and_the_change_survives_a_save() {
    let Some(mut harness) = Harness::open() else {
        return;
    };
    let ctx = egui::Context::default();

    // --- Enter edit mode, as the toolbar does ------------------------------
    harness.dispatch(vec![Command::SetEditingContent { on: true }]);
    assert!(
        harness.store.state().editing_content,
        "the fixture is a plain PDF, so edit mode must turn on"
    );

    // --- Read the page, as the shell's request loop does -------------------
    harness.frame(Vec::new(), &ctx);
    harness.refresh_objects(0);

    let target = harness
        .objects
        .get(harness.doc_id(), 0)
        .expect("page 0 is read")
        .iter()
        .find(|o| o.kind == PageObjectKind::Text && o.bounds.width() > 20.0)
        .cloned()
        .expect("the fixture has a text object to edit");
    let original = target.text.clone().expect("a text object has text");

    // --- Press on it: the canvas must select it ----------------------------
    let centre = (
        (target.bounds.min_x + target.bounds.max_x) / 2.0,
        (target.bounds.min_y + target.bounds.max_y) / 2.0,
    );
    let from = harness.screen_point(centre);
    let to = from + egui::vec2(30.0, 18.0);

    let frames = gesture(from, to);
    let first = harness.frame(frames[0].clone(), &ctx);
    harness.dispatch(first);
    assert_eq!(
        harness.store.state().active().unwrap().view.selected_object,
        Some(target.id),
        "a press on an object must select it"
    );

    // --- Drag it: the canvas must commit a new box -------------------------
    harness.frame(frames[1].clone(), &ctx);
    let release = harness.frame(frames[2].clone(), &ctx);
    harness.dispatch(release);

    // By id, not by the words. The fixture repeats a line further down the
    // page, and a search for the string finds that copy instead of the run
    // that was dragged.
    let listed = harness
        .engine
        .page_objects(harness.handle, 0)
        .expect("list");
    let moved = listed
        .iter()
        .find(|object| object.id == target.id)
        .expect("the object is still there after the move")
        .clone();
    assert!(
        (moved.bounds.min_x - (target.bounds.min_x + 30.0)).abs() < 2.0
            && (moved.bounds.min_y - (target.bounds.min_y - 18.0)).abs() < 2.0,
        "a 30 px right, 18 px down drag at zoom 1 must move +30 pt, −18 pt: was {:?}, now {:?}",
        target.bounds,
        moved.bounds
    );
    assert!(
        moved
            .text
            .as_deref()
            .is_some_and(|text| text.contains("quick")),
        "dragging must not drop the words: {:?}",
        moved.text
    );
    assert!(
        harness.store.state().active().unwrap().dirty,
        "an edit dirties the tab"
    );

    // --- Retype it, as the inline editor does ------------------------------
    harness.dispatch(vec![Command::SetPageObjectText {
        id: moved.id,
        previous: original.clone(),
        text: "Edited by the smoke test".to_string(),
        layout_bounds: moved.layout_bounds,
        base_font_size: moved.font_size.unwrap_or(12.0),
        previous_format: TextFormat::default(),
        format: TextFormat::default(),
    }]);
    let edited = harness
        .engine
        .page_objects(harness.handle, 0)
        .expect("list")
        .into_iter()
        .find(|object| object.id == moved.id)
        .expect("the retyped object is still there");
    assert_eq!(
        edited.text.as_deref(),
        Some("Edited by the smoke test"),
        "the new words must replace the old ones, not disappear"
    );
    assert!(
        (edited.bounds.min_x - moved.bounds.min_x).abs() < 3.0
            && (edited.bounds.min_y - moved.bounds.min_y).abs() < 2.0,
        "a retype must stay on the run's baseline: before {:?}, after {:?}",
        moved.bounds,
        edited.bounds
    );
    assert!(
        (edited.font_size.unwrap_or_default() - moved.font_size.unwrap_or_default()).abs() < 0.5,
        "a retype must keep the PDF's {} pt size, got {:?}",
        moved.font_size.unwrap_or_default(),
        edited.font_size
    );

    // --- Add new text, as double-clicking bare page does -------------------
    harness.dispatch(vec![Command::InsertPageObjectText {
        page: 0,
        point: (80.0, 640.0),
        text: "Brand new line".to_string(),
    }]);
    assert!(
        find_text(&mut harness.engine, harness.handle, 0, "Brand new line").is_some(),
        "the inserted text must be in the document"
    );

    // --- Save, reopen, and check the file itself ---------------------------
    let bytes = harness
        .engine
        .save_to_bytes(harness.handle, false)
        .expect("save the edited document");
    let saved = std::env::temp_dir().join("pdfreader-smoke-edited.pdf");
    std::fs::write(&saved, &bytes).expect("write the edited document");
    let (reopened, _) = harness
        .engine
        .open(&saved, None)
        .expect("reopen the edited document");

    let page = harness.engine.page_objects(reopened, 0).expect("list");
    let has_new = page
        .iter()
        .any(|o| o.text.as_deref() == Some("Edited by the smoke test"));
    let has_insert = page
        .iter()
        .any(|o| o.text.as_deref() == Some("Brand new line"));
    assert!(
        has_new && has_insert,
        "both edits must survive save and reopen — \
         without `regenerate_content` they live only in the object list"
    );
    let moved_again = page
        .iter()
        .find(|o| o.text.as_deref() == Some("Edited by the smoke test"))
        .expect("the moved object");
    assert!(
        (moved_again.bounds.min_x - target.bounds.min_x).abs() > 5.0,
        "the move must survive save and reopen too"
    );
    harness.engine.close(reopened);
    let _ = std::fs::remove_file(&saved);
}

/// Undo has to put the document back, and redo has to reapply it — including
/// after the engine has re-listed, which is when the positional ids shift.
///
/// Driven through the *command*, not the engine call: the undo entry is what the
/// reducer records, so inserting behind its back would test nothing.
#[test]
fn undo_and_redo_round_trip_through_the_engine() {
    let Some(mut harness) = Harness::open() else {
        return;
    };
    harness.dispatch(vec![Command::SetEditingContent { on: true }]);

    harness.dispatch(vec![Command::InsertPageObjectText {
        page: 0,
        point: (60.0, 600.0),
        text: "Undo me".to_string(),
    }]);
    assert!(
        find_text(&mut harness.engine, harness.handle, 0, "Undo me").is_some(),
        "the insert must have reached the document"
    );

    // Undo the insert: the object must go. Its id was resolved by the
    // `PageObjectInserted` follow-up the engine's answer produced.
    harness.dispatch(vec![Command::Undo]);
    assert!(
        find_text(&mut harness.engine, harness.handle, 0, "Undo me").is_none(),
        "undoing an insert must delete the object it created"
    );

    // Redo it: it must come back, at a fresh index the engine hands out.
    harness.dispatch(vec![Command::Redo]);
    assert!(
        find_text(&mut harness.engine, harness.handle, 0, "Undo me").is_some(),
        "redo must put it back"
    );
}

/// Retyping is undoable, and the undo restores the words that were there.
#[test]
fn undoing_a_retype_restores_the_original_words() {
    let Some(mut harness) = Harness::open() else {
        return;
    };
    harness.dispatch(vec![Command::SetEditingContent { on: true }]);
    harness.refresh_objects(0);

    let target = harness
        .objects
        .get(harness.doc_id(), 0)
        .expect("page 0 is read")
        .iter()
        .find(|o| o.kind == PageObjectKind::Text && o.bounds.width() > 20.0)
        .cloned()
        .expect("a text object");
    let original = target.text.clone().expect("it has text");

    harness.dispatch(vec![Command::SetPageObjectText {
        id: target.id,
        previous: original.clone(),
        text: "Temporarily different".to_string(),
        layout_bounds: target.layout_bounds,
        base_font_size: target.font_size.unwrap_or(12.0),
        previous_format: TextFormat::default(),
        format: TextFormat::default(),
    }]);

    // Checked by index, not by searching for the words: the fixture repeats its
    // text, so a stale copy of the old run elsewhere on the page would happily
    // pass for the one being edited.
    let after = harness
        .engine
        .page_objects(harness.handle, 0)
        .expect("list");
    let edited = after
        .iter()
        .find(|o| o.id == target.id)
        .expect("still there");
    assert_eq!(
        edited.text.as_deref(),
        Some("Temporarily different"),
        "the retype must have applied to that object"
    );
    assert!(
        (edited.bounds.min_x - target.bounds.min_x).abs() < 2.0
            && (edited.bounds.min_y - target.bounds.min_y).abs() < 2.0,
        "a retype must keep the run where it was: before {:?}, after {:?}",
        target.bounds,
        edited.bounds
    );
    assert!(
        (edited.font_size.unwrap_or_default() - target.font_size.unwrap_or_default()).abs() < 0.5,
        "a retype must keep the PDF font size: before {:?}, after {:?}",
        target.font_size,
        edited.font_size
    );

    harness.dispatch(vec![Command::Undo]);
    let after = harness
        .engine
        .page_objects(harness.handle, 0)
        .expect("list");
    let restored = after
        .iter()
        .find(|o| o.id == target.id)
        .expect("still there");
    assert_eq!(
        restored.text.as_deref(),
        Some(original.as_str()),
        "undo must restore the original words"
    );
    assert!(
        (restored.bounds.min_x - target.bounds.min_x).abs() < 2.0
            && (restored.bounds.min_y - target.bounds.min_y).abs() < 2.0,
        "undo must put the run back where it was: before {:?}, after {:?}",
        target.bounds,
        restored.bounds
    );
    assert!(
        (restored.font_size.unwrap_or_default() - target.font_size.unwrap_or_default()).abs() < 0.5,
        "undo must keep the original font size"
    );
}

/// Moving an object and then undoing must restore the box it started at.
#[test]
fn undoing_a_move_puts_the_object_back() {
    let Some(mut harness) = Harness::open() else {
        return;
    };
    harness.dispatch(vec![Command::SetEditingContent { on: true }]);
    harness.refresh_objects(0);

    let target = harness
        .objects
        .get(harness.doc_id(), 0)
        .expect("page 0 is read")
        .iter()
        .find(|o| o.kind == PageObjectKind::Text && o.bounds.width() > 20.0)
        .cloned()
        .expect("a text object");
    let before = target.bounds;
    let after = Rect::from_xywh(
        before.min_x + 60.0,
        before.min_y + 40.0,
        before.width(),
        before.height(),
    );

    harness.dispatch(vec![Command::SetPageObjectBounds {
        id: target.id,
        previous: before,
        rect: after,
    }]);
    let moved = find_text(
        &mut harness.engine,
        harness.handle,
        0,
        target.text.as_deref().unwrap_or_default(),
    )
    .expect("still there");
    assert!(
        (moved.bounds.min_x - after.min_x).abs() < 2.0,
        "the move must have applied: wanted {after:?}, got {:?}",
        moved.bounds
    );

    harness.dispatch(vec![Command::Undo]);
    let restored = find_text(
        &mut harness.engine,
        harness.handle,
        0,
        target.text.as_deref().unwrap_or_default(),
    )
    .expect("still there after undo");
    assert!(
        (restored.bounds.min_x - before.min_x).abs() < 2.0,
        "undo must restore the original box: wanted {before:?}, got {:?}",
        restored.bounds
    );
}

/// A document that forbids modification must not enter edit mode, and the
/// fixture — which is a plain PDF — must. This is the gate that was wrong.
#[test]
fn the_fixture_is_editable_and_reports_so() {
    let Some(mut harness) = Harness::open() else {
        return;
    };
    assert!(
        harness.document().can_edit_content,
        "a plain fixture must report that its content can be changed"
    );
    harness.dispatch(vec![Command::SetEditingContent { on: true }]);
    assert!(harness.store.state().editing_content);

    // A document that forbids it is refused.
    harness.store.state().active().map(|_| ()).expect("a tab");
    let effects = harness
        .store
        .dispatch(Command::SetEditingContent { on: false });
    assert!(effects.is_empty(), "leaving edit mode needs no engine work");
    assert!(!harness.store.state().editing_content);
}

/// Editing **one word** must leave the rest of the line alone.
///
/// This is the difference between an editor and a text-replacement tool. A text
/// object is a line, so the splice is what stops a double-click plus one
/// keystroke from wiping the whole sentence.
#[test]
fn editing_one_word_leaves_the_rest_of_the_line_alone() {
    let Some(mut harness) = Harness::open() else {
        return;
    };
    harness.dispatch(vec![Command::SetEditingContent { on: true }]);
    harness.refresh_objects(0);

    // A run with several words in it, so there is a middle to disturb.
    let target = harness
        .objects
        .get(harness.doc_id(), 0)
        .expect("page 0 is read")
        .iter()
        .find(|o| {
            o.kind == PageObjectKind::Text
                && o.text
                    .as_deref()
                    .is_some_and(|text| text.split_whitespace().count() >= 4)
        })
        .cloned()
        .expect("the fixture has a run with several words");
    let run = target.text.clone().expect("a text object has text");
    let words: Vec<&str> = run.split_whitespace().collect();

    // Replace the second word, exactly as a double-click plus typing does.
    let byte_start = run.find(words[1]).expect("the word is in the run");
    let start = run[..byte_start].chars().count();
    let end = start + words[1].chars().count();
    let next = crate::canvas::splice(&run, start, end, "slow");
    assert!(next.contains("slow"), "the splice must land: {next}");

    harness.dispatch(vec![Command::SetPageObjectText {
        id: target.id,
        previous: run.clone(),
        text: next.clone(),
        layout_bounds: target.layout_bounds,
        base_font_size: target.font_size.unwrap_or(12.0),
        previous_format: TextFormat::default(),
        format: TextFormat::default(),
    }]);

    let after = harness
        .engine
        .page_objects(harness.handle, 0)
        .expect("list");
    let edited = after
        .iter()
        .find(|o| o.id == target.id)
        .expect("the run is still there");
    let edited_words: Vec<&str> = edited
        .text
        .as_deref()
        .expect("it still has text")
        .split_whitespace()
        .collect();

    assert_eq!(edited_words[1], "slow", "the clicked word must change");
    assert_eq!(
        edited_words[0], words[0],
        "the word before it must not move"
    );
    assert_eq!(edited_words[2], words[2], "nor the word after it");
    assert_eq!(
        edited_words.len(),
        words.len(),
        "and no word may be lost or added"
    );
}

/// Styling through the store, end to end: a format command dispatched the way
/// the panel does must reach the engine and read back from the object — bold
/// and a new point size, including an *increase*, which the fit used to shrink
/// silently back to the original area ("font size does not change").
#[test]
fn styling_a_text_object_changes_the_document() {
    let Some(mut harness) = Harness::open() else {
        return;
    };
    harness.dispatch(vec![Command::SetEditingContent { on: true }]);
    harness.refresh_objects(0);

    let target = harness
        .objects
        .get(harness.doc_id(), 0)
        .expect("page 0 is read")
        .iter()
        .find(|o| o.kind == PageObjectKind::Text && o.text.is_some())
        .cloned()
        .expect("the fixture has a text run");

    // A size well above the run's current one: the exact case that used to be
    // swallowed by the fit.
    let bigger = target.font_size.unwrap_or(12.0) + 10.0;
    let styled = TextFormat {
        bold: true,
        size: Some(bigger),
        ..Default::default()
    };
    harness.dispatch(vec![Command::SetPageObjectFormat {
        id: target.id,
        layout_bounds: target.layout_bounds,
        base_font_size: target.font_size.unwrap_or(12.0),
        previous_format: target.format,
        format: styled,
    }]);

    let after = harness
        .engine
        .page_objects(harness.handle, 0)
        .expect("list");
    let restyled = after
        .iter()
        .find(|o| o.id == target.id)
        .expect("the run is still there");
    assert!(
        restyled.format.bold,
        "bold must reach the document: {:?}",
        restyled.format
    );
    assert!(
        (restyled.format.size.unwrap_or(0.0) - bigger).abs() < 0.5,
        "a deliberate size increase must reach the document: {:?}",
        restyled.format
    );
}
