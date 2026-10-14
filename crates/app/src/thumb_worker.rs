//! Background decoder for the small previews the export window shows.
//!
//! Decoding a photo is far too slow to do while a frame is being drawn — a
//! single 24-megapixel JPEG is tens of milliseconds of work — so the export
//! window's thumbnails are decoded on their own thread and handed back finished.
//! The window then only ever uploads a decoded image, which is cheap, and never
//! stalls a frame waiting on a decoder.
//!
//! The worker is deliberately dumb: it takes paths, decodes them one at a time,
//! and reports either pixels or a failure. Deciding *which* files need a preview
//! is the caller's job, because only the caller knows what is on screen.

use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender, unbounded};

/// Longest edge of a decoded preview, in pixels.
///
/// Comfortably larger than the box the window draws it in, so it stays sharp on
/// a high-density display, and small enough that uploading it costs nothing.
pub const THUMB_EDGE: u32 = 96;

/// A decoded preview, ready to be turned into a texture.
pub struct ThumbImage {
    /// Size in pixels, `[width, height]`.
    pub size: [usize; 2],
    /// Straight (un-premultiplied) RGBA8 pixels, row-major.
    pub rgba: Vec<u8>,
}

/// One finished request.
pub struct ThumbResult {
    /// The file the request was for.
    pub path: PathBuf,
    /// The preview, or `None` when the file could not be decoded.
    pub image: Option<ThumbImage>,
}

/// The decode thread and its two channels.
pub struct ThumbnailWorker {
    /// Paths waiting to be decoded.
    requests: Sender<PathBuf>,
    /// Finished previews, newest last.
    results: Receiver<ThumbResult>,
    /// Kept so the thread is joined when the worker is dropped.
    _thread: JoinHandle<()>,
}

impl ThumbnailWorker {
    /// Start the worker.
    ///
    /// One thread is enough: decoding is a burst of work after the user picks
    /// files, and a second decoder would only make the two compete for the same
    /// cores the renderer is using.
    pub fn new() -> Self {
        let (request_tx, request_rx) = unbounded::<PathBuf>();
        let (result_tx, result_rx) = unbounded::<ThumbResult>();

        let thread = std::thread::Builder::new()
            .name("export-thumbs".to_string())
            .spawn(move || {
                // `recv` ends when every sender is dropped, which is what stops
                // the thread when the window that owns the worker goes away.
                while let Ok(path) = request_rx.recv() {
                    let image = decode(&path);
                    if result_tx.send(ThumbResult { path, image }).is_err() {
                        break;
                    }
                }
            })
            .expect("spawning the export thumbnail thread");

        Self {
            requests: request_tx,
            results: result_rx,
            _thread: thread,
        }
    }

    /// Ask for a preview.
    ///
    /// Requests are queued rather than deduplicated: the caller tracks what it
    /// has already asked for, because only it knows when a file has left the
    /// list and may be asked for again.
    pub fn request(&self, path: PathBuf) {
        let _ = self.requests.send(path);
    }

    /// Take the next finished preview, if one is ready.
    pub fn try_recv(&self) -> Option<ThumbResult> {
        self.results.try_recv().ok()
    }
}

impl Default for ThumbnailWorker {
    fn default() -> Self {
        Self::new()
    }
}

/// Decode one file down to a preview.
///
/// Scaling during decode is not available for every codec, so this decodes at
/// full size and then shrinks — which is why it runs on the worker thread and
/// not in the frame.
fn decode(path: &Path) -> Option<ThumbImage> {
    let decoded = image::open(path).ok()?;
    let scaled = decoded.thumbnail(THUMB_EDGE, THUMB_EDGE);
    let rgba = scaled.to_rgba8();
    let size = [
        usize::try_from(rgba.width()).unwrap_or(0),
        usize::try_from(rgba.height()).unwrap_or(0),
    ];
    Some(ThumbImage {
        size,
        rgba: rgba.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The worker must answer for a real image and report a failure rather than
    /// panicking for something that is not one.
    #[test]
    fn a_real_image_decodes_and_a_bad_path_is_reported() {
        let dir = std::env::temp_dir().join("pdfreader-thumb-test");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let good = dir.join("square.png");
        let bad = dir.join("missing.png");

        // A 200x100 image, so the long edge is what gets clamped to THUMB_EDGE.
        let pixels = image::RgbaImage::from_pixel(200, 100, image::Rgba([10, 20, 30, 255]));
        pixels.save(&good).expect("writing the fixture");

        let worker = ThumbnailWorker::new();
        worker.request(good.clone());
        worker.request(bad.clone());

        let mut got_good = false;
        let mut got_bad = false;
        for _ in 0..100 {
            while let Some(result) = worker.try_recv() {
                if result.path == good {
                    let image = result.image.expect("a decodable image");
                    assert_eq!(image.size, [96, 48], "long edge clamps to {THUMB_EDGE}");
                    assert_eq!(image.rgba.len(), 96 * 48 * 4, "RGBA8, four bytes a pixel");
                    got_good = true;
                } else if result.path == bad {
                    assert!(result.image.is_none(), "a missing file has no preview");
                    got_bad = true;
                }
            }
            if got_good && got_bad {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        assert!(got_good && got_bad, "the worker answered both requests");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
