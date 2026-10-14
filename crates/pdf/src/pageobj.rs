//! Reading and changing the objects a page is made of.
//!
//! # What this can and cannot do
//!
//! PDFium hands out a page as a flat list of objects — text runs, images, vector
//! paths, shadings. This module exposes the two the editor can change, and the
//! operations PDFium actually supports on them:
//!
//! - read the list, with each object's box and (for text) its string and size;
//! - move or resize an object by transforming it;
//! - delete an object;
//! - rewrite a text run's string;
//! - swap an image's pixels;
//! - add a new text object.
//!
//! It deliberately does **not** promise more than PDFium can do. There is no
//! paragraph reflow: a PDF has positioned text runs, not a Word-style document
//! layout tree. When text is edited, the run keeps the font and point size it
//! already had. A longer string may extend past its old ink box; it is not
//! shrunk to fit, because that changed the type the page was set in. No
//! adjacent objects move.
//!
//! # Why every mutation ends with `regenerate_content`
//!
//! Object changes are staged in PDFium's page object list and only reach the
//! page's content stream when it is regenerated. Without that call the change
//! shows up in a rasterization but not in `save_to_bytes`, so it would be lost
//! on save. `formmark` learned this first; every function here follows it.

use std::path::Path;

use pdfium_render::prelude::*;
use pdfreader_core::{
    PageObjectId, PageObjectInfo, PageObjectKind, Rect, RestackTarget, TextFormat,
};

use crate::Result;
use crate::error::EngineError;
use crate::export::pdfium_error;

/// The page at `index`, or a range error.
///
/// Takes the document mutably because every caller here eventually mutates the
/// page; reading through `pages_mut` costs nothing extra.
fn page_of<'a>(document: &'a mut PdfDocument<'static>, index: u32) -> Result<PdfPage<'a>> {
    let count = u32::try_from(document.pages().len()).unwrap_or(u32::MAX);
    document
        .pages_mut()
        .get(index as PdfPageIndex)
        .map_err(|_| EngineError::PageOutOfRange { index, count })
}

/// The kind the editor can change, or `None` for everything it cannot.
///
/// Takes the type rather than the object so it can be tested without a document
/// to hang a page object off.
fn editable_kind(kind: PdfPageObjectType) -> Option<PageObjectKind> {
    match kind {
        PdfPageObjectType::Text => Some(PageObjectKind::Text),
        PdfPageObjectType::Image => Some(PageObjectKind::Image),
        _ => None,
    }
}

/// PDFium's rectangle, in core's bottom-left space.
fn rect_of(rect: PdfRect) -> Rect {
    Rect::from_xywh(
        rect.left().value,
        rect.bottom().value,
        rect.right().value - rect.left().value,
        rect.top().value - rect.bottom().value,
    )
}

/// One object's record, as the editor sees it.
fn info_of(object: &PdfPageObject<'_>, page: u32, index: usize) -> Option<PageObjectInfo> {
    let kind = editable_kind(object.object_type())?;
    let quad = object.bounds().ok()?;
    let (text, font_size, format) = match object.as_text_object() {
        Some(text) => (
            // PDFium's text read-back appends a trailing space that was never
            // in the run. Leaving it on makes a retype look like the words
            // changed, and an exact search misses the object.
            Some(text.text().trim_end().to_string()),
            // `set_unscaled_font_size` takes this value. Using scaled size here
            // would multiply in the text object's transform a second time when
            // fitting a moved or scaled run.
            Some(text.unscaled_font_size().value),
            // Bold is PDFium's filled-and-stroked render mode. The fill is the
            // ink the page was set in; undo and Reset style write this back.
            TextFormat {
                bold: text.render_mode() == PdfPageTextRenderMode::FilledThenStroked,
                color: object
                    .fill_color()
                    .ok()
                    .map(|color| [color.red(), color.green(), color.blue(), color.alpha()]),
                size: Some(text.unscaled_font_size().value),
            },
        ),
        None => (None, None, TextFormat::default()),
    };
    let bounds = rect_of(quad.to_rect());
    Some(PageObjectInfo {
        id: PageObjectId {
            page,
            index: u32::try_from(index).unwrap_or(u32::MAX),
        },
        kind,
        bounds,
        layout_bounds: bounds,
        text,
        font_size,
        format,
    })
}

