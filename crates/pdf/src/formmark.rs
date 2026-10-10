//! Painting form marks into the page itself.
//!
//! # Why this exists
//!
//! PDFium's form model cannot record every widget value. A checkbox carries its
//! value on its own dictionary and writes back fine; a **radio group** keeps its
//! value on the *parent* field dictionary, and `pdfium-render` exposes no way to
//! reach it — `set_checked` writes the widget's own `/V`, the group still reads
//! "Off", and the click is silently lost.
//!
//! Left there, a click on such a radio would be visible on screen (the canvas
//! draws form values itself) but absent from a printed copy and absent from the
//! saved file — the user's answer would evaporate the moment they saved.
//!
//! # What it does instead
//!
//! The widget is drawn into the page **content** as ordinary vector objects: an
//! outline, plus a dot (radio) or tick (checkbox) when selected. Content is what
//! PDFium rasterizes for the screen, what the print pipeline receives, and what
//! `save_to_bytes` writes to disk — so one change makes the mark appear
//! everywhere and stay there.
//!
//! The widget's own annotation is hidden at the same time. Without that, the
//! model's stale value would still be drawn on top, and a radio group would show
//! two selections at once. Hiding it and drawing the group ourselves also means
//! every viewer — not just this one — sees a consistent group.
//!
//! The objects are recorded by their bounds so a later change can erase exactly
//! what was drawn and replace it, rather than stacking marks on top of each
//! other.

use pdfium_render::prelude::*;
use pdfreader_core::{FieldId, Rect};

use crate::Result;
use crate::error::EngineError;
use crate::export::pdfium_error;

/// Ink used for reader-painted form marks.
///
/// Deliberately not pure black: a mark that reads as "typed on the page" rather
/// than as part of the document's own artwork, and one that survives being
/// photocopied without turning into a solid blob.
const INK: PdfColor = PdfColor::new(28, 28, 28, 255);

/// How a reader-painted widget looks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Shape {
    /// A circle — a radio button.
    Radio,
    /// A square — a checkbox.
    CheckBox,
}

/// Bounds of one page object, as `[bottom, left, top, right]`.
///
/// Stored as plain numbers rather than a `PdfRect` so the record stays valid
/// after the page it came from has been dropped and re-fetched.
pub(crate) type Bounds = [f32; 4];

/// One widget the reader has painted into a page.
#[derive(Clone, Debug)]
pub(crate) struct Painted {
    /// The field the mark stands for.
    pub id: FieldId,
    /// Whether this widget carries the mark, as opposed to an empty outline.
    pub selected: bool,
    /// Bounds of every object drawn, so they can be removed again.
    pub objects: Vec<Bounds>,
}

/// Draw one widget into the page content and hide its own annotation.
///
/// Returns the bounds of the objects added, in the order they were drawn.
pub(crate) fn paint(
    document: &mut PdfDocument<'_>,
    page_index: u32,
    annot_index: u32,
    rect: Rect,
    shape: Shape,
    selected: bool,
) -> Result<Vec<Bounds>> {
    let pages = document.pages_mut();
    let mut page = pages
        .get(page_index as PdfPageIndex)
        .map_err(pdfium_error)?;

    // Take the widget's own appearance out of the picture: PDFium would draw
    // the model's stale value there, which for a radio group means the *old*
    // selection showing through alongside ours.
    if let Ok(mut annotation) = page
        .annotations_mut()
        .get(annot_index as PdfPageAnnotationIndex)
    {
        let _ = annotation.set_is_hidden(true);
    }

    let mut drawn = Vec::new();
    {
        let objects = page.objects_mut();
        // Stroke weight follows the widget, so a small box does not end up
        // looking like a solid block and a large one does not look hairline.
        let line = PdfPoints::new((rect.width().min(rect.height()) * 0.08).clamp(0.5, 1.6));
        match shape {
            Shape::Radio => {
                let radius = rect.width().min(rect.height()) / 2.0 * 0.94;
                let (cx, cy) = (
                    (rect.min_x + rect.max_x) / 2.0,
                    (rect.min_y + rect.max_y) / 2.0,
                );
                drawn.push(bounds_of(
                    objects
                        .create_path_object_circle_at(
                            PdfPoints::new(cx),
                            PdfPoints::new(cy),
                            PdfPoints::new(radius),
                            Some(INK),
                            Some(line),
                            None,
                        )
                        .map_err(pdfium_error)?,
                )?);
                if selected {
                    drawn.push(bounds_of(
                        objects
                            .create_path_object_circle_at(
                                PdfPoints::new(cx),
                                PdfPoints::new(cy),
                                PdfPoints::new(radius * 0.52),
                                None,
                                None,
                                Some(INK),
                            )
                            .map_err(pdfium_error)?,
                    )?);
                }
            }
            Shape::CheckBox => {
                drawn.push(bounds_of(
                    objects
                        .create_path_object_rect(pdf_rect(rect), Some(INK), Some(line), None)
                        .map_err(pdfium_error)?,
                )?);
                if selected {
                    // A tick, drawn as two strokes meeting at the elbow.
                    let (w, h) = (rect.width(), rect.height());
                    let elbow = (rect.min_x + w * 0.42, rect.min_y + h * 0.24);
                    let start = (rect.min_x + w * 0.20, rect.min_y + h * 0.52);
                    let end = (rect.min_x + w * 0.80, rect.min_y + h * 0.78);
                    for (from, to) in [(start, elbow), (elbow, end)] {
                        drawn.push(bounds_of(
                            objects
                                .create_path_object_line(
                                    PdfPoints::new(from.0),
                                    PdfPoints::new(from.1),
                                    PdfPoints::new(to.0),
                                    PdfPoints::new(to.1),
                                    INK,
                                    line,
                                )
                                .map_err(pdfium_error)?,
                        )?);
                    }
                }
            }
        }
    }

    page.regenerate_content().map_err(pdfium_error)?;
    Ok(drawn)
}

