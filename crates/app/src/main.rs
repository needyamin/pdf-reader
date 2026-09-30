//! Application entry point.
//!
//! Owns the store, the engine thread, the tile canvas and the egui shell, and
//! is the only place that executes effects. The UI produces commands; this file
//! makes things happen and feeds results back as more commands.

mod canvas;
mod engine_thread;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::ViewportCommand;
use pdfreader_core::{
    Command, DocumentId, Effect, FieldValue, FormFieldType, SidebarTab, Store, Tab, ThemeId, Tool,
    ViewMode, ZoomMode,
};
use pdfreader_pdf::engine::{DocumentHandle, PdfiumEngine};
use pdfreader_search::SearchMatch;
use pdfreader_ui::{
    ANNOTATION_BAR_HEIGHT, MENUBAR_HEIGHT, STATUSBAR_HEIGHT, TABBAR_HEIGHT, TOOLBAR_HEIGHT,
    TOOLS_RAIL_WIDTH, annotation_bar, comments_panel, forms_panel, menu_bar, outline_tree,
    sidebar_tabs, status_bar, tab_bar, toolbar, tools_rail,
};
use serde::{Deserialize, Serialize};

use canvas::{Canvas, MARGIN, PendingThumb, PendingTile, SCROLLBAR_ALLOWANCE};
use engine_thread::{EngineRequest, EngineResponse, EngineThread};

/// Write PDF bytes to `path` atomically.
///
/// PDFium cannot save over a file it holds open, and a half-written PDF is
/// worse than an unwritten one, so the bytes land in a temp file in the same
/// directory (same filesystem, so the rename is atomic) and are then renamed
/// over the destination.
fn write_pdf_atomically(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(|| std::path::PathBuf::from("."), std::path::Path::to_path_buf);
    let file_name = path
        .file_name()
        .map_or_else(|| std::ffi::OsString::from("document.pdf"), std::ffi::OsString::from);

    let mut temp = directory.join(format!(
        ".{}.tmp-{}",
        file_name.to_string_lossy(),
        std::process::id()
    ));

    // A stale temp file from a crashed run must not be silently truncated into
    // a second writer's path; nudge the name until it is free.
    let mut bump = 0u32;
    while temp.exists() {
        bump += 1;
        temp = directory.join(format!(
            ".{}.tmp-{}-{}",
            file_name.to_string_lossy(),
            std::process::id(),
            bump
        ));
    }

    std::fs::write(&temp, bytes)?;
    match std::fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        // Windows can refuse to clobber a read-only destination; clean up
        // rather than leave a hidden temp file behind.
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            Err(error)
        }
    }
}

fn main() {
    init_tracing();

    let engine = match PdfiumEngine::bind(exe_dir().as_deref()) {
        Ok(engine) => engine,
        Err(error) => {
            // Nothing can be opened without PDFium, so fail loudly and early.
            eprintln!("failed to load PDFium: {error}");
            eprintln!("run `cargo xtask fetch-pdfium` to download it");
            std::process::exit(1);
        }
    };

    let engine_thread = match EngineThread::spawn(engine) {
        Ok(thread) => thread,
        Err(error) => {
            eprintln!("failed to start the engine thread: {error}");
            std::process::exit(1);
        }
    };

    // A path on the command line opens that document, so the app works as a
    // handler for "Open with" and file associations.
    let initial = std::env::args_os().nth(1).map(PathBuf::from);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([820.0, 560.0])
            // Native OS decorations: the standard minimize / maximize / close
            // buttons are always present and behave the way people expect.
            .with_decorations(true)
            .with_maximized(true)
            .with_title("PDF Reader"),
        ..Default::default()
    };

    if let Err(error) = eframe::run_native(
        "PDF Reader",
        options,
        Box::new(move |_cc| Ok(Box::new(App::new(engine_thread, initial)))),
    ) {
        eprintln!("eframe failed: {error}");
        std::process::exit(1);
    }
}

/// Install a tracing subscriber that writes to stderr.
fn init_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .try_init();
}

/// Directory the executable lives in, where PDFium is staged.
fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
}

/// Persisted user preferences.
#[derive(Default, Serialize, Deserialize)]
struct Settings {
    theme: Option<String>,
    sidebar_visible: Option<bool>,
}

/// State for the non-blocking encrypted-document password prompt.
struct PasswordPrompt {
    tab: pdfreader_core::TabId,
    path: PathBuf,
    passphrase: String,
    error: Option<String>,
    submitting: bool,
}

/// Where preferences are stored, per platform convention.
fn config_file() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("pdf-reader").join("settings.json"))
}

/// Load preferences. A missing file is normal; a corrupt one is preserved
/// as `settings.json.corrupt` (instead of being silently overwritten by the
/// next persist) so the user can inspect or recover it, and defaults are
/// used for this run.
fn load_settings() -> Settings {
    let Some(path) = config_file() else {
        return Settings::default();
    };
    let Some(text) = std::fs::read_to_string(&path).ok() else {
        return Settings::default();
    };
    match serde_json::from_str(&text) {
        Ok(settings) => settings,
        Err(error) => {
            tracing::warn!("settings file could not be parsed ({error}); preserving a copy");
            let backup = path.with_extension("json.corrupt");
            if std::fs::rename(&path, &backup).is_err() {
                // Renaming can fail (file locked, permissions); fall back to
                // a copy so the original content still survives the overwrite
                // that the next persist will do.
                let _ = std::fs::copy(&path, &backup);
            }
            Settings::default()
        }
    }
}

/// The eframe application.
struct App {
    /// Application state and the reducer.
    store: Store,
    /// Tile/thumbnail texture cache and canvas drawing.
    canvas: Canvas,
    /// The one thread allowed to touch PDFium.
    engine: EngineThread,
    /// Engine handles for documents the store knows about.
    handles: HashMap<DocumentId, DocumentHandle>,
    /// Receives the result of a native file dialog.
    dialog_rx: Receiver<Option<PathBuf>>,
    /// Sends file dialog results.
    dialog_tx: Sender<Option<PathBuf>>,
    /// Whether a dialog is already open, so we do not stack them.
    dialog_open: bool,
    /// Receives the result of the save-as dialog.
    save_dialog_rx: Receiver<Option<PathBuf>>,
    /// Sends save-as dialog results.
    save_dialog_tx: Sender<Option<PathBuf>>,
    /// Whether a save-as dialog is already open.
    save_dialog_open: bool,
    /// Tab awaiting the unsaved-changes prompt.
    ///
    /// Set by `Effect::ConfirmClose`; the tab is not closed until the user
    /// picks an option in the modal, which dispatches `ConfirmCloseTab`.
    pending_close: Option<pdfreader_core::TabId>,
    /// Tab to close as soon as its save finishes writing.
    close_after_save: Option<pdfreader_core::TabId>,
    /// Whether the last save failed, so the user is not left assuming a write
    /// succeeded when the disk rejected it.
    save_error: Option<String>,
    /// Destination for the save currently in flight, so the serialised bytes
    /// land where the user asked rather than where the file was opened from.
    save_target: Option<PathBuf>,
    /// Whether the window was maximized when fullscreen was entered, so
    /// leaving fullscreen can put it back. Borderless fullscreen from a
    /// maximized window is a known egui-winit conflict on Windows: entering
    /// without un-maximizing first can fail to cover the taskbar, and leaving
    /// without restoring leaves a small floating window.
    pre_fullscreen_maximized: Option<bool>,
    /// Passphrase prompts for encrypted documents, keyed by tab. Several
    /// encrypted documents can fail at once; a single slot would silently
    /// discard the passphrase a user is halfway through typing.
    password_prompts: HashMap<pdfreader_core::TabId, PasswordPrompt>,
    /// Current full-text search query from the toolbar.
    search_query: String,
    /// Pages matching the last completed search.
    search_results: Vec<SearchMatch>,
    /// Document whose search results are displayed.
    search_doc: Option<DocumentId>,
    /// Whether a search is currently running on the engine thread.
    search_in_progress: bool,
    /// When the current search started, so a dead engine thread cannot leave
    /// the spinner up forever.
    search_started: Option<std::time::Instant>,
    /// Last search error, if text extraction failed globally.
    search_error: Option<String>,
    /// Whether to fire `start_search` once the first document has loaded,
    /// driven by the `PDFREADER_INITIAL_SEARCH` debug env var.
    initial_search_pending: bool,
    /// Last theme applied, so we only restyle when it changes.
    applied_theme: Option<ThemeId>,
    /// Size of the canvas panel last frame, for resolving fit modes.
    canvas_size: Option<egui::Vec2>,
    /// A page the user asked to jump to, applied on the next canvas draw.
    scroll_to_page: Option<u32>,
    /// Diagnostics: frames drawn and tiles moved, logged once a second.
    frame_count: u64,
    tiles_requested: u64,
    tiles_delivered: u64,
    last_stats: std::time::Instant,
    /// Debug: when `PDFREADER_SCREENSHOT` is set, capture the window to that
    /// path and exit. Lets the GUI be inspected without a human at the screen.
    screenshot: Option<PathBuf>,
    screenshot_sent: bool,
    frames_total: u64,
}

