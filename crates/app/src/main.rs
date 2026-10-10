//! Application entry point.
//!
//! Owns the store, the engine thread, the tile canvas and the egui shell, and
//! is the only place that executes effects. The UI produces commands; this file
//! makes things happen and feeds results back as more commands.

// Release builds are real windowed applications: no console window flashes
// behind the GUI when the exe is launched from Explorer or an installer
// shortcut. Debug builds keep the console so tracing output stays visible.
#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

mod canvas;
mod engine_thread;
mod export_window;
mod fonts;
mod lens;
mod print;
mod print_window;
mod session;
mod settings;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::ViewportCommand;
use pdfreader_core::{
    Command, DocumentId, Effect, ExportTarget, ExportTask, FieldValue, FormFieldType, Rotation,
    SidebarTab, Store, Tab, ThemeId, Tool, ViewMode, ZoomMode,
};
use pdfreader_pdf::engine::{DocumentHandle, PdfiumEngine};
use pdfreader_search::SearchMatch;
use pdfreader_ui::{
    MENUBAR_HEIGHT, STATUSBAR_HEIGHT, TABBAR_HEIGHT, TOOLBAR_HEIGHT, TOOLS_RAIL_WIDTH, TextStyle,
    annotation_bar, annotation_properties_window, comments_panel, forms_panel, menu_bar, outline_tree, sidebar_tabs, status_bar,
    tab_bar, toolbar, tools_rail,
};
use serde::{Deserialize, Serialize};

use canvas::{Canvas, MARGIN, PendingThumb, PendingTile, SCROLLBAR_ALLOWANCE};
use engine_thread::{EngineRequest, EngineResponse, EngineThread};
use export_window::{ExportWindow, Field, OpenDocument};

/// Application icon, embedded at compile time so the window is branded even
/// when the exe is copied somewhere without `assets/` next to it.
const ICON_PNG: &[u8] = include_bytes!("../../../assets/icon.png");

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
        .map_or_else(
            || std::path::PathBuf::from("."),
            std::path::Path::to_path_buf,
        );
    let file_name = path.file_name().map_or_else(
        || std::ffi::OsString::from("document.pdf"),
        std::ffi::OsString::from,
    );

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

    // Native OS decorations: the standard minimize / maximize / close buttons
    // are always present and behave the way people expect. The window opens
    // maximized — filling the screen *with* its title bar — unless the user
    // turned that off. The first frames repeat the request, because winit does
    // not always honour the creation-time one.
    let maximized = wants_maximized(&load_settings());
    let mut viewport = egui::ViewportBuilder::default()
        .with_min_inner_size([820.0, 560.0])
        .with_decorations(true)
        .with_title("AnsNew PDF Reader");
    // Asking for an inner size *and* maximized at once leaves the window with
    // the maximized flag set but the requested size applied — it looks
    // maximized to winit and windowed on screen. Only one of them is set.
    viewport = if maximized {
        viewport.with_maximized(true)
    } else {
        viewport.with_inner_size([1400.0, 900.0])
    };

    // Title bar, taskbar and Alt-Tab. Without this Windows falls back to the
    // generic application icon even when the exe carries a resource.
    let viewport = match eframe::icon_data::from_png_bytes(ICON_PNG) {
        Ok(icon) => viewport.with_icon(std::sync::Arc::new(icon)),
        Err(error) => {
            eprintln!("failed to decode the application icon: {error}");
            viewport
        }
    };

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    if let Err(error) = eframe::run_native(
        "AnsNew PDF Reader",
        options,
        Box::new(move |cc| {
            // Ctrl+Plus / Ctrl+Minus / Ctrl+0 zoom the *document* here, so
            // egui's whole-GUI zoom has to be switched off: otherwise both
            // fire on the same keypress and the menus and toolbars scale along
            // with the page, which is what a PDF reader must never do.
            cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
            // Register the bundled Noto fonts so Bengali and other scripts
            // render instead of missing-glyph boxes. Safe to call before the
            // first frame; a missing assets/fonts directory is harmless.
            fonts::install(&cc.egui_ctx);
            Ok(Box::new(App::new(engine_thread, initial)))
        }),
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
                // `wgpu_hal` logs a warning when the Vulkan loader cannot find
                // the optional SDK layer manifests in the registry. That is
                // pure noise on any machine without the Vulkan SDK installed,
                // it says nothing about this app, and it made every launch look
                // like it had failed — so its warnings are dropped. Everything
                // else keeps its level, and `RUST_LOG` still overrides this.
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,wgpu_hal=error")),
        )
        .try_init();
}

/// Directory the executable lives in, where PDFium is staged.
pub(crate) fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
}

/// Persisted user preferences.
#[derive(Default, Serialize, Deserialize)]
struct Settings {
    theme: Option<String>,
    sidebar_visible: Option<bool>,
    /// Language the lookup popup's Google Translate button links to. It does
    /// not affect the offline answer, which is fixed to Bangla by the bundled
    /// dictionaries.
    lens_target: Option<String>,
    /// Whether the offline word-lookup popup is enabled.
    lookup_enabled: Option<bool>,
    /// Whether the popup's "Speak" (text-to-speech) button is enabled.
    speak_enabled: Option<bool>,
    /// Whether the selection is read aloud automatically when the card opens.
    auto_speak: Option<bool>,
    /// Whether the popup's "Google Translate" link is enabled.
    translate_enabled: Option<bool>,
    /// Interface scale (egui zoom factor), 1.0 = 100%.
    ui_scale: Option<f32>,
    /// View mode new documents open in: "single" or "continuous".
    default_view_mode: Option<String>,
    /// Zoom new documents open at: "fit-width", "fit-page" or "100".
    default_zoom: Option<String>,
    /// Whether the on-screen performance overlay is shown.
    show_stats: Option<bool>,
    /// Whether tiles just outside the viewport are prefetched.
    prefetch: Option<bool>,
    /// Font family used for form field text (empty = default).
    form_font: Option<String>,
    /// Point size used for form field text (0 = fit the field).
    form_font_size: Option<f32>,
    /// RGB ink for form field text. Absent means the default near-black.
    form_font_color: Option<[u8; 3]>,
    /// Whether form text is drawn heavier, when the user has chosen.
    form_font_bold: Option<bool>,
    /// Whether the window opens maximized. Absent means yes, which is the
    /// default a reader wants: the page is the point, but the title bar and its
    /// minimize / maximize / close buttons stay available.
    open_maximized: Option<bool>,
}

/// Whether the window should open maximized.
///
/// Maximized, not full screen: the window fills the screen *with* its title bar
/// and the standard window buttons, which is what "open big" means to a desktop
/// user. True borderless full screen stays on F11.
///
/// On by default (see [`Settings::open_maximized`]). A screenshot run opts out
/// so the captured size is predictable, and `PDFREADER_MAXIMIZED` forces it on.
fn wants_maximized(settings: &Settings) -> bool {
    if std::env::var_os("PDFREADER_MAXIMIZED").is_some() {
        return true;
    }
    settings.open_maximized.unwrap_or(true) && std::env::var_os("PDFREADER_SCREENSHOT").is_none()
}

/// State for the non-blocking encrypted-document password prompt.
struct PasswordPrompt {
    tab: pdfreader_core::TabId,
    path: PathBuf,
    passphrase: String,
    error: Option<String>,
    submitting: bool,
}

/// How often the session may be written while it keeps changing. Scrolling
/// changes it every frame; this keeps the disk off the hot path.
const SESSION_SAVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// Where the app keeps its files, per platform convention.
pub(crate) fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("pdf-reader"))
}

/// Where preferences are stored.
fn config_file() -> Option<PathBuf> {
    Some(config_dir()?.join("settings.json"))
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

/// Map the persisted view-mode name to its enum. Anything unrecognised — an
/// old file, a hand edit — falls back to the default rather than failing.
fn view_mode_from_name(name: Option<&str>) -> ViewMode {
    match name {
        Some("single") => ViewMode::Single,
        _ => ViewMode::Continuous,
    }
}

/// Map the persisted zoom name to its enum, with the same fallback rule.
fn zoom_mode_from_name(name: Option<&str>) -> ZoomMode {
    match name {
        Some("fit-page") => ZoomMode::FitPage,
        Some("100") => ZoomMode::Fixed(1.0),
        _ => ZoomMode::FitWidth,
    }
}

/// Image extensions the reader can decode.
///
/// Matches the `image` features the workspace enables; adding a codec there
/// means adding it here, or the picker will hide files the app can read.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg"];

/// What a native dialog is being opened for.
///
/// The intent is what turns a bare path back into the command that acts on it,
/// so it has to survive the round trip through the dialog thread.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DialogIntent {
    /// Pick one PDF to open.
    OpenPdf,
    /// Pick where to save the active document.
    SavePdf,
    /// Pick the export window's output file.
    ExportOutputFile,
    /// Pick the export window's output directory.
    ExportOutputFolder,
    /// Pick images to add to the export window's list.
    ExportAddImages,
    /// Pick PDFs to add to the export window's list.
    ExportAddPdfs,
}

/// What a native dialog came back with.
enum DialogResult {
    /// The user closed it without choosing.
    Cancelled,
    /// A single file or folder.
    Path(PathBuf),
    /// Several files, in the order the dialog returned them.
    Paths(Vec<PathBuf>),
}

/// Show one native dialog and normalise its answer.
///
/// Runs on the dialog thread, so it takes everything it needs by value and
/// never touches application state.
fn run_dialog(intent: DialogIntent, default_name: Option<String>) -> DialogResult {
    // Naming a save dialog's file is only meaningful when it is a save dialog.
    let with_name = |mut dialog: rfd::FileDialog| {
        if let Some(name) = default_name.clone() {
            dialog = dialog.set_file_name(name);
        }
        dialog
    };

    match intent {
        DialogIntent::OpenPdf => rfd::FileDialog::new()
            .add_filter("PDF document", &["pdf"])
            .set_title("Open PDF")
            .pick_file()
            .map_or(DialogResult::Cancelled, DialogResult::Path),

        DialogIntent::SavePdf => with_name(
            rfd::FileDialog::new()
                .add_filter("PDF document", &["pdf"])
                .set_title("Save PDF as"),
        )
        .save_file()
        .map_or(DialogResult::Cancelled, DialogResult::Path),

        // The export window's own pickers. They stay native — an in-app file
        // browser would be a worse file browser — but they feed the window
        // rather than ending the interaction.
        DialogIntent::ExportOutputFile => with_name(
            rfd::FileDialog::new()
                .add_filter("PDF document", &["pdf"])
                .add_filter("PNG image", &["png"])
                .add_filter("JPEG image", &["jpg", "jpeg"])
                .set_title("Save as"),
        )
        .save_file()
        .map_or(DialogResult::Cancelled, DialogResult::Path),

        DialogIntent::ExportOutputFolder => rfd::FileDialog::new()
            .set_title("Choose a folder for the exported pages")
            .pick_folder()
            .map_or(DialogResult::Cancelled, DialogResult::Path),

        DialogIntent::ExportAddImages => rfd::FileDialog::new()
            .add_filter("Images", IMAGE_EXTENSIONS)
            .set_title("Choose images, one page each")
            .pick_files()
            .map_or(DialogResult::Cancelled, DialogResult::Paths),

        DialogIntent::ExportAddPdfs => rfd::FileDialog::new()
            .add_filter("PDF document", &["pdf"])
            .set_title("Choose PDFs to merge, in order")
            .pick_files()
            .map_or(DialogResult::Cancelled, DialogResult::Paths),
    }
}