/// Remove page objects previously drawn at the given bounds.
///
/// Matching is by exact rectangle rather than by index: an index would shift as
/// soon as any other object is removed, and this runs after the page has been
/// re-fetched, so there is nothing left to correlate with but the geometry.
pub(crate) fn erase(
    document: &mut PdfDocument<'_>,
    page_index: u32,
    bounds: &[Bounds],
) -> Result<()> {
    if bounds.is_empty() {
        return Ok(());
    }

    let pages = document.pages_mut();
    let mut page = pages
        .get(page_index as PdfPageIndex)
        .map_err(pdfium_error)?;

    let mut hits = Vec::new();
    {
        let objects = page.objects();
        for index in 0..objects.len() {
            let Ok(object) = objects.get(index) else {
                continue;
            };
            if object.object_type() != PdfPageObjectType::Path {
                continue;
            }
            let Ok(quad) = object.bounds() else {
                continue;
            };
            let here = bounds_of_rect(quad.to_rect());
            if bounds.iter().any(|wanted| same(*wanted, here)) {
                hits.push(index);
            }
        }
    }

    // Highest index first, so the earlier indices stay valid as we go.
    for index in hits.into_iter().rev() {
        let _ = page.objects_mut().remove_object_at_index(index);
    }
    page.regenerate_content().map_err(pdfium_error)?;
    Ok(())
}

/// Whether the reader has painted this widget into the page, and whether it
/// carries the mark.
///
/// A document that has been saved and re-opened has no memory of what the
/// reader drew, and the model still reports the value the form shipped with.
/// The marks in the page are therefore the only record of the user's answer:
/// an outline at the widget's rectangle means the reader owns this widget, and
/// a mark *inside* that rectangle means it is the selected one.
pub(crate) fn painted_state(page: &PdfPage<'_>, rect: Rect) -> Option<bool> {
    let (width, height) = (rect.width(), rect.height());
    if width <= 0.0 || height <= 0.0 {
        return None;
    }

    // The outline is drawn at very nearly the widget's own size; the mark is
    // drawn well inside it. Comparing the two sizes separates them without
    // having to remember which object was drawn first — and without depending
    // on exact geometry, which the PDF round trip is free to round.
    const FULL: f32 = 0.7;

    let objects = page.objects();
    let mut outlined = false;
    let mut marked = false;

    for object in objects.iter() {
        if object.object_type() != PdfPageObjectType::Path {
            continue;
        }
        let Ok(quad) = object.bounds() else {
            continue;
        };
        let here = bounds_of_rect(quad.to_rect());

        // Containment is judged by the object's centre, not its edges: a stroke
        // grows an object a little past the geometry it was given, so the
        // outline's bounds sit just outside the widget's rectangle.
        let centre = ((here[1] + here[3]) / 2.0, (here[0] + here[2]) / 2.0);
        if !rect.contains(centre.0, centre.1) {
            continue;
        }

        let (object_width, object_height) = (here[3] - here[1], here[2] - here[0]);
        if object_width >= width * FULL && object_height >= height * FULL {
            outlined = true;
        } else {
            marked = true;
        }
    }

    outlined.then_some(marked)
}

/// Bounds of a page object as `[bottom, left, top, right]`.
fn bounds_of(object: PdfPageObject<'_>) -> Result<Bounds> {
    let quad = object.bounds().map_err(pdfium_error)?;
    Ok(bounds_of_rect(quad.to_rect()))
}

/// `[bottom, left, top, right]` of a PDFium rectangle.
fn bounds_of_rect(rect: PdfRect) -> Bounds {
    [
        rect.bottom().value,
        rect.left().value,
        rect.top().value,
        rect.right().value,
    ]
}

/// Whether two sets of bounds are the same rectangle, within a hair.
fn same(a: Bounds, b: Bounds) -> bool {
    a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < 0.05)
}

/// Convert a core rectangle into PDFium's, whose constructor takes
/// `(bottom, left, top, right)`.
pub(crate) fn pdf_rect(rect: Rect) -> PdfRect {
    PdfRect::new(
        PdfPoints::new(rect.min_y),
        PdfPoints::new(rect.min_x),
        PdfPoints::new(rect.max_y),
        PdfPoints::new(rect.max_x),
    )
}

/// A shape that could not be painted because the page refused it.
///
/// Kept separate from `EngineError` so callers can decide whether a missing mark
/// is worth reporting; in practice it is not, because the value is still held by
/// the form model and shown by the canvas.
#[allow(dead_code)]
pub(crate) fn unsupported(what: &str) -> EngineError {
    EngineError::Unsupported(what.to_string())
}