/// List the editable objects on a page, in object order.
pub(crate) fn list(document: &mut PdfDocument<'static>, page: u32) -> Result<Vec<PageObjectInfo>> {
    let page_handle = page_of(document, page)?;
    let objects = page_handle.objects();

    let mut found = Vec::new();
    for index in 0..objects.len() {
        let Ok(object) = objects.get(index) else {
            continue;
        };
        if let Some(info) = info_of(&object, page, index) {
            found.push(info);
        }
    }
    Ok(found)
}

/// Move or resize one object by transforming it.
///
/// Expressed as the transform that carries the object's current box onto the
/// target one, so a pure move and a resize are the same operation with different
/// scale factors. `Ok(false)` means the index no longer holds a transformable
/// object, which is not an error — ids are positional and may have shifted.
pub(crate) fn set_bounds(
    document: &mut PdfDocument<'static>,
    id: PageObjectId,
    rect: Rect,
) -> Result<bool> {
    let mut page = page_of(document, id.page)?;
    let index = id.index as PdfPageObjectIndex;

    {
        let objects = page.objects_mut();
        if index >= objects.len() {
            return Ok(false);
        }
        let mut object = objects.get(index).map_err(pdfium_error)?;
        if editable_kind(object.object_type()).is_none() {
            return Ok(false);
        }
        let current = rect_of(object.bounds().map_err(pdfium_error)?.to_rect());
        if current.is_degenerate() {
            return Ok(false);
        }

        let sx = rect.width() / current.width();
        let sy = rect.height() / current.height();
        if !sx.is_finite() || !sy.is_finite() || sx == 0.0 || sy == 0.0 {
            return Ok(false);
        }

        // Scale about the object's own lower-left corner, then move that corner
        // to where the target's is. With `sx == sy == 1` this is exactly a
        // translation, which is why a drag needs no separate code path.
        let tx = rect.min_x - sx * current.min_x;
        let ty = rect.min_y - sy * current.min_y;
        object
            .transform(sx, 0.0, 0.0, sy, tx, ty)
            .map_err(pdfium_error)?;
    }

    page.regenerate_content().map_err(pdfium_error)?;
    Ok(true)
}

/// Remove one object.
pub(crate) fn delete(document: &mut PdfDocument<'static>, id: PageObjectId) -> Result<bool> {
    let mut page = page_of(document, id.page)?;
    let index = id.index as PdfPageObjectIndex;

    {
        let objects = page.objects_mut();
        if index >= objects.len() {
            return Ok(false);
        }
        // A stale index must not remove whichever object shifted into the slot.
        let object = objects.get(index).map_err(pdfium_error)?;
        if editable_kind(object.object_type()).is_none() {
            return Ok(false);
        }
    }

    page.objects_mut()
        .remove_object_at_index(index)
        .map_err(pdfium_error)?;
    page.regenerate_content().map_err(pdfium_error)?;
    Ok(true)
}

/// Remove the object matching a record.
///
/// Used to redo a delete. The recorded index cannot be trusted there: undoing
/// the delete re-created the object, which renumbered the page. The match is on
/// the parts of the record a renumbering cannot change.
pub(crate) fn delete_matching(
    document: &mut PdfDocument<'static>,
    info: &PageObjectInfo,
) -> Result<bool> {
    let mut page = page_of(document, info.id.page)?;

    let mut target = None;
    {
        let objects = page.objects();
        for index in 0..objects.len() {
            let Ok(object) = objects.get(index) else {
                continue;
            };
            let Some(candidate) = info_of(&object, info.id.page, index) else {
                continue;
            };
            if candidate.matches(info, pdfreader_core::pageobj::OBJECT_MATCH_TOLERANCE) {
                target = Some(index);
                break;
            }
        }
    }

    let Some(index) = target else {
        return Ok(false);
    };

    page.objects_mut()
        .remove_object_at_index(index)
        .map_err(pdfium_error)?;
    page.regenerate_content().map_err(pdfium_error)?;
    Ok(true)
}

