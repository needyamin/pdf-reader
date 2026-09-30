//! The engine trait and its PDFium implementation.
//!
//! # Why there is exactly one of these
//!
//! PDFium is not thread-safe. `pdfium-render` makes it *safe* to call from
//! several threads by wrapping every FPDF call in a process-global mutex, which
//! serialises them. That prevents crashes but delivers no parallelism, so this
//! engine is deliberately designed to be owned by a single thread that works
//! through a priority queue. Nothing here should ever be called concurrently.

use std::collections::HashMap;
use std::path::Path;

use pdfium_render::prelude::*;
use pdfreader_core::{
    AnnotationId, AnnotationInfo, FormInfo, NewAnnotation, Outline, OutlineNode, PageGeometry,
    Rotation,
};

use crate::annot;
use crate::error::EngineError;
use crate::form::read_form;
use crate::text::{CharBox, TextPage};
use crate::{DocumentInfo, Result};

/// Opaque handle to a document held open by an engine.
///
/// Distinct from `core::DocumentId`: the engine assigns handles, the store
/// assigns domain ids, and the shell maps between them.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct DocumentHandle(u64);

impl DocumentHandle {
    /// Raw handle value, for logging.
    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// One tile to rasterize.
///
/// Pixel values are in the *rotated* output image's coordinate space with a
/// top-left origin, which is the space the render layer already works in. The
/// engine converts to PDF's bottom-left space internally so the flip happens in
/// exactly one place.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TileRequest {
    /// Zero-based page index.
    pub page: u32,
    /// Page rotation to bake into the render.
    pub rotation: Rotation,
    /// Device pixels per PDF point.
    pub scale: f32,
    /// Left edge of the tile within the full-page image, in pixels.
    pub origin_x: i32,
    /// Top edge of the tile within the full-page image, in pixels.
    pub origin_y: i32,
    /// Width of the tile bitmap.
    pub width: u32,
    /// Height of the tile bitmap.
    pub height: u32,
}

/// Rasterized tile pixels.
///
/// Byte order is BGRA, matching PDFium's native output and `wgpu`'s
/// `Bgra8Unorm`, so tiles upload without a swizzle.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TilePixels {
    /// Tile width in pixels.
    pub width: u32,
    /// Tile height in pixels.
    pub height: u32,
    /// Row-major BGRA bytes, `width * height * 4` long.
    pub data: Vec<u8>,
}

/// What the rest of the application is allowed to ask of a PDF engine.
pub trait PdfEngine {
    /// Open a file, returning a handle used for every subsequent call.
    fn open(
        &mut self,
        path: &Path,
        passphrase: Option<&str>,
    ) -> Result<(DocumentHandle, DocumentInfo)>;

    /// Release a document and its resources.
    fn close(&mut self, handle: DocumentHandle);

    /// Rasterize one tile of one page.
    fn render_tile(&mut self, handle: DocumentHandle, request: &TileRequest) -> Result<TilePixels>;

    /// Rasterize a whole page, used for page previews and export.
    fn render_page(
        &mut self,
        handle: DocumentHandle,
        page: u32,
        rotation: Rotation,
        scale: f32,
    ) -> Result<TilePixels>;

    /// Extract the text geometry of a page.
    fn page_text(&mut self, handle: DocumentHandle, page: u32) -> Result<TextPage>;

    /// Read the document's interactive form, if it has one.
    ///
    /// Returns an empty [`FormInfo`] for documents with no AcroForm rather than
    /// an error: "no form" is a normal state, not a failure.
    fn form_fields(&mut self, handle: DocumentHandle) -> Result<FormInfo>;

    /// Write a value into one form field.
    ///
    /// `id` is the `(page, annotation index)` pair reported by
    /// [`Self::form_fields`]. Returns `Ok(true)` when the value was written,
    /// `Ok(false)` when the widget is missing or read-only, and an error when
    /// the widget type cannot be written at all.
    fn set_field_value(
        &mut self,
        handle: DocumentHandle,
        id: pdfreader_core::FieldId,
        value: pdfreader_core::FieldValue,
    ) -> Result<bool>;

