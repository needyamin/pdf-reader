//! Annotation creation, enumeration and editing behind the engine seam.
//!
//! The domain types live in `pdfreader_core::annotation`; this module is the
//! only place that knows how PDFium represents annotations. Two quirks shape
//! the code:
//!
//! * Markup annotations (highlight, underline and friends) are positioned by
//!   **quad points**, not by their bounds — the bounds follow from the quads.
//!   So creating one means setting quads, and the rect is written as a single
//!   quad covering the dragged area.
//! * Every mutation is followed by `regenerate_content()`. Tile rendering runs
//!   on a separate request path, so without an explicit regeneration the old
//!   appearance could still be baked into cached tiles after a change.

use pdfium_render::prelude::{
    PdfColor, PdfDocument, PdfFormFieldCommon as _, PdfPageAnnotationCommon, PdfPageAnnotationType,
    PdfPoints, PdfQuadPoints,
};

use pdfreader_core::{AnnotationId, AnnotationInfo, AnnotationKind, NewAnnotation, Rect};

/// Read every non-widget annotation in the document, in page order.
///
/// Widget annotations are deliberately excluded: they are form fields, and the
/// Forms panel already lists those. Popup annotations are excluded too because
/// they are the pop-up children of sticky notes rather than content in their
/// own right.
pub(crate) fn list_annotations(document: &PdfDocument<'_>) -> Vec<AnnotationInfo> {
    let mut out = Vec::new();
    let pages = document.pages();
    let page_count = pages.len();

    for page_index in 0..page_count {
        let Ok(page) = pages.get(page_index) else {
            continue;
        };
        for (annot_index, annotation) in page.annotations().iter().enumerate() {
            let kind = classify(annotation.annotation_type());
            if matches!(kind, AnnotationKind::Other) && is_hidden_type(annotation.annotation_type())
            {
                continue;
            }
            let Ok(bounds) = annotation.bounds() else {
                continue;
            };
            // The stroke colour is what markup, rectangle and note annotations
            // draw with. pdfium-render (vendored, patched) returns an error —
            // never crashes — when the annotation has no colour entry, so an
            // unconditional read is safe: kinds without a colour simply report
            // `None`. See the [patch.crates-io] note in the workspace
            // Cargo.toml for why the vendored copy exists.
            let color = annotation
                .stroke_color()
                .ok()
                .map(|c| [c.red(), c.green(), c.blue(), c.alpha()])
                // Pdfium will not report a colour for an annotation that has an
                // appearance stream, so a line markup we drew has to be asked
                // for the colour it recorded in that stream.
                .or_else(|| recorded_color(&annotation));
            out.push(AnnotationInfo {
                id: AnnotationId::new(page_index as u32, annot_index as u32),
                kind,
                rect: Rect::from_xywh(
                    bounds.left().value,
                    bounds.bottom().value,
                    bounds.width().value,
                    bounds.height().value,
                ),
                contents: annotation.contents().filter(|c| !c.is_empty()),
                color,
            });
        }
    }

    out
}

/// Map PDFium's annotation type to our domain kind.
fn classify(annotation_type: PdfPageAnnotationType) -> AnnotationKind {
    match annotation_type {
        PdfPageAnnotationType::Highlight => AnnotationKind::Highlight,
        PdfPageAnnotationType::Underline => AnnotationKind::Underline,
        PdfPageAnnotationType::Strikeout => AnnotationKind::StrikeOut,
        PdfPageAnnotationType::Squiggly => AnnotationKind::Squiggly,
        PdfPageAnnotationType::Square | PdfPageAnnotationType::Circle => AnnotationKind::Square,
        PdfPageAnnotationType::Text => AnnotationKind::StickyNote,
        PdfPageAnnotationType::FreeText => AnnotationKind::FreeText,
        PdfPageAnnotationType::Stamp => AnnotationKind::Stamp,
        PdfPageAnnotationType::Link => AnnotationKind::Link,
        _ => AnnotationKind::Other,
    }
}

/// Types that carry no user-visible content of their own.
fn is_hidden_type(annotation_type: PdfPageAnnotationType) -> bool {
    matches!(
        annotation_type,
        PdfPageAnnotationType::Widget
            | PdfPageAnnotationType::Popup
            | PdfPageAnnotationType::Unknown
    )
}

/// Create an annotation described by `new` on `page`, returning its id.
/// Mark an annotation as printable.
///
/// PDFium creates annotations with no flags, and an annotation without the
/// `Print` flag is omitted when the document is printed — so a highlight or
/// note the user added on screen would silently vanish on paper. Every
/// annotation this app creates is one the user wants to keep, so all of them
/// are made printable.
fn mark_printable(
    annot: &mut impl PdfPageAnnotationCommon,
) -> Result<(), pdfium_render::prelude::PdfiumError> {
    annot.set_is_printed(true)
}

pub(crate) fn create_annotation(
    document: &mut PdfDocument<'_>,
    page: u32,
    new: &NewAnnotation,
) -> Result<AnnotationId, pdfium_render::prelude::PdfiumError> {
    let page_index = page;
    let mut page = document.pages_mut().get(page as i32).map_err(|error| {
        pdfium_render::prelude::PdfiumError::IoError(std::io::Error::new(
            std::io::ErrorKind::Other,
            error.to_string(),
        ))
    })?;
    let annotations = page.annotations_mut();

    match new {
        NewAnnotation::Highlight(rect) => {
            let mut annot = annotations.create_highlight_annotation()?;
            mark_printable(&mut annot)?;
            // PDFium may reject `/C` on an existing document whose annotation
            // appearance is not writable. The annotation itself is still valid
            // and must not disappear just because its optional colour could
            // not be changed; keep PDFium's default appearance in that case.
            place_markup(&mut annot, *rect, PdfColor::YELLOW)?;
        }
        NewAnnotation::Underline(rect) => {
            let mut annot = annotations.create_underline_annotation()?;
            mark_printable(&mut annot)?;
            place_markup(&mut annot, *rect, PdfColor::BLACK)?;
        }
        NewAnnotation::StrikeOut(rect) => {
            let mut annot = annotations.create_strikeout_annotation()?;
            mark_printable(&mut annot)?;
            place_markup(&mut annot, *rect, PdfColor::RED)?;
        }
        NewAnnotation::Squiggly(rect) => {
            let mut annot = annotations.create_squiggly_annotation()?;
            mark_printable(&mut annot)?;
            place_markup(&mut annot, *rect, PdfColor::GREEN)?;
        }
        NewAnnotation::Square(rect) => {
            let mut annot = annotations.create_square_annotation()?;
            mark_printable(&mut annot)?;
            annot.set_bounds(pdf_rect(*rect))?;
            // Give the border the same document weight as the line markups, so
            // the outline the canvas previews while dragging is the outline the
            // engine draws. Pdfium's own default for a `Square` is about 0.67pt,
            // which only happened to match a fixed-pixel preview at one zoom.
            //
            // The border is written before the colour because `SetBorder`
            // clears the appearance stream and Pdfium refuses to read or write
            // `/C` while one is present.
            let _ = annot.set_border_width(DEFAULT_MARKUP_THICKNESS);
            // Colour is an optional appearance property. Some PDFs expose a
            // read-only appearance stream, so a failed colour write must not
            // make the rectangle tool itself fail.
            let _ = annot.set_stroke_color(PdfColor::RED);
            // Deliberately no fill call.
            //
            // Setting a *fully transparent* fill — which is how an
            // outline-only rectangle was first written — makes PDFium skip the
            // border as well as the interior, so the rectangle rendered as
            // nothing at all. It was visible while the drag was in progress,
            // because the canvas draws its own preview, and then vanished the
            // moment the re-rendered page arrived.
            //
            // Leaving `/IC` alone still lets the page text show through, which
            // is what an outline is for.
        }
        NewAnnotation::StickyNote((x, y), contents) => {
            let mut annot = annotations.create_text_annotation(contents)?;
            mark_printable(&mut annot)?;
            // A sticky note's `/Rect` is its icon box, and `set_position`
            // anchors that box's *lower-left* at the point. The canvas draws
            // its note tag at the rect *centre*, so instead give the note a
            // known-size box centred on the clicked point: the tag then lands
            // exactly where the user clicked and sits over PDFium's own icon.
            const ICON_PT: f32 = 20.0;
            let half = ICON_PT / 2.0;
            annot.set_bounds(pdf_rect(Rect::from_xywh(
                *x - half,
                *y - half,
                ICON_PT,
                ICON_PT,
            )))?;
            let _ = annot.set_stroke_color(PdfColor::YELLOW);
            if contents.is_empty() {
                annot.set_contents("Note")?;
            }
        }
        NewAnnotation::FreeText(rect, contents) => {
            let mut annot = annotations.create_free_text_annotation(contents)?;
            mark_printable(&mut annot)?;
            annot.set_bounds(pdf_rect(*rect))?;
            let _ = annot.set_stroke_color(PdfColor::BLACK);
            annot.set_contents(contents)?;
        }
    }

    page.regenerate_content()?;
    // PDFium appends new annotations to the end of the page's list — and for
    // sticky notes it AUTO-CREATES a popup child right after the note, so the
    // created annotation is not necessarily the last one. The popup is hidden
    // from `list_annotations` but still occupies a positional index, so
    // returning `count - 1` hands back the POPUP's id: the undo of "add note"
    // would then delete the popup and leave the note on the page. Scan the
    // raw list for the last annotation of the subtype we just created
    // instead; popups report their own subtype and are skipped naturally.
    let wanted = match new {
        NewAnnotation::Highlight(_) => PdfPageAnnotationType::Highlight,
        NewAnnotation::Underline(_) => PdfPageAnnotationType::Underline,
        NewAnnotation::StrikeOut(_) => PdfPageAnnotationType::Strikeout,
        NewAnnotation::Squiggly(_) => PdfPageAnnotationType::Squiggly,
        NewAnnotation::Square(_) => PdfPageAnnotationType::Square,
        NewAnnotation::StickyNote(_, _) => PdfPageAnnotationType::Text,
        NewAnnotation::FreeText(_, _) => PdfPageAnnotationType::FreeText,
    };
    let count = page.annotations().len();
    // Scoped so the borrow of the page's annotation list ends here: the
    // appearance step below needs the page mutably.
    let index = {
        let raw: Vec<_> = page.annotations().iter().collect();
        raw.iter()
            .enumerate()
            .rev()
            .find(|(_, annotation)| annotation.annotation_type() == wanted)
            .map(|(index, _)| index)
            .unwrap_or(count - 1)
    };

    // Line markups need their appearance built here rather than inside
    // `place_markup`, because creating the annotation makes Pdfium generate and
    // *store* an appearance stream for it — and while one exists,
    // `FPDFAnnot_SetColor` refuses to write the mark's colour. Addressing the
    // annotation generically (as `PdfPageAnnotation`) lets the helper drop that
    // stream, write the colour, and install ours.
    //
    // The mark is born at the default thickness, so what appears on the page is
    // what the canvas previewed during the drag; only a vertical resize makes it
    // thicker.
    if let Some(kind) = line_markup_kind(new) {
        let rect = new.rect().expect("a line markup always has a rect");
        if let Ok(mut annotation) = page.annotations_mut().get(index) {
            let _ = apply_line_markup_appearance(
                &mut annotation,
                kind,
                rect,
                Some(markup_color(kind)),
                DEFAULT_MARKUP_THICKNESS,
            );
        }
        page.regenerate_content()?;
    }

    Ok(AnnotationId::new(page_index, index as u32))
}

