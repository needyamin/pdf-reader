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
                .map(|c| [c.red(), c.green(), c.blue(), c.alpha()]);
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
            // Leaving `/IC` alone gives the border PDFium's default width and
            // still lets the page text show through, which is what an outline
            // is for.
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
    let raw: Vec<_> = page.annotations().iter().collect();
    let index = raw
        .iter()
        .enumerate()
        .rev()
        .find(|(_, annotation)| annotation.annotation_type() == wanted)
        .map(|(index, _)| index)
        .unwrap_or(count - 1);
    Ok(AnnotationId::new(page_index, index as u32))
}

/// Set the bounds and quad points of a markup annotation over `rect`.
fn place_markup<'a>(
    annot: &mut impl MarkupAnnotation<'a>,
    rect: Rect,
    color: PdfColor,
) -> Result<(), pdfium_render::prelude::PdfiumError> {
    annot.set_bounds(pdf_rect(rect))?;
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
    // Pdfium generates a markup annotation's appearance stream the first time it
    // renders the annotation and then *caches* it. Changing the quad points (a
    // resize) leaves that cached appearance in place, so the mark keeps drawing
    // at its old size no matter what the annotation dictionary now says —
    // exactly why highlights, underlines and squigglies would not resize while
    // the bounds-based rectangle did. Clearing the stream makes Pdfium rebuild
    // it from the new geometry. Best-effort: a document that refuses the write
    // simply keeps Pdfium's existing appearance.
    let _ = annotation.remove_appearance_stream();
    page.regenerate_content()?;
    Ok(true)
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
    match annotation.set_stroke_color(PdfColor::new(color[0], color[1], color[2], color[3])) {
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
        let page = document.pages().get(0).expect("page");
        let config = PdfRenderConfig::new()
            .scale_page_by_factor(1.0)
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
}