    /// Serialise the document, optionally flattening annotations and form
    /// values into page content first.
    ///
    /// Returns the raw PDF bytes. Writing them back to disk is the caller's
    /// job, so this method never touches the file the document was opened from
    /// (PDFium cannot save over a file it holds open).
    fn save_to_bytes(&mut self, handle: DocumentHandle, flatten: bool) -> Result<Vec<u8>>;

    /// List every non-widget annotation in the document, in page order.
    fn annotations(&mut self, handle: DocumentHandle) -> Result<Vec<AnnotationInfo>>;

    /// Create an annotation described by `new` on `page`.
    fn add_annotation(
        &mut self,
        handle: DocumentHandle,
        page: u32,
        new: NewAnnotation,
    ) -> Result<()>;

    /// Remove one annotation.
    ///
    /// Returns `Ok(false)` when the id no longer resolves, which is not an
    /// error: the document may have been edited since the list was read.
    fn delete_annotation(&mut self, handle: DocumentHandle, id: AnnotationId) -> Result<bool>;

    /// Replace the text contents of one annotation.
    fn set_annotation_contents(
        &mut self,
        handle: DocumentHandle,
        id: AnnotationId,
        contents: &str,
    ) -> Result<()>;
}

/// PDFium-backed engine.
///
/// The `Pdfium` instance is deliberately leaked to a `'static` reference.
/// PDFium is a process-global singleton in practice — the mutex guarding it is
/// process-global — so giving it a process lifetime is honest rather than a
/// workaround. It also avoids a self-referential struct, since `PdfDocument`
/// borrows the `Pdfium` it was loaded from.
pub struct PdfiumEngine {
    pdfium: &'static Pdfium,
    documents: HashMap<DocumentHandle, PdfDocument<'static>>,
    next_handle: u64,
    /// Reusable tile bitmap, so steady-state rendering does not allocate.
    scratch: Option<ScratchBitmap>,
}

/// A bitmap kept between calls to avoid per-tile allocation.
struct ScratchBitmap {
    width: u32,
    height: u32,
    bitmap: PdfBitmap<'static>,
}

impl PdfiumEngine {
    /// Bind to a PDFium library, preferring one in the current directory.
    ///
    /// # Errors
    /// Returns [`EngineError::Library`] if no PDFium library can be loaded.
    pub fn new() -> Result<Self> {
        Self::bind(None)
    }

    /// Bind to a PDFium library, looking first in `dir` when given.
    ///
    /// # Errors
    /// Returns [`EngineError::Library`] if no PDFium library can be loaded.
    pub fn bind(dir: Option<&Path>) -> Result<Self> {
        let search_dir = dir.unwrap_or(Path::new("."));

        let bindings =
            Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(search_dir))
                .or_else(|_| Pdfium::bind_to_system_library())
                .map_err(|e| EngineError::Library(format!("{e:?}")))?;

        let pdfium: &'static Pdfium = Box::leak(Box::new(Pdfium::new(bindings)));

        Ok(Self {
            pdfium,
            documents: HashMap::new(),
            next_handle: 0,
            scratch: None,
        })
    }

    /// Borrow a document, mapping "not open" onto an error.
    fn document(&self, handle: DocumentHandle) -> Result<&PdfDocument<'static>> {
        self.documents
            .get(&handle)
            .ok_or(EngineError::NotOpen(handle.raw()))
    }

    /// Convert a core rotation into PDFium's render rotation.
    const fn pdfium_rotation(rotation: Rotation) -> PdfPageRenderRotation {
        match rotation {
            Rotation::None => PdfPageRenderRotation::None,
            Rotation::Cw90 => PdfPageRenderRotation::Degrees90,
            Rotation::Cw180 => PdfPageRenderRotation::Degrees180,
            Rotation::Cw270 => PdfPageRenderRotation::Degrees270,
        }
    }

    /// Build the render config shared by tile and full-page rendering.
    fn config(rotation: Rotation, scale: f32) -> PdfRenderConfig {
        PdfRenderConfig::new()
            .scale_page_by_factor(scale)
            .rotate(Self::pdfium_rotation(rotation), true)
            .set_format(PdfBitmapFormat::BGRA)
            .render_annotations(true)
            .limit_render_image_cache_size(true)
            // Tiles at the right and bottom edges of a page are only partly
            // covered by page content. Without clearing, the uncovered margin
            // keeps whatever the previous tile left behind, which shows up as
            // smeared garbage along page edges.
            .clear_before_rendering(true)
            .set_clear_color(PdfColor::WHITE)
    }
}