/// The line-markup kind `new` describes, or `None` for anything else.
fn line_markup_kind(new: &NewAnnotation) -> Option<AnnotationKind> {
    match new {
        NewAnnotation::Underline(_) => Some(AnnotationKind::Underline),
        NewAnnotation::StrikeOut(_) => Some(AnnotationKind::StrikeOut),
        NewAnnotation::Squiggly(_) => Some(AnnotationKind::Squiggly),
        _ => None,
    }
}

/// The colour each markup tool draws in, matching the canvas preview.
fn markup_color(kind: AnnotationKind) -> [u8; 4] {
    match kind {
        AnnotationKind::Underline => [0, 0, 0, 255],
        AnnotationKind::StrikeOut => [211, 47, 47, 255],
        AnnotationKind::Squiggly => [67, 160, 71, 255],
        _ => [0, 0, 0, 255],
    }
}

/// Set the bounds and quad points of a markup annotation over `rect`.
fn place_markup<'a>(
    annot: &mut impl MarkupAnnotation<'a>,
    rect: Rect,
    color: PdfColor,
) -> Result<(), pdfium_render::prelude::PdfiumError> {
    annot.set_bounds(pdf_rect(rect))?;
    // Creating an annotation makes Pdfium generate and *store* an appearance
    // stream for it, and `FPDFAnnot_SetColor` refuses to write a colour while
    // one is present. Dropping it first is what lets the colour through; it is
    // rebuilt below for the line markups, and Pdfium regenerates its own for
    // the rest on the next render.
    // Markup placement is the required part of creation. Colour is optional:
    // on some existing PDFs PDFium refuses to rewrite the annotation colour,
    // but the visible markup can still be created with its default colour.
    let _ = annot.set_stroke_color(color);
    // Quad points in the order Acrobat writes them: top-left, top-right,
    // bottom-left, bottom-right, in PDF's y-up space.
    annot
        .attachment_points_mut()
        .create_attachment_point_at_end(PdfQuadPoints::new(
            PdfPoints::new(rect.min_x),
            PdfPoints::new(rect.max_y),
            PdfPoints::new(rect.max_x),
            PdfPoints::new(rect.max_y),
            PdfPoints::new(rect.min_x),
            PdfPoints::new(rect.min_y),
            PdfPoints::new(rect.max_x),
            PdfPoints::new(rect.min_y),
        ))?;
    Ok(())
}

/// The subset of behaviour every markup annotation shares.
///
/// `'a` is the borrow the annotation itself carries; the returned quad-point
/// collection borrows the annotation's lifetime, not the `&mut self` call, so
/// the trait needs both named separately.
trait MarkupAnnotation<'a> {
    fn set_bounds(
        &mut self,
        bounds: pdfium_render::prelude::PdfRect,
    ) -> Result<(), pdfium_render::prelude::PdfiumError>;
    fn set_stroke_color(
        &mut self,
        color: PdfColor,
    ) -> Result<(), pdfium_render::prelude::PdfiumError>;
    fn attachment_points_mut(
        &mut self,
    ) -> &mut pdfium_render::prelude::PdfPageAnnotationAttachmentPoints<'a>;
}

macro_rules! impl_markup {
    ($($ty:ident),+ $(,)?) => {
        $(
            impl<'a> MarkupAnnotation<'a> for pdfium_render::prelude::$ty<'a> {
                fn set_bounds(
                    &mut self,
                    bounds: pdfium_render::prelude::PdfRect,
                ) -> Result<(), pdfium_render::prelude::PdfiumError> {
                    PdfPageAnnotationCommon::set_bounds(self, bounds)
                }
                fn set_stroke_color(
                    &mut self,
                    color: PdfColor,
                ) -> Result<(), pdfium_render::prelude::PdfiumError> {
                    PdfPageAnnotationCommon::set_stroke_color(self, color)
                }
                fn attachment_points_mut(
                    &mut self,
                ) -> &mut pdfium_render::prelude::PdfPageAnnotationAttachmentPoints<'a> {
                    Self::attachment_points_mut(self)
                }
            }
        )+
    };
}

impl_markup!(
    PdfPageHighlightAnnotation,
    PdfPageUnderlineAnnotation,
    PdfPageStrikeoutAnnotation,
    PdfPageSquigglyAnnotation,
);

use crate::formmark::pdf_rect;

/// Write a text value into every widget that shares `name`.
///
/// A PDF field whose name appears on several pages/positions is ONE field with
/// multiple kid widgets: the value belongs on the parent field dictionary,
/// which `pdfium-render` does not expose for writing. Writing each widget's own
/// dictionary keeps every in-app surface consistent (our state, this crate's
/// list) and persists the text into the file's widget dictionaries.
pub(crate) fn sync_shared_text_fields(
    document: &mut PdfDocument<'_>,
    name: &str,
    value: &str,
) -> Result<(), pdfium_render::prelude::PdfiumError> {
    let page_count = document.pages().len();
    for page_index in 0..page_count {
        let Ok(mut page) = document.pages_mut().get(page_index as i32) else {
            continue;
        };
        let targets: Vec<usize> = {
            let mut targets = Vec::new();
            for (index, annotation) in page.annotations().iter().enumerate() {
                let Some(field) = annotation.as_form_field() else {
                    continue;
                };
                if field.as_text_field().is_some() && field.name().as_deref() == Some(name) {
                    targets.push(index);
                }
            }
            targets
        };
        for index in targets {
            if let Ok(mut annotation) = page.annotations_mut().get(index) {
                if let Some(text) = annotation
                    .as_form_field_mut()
                    .and_then(|f| f.as_text_field_mut())
                {
                    text.set_value(value)?;
                }
            }
        }
    }
    Ok(())
}

/// Replace the text contents of one annotation.
pub(crate) fn change_contents(
    document: &mut PdfDocument<'_>,
    id: AnnotationId,
    contents: &str,
) -> Result<(), pdfium_render::prelude::PdfiumError> {
    let mut page = document.pages_mut().get(id.page as i32)?;
    let mut annotation = page.annotations_mut().get(id.annot_index as usize)?;
    annotation.set_contents(contents)?;
    page.regenerate_content()?;
    Ok(())
}

/// Move / resize one annotation to cover `rect` (PDF page space, y up).
///
/// Returns `Ok(false)` when the id no longer resolves — ids are positional
/// and shift after a removal, so a stale id names nothing rather than being
/// an error.
///
/// Markup annotations (highlight and friends) render from their **quad
/// points**, not their bounds, so moving one means rewriting the first quad
/// as well as the bounds — a bounds-only write would leave the visible ink at
/// the old spot. Quads beyond the first (foreign multi-line markup) are left
/// alone: the bounds still track the whole annotation, and our own marks
/// always carry exactly one quad.
pub(crate) fn update_annotation_bounds(
    document: &mut PdfDocument<'_>,
    id: AnnotationId,
    rect: Rect,
) -> Result<bool, pdfium_render::prelude::PdfiumError> {
    let mut page = document.pages_mut().get(id.page as i32)?;
    let Ok(mut annotation) = page.annotations_mut().get(id.annot_index as usize) else {
        return Ok(false);
    };
    let kind = classify(annotation.annotation_type());
    // Read the box and the stored thickness *before* the box moves: a line
    // markup's new thickness is derived from how far the drag went, so the old
    // height is needed as the reference point.
    let height_before = annotation
        .bounds()
        .map(|bounds| bounds.height().value)
        .unwrap_or(rect.max_y - rect.min_y);
    let previous_thickness = stored_thickness(&annotation);
    annotation.set_bounds(pdf_rect(rect))?;
    match kind {
        AnnotationKind::Highlight => {
            if let Some(mark) = annotation.as_highlight_annotation_mut() {
                rewrite_first_quad(mark, rect)?;
            }
        }
        AnnotationKind::Underline => {
            if let Some(mark) = annotation.as_underline_annotation_mut() {
                rewrite_first_quad(mark, rect)?;
            }
        }
        AnnotationKind::StrikeOut => {
            if let Some(mark) = annotation.as_strikeout_annotation_mut() {
                rewrite_first_quad(mark, rect)?;
            }
        }
        AnnotationKind::Squiggly => {
            if let Some(mark) = annotation.as_squiggly_annotation_mut() {
                rewrite_first_quad(mark, rect)?;
            }
        }
        _ => {}
    }
    if kind.is_line_markup() {
        // Underline, strikeout and squiggly are drawn as a *line*, and Pdfium's
        // own appearance generator hard-codes that line to 1pt: neither
        // `/BS /W` nor `/Border` is consulted, so the annotation dictionary
        // simply cannot express a thicker mark. Rewriting the quad's height
        // therefore never thickened anything — the vertical-resize report.
        //
        // The only lever is the appearance stream itself, so these three kinds
        // get one built here, with the drag's vertical distance driving the
        // stroke. Pdfium skips its own generator once `/AP /N` exists, so this
        // is what gets drawn. The colour is left alone (`None`) — a resize must
        // not repaint the mark. Best-effort: a document that refuses the write
        // keeps Pdfium's own mark.
        let thickness =
            dragged_thickness(previous_thickness, height_before, rect.max_y - rect.min_y);
        let _ = apply_line_markup_appearance(&mut annotation, kind, rect, None, thickness);
    } else {
        // Pdfium generates a markup annotation's appearance stream the first
        // time it renders the annotation and then *caches* it. Changing the
        // quad points (a resize) leaves that cached appearance in place, so the
        // mark keeps drawing at its old size no matter what the annotation
        // dictionary now says — exactly why highlights would not resize while
        // the bounds-based rectangle did. Clearing the stream makes Pdfium
        // rebuild it from the new geometry. Best-effort: a document that
        // refuses the write simply keeps Pdfium's existing appearance.
        let _ = annotation.remove_appearance_stream();
    }
    page.regenerate_content()?;
    Ok(true)
}