impl App {
    /// Build the application.
    fn new(engine: EngineThread, initial: Option<PathBuf>) -> Self {
        let (dialog_tx, dialog_rx) = channel();
        let (save_dialog_tx, save_dialog_rx) = channel();
        let settings = load_settings();

        let mut store = Store::new();
        if let Some(theme) = settings.theme.as_deref().and_then(ThemeId::from_name) {
            store.dispatch(Command::SetTheme(theme));
        }
        // The sidebar is hidden by default so the page fills the full width;
        // the user opens it from the toolbar or the View menu.
        if settings.sidebar_visible.unwrap_or(false) {
            store.dispatch(Command::ToggleSidebar);
        }

        let seeded_query = std::env::var("PDFREADER_INITIAL_SEARCH")
            .ok()
            .filter(|q| !q.is_empty());
        if seeded_query.is_some() {
            // Switch to the Search sidebar tab so the initial results are
            // visible without any further interaction.
            store.dispatch(Command::SetSidebarTab(SidebarTab::Search));
        }

        let mut app = Self {
            store,
            canvas: Canvas::new(),
            engine,
            handles: HashMap::new(),
            dialog_rx,
            dialog_tx,
            dialog_open: false,
            save_dialog_rx,
            save_dialog_tx,
            save_dialog_open: false,
            pending_close: None,
            close_after_save: None,
            save_error: None,
            save_target: None,
            pre_fullscreen_maximized: None,
            password_prompts: HashMap::new(),
            search_query: seeded_query.unwrap_or_default(),
            search_results: Vec::new(),
            search_doc: None,
            search_in_progress: false,
            search_started: None,
            search_error: None,
            initial_search_pending: std::env::var_os("PDFREADER_INITIAL_SEARCH").is_some(),
            applied_theme: None,
            canvas_size: None,
            scroll_to_page: None,
            frame_count: 0,
            tiles_requested: 0,
            tiles_delivered: 0,
            last_stats: std::time::Instant::now(),
            screenshot: std::env::var_os("PDFREADER_SCREENSHOT").map(PathBuf::from),
            screenshot_sent: false,
            frames_total: 0,
        };

        // Open whatever path was given, even if it is missing — the engine will
        // report a clear error rather than the UI silently doing nothing.
        if let Some(path) = initial {
            let effects = app.store.dispatch(Command::OpenPath(path));
            app.execute(effects);
        }

        app
    }

    /// Write preferences to disk (best effort).
    fn persist(&self) {
        let Some(path) = config_file() else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let settings = Settings {
            theme: Some(self.store.state().theme.name().to_string()),
            sidebar_visible: Some(self.store.state().sidebar_visible),
        };
        if let Ok(text) = serde_json::to_string_pretty(&settings) {
            let _ = std::fs::write(path, text);
        }
    }

