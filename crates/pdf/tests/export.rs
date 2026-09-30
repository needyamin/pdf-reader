//! Export, composition and merge tests against real documents.
//!
//! These exercise the parts of the engine that write files, so they need both a
//! staged `PDFium` library and the generated fixtures. Either can be absent in a
//! fresh checkout, so every test skips with a message instead of failing.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use pdfreader_core::{IMAGE_POINTS_PER_PIXEL, ImageFormat, ImagePageSize, PageRange, Rotation};
use pdfreader_pdf::engine::{PdfEngine, PdfiumEngine};
use pdfreader_pdf::{EngineError, JobProgress};

/// Workspace root, derived from this crate's manifest directory.
fn workspace_root() -> PathBuf {
    // crates/pdf -> ../.. -> repo root
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// Locate the staged `PDFium` library, scanning `target/native/*`.
fn find_pdfium() -> Option<PathBuf> {
    let native = workspace_root().join("target").join("native");
    let entries = std::fs::read_dir(&native).ok()?;
    for entry in entries.flatten() {
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

/// Bind an engine, or explain why the test cannot run.
fn engine() -> Option<PdfiumEngine> {
    let Some(dir) = find_pdfium() else {
        eprintln!("SKIP: no PDFium library staged under target/native");
        return None;
    };
    Some(PdfiumEngine::bind(Some(&dir)).expect("pdfium should bind"))
}

/// A generated fixture, or `None` with an explanation.
fn fixture(name: &str) -> Option<PathBuf> {
    let path = workspace_root().join("target").join("test-pdfs").join(name);
    if path.exists() {
        Some(path)
    } else {
        eprintln!(
            "SKIP: fixture {} not found (run: python3 tools/generate_fixtures.py target/test-pdfs)",
            path.display()
        );
        None
    }
}

/// An empty directory for one test's output.
///
/// Named per test so tests can run in parallel without treading on each other,
/// and cleared first so a rerun cannot pass against the previous run's files.
fn output_dir(name: &str) -> PathBuf {
    let dir = workspace_root()
        .join("target")
        .join("test-output")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("output directory should be creatable");
    dir
}

/// A progress handle over a flag and a log, for tests that do not care about
/// progress but still have to pass one.
///
/// A macro rather than a function because `JobProgress` borrows both the flag
/// and the closure, so a helper would have to hand back a value that outlives
/// its own frame.
macro_rules! job {
    ($cancel:expr, $log:expr) => {
        JobProgress::new($cancel, &mut |done, total| $log.push((done, total)))
    };
}

#[test]
fn merging_the_same_document_twice_doubles_its_pages() {
    let Some(mut engine) = engine() else { return };
    let Some(source) = fixture("small-3p.pdf") else {
        return;
    };

    let dir = output_dir("merge");
    let output = dir.join("merged.pdf");

    let cancel = AtomicBool::new(false);
    let mut log = Vec::new();
    let pages = engine
        .merge_pdfs(
            &[source.clone(), source],
            &output,
            &mut job!(&cancel, &mut log),
        )
        .expect("merge should succeed");

    assert_eq!(pages, 6);
    assert_eq!(log.last(), Some(&(2, 2)), "progress should reach the end");

    // Reopening is the only proof that what was written is a usable PDF.
    let (_handle, info) = engine.open(&output, None).expect("merged file should open");
    assert_eq!(info.pages.len(), 6);
}

#[test]
fn a_corrupt_source_is_reported_against_its_path() {
    let Some(mut engine) = engine() else { return };
    let (Some(good), Some(corrupt)) = (fixture("small-3p.pdf"), fixture("corrupt.pdf")) else {
        return;
    };

    let dir = output_dir("merge-corrupt");
    let cancel = AtomicBool::new(false);
    let mut log = Vec::new();

    let error = engine
        .merge_pdfs(
            &[good, corrupt.clone()],
            &dir.join("merged.pdf"),
            &mut job!(&cancel, &mut log),
        )
        .expect_err("a corrupt source must fail the job");

    match error {
        EngineError::Source { path, .. } => assert_eq!(path, corrupt),
        other => panic!("expected a named source error, got {other:?}"),
    }
}

#[test]
fn an_encrypted_source_is_named_rather_than_skipped() {
    let Some(mut engine) = engine() else { return };
    let (Some(good), Some(encrypted)) = (fixture("small-3p.pdf"), fixture("encrypted.pdf")) else {
        return;
    };

    let dir = output_dir("merge-encrypted");
    let cancel = AtomicBool::new(false);
    let mut log = Vec::new();

    let error = engine
        .merge_pdfs(
            &[good, encrypted.clone()],
            &dir.join("merged.pdf"),
            &mut job!(&cancel, &mut log),
        )
        .expect_err("an encrypted source must fail the job");

    match error {
        EngineError::Source { path, reason } => {
            assert_eq!(path, encrypted);
            assert!(reason.contains("encrypted"), "unhelpful reason: {reason}");
        }
        other => panic!("expected a named source error, got {other:?}"),
    }
}

#[test]
fn extracting_one_page_produces_a_one_page_document() {
    let Some(mut engine) = engine() else { return };
    let Some(source) = fixture("small-3p.pdf") else {
        return;
    };

    let (handle, info) = engine.open(&source, None).expect("fixture should open");
    let bytes = engine
        .export_pages_to_bytes(handle, Some(PageRange::single(1)))
        .expect("extraction should succeed");

    let dir = output_dir("extract");
    let output = dir.join("page-2.pdf");
    std::fs::write(&output, &bytes).expect("extracted bytes should be writable");

    let (_handle, extracted) = engine
        .open(&output, None)
        .expect("extracted file should open");
    assert_eq!(extracted.pages.len(), 1);
    assert_eq!(
        extracted.pages[0], info.pages[1],
        "the extracted page should keep its geometry"
    );
}

#[test]
fn exporting_the_whole_document_as_bytes_matches_the_page_count() {
    let Some(mut engine) = engine() else { return };
    let Some(source) = fixture("small-3p.pdf") else {
        return;
    };

    let (handle, _info) = engine.open(&source, None).expect("fixture should open");
    let bytes = engine
        .export_pages_to_bytes(handle, None)
        .expect("a full export should succeed");

    let dir = output_dir("extract-all");
    let output = dir.join("all.pdf");
    std::fs::write(&output, &bytes).expect("bytes should be writable");

    let (_handle, copied) = engine.open(&output, None).expect("copy should open");
    assert_eq!(copied.pages.len(), 3);
}

#[test]
fn exporting_a_page_writes_a_decodable_image() {
    let Some(mut engine) = engine() else { return };
    let Some(source) = fixture("small-3p.pdf") else {
        return;
    };

    let (handle, _info) = engine.open(&source, None).expect("fixture should open");
    let rendered = engine
        .render_page(handle, 0, Rotation::None, 1.0)
        .expect("page should render");

    let dir = output_dir("page-image");
    for format in ImageFormat::ALL {
        let path = dir.join(format!("page.{}", format.extension()));
        engine
            .export_page_image(handle, 0, Rotation::None, 1.0, format, &path)
            .unwrap_or_else(|error| panic!("{format:?} export failed: {error}"));

        let decoded =
            image::open(&path).unwrap_or_else(|error| panic!("{format:?} unreadable: {error}"));
        assert_eq!(
            (decoded.width(), decoded.height()),
            (rendered.width, rendered.height),
            "{format:?} dimensions should match the render"
        );
    }
}

#[test]
fn images_become_a_pdf_with_one_page_each() {
    let Some(mut engine) = engine() else { return };
    let dir = output_dir("images-to-pdf");

    // Two deliberately different sizes, so a page that ignored its image would
    // show up as a mismatch rather than passing by coincidence.
    let sizes: [(u16, u16); 2] = [(40, 20), (15, 35)];
    let mut images = Vec::new();
    for (index, (width, height)) in sizes.iter().enumerate() {
        let path = dir.join(format!("image-{index}.png"));
        image::RgbaImage::new(u32::from(*width), u32::from(*height))
            .save(&path)
            .expect("test image should save");
        images.push(path);
    }

    let output = dir.join("images.pdf");
    let cancel = AtomicBool::new(false);
    let mut log = Vec::new();
    let pages = engine
        .images_to_pdf(
            &images,
            &output,
            ImagePageSize::MatchImage,
            &mut job!(&cancel, &mut log),
        )
        .expect("images to PDF should succeed");

    assert_eq!(pages, 2);
    assert_eq!(log.last(), Some(&(2, 2)));

    let (_handle, info) = engine.open(&output, None).expect("built PDF should open");
    assert_eq!(info.pages.len(), 2);
    for (page, (width, height)) in info.pages.iter().zip(sizes) {
        // The page is the image at the assumed 96 dpi source resolution, so a
        // 96-pixel-wide image becomes a one-inch (72 point) page.
        let expected_width = f32::from(width) * IMAGE_POINTS_PER_PIXEL;
        let expected_height = f32::from(height) * IMAGE_POINTS_PER_PIXEL;
        // A tenth of a point of slack: the page size round-trips through the
        // PDF's own units, so an exact comparison would be testing PDFium.
        assert!(
            (page.width_pt - expected_width).abs() < 0.1,
            "page width {} should be the image's {expected_width}",
            page.width_pt
        );
        assert!(
            (page.height_pt - expected_height).abs() < 0.1,
            "page height {} should be the image's {expected_height}",
            page.height_pt
        );
    }
}

/// A sheet layout must produce real paper, and must not stretch the image to
/// fill it: the page is A4 and the image keeps its own aspect ratio.
#[test]
fn images_land_on_standard_sheets_without_being_stretched() {
    let Some(mut engine) = engine() else { return };
    let dir = output_dir("images-to-a4");

    // A wide image on a portrait sheet: limited by the width, so a layout that
    // stretched it would produce a page-tall image and be obvious here.
    let path = dir.join("wide.png");
    image::RgbaImage::new(1600, 900)
        .save(&path)
        .expect("test image should save");

    let output = dir.join("a4.pdf");
    let cancel = AtomicBool::new(false);
    let mut log = Vec::new();
    engine
        .images_to_pdf(
            std::slice::from_ref(&path),
            &output,
            ImagePageSize::A4,
            &mut job!(&cancel, &mut log),
        )
        .expect("images to PDF should succeed");

    let (_handle, info) = engine.open(&output, None).expect("built PDF should open");
    assert_eq!(info.pages.len(), 1);

    let (sheet_width, sheet_height) = ImagePageSize::A4.sheet().expect("A4 has a sheet");
    let page = &info.pages[0];
    assert!(
        (page.width_pt - sheet_width).abs() < 0.1,
        "page width {} should be A4's {sheet_width}",
        page.width_pt
    );
    assert!(
        (page.height_pt - sheet_height).abs() < 0.1,
        "page height {} should be A4's {sheet_height}",
        page.height_pt
    );
}

#[test]
fn exporting_all_pages_reports_progress_and_honours_cancellation() {
    let Some(mut engine) = engine() else { return };
    let Some(source) = fixture("small-3p.pdf") else {
        return;
    };

    let (handle, info) = engine.open(&source, None).expect("fixture should open");
    let expected = u32::try_from(info.pages.len()).expect("fixture has few pages");

    let dir = output_dir("all-pages");
    let cancel = AtomicBool::new(false);
    let mut log = Vec::new();
    let written = engine
        .export_all_pages(
            handle,
            &dir,
            "small",
            Rotation::None,
            1.0,
            ImageFormat::Png,
            &mut job!(&cancel, &mut log),
        )
        .expect("bulk export should succeed");

    assert_eq!(written, expected);
    assert_eq!(log.last(), Some(&(expected, expected)));
    assert_eq!(
        std::fs::read_dir(&dir)
            .expect("output directory should exist")
            .count(),
        info.pages.len()
    );

    // A job cancelled before it starts writes nothing at all, which is the
    // cheapest way to prove the flag is actually consulted.
    let cancelled = AtomicBool::new(true);
    let mut log = Vec::new();
    let error = engine
        .export_all_pages(
            handle,
            &output_dir("all-pages-cancelled"),
            "small",
            Rotation::None,
            1.0,
            ImageFormat::Png,
            &mut job!(&cancelled, &mut log),
        )
        .expect_err("a cancelled job must not report success");

    assert!(matches!(error, EngineError::Cancelled), "got {error:?}");
    assert!(log.is_empty(), "a cancelled job should not report progress");
}

#[test]
fn an_absurd_export_scale_is_refused_before_rendering() {
    let Some(mut engine) = engine() else { return };
    let Some(source) = fixture("small-3p.pdf") else {
        return;
    };

    let (handle, _info) = engine.open(&source, None).expect("fixture should open");
    let dir = output_dir("scale-limit");
    let path = dir.join("huge.png");

    let error = engine
        .export_page_image(handle, 0, Rotation::None, 200.0, ImageFormat::Png, &path)
        .expect_err("an absurd scale must be refused");

    assert!(matches!(error, EngineError::Export(_)), "got {error:?}");
    assert!(!path.exists(), "nothing should have been written");
}

/// A page index past the end of the document is a range error, not a panic.
#[test]
fn exporting_a_page_that_does_not_exist_is_an_error() {
    let Some(mut engine) = engine() else { return };
    let Some(source) = fixture("small-3p.pdf") else {
        return;
    };

    let (handle, _info) = engine.open(&source, None).expect("fixture should open");
    let dir = output_dir("out-of-range");

    let error = engine
        .export_page_image(
            handle,
            99,
            Rotation::None,
            1.0,
            ImageFormat::Png,
            &dir.join("nope.png"),
        )
        .expect_err("a missing page must be an error");

    assert!(
        matches!(error, EngineError::PageOutOfRange { index: 99, .. }),
        "got {error:?}"
    );
}

/// `PageRange` is clamped by the caller, so an inverted range must not silently
/// export something else.
#[test]
fn an_inverted_range_is_refused() {
    let Some(mut engine) = engine() else { return };
    let Some(source) = fixture("small-3p.pdf") else {
        return;
    };

    let (handle, _info) = engine.open(&source, None).expect("fixture should open");

    // `clamp_to` is what the shell uses; it collapses an inverted range to None,
    // which means "whole document" and must not be reached by accident.
    assert_eq!(PageRange::new(2, 0).clamp_to(3), None);

    let error = engine
        .export_pages_to_bytes(handle, Some(PageRange::new(2, 0)))
        .expect_err("an inverted range must not produce a document");
    assert!(matches!(error, EngineError::Pdfium(_)), "got {error:?}");
}

/// Writing into a directory that does not exist yet should work: the shell
/// picks a folder, but nothing stops the user deleting it in between.
#[test]
fn exporting_into_a_missing_directory_creates_it() {
    let Some(mut engine) = engine() else { return };
    let Some(source) = fixture("small-3p.pdf") else {
        return;
    };

    let (handle, _info) = engine.open(&source, None).expect("fixture should open");
    let dir = output_dir("nested").join("a").join("b");

    let cancel = AtomicBool::new(false);
    let mut log = Vec::new();
    let written = engine
        .export_all_pages(
            handle,
            &dir,
            "small",
            Rotation::None,
            1.0,
            ImageFormat::Jpeg,
            &mut job!(&cancel, &mut log),
        )
        .expect("export should create the directory");

    assert_eq!(written, 3);
    assert!(dir.join("small-0001.jpg").is_file());
}

/// Every format must produce a file whose bytes match its extension.
#[test]
fn image_files_start_with_their_format_signature() {
    let Some(mut engine) = engine() else { return };
    let Some(source) = fixture("small-3p.pdf") else {
        return;
    };

    let (handle, _info) = engine.open(&source, None).expect("fixture should open");
    let dir = output_dir("signatures");

    let signatures: [(ImageFormat, &[u8]); 2] = [
        (ImageFormat::Png, &[0x89, b'P', b'N', b'G']),
        (ImageFormat::Jpeg, &[0xFF, 0xD8, 0xFF]),
    ];

    for (format, signature) in signatures {
        let path = dir.join(format!("page.{}", format.extension()));
        engine
            .export_page_image(handle, 0, Rotation::None, 1.0, format, &path)
            .expect("export should succeed");

        let bytes = std::fs::read(&path).expect("exported file should be readable");
        assert!(
            bytes.starts_with(signature),
            "{format:?} output does not look like {}",
            format.label()
        );
    }
}

/// A missing image must name itself, not just fail the job.
#[test]
fn a_missing_image_is_reported_against_its_path() {
    let Some(mut engine) = engine() else { return };
    let dir = output_dir("missing-image");
    let missing = Path::new("does-not-exist.png").to_path_buf();

    let cancel = AtomicBool::new(false);
    let mut log = Vec::new();
    let error = engine
        .images_to_pdf(
            std::slice::from_ref(&missing),
            &dir.join("out.pdf"),
            ImagePageSize::MatchImage,
            &mut job!(&cancel, &mut log),
        )
        .expect_err("a missing image must fail the job");

    match error {
        EngineError::Source { path, .. } => assert_eq!(path, missing),
        other => panic!("expected a named source error, got {other:?}"),
    }
}