/// Re-order a page object within the page's paint order.
///
/// PDF paints objects in list order: index 0 is the bottom, the last index the
/// top. "Bring to front" / "Send to back" remove the object and re-insert it at
/// the target position; undo restores an exact index. The object is matched by
/// record, like a redone delete, because removing it renumbers the page.
pub(crate) fn restack(
    document: &mut PdfDocument<'static>,
    info: &PageObjectInfo,
    target: pdfreader_core::pageobj::RestackTarget,
) -> Result<bool> {
    let mut page = page_of(document, info.id.page)?;

    let index = {
        let objects = page.objects();
        let mut found = None;
        for candidate in 0..objects.len() {
            let Ok(object) = objects.get(candidate) else {
                continue;
            };
            let Some(candidate_info) = info_of(&object, info.id.page, candidate) else {
                continue;
            };
            if candidate_info.matches(info, pdfreader_core::pageobj::OBJECT_MATCH_TOLERANCE) {
                found = Some(candidate);
                break;
            }
        }
        found.ok_or_else(|| {
            EngineError::Unsupported("This object is no longer on the page.".into())
        })?
    };

    let object = page
        .objects_mut()
        .remove_object_at_index(index)
        .map_err(pdfium_error)?;

    // The removal shifted every index above `index` down by one, so an exact
    // target past it aims one lower than before. `Front` is simply the end of
    // the now-shorter list.
    let last: u32 = u32::try_from(page.objects().len()).unwrap_or(u32::MAX);
    let index = u32::try_from(index).unwrap_or(u32::MAX);
    let target: u32 = match target {
        RestackTarget::Front => last,
        RestackTarget::Back => 0,
        RestackTarget::At(at) => {
            let at = at.min(last);
            if at > index { at - 1 } else { at }
        }
    };

    page.objects_mut()
        .insert_object_at_index(target as PdfPageObjectIndex, object)
        .map_err(pdfium_error)?;
    page.regenerate_content().map_err(pdfium_error)?;
    Ok(true)
}

/// Replace the string a text object holds, keeping its font and point size.
///
/// PDFs contain positioned text runs, not a Word-style paragraph layout tree.
/// This replaces one run; it does not push later runs around, and it does not
/// shrink the type to stay inside the old ink box. A longer string simply
/// extends past that box. The font object already on the run is left in place.
/// `FPDFText_SetText` can reset the size and the placement matrix, so both are
/// written back afterwards. The size is the one the run had, or the size the
/// user explicitly chose. The matrix is restored with `reset_matrix`: applying
/// it again would multiply the old placement by itself and throw the run off
/// the page.
/// Undo is the same call with the previous string, so it lands on the same
/// font and size.
///
/// `format` is the styling to apply — synthetic bold, a colour override, and
/// an optional size. `previous` and `previous_format` are accepted for
/// signature symmetry with undo; this function only writes `text` and `format`.
pub(crate) fn set_text(
    document: &mut PdfDocument<'static>,
    id: PageObjectId,
    _previous: &str,
    text: &str,
    _layout_bounds: Rect,
    base_font_size: f32,
    _previous_format: TextFormat,
    format: TextFormat,
) -> Result<bool> {
    // A text-only edit keeps the size the PDF was set in. A size the user
    // picked in the panel is `format.size` and is applied exactly.
    let start_size = format.size.unwrap_or(base_font_size);
    if !base_font_size.is_finite()
        || base_font_size <= 0.0
        || !start_size.is_finite()
        || start_size <= 0.0
    {
        return Err(EngineError::Unsupported(
            "This text has no usable font size.".into(),
        ));
    }

    let mut page = page_of(document, id.page)?;
    let index = id.index as PdfPageObjectIndex;

    {
        let objects = page.objects_mut();
        if index >= objects.len() {
            return Ok(false);
        }
        let mut object = objects.get(index).map_err(pdfium_error)?;
        // Captured before `set_text`, which can replace the matrix and the
        // size while leaving the font. Putting the matrix back is what keeps
        // the run on its baseline at the scale the page used.
        let original_matrix = object.matrix().ok();
        let Some(text_object) = object.as_text_object_mut() else {
            return Ok(false);
        };

        text_object.set_text(text).map_err(pdfium_error)?;
        text_object
            .set_unscaled_font_size(PdfPoints::new(start_size))
            .map_err(pdfium_error)?;
        // `apply_matrix` multiplies. SetText usually leaves the matrix in
        // place, so multiplying the captured one back on doubles the
        // translation and the scale and the glyphs leave the page. Reset
        // writes the captured matrix back only when the edit actually
        // replaced it.
        if let Some(matrix) = original_matrix {
            let changed = object.matrix().ok().as_ref() != Some(&matrix);
            if changed {
                object.reset_matrix(matrix).map_err(pdfium_error)?;
            }
        }

        // Bold and colour do not touch the matrix or the font.
        apply_format_inline(&mut object, &format, base_font_size);
    }

    page.regenerate_content().map_err(pdfium_error)?;
    Ok(true)
}