    /// Run effects produced by the reducer.
    fn execute(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::OpenDocument { tab, path } => {
                    self.engine.send(EngineRequest::Open {
                        tab,
                        path,
                        passphrase: None,
                    });
                }
                Effect::CloseDocument { doc } => {
                    if let Some(handle) = self.handles.remove(&doc) {
                        self.engine.send(EngineRequest::Close { handle });
                    }
                    self.canvas.forget_document(doc);
                }
                Effect::InvalidateTiles => {
                    // Cancel raster work for the old viewport. The engine drops
                    // any request it has not started, so those keys would never
                    // get a response — clear them or the UI would repaint
                    // forever waiting for tiles that will not arrive.
                    self.engine.bump_generation();
                    self.canvas.clear_inflight();
                }
                Effect::ScrollToPage { page, .. } => {
                    self.scroll_to_page = Some(page);
                }
                Effect::PersistSession => {
                    self.persist();
                }
                Effect::SetField { doc, id, value } => {
                    if let Some(handle) = self.handles.get(&doc).copied() {
                        self.engine.send(EngineRequest::SetField {
                            doc,
                            handle,
                            id,
                            value,
                        });
                    }
                }
                Effect::SaveDocument { doc, path, flatten } => {
                    if let Some(handle) = self.handles.get(&doc).copied() {
                        self.save_target = Some(path.clone());
                        self.engine.send(EngineRequest::SaveDocument {
                            doc,
                            handle,
                            flatten,
                        });
                    }
                }
                Effect::ConfirmClose { tab } => {
                    self.pending_close = Some(tab);
                }
                Effect::LoadAnnotations { doc } => {
                    if let Some(handle) = self.handles.get(&doc).copied() {
                        self.engine
                            .send(EngineRequest::ListAnnotations { doc, handle });
                    }
                }
                Effect::AddAnnotation { doc, page, new } => {
                    tracing::info!(?doc, page, kind = ?new.kind(), "annotation create requested");
                    if let Some(handle) = self.handles.get(&doc).copied() {
                        self.engine.send(EngineRequest::AddAnnotation {
                            doc,
                            handle,
                            page,
                            new,
                        });
                    }
                    // InvalidateTiles alone cannot refresh the page: tile keys
                    // are unchanged by an annotation edit, so the cached
                    // textures would keep being served. Marking the page's
                    // bitmaps stale re-renders it without a flash.
                    self.canvas.invalidate_page(doc, page);
                }
                Effect::DeleteAnnotation { doc, id } => {
                    if let Some(handle) = self.handles.get(&doc).copied() {
                        self.engine.send(EngineRequest::DeleteAnnotation { doc, handle, id });
                    }
                    self.canvas.invalidate_page(doc, id.page);
                }
                Effect::SetAnnotationContents { doc, id, contents } => {
                    if let Some(handle) = self.handles.get(&doc).copied() {
                        self.engine.send(EngineRequest::SetAnnotationContents {
                            doc,
                            handle,
                            id,
                            contents,
                        });
                    }
                    self.canvas.invalidate_page(doc, id.page);
                }
            }
        }
    }

    /// Per-frame work that does not depend on the drawn layout.
    ///
    /// Returns whether the engine produced anything this frame.
    ///
    /// Safety net for a dead engine thread: a search that has been running
    /// longer than [`SEARCH_TIMEOUT`] is abandoned. Without this, losing the
    /// worker mid-search leaves the sidebar spinner up forever (the response
    /// channel never reopens).
    const SEARCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

    fn pump(&mut self, ctx: &egui::Context) -> bool {
        // Debug screenshot capture: if a screenshot was requested and has now
        // arrived, save it and quit. See `PDFREADER_SCREENSHOT`.
        if let Some(path) = self.screenshot.clone() {
            let image = ctx.input(|input| {
                input.events.iter().find_map(|event| match event {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
            });
            if let Some(image) = image {
                save_screenshot(&path, &image);
                std::process::exit(0);
            }
        }

        let theme_id = self.store.state().theme;
        if self.applied_theme != Some(theme_id) {
            pdfreader_ui::Theme::from_id(theme_id).apply(ctx);
            self.applied_theme = Some(theme_id);
        }

        if let Ok(picked) = self.dialog_rx.try_recv() {
            self.dialog_open = false;
            if let Some(path) = picked {
                let effects = self.store.dispatch(Command::OpenPath(path));
                self.execute(effects);
            }
        }

        if let Ok(picked) = self.save_dialog_rx.try_recv() {
            self.save_dialog_open = false;
            if let Some(path) = picked {
                let effects = self.store.dispatch(Command::SaveDocumentTo(path));
                self.execute(effects);
            }
        }

        // Open any files dropped onto the window.
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        for path in dropped {
            let effects = self.store.dispatch(Command::OpenPath(path));
            self.execute(effects);
        }

        let drained = self.drain_engine(ctx);

        // Debug seed: if `PDFREADER_INITIAL_SEARCH` seeded a query at startup,
        // fire the search and switch to the Search tab once the document has
        // actually loaded. `start_search` is a no-op before then, so waiting
        // for `drain_engine` to populate the handle avoids a wasted pass.
        if self.initial_search_pending
            && !self.search_query.is_empty()
            && self.store.state().active().is_some_and(|t| t.document.is_some())
        {
            self.initial_search_pending = false;
            self.start_search(self.search_query.clone());
        }

        if self.search_in_progress
            && self
                .search_started
                .is_some_and(|started| started.elapsed() > Self::SEARCH_TIMEOUT)
        {
            tracing::warn!("search timed out; the engine may have stopped");
            self.search_in_progress = false;
            self.search_started = None;
            self.search_error = Some("The search took too long and was cancelled.".to_string());
            ctx.request_repaint();
        }

        self.resolve_zoom();
        drained
    }

    /// Resolve fit-width / fit-page into a concrete factor for the current canvas.
    fn resolve_zoom(&mut self) {
        let Some(size) = self.canvas_size else { return };
        let Some(tab) = self.store.state().active() else {
            return;
        };
        let mode = tab.view.zoom_mode;
        if matches!(mode, ZoomMode::Fixed(_)) {
            return;
        }
        let Some(doc) = tab.document.as_ref() else {
            return;
        };
        let Some(geom) = doc
            .page(tab.view.current_page)
            .or_else(|| doc.pages.first().copied())
        else {
            return;
        };
        let (w_pt, h_pt) = geom.oriented(tab.view.rotation);
        if w_pt <= 0.0 || h_pt <= 0.0 {
            return;
        }
        // The vertical scroll bar is always reserved (thin style), so fit-width
        // must leave room for it or the page's right edge hides behind it.
        let avail_w = (size.x - 2.0 * MARGIN - SCROLLBAR_ALLOWANCE).max(1.0);
        let avail_h = (size.y - 2.0 * MARGIN).max(1.0);
        let factor = match mode {
            ZoomMode::FitWidth => avail_w / w_pt,
            _ => (avail_w / w_pt).min(avail_h / h_pt),
        };
        let effects = self.store.dispatch(Command::ResolvedZoom(factor));
        self.execute(effects);
    }

    /// Apply engine replies to the store and caches.
    ///
    /// Returns whether anything arrived, so the caller can keep repainting until
    /// the stream of tiles stops.
    fn drain_engine(&mut self, ctx: &egui::Context) -> bool {
        let responses = self.engine.poll();
        let drained = !responses.is_empty();
        for response in responses {
            match response {
                EngineResponse::Opened {
                    tab,
                    handle,
                    document,
                } => {
                    self.password_prompts.remove(&tab);
                    self.search_results.clear();
                    self.search_doc = None;
                    self.search_in_progress = false;
                    self.search_started = None;
                    let effects = self
                        .store
                        .dispatch(Command::DocumentOpened { tab, document });
                    self.execute(effects);

                    // The store assigns the real id, so read it back rather than
                    // inventing a second id here.
                    let assigned = self
                        .store
                        .state()
                        .tab(tab)
                        .and_then(|t| t.document.as_ref())
                        .map(|d| d.id);
                    if let Some(id) = assigned {
                        self.handles.insert(id, handle);
                        // Read the form and the annotations now that the handle
                        // exists. Sent here rather than as reducer effects
                        // because the engine answers in terms of a document id
                        // the store only assigns during the dispatch above.
                        self.engine
                            .send(EngineRequest::LoadForm { doc: id, handle });
                        self.engine
                            .send(EngineRequest::ListAnnotations { doc: id, handle });
                    } else {
                        // A tab can be closed while PDFium is still opening the
                        // file. The reducer then ignores the late response, so
                        // explicitly release the engine handle instead of
                        // leaking an open document until process exit.
                        self.engine.send(EngineRequest::Close { handle });
                    }
                    // New document: drop any in-flight requests from before.
                    self.engine.bump_generation();
                }
                EngineResponse::FormFields { doc, form } => {
                    let effects = self
                        .store
                        .dispatch(Command::FormFieldsLoaded { doc, form });
                    self.execute(effects);
                }
                EngineResponse::FieldSet {
                    doc,
                    id,
                    written,
                    error,
                } => {
                    if !written {
                        // The reducer already applied the value optimistically;
                        // a failed write means the document will not actually
                        // hold it, so say so rather than let a save silently
                        // drop the user's input.
                        tracing::warn!(
                            "form field {id:?} in document {doc:?} was not written: {}",
                            error.as_deref().unwrap_or("unknown reason")
                        );
                        self.save_error = Some(format!(
                            "Could not write field: {}",
                            error.as_deref().unwrap_or("unknown reason")
                        ));
                    }
                }
                EngineResponse::Saved { doc, bytes } => {
                    if let Some(path) = self.save_target.take() {
                        match write_pdf_atomically(&path, &bytes) {
                            Ok(()) => {
                                self.save_error = None;
                                let effects =
                                    self.store.dispatch(Command::DocumentSaved { doc });
                                self.execute(effects);
                                // A "save and close" prompt finishes here, once
                                // the bytes are actually on disk.
                                if let Some(tab) = self.close_after_save.take() {
                                    let effects =
                                        self.store.dispatch(Command::ConfirmCloseTab(tab));
                                    self.execute(effects);
                                }
                            }
                            Err(error) => {
                                tracing::error!("could not write {}: {error}", path.display());
                                self.save_error =
                                    Some(format!("Could not save {}: {error}", path.display()));
                                // The close is cancelled so the user keeps both
                                // the document and their unsaved edits.
                                self.close_after_save = None;
                            }
                        }
                    }
                }
                EngineResponse::SaveFailed { doc, reason } => {
                    tracing::error!("could not serialise document {doc:?}: {reason}");
                    self.save_error = Some(reason);
                }
                EngineResponse::Annotations { doc, annotations } => {
                    let effects = self
                        .store
                        .dispatch(Command::AnnotationsLoaded { doc, annotations });
                    self.execute(effects);
                }
                EngineResponse::AnnotationsFailed { doc, reason } => {
                    tracing::warn!("annotation operation failed on {doc:?}: {reason}");
                    self.save_error = Some(format!("Annotation failed: {reason}"));
                    // If the list was never read, un-stick the Comments panel.
                    let effects = self
                        .store
                        .dispatch(Command::AnnotationsLoadFailed { doc });
                    self.execute(effects);
                    // An echoed shape whose creation failed must not haunt the
                    // page until the next re-render.
                    self.canvas.cancel_pending(doc);
                }
                EngineResponse::Failed { tab, reason } => {
                    tracing::warn!("could not open document: {reason}");
                    let passphrase_error =
                        matches!(reason.as_str(), "password required" | "incorrect password");
                    if passphrase_error {
                        match self.password_prompts.get_mut(&tab) {
                            Some(prompt) => {
                                // Same tab retrying: surface the error in the
                                // dialog that is already open.
                                prompt.error = Some(reason.clone());
                                prompt.passphrase.clear();
                                prompt.submitting = false;
                            }
                            None => {
                                if let Some(path) =
                                    self.store.state().tab(tab).map(|t| t.path.clone())
                                {
                                    // A different encrypted tab failing gets
                                    // its own prompt; existing prompts are kept.
                                    self.password_prompts.insert(
                                        tab,
                                        PasswordPrompt {
                                            tab,
                                            path,
                                            passphrase: String::new(),
                                            error: Some(reason.clone()),
                                            submitting: false,
                                        },
                                    );
                                }
                            }
                        }
                    }
                    let effects = self.store.dispatch(Command::DocumentFailed { tab, reason });
                    self.execute(effects);
                }
                EngineResponse::Tile {
                    doc,
                    rotation,
                    key,
                    pixels,
                } => {
                    self.tiles_delivered += 1;
                    self.canvas.insert_tile(ctx, doc, rotation, key, &pixels);
                }
                EngineResponse::TileFailed { doc, rotation, key } => {
                    self.canvas.tile_failed(doc, rotation, key);
                }
                EngineResponse::Thumbnail {
                    doc,
                    rotation,
                    page,
                    pixels,
                } => self
                    .canvas
                    .insert_thumbnail(ctx, doc, rotation, page, &pixels),
                EngineResponse::ThumbnailFailed {
                    doc,
                    rotation,
                    page,
                } => {
                    self.canvas.thumbnail_failed(doc, rotation, page);
                }
                EngineResponse::SearchResults {
                    doc,
                    query,
                    matches,
                } => {
                    // A later query or tab switch must not overwrite the
                    // results currently visible in the sidebar.
                    if self.search_doc == Some(doc) && self.search_query == query {
                        self.search_results = matches;
                        self.search_in_progress = false;
                        self.search_started = None;
                    }
                }
            }
        }
        drained
    }

    /// Turn keyboard input into commands.
    fn shortcuts(&self, ctx: &egui::Context) -> Vec<Command> {
        let mut commands = Vec::new();

        // Space and the arrow keys are navigation keys, but egui also uses
        // them to operate focused widgets (a DragValue reacts to arrows,
        // Space clicks buttons). Only navigate when nothing owns the focus.
        // NOTE: read outside `ctx.input` — that closure holds the context
        // write lock, and a nested `ctx.memory` would deadlock on it.
        let free_focus = !ctx.memory(|m| m.focused().is_some());

        ctx.input(|input| {
            let ctrl = input.modifiers.ctrl || input.modifiers.command;

            if ctrl && input.key_pressed(egui::Key::O) {
                commands.push(Command::ShowOpenDialog);
            }
            if ctrl && input.key_pressed(egui::Key::B) {
                commands.push(Command::ToggleSidebar);
            }
            if ctrl && input.key_pressed(egui::Key::Plus) {
                commands.push(Command::ZoomIn);
            }
            // Ctrl+= is what most keyboards actually produce for "zoom in";
            // Ctrl+Plus requires Shift on the common layouts. Accept both.
            if ctrl && input.key_pressed(egui::Key::Equals) {
                commands.push(Command::ZoomIn);
            }
            if ctrl && input.key_pressed(egui::Key::Minus) {
                commands.push(Command::ZoomOut);
            }
            if ctrl && input.key_pressed(egui::Key::Num0) {
                commands.push(Command::ZoomReset);
            }
            if input.key_pressed(egui::Key::F11) {
                commands.push(Command::ToggleFullscreen);
            }
            if free_focus && input.key_pressed(egui::Key::PageDown) {
                commands.push(Command::NextPage);
            }
            if free_focus && input.key_pressed(egui::Key::PageUp) {
                commands.push(Command::PrevPage);
            }
            if free_focus && input.key_pressed(egui::Key::ArrowDown) {
                commands.push(Command::ScrollBy { dx: 0.0, dy: 96.0 });
            }
            if free_focus && input.key_pressed(egui::Key::ArrowUp) {
                commands.push(Command::ScrollBy { dx: 0.0, dy: -96.0 });
            }
            if free_focus && input.key_pressed(egui::Key::Space) {
                if input.modifiers.shift {
                    commands.push(Command::PrevPage);
                } else {
                    commands.push(Command::NextPage);
                }
            }
            // In single-page mode horizontal arrows have no scrolling job, so
            // they flip pages like the left/right arrows of book readers do.
            let single = self
                .store
                .state()
                .active()
                .is_some_and(|t| matches!(t.view.mode, ViewMode::Single));
            if single && free_focus {
                if input.key_pressed(egui::Key::ArrowRight) {
                    commands.push(Command::NextPage);
                }
                if input.key_pressed(egui::Key::ArrowLeft) {
                    commands.push(Command::PrevPage);
                }
            }
            if free_focus && input.key_pressed(egui::Key::Home) {
                commands.push(Command::GoToPage(0));
            }
            if free_focus && input.key_pressed(egui::Key::End) {
                commands.push(Command::GoToPage(u32::MAX));
            }
            if ctrl && input.key_pressed(egui::Key::W) {
                if let Some(id) = self.store.state().active_tab {
                    commands.push(Command::RequestCloseTab(id));
                }
            }
            if input.key_pressed(egui::Key::Escape) {
                if self.store.state().fullscreen {
                    commands.push(Command::ToggleFullscreen);
                } else if self.store.state().tool.annotation_kind().is_some() {
                    commands.push(Command::SetTool(Tool::Select));
                }
            }
            if ctrl && input.key_pressed(egui::Key::S) {
                if self.store.state().active().is_some_and(|tab| tab.document.is_some()) {
                    if input.modifiers.shift {
                        commands.push(Command::SaveDocumentAs);
                    } else {
                        commands.push(Command::SaveDocument);
                    }
                }
            }
        });

        commands
    }

    /// Open a native file dialog on a worker thread.
    ///
    /// The dialog blocks, and blocking the UI thread is exactly what this
    /// architecture exists to avoid, so it runs elsewhere and reports back
    /// through a channel.
    fn show_open_dialog(&mut self) {
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;

        let sender = self.dialog_tx.clone();

        if let Err(error) = std::thread::Builder::new()
            .name("file-dialog".to_string())
            .spawn(move || {
                let picked = rfd::FileDialog::new()
                    .add_filter("PDF document", &["pdf"])
                    .set_title("Open PDF")
                    .pick_file();

                let _ = sender.send(picked);
            })
        {
            tracing::error!("could not open a file dialog thread: {error}");
            self.dialog_open = false;
        }
    }

    /// Open a native save dialog on a worker thread.
    ///
    /// Mirrors `show_open_dialog`: the dialog blocks, so it runs elsewhere and
    /// reports back through a channel.
    fn show_save_dialog(&mut self) {
        if self.save_dialog_open {
            return;
        }
        self.save_dialog_open = true;

        let sender = self.save_dialog_tx.clone();
        let default_name = self
            .store
            .state()
            .active()
            .map(|tab| tab.path.file_name().map(|n| n.to_string_lossy().to_string()))
            .flatten();

        if let Err(error) = std::thread::Builder::new()
            .name("save-dialog".to_string())
            .spawn(move || {
                let mut dialog = rfd::FileDialog::new()
                    .add_filter("PDF document", &["pdf"])
                    .set_title("Save PDF as");
                if let Some(name) = default_name {
                    dialog = dialog.set_file_name(name);
                }
                let _ = sender.send(dialog.save_file());
            })
        {
            tracing::error!("could not open a save dialog thread: {error}");
            self.save_dialog_open = false;
        }
    }

    /// Forward canvas tile requests to the engine thread.
    fn request_tiles(&mut self, tiles: Vec<PendingTile>) {
        let generation = self.engine.generation();
        self.tiles_requested += tiles.len() as u64;
        for tile in tiles {
            if let Some(handle) = self.handles.get(&tile.doc).copied() {
                self.engine.send(EngineRequest::RenderTile {
                    doc: tile.doc,
                    handle,
                    key: tile.key,
                    request: tile.request,
                    generation,
                });
            }
        }
    }

    /// Forward sidebar thumbnail requests to the engine thread.
    fn request_thumbnails(&mut self, thumbs: Vec<PendingThumb>) {
        let generation = self.engine.generation();
        for thumb in thumbs {
            if let Some(handle) = self.handles.get(&thumb.doc).copied() {
                self.engine.send(EngineRequest::RenderThumbnail {
                    doc: thumb.doc,
                    handle,
                    page: thumb.page,
                    rotation: thumb.rotation,
                    scale: thumb.scale,
                    generation,
                });
            }
        }
    }

    /// Start a full-document search without blocking the UI thread.
    ///
    /// A search already in flight is superseded, not duplicated: the engine's
    /// search generation advances so the old pass is abandoned between pages.
    fn start_search(&mut self, query: String) {
        self.search_query = query.trim().to_owned();
        self.search_results.clear();
        self.search_error = None;
        let Some((doc_id, page_count)) = self
            .store
            .state()
            .active()
            .and_then(|tab| tab.document.as_ref().map(|doc| (doc.id, doc.page_count())))
        else {
            self.search_doc = None;
            self.search_in_progress = false;
            self.search_started = None;
            return;
        };
        // Results are useful only when the results panel is visible; opening
        // Search also makes the action discoverable after pressing Enter.
        self.store
            .dispatch(Command::SetSidebarTab(SidebarTab::Search));
        if self.search_query.is_empty() {
            self.search_doc = Some(doc_id);
            self.search_in_progress = false;
            self.search_started = None;
            return;
        }
        let Some(handle) = self.handles.get(&doc_id).copied() else {
            self.search_doc = Some(doc_id);
            self.search_in_progress = false;
            self.search_started = None;
            self.search_error = Some("The document is still loading.".to_string());
            return;
        };
        // Supersede any earlier search: this abandons its extraction pass
        // mid-document on the engine thread instead of queueing a second
        // full-text pass behind it.
        let generation = self.engine.bump_search_generation();
        self.search_doc = Some(doc_id);
        self.search_in_progress = true;
        self.search_started = Some(std::time::Instant::now());
        self.engine.send(EngineRequest::Search {
            doc: doc_id,
            handle,
            page_count,
            query: self.search_query.clone(),
            generation,
        });
    }

    /// Dispatch commands and run their effects.
    fn apply(&mut self, ctx: &egui::Context, commands: Vec<Command>) {
        for command in commands {
            if let Command::Search(query) = command {
                self.start_search(query);
                continue;
            }
            if matches!(command, Command::ShowOpenDialog) {
                self.show_open_dialog();
                continue;
            }
            if matches!(command, Command::SaveDocumentAs) {
                self.show_save_dialog();
                continue;
            }
            let is_scroll_by = matches!(command, Command::ScrollBy { .. });
            let fullscreen = matches!(command, Command::ToggleFullscreen);
            let effects = self.store.dispatch(command);
            if is_scroll_by {
                if let Some(view) = self.store.state().active().map(|tab| tab.view) {
                    self.canvas
                        .request_scroll_to(egui::vec2(view.scroll_x, view.scroll_y));
                }
            }
            if fullscreen {
                let entering = self.store.state().fullscreen;
                if entering {
                    // Un-maximize first: a maximized window going borderless
                    // fullscreen is the known "not truly fullscreen" failure.
                    let maximized = ctx.input(|input| input.viewport().maximized);
                    self.pre_fullscreen_maximized = maximized;
                    if maximized == Some(true) {
                        ctx.send_viewport_cmd(ViewportCommand::Maximized(false));
                    }
                    ctx.send_viewport_cmd(ViewportCommand::Fullscreen(true));
                } else {
                    ctx.send_viewport_cmd(ViewportCommand::Fullscreen(false));
                    if self.pre_fullscreen_maximized == Some(true) {
                        ctx.send_viewport_cmd(ViewportCommand::Maximized(true));
                    }
                    self.pre_fullscreen_maximized = None;
                }
                // The commands reach winit after this frame; log what the
                // viewport reported before them so a follow-up report is a
                // log line rather than a screenshot.
                tracing::info!(
                    entering,
                    before_fullscreen = ?ctx.input(|input| input.viewport().fullscreen),
                    before_maximized = ?ctx.input(|input| input.viewport().maximized),
                    "fullscreen toggled"
                );
                ctx.request_repaint();
            }
            self.execute(effects);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let dpr = ctx.pixels_per_point();

        let drained = self.pump(&ctx);

        let theme_id = self.store.state().theme;
        let palette = pdfreader_ui::Theme::from_id(theme_id).palette;

        let mut commands = self.shortcuts(&ctx);
        let mut tiles: Vec<PendingTile> = Vec::new();
        let mut thumbs: Vec<PendingThumb> = Vec::new();
        let mut canvas_size = self.canvas_size;

        // Draw every panel. Fields are borrowed separately so the closures that
        // need the canvas do not conflict with those that read the store.
        {
            let App {
                store,
                canvas,
                scroll_to_page,
                search_query,
                search_results,
                search_doc,
                search_in_progress,
                search_error,
                ..
            } = &mut *self;

            let frame = |fill: egui::Color32| egui::Frame::new().fill(fill);

            egui::Panel::top("menubar")
                .exact_size(MENUBAR_HEIGHT)
                .frame(frame(palette.panel_bg))
                .show(ui, |ui| {
                    commands.extend(menu_bar(ui, store.state(), &palette));
                });

            // Acrobat Pro DC places the document tab strip directly under the
            // menu bar, above the toolbar. egui consumes top panels in
            // declaration order, so the tabbar is declared first.
            if !store.state().tabs.is_empty() {
                egui::Panel::top("tabbar")
                    .exact_size(TABBAR_HEIGHT)
                    .frame(frame(palette.panel_bg))
                    .show(ui, |ui| {
                        commands.extend(tab_bar(ui, store.state(), &palette));
                    });
            }

            egui::Panel::top("toolbar")
                .exact_size(TOOLBAR_HEIGHT)
                .frame(frame(palette.panel_bg))
                .show(ui, |ui| {
                    // The hand tool is view state, not document state: it is a
                    // mode of the canvas, so the app owns it and the toolbar
                    // only toggles it.
                    let mut pan_tool = canvas.pan_tool();
                    commands.extend(toolbar(
                        ui,
                        store.state(),
                        &palette,
                        search_query,
                        &mut pan_tool,
                    ));
                    if pan_tool != canvas.pan_tool() {
                        canvas.set_pan_tool(pan_tool);
                    }
                });

            if store.state().sidebar_visible {
                egui::Panel::left("sidebar")
                    .default_size(240.0)
                    .resizable(true)
                    .frame(frame(palette.panel_bg))
                    .show(ui, |ui| {
                        commands.extend(sidebar_tabs(ui, store.state(), &palette));
                        ui.separator();

                        let sidebar_tab = store.state().sidebar_tab;
                        let active = store.state().active().cloned();
                        match active.and_then(|t| t.document.map(|d| (t.view, d))) {
                            Some((view, doc)) => match sidebar_tab {
                                SidebarTab::Thumbnails => {
                                    let result = canvas.draw_thumbnails(
                                        ui,
                                        &doc,
                                        &view,
                                        dpr,
                                        palette.accent,
                                        palette.text_dim,
                                    );
                                    if let Some(page) = result.clicked {
                                        commands.push(Command::GoToPage(page));
                                    }
                                    thumbs.extend(result.requests);
                                }
                                SidebarTab::Outline => {
                                    commands.extend(outline_tree(ui, &palette, &doc.outline));
                                }
                                SidebarTab::Search => {
                                    draw_search_results(
                                        ui,
                                        &palette,
                                        search_query,
                                        search_results,
                                        *search_in_progress,
                                        *search_doc == Some(doc.id),
                                        search_error.as_deref(),
                                        &mut commands,
                                    );
                                }
                                SidebarTab::Comments => {
                                    match store
                                        .state()
                                        .active()
                                        .and_then(|t| t.annotations.as_ref())
                                    {
                                        Some(list) => commands.extend(comments_panel(
                                            ui,
                                            &palette,
                                            doc.id,
                                            list,
                                            view.selected_annotation,
                                        )),
                                        None => {
                                            ui.add_space(6.0);
                                            ui.label(
                                                egui::RichText::new("Reading annotations…")
                                                    .color(palette.text_dim)
                                                    .size(12.0),
                                            );
                                        }
                                    }
                                }
                                SidebarTab::Forms => {
                                    // The tab only appears in the strip once a
                                    // form exists, so an unread form here means
                                    // the engine has not answered yet.
                                    match store.state().active().and_then(|t| t.form.as_ref()) {
                                        Some(form) => commands.extend(forms_panel(
                                            ui,
                                            &palette,
                                            doc.id,
                                            form,
                                            view.selected_field,
                                        )),
                                        None => {
                                            ui.add_space(6.0);
                                            ui.label(
                                                egui::RichText::new("Reading form fields…")
                                                    .color(palette.text_dim)
                                                    .size(12.0),
                                            );
                                        }
                                    }
                                }
                                other => {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{} is not available yet.",
                                            sidebar_name(other)
                                        ))
                                        .color(palette.text_dim)
                                        .size(12.0),
                                    );
                                }
                            },
                            None => {
                                ui.add_space(6.0);
                                ui.label(
                                    egui::RichText::new("Open a PDF to get started.")
                                        .color(palette.text_dim)
                                        .size(12.0),
                                );
                            }
                        }
                    });
            }

            // The status bar is a bottom panel, so it must be added before the
            // central panel: egui requires `CentralPanel` to be added last.
            egui::Panel::bottom("statusbar")
                .exact_size(STATUSBAR_HEIGHT)
                .frame(frame(palette.panel_bg))
                .show(ui, |ui| {
                    status_bar(ui, store.state(), &palette);
                });

            // Right-hand tools rail, in the spirit of Acrobat Pro DC. Side
            // panels are consumed before the central panel.
            egui::Panel::right("tools-rail")
                .exact_size(TOOLS_RAIL_WIDTH)
                .resizable(false)
                .frame(frame(palette.panel_bg))
                .show(ui, |ui| {
                    commands.extend(tools_rail(ui, store.state(), &palette));
                });

            egui::CentralPanel::default()
                .frame(frame(palette.window_bg))
                .show(ui, |ui| {
                    // The annotation tool strip sits above the page; it only
                    // exists while a document is open, so an empty launch shows
                    // just the welcome card.
                    if store.state().active().is_some_and(|tab| tab.document.is_some()) {
                        commands.extend(annotation_bar(ui, &palette, store.state()));
                        ui.separator();
                    }
                    canvas_size = Some(ui.available_size());

                    let state = store.state();
                    match state.active() {
                        None => empty_state(ui, &palette, &mut commands),
                        Some(tab) if tab.loading => loading_state(ui, &palette),
                        Some(tab) if tab.error.is_some() => {
                            error_state(ui, &palette, tab, &mut commands);
                        }
                        Some(tab) => {
                            if let Some(doc) = tab.document.as_ref() {
                                // Field drafts belong to one document; a draft
                                // typed into the previous tab must never leak
                                // into another that reuses the same field ids.
                                if canvas.active_document() != Some(doc.id) {
                                    canvas.forget_edits();
                                }
                                // A programmatic jump scrolls inside `draw`, but
                                // the centre page reported by this frame is still
                                // the pre-jump one. Skip the update or it would
                                // revert `current_page` for a frame (the page
                                // indicator flickers back, and the jump can fight
                                // itself on slow frames).
                                let jumping = scroll_to_page.is_some();
                                let pending = canvas.draw(
                                    ui,
                                    doc,
                                    &tab.view,
                                    tab.view.zoom,
                                    dpr,
                                    palette.window_bg,
                                    palette.page_shadow,
                                    scroll_to_page.take(),
                                    tab.form.as_ref(),
                                    tab.view.selected_field,
                                    palette.accent,
                                    store.state().tool,
                                );
                                tiles.extend(pending);

                                // Resolve a click on the page into a form field
                                // selection — but only in Select mode, where a
                                // click cannot mean "place an annotation".
                                if let Some(point) = canvas.take_click().filter(|_| {
                                    store.state().tool.annotation_kind().is_none()
                                }) {
                                    let hit = pdfreader_render::hit_page(
                                        canvas.page_spaces(),
                                        point,
                                    )
                                    .and_then(|space| {
                                        let (x, y) = space.to_page(point);
                                        tab.form.as_ref()?.fields.iter().find(|f| {
                                            f.id.page == space.index && f.rect.contains(x, y)
                                        })
                                    })
                                    .cloned();
                                    commands
                                        .push(Command::SelectFormField(hit.as_ref().map(|f| f.id)));

                                    // Boolean widgets toggle on click, the way
                                    // they do in every PDF viewer — PDFium has
                                    // already drawn their current appearance,
                                    // so the click is the whole interaction.
                                    if let Some(field) = hit {
                                        if field.is_editable() {
                                            match field.kind {
                                                FormFieldType::CheckBox => {
                                                    commands.push(Command::SetFormFieldValue {
                                                        id: field.id,
                                                        value: FieldValue::Checked(
                                                            field.value != FieldValue::Checked(true),
                                                        ),
                                                    });
                                                }
                                                FormFieldType::RadioButton
                                                    if field.value != FieldValue::Checked(true) =>
                                                {
                                                    commands.push(Command::SetFormFieldValue {
                                                        id: field.id,
                                                        value: FieldValue::Checked(true),
                                                    });
                                                }
                                                _ => {}
                                            }
                                        }
                                    }
                                }
                                commands.extend(canvas.take_edits());

                                // Keep both the page indicator and the
                                // persisted viewport offset in step with
                                // wheel/trackpad scrolling.
                                if !jumping {
                                    let center = canvas.center_page();
                                    let offset = canvas.scroll_offset();
                                    if center != tab.view.current_page
                                        || (offset.x - tab.view.scroll_x).abs() > 0.5
                                        || (offset.y - tab.view.scroll_y).abs() > 0.5
                                    {
                                        commands.push(Command::ViewportChanged {
                                            scroll_x: canvas.scroll_offset().x,
                                            scroll_y: canvas.scroll_offset().y,
                                            current_page: center,
                                        });
                                    }
                                }
                            }
                        }
                    }
                });
        }

        // Draw one modal per pending passphrase prompt. They are keyed by
        // tab, so two encrypted documents failing at the same time each keep
        // their own dialog and their own typed text.
        let mut submit_actions: Vec<(pdfreader_core::TabId, PathBuf, String)> = Vec::new();
        let mut cancelled: Vec<pdfreader_core::TabId> = Vec::new();
        for (tab, prompt) in &mut self.password_prompts {
            match password_dialog(&ctx, &palette, prompt) {
                Some(PasswordAction::Submit {
                    tab: submit_tab,
                    path,
                    passphrase,
                }) => {
                    prompt.error = None;
                    prompt.submitting = true;
                    submit_actions.push((submit_tab, path, passphrase));
                }
                Some(PasswordAction::Cancel) => cancelled.push(*tab),
                None => {}
            }
        }
        for (tab, path, passphrase) in submit_actions {
            self.engine.send(EngineRequest::Open {
                tab,
                path,
                passphrase: Some(passphrase),
            });
        }
        for tab in cancelled {
            self.password_prompts.remove(&tab);
        }

        // The unsaved-changes prompt. Saving first means the close only
        // completes once the engine has serialised and the bytes are on disk,
        // which is why `close_after_save` exists rather than closing inline.
        if let Some(tab) = self.pending_close {
            let path = self
                .store
                .state()
                .tab(tab)
                .map(|t| t.path.clone())
                .unwrap_or_default();
            match confirm_close_dialog(&ctx, &palette, &path) {
                Some(CloseChoice::SaveAndClose) => {
                    self.pending_close = None;
                    if let Some(doc) = self
                        .store
                        .state()
                        .tab(tab)
                        .and_then(|t| t.document.as_ref())
                        .map(|d| d.id)
                    {
                        if let Some(handle) = self.handles.get(&doc).copied() {
                            self.save_target = Some(path.clone());
                            self.close_after_save = Some(tab);
                            self.engine.send(EngineRequest::SaveDocument {
                                doc,
                                handle,
                                flatten: false,
                            });
                        }
                    }
                }
                Some(CloseChoice::Discard) => {
                    self.pending_close = None;
                    let effects = self.store.dispatch(Command::ConfirmCloseTab(tab));
                    self.execute(effects);
                }
                Some(CloseChoice::Cancel) => {
                    self.pending_close = None;
                }
                None => {}
            }
        }

        // A failed save or field write surfaces here so "nothing happened" is
        // never the only feedback. Click anywhere on it to dismiss.
        if let Some(error) = self.save_error.clone() {
            let response = egui::Area::new(egui::Id::new("save-error"))
                .anchor(egui::Align2::CENTER_BOTTOM, egui::Vec2::new(0.0, -36.0))
                .show(&ctx, |ui| {
                    egui::Frame::new()
                        .fill(palette.panel_bg)
                        .stroke(egui::Stroke::new(1.0, palette.danger))
                        .corner_radius(3.0)
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(format!("{error}  (click to dismiss)"))
                                    .color(palette.text)
                                    .size(12.0),
                            );
                        });
                    ui.interact(
                        ui.min_rect(),
                        egui::Id::new("save-error-dismiss"),
                        egui::Sense::click(),
                    )
                })
                .inner;
            if response.clicked() {
                self.save_error = None;
            }
        }

        self.canvas_size = canvas_size;

        self.request_tiles(tiles);
        self.request_thumbnails(thumbs);
        self.apply(&ctx, commands);

        // Keep the frame loop alive while tiles/thumbnails are still streaming
        // in, so pages fill in promptly instead of waiting for the next input
        // event (which would look like a freeze). Goes idle at 0% CPU otherwise.
        if drained || self.canvas.has_pending() {
            ctx.request_repaint();
        }

        self.frames_total += 1;
        if self.screenshot.is_some() {
            if !self.screenshot_sent && self.frames_total >= 20 {
                ctx.send_viewport_cmd(ViewportCommand::Screenshot(egui::UserData::default()));
                self.screenshot_sent = true;
            }
            // Keep frames coming until the screenshot event arrives, otherwise
            // an idle UI would never deliver it.
            ctx.request_repaint();
        }

        self.frame_count += 1;
        let elapsed = self.last_stats.elapsed();
        if elapsed >= std::time::Duration::from_secs(1) {
            let causes: Vec<String> = ctx
                .repaint_causes()
                .iter()
                .map(ToString::to_string)
                .collect();
            // Opt-in diagnostics: RUST_LOG=stats=debug
            tracing::debug!(
                target: "stats",
                fps = self.frame_count,
                tiles_requested = self.tiles_requested,
                tiles_delivered = self.tiles_delivered,
                inflight = self.canvas.inflight_count(),
                cached = self.canvas.cached_tiles(),
                pending = self.canvas.has_pending(),
                zoom = self.store.state().active().map_or(0.0, |t| t.view.zoom),
                page = self.store.state().active().map_or(0, |t| t.view.current_page),
                causes = ?causes,
                "ui stats"
            );
            self.frame_count = 0;
            self.tiles_requested = 0;
            self.tiles_delivered = 0;
            self.last_stats = std::time::Instant::now();
        }
    }
}