/// Marks the appearance-stream comment that restates a line markup's colour,
/// as `% <marker> <r> <g> <b> <a>`.
const COLOR_MARKER: &str = "PDFREADER-COLOR";

/// The thickness a line markup is born with, in points.
///
/// A hairline: this is the weight the reader gives every drawn mark, and the
/// canvas previews it while the mark is being dragged (`PREVIEW_STROKE_PT` in
/// the app crate), so a freshly applied underline looks like the one that was
/// drawn. Thickness only changes once the user resizes vertically. Public so the
/// canvas can assert the two stay equal rather than relying on a comment.
pub const DEFAULT_MARKUP_THICKNESS: f32 = 0.45;

/// The thinnest a line markup may be dragged down to, in points.
///
/// Deliberately below [`DEFAULT_MARKUP_THICKNESS`]: a weight floor of the
/// default made the stroke stop dead the moment the user tried to thin it — the
/// "it locks suddenly" report, since every downward drag clamped straight back
/// to the weight it was drawn at. It exists only to keep the mark from being
/// dragged away to nothing, not as a target weight.
pub const MIN_MARKUP_THICKNESS: f32 = 0.2;

/// The height of a squiggly mark's wave, in points, from its floor to a crest.
///
/// Kept small deliberately: the wave is a *rule*, not a decoration, so it wants
/// to read as a tight zig-zag under the text rather than a tall saw. This is the
/// height the canvas previews, so the drawn mark and the applied one agree.
///
/// A fixed document distance rather than a multiple of the stroke weight — see
/// the squiggly arm of [`line_markup_appearance`].
pub const SQUIGGLY_RISE_PT: f32 = 1.2;

/// The horizontal distance between two crests of a squiggly mark, in points.
///
/// Shared with the canvas preview for the same reason as [`SQUIGGLY_RISE_PT`].
pub const SQUIGGLY_STEP_PT: f32 = 4.0;

/// The thickness after a vertical drag.
///
/// Thickness tracks the drag *delta* rather than the box height, which is what
/// keeps the two requirements compatible: a mark drawn over a tall band stays a
/// thin underline (the box height at creation is not a thickness), while
/// pulling the top handle up by 20pt makes the mark 20pt thicker. A purely
/// horizontal drag has a zero delta and therefore leaves the thickness alone.
///
/// The result never drops below [`MIN_MARKUP_THICKNESS`] — so the stroke can be
/// dragged down to a hairline, but never to nothing — and never exceeds the
/// box, which would spill ink outside the selection the user is dragging.
fn dragged_thickness(previous: f32, height_before: f32, height_after: f32) -> f32 {
    let ceiling = height_after.max(MIN_MARKUP_THICKNESS);
    (previous + (height_after - height_before)).clamp(MIN_MARKUP_THICKNESS, ceiling)
}

/// Gives a line markup the appearance stream that draws it at `thickness`.
///
/// Pdfium's own appearance generator draws these three subtypes as a hard-coded
/// 1pt line and consults neither `/BS /W` nor `/Border`, so the annotation
/// dictionary alone cannot express a thicker mark — the stream has to be
/// supplied. `/Border` is still written, because it is the PDF property for a
/// line's width and the honest place to record the thickness for the next
/// resize to read back.
///
/// `color` is `Some` to set the mark's colour (creating it, or recolouring it)
/// and `None` to keep whatever colour it already declares.
///
/// The order of the calls is load-bearing. Pdfium refuses to read *or* write a
/// colour while an appearance stream is present, and `FPDFAnnot_SetBorder`
/// clears the stream — so the border write comes first, and only then can the
/// colour be read back. `None` therefore means "read it now, while the stream is
/// gone", which is what stops a resize from repainting a mark another editor
/// coloured.
fn apply_line_markup_appearance(
    annotation: &mut pdfium_render::prelude::PdfPageAnnotation<'_>,
    kind: AnnotationKind,
    rect: Rect,
    color: Option<[u8; 4]>,
    thickness: f32,
) -> Result<(), pdfium_render::prelude::PdfiumError> {
    annotation.set_border_width(thickness)?;
    let color = match color {
        Some(color) => {
            annotation.set_stroke_color(PdfColor::new(color[0], color[1], color[2], color[3]))?;
            color
        }
        // Pdfium reports a default colour — rather than failing — for an
        // annotation with no `/C` entry, so this always yields something.
        None => annotation
            .stroke_color()
            .map(|c| [c.red(), c.green(), c.blue(), c.alpha()])
            .unwrap_or_else(|_| markup_color(kind)),
    };
    let stream = line_markup_appearance(kind, rect, color, thickness).ok_or(
        pdfium_render::prelude::PdfiumError::PdfiumLibraryInternalError(
            pdfium_render::prelude::PdfiumInternalError::Unknown,
        ),
    )?;
    annotation.set_normal_appearance_stream(&stream)
}

/// The colour recorded inside a line markup's appearance stream, if it is one
/// this app generated.
///
/// `FPDFAnnot_GetColor` — and its `Set` counterpart — refuse to work at all
/// while an annotation has an appearance stream, and every line markup does.
/// The colour therefore has to be read back from the stream itself; see the
/// comment [`line_markup_appearance`] writes for it.
fn recorded_color(annotation: &pdfium_render::prelude::PdfPageAnnotation<'_>) -> Option<[u8; 4]> {
    let stream = annotation.normal_appearance_stream()?;
    let marker = format!("% {COLOR_MARKER}");
    let line = stream
        .lines()
        .find(|line| line.trim_start().starts_with(&marker))?;
    let mut parts = line.split_whitespace();
    parts.next()?; // the `%` that opens the comment
    parts.next()?; // the marker itself
    let mut channel = || parts.next()?.parse::<u8>().ok();
    Some([channel()?, channel()?, channel()?, channel()?])
}

/// The thickness stored on a line markup, falling back to the default when the
/// annotation has no usable `/Border` entry of its own.
fn stored_thickness(annotation: &pdfium_render::prelude::PdfPageAnnotation<'_>) -> f32 {
    annotation
        .border_width()
        .filter(|width| *width > 0.0)
        .unwrap_or(DEFAULT_MARKUP_THICKNESS)
}

/// Appearance-stream content for an underline, strikeout or squiggly mark.
///
/// Written in **absolute page coordinates**: Pdfium sets the stream's `/BBox`
/// to the annotation's `/Rect` and maps that box straight onto the same rect,
/// so a mark drawn at the rect's own coordinates lands exactly inside the box.
/// Pdfium's own generators work in the same space.
///
/// Returns `None` for a degenerate box, or for a kind that is not a line markup.
fn line_markup_appearance(
    kind: AnnotationKind,
    rect: Rect,
    color: [u8; 4],
    thickness: f32,
) -> Option<String> {
    let width = rect.max_x - rect.min_x;
    let height = rect.max_y - rect.min_y;
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    // Stroke colour, plus the annotation's opacity as a graphics-state alpha.
    // `RG`/`CA` are plain content-stream operators, so no ExtGState resource is
    // needed — which matters, because the stream Pdfium builds for us carries
    // no `/Resources`.
    let (r, g, b, a) = (
        color[0] as f32 / 255.0,
        color[1] as f32 / 255.0,
        color[2] as f32 / 255.0,
        color[3] as f32 / 255.0,
    );
    // The colour is restated as a content-stream comment. Pdfium refuses to
    // *report* a colour while an appearance stream is present (see
    // `recorded_color`), so without this the properties panel would show no
    // colour at all for these marks.
    let mut stream = format!(
        "% {COLOR_MARKER} {} {} {} {}\n{} {} {} RG\n",
        color[0],
        color[1],
        color[2],
        color[3],
        num(r),
        num(g),
        num(b)
    );
    if a < 1.0 {
        stream.push_str(&format!("{} CA\n", num(a)));
    }
    // Round caps and joins so the squiggly's crests are not clipped square.
    stream.push_str(&format!("{} w\n1 J 1 j\n", num(thickness)));
    match kind {
        AnnotationKind::Underline => {
            // A bar sitting on the box floor: half the stroke falls outside the
            // box, so the ink spans exactly `thickness` above the bottom edge.
            let y = rect.min_y + thickness / 2.0;
            stream.push_str(&format!(
                "{} {} m {} {} l S\n",
                num(rect.min_x),
                num(y),
                num(rect.max_x),
                num(y)
            ));
        }
        AnnotationKind::StrikeOut => {
            let y = (rect.min_y + rect.max_y) / 2.0;
            stream.push_str(&format!(
                "{} {} m {} {} l S\n",
                num(rect.min_x),
                num(y),
                num(rect.max_x),
                num(y)
            ));
        }
        AnnotationKind::Squiggly => {
            // A zig-zag wave of a fixed document size, sitting on the box floor
            // and rising above it — like the underline, and like the canvas
            // previews it. The wave is deliberately *not* a function of the
            // stroke weight: a squiggly rule is judged by its wave, and scaling
            // the wave with the weight would flatten a hairline squiggle into a
            // straight line. Only the stroke itself follows the drag.
            let floor = rect.min_y + thickness / 2.0;
            let rise = SQUIGGLY_RISE_PT
                .min((height - thickness).max(0.0))
                .max(0.0);
            let step = SQUIGGLY_STEP_PT;
            let mut x = rect.min_x;
            let mut crest = true;
            stream.push_str(&format!(
                "{} {} m\n",
                num(x),
                num(if crest { floor + rise } else { floor })
            ));
            x += step;
            while x < rect.max_x {
                crest = !crest;
                stream.push_str(&format!(
                    "{} {} l\n",
                    num(x),
                    num(if crest { floor + rise } else { floor })
                ));
                x += step;
            }
            crest = !crest;
            stream.push_str(&format!(
                "{} {} l S\n",
                num(rect.max_x),
                num(if crest { floor + rise } else { floor })
            ));
        }
        _ => return None,
    }
    Some(stream)
}

