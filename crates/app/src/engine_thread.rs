//! The single thread that owns the PDF engine.
//!
//! PDFium serialises every call behind a process-global mutex, so there is no
//! point having more than one of these. Everything that touches PDFium goes
//! through here, which also means a PDFium crash or hang can be contained
//! instead of taking the UI down with it.
//!
//! Rasterization requests carry the viewport [`Generation`] they were created
//! in. The engine compares it against the latest generation before doing any
//! work, so a fast scroll discards tiles the user has already scrolled past
//! instead of rendering a growing backlog.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender, unbounded};
use pdfreader_core::{Document, DocumentId, FormInfo, Rotation, TabId};
use pdfreader_pdf::DocumentInfo;
use pdfreader_pdf::engine::{DocumentHandle, PdfEngine, PdfiumEngine, TilePixels, TileRequest};
use pdfreader_render::TileKey;

/// Work the engine thread can be asked to do.
pub enum EngineRequest {
    /// Open a file and report its structure back.
    Open {
        /// Tab waiting for the document.
        tab: TabId,
        /// File to open.
        path: PathBuf,
        /// Passphrase, if the document turns out to be encrypted.
        passphrase: Option<String>,
    },
    /// Release a document.
    Close {
        /// Handle to release.
        handle: DocumentHandle,
    },
    /// Rasterize one tile of one page.
    RenderTile {
        /// Document the tile belongs to.
        doc: DocumentId,
        /// Document to render from.
        handle: DocumentHandle,
        /// Tile identity, echoed back with the pixels.
        key: TileKey,
        /// What to rasterize.
        request: TileRequest,
        /// Viewport generation this request belongs to.
        generation: u64,
    },
    /// Rasterize a whole page at a small scale, for a thumbnail.
    RenderThumbnail {
        /// Document the thumbnail belongs to.
        doc: DocumentId,
        /// Document to render from.
        handle: DocumentHandle,
        /// Zero-based page index.
        page: u32,
        /// Page rotation to bake in.
        rotation: Rotation,
        /// Device pixels per PDF point.
        scale: f32,
        /// Viewport generation this request belongs to.
        generation: u64,
    },
    /// Enumerate a document's AcroForm fields.
    ///
    /// Issued once per document, right after it opens. Most PDFs have no form,
    /// so a negative answer is the common case and costs nothing in the UI.
    LoadForm {
        /// Document the fields belong to.
        doc: DocumentId,
        /// Document to read from.
        handle: DocumentHandle,
    },
    /// Extract and search all pages of a document on the engine thread.
    Search {
        /// Document the results belong to.
        doc: DocumentId,
        /// Document to search.
        handle: DocumentHandle,
        /// Number of pages to inspect.
        page_count: u32,
        /// User-entered query.
        query: String,
        /// Search generation this request belongs to. Starting a new search
        /// (or closing the tab) advances it, so a superseded search is
        /// abandoned mid-extraction instead of starving tile rendering for
        /// the full document.
        generation: u64,
    },
}