impl PdfEngine for PdfiumEngine {
    fn open(
        &mut self,
        path: &Path,
        passphrase: Option<&str>,
    ) -> Result<(DocumentHandle, DocumentInfo)> {
        let document = match self.pdfium.load_pdf_from_file(path, passphrase) {
            Ok(document) => document,
            // PDFium reports both "no password supplied" and "wrong password"
            // through the same internal code, so disambiguate using whether we
            // already tried one.
            Err(PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError)) => {
                return Err(if passphrase.is_some() {
                    EngineError::BadPassword
                } else {
                    EngineError::PasswordRequired
                });
            }
            // Report failures in terms a user can act on, not PDFium internals.
            Err(PdfiumError::IoError(inner)) => {
                return Err(EngineError::Io {
                    path: path.to_path_buf(),
                    reason: inner.to_string(),
                });
            }
            Err(PdfiumError::PdfiumLibraryInternalError(inner)) => {
                let reason = match inner {
                    PdfiumInternalError::FileError => "the file could not be read",
                    PdfiumInternalError::FormatError => "the file is damaged or is not a PDF",
                    PdfiumInternalError::SecurityError => {
                        "the file uses an unsupported security scheme"
                    }
                    _ => "the file could not be opened",
                };
                return Err(EngineError::Malformed(reason.to_string()));
            }
            Err(_) => {
                return Err(EngineError::Malformed(
                    "the file could not be opened".to_string(),
                ));
            }
        };

        // `Unprotected` is the revision reported for a document with no security
        // handler; any other revision means the file is encrypted. Revisions the
        // binding does not know (e.g. AES-256 R5/R6) surface as `Err`, so treat
        // every non-`Unprotected` result as encrypted rather than the reverse.
        let encrypted = !matches!(
            document.permissions().security_handler_revision(),
            Ok(PdfSecurityHandlerRevision::Unprotected)
        );

        let mut pages = Vec::new();
        for page in document.pages().iter() {
            pages.push(PageGeometry {
                width_pt: page.width().value,
                height_pt: page.height().value,
            });
        }

        let info = DocumentInfo {
            pages,
            outline: Outline {
                root: collect_outline(&document),
            },
            encrypted,
        };

        self.next_handle += 1;
        let handle = DocumentHandle(self.next_handle);
        self.documents.insert(handle, document);