/// Apply synthetic styling to a text object without touching its string.
///
/// Bold is PDFium's filled-and-stroked render mode with a hair of stroke width;
/// colour overrides the fill (and, when bold, the stroke too). Neither touches
/// the object's matrix, so neither can move the run. Size is set by the caller
/// beforehand.
fn apply_format_inline(object: &mut PdfPageObject<'_>, format: &TextFormat, base_font_size: f32) {
    if let Some(text_object) = object.as_text_object_mut() {
        let mode = if format.bold {
            PdfPageTextRenderMode::FilledThenStroked
        } else {
            PdfPageTextRenderMode::FilledUnstroked
        };
        let _ = text_object.set_render_mode(mode);
    }

    if format.bold {
        // A small stroke makes the fake bold read as heavier without obliterating
        // the glyphs. Scaled to the font so it looks the same at any size; a
        // size override travels through `format.size`.
        let width = (format.size.unwrap_or(base_font_size) * 0.04).max(0.25);
        let _ = object.set_stroke_width(PdfPoints::new(width));
    } else {
        // Turning bold off must drop the stroke. Leaving the width in place
        // keeps the glyphs looking heavy after undo or Reset style.
        let _ = object.set_stroke_width(PdfPoints::ZERO);
    }

    if let Some([r, g, b, a]) = format.color {
        let color = PdfColor::new(r, g, b, a);
        let _ = object.set_fill_color(color);
        if format.bold {
            let _ = object.set_stroke_color(color);
        }
    }
}

/// Restyle a text object in place, keeping its current string.
///
/// Used by the formatting panel (bold / size / colour) and by undo, which
/// restores the previous format. Unlike [`set_text`] there is **no fit pass**:
/// the string is unchanged, and a size the user picked explicitly is applied
/// exactly as asked — shrinking it back into the run's original ink area made
/// every size *increase* a silent no-op, which read as "font size does not
/// change". Growing into a neighbour is the user's call, like any editor.
///
/// `format.size` of `None` leaves the size exactly as it is (the undo of a
/// size change back to an unstyled object restores nothing, because the
/// original size is already the object's own). `format.color` of `None` leaves
/// the ink colour untouched for the same reason — PDFium can neither report
/// nor clear a run's original colour.
pub(crate) fn apply_format(
    document: &mut PdfDocument<'static>,
    id: PageObjectId,
    _layout_bounds: Rect,
    base_font_size: f32,
    _previous_format: TextFormat,
    format: TextFormat,
) -> Result<bool> {
    let index = id.index as PdfPageObjectIndex;

    let mut page = page_of(document, id.page)?;
    {
        let objects = page.objects_mut();
        if index >= objects.len() {
            return Ok(false);
        }
        let mut object = objects.get(index).map_err(pdfium_error)?;
        if object.as_text_object().is_none() {
            // Colour and bold only make sense on text; refuse quietly rather
            // than restyling a path or an image.
            return Ok(false);
        }
        if let Some(size) = format.size.filter(|s| s.is_finite() && *s > 0.0) {
            object
                .as_text_object_mut()
                .ok_or_else(|| EngineError::Unsupported("Text object disappeared".into()))?
                .set_unscaled_font_size(PdfPoints::new(size))
                .map_err(pdfium_error)?;
        }
        apply_format_inline(&mut object, &format, base_font_size);
    }
    page.regenerate_content().map_err(pdfium_error)?;
    Ok(true)
}