/// Sidebar tab display name.
const fn sidebar_name(tab: SidebarTab) -> &'static str {
    match tab {
        SidebarTab::Thumbnails => "Thumbnails",
        SidebarTab::Outline => "Outline",
        SidebarTab::Search => "Search",
        SidebarTab::Bookmarks => "Bookmarks",
        SidebarTab::Comments => "Comments",
        SidebarTab::Forms => "Form fields",
    }
}

/// Build a one-line text job for a result excerpt with every occurrence of
/// the query tinted, so a hit can be spotted without reading the whole line.
///
/// Falls back to plain text when the query is empty, or when lowercasing
/// changes the character count (a few scripts expand), because then character
/// offsets into the excerpt no longer line up with the lowercased copy.
fn highlighted_snippet(
    snippet: &str,
    query: &str,
    palette: &pdfreader_ui::Palette,
) -> egui::text::LayoutJob {
    let font = egui::FontId::proportional(11.0);
    let plain = egui::TextFormat {
        font_id: font.clone(),
        color: palette.text_dim,
        ..Default::default()
    };
    let mut job = egui::text::LayoutJob::default();
    job.wrap = egui::text::TextWrapping {
        max_rows: 2,
        break_anywhere: true,
        ..Default::default()
    };

    let needle: Vec<char> = query.trim().to_lowercase().chars().collect();
    let chars: Vec<char> = snippet.chars().collect();
    let lower: Vec<char> = snippet.to_lowercase().chars().collect();
    if snippet.is_empty() || needle.is_empty() || lower.len() != chars.len() {
        job.append(snippet, 0.0, plain);
        return job;
    }

    let hit = egui::TextFormat {
        font_id: font,
        color: palette.text,
        background: palette.accent_soft,
        ..Default::default()
    };

    let mut cursor = 0usize;
    let mut plain_start = 0usize;
    while cursor + needle.len() <= lower.len() {
        if lower[cursor..cursor + needle.len()] == needle[..] {
            if cursor > plain_start {
                let text: String = chars[plain_start..cursor].iter().collect();
                job.append(&text, 0.0, plain.clone());
            }
            let text: String = chars[cursor..cursor + needle.len()].iter().collect();
            job.append(&text, 0.0, hit.clone());
            cursor += needle.len();
            plain_start = cursor;
        } else {
            cursor += 1;
        }
    }
    if plain_start < chars.len() {
        let text: String = chars[plain_start..].iter().collect();
        job.append(&text, 0.0, plain);
    }
    job
}

