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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender, unbounded};
use pdfreader_core::{
    Document, DocumentId, ExportTarget, FormInfo, ImageFormat, PageRange, Rotation, TabId,
};
use pdfreader_pdf::engine::{DocumentHandle, PdfEngine, PdfiumEngine, TilePixels, TileRequest};
use pdfreader_pdf::{DocumentInfo, EngineError, JobProgress};
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
    /// Write a value into one form field of the open document.
    SetField {
        /// Document holding the field.
        doc: DocumentId,
        /// Document to write to.
        handle: DocumentHandle,
        /// Which widget to change.
        id: pdfreader_core::FieldId,
        /// The new value.
        value: pdfreader_core::FieldValue,
    },
    /// Serialise a document, optionally flattening first.
    SaveDocument {
        /// Document to write.
        doc: DocumentId,
        /// Document to serialise.
        handle: DocumentHandle,
        /// Whether to bake annotations and field values into page content.
        flatten: bool,
    },
    /// Enumerate a document's annotations.
    ListAnnotations {
        /// Document the annotations belong to.
        doc: DocumentId,
        /// Document to read from.
        handle: DocumentHandle,
    },
    /// Create an annotation.
    AddAnnotation {
        /// Document to edit.
        doc: DocumentId,
        /// Document to edit.
        handle: DocumentHandle,
        /// Page to draw on.
        page: u32,
        /// What to create and where.
        new: pdfreader_core::NewAnnotation,
    },
    /// Delete an annotation.
    DeleteAnnotation {
        /// Document to edit.
        doc: DocumentId,
        /// Document to edit.
        handle: DocumentHandle,
        /// Which annotation.
        id: pdfreader_core::AnnotationId,
    },
    /// Rewrite an annotation's contents.
    SetAnnotationContents {
        /// Document to edit.
        doc: DocumentId,
        /// Document to edit.
        handle: DocumentHandle,
        /// Which annotation.
        id: pdfreader_core::AnnotationId,
        /// The new text.
        contents: String,
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
    /// Write one page out as an image file.
    ExportPageImage {
        /// Document to read from.
        handle: DocumentHandle,
        /// Zero-based page index.
        page: u32,
        /// Page rotation to bake into the export.
        rotation: Rotation,
        /// Device pixels per PDF point.
        scale: f32,
        /// Image format to write.
        format: ImageFormat,
        /// Where to write it.
        path: PathBuf,
        /// Job this belongs to.
        job: u64,
        /// Set by the shell to stop the job early.
        cancel: Arc<AtomicBool>,
    },
    /// Write every page out as image files in a directory.
    ExportAllPages {
        /// Document to read from.
        handle: DocumentHandle,
        /// Directory to write into; created if it does not exist.
        dir: PathBuf,
        /// Name stem for the files, e.g. `report` → `report-0001.png`.
        stem: String,
        /// Page rotation to bake into the exports.
        rotation: Rotation,
        /// Device pixels per PDF point.
        scale: f32,
        /// Image format to write.
        format: ImageFormat,
        /// Job this belongs to.
        job: u64,
        /// Set by the shell to stop the job early.
        cancel: Arc<AtomicBool>,
    },
    /// Serialise a page range as a standalone PDF.
    ExportPages {
        /// Document to read from.
        handle: DocumentHandle,
        /// Pages to include; `None` means the whole document.
        range: Option<PageRange>,
        /// Where to write the result.
        path: PathBuf,
        /// Why the PDF is being built, which decides what happens next.
        target: ExportTarget,
        /// Job this belongs to.
        job: u64,
        /// Set by the shell to stop the job early.
        cancel: Arc<AtomicBool>,
    },
    /// Concatenate documents into one, in the order given.
    MergePdfs {
        /// Documents to merge.
        sources: Vec<PathBuf>,
        /// Where to write the result.
        output: PathBuf,
        /// Job this belongs to.
        job: u64,
        /// Set by the shell to stop the job early.
        cancel: Arc<AtomicBool>,
    },
    /// Build a PDF with one page per image, each page sized to its image.
    ImagesToPdf {
        /// Images to place, one page each.
        images: Vec<PathBuf>,
        /// Where to write the result.
        output: PathBuf,
        /// Points per pixel for the page size.
        scale: f32,
        /// Job this belongs to.
        job: u64,
        /// Set by the shell to stop the job early.
        cancel: Arc<AtomicBool>,
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
    /// A form field write finished.
    FieldSet {
        /// Document that was written to.
        doc: DocumentId,
        /// Which widget was targeted.
        id: pdfreader_core::FieldId,
        /// Whether the value was written. `false` means the widget vanished,
        /// is read-only, or its type cannot be written.
        written: bool,
        /// Why the write failed, when it did.
        error: Option<String>,
    },
    /// A document was serialised successfully.
    Saved {
        /// Document that was written.
        doc: DocumentId,
        /// The PDF bytes to write to disk.
        bytes: Vec<u8>,
    },
    /// A document could not be serialised.
    SaveFailed {
        /// Document that failed.
        doc: DocumentId,
        /// Human-readable reason.
        reason: String,
    },
    /// The current annotation list, also the answer to every mutation.
    Annotations {
        /// Document the annotations belong to.
        doc: DocumentId,
        /// Every non-widget annotation, in page order.
        annotations: Vec<pdfreader_core::AnnotationInfo>,
    },
    /// An annotation was created; carries its id and the fresh list.
    AnnotationAdded {
        /// Document that was edited.
        doc: DocumentId,
        /// The created annotation.
        id: pdfreader_core::AnnotationId,
        /// Every non-widget annotation after the creation.
        annotations: Vec<pdfreader_core::AnnotationInfo>,
    },
    /// An annotation operation failed.
    AnnotationsFailed {
        /// Document that was being edited.
        doc: DocumentId,
        /// Human-readable reason.
        reason: String,
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
    /// One step of a long-running export or composition job.
    JobProgress {
        /// Job this belongs to.
        job: u64,
        /// Items finished so far.
        done: u32,
        /// Items in total.
        total: u32,
    },
    /// A job finished and wrote its output.
    JobDone {
        /// Job this belongs to.
        job: u64,
        /// What was written, so the shell can report it or hand it to the
        /// printer.
        output: PathBuf,
        /// How many pages or files were produced.
        pages: u32,
        /// Why the output was built.
        target: ExportTarget,
    },
    /// A job stopped because the shell asked it to.
    JobCancelled {
        /// Job this belongs to.
        job: u64,
    },
    /// A job failed.
    JobFailed {
        /// Job this belongs to.
        job: u64,
        /// Human-readable reason.
        reason: String,
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
    /// Hands out identifiers for export and composition jobs.
    next_job: AtomicU64,
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
            next_job: AtomicU64::new(0),
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

    /// Allocate an identifier for a new long-running job.
    ///
    /// Job ids only exist to match progress and completion reports back to the
    /// job that produced them, so a plain counter is enough. Cancellation is
    /// handled by the flag the caller sends alongside the request, not by the
    /// engine thread, which keeps the two concerns independent.
    pub fn new_job(&self) -> u64 {
        self.next_job.fetch_add(1, Ordering::Relaxed) + 1
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
                if responses
                    .send(EngineResponse::FormFields { doc, form })
                    .is_err()
                {
                    return;
                }
            }
            EngineRequest::SetField {
                doc,
                handle,
                id,
                value,
            } => {
                let (written, error) = match engine.set_field_value(handle, id, value) {
                    Ok(true) => (true, None),
                    Ok(false) => (
                        false,
                        Some("the field is read-only or no longer exists".to_string()),
                    ),
                    Err(error) => (false, Some(error.to_string())),
                };
                if responses
                    .send(EngineResponse::FieldSet {
                        doc,
                        id,
                        written,
                        error,
                    })
                    .is_err()
                {
                    return;
                }
            }
            EngineRequest::SaveDocument {
                doc,
                handle,
                flatten,
            } => {
                let response = match engine.save_to_bytes(handle, flatten) {
                    Ok(bytes) => EngineResponse::Saved { doc, bytes },
                    Err(error) => EngineResponse::SaveFailed {
                        doc,
                        reason: error.to_string(),
                    },
                };
                if responses.send(response).is_err() {
                    return;
                }
            }
            EngineRequest::ListAnnotations { doc, handle } => {
                let response = match engine.annotations(handle) {
                    Ok(annotations) => EngineResponse::Annotations { doc, annotations },
                    Err(error) => EngineResponse::AnnotationsFailed {
                        doc,
                        reason: error.to_string(),
                    },
                };
                if responses.send(response).is_err() {
                    return;
                }
            }
            EngineRequest::AddAnnotation {
                doc,
                handle,
                page,
                new,
            } => {
                let response = match engine.add_annotation(handle, page, new) {
                    Ok(id) => match engine.annotations(handle) {
                        Ok(annotations) => EngineResponse::AnnotationAdded {
                            doc,
                            id,
                            annotations,
                        },
                        Err(error) => EngineResponse::AnnotationsFailed {
                            doc,
                            reason: error.to_string(),
                        },
                    },
                    Err(error) => EngineResponse::AnnotationsFailed {
                        doc,
                        reason: error.to_string(),
                    },
                };
                if responses.send(response).is_err() {
                    return;
                }
            }
            EngineRequest::DeleteAnnotation { doc, handle, id } => {
                let response = match engine.delete_annotation(handle, id) {
                    Ok(_) => match engine.annotations(handle) {
                        Ok(annotations) => EngineResponse::Annotations { doc, annotations },
                        Err(error) => EngineResponse::AnnotationsFailed {
                            doc,
                            reason: error.to_string(),
                        },
                    },
                    Err(error) => EngineResponse::AnnotationsFailed {
                        doc,
                        reason: error.to_string(),
                    },
                };
                if responses.send(response).is_err() {
                    return;
                }
            }
            EngineRequest::SetAnnotationContents {
                doc,
                handle,
                id,
                contents,
            } => {
                let response = match engine.set_annotation_contents(handle, id, &contents) {
                    Ok(()) => match engine.annotations(handle) {
                        Ok(annotations) => EngineResponse::Annotations { doc, annotations },
                        Err(error) => EngineResponse::AnnotationsFailed {
                            doc,
                            reason: error.to_string(),
                        },
                    },
                    Err(error) => EngineResponse::AnnotationsFailed {
                        doc,
                        reason: error.to_string(),
                    },
                };
                if responses.send(response).is_err() {
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
            EngineRequest::ExportPageImage {
                handle,
                page,
                rotation,
                scale,
                format,
                path,
                job,
                cancel,
            } => {
                // A single page has nothing to report progress against, so the
                // callback is left alone and only the outcome is sent.
                let done = run_job(
                    job,
                    &cancel,
                    &responses,
                    path.clone(),
                    ExportTarget::SaveFile,
                    |_| {
                        engine
                            .export_page_image(handle, page, rotation, scale, format, &path)
                            .map(|()| 1)
                    },
                );
                if !done {
                    return;
                }
            }
            EngineRequest::ExportAllPages {
                handle,
                dir,
                stem,
                rotation,
                scale,
                format,
                job,
                cancel,
            } => {
                let done = run_job(
                    job,
                    &cancel,
                    &responses,
                    dir.clone(),
                    ExportTarget::SaveFile,
                    |progress| {
                        engine.export_all_pages(
                            handle, &dir, &stem, rotation, scale, format, progress,
                        )
                    },
                );
                if !done {
                    return;
                }
            }
            EngineRequest::ExportPages {
                handle,
                range,
                path,
                target,
                job,
                cancel,
            } => {
                let done = run_job(job, &cancel, &responses, path.clone(), target, |_| {
                    // Serialising is one indivisible step, so the only
                    // meaningful progress report is the one at the end.
                    let bytes = engine.export_pages_to_bytes(handle, range)?;
                    std::fs::write(&path, &bytes).map_err(|error| EngineError::Io {
                        path: path.clone(),
                        reason: error.to_string(),
                    })?;
                    Ok(1)
                });
                if !done {
                    return;
                }
            }
            EngineRequest::MergePdfs {
                sources,
                output,
                job,
                cancel,
            } => {
                let done = run_job(
                    job,
                    &cancel,
                    &responses,
                    output.clone(),
                    ExportTarget::SaveFile,
                    |progress| engine.merge_pdfs(&sources, &output, progress),
                );
                if !done {
                    return;
                }
            }
            EngineRequest::ImagesToPdf {
                images,
                output,
                scale,
                job,
                cancel,
            } => {
                let done = run_job(
                    job,
                    &cancel,
                    &responses,
                    output.clone(),
                    ExportTarget::SaveFile,
                    |progress| engine.images_to_pdf(&images, &output, scale, progress),
                );
                if !done {
                    return;
                }
            }
        }
    }
}

/// Run one long job, reporting progress and exactly one outcome.
///
/// Every job answers exactly once, whatever happens, so the shell can always
/// clear its busy state; a job that reported nothing on the way out would leave
/// a progress indicator running forever.
///
/// Returns `false` when the response channel is gone, which means the UI has
/// shut down and the worker should stop.
fn run_job<F>(
    job: u64,
    cancel: &AtomicBool,
    responses: &Sender<EngineResponse>,
    output: PathBuf,
    target: ExportTarget,
    body: F,
) -> bool
where
    F: FnOnce(&mut JobProgress<'_>) -> Result<u32, EngineError>,
{
    let result = {
        let mut report = |done, total| {
            let _ = responses.send(EngineResponse::JobProgress { job, done, total });
        };
        let mut progress = JobProgress::new(cancel, &mut report);
        body(&mut progress)
    };

    let response = match result {
        Ok(pages) => EngineResponse::JobDone {
            job,
            output,
            pages,
            target,
        },
        // A cancellation is a normal outcome, not a failure: the user asked for
        // it, so it must not surface as an error message.
        Err(EngineError::Cancelled) => EngineResponse::JobCancelled { job },
        Err(error) => EngineResponse::JobFailed {
            job,
            reason: error.to_string(),
        },
    };

    responses.send(response).is_ok()
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
