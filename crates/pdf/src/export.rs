//! Exporting pages as images, and building new PDFs out of existing material.
//!
//! Three kinds of work live here — rasterizing pages to image files, composing
//! a PDF out of pages or images, and concatenating documents — and they share
//! one shape: each is long compared to a render, each writes files, and the
//! user has to be able to stop it. Every entry point therefore takes a
//! [`JobProgress`] handle carrying both the progress callback and the
//! cancellation flag.
//!
//! Nothing here touches the UI. The engine owns the only `Pdfium` instance, so
//! the composition helpers take it as an argument rather than reaching for a
//! process-global.

use std::fs::File;
use std::io::{BufWriter, Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use image::codecs::jpeg::JpegEncoder;
use pdfium_render::prelude::*;
use pdfreader_core::{ImageFormat, PageRange};

use crate::Result;
use crate::engine::TilePixels;
use crate::error::EngineError;

/// Largest rasterized page, in megapixels, that an export will produce.
///
/// One page becomes one contiguous bitmap and the render scale is a
/// user-facing zoom level, so without a ceiling a stray scale turns a page into
/// a multi-gigabyte allocation inside FFI — which aborts the process instead of
/// returning an error. A hundred megapixels is comfortably above any real page
/// at print resolution (A4 at 600 dpi is about 35 MP).
pub const MAX_EXPORT_MEGAPIXELS: f64 = 100.0;

/// JPEG quality for exported pages.
///
/// Text recompresses badly: below about 80 the ringing around glyph edges
/// becomes visible when zoomed in. At 90 the file is still several times
/// smaller than the equivalent PNG.
const JPEG_QUALITY: u8 = 90;

/// Progress and cancellation for one long-running job.
///
/// The reporting callback is a trait object rather than a generic parameter so
/// that the engine's trait methods stay object-safe and this type does not
/// infect every signature above it.
pub struct JobProgress<'a> {
    /// Set by the caller when the user asks the job to stop.
    cancel: &'a AtomicBool,
    /// Called with `(done, total)` after each item.
    report: &'a mut dyn FnMut(u32, u32),
}

impl<'a> JobProgress<'a> {
    /// Wrap a cancellation flag and a progress callback.
    pub fn new(cancel: &'a AtomicBool, report: &'a mut dyn FnMut(u32, u32)) -> Self {
        Self { cancel, report }
    }

    /// Whether the caller has asked the job to stop.
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Report that `done` of `total` items are finished.
    pub fn step(&mut self, done: u32, total: u32) {
        (self.report)(done, total);
    }