/// Draw one result as a card: page badge, excerpt with the query tinted, and
/// the hit count. Clicking jumps to the page.
fn search_result_card(
    ui: &mut egui::Ui,
    palette: &pdfreader_ui::Palette,
    query: &str,
    result: &SearchMatch,
) -> egui::Response {
    // Allocate first so the hover state is known before anything is painted.
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 48.0), egui::Sense::click());
    let hovered = response.hovered();

    let painter = ui.painter();
    painter.rect_filled(
        rect,
        6.0,
        if hovered {
            palette.surface_hover
        } else {
            palette.surface
        },
    );
    painter.rect_stroke(
        rect,
        6.0,
        egui::Stroke::new(
            1.0,
            if hovered {
                palette.accent_soft
            } else {
                palette.border
            },
        ),
        egui::StrokeKind::Inside,
    );

    // The parent already advanced past `rect`, so the child only has to draw
    // inside it; no extra space accounting is needed.
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(rect.shrink2(egui::vec2(8.0, 6.0))),
        |ui| {
            ui.horizontal(|ui| {
                // Page badge: a fixed-width chip so the excerpts line up in a
                // column no matter how many digits the page number has.
                let (badge, _) =
                    ui.allocate_exact_size(egui::vec2(30.0, 18.0), egui::Sense::hover());
                ui.painter().rect_filled(badge, 4.0, palette.accent_soft);
                ui.painter().text(
                    badge.center(),
                    egui::Align2::CENTER_CENTER,
                    format!("{}", result.page + 1),
                    egui::FontId::proportional(11.0),
                    palette.accent,
                );
                ui.add_space(4.0);

                ui.vertical(|ui| {
                    ui.add(egui::Label::new(highlighted_snippet(
                        &result.snippet,
                        query,
                        palette,
                    )));
                    ui.label(
                        egui::RichText::new(format!(
                            "Page {} · {} match{}",
                            result.page + 1,
                            result.count,
                            if result.count == 1 { "" } else { "es" }
                        ))
                        .color(palette.text_dim)
                        .size(10.0),
                    );
                });
            });
        },
    );

    response
}

