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
    PdfColor, PdfDocument, PdfFormFieldCommon as _, PdfPageAnnotationCommon,
    PdfPageAnnotationType, PdfPoints, PdfQuadPoints,
};

use pdfreader_core::{
    AnnotationId, AnnotationInfo, AnnotationKind, NewAnnotation, Rect,
};

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
            if matches!(kind, AnnotationKind::Other) && is_hidden_type(annotation.annotation_type()) {
                continue;
            }
            let Ok(bounds) = annotation.bounds() else {
                continue;
            };
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
pub(crate) fn create_annotation(
    document: &mut PdfDocument<'_>,
    page: u32,
    new: &NewAnnotation,
) -> Result<AnnotationId, pdfium_render::prelude::PdfiumError> {
    let page_index = page;
    let mut page = document
        .pages_mut()
        .get(page as i32)
        .map_err(|error| pdfium_render::prelude::PdfiumError::IoError(std::io::Error::new(
            std::io::ErrorKind::Other,
            error.to_string(),
        )))?;
    let annotations = page.annotations_mut();

    match new {
        NewAnnotation::Highlight(rect) => {
            let mut annot = annotations.create_highlight_annotation()?;
            place_markup(&mut annot, *rect, PdfColor::YELLOW)?;
        }
        NewAnnotation::Underline(rect) => {
            let mut annot = annotations.create_underline_annotation()?;
            place_markup(&mut annot, *rect, PdfColor::BLACK)?;
        }
        NewAnnotation::StrikeOut(rect) => {
            let mut annot = annotations.create_strikeout_annotation()?;
            place_markup(&mut annot, *rect, PdfColor::RED)?;
        }
        NewAnnotation::Squiggly(rect) => {
            let mut annot = annotations.create_squiggly_annotation()?;
            place_markup(&mut annot, *rect, PdfColor::GREEN)?;
        }
        NewAnnotation::Square(rect) => {
            let mut annot = annotations.create_square_annotation()?;
            annot.set_bounds(pdf_rect(*rect))?;
            annot.set_stroke_color(PdfColor::RED)?;
            // Outline only: a zero-alpha fill keeps the page text visible.
            annot.set_fill_color(PdfColor::new(255, 0, 0, 0))?;
        }
        NewAnnotation::StickyNote((x, y), contents) => {
            let mut annot = annotations.create_text_annotation(contents)?;
            annot.set_position(PdfPoints::new(*x), PdfPoints::new(*y))?;
            annot.set_stroke_color(PdfColor::YELLOW)?;
            if contents.is_empty() {
                annot.set_contents("Note")?;
            }
        }
        NewAnnotation::FreeText(rect, contents) => {
            let mut annot = annotations.create_free_text_annotation(contents)?;
            annot.set_bounds(pdf_rect(*rect))?;
            annot.set_stroke_color(PdfColor::BLACK)?;
            annot.set_contents(contents)?;
        }
    }

    page.regenerate_content()?;
    // PDFium appends new annotations to the end of the page's list; popups are
    // hidden from `list_annotations` but still occupy indices, so the created
    // widget is the LAST annotation on the page.
    let count = page.annotations().len();
    Ok(AnnotationId::new(page_index, (count - 1) as u32))
}

/// Set the bounds and quad points of a markup annotation over `rect`.
fn place_markup<'a>(
    annot: &mut impl MarkupAnnotation<'a>,
    rect: Rect,
    color: PdfColor,
) -> Result<(), pdfium_render::prelude::PdfiumError> {
    annot.set_bounds(pdf_rect(rect))?;
    annot.set_stroke_color(color)?;
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
    fn attachment_points_mut(&mut self) -> &mut pdfium_render::prelude::PdfPageAnnotationAttachmentPoints<'a>;
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

/// Build a PDFium rect from our y-up rectangle.
fn pdf_rect(rect: Rect) -> pdfium_render::prelude::PdfRect {
    pdfium_render::prelude::PdfRect::new(
        PdfPoints::new(rect.min_y),
        PdfPoints::new(rect.min_x),
        PdfPoints::new(rect.max_y),
        PdfPoints::new(rect.max_x),
    )
}

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
                if let Some(mut text) =
                    annotation.as_form_field_mut().and_then(|f| f.as_text_field_mut())
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
