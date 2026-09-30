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

/// Which export the export window is set up to do.
///
/// One window serves all of them, because they are the same job with different
/// inputs: pick some material, choose a shape, name an output. Five separate
/// dialogs would make the user pick the right one from a menu before they can
/// even see the options, and switching between them would mean starting over.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum ExportTask {
    /// The page in view, as one image file.
    #[default]
    PageImage,
    /// Every page, as one image file each.
    AllPageImages,
    /// A page range, as a new PDF.
    PagePdf,
    /// Images the user picks, as a new PDF with one page each.
    ImagesToPdf,
    /// Documents the user picks, concatenated into one.
    MergePdfs,
}

impl ExportTask {
    /// Every task, in the order the window lists them.
    pub const ALL: [Self; 5] = [
        Self::PageImage,
        Self::AllPageImages,
        Self::PagePdf,
        Self::ImagesToPdf,
        Self::MergePdfs,
    ];

    /// Short name, used for the task strip and the window title.
    pub const fn label(self) -> &'static str {
        match self {
            Self::PageImage => "Page to image",
            Self::AllPageImages => "Pages to images",
            Self::PagePdf => "Pages to PDF",
            Self::ImagesToPdf => "Images to PDF",
            Self::MergePdfs => "Merge PDFs",
        }
    }

    /// One sentence saying what the task produces.
    pub const fn blurb(self) -> &'static str {
        match self {
            Self::PageImage => "Save the page you are looking at as a picture.",
            Self::AllPageImages => "Save every page as its own picture, numbered in order.",
            Self::PagePdf => "Make a new PDF containing only the pages you choose.",
            Self::ImagesToPdf => "Turn pictures into pages of a new PDF, one page each.",
            Self::MergePdfs => "Join several PDFs into one, in the order you set.",
        }
    }

    /// Whether the task reads the open document.
    ///
    /// The two composition tasks work from files the user picks, so they are
    /// usable from an empty window — which is exactly when someone wants to
    /// merge two PDFs they have not opened.
    pub const fn needs_document(self) -> bool {
        match self {
            Self::PageImage | Self::AllPageImages | Self::PagePdf => true,
            Self::ImagesToPdf | Self::MergePdfs => false,
        }
    }

    /// Label for the button that starts the task.
    pub const fn action_label(self) -> &'static str {
        match self {
            Self::PageImage => "Save image",
            Self::AllPageImages => "Save images",
            Self::PagePdf => "Save PDF",
            Self::ImagesToPdf => "Create PDF",
            Self::MergePdfs => "Merge",
        }
    }

    /// Whether the task reads files the user picks rather than the document.
    pub const fn needs_sources(self) -> bool {
        match self {
            Self::ImagesToPdf | Self::MergePdfs => true,
            Self::PageImage | Self::AllPageImages | Self::PagePdf => false,
        }
    }

    /// Whether the task writes one image per page into a directory.
    pub const fn writes_a_directory(self) -> bool {
        matches!(self, Self::AllPageImages)
    }
}

/// Resolution an exported page is rasterized at.
///
/// Presets rather than a free number: the useful values are a handful of
/// well-known ones, and a spinner invites values that are either uselessly
/// small or large enough to trip the render budget.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub enum ExportDpi {
    /// 72 dpi — one pixel per PDF point, the size the page is defined at.
    Point,
    /// 150 dpi — a good compromise for a screen-only copy.
    Screen,
    /// 300 dpi — print resolution, and the default.
    #[default]
    Print,
    /// 600 dpi — for a page that will be enlarged or reprinted.
    Maximum,
}

impl ExportDpi {
    /// Every preset, from smallest to largest.
    pub const ALL: [Self; 4] = [Self::Point, Self::Screen, Self::Print, Self::Maximum];

    /// Dots per inch.
    pub const fn dpi(self) -> u32 {
        match self {
            Self::Point => 72,
            Self::Screen => 150,
            Self::Print => 300,
            Self::Maximum => 600,
        }
    }

    /// Short label for the picker.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Point => "72 dpi",
            Self::Screen => "150 dpi",
            Self::Print => "300 dpi",
            Self::Maximum => "600 dpi",
        }
    }

    /// What the preset is for, shown under the picker.
    pub const fn blurb(self) -> &'static str {
        match self {
            Self::Point => "Page size, for a quick copy",
            Self::Screen => "Sharp on a screen, small file",
            Self::Print => "Print quality — the usual choice",
            Self::Maximum => "For enlarging or reprinting",
        }
    }

    /// Device pixels per PDF point, which is what the renderer takes.
    ///
    /// `u32` to `f32` is lossy in principle; these four values are all exactly
    /// representable, and a dpi figure large enough to lose precision would be
    /// past the render budget long before it got here.
    #[allow(clippy::cast_precision_loss)]
    pub fn scale(self) -> f32 {
        self.dpi() as f32 / 72.0
    }
}