/// Draw the Find panel: a query field plus the match list.
fn draw_search_results(
    ui: &mut egui::Ui,
    palette: &pdfreader_ui::Palette,
    query: &mut String,
    results: &[SearchMatch],
    in_progress: bool,
    current_document: bool,
    error: Option<&str>,
    commands: &mut Vec<Command>,
) {
    ui.add_space(2.0);

    // The query field lives here as well as in the toolbar: the toolbar one is
    // easy to miss once you are reading the results, and re-running a search
    // should not mean hunting for the box again.
    // Match the toolbar search box: a contained, padded field rather than a
    // full-bleed bar, so it reads as part of the chrome instead of oversized.
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let clear_w = 26.0;
                let input_w = (ui.available_width() - clear_w).max(60.0).min(168.0);
                let response = ui.add_sized(
                    egui::vec2(input_w, 30.0),
                    egui::TextEdit::singleline(query)
                        .hint_text("Find in document")
                        .font(egui::FontId::new(12.0, egui::FontFamily::Proportional)),
                );
                if response.has_focus()
                    && ui.input(|input| input.key_pressed(egui::Key::Enter))
                {
                    commands.push(Command::Search(query.trim().to_owned()));
                }
                if !query.is_empty()
                    && ui
                        .add(
                            egui::Button::new(egui::RichText::new("×").size(14.0))
                                .fill(egui::Color32::TRANSPARENT)
                                .min_size(egui::vec2(22.0, 22.0)),
                        )
                        .on_hover_text("Clear search")
                        .clicked()
                {
                    query.clear();
                    commands.push(Command::Search(String::new()));
                }
            });
        });
    ui.add_space(6.0);

    if query.is_empty() {
        ui.label(
            egui::RichText::new("Type a word or phrase and press Enter.")
                .color(palette.text_dim)
                .size(12.0),
        );
        return;
    }
    if !current_document {
        ui.label(
            egui::RichText::new("These results belong to another tab.")
                .color(palette.text_dim)
                .size(12.0),
        );
        return;
    }
    if in_progress {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(
                egui::RichText::new("Searching pages…")
                    .color(palette.text_dim)
                    .size(12.0),
            );
        });
        return;
    }
    if let Some(error) = error {
        ui.label(egui::RichText::new(error).color(palette.danger).size(12.0));
        return;
    }
    if results.is_empty() {
        ui.label(
            egui::RichText::new(format!("No results for “{query}”."))
                .color(palette.text_dim)
                .size(12.0),
        );
        return;
    }

    let total: usize = results.iter().map(|r| r.count).sum();
    ui.label(
        egui::RichText::new(format!(
            "{total} match{} on {} page{}",
            if total == 1 { "" } else { "es" },
            results.len(),
            if results.len() == 1 { "" } else { "s" }
        ))
        .color(palette.text_dim)
        .size(11.0),
    );
    ui.add_space(4.0);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for result in results {
                if search_result_card(ui, palette, query, result)
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    commands.push(Command::GoToPage(result.page));
                }
                ui.add_space(4.0);
            }
        });
}