/// What the engine thread reports back.
pub enum EngineResponse {
    /// A document was opened successfully.
    Opened {
        /// Tab the document belongs to.
        tab: TabId,
        /// Engine handle for later requests.
        handle: DocumentHandle,
        /// Parsed document ready for the store.
        document: Document,
    },
    /// A document could not be opened.
    Failed {
        /// Tab that failed to load.
        tab: TabId,
        /// Human-readable reason.
        reason: String,
    },
    /// A tile finished rasterizing.
    Tile {
        /// Document the tile belongs to.
        doc: DocumentId,
        /// Rotation the tile was rendered with.
        rotation: Rotation,
        /// Tile identity.
        key: TileKey,
        /// Rasterized pixels (BGRA).
        pixels: TilePixels,
    },
    /// A tile could not be rasterized.
    TileFailed {
        /// Document the tile belongs to.
        doc: DocumentId,
        /// Rotation the tile was requested with.
        rotation: Rotation,
        /// Tile identity.
        key: TileKey,
    },
    /// A thumbnail finished rasterizing.
    Thumbnail {
        /// Document the thumbnail belongs to.
        doc: DocumentId,
        /// Rotation the thumbnail was rendered with.
        rotation: Rotation,
        /// Zero-based page index.
        page: u32,
        /// Rasterized pixels (BGRA).
        pixels: TilePixels,
    },
    /// A thumbnail could not be rasterized.
    ThumbnailFailed {
        /// Document the thumbnail belongs to.
        doc: DocumentId,
        /// Rotation the thumbnail was requested with.
        rotation: Rotation,
        /// Zero-based page index.
        page: u32,
    },
    /// A document's form fields, possibly empty.
    FormFields {
        /// Document the fields belong to.
        doc: DocumentId,
        /// The form, empty when the document has no interactive fields.
        form: FormInfo,
    },
    /// Search results for one query.
    SearchResults {
        /// Document the results belong to.
        doc: DocumentId,
        /// Query that produced these results.
        query: String,
        /// Matching pages in ascending order.
        matches: Vec<pdfreader_search::SearchMatch>,
    },
}

/// Handle to the running engine thread.
pub struct EngineThread {
    /// The sender is optional so Drop can close the channel before joining.
    requests: Option<Sender<EngineRequest>>,
    responses: Receiver<EngineResponse>,
    generation: Arc<AtomicU64>,
    /// Independent generation for searches, so viewport churn (zoom, scroll
    /// invalidations) cannot cancel a running search, and a new search
    /// cancels only searches.
    search_generation: Arc<AtomicU64>,
    worker: Option<JoinHandle<()>>,
}

impl EngineThread {
    /// Bind to PDFium and start the worker.
    ///
    /// Binding happens on the calling thread so a missing library is reported
    /// immediately rather than surfacing as a dead worker.
    ///
    /// # Errors
    /// Returns an error if PDFium cannot be loaded or the thread cannot start.
    pub fn spawn(engine: PdfiumEngine) -> Result<Self, pdfreader_pdf::EngineError> {
        let (request_tx, request_rx) = unbounded::<EngineRequest>();
        let (response_tx, response_rx) = unbounded::<EngineResponse>();

        let generation = Arc::new(AtomicU64::new(0));
        let worker_generation = Arc::clone(&generation);
        let search_generation = Arc::new(AtomicU64::new(0));
        let worker_search_generation = Arc::clone(&search_generation);

        let worker = std::thread::Builder::new()
            .name("pdf-engine".to_string())
            .spawn(move || {
                run(
                    engine,
                    request_rx,
                    response_tx,
                    worker_generation,
                    worker_search_generation,
                );
            })
            .map_err(|e| {
                pdfreader_pdf::EngineError::Library(format!("thread spawn failed: {e}"))
            })?;

        Ok(Self {
            requests: Some(request_tx),
            responses: response_rx,
            generation,
            search_generation,
            worker: Some(worker),
        })
    }

    /// Queue work for the engine.
    pub fn send(&self, request: EngineRequest) {
        // A failure here means the worker is gone; there is nothing useful the
        // UI can do about that, and every later call would fail the same way.
        if let Some(requests) = &self.requests {
            let _ = requests.send(request);
        }
    }