/// Formats a coordinate for a content stream.
///
/// Fixed precision keeps the text free of the exponent notation Rust's default
/// `f32` formatting can emit for extreme values, which would not be a valid PDF
/// number.
fn num(value: f32) -> String {
    format!("{value:.4}")
}

/// Replace the first quad of a markup annotation with one covering `rect`,
/// creating the quad when the annotation has none.
fn rewrite_first_quad<'a>(
    annot: &mut impl MarkupAnnotation<'a>,
    rect: Rect,
) -> Result<(), pdfium_render::prelude::PdfiumError> {
    let quad = PdfQuadPoints::new(
        PdfPoints::new(rect.min_x),
        PdfPoints::new(rect.max_y),
        PdfPoints::new(rect.max_x),
        PdfPoints::new(rect.max_y),
        PdfPoints::new(rect.min_x),
        PdfPoints::new(rect.min_y),
        PdfPoints::new(rect.max_x),
        PdfPoints::new(rect.min_y),
    );
    let points = annot.attachment_points_mut();
    if points.is_empty() {
        points.create_attachment_point_at_end(quad)?;
    } else {
        points.set_attachment_point_at_index(0, quad)?;
    }
    Ok(())
}

/// Recolour one annotation.
///
/// The alpha byte is the annotation's rendered opacity, so this one call
/// serves both the colour swatches and the opacity slider: changing opacity
/// keeps the RGB and rewrites the alpha.
pub(crate) fn set_annotation_color(
    document: &mut PdfDocument<'_>,
    id: AnnotationId,
    color: [u8; 4],
) -> Result<bool, pdfium_render::prelude::PdfiumError> {
    let mut page = document.pages_mut().get(id.page as i32)?;
    let Ok(mut annotation) = page.annotations_mut().get(id.annot_index as usize) else {
        return Ok(false);
    };
    let kind = classify(annotation.annotation_type());
    let applied = if kind.is_line_markup() {
        // A line markup's colour is baked into the appearance stream built for
        // it (see `line_markup_appearance`), so recolouring means rebuilding
        // that stream, keeping the thickness the user had dragged it to. Only
        // these kinds are touched: discarding an imported annotation's own
        // appearance would destroy artwork this app never authored.
        match annotation.bounds() {
            Ok(bounds) => {
                let rect = Rect::from_xywh(
                    bounds.left().value,
                    bounds.bottom().value,
                    bounds.width().value,
                    bounds.height().value,
                );
                let thickness = stored_thickness(&annotation);
                apply_line_markup_appearance(&mut annotation, kind, rect, Some(color), thickness)
            }
            Err(error) => Err(error),
        }
    } else {
        apply_stroke_colour(&mut annotation, kind, PdfColor::new(color[0], color[1], color[2], color[3]))
    };
    match applied {
        Ok(()) => {
            page.regenerate_content()?;
            Ok(true)
        }
        // A colour edit is optional for PDF annotations. In particular,
        // imported /AP-only annotations can reject FPDFAnnot_SetColor; return
        // false so the engine thread re-lists the real state instead of
        // surfacing a scary "Annotation failed" error or crashing.
        Err(_) => Ok(false),
    }
}

/// Write a mark's colour, retrying past a cached appearance stream.
///
/// Pdfium caches the appearance stream it generated for an annotation the first
/// time that annotation is rendered, and refuses to read or write `/C` while
/// that stream is present — so a highlight drawn a moment ago could never be
/// recoloured: `FPDFAnnot_SetColor` simply returns `false`, and the mark keeps
/// whatever colour it was born with. That is the "highlight properties do
/// nothing" report, because the page is rendered from the moment it opens.
///
/// Dropping the stream lets the colour through, and Pdfium rebuilds the
/// appearance from the new `/C` on the next render. This is safe only for the
/// kinds this app draws, whose appearance Pdfium generates itself; an
/// *imported* annotation's own stream is artwork this app did not author, and
/// discarding it would destroy that artwork. Those kinds report the failure.
fn apply_stroke_colour(
    annotation: &mut pdfium_render::prelude::PdfPageAnnotation<'_>,
    kind: AnnotationKind,
    color: PdfColor,
) -> Result<(), pdfium_render::prelude::PdfiumError> {
    let first = annotation.set_stroke_color(color);
    if first.is_ok() {
        return first;
    }
    if !kind.is_creatable() {
        return first;
    }
    // Same treatment `update_annotation_bounds` already gives these kinds when
    // a resize moves them.
    let _ = annotation.remove_appearance_stream();
    annotation.set_stroke_color(color)
}