enum PasswordAction {
    Submit {
        tab: pdfreader_core::TabId,
        path: PathBuf,
        passphrase: String,
    },
    Cancel,
}

/// Draw the passphrase prompt without blocking the render or engine threads.
/// What the user chose in the unsaved-changes prompt.
enum CloseChoice {
    /// Write the changes first, then close the tab.
    SaveAndClose,
    /// Throw the changes away and close the tab.
    Discard,
    /// Do nothing; keep the tab and its changes.
    Cancel,
}

/// The unsaved-changes prompt for a dirty tab.
fn confirm_close_dialog(
    ctx: &egui::Context,
    palette: &pdfreader_ui::Palette,
    path: &std::path::Path,
) -> Option<CloseChoice> {
    let mut choice = None;
    egui::Window::new("Unsaved changes")
        .collapsible(false)
        .resizable(false)
        .frame(
            egui::Frame::new()
                .fill(palette.panel_bg)
                .stroke(egui::Stroke::new(1.0, palette.border))
                .corner_radius(3.0)
                .inner_margin(egui::Margin::same(18)),
        )
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.set_min_width(330.0);
            ui.label(
                egui::RichText::new(format!(
                    "{} has changes that are not saved.",
                    path.display()
                ))
                .color(palette.text)
                .size(13.0),
            );
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if ui
                    .add(egui::Button::new(egui::RichText::new("Save and close").size(12.5)))
                    .clicked()
                {
                    choice = Some(CloseChoice::SaveAndClose);
                }
                if ui
                    .add(egui::Button::new(egui::RichText::new("Discard changes").size(12.5)))
                    .clicked()
                {
                    choice = Some(CloseChoice::Discard);
                }
                if ui
                    .add(egui::Button::new(egui::RichText::new("Cancel").size(12.5)))
                    .clicked()
                {
                    choice = Some(CloseChoice::Cancel);
                }
            });
        });
    choice
}