        Ok((handle, info))
    }

    fn close(&mut self, handle: DocumentHandle) {
        self.documents.remove(&handle);
    }

    fn form_fields(&mut self, handle: DocumentHandle) -> Result<FormInfo> {
        let document = self
            .documents
            .get(&handle)
            .ok_or(EngineError::NotOpen(handle.raw()))?;
        Ok(read_form(document))
    }

    fn set_field_value(
        &mut self,
        handle: DocumentHandle,
        id: pdfreader_core::FieldId,
        value: pdfreader_core::FieldValue,
    ) -> Result<bool> {
        let Self { documents, .. } = self;
        let document = documents
            .get(&handle)
            .ok_or(EngineError::NotOpen(handle.raw()))?;

        let mut page = page_of(document, id.page)?;
        // A stale or bogus widget id means "nothing to write", not a failure:
        // the form was re-read or the document changed between the request and
        // the write, and neither is exceptional.
        let Ok(mut annotation) = page
            .annotations_mut()
            .get(id.annot_index as PdfPageAnnotationIndex)
        else {
            return Ok(false);
        };

        let Some(field) = annotation.as_form_field_mut() else {
            return Ok(false);
        };
        if field.is_read_only() {
            return Ok(false);
        }

        let outcome = match value {
            pdfreader_core::FieldValue::Text(text) => field
                .as_text_field_mut()
                .ok_or_else(|| EngineError::Unsupported("editing this widget".to_string()))
                .and_then(|text_field| {
                    text_field
                        .set_value(&text)
                        .map_err(|error| EngineError::Pdfium(error.to_string()))
                }),
            pdfreader_core::FieldValue::Checked(on) => {
                // Selecting a radio button means checking that widget; PDFium's
                // radio group deselects its siblings. Unchecking one outright
                // has no setter, so radios are toggle-on only.
                if let Some(checkbox) = field.as_checkbox_field_mut() {
                    checkbox
                        .set_checked(on)
                        .map_err(|error| EngineError::Pdfium(error.to_string()))
                } else if let Some(radio) = field.as_radio_button_field_mut() {
                    if on {
                        radio
                            .set_checked()
                            .map_err(|error| EngineError::Pdfium(error.to_string()))
                    } else {
                        Err(EngineError::Unsupported(
                            "clearing a radio button".to_string(),
                        ))
                    }
                } else {
                    Err(EngineError::Unsupported("editing this widget".to_string()))
                }
            }
            // pdfium-render exposes combo and list box fields read-only, so a
            // choice cannot be written back through the safe binding.
            pdfreader_core::FieldValue::Choice(_) => Err(EngineError::Unsupported(
                "changing a dropdown or list selection".to_string(),
            )),
            pdfreader_core::FieldValue::Empty => Err(EngineError::Unsupported(
                "clearing this widget".to_string(),
            )),
        };
        outcome.map(|()| true)
    }

    fn save_to_bytes(&mut self, handle: DocumentHandle, flatten: bool) -> Result<Vec<u8>> {
        let document = self
            .documents
            .get_mut(&handle)
            .ok_or(EngineError::NotOpen(handle.raw()))?;

        if flatten {
            let page_count = document.pages().len();
            let pages = document.pages_mut();
            for index in 0..page_count {
                if let Ok(mut page) = pages.get(index) {
                    page.flatten()
                        .map_err(|error| EngineError::Pdfium(error.to_string()))?;
                }
            }
        }

        document
            .save_to_bytes()
            .map_err(|error| EngineError::Pdfium(error.to_string()))
    }

    fn annotations(&mut self, handle: DocumentHandle) -> Result<Vec<AnnotationInfo>> {
        let document = self
            .documents
            .get(&handle)
            .ok_or(EngineError::NotOpen(handle.raw()))?;
        Ok(annot::list_annotations(document))
    }

    fn add_annotation(
        &mut self,
        handle: DocumentHandle,
        page: u32,
        new: NewAnnotation,
    ) -> Result<()> {
        let document = self
            .documents
            .get_mut(&handle)
            .ok_or(EngineError::NotOpen(handle.raw()))?;
        annot::create_annotation(document, page, &new)
            .map_err(|error| EngineError::Pdfium(error.to_string()))
    }

    fn delete_annotation(&mut self, handle: DocumentHandle, id: AnnotationId) -> Result<bool> {
        let document = self
            .documents
            .get_mut(&handle)
            .ok_or(EngineError::NotOpen(handle.raw()))?;
        annot::remove_annotation(document, id)
            .map_err(|error| EngineError::Pdfium(error.to_string()))
    }

    fn set_annotation_contents(
        &mut self,
        handle: DocumentHandle,
        id: AnnotationId,
        contents: &str,
    ) -> Result<()> {
        let document = self
            .documents
            .get_mut(&handle)
            .ok_or(EngineError::NotOpen(handle.raw()))?;
        annot::change_contents(document, id, contents)
            .map_err(|error| EngineError::Pdfium(error.to_string()))
    }

    fn render_tile(&mut self, handle: DocumentHandle, request: &TileRequest) -> Result<TilePixels> {
        // Destructure so the document map and the scratch bitmap are borrowed
        // as disjoint fields. Borrowing both through `self` in one call is not
        // something the borrow checker will allow, because `PdfPage` keeps the
        // immutable borrow of `self.documents` alive for the whole render.
        let Self {
            documents, scratch, ..
        } = self;

        let document = documents
            .get(&handle)
            .ok_or(EngineError::NotOpen(handle.raw()))?;

        let page = page_of(document, request.page)?;

        let needs_new = scratch
            .as_ref()
            .is_none_or(|s| s.width != request.width || s.height != request.height);

        if needs_new {
            let bitmap = PdfBitmap::empty(
                request.width as Pixels,
                request.height as Pixels,
                PdfBitmapFormat::BGRA,
            )
            .map_err(|e| EngineError::Pdfium(format!("{e:?}")))?;

            *scratch = Some(ScratchBitmap {
                width: request.width,
                height: request.height,
                bitmap,
            });
        }

        let Some(scratch) = scratch.as_mut() else {
            return Err(EngineError::Pdfium(
                "could not allocate the tile bitmap".to_string(),
            ));
        };
        let bitmap = &mut scratch.bitmap;

        // Tiling uses `set_origin`, not `clip`. PDFium switches to a different
        // render path when a clip rectangle is set, and that path ignores the
        // origin entirely — every tile would show the same region. Offsetting
        // the page inside a tile-sized bitmap gets the clipping from the
        // bitmap's own bounds and leaves form rendering intact.
        let config = Self::config(request.rotation, request.scale)
            .set_origin(-request.origin_x, -request.origin_y);

        page.render_into_bitmap_with_config(bitmap, &config)
            .map_err(|e| EngineError::Pdfium(format!("{e:?}")))?;

        Ok(TilePixels {
            width: request.width,
            height: request.height,
            data: bitmap.as_raw_bytes(),
        })
    }

    fn render_page(
        &mut self,
        handle: DocumentHandle,
        page_index: u32,
        rotation: Rotation,
        scale: f32,
    ) -> Result<TilePixels> {
        let document = self.document(handle)?;

        let page = page_of(document, page_index)?;

        let config = Self::config(rotation, scale);
        let bitmap = page
            .render_with_config(&config)
            .map_err(|e| EngineError::Pdfium(format!("{e:?}")))?;

        // Trust the size PDFium actually produced rather than the size we
        // asked for; rounding of the page scale can shift it by a pixel.
        Ok(TilePixels {
            width: bitmap.width() as u32,
            height: bitmap.height() as u32,
            data: bitmap.as_raw_bytes(),
        })
    }

    fn page_text(&mut self, handle: DocumentHandle, page_index: u32) -> Result<TextPage> {
        let document = self.document(handle)?;

        let page = page_of(document, page_index)?;

        let text = page
            .text()
            .map_err(|e| EngineError::Pdfium(format!("{e:?}")))?;

        let mut chars = Vec::new();
        for character in text.chars().iter() {
            let Some(ch) = character.unicode_char() else {
                continue;
            };
            // `loose_bounds` is the line-height box, which is what selection
            // and highlight rectangles should use; `tight_bounds` hugs the
            // glyph outline and makes highlights look ragged.
            let Ok(bounds) = character.loose_bounds() else {
                continue;
            };
            chars.push(CharBox {
                ch,
                rect: crate::geometry::Rect::from_xywh(
                    bounds.left().value,
                    bounds.bottom().value,
                    bounds.width().value,
                    bounds.height().value,
                ),
                line: 0,
                word: 0,
            });
        }

        Ok(TextPage {
            chars,
            words: Vec::new(),
            lines: Vec::new(),
        })
    }
}