    /// Stop the job when the caller has asked for it.
    ///
    /// Called at the top of every item, so a cancelled job stops *between*
    /// pages rather than in the middle of one: the engine thread is shared with
    /// tile rendering, and abandoning a half-written file would be worse than
    /// finishing the page already in flight.
    pub fn check(&self) -> Result<()> {
        if self.cancelled() {
            Err(EngineError::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// Refuse a render that would need an unreasonable amount of memory.
///
/// Both inputs are untrusted: the scale is a zoom level the user chose, and the
/// page size comes from the document. Checking before the render is the only
/// point at which the mistake is still recoverable.
pub(crate) fn check_render_budget(width_pt: f32, height_pt: f32, scale: f32) -> Result<()> {
    let scale = f64::from(scale);
    let megapixels = f64::from(width_pt) * f64::from(height_pt) * scale * scale / 1_000_000.0;

    if megapixels > MAX_EXPORT_MEGAPIXELS {
        return Err(EngineError::Export(format!(
            "a {width_pt:.0}x{height_pt:.0} point page at {scale:.2}x needs {megapixels:.0} megapixels, \
             and the limit is {MAX_EXPORT_MEGAPIXELS:.0}"
        )));
    }
    Ok(())
}

/// File name for page `index` (zero-based) of a document exported as images.
///
/// Pages are numbered from one and padded to four digits so that a directory
/// listing sorts the same way the document reads.
pub(crate) fn page_file_name(stem: &str, index: u32, format: ImageFormat) -> String {
    format!("{stem}-{:04}.{}", index + 1, format.extension())
}

/// Rasterize one page and write it out as an image file.
pub(crate) fn write_page_image(tile: &TilePixels, format: ImageFormat, path: &Path) -> Result<()> {
    let bytes = encode_image(tile, format)?;
    write_file(path, &bytes)
}

/// Convert a rendered page into RGBA and encode it as PNG or JPEG.
fn encode_image(tile: &TilePixels, format: ImageFormat) -> Result<Vec<u8>> {
    let mut pixels = tile.data.clone();
    to_opaque_rgba(&mut pixels);

    let (width, height) = (tile.width, tile.height);
    let buffer = image::RgbaImage::from_raw(width, height, pixels).ok_or_else(|| {
        EngineError::Export(format!(
            "the rendered page is not {width}x{height} pixels of RGBA data"
        ))
    })?;
    let dynamic = image::DynamicImage::ImageRgba8(buffer);

    let mut out = Cursor::new(Vec::new());
    match format {
        ImageFormat::Png => dynamic
            .write_to(&mut out, image::ImageFormat::Png)
            .map_err(|error| EngineError::Export(error.to_string()))?,
        ImageFormat::Jpeg => {
            let encoder = JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY);
            dynamic
                .write_with_encoder(encoder)
                .map_err(|error| EngineError::Export(error.to_string()))?;
        }
    }

    Ok(out.into_inner())
}

/// Turn `PDFium`'s BGRA page into opaque RGBA, in place.
///
/// The render path keeps pixels in BGRA so tiles upload to the GPU without a
/// swizzle; every image encoder wants RGBA. Alpha is forced opaque because a
/// rendered page is paper, not a sprite: `PDFium` leaves the channel unset in
/// some builds, and a PNG of a blank page should not come out fully
/// transparent.
fn to_opaque_rgba(bytes: &mut [u8]) {
    for pixel in bytes.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
        pixel[3] = 255;
    }
}

/// Serialise `range` of `source` as a standalone PDF.
///
/// `None` means the whole document, which is a straight copy rather than a
/// rebuild — cheaper, and it preserves everything a rebuild would drop.
pub(crate) fn export_range(
    pdfium: &Pdfium,
    source: &PdfDocument<'static>,
    range: Option<PageRange>,
) -> Result<Vec<u8>> {
    let Some(range) = range else {
        return source.save_to_bytes().map_err(pdfium_error);
    };

    let count = u32::try_from(source.pages().len()).unwrap_or(0);
    let Ok(first) = PdfPageIndex::try_from(range.first) else {
        return Err(EngineError::PageOutOfRange {
            index: range.first,
            count,
        });
    };
    let Ok(last) = PdfPageIndex::try_from(range.last) else {
        return Err(EngineError::PageOutOfRange {
            index: range.last,
            count,
        });
    };

    let mut out = pdfium.create_new_pdf().map_err(pdfium_error)?;
    out.pages_mut()
        .copy_page_range_from_document(source, first..=last, 0)
        .map_err(pdfium_error)?;
    out.save_to_bytes().map_err(pdfium_error)
}

/// Concatenate `sources` into `output`, in the order given.
///
/// Returns the number of pages written. Each source is opened, appended and
/// dropped before the next is touched: holding every input open at once would
/// multiply peak memory by the number of files for no benefit.
pub(crate) fn merge_pdfs(
    pdfium: &Pdfium,
    sources: &[PathBuf],
    output: &Path,
    job: &mut JobProgress<'_>,
) -> Result<u32> {
    let mut out = pdfium.create_new_pdf().map_err(pdfium_error)?;
    let total = u32::try_from(sources.len()).unwrap_or(u32::MAX);
    let mut pages = 0u32;

    for (index, path) in sources.iter().enumerate() {
        job.check()?;

        let source = open_source(pdfium, path)?;
        out.pages_mut().append(&source).map_err(pdfium_error)?;
        pages += u32::try_from(source.pages().len()).unwrap_or(0);

        job.step(u32::try_from(index).unwrap_or(0) + 1, total);
    }

    save_to_file(&out, output)?;
    Ok(pages)
}

/// Build a PDF with one page per image, each page sized to its image.
///
/// `scale` is points per pixel. The page is sized with `new_custom` rather than
/// `from_points`, because the latter snaps to the nearest standard paper size
/// and "the page is the image" has to be exact.
pub(crate) fn images_to_pdf(
    pdfium: &Pdfium,
    images: &[PathBuf],
    output: &Path,
    scale: f32,
    job: &mut JobProgress<'_>,
) -> Result<u32> {
    let mut out = pdfium.create_new_pdf().map_err(pdfium_error)?;
    let total = u32::try_from(images.len()).unwrap_or(u32::MAX);

    for (index, path) in images.iter().enumerate() {
        job.check()?;

        let image = image::open(path).map_err(|error| EngineError::Source {
            path: path.clone(),
            reason: error.to_string(),
        })?;

        let width = points(image.width(), scale);
        let height = points(image.height(), scale);

        // The page handle is a handle plus a `PhantomData`, so it does not
        // borrow the document: the mutable borrow taken by `pages_mut` ends
        // with this statement, leaving `out` free to be serialised below.
        let mut page = out
            .pages_mut()
            .create_page_at_end(PdfPagePaperSize::new_custom(width, height))
            .map_err(pdfium_error)?;

        page.objects_mut()
            .create_image_object(
                PdfPoints::ZERO,
                PdfPoints::ZERO,
                &image,
                Some(width),
                Some(height),
            )
            .map_err(pdfium_error)?;

        job.step(u32::try_from(index).unwrap_or(0) + 1, total);
    }

    save_to_file(&out, output)?;
    Ok(total)
}

/// Open one input of a multi-file job, naming the file in any failure.
fn open_source<'a>(pdfium: &'a Pdfium, path: &Path) -> Result<PdfDocument<'a>> {
    pdfium.load_pdf_from_file(path, None).map_err(|error| {
        let reason = match map_load_error(path, error, false) {
            EngineError::PasswordRequired | EngineError::BadPassword => {
                "the document is encrypted".to_string()
            }
            EngineError::Malformed(reason) | EngineError::Io { reason, .. } => reason,
            other => other.to_string(),
        };
        EngineError::Source {
            path: path.to_path_buf(),
            reason,
        }
    })
}