fn password_dialog(
    ctx: &egui::Context,
    palette: &pdfreader_ui::Palette,
    prompt: &mut PasswordPrompt,
) -> Option<PasswordAction> {
    let mut action = None;
    egui::Window::new("Password required")
        .collapsible(false)
        .resizable(false)
        .frame(
            egui::Frame::new()
                .fill(palette.panel_bg)
                .stroke(egui::Stroke::new(1.0, palette.border))
                .corner_radius(3.0)
                .inner_margin(egui::Margin::same(18)),
        )
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.set_min_width(330.0);
            ui.label(
                egui::RichText::new(format!("Enter the password for {}", prompt.path.display()))
                    .color(palette.text)
                    .size(13.0),
            );
            ui.add_space(8.0);
            if let Some(error) = prompt.error.as_deref() {
                ui.label(
                    egui::RichText::new(error)
                        .color(palette.text_dim)
                        .size(12.0),
                );
                ui.add_space(4.0);
            }
            if prompt.submitting {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(
                        egui::RichText::new("Verifying…")
                            .color(palette.text_dim)
                            .size(12.0),
                    );
                });
            } else {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut prompt.passphrase)
                        .password(true)
                        .hint_text("Password")
                        .desired_width(320.0),
                );
                let enter =
                    response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new("Cancel")
                                .fill(palette.surface)
                                .corner_radius(7.0),
                        )
                        .clicked()
                    {
                        action = Some(PasswordAction::Cancel);
                    }
                    let enabled = !prompt.passphrase.is_empty();
                    if ui
                        .add_enabled(
                            enabled,
                            egui::Button::new("Unlock")
                                .fill(palette.accent_soft)
                                .corner_radius(7.0),
                        )
                        .clicked()
                        || (enter && enabled)
                    {
                        action = Some(PasswordAction::Submit {
                            tab: prompt.tab,
                            path: prompt.path.clone(),
                            passphrase: prompt.passphrase.clone(),
                        });
                    }
                });
            }
        });
    action
}

/// Empty-state canvas with a call to action.
fn empty_state(ui: &mut egui::Ui, palette: &pdfreader_ui::Palette, commands: &mut Vec<Command>) {
    // The empty-state card. Responsive width (tracks the window with a 48px outer
    // margin on each side, capped at a comfortable 560px reading width and floored
    // at a readable 320px) and natural height, centered both horizontally and
    // vertically in the available canvas.
    let outer = ui.available_size();
    let outer_margin = 48.0;
    let card_w = ((outer.x - 2.0 * outer_margin) as f32)
        .max(320.0)
        .min(560.0);
    // Natural content height: icon + spacings + labels + button + hints. We bind
    // the Frame's content Ui to this exact height so the egui layout system
    // can't expand it to the full available area.
    let content_h: f32 = 52.0 + 14.0 + 22.0 + 6.0 + 16.0 + 18.0 + 34.0 + 10.0 + 14.0 + 14.0;
    let card_h = content_h + 60.0 + 2.0;

    // Compute the centered rect for the card, anchored relative to the current
    // cursor's top-left (which is the top-left of the canvas area the caller
    // handed us). Doing this manually avoids fighting with `egui`'s layout
    // system, which otherwise lets the Frame expand to the full available
    // height when wrapped in `vertical_centered` or placed under a centered
    // parent.
    let offset = egui::vec2(
        ((outer.x - card_w) * 0.5).max(0.0),
        ((outer.y - card_h) * 0.5).max(0.0),
    );
    let frame_rect = egui::Rect::from_min_size(ui.cursor().min + offset, egui::vec2(card_w, card_h));

    // Reserve the Frame's outer rect in the parent so subsequent widgets don't
    // overlap it.
    ui.allocate_rect(frame_rect, egui::Sense::hover());

    // Build a child Ui whose max_rect is exactly the Frame's outer rect, so
    // the Frame can paint and lay out content within those bounds.
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(frame_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );

    egui::Frame::new()
        .fill(palette.panel_bg)
        .stroke(egui::Stroke::new(1.0, palette.border))
        .corner_radius(3.0)
        .inner_margin(egui::Margin::symmetric(34, 30))
        .show(&mut child, |ui| {
            // Bind the Frame's content to a known height so it doesn't stretch.
            ui.set_min_width(card_w);
            ui.set_max_width(card_w);
            ui.set_min_height(content_h);
            ui.set_max_height(content_h);
            ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                let (rect, _) = ui.allocate_exact_size(
                    egui::Vec2::splat(52.0),
                    egui::Sense::hover(),
                );
                ui.painter()
                    .circle_filled(rect.center(), 26.0, palette.accent_soft);
                ui.painter().rect_stroke(
                    egui::Rect::from_center_size(rect.center(), egui::vec2(20.0, 25.0)),
                    3.0,
                    egui::Stroke::new(1.8, palette.accent),
                    egui::StrokeKind::Inside,
                );
                ui.add_space(14.0);
                ui.add(egui::Label::new(
                    egui::RichText::new("Open a document")
                        .strong()
                        .color(palette.text)
                        .size(20.0),
                ));
                ui.add_space(6.0);
                ui.add(egui::Label::new(
                    egui::RichText::new(
                        "Start reading, searching, and navigating your PDF in one focused workspace.",
                    )
                    .color(palette.text_dim)
                    .size(12.5),
                ));
                ui.add_space(18.0);
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new("Open PDF…").color(palette.text),
                        )
                        .fill(palette.accent_soft)
                        .corner_radius(2.0)
                        .min_size(egui::vec2(132.0, 34.0)),
                    )
                    .clicked()
                {
                    commands.push(Command::ShowOpenDialog);
                }
                ui.add_space(10.0);
                ui.add(egui::Label::new(
                    egui::RichText::new("or drag a PDF anywhere into this window")
                        .color(palette.text_dim)
                        .size(11.0),
                ));
                ui.add(egui::Label::new(
                    egui::RichText::new("Ctrl+O to open from the keyboard")
                        .color(palette.text_dim)
                        .size(11.0),
                ));
            });
        });
}

/// Loading-state canvas.
fn loading_state(ui: &mut egui::Ui, palette: &pdfreader_ui::Palette) {
    ui.centered_and_justified(|ui| {
        egui::Frame::new()
            .fill(palette.panel_bg)
            .stroke(egui::Stroke::new(1.0, palette.border))
            .corner_radius(3.0)
            .inner_margin(egui::Margin::symmetric(28, 22))
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.spinner();
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new("Opening document…")
                            .strong()
                            .color(palette.text)
                            .size(14.0),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("Preparing pages and thumbnails")
                            .color(palette.text_dim)
                            .size(11.5),
                    );
                });
            });
    });
}

/// Error-state canvas.
fn error_state(
    ui: &mut egui::Ui,
    palette: &pdfreader_ui::Palette,
    tab: &Tab,
    commands: &mut Vec<Command>,
) {
    let reason = tab.error.as_deref().unwrap_or("Unknown error");
    ui.centered_and_justified(|ui| {
        egui::Frame::new()
            .fill(palette.panel_bg)
            .stroke(egui::Stroke::new(1.0, palette.danger.gamma_multiply(0.55)))
            .corner_radius(12.0)
            .inner_margin(egui::Margin::symmetric(30, 24))
            .show(ui, |ui| {
                ui.set_max_width(420.0);
                ui.vertical_centered(|ui| {
                    ui.label(
                        egui::RichText::new("Could not open this document")
                            .strong()
                            .color(palette.text)
                            .size(15.0),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(reason)
                            .color(palette.text_dim)
                            .size(12.0),
                    );
                    ui.add_space(16.0);
                    if ui
                        .add(
                            egui::Button::new(egui::RichText::new("Dismiss").color(palette.text))
                                .fill(palette.surface)
                                .corner_radius(7.0)
                                .min_size(egui::vec2(96.0, 32.0)),
                        )
                        .clicked()
                    {
                        commands.push(Command::DismissError);
                    }
                });
            });
    });
}

/// Write a captured frame to a PNG.
///
/// Debug aid for `PDFREADER_SCREENSHOT=<path>`: it lets the GUI be inspected
/// without a human looking at the screen.
fn save_screenshot(path: &std::path::Path, image: &egui::ColorImage) {
    let width = image.size[0] as u32;
    let height = image.size[1] as u32;
    let mut bytes = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for pixel in &image.pixels {
        bytes.extend_from_slice(&pixel.to_array());
    }

    match image::RgbaImage::from_raw(width, height, bytes) {
        Some(buffer) => match buffer.save(path) {
            Ok(()) => println!("screenshot saved: {}", path.display()),
            Err(error) => eprintln!("screenshot save failed: {error}"),
        },
        None => eprintln!("screenshot: unexpected buffer size"),
    }
}