/// Swap an image object's pixels for a picture on disk.
///
/// The object keeps its place on the page: only what it shows changes, so a
/// replaced logo stays the size and position the layout gave it.
pub(crate) fn replace_image(
    document: &mut PdfDocument<'static>,
    id: PageObjectId,
    path: &Path,
) -> Result<bool> {
    let image = image::open(path).map_err(|error| EngineError::Source {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;

    let mut page = page_of(document, id.page)?;
    let index = id.index as PdfPageObjectIndex;

    {
        let objects = page.objects_mut();
        if index >= objects.len() {
            return Ok(false);
        }
        let mut object = objects.get(index).map_err(pdfium_error)?;
        let Some(image_object) = object.as_image_object_mut() else {
            return Ok(false);
        };
        image_object.set_image(&image).map_err(pdfium_error)?;
    }

    page.regenerate_content().map_err(pdfium_error)?;
    Ok(true)
}

/// Add a new text object, returning where it landed.
///
/// `x` and `y` are the text's baseline-left corner. Helvetica is used because it
/// is one of the standard fourteen: it is always available, needs nothing
/// embedded, and is what every reader can render without a font file.
pub(crate) fn insert_text(
    document: &mut PdfDocument<'static>,
    page: u32,
    x: f32,
    y: f32,
    text: &str,
    font_size: f32,
) -> Result<PageObjectId> {
    // The font token is `Copy` and the fonts borrow ends with this statement,
    // leaving the document free to be borrowed again for the page.
    let font = document.fonts_mut().helvetica();

    let mut page_handle = page_of(document, page)?;
    let index;
    {
        let objects = page_handle.objects_mut();
        // `create_text_object` appends, so the object's index is the length
        // before the call. Capture it first: afterwards there is no way to ask.
        index = objects.len();
        objects
            .create_text_object(
                PdfPoints::new(x),
                PdfPoints::new(y),
                text,
                font,
                PdfPoints::new(font_size),
            )
            .map_err(pdfium_error)?;
    }

    page_handle.regenerate_content().map_err(pdfium_error)?;

    Ok(PageObjectId {
        page,
        index: u32::try_from(index).unwrap_or(u32::MAX),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pdfium_rectangle_becomes_a_core_rectangle() {
        let rect = rect_of(PdfRect::new(
            PdfPoints::new(10.0),
            PdfPoints::new(20.0),
            PdfPoints::new(40.0),
            PdfPoints::new(70.0),
        ));
        assert_eq!((rect.min_x, rect.min_y), (20.0, 10.0));
        assert_eq!((rect.max_x, rect.max_y), (70.0, 40.0));
        assert_eq!((rect.width(), rect.height()), (50.0, 30.0));
    }

    /// Only the two kinds the editor can change are reported; everything else on
    /// a page is skipped rather than offered and then refused.
    #[test]
    fn only_text_and_images_are_editable() {
        assert_eq!(
            editable_kind(PdfPageObjectType::Text),
            Some(PageObjectKind::Text)
        );
        assert_eq!(
            editable_kind(PdfPageObjectType::Image),
            Some(PageObjectKind::Image)
        );
        for other in [
            PdfPageObjectType::Path,
            PdfPageObjectType::Shading,
            PdfPageObjectType::XObjectForm,
            PdfPageObjectType::Unsupported,
        ] {
            assert_eq!(editable_kind(other), None, "{other:?} must be skipped");
        }
    }
}