/// Fetch a page, turning PDFium's opaque failure into a range error.
fn page_of<'a>(document: &'a PdfDocument<'static>, index: u32) -> Result<PdfPage<'a>> {
    let count = document.pages().len() as u32;

    document
        .pages()
        .get(index as PdfPageIndex)
        .map_err(|_| EngineError::PageOutOfRange { index, count })
}

/// Deepest outline level ingested from a document. PDF outlines are
/// attacker-controlled: a crafted file can nest bookmarks deep enough to
/// overflow the stack during conversion, counting and rendering, so recursion
/// is capped well above anything a real document produces.
const MAX_OUTLINE_DEPTH: usize = 32;

/// Walk the PDFium bookmark tree into the domain outline.
fn collect_outline(document: &PdfDocument) -> Vec<OutlineNode> {
    document
        .bookmarks()
        .iter()
        .map(|bookmark| convert_bookmark(&bookmark, 0))
        .collect()
}

/// Convert one bookmark and its children, up to [`MAX_OUTLINE_DEPTH`] levels.
fn convert_bookmark(bookmark: &PdfBookmark, depth: usize) -> OutlineNode {
    let page = bookmark
        .destination()
        .and_then(|destination| destination.page_index().ok())
        .map(|index| index as u32);

    let children = if depth < MAX_OUTLINE_DEPTH {
        bookmark
            .iter_direct_children()
            .map(|child| convert_bookmark(&child, depth + 1))
            .collect()
    } else {
        Vec::new()
    };

    OutlineNode {
        title: bookmark.title().unwrap_or_default(),
        page,
        children,
    }
}
