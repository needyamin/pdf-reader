//! Printing, built into the application.
//!
//! The shell's `print` verb cannot be trusted: on a modern Windows box the
//! default PDF handler — a UWP viewer such as Edge, or this application itself —
//! usually registers no `print` verb, so `ShellExecute` fails and nothing is
//! ever printed. Pages are therefore rasterized here and sent to the printer
//! through GDI, after the standard printer dialog has chosen the device.
//!
//! Nothing is printed silently: the dialog is always shown, and the job is
//! whatever the user confirms there.

use pdfreader_core::{FieldValue, FormFieldInfo, FormFieldType};
use pdfreader_pdf::engine::TilePixels;

/// One rasterized page, ready for the printer.
///
/// Byte order is BGRA — what `PDFium` produces and what a 32-bit `BI_RGB` DIB
/// expects, so the pixels are handed to the printer without a swizzle. Rows run
/// top to bottom, which the DIB header declares with a negative height.
pub struct PageBitmap {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Row-major BGRA bytes, `width * height * 4` long.
    pub data: Vec<u8>,
}

/// Why a print job did not finish.
#[derive(Debug)]
pub enum PrintError {
    /// The user stopped the job; not a failure.
    Cancelled,
    /// Something went wrong, described for the user.
    Failed(String),
}

/// Highest render scale a printer's reported resolution may ask for.
///
/// 16x is 1152 dpi, past any desktop printer; a driver reporting more than that
/// is reporting nonsense, not resolution.
const MAX_PRINT_SCALE: f32 = 16.0;

/// Most pixels one page may be rasterized to.
///
/// Full printer resolution is the goal — 600 dpi on A4 is 139 MB of pixels for
/// one sheet — but a poster at 1200 dpi would be gigabytes, so the scale is
/// trimmed once this budget is reached. It is high enough that ordinary pages
/// are never reduced.
pub const MAX_PRINT_MEGAPIXELS: f64 = 100.0;

/// The render scale to use for one page: `scale`, reduced only if rasterizing
/// the page at that scale would exceed [`MAX_PRINT_MEGAPIXELS`].
pub fn clamp_scale(scale: f32, width_pt: f32, height_pt: f32) -> f32 {
    let pixels = f64::from(width_pt) * f64::from(height_pt) * f64::from(scale) * f64::from(scale);
    let limit = MAX_PRINT_MEGAPIXELS * 1_000_000.0;
    // `!(pixels > limit)` also catches a NaN size, which must not shrink a page.
    if !(pixels > limit) {
        return scale;
    }
    (f64::from(scale) * (limit / pixels).sqrt()) as f32
}