/// Remove one annotation.
pub(crate) fn remove_annotation(
    document: &mut PdfDocument<'_>,
    id: AnnotationId,
) -> Result<bool, pdfium_render::prelude::PdfiumError> {
    let mut page = document.pages_mut().get(id.page as i32)?;
    let Ok(annotation) = page.annotations_mut().get(id.annot_index as usize) else {
        // Ids are positional and shift on every removal, so a stale id simply
        // names nothing.
        return Ok(false);
    };
    page.annotations_mut().delete_annotation(annotation)?;
    page.regenerate_content()?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfium_render::prelude::{
        PdfBitmapFormat, PdfPageAnnotationCommon, PdfPagePaperSize, PdfRenderConfig, Pdfium,
    };
    use std::path::PathBuf;

    /// Locate the staged PDFium library under `target/native/*`.
    fn find_pdfium() -> Option<PathBuf> {
        let native = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("target")
            .join("native");
        for entry in std::fs::read_dir(native).ok()?.flatten() {
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

    /// Annotations must carry the PDF `Print` flag. Without it a highlight or
    /// note the user added on screen would be silently omitted from a printed
    /// copy — the whole point of printing a marked-up document.
    #[test]
    fn created_annotations_carry_the_print_flag() {
        let Some(dir) = find_pdfium() else {
            eprintln!("SKIP: no PDFium library staged under target/native");
            return;
        };
        let bindings = Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(&dir))
            .or_else(|_| Pdfium::bind_to_system_library())
            .expect("bind PDFium");
        let pdfium = Pdfium::new(bindings);

        let mut document = pdfium.create_new_pdf().expect("new document");
        document
            .pages_mut()
            .create_page_at_end(PdfPagePaperSize::a4())
            .expect("add a page");

        create_annotation(
            &mut document,
            0,
            &NewAnnotation::Highlight(Rect::from_xywh(20.0, 20.0, 80.0, 12.0)),
        )
        .expect("create a highlight");

        let page = document.pages().get(0).expect("page");
        let printed = page.annotations().iter().any(|a| a.is_printed());
        assert!(printed, "a created annotation must be printable");
    }

    /// Read the first quad of the highlight at `index`, as (left, bottom, right, top).
    fn highlight_quad(document: &mut PdfDocument<'_>, index: usize) -> (f32, f32, f32, f32) {
        let mut page = document.pages_mut().get(0).expect("page");
        let mut annotation = page.annotations_mut().get(index).expect("annotation");
        let highlight = annotation
            .as_highlight_annotation_mut()
            .expect("a highlight annotation");
        let quad = highlight.attachment_points_mut().first().expect("a quad");
        (
            quad.left().value,
            quad.bottom().value,
            quad.right().value,
            quad.top().value,
        )
    }

    /// Resizing a markup annotation must rewrite its quad points, not just its
    /// `/Rect`: PDFium renders a highlight (and underline/strikeout/squiggly)
    /// from the quads, so a bounds-only update leaves the ink exactly where it
    /// was — the "resize does nothing for highlights" report.
    #[test]
    fn resizing_a_highlight_rewrites_its_quad() {
        let Some(dir) = find_pdfium() else {
            eprintln!("SKIP: no PDFium library staged under target/native");
            return;
        };
        let bindings = Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(&dir))
            .or_else(|_| Pdfium::bind_to_system_library())
            .expect("bind PDFium");
        let pdfium = Pdfium::new(bindings);

        let mut document = pdfium.create_new_pdf().expect("new document");
        document
            .pages_mut()
            .create_page_at_end(PdfPagePaperSize::a4())
            .expect("add a page");

        let a = Rect::from_xywh(40.0, 700.0, 120.0, 14.0);
        let id = create_annotation(&mut document, 0, &NewAnnotation::Highlight(a)).expect("create");
        let (l, b, r, t) = highlight_quad(&mut document, id.annot_index as usize);
        assert!(
            (l - a.min_x).abs() < 0.5
                && (b - a.min_y).abs() < 0.5
                && (r - a.max_x).abs() < 0.5
                && (t - a.max_y).abs() < 0.5,
            "the created quad must match the rect: quad ({l},{b},{r},{t}) vs {a:?}"
        );

        let grown = Rect::from_xywh(40.0, 700.0, 240.0, 14.0);
        assert!(update_annotation_bounds(&mut document, id, grown).expect("resize"));
        let (l, b, r, t) = highlight_quad(&mut document, id.annot_index as usize);
        assert!(
            (l - grown.min_x).abs() < 0.5 && (r - grown.max_x).abs() < 0.5,
            "resizing must rewrite the quad, got ({l},{b},{r},{t}) want {grown:?}"
        );
    }

    /// Count the non-white pixels PDFium rasterises for the page. Used to prove
    /// a resize changes the rendered ink, not merely the reported bounds.
    fn ink_pixels(document: &mut PdfDocument<'_>) -> usize {
        let page = document.pages().get(0).expect("page");
        let config = PdfRenderConfig::new()
            .scale_page_by_factor(1.0)
            .set_format(PdfBitmapFormat::BGRA)
            .render_annotations(true)
            .clear_before_rendering(true)
            .set_clear_color(PdfColor::WHITE);
        let bitmap = page.render_with_config(&config).expect("render");
        let data = bitmap.as_raw_bytes();
        data.chunks_exact(4)
            .filter(|p| !(p[0] > 240 && p[1] > 240 && p[2] > 240))
            .count()
    }

    /// Bounding box (min_x, min_y, max_x, max_y) of the page's non-white pixels.
    fn ink_bbox(document: &mut PdfDocument<'_>) -> (usize, usize, usize, usize) {
        ink_bbox_at(document, 1.0)
    }

    /// As [`ink_bbox`], but rendered at `scale` pixels per point.
    ///
    /// Needed for anything measured against a sub-point distance: at 1:1 a
    /// hairline wave is a pixel or two, which is too coarse to assert on.
    fn ink_bbox_at(document: &mut PdfDocument<'_>, scale: f32) -> (usize, usize, usize, usize) {
        let page = document.pages().get(0).expect("page");
        let config = PdfRenderConfig::new()
            .scale_page_by_factor(scale)
            .set_format(PdfBitmapFormat::BGRA)
            .render_annotations(true)
            .clear_before_rendering(true)
            .set_clear_color(PdfColor::WHITE);
        let bitmap = page.render_with_config(&config).expect("render");
        let w = bitmap.width() as usize;
        let h = bitmap.height() as usize;
        let data = bitmap.as_raw_bytes();
        let (mut minx, mut miny, mut maxx, mut maxy) = (usize::MAX, usize::MAX, 0, 0);
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                if !(data[i] > 240 && data[i + 1] > 240 && data[i + 2] > 240) {
                    minx = minx.min(x);
                    miny = miny.min(y);
                    maxx = maxx.max(x);
                    maxy = maxy.max(y);
                }
            }
        }
        (minx, miny, maxx, maxy)
    }

    /// The whole point of the quad rewrite: growing a highlight must add ink to
    /// the rasterised page. If it does not, the mark is stuck at its old size
    /// no matter what the annotation list reports.
    #[test]
    fn resizing_a_highlight_changes_the_rendered_ink() {
        let Some(dir) = find_pdfium() else {
            eprintln!("SKIP: no PDFium library staged under target/native");
            return;
        };
        let bindings = Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(&dir))
            .or_else(|_| Pdfium::bind_to_system_library())
            .expect("bind PDFium");
        let pdfium = Pdfium::new(bindings);

        let mut document = pdfium.create_new_pdf().expect("new document");
        document
            .pages_mut()
            .create_page_at_end(PdfPagePaperSize::a4())
            .expect("add a page");

        let a = Rect::from_xywh(100.0, 700.0, 100.0, 20.0);
        let id = create_annotation(&mut document, 0, &NewAnnotation::Highlight(a)).expect("create");
        // Render once *before* the resize: that first render is what makes
        // Pdfium cache the annotation's appearance stream, which is the whole
        // reason the resize used to be invisible.
        let before = ink_pixels(&mut document);
        let bbox_before = ink_bbox(&mut document);

        let grown = Rect::from_xywh(100.0, 700.0, 300.0, 20.0);
        assert!(update_annotation_bounds(&mut document, id, grown).expect("resize"));
        let after = ink_pixels(&mut document);
        let bbox_after = ink_bbox(&mut document);

        assert!(
            after > before,
            "growing the highlight must add ink: before {before} {bbox_before:?}, after {after} {bbox_after:?}"
        );
        assert!(
            bbox_after.2 >= 290,
            "the rendered ink must widen to ~x299, got {bbox_after:?}"
        );
    }

    /// Vertical resize: growing the HEIGHT of a markup annotation must add ink
    /// vertically, not just horizontally. This is the axis the "vertical resize
    /// does nothing / only the length changes" report is about.
    #[test]
    fn resizing_a_highlight_vertically_changes_the_rendered_ink() {
        let Some(dir) = find_pdfium() else {
            eprintln!("SKIP: no PDFium library staged under target/native");
            return;
        };
        let bindings = Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(&dir))
            .or_else(|_| Pdfium::bind_to_system_library())
            .expect("bind PDFium");
        let pdfium = Pdfium::new(bindings);

        let mut document = pdfium.create_new_pdf().expect("new document");
        document
            .pages_mut()
            .create_page_at_end(PdfPagePaperSize::a4())
            .expect("add a page");

        let a = Rect::from_xywh(100.0, 700.0, 200.0, 20.0);
        let id = create_annotation(&mut document, 0, &NewAnnotation::Highlight(a)).expect("create");
        // Render first so Pdfium caches the appearance stream (the whole reason
        // a resize used to be invisible).
        let before = ink_bbox(&mut document);

        let grown = Rect::from_xywh(100.0, 700.0, 200.0, 90.0);
        assert!(update_annotation_bounds(&mut document, id, grown).expect("resize"));
        let after = ink_bbox(&mut document);

        assert!(
            (after.3 - after.1) > (before.3 - before.1) + 20,
            "growing the highlight height must add ink vertically: before {before:?}, after {after:?}"
        );
    }

    /// A vertical resize of an underline must update the annotation's bounds —
    /// the geometry the app draws the selection box and handles from — and the
    /// thickness it records in `/Border`, which is where the next resize reads
    /// it from.
    #[test]
    fn resizing_an_underline_vertically_updates_its_bounds() {
        let Some(dir) = find_pdfium() else {
            eprintln!("SKIP: no PDFium library staged under target/native");
            return;
        };
        let bindings = Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(&dir))
            .or_else(|_| Pdfium::bind_to_system_library())
            .expect("bind PDFium");
        let pdfium = Pdfium::new(bindings);

        let mut document = pdfium.create_new_pdf().expect("new document");
        document
            .pages_mut()
            .create_page_at_end(PdfPagePaperSize::a4())
            .expect("add a page");

        let a = Rect::from_xywh(100.0, 700.0, 200.0, 16.0);
        let id = create_annotation(&mut document, 0, &NewAnnotation::Underline(a)).expect("create");

        let grown = Rect::from_xywh(100.0, 700.0, 200.0, 80.0);
        assert!(update_annotation_bounds(&mut document, id, grown).expect("resize"));
        let rect = list_annotations(&document)
            .into_iter()
            .find(|x| x.id == id)
            .expect("underline listed")
            .rect;

        assert!(
            (rect.max_y - rect.min_y) > 60.0 && (rect.max_x - rect.min_x) > 190.0,
            "a vertical resize must grow the underline's bounds height while keeping its width: {rect:?}"
        );
    }

    /// Recolour and resize a freshly created Square and read it back. This is
    /// the engine round-trip the properties panel depends on: every edit is
    /// followed by a re-list, and `AnnotationsLoaded` replaces the whole
    /// annotation list with whatever that re-list returns. If `set_stroke_color`
    /// or `set_bounds` is silently rejected (the patched best-effort path
    /// returns `Ok(false)`), the re-list carries the OLD values and the edit
    /// appears to do nothing on screen.
    #[test]
    fn square_colour_and_bounds_round_trip_on_relist() {
        let Some(dir) = find_pdfium() else {
            eprintln!("SKIP: no PDFium library staged under target/native");
            return;
        };
        let bindings = Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(&dir))
            .or_else(|_| Pdfium::bind_to_system_library())
            .expect("bind PDFium");
        let pdfium = Pdfium::new(bindings);

        let mut document = pdfium.create_new_pdf().expect("new document");
        document
            .pages_mut()
            .create_page_at_end(PdfPagePaperSize::a4())
            .expect("add a page");

        let id = create_annotation(
            &mut document,
            0,
            &NewAnnotation::Square(Rect::from_xywh(80.0, 700.0, 140.0, 80.0)),
        )
        .expect("create a square");

        let before = list_annotations(&document);
        let before = before.iter().find(|a| a.id == id).expect("square present");
        eprintln!("before colour = {:?} bounds = {:?}", before.color, before.rect);
        // The default square colour is red (see create_annotation).
        assert_eq!(before.color, Some([255, 0, 0, 255]), "square starts red");

        // Recolour to blue.
        let applied = set_annotation_color(&mut document, id, [33, 150, 243, 255])
            .expect("no PDFium error");
        eprintln!("set_annotation_color applied = {applied}");
        let after_color = list_annotations(&document);
        let after_color = after_color.iter().find(|a| a.id == id).expect("square present");
        eprintln!("after colour = {:?}", after_color.color);
        assert_eq!(
            after_color.color,
            Some([33, 150, 243, 255]),
            "blue must round-trip through the re-list"
        );

        // Resize.
        let moved = Rect::from_xywh(100.0, 600.0, 220.0, 120.0);
        let bounds_applied = update_annotation_bounds(&mut document, id, moved).expect("no error");
        eprintln!("update_annotation_bounds applied = {bounds_applied}");
        let after_bounds = list_annotations(&document);
        let after_bounds = after_bounds.iter().find(|a| a.id == id).expect("square present");
        eprintln!("after bounds = {:?}", after_bounds.rect);
        assert_eq!(after_bounds.rect, moved, "bounds must round-trip");

        // Opacity is carried as the colour alpha in this design; confirm the
        // alpha survives a re-list too.
        set_annotation_color(&mut document, id, [33, 150, 243, 128])
            .expect("no PDFium error");
        let after_alpha = list_annotations(&document);
        let after_alpha = after_alpha.iter().find(|a| a.id == id).expect("square present");
        eprintln!("after alpha colour = {:?}", after_alpha.color);
        assert_eq!(
            after_alpha.color,
            Some([33, 150, 243, 128]),
            "alpha (opacity) must round-trip through /C"
        );
    }

    /// A highlight's colour and opacity must survive its first render.
    ///
    /// Regression: Pdfium caches the appearance it generated for an annotation
    /// the first time that annotation is rendered, and then refuses to read or
    /// write `/C` while that stream is present. The page is rendered from the
    /// moment it opens, so drawing a highlight and then recolouring it in the
    /// properties panel changed nothing at all — `FPDFAnnot_SetColor` was
    /// rejected and the mark kept the colour it was born with. The same blocked
    /// the opacity slider, whose value rides in the colour's alpha.
    ///
    /// Measured with the engine's own render configuration, so the bytes are
    /// the native BGRA the canvas reads.
    #[test]
    fn a_highlight_can_be_recoloured_after_it_has_been_rendered() {
        let Some(pdfium) = pdfium() else {
            eprintln!("SKIP: no PDFium library staged under target/native");
            return;
        };
        let mut document = blank_document(&pdfium);

        let config = PdfRenderConfig::new()
            .scale_page_by_factor(1.0)
            .set_format(PdfBitmapFormat::BGRA)
            .set_reverse_byte_order(false)
            .render_annotations(true)
            .clear_before_rendering(true)
            .set_clear_color(PdfColor::WHITE);

        let id = create_annotation(
            &mut document,
            0,
            &NewAnnotation::Highlight(Rect::from_xywh(100.0, 600.0, 200.0, 40.0)),
        )
        .expect("create");

        /// One pixel at the centre of the band, as native BGRA bytes.
        fn sample(document: &PdfDocument<'_>, config: &PdfRenderConfig) -> [u8; 4] {
            let page = document.pages().get(0).expect("page");
            let bitmap = page.render_with_config(config).expect("render");
            let data = bitmap.as_raw_bytes();
            let width = bitmap.width() as usize;
            let height = bitmap.height() as usize;
            let index = ((height - 620) * width + 200) * 4;
            [data[index], data[index + 1], data[index + 2], data[index + 3]]
        }

        // The highlight is born the yellow the canvas previews: B,G,R,A with
        // red and green on and blue off.
        assert_eq!(
            sample(&document, &config),
            [0, 255, 255, 255],
            "a freshly drawn highlight must be yellow"
        );

        // That render is exactly the state the real app is in when the user
        // opens the properties panel: Pdfium has now cached its own appearance
        // stream, and a naive `/C` write is rejected.
        assert!(
            set_annotation_color(&mut document, id, [255, 0, 0, 255]).expect("no PDFium error"),
            "recolouring a rendered highlight must apply"
        );
        assert_eq!(
            sample(&document, &config),
            [0, 0, 255, 255],
            "the recoloured highlight must be red, not the yellow it was born with"
        );

        // Opacity rides in the colour's alpha and composites over the page,
        // so 27% red over white is a pale red rather than a solid one.
        assert!(
            set_annotation_color(&mut document, id, [255, 0, 0, 90]).expect("no PDFium error"),
            "setting a highlight's opacity must apply"
        );
        let faded = sample(&document, &config);
        assert_eq!(faded[2], 255, "red channel stays saturated: {faded:?}");
        assert!(
            faded[1] > 0 && faded[1] < 255,
            "a 27% red over white must come out pale, got {faded:?}"
        );
        let _ = id;
    }


    /// PROBE: what a Highlight actually *renders*, measured with the engine's
    /// own render config so the byte order is the BGRA the app assumes.
    ///
    /// The engine sets `reverse_byte_order(false)`, so `as_raw_bytes()` is
    /// genuinely B,G,R,A. Note pdfium-render's `new()` default is `true`, which
    /// hands back R,G,B,A instead — using that here would make the bytes lie.
    #[test]
    fn probe_highlight_behaviour() {
        let Some(pdfium) = pdfium() else {
            eprintln!("SKIP: no PDFium library staged under target/native");
            return;
        };
        let mut document = blank_document(&pdfium);

        // The engine's exact configuration: native BGRA, annotations on,
        // cleared to white so a translucent mark composites visibly.
        let config = PdfRenderConfig::new()
            .scale_page_by_factor(1.0)
            .set_format(PdfBitmapFormat::BGRA)
            .set_reverse_byte_order(false)
            .render_annotations(true)
            .clear_before_rendering(true)
            .set_clear_color(PdfColor::WHITE);

        let id = create_annotation(
            &mut document,
            0,
            &NewAnnotation::Highlight(Rect::from_xywh(100.0, 600.0, 200.0, 40.0)),
        )
        .expect("create");

        /// Render and read one pixel inside the highlight, as BGRA bytes.
        fn sample(document: &PdfDocument<'_>, config: &PdfRenderConfig) -> [u8; 4] {
            let page = document.pages().get(0).expect("page");
            let bitmap = page.render_with_config(config).expect("render");
            let data = bitmap.as_raw_bytes();
            let width = bitmap.width() as usize;
            let height = bitmap.height() as usize;
            // PDF (200, 620) — centre of the band, y measured from the top.
            let i = ((height - 620) * width + 200) * 4;
            [data[i], data[i + 1], data[i + 2], data[i + 3]]
        }

        let show = |label: &str, px: [u8; 4]| {
            let (blue, green, red) = (px[0], px[1], px[2]);
            eprintln!("PROBE {label:<28} BGRA bytes {px:?}  -> rgb({red},{green},{blue})");
        };

        show("as created (yellow)", sample(&document, &config));

        // Recolour: does /C reach the pixels, or is a cached AP in the way?
        set_annotation_color(&mut document, id, [255, 0, 0, 255]).expect("recolour red");
        show("after recolour red", sample(&document, &config));

        // Does the alpha byte in /C composite over the white page, or is the
        // mark always opaque?
        set_annotation_color(&mut document, id, [255, 0, 0, 90]).expect("recolour faded");
        show("after recolour red @27%", sample(&document, &config));
    }


    /// Bind PDFium, or `None` when the native library has not been staged.
    fn pdfium() -> Option<Pdfium> {
        let dir = find_pdfium()?;
        let bindings = Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(&dir))
            .or_else(|_| Pdfium::bind_to_system_library())
            .ok()?;
        Some(Pdfium::new(bindings))
    }

    /// A one-page document ready to be marked up.
    fn blank_document(pdfium: &Pdfium) -> PdfDocument<'_> {
        let mut document = pdfium.create_new_pdf().expect("new document");
        document
            .pages_mut()
            .create_page_at_end(PdfPagePaperSize::a4())
            .expect("add a page");
        document
    }

    /// Vertical ink extent of the page, in pixels.
    fn ink_height(document: &mut PdfDocument<'_>) -> usize {
        let bbox = ink_bbox(document);
        bbox.3.saturating_sub(bbox.1)
    }

    /// The five annotation kinds the vertical-resize contract covers, each with
    /// the `NewAnnotation` that creates it and the box it starts in.
    ///
    /// `Highlight`, `Underline`, `StrikeOut` and `Squiggly` are all drawn by
    /// Pdfium from an appearance stream; `Square` is drawn from its `/Rect`.
    fn vertical_resize_cases() -> Vec<(&'static str, fn(Rect) -> NewAnnotation)> {
        vec![
            ("highlight", NewAnnotation::Highlight),
            ("underline", NewAnnotation::Underline),
            ("strikeout", NewAnnotation::StrikeOut),
            ("squiggly", NewAnnotation::Squiggly),
            ("rectangle", NewAnnotation::Square),
        ]
    }

    /// The contract the user reported broken: dragging a top or bottom handle
    /// must change the annotation's VERTICAL size on the rendered page.
    ///
    /// This is the test that used to fail for every markup kind. Pdfium's
    /// appearance generator draws underline / strikeout / squiggly as a
    /// hard-coded 1pt line and ignores both `/BS /W` and `/Border`, so the box
    /// height had no effect on the ink at all; those kinds now get an appearance
    /// stream built from the box instead. Highlight scales with its quad and
    /// rectangle with its `/Rect`, so all five are covered here.
    #[test]
    fn every_annotation_kind_grows_vertically_when_resized_vertically() {
        let Some(pdfium) = pdfium() else {
            eprintln!("SKIP: no PDFium library staged under target/native");
            return;
        };

        for (name, make) in vertical_resize_cases() {
            let mut document = blank_document(&pdfium);
            let start = Rect::from_xywh(100.0, 620.0, 200.0, 40.0);
            let id = create_annotation(&mut document, 0, &make(start)).expect("create");
            // Render before resizing: the first render is what would otherwise
            // bake Pdfium's own appearance in.
            let before = ink_height(&mut document);

            let taller = Rect::from_xywh(100.0, 620.0, 200.0, 140.0);
            assert!(
                update_annotation_bounds(&mut document, id, taller).expect("resize"),
                "{name}: resize must apply"
            );
            let after = ink_height(&mut document);

            assert!(
                after > before,
                "{name}: a vertical resize must grow the ink vertically \
                 (before {before}px, after {after}px)"
            );
        }
    }

    /// Shrinking vertically must thin the mark again, not leave it at its old
    /// size — the other direction of the same contract.
    #[test]
    fn every_annotation_kind_shrinks_vertically_when_resized_vertically() {
        let Some(pdfium) = pdfium() else {
            eprintln!("SKIP: no PDFium library staged under target/native");
            return;
        };

        for (name, make) in vertical_resize_cases() {
            let mut document = blank_document(&pdfium);
            let id = create_annotation(
                &mut document,
                0,
                &make(Rect::from_xywh(100.0, 620.0, 200.0, 40.0)),
            )
            .expect("create");

            // Grow first, so the mark is genuinely thick before it is thinned.
            let grown = Rect::from_xywh(100.0, 620.0, 200.0, 160.0);
            assert!(update_annotation_bounds(&mut document, id, grown).expect("grow"));
            let before = ink_height(&mut document);

            let shorter = Rect::from_xywh(100.0, 620.0, 200.0, 30.0);
            assert!(update_annotation_bounds(&mut document, id, shorter).expect("shrink"));
            let after = ink_height(&mut document);

            assert!(
                after < before,
                "{name}: shrinking vertically must thin the ink \
                 (before {before}px, after {after}px)"
            );
        }
    }

    /// The rendered height of the page's ink, in pixels, at `scale`.
    ///
    /// Measured as ink *coverage* divided by the mark's own width, not as a
    /// count of touched pixels: a stroke thinner than a device pixel is spread
    /// across rows by the rasterizer, so counting touched pixels saturates at
    /// one row and hides the difference between two weights.
    fn rendered_ink_height(document: &mut PdfDocument<'_>, scale: f32, mark_width_pt: f32) -> f32 {
        let page = document.pages().get(0).expect("page");
        let config = PdfRenderConfig::new()
            .scale_page_by_factor(scale)
            .set_format(PdfBitmapFormat::BGRA)
            .render_annotations(true)
            .clear_before_rendering(true)
            .set_clear_color(PdfColor::WHITE);
        let bitmap = page.render_with_config(&config).expect("render");
        let coverage: f32 = bitmap
            .as_raw_bytes()
            .chunks_exact(4)
            .map(|p| {
                let mean = (u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])) as f32 / 3.0;
                (255.0 - mean) / 255.0
            })
            .sum();
        coverage / (mark_width_pt * scale)
    }

    /// The engine draws a line markup's weight in *points*, so its thickness on
    /// screen grows with the render scale.
    ///
    /// This is the premise the canvas preview rests on: it previews the mark at
    /// `PREVIEW_STROKE_PT × zoom` screen pixels, which only matches what the
    /// engine draws if the rendered weight scales with the zoom. A weight fixed
    /// in pixels is what made the mark look thicker than its preview the moment
    /// the drag ended on a zoomed page.
    #[test]
    fn the_rendered_stroke_weight_scales_with_the_render_scale() {
        let Some(pdfium) = pdfium() else {
            eprintln!("SKIP: no PDFium library staged under target/native");
            return;
        };

        // A blank page, so the only ink in the render is the mark's.
        //
        // Scales of 4x and up: the mark is a hairline, and a stroke thinner
        // than a device pixel is widened by the rasterizer so it stays visible.
        // The same 0.25pt rule therefore measures 0.68px at 1x and 0.86px at 2x,
        // then exactly 1.00px at 4x, 2.00px at 8x and 4.01px at 16x — the floor
        // flattens the weight at low zoom, and above it the weight is exact.
        let width_pt = 200.0;
        let mut measured = Vec::new();
        for scale in [4.0_f32, 8.0, 16.0] {
            let mut document = blank_document(&pdfium);
            create_annotation(
                &mut document,
                0,
                &NewAnnotation::Underline(Rect::from_xywh(100.0, 400.0, width_pt, 14.0)),
            )
            .expect("create");
            measured.push((scale, rendered_ink_height(&mut document, scale, width_pt)));
        }

        for (scale, height) in &measured {
            let expected = DEFAULT_MARKUP_THICKNESS * scale;
            assert!(
                (height - expected).abs() < 0.1 * scale,
                "at {scale}x the mark must render {expected:.2}px thick — the \
                 weight the canvas previews — but it measured {height:.2}px"
            );
        }

        // Proportional, not merely monotonic: the ratio of heights is the ratio
        // of scales, which is what makes the weight a document width.
        let (s0, h0) = measured[0];
        let (s1, h1) = *measured.last().expect("measured");
        let (ratio, expected) = (h1 / h0, s1 / s0);
        assert!(
            (ratio - expected).abs() < 0.35,
            "the stroke must scale in proportion to the render scale: \
             {ratio:.2}x over a {expected:.2}x change ({measured:?})"
        );
    }

    /// A squiggly mark's wave must be small, and must not shrink with its
    /// stroke weight.
    ///
    /// The wave was derived from the weight, which was invisible while the
    /// weight was 1.4pt and wrong the moment it became a hairline: the wave
    /// would have collapsed to a straight line. It is a fixed document size, so
    /// thinning the rule must thin the ink without flattening the wave.
    #[test]
    fn a_squiggly_keeps_its_small_wave_when_the_stroke_is_thinned() {
        let Some(pdfium) = pdfium() else {
            eprintln!("SKIP: no PDFium library staged under target/native");
            return;
        };

        // At 4:1 the wave is several pixels tall, so it can be measured. Its
        // ink spans the rise plus the stroke, one half either side.
        const SCALE: f32 = 4.0;
        let band = 14.0;
        let mut heights = Vec::new();
        for shrink in [0.0_f32, 1.0] {
            let mut document = blank_document(&pdfium);
            let id = create_annotation(
                &mut document,
                0,
                &NewAnnotation::Squiggly(Rect::from_xywh(100.0, 400.0, 200.0, band)),
            )
            .expect("create");
            if shrink > 0.0 {
                let thinned = Rect::from_xywh(100.0, 400.0, 200.0, band - shrink);
                assert!(
                    update_annotation_bounds(&mut document, id, thinned).expect("thin"),
                    "the thinning resize must apply"
                );
            }
            let bbox = ink_bbox_at(&mut document, SCALE);
            let height = bbox.3.saturating_sub(bbox.1);
            heights.push((shrink, height));
        }

        let (_, full) = heights[0];
        let (_, thin) = heights[1];
        let wave = SQUIGGLY_RISE_PT * SCALE;
        let flat = DEFAULT_MARKUP_THICKNESS * SCALE;

        // A wave, not a flat rule: clearly taller than the stroke alone.
        assert!(
            full as f32 > flat * 1.5,
            "the mark must be drawn as a wave, not a straight rule: {full}px of ink \
             for a {wave:.1}px wave over a {flat:.1}px stroke"
        );
        // ... and small: nowhere near a tall saw.
        assert!(
            full as f32 <= wave * 2.0,
            "the wave must stay small, but the mark measured {full}px for a \
             {wave:.1}px rise"
        );
        // Thinning the stroke must not flatten it.
        assert!(
            (full as i64 - thin as i64).unsigned_abs() <= 2,
            "thinning the stroke must not flatten the wave: {full}px tall before, \
             {thin}px after"
        );
    }

    /// The weight of a box annotation's border, in points, measured by scanning
    /// a column down the box's centre: it crosses the top and bottom borders, so
    /// half the coverage is one border.
    ///
    /// The ink is normalised by the darkest pixel in the column rather than by
    /// full black, because the border is drawn in the annotation's own colour —
    /// a red rule never reaches `255 - mean == 255`. `scale` must be high enough
    /// that some pixel is fully covered, or the peak is a partial pixel and the
    /// weight comes out too large.
    fn rendered_border_weight(document: &mut PdfDocument<'_>, scale: f32, rect: Rect) -> f32 {
        /// Slack past each edge, in points, so the border's outer half is not
        /// clipped by the scan.
        const PAD: f32 = 4.0;
        let page = document.pages().get(0).expect("page");
        let page_height = page.height().value;
        let config = PdfRenderConfig::new()
            .scale_page_by_factor(scale)
            .set_format(PdfBitmapFormat::BGRA)
            .render_annotations(true)
            .clear_before_rendering(true)
            .set_clear_color(PdfColor::WHITE);
        let bitmap = page.render_with_config(&config).expect("render");
        let (width, height) = (bitmap.width() as usize, bitmap.height() as usize);
        let data = bitmap.as_raw_bytes();
        let coverage = |x: usize, y: usize| -> f32 {
            let i = (y * width + x) * 4;
            let mean =
                (u32::from(data[i]) + u32::from(data[i + 1]) + u32::from(data[i + 2])) as f32 / 3.0;
            (255.0 - mean) / 255.0
        };
        // Page space is y-up and the bitmap y-down, so the box's top edge is the
        // smaller bitmap y. The range is padded past both edges: the border
        // straddles the rectangle, so half of it lies outside.
        let x = ((rect.min_x + rect.max_x) / 2.0 * scale) as usize;
        let top = ((page_height - rect.max_y - PAD) * scale).max(0.0) as usize;
        let bottom = ((page_height - rect.min_y + PAD) * scale).min(height as f32 - 1.0) as usize;
        let profile: Vec<f32> = (top..=bottom).map(|y| coverage(x, y)).collect();
        let peak = profile.iter().copied().fold(0.0_f32, f32::max);
        assert!(peak > 0.1, "the border must be drawn at all");
        profile.iter().sum::<f32>() / (2.0 * peak) / scale
    }

    /// A rectangle must be drawn at the weight the canvas previews, like every
    /// other drawn mark.
    ///
    /// Pdfium's own default `Square` border is about 0.67pt — half the previewed
    /// weight, so the outline changed the moment the drag ended, and would have
    /// drifted further once the preview started scaling with the zoom.
    #[test]
    fn a_rectangle_is_drawn_at_the_weight_the_canvas_previews() {
        let Some(pdfium) = pdfium() else {
            eprintln!("SKIP: no PDFium library staged under target/native");
            return;
        };

        let rect = Rect::from_xywh(100.0, 400.0, 140.0, 80.0);
        // High scales only: the measurement normalises against the darkest
        // pixel, which needs at least one fully covered row, and the border is a
        // hairline.
        for scale in [4.0_f32, 8.0] {
            let mut document = blank_document(&pdfium);
            create_annotation(&mut document, 0, &NewAnnotation::Square(rect)).expect("create");
            let weight = rendered_border_weight(&mut document, scale, rect);
            assert!(
                (weight - DEFAULT_MARKUP_THICKNESS).abs() < 0.2,
                "at {scale}x the rectangle's border must be drawn at \
                 {DEFAULT_MARKUP_THICKNESS}pt — the weight the canvas previews — \
                 but it measured {weight:.2}pt"
            );
        }
    }

    /// A line markup must be thinnable *below* the weight it was drawn at.
    ///
    /// This is the "when I resize I can't make it very very thin, it locks
    /// suddenly" report: the weight floor used to be
    /// [`DEFAULT_MARKUP_THICKNESS`] itself, so every downward drag clamped
    /// straight back to 1.4pt and the stroke stopped dead the moment the user
    /// tried to thin it. Pure arithmetic, so no PDFium needed.
    #[test]
    fn a_downward_drag_thins_a_line_markup_past_its_drawn_weight() {
        // A text-line band, shrunk to the smallest box the canvas allows.
        let thinned = dragged_thickness(DEFAULT_MARKUP_THICKNESS, 14.0, 8.0);
        assert!(
            thinned < DEFAULT_MARKUP_THICKNESS,
            "a downward drag must thin the mark below the weight it was drawn \
             at, got {thinned}pt"
        );
        assert_eq!(thinned, MIN_MARKUP_THICKNESS);

        // It bottoms out at the floor rather than vanishing.
        assert_eq!(
            dragged_thickness(DEFAULT_MARKUP_THICKNESS, 14.0, 0.5),
            MIN_MARKUP_THICKNESS
        );

        // Thinning is continuous away from the floor: a 1pt nudge is a 1pt
        // thinner stroke.
        assert!(
            (dragged_thickness(6.0, 14.0, 13.0) - 5.0).abs() < 1e-4,
            "a 1pt drag must be a 1pt thinner stroke"
        );

        // A horizontal drag changes no height, so it changes no weight.
        assert_eq!(dragged_thickness(6.0, 14.0, 14.0), 6.0);

        // The floor sits below the drawn weight, so a downward drag can always
        // thin the mark at least a little.
        assert!(
            MIN_MARKUP_THICKNESS < DEFAULT_MARKUP_THICKNESS,
            "the floor must be below the drawn weight, or a downward drag could not thin"
        );
    }

    /// The applied mark must look like the one that was drawn: dragging the
    /// underline tool over a tall band leaves a thin underline, not a solid bar
    /// as tall as the band. Thickness only grows once the mark is resized
    /// vertically, which is why the box height at creation is not a thickness.
    #[test]
    fn a_drawn_line_markup_stays_thin_however_tall_the_drag_was() {
        let Some(pdfium) = pdfium() else {
            eprintln!("SKIP: no PDFium library staged under target/native");
            return;
        };

        for (name, make) in [
            ("underline", NewAnnotation::Underline as fn(Rect) -> NewAnnotation),
            ("strikeout", NewAnnotation::StrikeOut),
            ("squiggly", NewAnnotation::Squiggly),
        ] {
            let mut document = blank_document(&pdfium);
            // A deliberately tall band: 200pt.
            let id = create_annotation(
                &mut document,
                0,
                &make(Rect::from_xywh(100.0, 500.0, 200.0, 200.0)),
            )
            .expect("create");

            let height = ink_height(&mut document);
            assert!(
                height <= 8,
                "{name}: a freshly drawn mark must be as thin as the preview, \
                 got {height}px of ink in a 200pt band"
            );

            // The band itself is what the user dragged, so the mark still spans
            // it horizontally and keeps its bounds.
            let rect = list_annotations(&document)
                .into_iter()
                .find(|x| x.id == id)
                .expect("listed")
                .rect;
            assert!(
                (rect.max_x - rect.min_x - 200.0).abs() < 1.0,
                "{name}: the mark must keep its width, got {rect:?}"
            );

            // ... and a vertical drag is what thickens it.
            let taller = Rect::from_xywh(100.0, 500.0, 200.0, 260.0);
            assert!(update_annotation_bounds(&mut document, id, taller).expect("resize"));
            assert!(
                ink_height(&mut document) > height + 40,
                "{name}: after a vertical drag the mark must be much thicker"
            );
        }
    }

    /// A line markup must be drawn in the colour its tool previews — a
    /// strikeout is red and a squiggly green the moment they are applied, not
    /// Pdfium's black default.
    ///
    /// This is a rendering assertion on purpose: Pdfium refuses to *report* a
    /// colour while an appearance stream is present, so reading `/C` back would
    /// prove nothing.
    #[test]
    fn created_line_markups_are_drawn_in_their_preview_colour() {
        let Some(pdfium) = pdfium() else {
            eprintln!("SKIP: no PDFium library staged under target/native");
            return;
        };

        // (name, constructor, wants red ink, wants green ink)
        for (name, make, red, green) in [
            (
                "underline",
                NewAnnotation::Underline as fn(Rect) -> NewAnnotation,
                false,
                false,
            ),
            ("strikeout", NewAnnotation::StrikeOut, true, false),
            ("squiggly", NewAnnotation::Squiggly, false, true),
        ] {
            let mut document = blank_document(&pdfium);
            create_annotation(
                &mut document,
                0,
                &make(Rect::from_xywh(100.0, 500.0, 200.0, 40.0)),
            )
            .expect("create");
            let (ink, redness, greenness) = ink_stats(&mut document);
            assert!(ink > 100, "{name}: the mark must be drawn at all");

            if red {
                assert!(
                    redness > 500,
                    "{name}: must be drawn red, got redness {redness} over {ink} inked pixels"
                );
            }
            if green {
                assert!(
                    greenness > 500,
                    "{name}: must be drawn green, got greenness {greenness} over {ink} inked pixels"
                );
            }
            if !red && !green {
                assert!(
                    redness.abs() < 500 && greenness.abs() < 500,
                    "{name}: must be drawn in neutral ink, got redness {redness}, \
                     greenness {greenness}"
                );
            }
        }
    }

    /// Horizontal-only resizing must leave the vertical size alone. This is the
    /// "do not break horizontal resizing" half of the contract.
    #[test]
    fn horizontal_resize_does_not_change_the_vertical_extent() {
        let Some(pdfium) = pdfium() else {
            eprintln!("SKIP: no PDFium library staged under target/native");
            return;
        };

        for (name, make) in vertical_resize_cases() {
            let mut document = blank_document(&pdfium);
            let start = Rect::from_xywh(100.0, 620.0, 120.0, 40.0);
            let id = create_annotation(&mut document, 0, &make(start)).expect("create");
            let before = ink_height(&mut document);
            let bbox_before = ink_bbox(&mut document);

            let wider = Rect::from_xywh(100.0, 620.0, 300.0, 40.0);
            assert!(update_annotation_bounds(&mut document, id, wider).expect("resize"));
            let after = ink_height(&mut document);
            let bbox_after = ink_bbox(&mut document);

            assert!(
                bbox_after.2 > bbox_before.2 + 100,
                "{name}: a horizontal resize must widen the ink \
                 ({bbox_before:?} -> {bbox_after:?})"
            );
            assert!(
                after.abs_diff(before) <= 2,
                "{name}: a horizontal resize must not change the vertical extent \
                 (before {before}px, after {after}px)"
            );
        }
    }

    /// Recolouring a line markup must be visible. Its colour lives inside the
    /// appearance stream we generate, so a `/C` write on its own would change
    /// nothing on screen.
    #[test]
    fn recolouring_a_line_markup_rebuilds_its_appearance() {
        let Some(pdfium) = pdfium() else {
            eprintln!("SKIP: no PDFium library staged under target/native");
            return;
        };

        let mut document = blank_document(&pdfium);
        let id = create_annotation(
            &mut document,
            0,
            &NewAnnotation::Underline(Rect::from_xywh(100.0, 500.0, 200.0, 60.0)),
        )
        .expect("create");
        // Black underline on white paper.
        let (before_ink, before_redness, _) = ink_stats(&mut document);

        assert!(
            set_annotation_color(&mut document, id, [255, 0, 0, 255]).expect("recolour"),
            "recolour must apply"
        );
        let (after_ink, after_redness, _) = ink_stats(&mut document);

        // Red and black ink are both "non-white", so the pixel count is a poor
        // witness — it must merely not lose the mark. The witness that matters
        // is how much redder the ink got.
        assert!(
            after_ink > before_ink / 2,
            "recolouring must not destroy the mark ({before_ink} -> {after_ink} inked pixels)"
        );
        assert!(
            after_redness > before_redness + 100,
            "the recoloured underline must actually render red \
             (redness {before_redness} -> {after_redness})"
        );
    }

    /// Pdfium cannot report a colour for an annotation that carries an
    /// appearance stream, so the colour of a line markup — and therefore the
    /// swatch the properties panel shows — is read back from the stream we
    /// generated. This is the round-trip the smoke test depends on.
    #[test]
    fn a_line_markups_colour_reads_back_from_its_appearance() {
        let Some(pdfium) = pdfium() else {
            eprintln!("SKIP: no PDFium library staged under target/native");
            return;
        };

        for (name, make, expected) in [
            (
                "underline",
                NewAnnotation::Underline as fn(Rect) -> NewAnnotation,
                [0, 0, 0, 255],
            ),
            ("strikeout", NewAnnotation::StrikeOut, [211, 47, 47, 255]),
            ("squiggly", NewAnnotation::Squiggly, [67, 160, 71, 255]),
        ] {
            let mut document = blank_document(&pdfium);
            let id = create_annotation(
                &mut document,
                0,
                &make(Rect::from_xywh(100.0, 500.0, 200.0, 40.0)),
            )
            .expect("create");
            let listed = list_annotations(&document)
                .into_iter()
                .find(|x| x.id == id)
                .expect("listed");
            assert_eq!(
                listed.color,
                Some(expected),
                "{name}: the colour must be reportable"
            );

            // A recolour must round-trip too.
            let recoloured = [12, 34, 56, 78];
            assert!(set_annotation_color(&mut document, id, recoloured).expect("recolour"));
            let listed = list_annotations(&document)
                .into_iter()
                .find(|x| x.id == id)
                .expect("listed");
            assert_eq!(
                listed.color,
                Some(recoloured),
                "{name}: a recolour must round-trip"
            );
        }
    }

    /// How the page's ink is coloured: inked pixel count, its red excess over
    /// green, and its green excess over red.
    ///
    /// Both excesses are zero for greyscale ink, which makes them a colour
    /// witness independent of the bitmap's alpha byte. Red and blue swap places
    /// between the RGBA and BGRA layouts, so the comparison deliberately uses
    /// the green byte (always second) against the larger of the other two.
    fn ink_stats(document: &mut PdfDocument<'_>) -> (usize, i64, i64) {
        let page = document.pages().get(0).expect("page");
        let config = PdfRenderConfig::new()
            .scale_page_by_factor(1.0)
            .set_format(PdfBitmapFormat::BGRA)
            .render_annotations(true)
            .clear_before_rendering(true)
            .set_clear_color(PdfColor::WHITE);
        let bitmap = page.render_with_config(&config).expect("render");
        let data = bitmap.as_raw_bytes();
        let mut ink = 0;
        let mut redness = 0i64;
        let mut greenness = 0i64;
        for p in data.chunks_exact(4) {
            if p[0] > 240 && p[1] > 240 && p[2] > 240 {
                continue;
            }
            ink += 1;
            let warm = i64::from(p[0].max(p[2]));
            let green = i64::from(p[1]);
            redness += warm - green;
            greenness += green - warm;
        }
        (ink, redness, greenness)
    }

}