/// How the page is sized when an image becomes one.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub enum ImagePageSize {
    /// The page is the image: a 96 dpi screenshot keeps the size it had on
    /// screen. The right choice for a single screenshot or a scan.
    #[default]
    MatchImage,
    /// A4 portrait, with the image scaled to fit and centred.
    A4,
    /// US Letter portrait, with the image scaled to fit and centred.
    Letter,
}

impl ImagePageSize {
    /// Every option, in picker order.
    pub const ALL: [Self; 3] = [Self::MatchImage, Self::A4, Self::Letter];

    /// Short label for the picker.
    pub const fn label(self) -> &'static str {
        match self {
            Self::MatchImage => "Match the image",
            Self::A4 => "A4 portrait",
            Self::Letter => "US Letter portrait",
        }
    }

    /// What the option does to the output.
    pub const fn blurb(self) -> &'static str {
        match self {
            Self::MatchImage => "Each page is exactly its image",
            Self::A4 => "210 × 297 mm, image fitted and centred",
            Self::Letter => "8.5 × 11 in, image fitted and centred",
        }
    }

    /// Sheet size in points, or `None` when the page is the image.
    ///
    /// PDF points are 1/72 inch, so the sizes are the paper sizes converted
    /// once here rather than at every call site.
    pub const fn sheet(self) -> Option<(f32, f32)> {
        match self {
            Self::MatchImage => None,
            // 210mm x 297mm at 72 points per inch.
            Self::A4 => Some((595.276, 841.89)),
            // 8.5in x 11in at 72 points per inch.
            Self::Letter => Some((612.0, 792.0)),
        }
    }
}

/// The pixel size a page renders to at a given scale.
///
/// Exposed so the window can tell the user what they are about to write before
/// they commit to it — "2480 × 3508 px" is the number that decides whether the
/// result is what they wanted, and it is not obvious from a dpi figure alone.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn pixel_size(width_pt: f32, height_pt: f32, scale: f32) -> (u32, u32) {
    (
        (width_pt * scale).round().max(1.0) as u32,
        (height_pt * scale).round().max(1.0) as u32,
    )
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
/// Print resolution, and the default the window opens on. Exporting at the
/// on-screen zoom instead would turn "save this page as an image" into "save a
/// thumbnail" whenever the view happened to be zoomed out, which is the
/// opposite of what the menu item promises.
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

    #[test]
    fn every_resolution_preset_matches_its_dpi() {
        for dpi in ExportDpi::ALL {
            let expected = f32::from(u16::try_from(dpi.dpi()).unwrap_or(u16::MAX)) / 72.0;
            assert!(
                (dpi.scale() - expected).abs() < f32::EPSILON,
                "{dpi:?} scale mismatch"
            );
        }
        // The default has to be the print preset, or the window would open on
        // something other than the documented resolution.
        assert_eq!(ExportDpi::default(), ExportDpi::Print);
        assert!((ExportDpi::default().scale() - EXPORT_SCALE).abs() < f32::EPSILON);
    }

    #[test]
    fn a_page_at_three_hundred_dpi_is_the_size_people_expect() {
        // A4 at 300 dpi, the number a user can check against any image editor.
        let (width, height) = pixel_size(595.276, 841.89, ExportDpi::Print.scale());
        assert!(width.abs_diff(2480) <= 1, "got {width}");
        assert!(height.abs_diff(3508) <= 1, "got {height}");

        // Degenerate inputs must still describe a real image: a zero-sized
        // render would fail deep inside the engine with a worse message.
        assert_eq!(pixel_size(0.0, 0.0, 1.0), (1, 1));
    }

    #[test]
    fn only_the_composition_tasks_work_without_a_document() {
        for task in ExportTask::ALL {
            assert_eq!(
                task.needs_document(),
                !task.needs_sources(),
                "{task:?} must read either the document or picked files, not both"
            );
        }
        assert!(!ExportTask::MergePdfs.needs_document());
        assert!(!ExportTask::ImagesToPdf.needs_document());
        assert!(ExportTask::PageImage.needs_document());
        assert!(ExportTask::PagePdf.needs_document());
        // Exactly one task writes a directory rather than a single file.
        let directories = ExportTask::ALL
            .iter()
            .filter(|task| task.writes_a_directory())
            .count();
        assert_eq!(directories, 1);
    }

    #[test]
    fn standard_sheets_are_their_real_paper_sizes_and_matching_is_not_a_sheet() {
        assert_eq!(ImagePageSize::MatchImage.sheet(), None);

        let (width, height) = ImagePageSize::A4.sheet().expect("A4 has a sheet");
        assert!((width - 595.276).abs() < 0.01, "got {width}");
        assert!((height - 841.89).abs() < 0.01, "got {height}");

        let (width, height) = ImagePageSize::Letter.sheet().expect("Letter has a sheet");
        assert!((width - 612.0).abs() < f32::EPSILON, "got {width}");
        assert!((height - 792.0).abs() < f32::EPSILON, "got {height}");

        // Both sheets are portrait, which is what the labels promise.
        for size in ImagePageSize::ALL {
            if let Some((width, height)) = size.sheet() {
                assert!(height > width, "{size:?} must be portrait");
            }
        }
    }
}
