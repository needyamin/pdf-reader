//! Export and conversion vocabulary.
//!
//! The reducer has to be able to say "write this page out as a PNG" without
//! depending on the PDF engine crate, so the small set of types shared by the
//! UI, the reducer and the shell lives here. Everything in this module is pure
//! data — no IO, no engine, no UI toolkit.

use serde::{Deserialize, Serialize};

/// Image format an exported page is written as.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub enum ImageFormat {
    /// Lossless, larger. The right default for text and line art.
    #[default]
    Png,
    /// Lossy, far smaller. Better for scanned pages and photographs.
    Jpeg,
}

impl ImageFormat {
    /// File extension, without the dot.
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
        }
    }

    /// Short label for menu entries.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
        }
    }

    /// Every format, in menu order.
    pub const ALL: [Self; 2] = [Self::Png, Self::Jpeg];
}

/// An inclusive, zero-based page range.
///
/// Half-open ranges are the Rust default, but page ranges as users type and
/// read them are inclusive ("pages 3 to 7"), so the conversion happens once,
/// here, rather than at every call site.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PageRange {
    /// First page, zero-based.
    pub first: u32,
    /// Last page, zero-based and included.
    pub last: u32,
}

impl PageRange {
    /// A range from `first` to `last`, both included.
    pub const fn new(first: u32, last: u32) -> Self {
        Self { first, last }
    }

    /// A range covering exactly one page.
    pub const fn single(page: u32) -> Self {
        Self {
            first: page,
            last: page,
        }
    }

    /// A range covering a whole document of `count` pages.
    ///
    /// Returns `None` for an empty document, which has no pages to export.
    pub const fn whole(count: u32) -> Option<Self> {
        if count == 0 {
            None
        } else {
            Some(Self {
                first: 0,
                last: count - 1,
            })
        }
    }

    /// How many pages the range covers, or 0 when it is empty.
    pub const fn len(self) -> u32 {
        if self.last < self.first {
            0
        } else {
            self.last - self.first + 1
        }
    }

    /// Whether the range covers no pages at all.
    ///
    /// An inverted range is empty rather than an error: it is what a caller
    /// gets from arithmetic that ran off the end, and the honest answer is
    /// that there is nothing to export.
    pub const fn is_empty(self) -> bool {
        self.last < self.first
    }

    /// Clamp to a document of `count` pages.
    ///
    /// A range that starts past the end of the document, or an inverted range,
    /// collapses to `None`: there is nothing to export, and silently exporting
    /// the last page instead would be worse than saying so.
    pub const fn clamp_to(self, count: u32) -> Option<Self> {
        if count == 0 || self.first >= count || self.last < self.first {
            return None;
        }
        Some(Self {
            first: self.first,
            last: if self.last >= count {
                count - 1
            } else {
                self.last
            },
        })
    }
}

/// Why a PDF is being produced, which decides what happens to it afterwards.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExportTarget {
    /// The user picked a destination; the file stays there.
    SaveFile,
    /// A temporary copy handed to the operating system's print pipeline.
    ///
    /// The shell writes it to the temp directory, prints it, and sweeps it up
    /// on the next launch rather than deleting it immediately: the print
    /// hand-off returns before the viewer has read the file.
    Print,
}

/// Resolution a page is rasterized at when exported as an image, in device
/// pixels per PDF point.
///
/// 300 dpi, which is print resolution. Exporting at the on-screen zoom instead
/// would turn "save this page as an image" into "save a thumbnail" whenever the
/// view happened to be zoomed out, which is the opposite of what the menu item
/// promises.
pub const EXPORT_SCALE: f32 = 300.0 / 72.0;

/// Assumed resolution of an image being turned into a PDF page, in dpi.
///
/// 96 dpi is the usual resolution of a screen image, so a screenshot of a page
/// becomes a page of the size it appeared at on screen. Assuming the PDF's
/// native 72 dpi would inflate every page by a third; assuming nothing and
/// treating a pixel as a point would turn a 300 dpi scan into a poster.
pub const IMAGE_SOURCE_DPI: f32 = 96.0;

/// Page size per pixel for "images to PDF", in points.
pub const IMAGE_POINTS_PER_PIXEL: f32 = 72.0 / IMAGE_SOURCE_DPI;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_covers_every_page_and_refuses_an_empty_document() {
        assert_eq!(PageRange::whole(3), Some(PageRange::new(0, 2)));
        assert_eq!(PageRange::whole(1), Some(PageRange::new(0, 0)));
        assert_eq!(PageRange::whole(0), None);
    }

    #[test]
    fn len_counts_inclusively_and_an_inverted_range_is_empty() {
        assert_eq!(PageRange::new(0, 0).len(), 1);
        assert_eq!(PageRange::new(2, 5).len(), 4);
        assert_eq!(PageRange::new(5, 2).len(), 0);

        // `is_empty` must agree with `len` in both directions.
        assert!(!PageRange::new(0, 0).is_empty());
        assert!(!PageRange::new(2, 5).is_empty());
        assert!(PageRange::new(5, 2).is_empty());
    }

    #[test]
    fn clamping_trims_the_end_and_rejects_ranges_past_the_document() {
        assert_eq!(
            PageRange::new(1, 99).clamp_to(4),
            Some(PageRange::new(1, 3))
        );
        assert_eq!(PageRange::new(3, 3).clamp_to(4), Some(PageRange::single(3)));
        // Past the end, inverted, or an empty document: nothing to export.
        assert_eq!(PageRange::new(4, 4).clamp_to(4), None);
        assert_eq!(PageRange::new(3, 1).clamp_to(4), None);
        assert_eq!(PageRange::new(0, 0).clamp_to(0), None);
    }

    #[test]
    fn formats_have_distinct_extensions() {
        let mut extensions: Vec<&str> = ImageFormat::ALL
            .iter()
            .map(|format| format.extension())
            .collect();
        let count = extensions.len();
        extensions.sort_unstable();
        extensions.dedup();
        assert_eq!(extensions.len(), count, "extensions must be unique");
    }

    #[test]
    fn export_resolution_is_print_quality_and_image_size_is_physical() {
        // 300 dpi: an A4 page exports at roughly 2480x3508 pixels.
        assert!((EXPORT_SCALE - 4.166_667).abs() < 0.000_1, "{EXPORT_SCALE}");

        // 96 dpi source: a 96-pixel-wide image becomes a one-inch page.
        assert!((IMAGE_POINTS_PER_PIXEL - 0.75).abs() < f32::EPSILON);
        assert!((96.0 * IMAGE_POINTS_PER_PIXEL - 72.0).abs() < 0.000_1);
    }
}