/// Serialise a document straight into `output`.
///
/// Streams through a buffered writer instead of building the file in memory:
/// a merged document of several hundred pages is large, and `save_to_bytes`
/// would hold a second copy of all of it.
fn save_to_file(document: &PdfDocument<'_>, output: &Path) -> Result<()> {
    let file = File::create(output).map_err(|error| EngineError::Io {
        path: output.to_path_buf(),
        reason: error.to_string(),
    })?;
    let mut writer = BufWriter::new(file);

    document.save_to_writer(&mut writer).map_err(pdfium_error)?;

    writer.flush().map_err(|error| EngineError::Io {
        path: output.to_path_buf(),
        reason: error.to_string(),
    })
}

/// Write bytes to `path`, reporting an IO failure against the path that failed.
fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::write(path, bytes).map_err(|error| EngineError::Io {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })
}

/// Map a `PDFium` failure onto the engine's error type.
///
/// Takes the error by value so it can be handed straight to `map_err`, which
/// is the only way this is ever used.
#[allow(clippy::needless_pass_by_value)]
fn pdfium_error(error: PdfiumError) -> EngineError {
    EngineError::Pdfium(error.to_string())
}

/// An image dimension in PDF points, at `scale` points per pixel.
///
/// `u32` to `f32` is lossy in principle, but PDF points are `f32` throughout
/// and an image wider than 2^24 pixels cannot be decoded in the first place.
#[allow(clippy::cast_precision_loss)]
fn points(pixels: u32, scale: f32) -> PdfPoints {
    PdfPoints::new(pixels as f32 * scale)
}

/// Map a document-load failure onto a message the user can act on.
///
/// Shared by opening a document in the UI and by every multi-file job, so a
/// file that fails to load says the same thing wherever it is used.
pub(crate) fn map_load_error(path: &Path, error: PdfiumError, had_passphrase: bool) -> EngineError {
    match error {
        // PDFium reports "no password supplied" and "wrong password" through
        // the same internal code, so disambiguate using whether we already
        // tried one.
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => {
            if had_passphrase {
                EngineError::BadPassword
            } else {
                EngineError::PasswordRequired
            }
        }
        PdfiumError::IoError(inner) => EngineError::Io {
            path: path.to_path_buf(),
            reason: inner.to_string(),
        },
        PdfiumError::PdfiumLibraryInternalError(inner) => {
            let reason = match inner {
                PdfiumInternalError::FileError => "the file could not be read",
                PdfiumInternalError::FormatError => "the file is damaged or is not a PDF",
                PdfiumInternalError::SecurityError => {
                    "the file uses an unsupported security scheme"
                }
                _ => "the file could not be opened",
            };
            EngineError::Malformed(reason.to_string())
        }
        _ => EngineError::Malformed("the file could not be opened".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgra_pixels_become_opaque_rgba() {
        let mut bytes = vec![10, 20, 30, 0, 40, 50, 60, 7];
        to_opaque_rgba(&mut bytes);
        assert_eq!(bytes, vec![30, 20, 10, 255, 60, 50, 40, 255]);
    }

    #[test]
    fn the_render_budget_allows_a_real_page_and_refuses_an_absurd_one() {
        // A4 at 4x is about 8 megapixels; at 40x it is 800.
        assert!(check_render_budget(595.0, 842.0, 4.0).is_ok());
        assert!(matches!(
            check_render_budget(595.0, 842.0, 40.0),
            Err(EngineError::Export(_))
        ));
    }

    #[test]
    fn page_file_names_are_one_based_and_zero_padded() {
        assert_eq!(
            page_file_name("report", 0, ImageFormat::Png),
            "report-0001.png"
        );
        assert_eq!(
            page_file_name("report", 9, ImageFormat::Jpeg),
            "report-0010.jpg"
        );
        assert_eq!(
            page_file_name("report", 499, ImageFormat::Png),
            "report-0500.png"
        );
    }

    #[test]
    fn a_cancelled_job_stops_before_the_next_item() {
        let cancel = AtomicBool::new(false);
        let mut reported = Vec::new();

        {
            let mut report = |done, total| reported.push((done, total));
            let mut job = JobProgress::new(&cancel, &mut report);

            assert!(job.check().is_ok());
            job.step(1, 3);

            cancel.store(true, Ordering::Relaxed);
            assert!(matches!(job.check(), Err(EngineError::Cancelled)));
        }

        assert_eq!(reported, vec![(1, 3)]);
    }
}