/// The dialog a command opens, if it opens one.
///
/// Dialog commands are intercepted by the shell rather than reduced, exactly
/// like `ShowOpenDialog`: the reducer cannot show a window, and it must not
/// invent a path the user never chose.
fn dialog_intent_for(command: &Command) -> Option<DialogIntent> {
    match command {
        Command::ShowOpenDialog => Some(DialogIntent::OpenPdf),
        Command::SaveDocumentAs => Some(DialogIntent::SavePdf),
        _ => None,
    }
}

/// The export-window field a picker fills in.
///
/// The export window asks for a picker by naming the field it wants filled, and
/// this is the one place that knows which native dialog that means.
fn dialog_intent_for_field(field: Field) -> DialogIntent {
    match field {
        Field::OutputFile => DialogIntent::ExportOutputFile,
        Field::OutputFolder => DialogIntent::ExportOutputFolder,
        Field::AddImages => DialogIntent::ExportAddImages,
        Field::AddPdfs => DialogIntent::ExportAddPdfs,
    }
}

/// Debug aid for `PDFREADER_EXPORT_WINDOW=<name>`.
///
/// Opening the window on a named task at launch is the only way to check that
/// all five layouts draw without a human at the screen. The names are the ones
/// a reader would guess from the menu; anything else is reported and ignored.
fn export_task_from_name(name: &str) -> Option<ExportTask> {
    let name = name.trim().to_ascii_lowercase();
    ExportTask::ALL.into_iter().find(|task| {
        // "Page to image" → "page-to-image", so the shell spelling is accepted.
        task.label().to_ascii_lowercase().replace(' ', "-") == name
    })
}

/// What a status message is telling the user, which decides how it looks.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ToastKind {
    /// Something went wrong and needs attention.
    Error,
    /// Work is in progress; the message carries a Cancel button.
    Busy,
    /// Something worked.
    Success,
}

/// A short sentence describing what a finished job produced.
///
/// A directory means "all pages as images", which reports a file count; a file
/// reports its page count. `pages` is 1 for a single-page export, which reads
/// better without the count spelled out.
fn describe_output(path: &std::path::Path, pages: u32) -> String {
    if path.is_dir() {
        return format!("Wrote {pages} images to {}", path.display());
    }

    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );

    if pages > 1 {
        format!("Wrote {name} ({pages} pages)")
    } else {
        format!("Wrote {name}")
    }
}

/// An export, composition or merge job the shell is waiting on.
///
/// Jobs run one at a time on the engine thread, so the shell tracks exactly
/// one. Starting another cancels this one first rather than queueing behind it,
/// which would otherwise leave the user watching a progress bar for work they
/// no longer want.
struct RunningJob {
    /// Identifier the engine echoes back on every report.
    id: u64,
    /// What the job is doing, for the progress message.
    label: String,
    /// Set to ask the engine to stop at the next item.
    cancel: Arc<AtomicBool>,
    /// Items finished and total, once the engine has reported any.
    progress: Option<(u32, u32)>,
}

/// The eframe application.
///
/// The many booleans are independent feature flags and view toggles; grouping
/// them into sub-structs would not make the shell any clearer.
#[allow(clippy::struct_excessive_bools)]
struct App {
    /// Application state and the reducer.
    store: Store,
    /// Tile/thumbnail texture cache and canvas drawing.
    canvas: Canvas,
    /// The one thread allowed to touch PDFium.
    engine: EngineThread,
    /// Engine handles for documents the store knows about.
    handles: HashMap<DocumentId, DocumentHandle>,
    /// Receives the result of whichever native file dialog is open.
    ///
    /// One channel and one slot serve every dialog: only one can be open at a
    /// time, and the intent recorded alongside it is what says how to read the
    /// answer. A channel per dialog would be six copies of the same plumbing.
    dialog_rx: Receiver<DialogResult>,
    /// Sends file dialog results.
    dialog_tx: Sender<DialogResult>,
    /// What the open dialog is for, so its answer can be routed. `None` means
    /// no dialog is open.
    dialog_intent: Option<DialogIntent>,
    /// The export window, when one is open.
    ///
    /// At most one at a time: it is a modal, so a second one could only ever be
    /// behind the first.
    export_window: Option<ExportWindow>,
    /// The print window, open while the user is choosing what to print.
    print_window: Option<print_window::PrintWindow>,
    /// The export task the window was last opened on, so reopening it returns
    /// to what the user was doing rather than to the first task every time.
    last_export_task: ExportTask,
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
    /// The export or composition job currently running, if any.
    running_job: Option<RunningJob>,
    /// What the last finished job did, shown until the user dismisses it.
    ///
    /// Separate from `save_error` so a success can be reported too: an export
    /// that wrote a file to a folder the user then has to go and find deserves
    /// an acknowledgement.
    job_notice: Option<String>,
    /// Annotation jump waiting to be applied: page + PDF-space point to
    /// centre in the viewport.
    scroll_to_point: Option<(u32, (f32, f32))>,
    /// Whether the window was maximized when fullscreen was entered, so
    /// leaving fullscreen can put it back. Borderless fullscreen from a
    /// maximized window is a known egui-winit conflict on Windows: entering
    /// without un-maximizing first can fail to cover the taskbar, and leaving
    /// without restoring leaves a small floating window.
    pre_fullscreen_maximized: Option<bool>,
    /// A viewport change held back a frame or two so the change before it can
    /// settle. Entering fullscreen from a maximized window must un-maximize
    /// first: doing both in one frame leaves the window fullscreen-sized with
    /// the taskbar still showing. Leaving restores the maximized state the same
    /// way. The `u8` counts frames waited, so a viewport that never reports the
    /// expected state still moves on.
    pending_viewport: Option<(u8, ViewportCommand)>,
    /// Passphrase prompts for encrypted documents, keyed by tab. Several
    /// encrypted documents can fail at once; a single slot would silently
    /// discard the passphrase a user is halfway through typing.
    password_prompts: HashMap<pdfreader_core::TabId, PasswordPrompt>,
    /// The bundled dictionaries, once the background thread has read them.
    ///
    /// `None` until the load finishes, which is why a lookup made in the first
    /// moment after launch answers "loading" rather than "not found".
    lens: Option<pdfreader_lens::Dictionaries>,
    /// Receives the dictionaries from the loading thread, exactly once.
    ///
    /// A `crossbeam` receiver rather than the `std::sync::mpsc` one the dialogs
    /// use, because this one is polled every frame and must be non-blocking.
    lens_rx: Option<crossbeam_channel::Receiver<Result<pdfreader_lens::Dictionaries, String>>>,
    /// Why the dictionaries are unavailable, shown in the popup when it is.
    lens_error: Option<String>,
    /// Extracted text for the pages that have been on screen.
    page_text: lens::PageTextCache,
    /// The floating card that answers a selected word.
    lens_popup: lens::LensPopup,
    /// Language the Google Translate button links to.
    lens_target: String,
    /// Whether the offline word-lookup popup is enabled (default true).
    lookup_enabled: bool,
    /// Whether the popup's "Speak" (text-to-speech) button is enabled.
    speak_enabled: bool,
    /// Whether the selection is read aloud automatically when the card opens.
    auto_speak: bool,
    /// Whether the popup's "Google Translate" link is enabled.
    translate_enabled: bool,
    /// Interface scale (egui zoom factor), 1.0 = 100%.
    ui_scale: f32,
    /// View mode new documents open in.
    default_view_mode: ViewMode,
    /// Zoom new documents open at.
    default_zoom: ZoomMode,
    /// Whether the on-screen performance overlay is shown.
    show_stats: bool,
    /// Whether tiles just outside the viewport are prefetched.
    prefetch: bool,
    /// Font family used for form field text (empty = default).
    form_font: String,
    /// Point size used for form field text (0 = fit the field).
    form_font_size: f32,
    /// RGB ink for text typed into form fields.
    form_font_color: [u8; 3],
    /// Whether typed form text is drawn heavier (synthetic bold).
    form_font_bold: bool,
    /// Whether the next launch opens maximized (Settings → Reading → Window).
    open_maximized: bool,
    /// Frames left to keep asking winit to maximize the window. A creation-time
    /// request is not always honoured, so it is repeated for a moment and then
    /// dropped — after that the user's own window buttons win.
    start_maximized: u8,
    /// State of the full Settings window (open flag and selected category).
    settings_window: settings::SettingsWindow,
    /// Set by Edit → Find… so the toolbar's find box takes focus next frame.
    focus_search: bool,
    /// Debug: a sample word to open the lookup card on, for `PDFREADER_DEMO_POPUP`
    /// screenshot QA. Taken once the dictionaries are ready.
    demo_popup: Option<String>,
    /// Whether the keyboard-shortcuts reference window is open.
    shortcuts_open: bool,
    /// Whether the "About Us" window is open.
    about_open: bool,
    /// The profile photo, decoded once on first use.
    profile_texture: Option<egui::TextureHandle>,
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
    /// Frame at which `PDFREADER_SCREENSHOT` fires; raise it when the shot
    /// needs something that loads late (dictionaries).
    screenshot_frames: u64,
    frames_total: u64,
    /// Whether the debug form hook has already fired. The form is re-read after
    /// a painted write, so an unguarded hook would toggle the choice back.
    debug_form_done: bool,
    /// The session as last written to disk, for change detection.
    session: session::Session,
    /// When the session was last written, so a scroll does not hit the disk on
    /// every frame.
    last_session_save: std::time::Instant,
}