    /// Current viewport generation.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    /// Advance the generation, cancelling queued raster work from before.
    pub fn bump_generation(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Advance the search generation, abandoning any search in flight.
    pub fn bump_search_generation(&self) -> u64 {
        self.search_generation.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Collect any finished work without blocking.
    pub fn poll(&self) -> Vec<EngineResponse> {
        let mut out = Vec::new();
        while let Ok(response) = self.responses.try_recv() {
            out.push(response);
        }
        out
    }
}

impl Drop for EngineThread {
    fn drop(&mut self) {
        // Take (rather than clone) the sender so the worker's receiver closes
        // before join. Dropping a clone leaves the real sender alive and makes
        // shutdown wait forever in the worker's `for request in requests` loop.
        self.requests.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// The worker loop.
fn run(
    mut engine: PdfiumEngine,
    requests: Receiver<EngineRequest>,
    responses: Sender<EngineResponse>,
    generation: Arc<AtomicU64>,
    search_generation: Arc<AtomicU64>,
) {
    for request in requests {
        match request {
            EngineRequest::Open {
                tab,
                path,
                passphrase,
            } => {
                let response = match engine.open(&path, passphrase.as_deref()) {
                    Ok((handle, info)) => EngineResponse::Opened {
                        tab,
                        handle,
                        document: build_document(&path, info),
                    },
                    Err(error) => EngineResponse::Failed {
                        tab,
                        reason: error.to_string(),
                    },
                };
                if responses.send(response).is_err() {
                    return;
                }
            }
            EngineRequest::Close { handle } => engine.close(handle),
            EngineRequest::RenderTile {
                doc,
                handle,
                key,
                request,
                generation: requested,
            } => {
                // Drop work the user has already scrolled past.
                if requested != generation.load(Ordering::Relaxed) {
                    continue;
                }
                let rotation = request.rotation;
                let response = match engine.render_tile(handle, &request) {
                    Ok(pixels) => EngineResponse::Tile {
                        doc,
                        rotation,
                        key,
                        pixels,
                    },
                    Err(_) => EngineResponse::TileFailed { doc, rotation, key },
                };
                if responses.send(response).is_err() {
                    return;
                }
            }
            EngineRequest::RenderThumbnail {
                doc,
                handle,
                page,
                rotation,
                scale,
                generation: requested,
            } => {
                if requested != generation.load(Ordering::Relaxed) {
                    continue;
                }
                // Always answer, even on failure, so the UI can clear its
                // in-flight marker and stop repainting.
                let response = match engine.render_page(handle, page, rotation, scale) {
                    Ok(pixels) => EngineResponse::Thumbnail {
                        doc,
                        rotation,
                        page,
                        pixels,
                    },
                    Err(_) => EngineResponse::ThumbnailFailed {
                        doc,
                        rotation,
                        page,
                    },
                };
                if responses.send(response).is_err() {
                    return;
                }
            }
            EngineRequest::LoadForm { doc, handle } => {
                // Reading the form walks every annotation of every page, which
                // is cheap next to rasterisation but still belongs off the UI
                // thread. A document without a form simply reports an empty one.
                let form = engine.form_fields(handle).unwrap_or_default();
                if responses.send(EngineResponse::FormFields { doc, form }).is_err() {
                    return;
                }
            }
            EngineRequest::Search {
                doc,
                handle,
                page_count,
                query,
                generation: requested,
            } => {
                let mut pages = Vec::new();
                for page in 0..page_count {
                    // Abandon superseded searches between pages: text
                    // extraction is the expensive part, and without this
                    // check a new query would queue behind a full pass over
                    // the document while tiles starve on this same thread.
                    if requested != search_generation.load(Ordering::Relaxed) {
                        continue;
                    }
                    if let Ok(text) = engine.page_text(handle, page) {
                        pages.push((page, text.text()));
                    }
                }
                // A superseded search reports nothing rather than racing the
                // newer one into the results sidebar.
                if requested != search_generation.load(Ordering::Relaxed) {
                    continue;
                }
                let matches = pdfreader_search::search_pages(
                    pages.iter().map(|(page, text)| (*page, text.as_str())),
                    &query,
                );
                if responses
                    .send(EngineResponse::SearchResults {
                        doc,
                        query,
                        matches,
                    })
                    .is_err()
                {
                    return;
                }
            }
        }
    }
}

/// Turn engine metadata into a domain document.
fn build_document(path: &std::path::Path, info: DocumentInfo) -> Document {
    let title = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Untitled".to_string());

    Document {
        id: pdfreader_core::DocumentId::from_raw(0),
        path: path.to_path_buf(),
        title,
        pages: info.pages,
        encrypted: info.encrypted,
        outline: info.outline,
    }
}