/// Ask the user to choose a printer, returning its name.
///
/// `Ok(None)` means the dialog was cancelled. `Err` means the dialog could not
/// be shown at all — for instance on a platform with no shell print support.
#[cfg(windows)]
#[allow(unsafe_code)]
pub fn choose_printer() -> Result<Option<String>, String> {
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::Graphics::Gdi::DeleteDC;
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    use windows_sys::Win32::UI::Controls::Dialogs::{
        PD_NOPAGENUMS, PD_RETURNDC, PRINTDLGW, PrintDlgW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    /// The header of a `DEVNAMES` block: four offsets, then the strings.
    #[repr(C)]
    struct DevNames {
        driver: u16,
        device: u16,
        output: u16,
        default: u16,
    }

    // SAFETY: `PRINTDLGW` is a plain C struct of integers and pointers, and all
    // zero is its documented "no owner, default state" starting point.
    let mut dialog: PRINTDLGW = unsafe { std::mem::zeroed() };
    dialog.lStructSize = std::mem::size_of::<PRINTDLGW>() as u32;
    // The pages were chosen in the app's print window, preview and all, so the
    // dialog's own range controls are disabled rather than offering a second,
    // conflicting place to choose them.
    dialog.Flags = PD_RETURNDC | PD_NOPAGENUMS;
    // Own the dialog with our window so it is modal to the app and cannot be
    // lost behind it. The user just clicked Print here, so the foreground
    // window is ours.
    dialog.hwndOwner = unsafe { GetForegroundWindow() };

    // Release whatever the dialog allocated. Called on every path out.
    let release = |dialog: &mut PRINTDLGW| {
        // SAFETY: the handles belong to `dialog` and are released exactly once.
        unsafe {
            if !dialog.hDC.is_null() {
                DeleteDC(dialog.hDC);
                dialog.hDC = std::ptr::null_mut();
            }
            if !dialog.hDevMode.is_null() {
                GlobalFree(dialog.hDevMode);
                dialog.hDevMode = std::ptr::null_mut();
            }
            if !dialog.hDevNames.is_null() {
                GlobalFree(dialog.hDevNames);
                dialog.hDevNames = std::ptr::null_mut();
            }
        }
    };

    // SAFETY: `dialog` is fully initialized and outlives the call. The dialog is
    // modal on this thread — the UI thread — and pumps its own events.
    let accepted = unsafe { PrintDlgW(&mut dialog) };
    if accepted == 0 {
        // Cancelled, or the dialog itself failed. Either way, nothing to print.
        release(&mut dialog);
        return Ok(None);
    }

    // SAFETY: on success `hDevNames` is a `DEVNAMES` block owned by the dialog.
    let name = unsafe {
        let mut name = None;
        if !dialog.hDevNames.is_null() {
            let base = GlobalLock(dialog.hDevNames) as *const u16;
            if !base.is_null() {
                let header = base as *const DevNames;
                let start = base.add((*header).device as usize);
                let mut len = 0usize;
                while *start.add(len) != 0 {
                    len += 1;
                }
                name = Some(String::from_utf16_lossy(std::slice::from_raw_parts(
                    start, len,
                )));
                GlobalUnlock(dialog.hDevNames);
            }
        }
        name
    };

    release(&mut dialog);

    Ok(name.filter(|name| !name.is_empty()))
}

/// Ask the user to choose a printer, returning its name.
///
/// Only Windows can print; every other platform reports that up front rather
/// than pretending a dialog appeared.
#[cfg(not(windows))]
pub fn choose_printer() -> Result<Option<String>, String> {
    Err("printing is only supported on Windows".to_string())
}

/// Print `pages` (zero-based page indices, in order) to `printer`.
///
/// `render(page, scale)` rasterizes one page at `scale` device pixels per PDF
/// point; it is called once per page, so only one page image is ever alive.
/// `cancelled` is polled before each page. `doc_name` is what the printer's job
/// list shows.
#[cfg(windows)]
#[allow(unsafe_code)]
pub fn print_pages<F>(
    printer: &str,
    doc_name: &str,
    pages: &[u32],
    mut render: F,
    mut cancelled: impl FnMut() -> bool,
) -> Result<u32, PrintError>
where
    F: FnMut(u32, f32) -> Result<PageBitmap, PrintError>,
{
    use windows_sys::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDCW, DIB_RGB_COLORS, DeleteDC, GetDeviceCaps,
        HALFTONE, HORZRES, LOGPIXELSY, PHYSICALOFFSETX, PHYSICALOFFSETY, RGBQUAD, SRCCOPY,
        SetStretchBltMode, StretchDIBits, VERTRES,
    };
    use windows_sys::Win32::Storage::Xps::{
        AbortDoc, DOCINFOW, EndDoc, EndPage, StartDocW, StartPage,
    };

    let wide = |text: &str| -> Vec<u16> { text.encode_utf16().chain(std::iter::once(0)).collect() };

    let driver = wide("WINSPOOL");
    let device = wide(printer);

    // SAFETY: NUL-terminated wide strings that outlive the call; a null devmode
    // asks for the printer's defaults.
    let hdc = unsafe {
        CreateDCW(
            driver.as_ptr(),
            device.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if hdc.is_null() {
        return Err(PrintError::Failed(format!(
            "could not open the printer \"{printer}\""
        )));
    }

    // Delete the DC on every path out, including the early returns below.
    struct Dc(windows_sys::Win32::Graphics::Gdi::HDC);
    impl Drop for Dc {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            // SAFETY: the handle came from `CreateDCW` and is deleted once.
            unsafe { DeleteDC(self.0) };
        }
    }
    let dc = Dc(hdc);
    let hdc = dc.0;

    // SAFETY: `hdc` is a live printer DC.
    let (area_w, area_h, off_x, off_y, dpi) = unsafe {
        (
            GetDeviceCaps(hdc, HORZRES as i32),
            GetDeviceCaps(hdc, VERTRES as i32),
            GetDeviceCaps(hdc, PHYSICALOFFSETX as i32),
            GetDeviceCaps(hdc, PHYSICALOFFSETY as i32),
            GetDeviceCaps(hdc, LOGPIXELSY as i32),
        )
    };
    if area_w <= 0 || area_h <= 0 {
        return Err(PrintError::Failed(
            "the printer reported no printable area".to_string(),
        ));
    }
    let (area_w, area_h) = (area_w as f32, area_h as f32);
    // Render at the printer's own resolution. A driver reporting 600 dpi means
    // 600 dpi here too, so the page reaches the paper without a second
    // resampling pass — the whole point of printing the page rather than a
    // thumbnail. The clamp only guards against a nonsensical report; the
    // per-page pixel budget is applied by the caller, which knows page sizes.
    let scale = (dpi.max(72) as f32 / 72.0).clamp(1.0, MAX_PRINT_SCALE);

    let name = wide(doc_name);
    let info = DOCINFOW {
        cbSize: std::mem::size_of::<DOCINFOW>() as i32,
        lpszDocName: name.as_ptr(),
        lpszOutput: std::ptr::null(),
        lpszDatatype: std::ptr::null(),
        fwType: 0,
    };

    // SAFETY: `hdc` is live, and `info` and the strings it points at outlive it.
    if unsafe { StartDocW(hdc, &info) } <= 0 {
        return Err(PrintError::Failed(
            "the printer rejected the document".to_string(),
        ));
    }

    // Halftone stretching keeps photographs from banding.
    // SAFETY: `hdc` is a live printer DC.
    unsafe { SetStretchBltMode(hdc, HALFTONE) };

    let mut printed = 0u32;
    for &page in pages {
        if cancelled() {
            // SAFETY: `hdc` is live and the document is open.
            unsafe { AbortDoc(hdc) };
            return Err(PrintError::Cancelled);
        }

        let bitmap = match render(page, scale) {
            Ok(bitmap) => bitmap,
            Err(error) => {
                // SAFETY: `hdc` is live and the document is open.
                unsafe { AbortDoc(hdc) };
                return Err(error);
            }
        };
        if bitmap.width == 0 || bitmap.height == 0 {
            continue;
        }

        // SAFETY: `hdc` is live and the document is open.
        if unsafe { StartPage(hdc) } <= 0 {
            unsafe { AbortDoc(hdc) };
            return Err(PrintError::Failed(
                "the printer rejected a page".to_string(),
            ));
        }

        let (draw_w, draw_h) = draw_size(bitmap.width as f32, bitmap.height as f32, area_w, area_h);
        let x = off_x + ((area_w - draw_w) / 2.0) as i32;
        let y = off_y + ((area_h - draw_h) / 2.0) as i32;

        let header = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: bitmap.width as i32,
            // Negative height declares top-down rows, matching the raster order.
            biHeight: -(bitmap.height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        };
        let bmi = BITMAPINFO {
            bmiHeader: header,
            bmiColors: [RGBQUAD {
                rgbBlue: 0,
                rgbGreen: 0,
                rgbRed: 0,
                rgbReserved: 0,
            }],
        };

        // SAFETY: `hdc` is live, and `bmi` describes `bitmap.data` — exactly
        // `width * height * 4` BGRA bytes.
        let copied = unsafe {
            StretchDIBits(
                hdc,
                x,
                y,
                draw_w as i32,
                draw_h as i32,
                0,
                0,
                bitmap.width as i32,
                bitmap.height as i32,
                bitmap.data.as_ptr() as *const core::ffi::c_void,
                &bmi,
                DIB_RGB_COLORS,
                SRCCOPY,
            )
        };
        if copied <= 0 {
            // SAFETY: `hdc` is live and the document is open.
            unsafe { AbortDoc(hdc) };
            return Err(PrintError::Failed(
                "the printer rejected a page".to_string(),
            ));
        }

        // SAFETY: `hdc` is live and the document is open.
        if unsafe { EndPage(hdc) } <= 0 {
            unsafe { AbortDoc(hdc) };
            return Err(PrintError::Failed(
                "the printer rejected a page".to_string(),
            ));
        }
        printed += 1;
    }

    // SAFETY: `hdc` is live and the document is open.
    if unsafe { EndDoc(hdc) } <= 0 {
        return Err(PrintError::Failed(
            "the printer did not accept the document".to_string(),
        ));
    }

    Ok(printed)
}

/// Print `pages` (zero-based page indices, in order) to `printer`.
#[cfg(not(windows))]
pub fn print_pages<F>(
    _printer: &str,
    _doc_name: &str,
    _pages: &[u32],
    _render: F,
    _cancelled: impl FnMut() -> bool,
) -> Result<u32, PrintError>
where
    F: FnMut(u32, f32) -> Result<PageBitmap, PrintError>,
{
    Err(PrintError::Failed(
        "printing is only supported on Windows".to_string(),
    ))
}

/// The size one rendered page is drawn at, in device pixels.
///
/// Fitted to the printable area but never enlarged: blowing a raster up adds no
/// detail, it only softens it, so a page smaller than the sheet prints at its
/// own size and is centred. A page larger than the sheet is scaled down, which
/// it has to be to fit.
fn draw_size(width: f32, height: f32, area_w: f32, area_h: f32) -> (f32, f32) {
    let (fit_w, fit_h) = fit_into(width, height, area_w, area_h);
    if fit_w > width {
        (width, height)
    } else {
        (fit_w, fit_h)
    }
}

/// Ink the printed form marks are drawn in, matching the canvas.
const MARK_INK: [u8; 3] = [28, 28, 28];

/// Paint the form values the reader is showing onto a rendered page.
///
/// PDFium never regenerates a widget's appearance after a value is written, so
/// a ticked checkbox or a chosen radio is drawn by the reader rather than by
/// the document. Printing rasterizes the document, so those marks have to be
/// painted on here — otherwise the paper would show an empty box where the
/// screen shows a tick.
///
/// `page_height_pt` is the unrotated page height: field rectangles are in PDF
/// space, measured from the bottom, and the bitmap is measured from the top.
pub fn paint_form_marks(
    bitmap: &mut TilePixels,
    page: u32,
    page_height_pt: f32,
    scale: f32,
    fields: &[FormFieldInfo],
) {
    for field in fields.iter().filter(|field| field.id.page == page) {
        if !matches!(field.value, FieldValue::Checked(true)) || !field.kind.is_boolean() {
            continue;
        }
        // A field the reader has painted into the page is already in the
        // raster — PDFium rendered it along with the rest of the content — so
        // painting it again here would print two marks on top of each other.
        if field.rendered {
            continue;
        }
        paint_boolean_mark(bitmap, field, page_height_pt, scale);
    }
}

/// Draw one checked checkbox or radio button onto the bitmap.
fn paint_boolean_mark(
    bitmap: &mut TilePixels,
    field: &FormFieldInfo,
    page_height_pt: f32,
    scale: f32,
) {
    let rect = field.rect;
    let x = rect.min_x * scale;
    // PDF space measures up from the bottom; the bitmap measures down from the
    // top, so the field's *top* edge is what becomes `y`.
    let y = (page_height_pt - rect.max_y) * scale;
    let w = (rect.max_x - rect.min_x) * scale;
    let h = (rect.max_y - rect.min_y) * scale;
    if w <= 0.0 || h <= 0.0 || scale <= 0.0 {
        return;
    }

    if field.kind == FormFieldType::RadioButton {
        // The dot the canvas draws, at the same proportion of the widget.
        fill_disc(bitmap, x + w / 2.0, y + h / 2.0, (w.min(h) * 0.26).max(1.0));
        return;
    }

    // A check mark: the two strokes of the tick, matching the canvas.
    let t = (w.min(h) * 0.17).max(1.0);
    stroke_line(
        bitmap,
        x + w * 0.22,
        y + h * 0.52,
        x + w * 0.42,
        y + h * 0.74,
        t,
    );
    stroke_line(
        bitmap,
        x + w * 0.42,
        y + h * 0.74,
        x + w * 0.80,
        y + h * 0.26,
        t,
    );
}

/// Fill a disc of `radius` centred on `(cx, cy)`, clipped to the bitmap.
fn fill_disc(bitmap: &mut TilePixels, cx: f32, cy: f32, radius: f32) {
    let r2 = radius * radius;
    let x0 = (cx - radius).floor().max(0.0) as u32;
    let x1 = ((cx + radius).ceil())
        .min(bitmap.width as f32 - 1.0)
        .max(0.0) as u32;
    let y0 = (cy - radius).floor().max(0.0) as u32;
    let y1 = ((cy + radius).ceil())
        .min(bitmap.height as f32 - 1.0)
        .max(0.0) as u32;

    for py in y0..=y1 {
        for px in x0..=x1 {
            let (dx, dy) = (px as f32 + 0.5 - cx, py as f32 + 0.5 - cy);
            if dx * dx + dy * dy <= r2 {
                put(bitmap, px, py);
            }
        }
    }
}

/// Draw a straight stroke of width `width` between two points.
fn stroke_line(bitmap: &mut TilePixels, x0: f32, y0: f32, x1: f32, y1: f32, width: f32) {
    let steps = ((x1 - x0).abs().max((y1 - y0).abs()) * 2.0).max(1.0) as u32;
    for step in 0..=steps {
        let t = step as f32 / steps as f32;
        fill_disc(bitmap, x0 + (x1 - x0) * t, y0 + (y1 - y0) * t, width / 2.0);
    }
}

/// Write one opaque BGRA pixel, if it is inside the bitmap.
fn put(bitmap: &mut TilePixels, x: u32, y: u32) {
    if x >= bitmap.width || y >= bitmap.height {
        return;
    }
    let index = ((y * bitmap.width + x) * 4) as usize;
    if let Some(pixel) = bitmap.data.get_mut(index..index + 4) {
        pixel.copy_from_slice(&[MARK_INK[2], MARK_INK[1], MARK_INK[0], 255]);
    }
}

/// Scale `width` x `height` to the largest size that fits inside `area`
/// without distorting the aspect ratio.
///
/// Returns `(0, 0)` for an empty source so a degenerate page is skipped rather
/// than stretched into a line.
fn fit_into(width: f32, height: f32, area_w: f32, area_h: f32) -> (f32, f32) {
    if width <= 0.0 || height <= 0.0 || area_w <= 0.0 || area_h <= 0.0 {
        return (0.0, 0.0);
    }
    let scale = (area_w / width).min(area_h / height);
    (width * scale, height * scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_is_fitted_inside_the_printable_area_without_distortion() {
        // A landscape page on a portrait sheet: width-bound, bars top and bottom.
        let (w, h) = fit_into(1000.0, 500.0, 600.0, 800.0);
        assert!((w - 600.0).abs() < 0.01);
        assert!((h - 300.0).abs() < 0.01);
        assert!((w / h - 2.0).abs() < 0.001, "aspect ratio is preserved");

        // A portrait page on the same sheet: height-bound.
        let (w, h) = fit_into(500.0, 1000.0, 600.0, 800.0);
        assert!((w - 400.0).abs() < 0.01);
        assert!((h - 800.0).abs() < 0.01);
    }

    #[test]
    fn a_degenerate_page_fits_to_nothing() {
        assert_eq!(fit_into(0.0, 100.0, 600.0, 800.0), (0.0, 0.0));
        assert_eq!(fit_into(100.0, 100.0, 0.0, 0.0), (0.0, 0.0));
    }

    /// Full printer resolution is kept for an ordinary page — that is the whole
    /// point of printing rather than previewing. A4 at 600 dpi is 8.33x and
    /// about 35 megapixels, well inside the budget.
    #[test]
    fn an_ordinary_page_keeps_its_full_resolution() {
        assert_eq!(clamp_scale(8.33, 595.0, 842.0), 8.33);
    }

    /// A page that would need more than the budget is trimmed to exactly it,
    /// and no further.
    #[test]
    fn an_enormous_page_is_trimmed_to_the_budget() {
        // An A2 poster (1190x1684 pt) at 16x would be 513 megapixels.
        let scale = clamp_scale(16.0, 1190.0, 1684.0);
        assert!(scale < 16.0, "it must be trimmed");

        let megapixels = 1190.0 * 1684.0 * f64::from(scale) * f64::from(scale) / 1_000_000.0;
        assert!(
            (megapixels - MAX_PRINT_MEGAPIXELS).abs() < 0.5,
            "trimmed to the budget, not past it: {megapixels}"
        );
    }

    /// A page smaller than the sheet prints at its own size: enlarging a raster
    /// adds no detail, only softness.
    #[test]
    fn a_small_page_is_never_blown_up() {
        assert_eq!(draw_size(200.0, 300.0, 2550.0, 3300.0), (200.0, 300.0));
    }

    /// A page larger than the sheet is scaled down to fit, aspect intact.
    #[test]
    fn a_large_page_is_scaled_down_to_fit() {
        let (w, h) = draw_size(2000.0, 1000.0, 600.0, 800.0);
        assert!((w - 600.0).abs() < 0.01);
        assert!((h - 300.0).abs() < 0.01);
    }

    /// A checked radio must be painted onto the printed raster. PDFium does not
    /// draw a field's value, so without this the paper shows an empty circle.
    #[test]
    fn a_checked_radio_is_painted_onto_the_printed_page() {
        use pdfreader_core::{FieldId, Rect};

        let mut bitmap = TilePixels {
            width: 200,
            height: 200,
            data: vec![255; 200 * 200 * 4],
        };
        let field = FormFieldInfo {
            id: FieldId::new(0, 0),
            name: "Colour".into(),
            alternate_name: None,
            rect: Rect::from_xywh(40.0, 40.0, 10.0, 10.0),
            kind: FormFieldType::RadioButton,
            value: FieldValue::Checked(true),
            options: Vec::new(),
            read_only: false,
            required: false,
            multiline: false,
            rendered: false,
        };

        // A 10 pt widget at 2x on a 100 pt page: x 80..100 and, because PDF
        // space is measured from the bottom, y 100..120 — centre (90, 110).
        paint_form_marks(&mut bitmap, 0, 100.0, 2.0, std::slice::from_ref(&field));

        let centre = ((110 * 200 + 90) * 4) as usize;
        assert_eq!(&bitmap.data[centre..centre + 3], &[28, 28, 28]);
        assert_eq!(
            &bitmap.data[0..3],
            &[255, 255, 255],
            "the rest is untouched"
        );
    }

    /// An unchecked field, and a field on another page, paint nothing.
    #[test]
    fn only_checked_fields_on_the_printed_page_are_painted() {
        use pdfreader_core::{FieldId, Rect};

        let mut bitmap = TilePixels {
            width: 200,
            height: 200,
            data: vec![255; 200 * 200 * 4],
        };
        let mut field = FormFieldInfo {
            id: FieldId::new(1, 0),
            name: "Colour".into(),
            alternate_name: None,
            rect: Rect::from_xywh(40.0, 40.0, 10.0, 10.0),
            kind: FormFieldType::RadioButton,
            value: FieldValue::Checked(true),
            options: Vec::new(),
            read_only: false,
            required: false,
            multiline: false,
            rendered: false,
        };

        paint_form_marks(&mut bitmap, 0, 100.0, 2.0, std::slice::from_ref(&field));
        assert!(
            bitmap.data.iter().all(|byte| *byte == 255),
            "a field on another page must not be painted"
        );

        field.id = FieldId::new(0, 0);
        field.value = FieldValue::Checked(false);
        paint_form_marks(&mut bitmap, 0, 100.0, 2.0, std::slice::from_ref(&field));
        assert!(
            bitmap.data.iter().all(|byte| *byte == 255),
            "an unchecked field must not be painted"
        );
    }

    /// An unknown printer must be reported, not panic — and the render callback
    /// must never run, because there is nothing to draw on.
    #[cfg(windows)]
    #[test]
    fn an_unknown_printer_is_reported_not_panicked() {
        let result = print_pages(
            "AnsNew No Such Printer",
            "Test",
            &[0],
            |_page, _scale| Err(PrintError::Failed("render must not run".to_string())),
            || false,
        );
        assert!(
            matches!(result, Err(PrintError::Failed(_))),
            "expected a failure, got {result:?}"
        );
    }
}