impl App {
    /// Build the application.
    fn new(engine: EngineThread, initial: Option<PathBuf>) -> Self {
        let (dialog_tx, dialog_rx) = channel();
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
        // Debug: pick the initial sidebar tab (PDFREADER_SIDEBAR_TAB), so a
        // panel can be captured without clicking its tab.
        if let Ok(tab) = std::env::var("PDFREADER_SIDEBAR_TAB") {
            let tab = match tab.to_ascii_lowercase().as_str() {
                "outline" => SidebarTab::Outline,
                "search" => SidebarTab::Search,
                "comments" => SidebarTab::Comments,
                "forms" => SidebarTab::Forms,
                "bookmarks" => SidebarTab::Bookmarks,
                _ => SidebarTab::Thumbnails,
            };
            store.dispatch(Command::SetSidebarTab(tab));
        }

        // Read before `settings` is partially moved into the struct below.
        let start_maximized = wants_maximized(&settings);

        let mut app = Self {
            store,
            canvas: Canvas::new(),
            engine,
            handles: HashMap::new(),
            dialog_rx,
            dialog_tx,
            dialog_intent: None,
            export_window: None,
            print_window: None,
            last_export_task: ExportTask::default(),
            pending_close: None,
            close_after_save: None,
            save_error: None,
            save_target: None,
            running_job: None,
            job_notice: None,
            pre_fullscreen_maximized: None,
            pending_viewport: None,
            scroll_to_point: None,
            password_prompts: HashMap::new(),
            lens: None,
            lens_rx: Some(lens::spawn_load(
                pdfreader_lens::load::find_dir(exe_dir()).as_deref(),
            )),
            lens_error: None,
            page_text: lens::PageTextCache::new(),
            lens_popup: lens::LensPopup::new(),
            lens_target: settings
                .lens_target
                .as_deref()
                .filter(|target| pdfreader_lens::is_valid_target(target))
                .unwrap_or(pdfreader_lens::DEFAULT_TARGET)
                .to_owned(),
            // The helper tools default to on; an absent value means "use the
            // default", not "off", so the popup does not surprise a new user
            // by being hidden.
            lookup_enabled: settings.lookup_enabled.unwrap_or(true),
            speak_enabled: settings.speak_enabled.unwrap_or(true),
            auto_speak: settings.auto_speak.unwrap_or(true),
            translate_enabled: settings.translate_enabled.unwrap_or(true),
            ui_scale: settings
                .ui_scale
                .filter(|scale| (0.5..=3.0).contains(scale))
                .unwrap_or(1.0),
            default_view_mode: view_mode_from_name(settings.default_view_mode.as_deref()),
            default_zoom: zoom_mode_from_name(settings.default_zoom.as_deref()),
            show_stats: settings.show_stats.unwrap_or(false),
            prefetch: settings.prefetch.unwrap_or(true),
            form_font: settings.form_font.unwrap_or_default(),
            form_font_size: settings.form_font_size.unwrap_or(0.0).max(0.0),
            form_font_bold: settings.form_font_bold.unwrap_or(false),
            form_font_color: settings.form_font_color.unwrap_or(canvas::DEFAULT_FORM_INK),
            open_maximized: settings.open_maximized.unwrap_or(true),
            start_maximized: if start_maximized { 15 } else { 0 },
            settings_window: settings::SettingsWindow::new(),
            focus_search: false,
            demo_popup: std::env::var("PDFREADER_DEMO_POPUP").ok(),
            shortcuts_open: false,
            about_open: std::env::var_os("PDFREADER_ABOUT").is_some(),
            profile_texture: None,
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
            session: session::Session::default(),
            last_session_save: std::time::Instant::now(),
            screenshot_frames: std::env::var("PDFREADER_SCREENSHOT_FRAMES")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(20),
            frames_total: 0,
            debug_form_done: false,
        };

        // The canvas owns the prefetch flag (it is what draws the tiles), so
        // seed it from the persisted preference now that `app` exists.
        app.canvas.set_prefetch(app.prefetch);
        app.canvas.set_form_font(canvas::FormFont {
            family: app.form_font.clone(),
            size: app.form_font_size,
            color: app.form_font_color,
            bold: app.form_font_bold,
        });

        // Reopen the tabs from last run, each where it was left. A file that has
        // since been deleted is dropped rather than reopened as an error tab on
        // every launch.
        let remembered = session::load().retaining_existing_files();
        if !remembered.tabs.is_empty() {
            let effects = app.store.dispatch(Command::RestoreTabs {
                tabs: remembered
                    .tabs
                    .iter()
                    .map(|tab| (tab.path.clone(), tab.view))
                    .collect(),
                active: remembered.active,
            });
            app.execute(effects);
        }
        app.session = remembered;

        // Debug: open the Settings window on launch so its layout can be
        // captured without a human clicking through the menu (pair with
        // `PDFREADER_SCREENSHOT`). `PDFREADER_SETTINGS_PAGE` picks the page.
        if std::env::var_os("PDFREADER_OPEN_SETTINGS").is_some() {
            app.settings_window.open = true;
            if let Ok(page) = std::env::var("PDFREADER_SETTINGS_PAGE") {
                app.settings_window.select_by_name(&page);
            }
        }
        // Debug: open the keyboard-shortcuts reference on launch (screenshot QA).
        if std::env::var_os("PDFREADER_SHOW_SHORTCUTS").is_some() {
            app.shortcuts_open = true;
        }
        // Debug: open the print window on launch so its layout can be captured
        // without a human clicking through the menu (pair with
        // `PDFREADER_SCREENSHOT`). The page count is read live, so the document
        // passed on the command line fills the window in as it loads.
        if std::env::var_os("PDFREADER_PRINT_WINDOW").is_some() {
            app.print_window = Some(print_window::PrintWindow::new(0));
        }

        // Open whatever path was given, even if it is missing — the engine will
        // report a clear error rather than the UI silently doing nothing.
        if let Some(path) = initial {
            let effects = app.store.dispatch(Command::OpenPath(path));
            app.execute(effects);
        }

        // Debug: open the export window straight away so its layout can be
        // exercised without a human clicking through the menu. The value is a
        // task name from `ExportTask::label`, optionally followed by a
        // comma-separated list of files to put in the window's list:
        // `PDFREADER_EXPORT_WINDOW="merge-pdfs:a.pdf,b.pdf"`.
        if let Ok(value) = std::env::var("PDFREADER_EXPORT_WINDOW") {
            let (name, files) = value.split_once(':').unwrap_or((value.as_str(), ""));
            if let Some(task) = export_task_from_name(name) {
                app.open_export_window(task);
                let files: Vec<PathBuf> = files
                    .split(',')
                    .map(str::trim)
                    .filter(|file| !file.is_empty())
                    .map(PathBuf::from)
                    .collect();
                if let Some(window) = app.export_window.as_mut() {
                    window.add_sources(files);
                }
            } else {
                tracing::warn!("unknown PDFREADER_EXPORT_WINDOW task: {name}");
            }
        }

        app
    }

    /// Push the form-font preference to the canvas, which owns the drawing.
    fn sync_form_font(&mut self) {
        self.canvas.set_form_font(canvas::FormFont {
            family: self.form_font.clone(),
            size: self.form_font_size,
            color: self.form_font_color,
            bold: self.form_font_bold,
        });
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
            lens_target: Some(self.lens_target.clone()),
            lookup_enabled: Some(self.lookup_enabled),
            speak_enabled: Some(self.speak_enabled),
            auto_speak: Some(self.auto_speak),
            translate_enabled: Some(self.translate_enabled),
            ui_scale: Some(self.ui_scale),
            default_view_mode: Some(
                match self.default_view_mode {
                    ViewMode::Single => "single",
                    ViewMode::Continuous => "continuous",
                }
                .to_owned(),
            ),
            default_zoom: Some(
                match self.default_zoom {
                    ZoomMode::FitWidth => "fit-width",
                    ZoomMode::FitPage => "fit-page",
                    ZoomMode::Fixed(_) => "100",
                }
                .to_owned(),
            ),
            show_stats: Some(self.show_stats),
            prefetch: Some(self.prefetch),
            form_font: Some(self.form_font.clone()),
            form_font_size: Some(self.form_font_size),
            form_font_color: Some(self.form_font_color),
            form_font_bold: Some(self.form_font_bold),
            open_maximized: Some(self.open_maximized),
        };
        if let Ok(text) = serde_json::to_string_pretty(&settings) {
            let _ = std::fs::write(path, text);
        }
    }

    /// The session as it stands now: the open tabs and where each is left.
    fn current_session(&self) -> session::Session {
        let state = self.store.state();
        let mut tabs = Vec::new();
        let mut active = None;
        for tab in &state.tabs {
            // A tab that failed to load is not worth remembering; a file that
            // has since gone is skipped on restore anyway.
            if tab.error.is_some() {
                continue;
            }
            if Some(tab.id) == state.active_tab {
                active = Some(tabs.len());
            }
            tabs.push(session::SessionTab {
                path: tab.path.clone(),
                view: tab.view,
            });
        }
        session::Session { tabs, active }
    }

    /// Write the session when it has changed.
    ///
    /// Scrolling changes the session every frame, so an unforced write is
    /// throttled to [`SESSION_SAVE_INTERVAL`]; structural changes and shutdown
    /// pass `force` and write at once.
    /// Returns `true` when a change is still waiting out the debounce, so the
    /// caller can arrange a frame to finish the write.
    fn persist_session(&mut self, force: bool) -> bool {
        let current = self.current_session();
        if current == self.session {
            return false;
        }
        if !force && self.last_session_save.elapsed() < SESSION_SAVE_INTERVAL {
            return true;
        }
        session::save(&current);
        self.session = current;
        self.last_session_save = std::time::Instant::now();
        false
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
                    self.page_text.drop_document(doc);
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
                Effect::RevealField { tab, page, rect } => {
                    // The canvas lays out one document at a time, so a reveal
                    // for a tab that is no longer active would move the wrong
                    // view. The canvas works out whether any scrolling is
                    // actually needed; a field already on screen does not move.
                    if self.store.state().active_tab == Some(tab) {
                        self.canvas.reveal_field(page, rect);
                    }
                }
                Effect::PersistSession => {
                    self.persist();
                    // A structural change — a tab opened or closed — is worth
                    // writing at once rather than waiting out the debounce.
                    let _ = self.persist_session(true);
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
                Effect::ScrollToPoint {
                    tab: _,
                    page,
                    point,
                } => {
                    self.scroll_to_point = Some((page, point));
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
                        self.engine
                            .send(EngineRequest::DeleteAnnotation { doc, handle, id });
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
                Effect::UpdateAnnotationBounds { doc, id, rect } => {
                    if let Some(handle) = self.handles.get(&doc).copied() {
                        self.engine.send(EngineRequest::UpdateAnnotationBounds {
                            doc,
                            handle,
                            id,
                            rect,
                        });
                    }
                    self.canvas.invalidate_page(doc, id.page);
                }
                Effect::SetAnnotationColor { doc, id, color } => {
                    if let Some(handle) = self.handles.get(&doc).copied() {
                        self.engine
                            .send(EngineRequest::SetAnnotationColor { doc, handle, id, color });
                    }
                    self.canvas.invalidate_page(doc, id.page);
                }
                Effect::ExportPageImage {
                    doc,
                    page,
                    rotation,
                    scale,
                    format,
                    path,
                } => {
                    if let Some(handle) = self.handles.get(&doc).copied() {
                        let label = format!("Exporting page {} as {}", page + 1, format.label());
                        let (job, cancel) = self.begin_job(label);
                        self.engine.send(EngineRequest::ExportPageImage {
                            handle,
                            page,
                            rotation,
                            scale,
                            format,
                            path,
                            job,
                            cancel,
                        });
                    }
                }
                Effect::ExportAllPages {
                    doc,
                    dir,
                    stem,
                    rotation,
                    scale,
                    format,
                } => {
                    if let Some(handle) = self.handles.get(&doc).copied() {
                        let label = format!("Exporting every page as {}", format.label());
                        let (job, cancel) = self.begin_job(label);
                        self.engine.send(EngineRequest::ExportAllPages {
                            handle,
                            dir,
                            stem,
                            rotation,
                            scale,
                            format,
                            job,
                            cancel,
                        });
                    }
                }
                Effect::ExportPages { doc, range, path } => {
                    if let Some(handle) = self.handles.get(&doc).copied() {
                        let label = match range {
                            Some(_) => "Exporting the page as PDF",
                            None => "Exporting the document as PDF",
                        };
                        let (job, cancel) = self.begin_job(label.to_string());
                        self.engine.send(EngineRequest::ExportPages {
                            handle,
                            range,
                            path,
                            target: ExportTarget::SaveFile,
                            job,
                            cancel,
                        });
                    }
                }
                Effect::Print { doc, range } => {
                    if self.handles.contains_key(&doc) {
                        // The window is where the pages are chosen, so only the
                        // page count is needed here. "Print Current Page" opens
                        // with the page in view already selected; File → Print
                        // opens on the whole document.
                        let count = self
                            .store
                            .state()
                            .tabs
                            .iter()
                            .find(|t| t.document.as_ref().is_some_and(|d| d.id == doc))
                            .map_or(0, |t| t.page_count());
                        self.print_window = Some(if range.is_some() {
                            print_window::PrintWindow::current_page(count)
                        } else {
                            print_window::PrintWindow::new(count)
                        });
                    }
                }
                Effect::ImagesToPdf {
                    images,
                    output,
                    size,
                } => {
                    let label = format!("Building a PDF from {} images", images.len());
                    let (job, cancel) = self.begin_job(label);
                    self.engine.send(EngineRequest::ImagesToPdf {
                        images,
                        output,
                        size,
                        job,
                        cancel,
                    });
                }
                Effect::MergePdfs { sources, output } => {
                    let label = format!("Merging {} documents", sources.len());
                    let (job, cancel) = self.begin_job(label);
                    self.engine.send(EngineRequest::MergePdfs {
                        sources,
                        output,
                        job,
                        cancel,
                    });
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
            let theme = pdfreader_ui::Theme::from_id(theme_id);
            // The canvas paints the on-page note editor by hand, so it has to
            // be told the radius the rest of the chrome is using.
            self.canvas.set_corner_radius(theme.palette.radius);
            theme.apply(ctx);
            self.applied_theme = Some(theme_id);
        }

        if let Ok(result) = self.dialog_rx.try_recv() {
            // The intent is taken before routing, which frees the slot for a
            // two-step job's second dialog.
            if let Some(intent) = self.dialog_intent.take() {
                self.handle_dialog_result(ctx, intent, result);
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
            && self
                .store
                .state()
                .active()
                .is_some_and(|t| t.document.is_some())
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

                    // Apply the default view mode and zoom (Settings → Reading)
                    // to the freshly opened document. `SetViewMode`/`SetZoom`
                    // act on the active tab, and opening made this tab active.
                    if self.store.state().active_tab == Some(tab) {
                        let mode = self.default_view_mode;
                        let zoom = self.default_zoom;
                        let effects = self.store.dispatch(Command::SetViewMode(mode));
                        self.execute(effects);
                        let effects = self.store.dispatch(Command::SetZoom(zoom));
                        self.execute(effects);
                    }

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
                    // Debug: draw one of each annotation so the marks can be
                    // captured without a mouse (PDFREADER_DRAW_SAMPLES). Goes
                    // through the same command a drag commits, so what appears
                    // is what a drag produces.
                    if !self.debug_form_done && std::env::var_os("PDFREADER_DRAW_SAMPLES").is_some()
                    {
                        self.debug_form_done = true;
                        for new in [
                            pdfreader_core::NewAnnotation::Highlight(
                                pdfreader_core::Rect::from_xywh(80.0, 700.0, 200.0, 18.0),
                            ),
                            pdfreader_core::NewAnnotation::Underline(
                                pdfreader_core::Rect::from_xywh(80.0, 660.0, 200.0, 18.0),
                            ),
                            pdfreader_core::NewAnnotation::StrikeOut(
                                pdfreader_core::Rect::from_xywh(80.0, 620.0, 200.0, 18.0),
                            ),
                            pdfreader_core::NewAnnotation::Squiggly(
                                pdfreader_core::Rect::from_xywh(80.0, 580.0, 200.0, 18.0),
                            ),
                            pdfreader_core::NewAnnotation::Square(pdfreader_core::Rect::from_xywh(
                                80.0, 520.0, 200.0, 40.0,
                            )),
                        ]
                        .into_iter()
                        {
                            let effects =
                                self.store.dispatch(Command::AddAnnotation { page: 0, new });
                            self.execute(effects);
                        }
                    }

                    // New document: drop any in-flight requests from before.
                    self.engine.bump_generation();
                }
                EngineResponse::FormFields { doc, form } => {
                    // Debug hooks, so a mark can be captured without a mouse:
                    // tick the first checkbox (PDFREADER_CHECK_FIRST), select the
                    // first unselected radio (PDFREADER_SELECT_RADIO), or open
                    // the first text field for editing (PDFREADER_EDIT_FIELD).
                    // Fired once: the form is re-read after a painted write, and
                    // acting again would toggle the choice straight back.
                    let debug = (!self.debug_form_done).then(|| {
                        if std::env::var_os("PDFREADER_CHECK_FIRST").is_some() {
                            form.fields
                                .iter()
                                .find(|f| f.kind == pdfreader_core::FormFieldType::CheckBox)
                                .map(|f| (f.id, None))
                        } else if std::env::var_os("PDFREADER_SELECT_RADIO").is_some() {
                            form.fields
                                .iter()
                                .find(|f| {
                                    f.kind == pdfreader_core::FormFieldType::RadioButton
                                        && f.value == pdfreader_core::FieldValue::Checked(false)
                                })
                                .map(|f| (f.id, Some(pdfreader_core::FieldValue::Checked(true))))
                        } else if std::env::var_os("PDFREADER_EDIT_FIELD").is_some() {
                            form.fields
                                .iter()
                                .find(|f| f.kind == pdfreader_core::FormFieldType::Text)
                                .map(|f| (f.id, None))
                        } else {
                            None
                        }
                    });
                    let effects = self.store.dispatch(Command::FormFieldsLoaded { doc, form });
                    self.execute(effects);
                    if let Some(Some((id, value))) = debug {
                        self.debug_form_done = true;
                        let command = match value {
                            Some(value) => Command::SetFormFieldValue { id, value },
                            // No value to write: just open the field, which is
                            // what a click on it would do.
                            None => Command::SelectFormField(Some(id)),
                        };
                        let effects = self.store.dispatch(command);
                        self.execute(effects);
                    }
                }
                EngineResponse::FieldSet {
                    doc,
                    id,
                    written,
                    painted,
                    error,
                } => {
                    if painted {
                        // The value could not be recorded in the form model, so
                        // it was drawn into the page instead. Two things follow.
                        //
                        // The page content changed, so its raster is out of
                        // date: marking it stale re-renders in place, and the
                        // mark appears without a flash or a manual scroll.
                        self.canvas.invalidate_page(doc, id.page);
                        // And the fields it touched now report a value the model
                        // does not hold, so the form is re-read — the canvas has
                        // to know they are already on the page, or it would draw
                        // a second copy of the mark over the top of the painted
                        // one.
                        if let Some(handle) = self.handles.get(&doc).copied() {
                            self.engine.send(EngineRequest::LoadForm { doc, handle });
                        }
                    } else if !written {
                        let reason = error.as_deref().unwrap_or("unknown reason");
                        tracing::warn!(
                            "form field {id:?} in document {doc:?} was not written: {reason}"
                        );
                        // The reducer applied the value optimistically; a failed
                        // write means the document will not hold it, so say so
                        // rather than let a save drop the user's input.
                        self.save_error = Some(format!("Could not write field: {reason}"));
                    }
                }
                EngineResponse::Saved { doc, bytes } => {
                    if let Some(path) = self.save_target.take() {
                        match write_pdf_atomically(&path, &bytes) {
                            Ok(()) => {
                                self.save_error = None;
                                let effects = self.store.dispatch(Command::DocumentSaved { doc });
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
                    let effects = self.store.dispatch(Command::AnnotationsLoaded {
                        doc,
                        annotations,
                        created: None,
                    });
                    self.execute(effects);
                }
                EngineResponse::AnnotationAdded {
                    doc,
                    id,
                    annotations,
                } => {
                    let effects = self.store.dispatch(Command::AnnotationsLoaded {
                        doc,
                        annotations,
                        created: Some(id),
                    });
                    self.execute(effects);
                }
                EngineResponse::AnnotationsFailed { doc, reason } => {
                    tracing::warn!("annotation operation failed on {doc:?}: {reason}");
                    self.save_error = Some(format!("Annotation failed: {reason}"));
                    // If the list was never read, un-stick the Comments panel.
                    let effects = self.store.dispatch(Command::AnnotationsLoadFailed { doc });
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
                EngineResponse::PageText { doc, page, text } => {
                    self.page_text.insert(doc, page, text);
                }
                EngineResponse::PageTextFailed { doc, page, reason } => {
                    // A page PDFium could not read is stored as empty text, so
                    // it reads as "nothing selectable here" rather than being
                    // asked for again every frame.
                    self.page_text.failed(doc, page);
                    tracing::warn!("text extraction failed for page {page} ({reason})");
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
                EngineResponse::JobProgress { job, done, total } => {
                    // A late report from a job that was already cancelled must
                    // not resurrect its progress bar.
                    if let Some(running) = self.running_job.as_mut()
                        && running.id == job
                    {
                        running.progress = Some((done, total));
                    }
                }
                EngineResponse::JobDone {
                    job,
                    output,
                    pages,
                    target,
                } => {
                    // A report from a job that was already superseded must not
                    // clear the state of the job that replaced it.
                    if self.take_job(job).is_none() {
                        continue;
                    }
                    match target {
                        // The file is written; all that is left is to tell the
                        // user where it went.
                        ExportTarget::SaveFile => {
                            self.job_notice = Some(describe_output(&output, pages));
                        }
                        ExportTarget::Print => {
                            // The pages went straight to the printer; all that
                            // is left is to say so.
                            tracing::info!("sent {pages} page(s) to the printer");
                            self.job_notice = Some("Sent to the printer".to_string());
                        }
                    }
                }
                EngineResponse::JobCancelled { job } => {
                    // Cancelling is what the user asked for, so it is not worth
                    // a message: the progress bar simply goes away.
                    if self.take_job(job).is_some() {
                        tracing::info!("job {job} cancelled");
                    }
                }
                EngineResponse::JobFailed { job, reason } => {
                    if self.take_job(job).is_none() {
                        continue;
                    }
                    tracing::warn!("job {job} failed: {reason}");
                    self.save_error = Some(reason);
                }
            }
        }
        drained
    }

    /// Clear the running job if `job` is the one being tracked.
    ///
    /// Returns the job when it was, so a stale report from a superseded job
    /// cannot clear the state of the job that replaced it.
    fn take_job(&mut self, job: u64) -> Option<RunningJob> {
        if self.running_job.as_ref().is_some_and(|r| r.id == job) {
            self.running_job.take()
        } else {
            None
        }
    }

    /// Start a job, cancelling whatever was running before.
    ///
    /// Long jobs are not queued: the engine thread also serves tile rendering,
    /// so a second export would otherwise freeze the page for the duration of
    /// the first. Cancelling is honoured between items, so the old job stops
    /// promptly.
    fn begin_job(&mut self, label: String) -> (u64, Arc<AtomicBool>) {
        self.cancel_running_job();

        let id = self.engine.new_job();
        let cancel = Arc::new(AtomicBool::new(false));
        self.running_job = Some(RunningJob {
            id,
            label,
            cancel: Arc::clone(&cancel),
            progress: None,
        });

        (id, cancel)
    }

    /// Ask the running job to stop, if there is one.
    fn cancel_running_job(&mut self) {
        if let Some(running) = self.running_job.take() {
            running.cancel.store(true, Ordering::Relaxed);
        }
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
            // Ctrl+F focuses the find box, the same as Edit → Find….
            if ctrl && input.key_pressed(egui::Key::F) {
                commands.push(Command::FocusSearch);
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
            if input.key_pressed(egui::Key::Z) && input.modifiers.command {
                // Ctrl+Shift+Z is redo on every platform; Ctrl+Y is the
                // Windows spelling of the same thing.
                let can_undo = self
                    .store
                    .state()
                    .active()
                    .is_some_and(|tab| !tab.undo_stack.is_empty());
                let can_redo = self
                    .store
                    .state()
                    .active()
                    .is_some_and(|tab| !tab.redo_stack.is_empty());
                if input.modifiers.shift {
                    if can_redo {
                        commands.push(Command::Redo);
                    }
                } else if can_undo {
                    commands.push(Command::Undo);
                }
            }
            if ctrl && input.key_pressed(egui::Key::Y) {
                if self
                    .store
                    .state()
                    .active()
                    .is_some_and(|tab| !tab.redo_stack.is_empty())
                {
                    commands.push(Command::Redo);
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
                if self
                    .store
                    .state()
                    .active()
                    .is_some_and(|tab| tab.document.is_some())
                {
                    if input.modifiers.shift {
                        commands.push(Command::SaveDocumentAs);
                    } else {
                        commands.push(Command::SaveDocument);
                    }
                }
            }
            // Ctrl+P prints the whole document, the same as the File menu
            // entry. It needs a document, so it is a no-op on an empty window.
            if ctrl && input.key_pressed(egui::Key::P) {
                if self
                    .store
                    .state()
                    .active()
                    .is_some_and(|tab| tab.document.is_some())
                {
                    commands.push(Command::Print);
                }
            }
            // Ctrl+E reopens the export window on whichever job was used last,
            // so repeating an export does not mean walking the menu again. It
            // is deliberately not gated on an open document: two of the jobs
            // build a document from files and work fine on an empty window, and
            // for the rest the window itself says a document is needed. An
            // already-open window is left alone rather than reset, so a stray
            // keypress cannot discard settings the user is part-way through.
            if ctrl && input.key_pressed(egui::Key::E) && self.export_window.is_none() {
                commands.push(Command::ShowExportWindow(self.last_export_task));
            }
        });

        commands
    }

    /// Open a native file dialog on a worker thread.
    ///
    /// The dialog blocks, and blocking the UI thread is exactly what this
    /// architecture exists to avoid, so it runs elsewhere and reports back
    /// through a channel. One dialog at a time: a second request while one is
    /// open is dropped rather than stacked, because a modal window that appears
    /// behind another one looks like the application has hung.
    fn show_dialog(&mut self, intent: DialogIntent) {
        if self.dialog_intent.is_some() {
            return;
        }
        self.dialog_intent = Some(intent);

        let sender = self.dialog_tx.clone();
        let default_name = self.dialog_default_name(intent);

        if let Err(error) = std::thread::Builder::new()
            .name("file-dialog".to_string())
            .spawn(move || {
                let _ = sender.send(run_dialog(intent, default_name));
            })
        {
            tracing::error!("could not open a file dialog thread: {error}");
            self.dialog_intent = None;
        }
    }

    /// A sensible pre-filled file name for the dialog being opened.
    ///
    /// Only save dialogs take one, and only the two that save the document have
    /// a name worth guessing: the export window's own suggestion is already
    /// shown inside the window, so its picker opens unprefilled rather than
    /// second-guessing what the user is about to type.
    fn dialog_default_name(&self, intent: DialogIntent) -> Option<String> {
        match intent {
            DialogIntent::SavePdf => self.store.state().active().and_then(|tab| {
                tab.path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            }),
            DialogIntent::OpenPdf
            | DialogIntent::ExportOutputFile
            | DialogIntent::ExportOutputFolder
            | DialogIntent::ExportAddImages
            | DialogIntent::ExportAddPdfs => None,
        }
    }

    /// Route a dialog's answer to the command that acts on it.
    fn handle_dialog_result(
        &mut self,
        ctx: &egui::Context,
        intent: DialogIntent,
        result: DialogResult,
    ) {
        // The export window's pickers fill in the window rather than running
        // anything: the user is still choosing, and the window is where they
        // confirm. A cancelled picker changes nothing, which is why it needs no
        // special case here.
        if let Some(window) = self.export_window.as_mut() {
            match (intent, &result) {
                (
                    DialogIntent::ExportOutputFile | DialogIntent::ExportOutputFolder,
                    DialogResult::Path(path),
                ) => {
                    window.set_output(path.clone());
                }
                (
                    DialogIntent::ExportAddImages | DialogIntent::ExportAddPdfs,
                    DialogResult::Paths(paths),
                ) => {
                    window.add_sources(paths.clone());
                }
                _ => {}
            }
            if matches!(
                intent,
                DialogIntent::ExportOutputFile
                    | DialogIntent::ExportOutputFolder
                    | DialogIntent::ExportAddImages
                    | DialogIntent::ExportAddPdfs
            ) {
                return;
            }
        }

        match (intent, result) {
            (DialogIntent::OpenPdf, DialogResult::Path(path)) => {
                self.apply(ctx, vec![Command::OpenPath(path)]);
            }
            (DialogIntent::SavePdf, DialogResult::Path(path)) => {
                self.apply(ctx, vec![Command::SaveDocumentTo(path)]);
            }
            // A cancelled dialog, or an intent/result pairing that cannot
            // happen: nothing to do.
            _ => {}
        }
    }

    /// Open the export window on a task.
    ///
    /// A window that is already open is retargeted rather than replaced, so the
    /// list of files the user has already added survives picking a different
    /// task from the menu.
    fn open_export_window(&mut self, task: ExportTask) {
        self.last_export_task = task;
        let doc = OpenDocument::snapshot(self.store.state().active());

        match self.export_window.as_mut() {
            Some(window) => window.set_task(task, &doc),
            None => self.export_window = Some(ExportWindow::new(task, &doc)),
        }
    }

    /// Act on what the user did in the export window.
    fn handle_export_action(&mut self, ctx: &egui::Context, action: export_window::Action) {
        match action {
            export_window::Action::None => {}
            export_window::Action::Cancel => self.export_window = None,
            export_window::Action::Browse(field) => {
                // A picker needs the window still open to fill in, so it is not
                // closed here; `handle_dialog_result` puts the answer back.
                self.show_dialog(dialog_intent_for_field(field));
            }
            export_window::Action::Start(request) => {
                self.export_window = None;
                self.apply(ctx, vec![request.into_command()]);
            }
        }
    }

    /// Ask the engine for the page preview the export window shows.
    ///
    /// Only the document tasks have a page to show, and only the page in view:
    /// the window is about to export what the user was looking at. Reuses the
    /// sidebar's thumbnail cache, so opening the window on a page the strip has
    /// already drawn costs nothing.
    fn ensure_export_preview(&mut self, dpr: f32) {
        let Some(window) = self.export_window.as_ref() else {
            return;
        };
        if !window.task().needs_document() {
            return;
        }

        let Some(tab) = self.store.state().active() else {
            return;
        };
        let Some(document) = tab.document.as_ref() else {
            return;
        };
        let page = tab.view.current_page;

        // Already rasterized: nothing to ask for.
        if self
            .canvas
            .thumbnail(document.id, tab.view.rotation, page)
            .is_some()
        {
            return;
        }

        if let Some(thumb) = self.canvas.request_thumbnail(
            document,
            tab.view.rotation,
            page,
            export_window::PREVIEW_WIDTH,
            dpr,
        ) {
            self.request_thumbnails(vec![thumb]);
        }
    }

    /// Ask the engine for the page previews the print window shows.
    ///
    /// Reuses the sidebar's thumbnail cache, so a page the strip has already
    /// drawn costs nothing, and the canvas deduplicates in-flight requests, so
    /// asking for the whole selection every frame only ever sends the missing
    /// ones.
    fn ensure_print_previews(&mut self, dpr: f32) {
        let Some(window) = self.print_window.as_ref() else {
            return;
        };
        let Some(tab) = self.store.state().active() else {
            return;
        };
        let Some(document) = tab.document.as_ref() else {
            return;
        };
        let rotation = tab.view.rotation;
        let doc_id = document.id;
        let pages = window.preview_pages(document.page_count(), tab.view.current_page);

        let mut requests = Vec::new();
        for page in pages {
            if self.canvas.thumbnail(doc_id, rotation, page).is_some() {
                continue;
            }
            if let Some(thumb) = self.canvas.request_thumbnail(
                document,
                rotation,
                page,
                print_window::PREVIEW_WIDTH,
                dpr,
            ) {
                requests.push(thumb);
            }
        }
        if !requests.is_empty() {
            self.request_thumbnails(requests);
        }
    }

    /// Act on what the user did in the print window.
    fn handle_print_action(&mut self, action: print_window::Action) {
        match action {
            print_window::Action::None => {}
            print_window::Action::Cancel => self.print_window = None,
            print_window::Action::Print(pages) => {
                self.print_window = None;

                let active = self.store.state().active();
                let doc = active.and_then(|tab| tab.document.as_ref()).map(|d| d.id);
                let title = active
                    .and_then(|tab| tab.document.as_ref())
                    .map(|d| d.title.clone())
                    .unwrap_or_else(|| "Document".to_string());
                // The form as the reader is showing it, so the print path can
                // paint the values PDFium will not render.
                let fields = active
                    .and_then(|tab| tab.form.as_ref())
                    .map(|form| form.fields.clone())
                    .unwrap_or_default();
                let Some(handle) = doc.and_then(|doc| self.handles.get(&doc).copied()) else {
                    return;
                };

                match print::choose_printer() {
                    // Cancelled: printing is what the user changed their mind
                    // about, so it needs no message.
                    Ok(None) => {}
                    Err(reason) => self.save_error = Some(format!("Could not print: {reason}")),
                    Ok(Some(printer)) => {
                        let (job, cancel) = self.begin_job("Sending to the printer".to_string());
                        self.engine.send(EngineRequest::PrintPages {
                            handle,
                            pages,
                            printer,
                            doc_name: title,
                            fields,
                            job,
                            cancel,
                        });
                    }
                }
            }
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
            // The export window is drawn by the shell, so opening it is shell
            // work exactly like a native dialog: the reducer has nothing to
            // decide until the user commits to a set of settings.
            if let Command::ShowExportWindow(task) = command {
                self.open_export_window(task);
                continue;
            }
            if let Some(intent) = dialog_intent_for(&command) {
                self.show_dialog(intent);
                continue;
            }
            // The helper-tool toggles and the translate target are app-local
            // preferences, like the dictionaries path: the reducer has nothing
            // to decide, so the shell flips the flag and persists it here.
            match command {
                Command::ToggleLookup => {
                    self.lookup_enabled = !self.lookup_enabled;
                    if !self.lookup_enabled {
                        self.lens_popup.close();
                        self.canvas.clear_selection();
                    }
                    self.persist();
                    continue;
                }
                Command::ToggleSpeak => {
                    self.speak_enabled = !self.speak_enabled;
                    self.persist();
                    continue;
                }
                Command::ToggleAutoSpeak => {
                    self.auto_speak = !self.auto_speak;
                    self.persist();
                    continue;
                }
                Command::SetFormFont(family) => {
                    self.form_font = family;
                    self.sync_form_font();
                    self.persist();
                    continue;
                }
                Command::SetFormFontSize(size) => {
                    self.form_font_size = size.max(0.0);
                    self.sync_form_font();
                    self.persist();
                    continue;
                }
                Command::SetFormFontColor(red, green, blue) => {
                    self.form_font_color = [red, green, blue];
                    self.sync_form_font();
                    self.persist();
                    continue;
                }
                Command::SetFormFontBold(bold) => {
                    self.form_font_bold = bold;
                    self.sync_form_font();
                    self.persist();
                    continue;
                }
                Command::ToggleTranslate => {
                    self.translate_enabled = !self.translate_enabled;
                    self.persist();
                    continue;
                }
                Command::SetLensTarget(target) => {
                    if pdfreader_lens::is_valid_target(&target) {
                        self.lens_target = target.clone();
                        self.persist();
                    }
                    continue;
                }
                Command::ShowSettings => {
                    self.settings_window.open = true;
                    continue;
                }
                Command::ShowAbout => {
                    self.about_open = true;
                    continue;
                }
                Command::ShowShortcuts => {
                    self.shortcuts_open = true;
                    continue;
                }
                Command::FocusSearch => {
                    self.focus_search = true;
                    continue;
                }
                _ => {}
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
                // A fresh toggle cancels any transition still in flight.
                self.pending_viewport = None;
                if entering {
                    // Un-maximize first: a maximized window going borderless
                    // fullscreen is the known "not truly fullscreen" failure.
                    // The fullscreen command is deferred so winit can apply the
                    // un-maximize before it, which is what actually fixes the
                    // taskbar-still-showing case.
                    let maximized = ctx.input(|input| input.viewport().maximized);
                    self.pre_fullscreen_maximized = maximized;
                    if maximized == Some(true) {
                        ctx.send_viewport_cmd(ViewportCommand::Maximized(false));
                        self.pending_viewport = Some((0, ViewportCommand::Fullscreen(true)));
                    } else {
                        ctx.send_viewport_cmd(ViewportCommand::Fullscreen(true));
                    }
                } else {
                    ctx.send_viewport_cmd(ViewportCommand::Fullscreen(false));
                    if self.pre_fullscreen_maximized == Some(true) {
                        // Restore the maximized window once the fullscreen exit
                        // has settled, for the same reason as above.
                        self.pending_viewport = Some((0, ViewportCommand::Maximized(true)));
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

        // Apply the persisted interface scale before anything is laid out, so
        // the pixels-per-point read below already reflects it.
        if (ctx.zoom_factor() - self.ui_scale).abs() > 1e-4 {
            ctx.set_zoom_factor(self.ui_scale);
        }
        let dpr = ctx.pixels_per_point();

        // Open maximized. Winit does not always honour a creation-time request,
        // so it is repeated for the first few frames and then dropped — the
        // title bar and its window buttons stay visible either way, which is
        // the whole point of maximizing rather than going full screen.
        if self.start_maximized > 0 {
            self.start_maximized -= 1;
            ctx.send_viewport_cmd(ViewportCommand::Maximized(true));
            ctx.request_repaint();
        }

        // Send a viewport change held back so the change before it could settle
        // (see `pending_viewport`): un-maximizing and going fullscreen in the
        // same frame is what leaves the window fullscreen-sized with the
        // taskbar still showing. Waits until the viewport reports the expected
        // state, with a hard cap so it can never get stuck.
        if let Some((waited, command)) = self.pending_viewport.take() {
            let settled = match &command {
                ViewportCommand::Fullscreen(true) => {
                    ctx.input(|input| input.viewport().maximized != Some(true))
                }
                ViewportCommand::Maximized(true) => {
                    ctx.input(|input| input.viewport().fullscreen != Some(true))
                }
                _ => true,
            };
            if (settled && waited >= 1) || waited >= 3 {
                ctx.send_viewport_cmd(command);
            } else {
                self.pending_viewport = Some((waited + 1, command));
            }
            ctx.request_repaint();
        }

        // Snapshot the helper-tool preferences up front. They are read-only
        // during this frame, but `self` is borrowed for most of the draw, so a
        // local copy lets the menu and canvas closures read them without a
        // second borrow of `self`.
        let lookup_enabled = self.lookup_enabled;
        let speak_enabled = self.speak_enabled;
        let auto_speak = self.auto_speak;
        let translate_enabled = self.translate_enabled;
        let lens_target = self.lens_target.clone();
        let form_font = self.form_font.clone();
        let form_font_size = self.form_font_size;
        let form_font_color = self.form_font_color;
        // One bundle for both places that edit the typing font — the Forms
        // panel and the quick bar above the page.
        let text_style = TextStyle {
            family: &form_font,
            size: form_font_size,
            bold: self.form_font_bold,
            color: form_font_color,
        };
        // Edit → Find… asks the toolbar's find box to take focus. Consume the
        // flag here so the request lasts exactly one frame.
        let focus_search = self.focus_search;
        self.focus_search = false;

        // The dictionaries are read on a background thread. Take them the frame
        // they arrive, so a lookup stops answering "loading" as soon as it can.
        if let Some(rx) = self.lens_rx.take() {
            match rx.try_recv() {
                Ok(Ok(dictionaries)) => {
                    tracing::info!(entries = dictionaries.entry_count(), "dictionaries loaded");
                    self.lens = Some(dictionaries);
                }
                Ok(Err(reason)) => {
                    tracing::warn!("the dictionaries could not be loaded ({reason})");
                    self.lens_error = Some(reason);
                }
                Err(crossbeam_channel::TryRecvError::Empty) => self.lens_rx = Some(rx),
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.lens_error = Some("the dictionary thread stopped".to_owned());
                }
            }
        }

        // Debug: open the lookup card on a sample word so its layout can be
        // captured without a mouse (pair with PDFREADER_SCREENSHOT). Waits for
        // the dictionaries so the card shows a real answer.
        if let Some(word) = self.demo_popup.take() {
            match self
                .lens
                .as_ref()
                .map(|dict| lens::Answer::from_lookup(dict.lookup(&word)))
            {
                Some(answer) => self.lens_popup.open(
                    lens::LookupRequest {
                        page: 0,
                        text: word,
                        anchor: egui::pos2(280.0, 240.0),
                    },
                    answer,
                    false,
                ),
                None => self.demo_popup = Some(word),
            }
        }

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
            // More fields are borrowed here than read: everything the canvas
            // closure touches has to come through this destructuring, because
            // reaching for `self.…` inside the closure while it also captures
            // these mutable borrows is a second borrow of the same value.
            let App {
                store,
                canvas,
                page_text,
                lens_popup,
                handles,
                engine,
                lens,
                lens_error,
                scroll_to_page,
                scroll_to_point,
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

            // The document tab strip sits directly under the menu bar, above
            // the toolbar. egui consumes top panels in declaration order, so
            // the tabbar is declared first.
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
                        focus_search,
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
                                            text_style,
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

            // Right-hand tools rail. Side panels are consumed before the
            // central panel.
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
                    if store
                        .state()
                        .active()
                        .is_some_and(|tab| tab.document.is_some())
                    {
                        commands.extend(annotation_bar(ui, &palette, store.state(), text_style));
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
                                    page_text,
                                    tab.annotations.as_deref().unwrap_or(&[]),
                                    tab.view.selected_annotation,
                                );
                                tiles.extend(pending);

                                // The properties window for whichever
                                // annotation is selected on the canvas. Drawn
                                // in the same frame as the canvas so a fresh
                                // selection opens the panel immediately.
                                commands.extend(annotation_properties_window(
                                    ui.ctx(),
                                    &palette,
                                    tab.annotations.as_deref().unwrap_or(&[]),
                                    tab.view.selected_annotation,
                                ));

                                // Text for every page on screen, so a click can
                                // be hit-tested to a character. Extraction is
                                // the cheap part of PDFium's work and the cache
                                // asks only once per page, so this does not
                                // compete with tiles in any meaningful way.
                                if let Some(handle) = handles.get(&doc.id).copied() {
                                    for space in canvas.page_spaces() {
                                        if page_text.request(doc.id, space.index) {
                                            engine.send(EngineRequest::PageText {
                                                doc: doc.id,
                                                handle,
                                                page: space.index,
                                            });
                                        }
                                    }
                                }

                                // Resolve a click on the page into a form field
                                // selection — but only in Select mode, where a
                                // click cannot mean "place an annotation".
                                let mut field_hit = false;
                                let clicked = canvas
                                    .take_click()
                                    .filter(|_| store.state().tool.annotation_kind().is_none());
                                if let Some(point) = clicked {
                                    let hit =
                                        pdfreader_render::hit_page(canvas.page_spaces(), point)
                                            .and_then(|space| {
                                                let (x, y) = space.to_page(point);
                                                tab.form.as_ref()?.fields.iter().find(|f| {
                                                    f.id.page == space.index
                                                        && f.rect.contains(x, y)
                                                })
                                            })
                                            .cloned();
                                    field_hit = hit.is_some();
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
                                                            field.value
                                                                != FieldValue::Checked(true),
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
                                // A word or phrase picked off the page opens the
                                // lookup popup. A click that landed on a form
                                // field already means "select this field", so it
                                // does not also open the card. When word lookup
                                // is switched off in Settings, the pick is
                                // dropped so no card appears.
                                match canvas.take_lookup() {
                                    Some(request) if !field_hit && lookup_enabled => {
                                        let answer = match lens.as_ref() {
                                            Some(dictionaries) => lens::Answer::from_lookup(
                                                dictionaries.lookup(&request.text),
                                            ),
                                            None => lens_error.clone().map_or(
                                                lens::Answer::Loading,
                                                lens::Answer::Missing,
                                            ),
                                        };
                                        // Auto-speak only when the whole TTS
                                        // tool is on; the setting alone does
                                        // not override a disabled Speak button.
                                        lens_popup.open(
                                            request,
                                            answer,
                                            speak_enabled && auto_speak,
                                        );
                                    }
                                    Some(_) => canvas.clear_selection(),
                                    // A click that picked up no word — blank
                                    // paper, or a scanned page with no text
                                    // layer — dismisses the card rather than
                                    // leaving a stale answer on screen.
                                    None if clicked.is_some() => {
                                        canvas.clear_selection();
                                        lens_popup.close();
                                    }
                                    None => {}
                                }

                                commands.extend(canvas.take_edits());

                                // A comment click wants the exact noted area
                                // centred, not the top of its page: convert the
                                // PDF-space point to layout coords and set an
                                // absolute scroll offset for the next frame.
                                if let Some((page, (px, py))) = scroll_to_point.take() {
                                    if let Some(space) =
                                        canvas.page_spaces().iter().find(|sp| sp.index == page)
                                    {
                                        let (lx, ly) = space.to_screen_point((px, py));
                                        if let Some(size) = canvas_size {
                                            canvas.request_scroll_to(egui::Vec2::new(
                                                (lx - size.x / 2.0).max(0.0),
                                                (ly - size.y / 2.0).max(0.0),
                                            ));
                                        }
                                    }
                                }

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

        // The export window, drawn last so it sits above the panels. Its page
        // preview comes from the thumbnail cache, which is why the request is
        // issued here: one frame later the texture exists and the spinner is
        // replaced by the page.
        if self.export_window.is_some() {
            self.ensure_export_preview(dpr);

            let doc = OpenDocument::snapshot(self.store.state().active());
            let rotation = self
                .store
                .state()
                .active()
                .map_or(Rotation::None, |tab| tab.view.rotation);
            let preview = self
                .store
                .state()
                .active()
                .and_then(|tab| tab.document.as_ref().map(|document| document.id))
                .and_then(|id| self.canvas.thumbnail(id, rotation, doc.current_page));

            let action = match self.export_window.as_mut() {
                Some(window) => window.show(&ctx, &palette, &doc, preview),
                None => export_window::Action::None,
            };
            self.handle_export_action(&ctx, action);
        }

        // The print window, drawn last so it sits above the panels. Its page
        // previews come from the thumbnail cache, which is why the requests are
        // issued here: a frame later the textures exist and the spinners are
        // replaced by pages.
        if self.print_window.is_some() {
            self.ensure_print_previews(dpr);

            let (title, page_count, current_page, rotation, doc_id) =
                match self.store.state().active() {
                    Some(tab) => (
                        tab.document
                            .as_ref()
                            .map_or_else(|| "Document".to_string(), |d| d.title.clone()),
                        tab.page_count(),
                        tab.view.current_page,
                        tab.view.rotation,
                        tab.document.as_ref().map(|d| d.id),
                    ),
                    None => ("Document".to_string(), 0, 0, Rotation::None, None),
                };

            let preview_pages = self
                .print_window
                .as_ref()
                .map(|window| window.preview_pages(page_count, current_page))
                .unwrap_or_default();
            let previews: Vec<(u32, Option<egui::TextureHandle>)> = preview_pages
                .into_iter()
                .map(|page| {
                    let texture = doc_id
                        .and_then(|id| self.canvas.thumbnail(id, rotation, page))
                        .cloned();
                    (page, texture)
                })
                .collect();

            let action = match self.print_window.as_mut() {
                Some(window) => {
                    window.show(&ctx, &palette, &title, page_count, current_page, &previews)
                }
                None => print_window::Action::None,
            };
            self.handle_print_action(action);
        }

        // Status feedback: a failure, a job in progress, or a finished job.
        // One slot rather than a stack — at most one of these is interesting at
        // a time, and the newest is always the most relevant. Click a finished
        // message to dismiss it; a running job offers Cancel instead.
        let toast = self
            .save_error
            .as_ref()
            .map(|text| (ToastKind::Error, text.clone()))
            .or_else(|| {
                self.running_job
                    .as_ref()
                    .map(|job| (ToastKind::Busy, job.label.clone()))
            })
            .or_else(|| {
                self.job_notice
                    .as_ref()
                    .map(|text| (ToastKind::Success, text.clone()))
            });

        if let Some((kind, text)) = toast {
            let progress = self.running_job.as_ref().and_then(|job| job.progress);
            let mut dismiss = false;
            let mut cancel = false;

            egui::Area::new(egui::Id::new("status-toast"))
                .anchor(egui::Align2::CENTER_BOTTOM, egui::Vec2::new(0.0, -36.0))
                .show(&ctx, |ui| {
                    let stroke = match kind {
                        ToastKind::Error => palette.danger,
                        ToastKind::Busy | ToastKind::Success => palette.accent,
                    };
                    pdfreader_ui::toast_frame(&palette, stroke).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let message = match (kind, progress) {
                                (ToastKind::Busy, Some((done, total))) => {
                                    format!("{text}  ({done}/{total})")
                                }
                                _ => text.clone(),
                            };
                            ui.label(egui::RichText::new(message).color(palette.text).size(12.0));

                            if kind == ToastKind::Busy {
                                if ui
                                    .button(egui::RichText::new("Cancel").size(12.0))
                                    .clicked()
                                {
                                    cancel = true;
                                }
                            } else {
                                ui.label(
                                    egui::RichText::new("(click to dismiss)")
                                        .color(palette.text_dim)
                                        .size(11.0),
                                );
                            }
                        });
                    });

                    if kind != ToastKind::Busy {
                        ui.interact(
                            ui.min_rect(),
                            egui::Id::new("status-toast-dismiss"),
                            egui::Sense::click(),
                        )
                        .clicked()
                        .then(|| dismiss = true);
                    }
                });

            if cancel {
                self.cancel_running_job();
            }
            if dismiss {
                self.save_error = None;
                self.job_notice = None;
            }
        }

        // The lookup card. Drawn last as a top-level area so it sits above
        // every panel and is never clipped by the canvas or the scroll area.
        let popup_rect = self.lens_popup.show(
            &ctx,
            &palette,
            &lens_target,
            speak_enabled,
            translate_enabled,
        );

        // A floating card swallows the wheel — egui only scrolls a ScrollArea
        // when the pointer's top layer is that area, and the card sits above
        // the canvas. While the pointer is over the card, forward the
        // unconsumed wheel delta to the canvas so the document keeps scrolling
        // (selection intact), the way a browser behaves.
        if let Some(rect) = popup_rect {
            let over = ctx
                .input(|i| i.pointer.latest_pos())
                .is_some_and(|pos| rect.contains(pos));
            if over {
                let delta = ctx.input(|i| i.smooth_scroll_delta);
                if delta != egui::Vec2::ZERO {
                    ctx.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
                    let offset = self.canvas.scroll_offset();
                    self.canvas.request_scroll_to(offset - delta);
                }
            }
        }

        // The full Settings window (when open), the keyboard-shortcuts
        // reference, and the optional performance overlay — all above the
        // panels.
        self.draw_settings_window(&ctx, &palette);
        self.draw_shortcuts_window(&ctx, &palette);
        self.draw_about_window(&ctx, &palette);
        if self.show_stats {
            self.draw_stats_overlay(&ctx, &palette);
        }

        self.canvas_size = canvas_size;

        self.request_tiles(tiles);
        self.request_thumbnails(thumbs);
        self.apply(&ctx, commands);

        // Remember the open tabs and where each is left. Scrolling changes this
        // every frame, so the write is debounced; when one is still pending,
        // keep a frame coming so it lands even after the user stops moving.
        if self.persist_session(false) {
            ctx.request_repaint_after(SESSION_SAVE_INTERVAL);
        }

        // Keep the frame loop alive while tiles/thumbnails are still streaming
        // in, so pages fill in promptly instead of waiting for the next input
        // event (which would look like a freeze). Goes idle at 0% CPU otherwise.
        if drained || self.canvas.has_pending() {
            ctx.request_repaint();
        }

        self.frames_total += 1;
        if self.screenshot.is_some() {
            if !self.screenshot_sent && self.frames_total >= self.screenshot_frames {
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

    fn on_exit(&mut self) {
        // The last change may sit inside the debounce window; make the session
        // on disk match what was actually open.
        let _ = self.persist_session(true);
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
    index: usize,
) -> egui::Response {
    let id = ui.id().with(("search-card", result.page, index));

    // Hover is read from the previous frame's response: the card's own
    // rectangle is not known until its content has been laid out, and the
    // background has to be reserved before that happens.
    let hovered = ui
        .ctx()
        .read_response(id)
        .is_some_and(|response| response.hovered());

    // Reserved now, painted once the content has decided how tall the card is.
    // A fixed height used to clip the second line of a two-row snippet, which is
    // what a card with too little padding looks like.
    let background = ui.painter().add(egui::Shape::Noop);
    let border = ui.painter().add(egui::Shape::Noop);

    let inner = ui.scope_builder(egui::UiBuilder::new(), |ui| {
        ui.horizontal(|ui| {
            // Page badge: a fixed-width chip so the excerpts line up in a
            // column no matter how many digits the page number has.
            let (badge, _) = ui.allocate_exact_size(egui::vec2(30.0, 18.0), egui::Sense::hover());
            ui.painter().rect_filled(badge, 4.0, palette.accent_soft);
            ui.painter().text(
                badge.center(),
                egui::Align2::CENTER_CENTER,
                format!("{}", result.page + 1),
                egui::FontId::proportional(11.0),
                palette.accent,
            );
            ui.add_space(pdfreader_ui::SIDEBAR_GAP);

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
    });

    let rect = inner
        .response
        .rect
        .expand2(egui::vec2(pdfreader_ui::ROW_PAD_X, pdfreader_ui::ROW_PAD_Y));
    ui.painter().set(
        background,
        egui::Shape::rect_filled(
            rect,
            6.0,
            if hovered {
                palette.surface_hover
            } else {
                palette.surface
            },
        ),
    );
    ui.painter().set(
        border,
        egui::Shape::rect_stroke(
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
        ),
    );

    ui.interact(rect, id, egui::Sense::click())
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
    // Every line in this panel sits in the same content margin, so the query
    // box, the notices and the match count line up with each other instead of
    // the notices hugging the sidebar edge while the box is indented.
    let notice = |ui: &mut egui::Ui, text: egui::RichText| {
        egui::Frame::new()
            .inner_margin(pdfreader_ui::sidebar_note_margin())
            .show(ui, |ui| {
                ui.label(text);
            });
    };

    // The query field lives here as well as in the toolbar: the toolbar one is
    // easy to miss once you are reading the results, and re-running a search
    // should not mean hunting for the box again. It is the same input-group
    // control the toolbar draws, so the two are identical by construction; the
    // frame only adds the sidebar's own content margin around it.
    egui::Frame::new()
        .inner_margin(pdfreader_ui::sidebar_margin())
        .show(ui, |ui| {
            let width = ui.available_width().min(240.0);
            let outcome =
                pdfreader_ui::search_field(ui, palette, query, width, "Search document", false);
            if outcome.submitted {
                commands.push(Command::Search(query.trim().to_owned()));
            }
            if outcome.cleared {
                commands.push(Command::Search(String::new()));
            }
        });
    ui.add_space(pdfreader_ui::SIDEBAR_GAP);

    if query.is_empty() {
        notice(
            ui,
            egui::RichText::new("Type a word or phrase and press Enter.")
                .color(palette.text_dim)
                .size(12.0),
        );
        return;
    }
    if !current_document {
        notice(
            ui,
            egui::RichText::new("These results belong to another tab.")
                .color(palette.text_dim)
                .size(12.0),
        );
        return;
    }
    if in_progress {
        egui::Frame::new()
            .inner_margin(pdfreader_ui::sidebar_note_margin())
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(
                        egui::RichText::new("Searching pages…")
                            .color(palette.text_dim)
                            .size(12.0),
                    );
                });
            });
        return;
    }
    if let Some(error) = error {
        notice(
            ui,
            egui::RichText::new(error).color(palette.danger).size(12.0),
        );
        return;
    }
    if results.is_empty() {
        notice(
            ui,
            egui::RichText::new(format!("No results for “{query}”."))
                .color(palette.text_dim)
                .size(12.0),
        );
        return;
    }

    let total: usize = results.iter().map(|r| r.count).sum();
    egui::Frame::new()
        .inner_margin(pdfreader_ui::sidebar_note_margin())
        .show(ui, |ui| {
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
        });
    ui.add_space(pdfreader_ui::SIDEBAR_GAP);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(pdfreader_ui::sidebar_list_margin())
                .show(ui, |ui| {
                    for (index, result) in results.iter().enumerate() {
                        if search_result_card(ui, palette, query, result, index)
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            commands.push(Command::GoToPage(result.page));
                        }
                        ui.add_space(pdfreader_ui::SIDEBAR_GAP);
                    }
                });
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
    pdfreader_ui::dialog_window("Unsaved changes", palette)
        .resizable(false)
        .min_width(pdfreader_ui::DIALOG_MIN_WIDTH)
        .show(ctx, |ui| {
            ui.set_min_width(pdfreader_ui::DIALOG_MIN_WIDTH);
            ui.label(
                egui::RichText::new(format!(
                    "{} has changes that are not saved.",
                    path.display()
                ))
                .color(palette.text)
                .size(13.0),
            );
            ui.add_space(6.0);
            ui.label(pdfreader_ui::hint(
                palette,
                "Saving writes the document first, then closes the tab.",
            ));
            ui.add_space(14.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add(pdfreader_ui::primary_button(palette, "Save and close"))
                    .clicked()
                {
                    choice = Some(CloseChoice::SaveAndClose);
                }
                if ui
                    .add(pdfreader_ui::secondary_button(palette, "Discard changes"))
                    .clicked()
                {
                    choice = Some(CloseChoice::Discard);
                }
                if ui
                    .add(pdfreader_ui::secondary_button(palette, "Cancel"))
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
    pdfreader_ui::dialog_window("Password required", palette)
        .resizable(false)
        .min_width(pdfreader_ui::DIALOG_MIN_WIDTH)
        .show(ctx, |ui| {
            ui.set_min_width(pdfreader_ui::DIALOG_MIN_WIDTH);
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
                        .add(pdfreader_ui::secondary_button(palette, "Cancel"))
                        .clicked()
                    {
                        action = Some(PasswordAction::Cancel);
                    }
                    let enabled = !prompt.passphrase.is_empty();
                    if ui
                        .add_enabled(enabled, pdfreader_ui::primary_button(palette, "Unlock"))
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
    let frame_rect =
        egui::Rect::from_min_size(ui.cursor().min + offset, egui::vec2(card_w, card_h));

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

    pdfreader_ui::card_frame(palette)
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
                    .add(pdfreader_ui::primary_button(palette, "Open PDF…"))
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
        pdfreader_ui::card_frame(palette)
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
        pdfreader_ui::card_frame(palette)
            .stroke(egui::Stroke::new(1.0, palette.danger.gamma_multiply(0.55)))
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
                        .add(pdfreader_ui::secondary_button(palette, "Dismiss"))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_toggles_round_trip_through_settings() {
        let settings = Settings {
            theme: Some("dark".to_owned()),
            sidebar_visible: Some(true),
            lens_target: Some("zh-CN".to_owned()),
            lookup_enabled: Some(false),
            speak_enabled: Some(false),
            auto_speak: Some(false),
            translate_enabled: Some(true),
            ui_scale: Some(1.25),
            default_view_mode: Some("single".to_owned()),
            default_zoom: Some("fit-page".to_owned()),
            show_stats: Some(true),
            prefetch: Some(false),
            form_font: Some("NotoSansBengali-Regular".to_owned()),
            form_font_size: Some(14.0),
            form_font_color: Some([12, 34, 56]),
            form_font_bold: Some(true),
            open_maximized: Some(false),
        };
        let json = serde_json::to_string(&settings).expect("settings serialise");
        let back: Settings = serde_json::from_str(&json).expect("settings deserialise");
        assert_eq!(back.lookup_enabled, Some(false));
        assert_eq!(back.speak_enabled, Some(false));
        assert_eq!(back.auto_speak, Some(false));
        assert_eq!(back.translate_enabled, Some(true));
        assert_eq!(back.lens_target.as_deref(), Some("zh-CN"));
        assert_eq!(back.ui_scale, Some(1.25));
        assert_eq!(back.default_view_mode.as_deref(), Some("single"));
        assert_eq!(back.default_zoom.as_deref(), Some("fit-page"));
        assert_eq!(back.show_stats, Some(true));
        assert_eq!(back.prefetch, Some(false));
        assert_eq!(back.form_font.as_deref(), Some("NotoSansBengali-Regular"));
        assert_eq!(back.form_font_size, Some(14.0));
        assert_eq!(back.form_font_color, Some([12, 34, 56]));
        assert_eq!(back.form_font_bold, Some(true));
        assert_eq!(back.open_maximized, Some(false));
    }

    #[test]
    fn absent_toggles_read_as_defaults_not_off() {
        // A settings file written before these options existed must not read
        // back as "every tool off": an absent field is the default (on).
        let settings: Settings = serde_json::from_str("{}").expect("empty settings parse");
        assert_eq!(settings.lookup_enabled, None);
        assert!(settings.lookup_enabled.unwrap_or(true));
        assert!(settings.speak_enabled.unwrap_or(true));
        assert!(settings.auto_speak.unwrap_or(true));
        assert!(settings.translate_enabled.unwrap_or(true));
    }

    #[test]
    fn view_and_zoom_names_map_with_a_default_fallback() {
        assert_eq!(view_mode_from_name(Some("single")), ViewMode::Single);
        assert_eq!(
            view_mode_from_name(Some("continuous")),
            ViewMode::Continuous
        );
        assert_eq!(view_mode_from_name(None), ViewMode::Continuous);
        assert_eq!(view_mode_from_name(Some("nonsense")), ViewMode::Continuous);

        assert_eq!(zoom_mode_from_name(Some("fit-page")), ZoomMode::FitPage);
        assert_eq!(zoom_mode_from_name(Some("fit-width")), ZoomMode::FitWidth);
        assert_eq!(zoom_mode_from_name(Some("100")), ZoomMode::Fixed(1.0));
        assert_eq!(zoom_mode_from_name(None), ZoomMode::FitWidth);
        assert_eq!(zoom_mode_from_name(Some("nonsense")), ZoomMode::FitWidth);
    }
}
