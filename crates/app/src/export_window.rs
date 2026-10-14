//! The export window: one in-app panel for every export, conversion and merge.
//!
//! These operations used to be native file dialogs, and two of them needed two
//! dialogs in a row — pick the inputs, then pick the destination — with the
//! options fixed in advance. A native dialog cannot show a page preview, cannot
//! reorder a list, and cannot explain why a job is not ready, so the interesting
//! decisions were being made for the user instead of by them.
//!
//! The window is shell state, not domain state. Nothing here reaches the store
//! until the user commits: the window collects settings, and the single
//! [`Request`] it produces becomes one `Command` that the reducer turns into an
//! effect. That keeps the reducer free of transient widget state, which is the
//! whole reason the architecture has a shell layer at all.
//!
//! Everything that decides *what* will be exported — the range maths, the
//! readiness rules, the suggested file names — is a pure function on this type
//! so it can be unit tested without a window or a document.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use egui::{
    Align2, Color32, Context, FontId, Id, Pos2, Rect, RichText, Sense, Shape, Stroke, StrokeKind,
    Vec2,
};
use pdfreader_core::{
    Command, ExportDpi, ExportTask, ImageFormat, ImagePageSize, PageRange, Tab, pixel_size,
};
use pdfreader_ui::{
    Icon, Palette, card_frame, disabled_button, draw_icon, field_frame, primary_button,
    secondary_button, section,
};

/// Image extensions the reader can decode.
///
/// Kept in step with the `image` features the workspace enables: a codec added
/// there but not here would hide readable files from the picker.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg"];

/// Width of the whole window, in points.
///
/// Enlarged past the default Shortcuts window (880) so the export pane has room
/// for the task strip, the settings and the page preview side by side without
/// feeling cramped.
const WINDOW_WIDTH: f32 = 1140.0;

/// Preferred height of the task-strip / settings row.
///
/// The window is a modal sized by its content, so a short task (Page to image
/// is two sections) used to produce a squat window next to a tall one (Merge
/// PDFs, with the file list). Pinning the row's height keeps every task the
/// same shape.
///
/// It is a *preference*, not a promise: [`show`](ExportWindow::show) shrinks the
/// row when the viewport cannot hold it, so the header and the footer always
/// stay on screen.
const BODY_HEIGHT: f32 = 560.0;

/// Smallest the body is ever squeezed to.
///
/// Below this the pane stops being usable, so on a very short viewport the
/// window is allowed to reach the edge rather than collapse to nothing.
const MIN_BODY_HEIGHT: f32 = 240.0;

/// Margin kept between the window and the edge of the viewport, top and bottom.
const MODAL_MARGIN: f32 = 24.0;

/// Vertical room the footer block needs: the gap above it, its separator, the
/// space under that, and the button row.
const FOOTER_BLOCK: f32 = 16.0 + 1.0 + 12.0 + 40.0;

/// Width of the task strip on the left.
///
/// Wide enough for a two-line card — the task name over the sentence that says
/// what it makes — because the chooser is the first thing a user reads and a
/// bare name ("Pages to images") leaves them guessing what will come out.
const STRIP_WIDTH: f32 = 268.0;

/// Height of one card in the export chooser.
///
/// Fits the name plus two lines of description, which is what every task's
/// sentence needs at this width, with a little air around them.
const CARD_HEIGHT: f32 = 58.0;

/// Width reserved at the left of a chooser card for its pictogram.
const GLYPH_COLUMN: f32 = 40.0;

/// Height of the scrollable file list for the composition tasks.
///
/// Bounded rather than unbounded so a long stack cannot push the output
/// preview, the destination and the result card out of reach.
const LIST_HEIGHT: f32 = 240.0;

/// Height of one row in the ordered file stack.
const ROW_HEIGHT: f32 = 46.0;
/// Size of the drag handle at the head of a row.
const HANDLE_SIZE: f32 = 18.0;
/// Size of a row's icon button.
const BUTTON_SIZE: f32 = 22.0;
/// Size of the square preview box on a row.
const TILE_SIZE: f32 = 34.0;
/// Padding inside a row, left and right.
const ROW_PAD: f32 = 6.0;
/// Size of one tile in the output-order strip.
const PREVIEW_TILE_W: f32 = 34.0;
/// Height of one tile in the output-order strip, number included.
const PREVIEW_TILE_H: f32 = 44.0;

/// Size of the square the page preview is fitted into.
///
/// Public because the shell has to ask the engine for a thumbnail at least this
/// wide; keeping the two numbers in one place is what stops the preview from
/// arriving blurry after someone enlarges the box.
pub const PREVIEW_WIDTH: f32 = 224.0;

/// One file the user added to a composition task.
struct SourceEntry {
    /// Where the file is.
    path: PathBuf,
    /// What the window can say about it without opening it: the pixel size of
    /// an image, or the size on disk of a PDF. Read once when it is added,
    /// because reading it every frame would stat every file sixty times a
    /// second.
    detail: String,
}

/// What the window needs to know about the open document.
///
/// A plain snapshot rather than a `&Tab`: the window has no business reaching
/// into application state, and passing values keeps the borrow checker out of
/// the way when the shell renders the window and reads the store in the same
/// frame.
pub struct OpenDocument<'a> {
    /// Title of the open document, used for suggested file names.
    pub title: Option<&'a str>,
    /// Folder to suggest for output — the document's own folder when there is
    /// one, so an export lands next to the file it came from.
    pub folder: PathBuf,
    /// How many pages the document has, or 0 when nothing is open.
    pub pages: u32,
    /// Page currently in view, zero-based.
    pub current_page: u32,
    /// Size of the page in view, in points, when it is known.
    pub page_size: Option<(f32, f32)>,
}

impl<'a> OpenDocument<'a> {
    /// Take a snapshot of the tab in view, or of nothing when none is open.
    ///
    /// The one place that knows how to read a [`Tab`] into this shape, so the
    /// shell does not have to and the window never sees application state.
    pub fn snapshot(tab: Option<&'a Tab>) -> Self {
        let fallback = || std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        let Some(tab) = tab else {
            return Self {
                title: None,
                folder: fallback(),
                pages: 0,
                current_page: 0,
                page_size: None,
            };
        };

        let document = tab.document.as_ref();
        Self {
            title: document.map(|document| document.title.as_str()),
            // The document's own folder, so an export lands next to the file it
            // came from rather than in whatever directory the app was launched
            // from.
            folder: tab
                .path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map_or_else(fallback, Path::to_path_buf),
            pages: document.map_or(0, |document| {
                u32::try_from(document.pages.len()).unwrap_or(u32::MAX)
            }),
            current_page: tab.view.current_page,
            page_size: document
                .and_then(|document| {
                    document
                        .pages
                        .get(usize::try_from(tab.view.current_page).unwrap_or(0))
                })
                .map(|page| (page.width_pt, page.height_pt)),
        }
    }

    /// Whether a document is open at all.
    fn is_open(&self) -> bool {
        self.title.is_some()
    }

    /// Name stem for exported files.
    fn stem(&self) -> String {
        self.title.unwrap_or("document").to_string()
    }
}

/// A field a native picker fills in.
///
/// The pickers stay native — an in-app file browser would be a worse file
/// browser — but they now feed this window instead of ending the interaction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Field {
    /// The output file for a single-file task.
    OutputFile,
    /// The output directory for "every page as an image".
    OutputFolder,
    /// More images for "images to PDF".
    AddImages,
    /// More documents for "merge".
    AddPdfs,
}

/// What the window decided to run.
#[derive(Clone, PartialEq, Debug)]
pub enum Request {
    /// One page as one image file.
    PageImage {
        /// Destination file.
        path: PathBuf,
        /// Format to write.
        format: ImageFormat,
        /// Device pixels per PDF point.
        scale: f32,
    },
    /// Every page as its own image file.
    AllPages {
        /// Destination directory.
        dir: PathBuf,
        /// Format to write.
        format: ImageFormat,
        /// Device pixels per PDF point.
        scale: f32,
    },
    /// A page range as a standalone PDF.
    PagesPdf {
        /// Destination file.
        path: PathBuf,
        /// Pages to include.
        range: PageRange,
    },
    /// Images as the pages of a new PDF.
    ImagesToPdf {
        /// Images to place, one page each.
        images: Vec<PathBuf>,
        /// Destination file.
        output: PathBuf,
        /// How each page is sized.
        size: ImagePageSize,
    },
    /// Documents concatenated into one.
    MergePdfs {
        /// Documents to merge, in order.
        sources: Vec<PathBuf>,
        /// Destination file.
        output: PathBuf,
    },
}

impl Request {
    /// The command that carries this request to the reducer.
    pub fn into_command(self) -> Command {
        match self {
            Self::PageImage {
                path,
                format,
                scale,
            } => Command::ExportPageImage {
                path,
                format,
                scale,
            },
            Self::AllPages { dir, format, scale } => Command::ExportAllPages { dir, format, scale },
            Self::PagesPdf { path, range } => Command::ExportPagesPdf { path, range },
            Self::ImagesToPdf {
                images,
                output,
                size,
            } => Command::ImagesToPdf {
                images,
                output,
                size,
            },
            Self::MergePdfs { sources, output } => Command::MergePdfs { sources, output },
        }
    }
}

/// What the user did in the window.
pub enum Action {
    /// Nothing yet.
    None,
    /// Close without exporting.
    Cancel,
    /// Open a native picker and feed the answer back in.
    Browse(Field),
    /// Run the export.
    Start(Request),
}

/// The export window and everything the user has set in it.
pub struct ExportWindow {
    /// Which export is being set up.
    task: ExportTask,
    /// Format for the image tasks.
    format: ImageFormat,
    /// Resolution for the image tasks.
    dpi: ExportDpi,
    /// Page size for "images to PDF".
    page_size: ImagePageSize,
    /// First page to export, one-based, as typed. Empty means "from the start".
    range_first: String,
    /// Last page to export, one-based, as typed. Empty means "to the end".
    range_last: String,
    /// Files for the two composition tasks, in the order they will be used.
    sources: Vec<SourceEntry>,
    /// Where the output goes.
    ///
    /// Never empty: the window always has a suggestion, because a user who has
    /// to pick a destination before they can even see the options has been
    /// given an extra step for no reason. Browse replaces it.
    output: PathBuf,
    /// A picker the user asked for while the body was being drawn.
    ///
    /// The Browse buttons live several calls deep inside the task bodies, and
    /// threading a `&mut Action` down to each of them would mean every drawing
    /// helper taking one. Recording the request and reading it after the modal
    /// has been shown keeps the drawing functions about drawing.
    pending_browse: Option<Field>,
}

impl ExportWindow {
    /// Open the window for one task.
    ///
    /// The suggested destination is computed straight away so the common case —
    /// accept the default and press the button — is one click, not three.
    pub fn new(task: ExportTask, doc: &OpenDocument<'_>) -> Self {
        let mut window = Self {
            task,
            format: ImageFormat::default(),
            dpi: ExportDpi::default(),
            page_size: ImagePageSize::default(),
            range_first: String::new(),
            range_last: String::new(),
            sources: Vec::new(),
            output: PathBuf::new(),
            pending_browse: None,
        };
        window.output = window.suggested_output(doc);
        window
    }

    /// Which task the window is on.
    pub fn task(&self) -> ExportTask {
        self.task
    }

    /// Switch tasks, resetting the destination.
    ///
    /// The destination is not carried across: an export to a directory and an
    /// export to a file are different kinds of path, and keeping the old one
    /// would produce a request that cannot work.
    pub fn set_task(&mut self, task: ExportTask, doc: &OpenDocument<'_>) {
        if task == self.task {
            return;
        }
        self.task = task;
        self.output = self.suggested_output(doc);
        // The range belongs to the document, so it survives a task switch; the
        // file list does not, because the two composition tasks want different
        // kinds of file.
        if !task.needs_sources() {
            self.sources.clear();
        }
    }

    /// Record the file a picker returned.
    pub fn set_output(&mut self, path: PathBuf) {
        self.output = path;
    }

    /// Add files a picker returned, keeping the existing order.
    ///
    /// Duplicates are dropped: picking the same PDF twice is a slip, and a
    /// merged document that contains it twice is a worse outcome than ignoring
    /// the second pick.
    pub fn add_sources(&mut self, paths: Vec<PathBuf>) {
        for path in paths {
            if self.sources.iter().any(|entry| entry.path == path) {
                continue;
            }
            let detail = describe_source(&path);
            self.sources.push(SourceEntry { path, detail });
        }
    }

    /// Move one entry up or down the list.
    pub fn move_source(&mut self, index: usize, delta: isize) {
        let Some(target) = index.checked_add_signed(delta) else {
            return;
        };
        if index >= self.sources.len() || target >= self.sources.len() {
            return;
        }
        self.sources.swap(index, target);
    }

    /// Drop one entry.
    pub fn remove_source(&mut self, index: usize) {
        if index < self.sources.len() {
            self.sources.remove(index);
        }
    }

    /// Drop every entry.
    pub fn clear_sources(&mut self) {
        self.sources.clear();
    }

    /// Move one entry to a new position, sliding the rest along.
    ///
    /// The destination is the index the entry ends up at, not the gap it is
    /// dropped into: a drag from row 4 onto row 1 puts it first, which is what
    /// the gesture looks like it should do.
    pub fn move_source_to(&mut self, from: usize, to: usize) {
        if from == to || from >= self.sources.len() || to >= self.sources.len() {
            return;
        }
        let entry = self.sources.remove(from);
        self.sources.insert(to, entry);
    }

    /// Flip the list end to end.
    ///
    /// The usual fix for a stack of scans that arrived in the wrong direction,
    /// and the reason it is a button rather than something to do by hand.
    pub fn reverse_sources(&mut self) {
        self.sources.reverse();
    }

    /// Put the list in file-name order, case-insensitively.
    pub fn sort_sources_by_name(&mut self) {
        self.sources.sort_by_key(|entry| {
            entry
                .path
                .file_name()
                .map(|name| name.to_string_lossy().to_lowercase())
                .unwrap_or_default()
        });
    }

    /// The files in the list, in the order they will be used.
    pub fn source_paths(&self) -> Vec<&Path> {
        self.sources
            .iter()
            .map(|entry| entry.path.as_path())
            .collect()
    }

    /// The destination to pre-fill for the current task.
    ///
    /// `None` for a task that has no sensible guess, which leaves the Export
    /// button disabled until the user picks one — better than writing to a
    /// location they did not choose.
    fn suggested_output(&self, doc: &OpenDocument<'_>) -> PathBuf {
        let stem = doc.stem();
        let name = match self.task {
            ExportTask::PageImage => format!(
                "{stem}-page-{:04}.{}",
                doc.current_page + 1,
                self.format.extension()
            ),
            ExportTask::AllPageImages => format!("{stem}-pages"),
            ExportTask::PagePdf => format!("{stem}-pages.pdf"),
            ExportTask::ImagesToPdf => "images.pdf".to_string(),
            ExportTask::MergePdfs => "merged.pdf".to_string(),
        };
        doc.folder.join(name)
    }

    /// The range the user has typed, or why it cannot be used.
    ///
    /// One-based on both sides because that is how pages are numbered in the
    /// window, in the page box and in every other reader. The conversion to the
    /// zero-based [`PageRange`] happens here and nowhere else.
    fn typed_range(&self, doc: &OpenDocument<'_>) -> Result<PageRange, String> {
        if doc.pages == 0 {
            return Err("This document has no pages to export.".to_string());
        }

        let first = match self.range_first.trim() {
            "" => 1,
            text => parse_page(text)?,
        };
        let last = match self.range_last.trim() {
            "" => doc.pages,
            text => parse_page(text)?,
        };

        if last > doc.pages {
            return Err(format!(
                "This document has {} page{}, so page {last} does not exist.",
                doc.pages,
                if doc.pages == 1 { "" } else { "s" }
            ));
        }
        if first > last {
            return Err(format!(
                "Page {first} comes after page {last}; the first page cannot be later than the last."
            ));
        }
        Ok(PageRange::new(first - 1, last - 1))
    }

    /// What the window would run right now, or why it cannot run anything.
    ///
    /// The single source of truth for both the Export button and the message
    /// beside it, so the button can never be enabled while the message says
    /// something is wrong.
    fn readiness(&self, doc: &OpenDocument<'_>) -> Result<Request, String> {
        if self.task.needs_document() && !doc.is_open() {
            return Err(
                "Open a document first — this export reads the page on screen.".to_string(),
            );
        }
        if self.task.needs_sources() && self.sources.is_empty() {
            return Err(match self.task {
                ExportTask::MergePdfs => "Add at least two PDFs to merge.".to_string(),
                _ => "Add at least one image.".to_string(),
            });
        }
        // Merging one file is copying it, which the user almost certainly did
        // not mean, and the result would be indistinguishable from the input.
        if self.task == ExportTask::MergePdfs && self.sources.len() == 1 {
            return Err("Merging needs at least two PDFs.".to_string());
        }

        // The destination is always known: the window suggests one and Browse
        // replaces it, so there is no "you have not chosen yet" state to report.
        let output = self.output.clone();
        let scale = self.dpi.scale();
        Ok(match self.task {
            ExportTask::PageImage => Request::PageImage {
                path: output,
                format: self.format,
                scale,
            },
            ExportTask::AllPageImages => Request::AllPages {
                dir: output,
                format: self.format,
                scale,
            },
            ExportTask::PagePdf => Request::PagesPdf {
                path: output,
                range: self.typed_range(doc)?,
            },
            ExportTask::ImagesToPdf => Request::ImagesToPdf {
                images: self
                    .sources
                    .iter()
                    .map(|entry| entry.path.clone())
                    .collect(),
                output,
                size: self.page_size,
            },
            ExportTask::MergePdfs => Request::MergePdfs {
                sources: self
                    .sources
                    .iter()
                    .map(|entry| entry.path.clone())
                    .collect(),
                output,
            },
        })
    }

    /// The plain-language result of pressing the action button.
    ///
    /// Two strings: a headline saying what comes out ("12 image files"), and a
    /// detail line carrying the numbers that decide whether it is the *right*
    /// result — the pixel size, the format, the page size. It is shown while the
    /// options are still being set, so the outcome is known before the button is
    /// pressed rather than discovered after.
    fn outcome(&self, doc: &OpenDocument<'_>) -> (String, String) {
        match self.task {
            ExportTask::PageImage => {
                let detail = doc.page_size.map_or_else(
                    || format!("{} · {}", self.format.label(), self.dpi.label()),
                    |(width, height)| {
                        let (w, h) = pixel_size(width, height, self.dpi.scale());
                        format!(
                            "{w} × {h} px · {} · {}",
                            self.format.label(),
                            self.dpi.label()
                        )
                    },
                );
                (format!("1 {} image", self.format.label()), detail)
            }
            ExportTask::AllPageImages => {
                let detail = doc.page_size.map_or_else(
                    || format!("{} · {}", self.format.label(), self.dpi.label()),
                    |(width, height)| {
                        let (w, h) = pixel_size(width, height, self.dpi.scale());
                        format!(
                            "Each {w} × {h} px · {} · {}",
                            self.format.label(),
                            self.dpi.label()
                        )
                    },
                );
                let headline = if doc.pages == 1 {
                    "1 image file".to_string()
                } else {
                    format!("{} image files", doc.pages)
                };
                (headline, detail)
            }
            ExportTask::PagePdf => {
                let pages = self.typed_range(doc).map_or(0, PageRange::len);
                (
                    format!("A {pages}-page PDF"),
                    "Contains only the pages you chose.".to_string(),
                )
            }
            ExportTask::ImagesToPdf => (
                format!("A {}-page PDF", self.sources.len()),
                format!("One image per page · {}", self.page_size.label()),
            ),
            ExportTask::MergePdfs => (
                format!("A PDF from {} files", self.sources.len()),
                "Joined in the order shown above.".to_string(),
            ),
        }
    }

    /// The "what you'll get" card, drawn under the settings.
    ///
    /// A dot carries the ready / not-ready state, matching the status bar's own
    /// dot, so the card reads as a live summary rather than a static label.
    fn outcome_card(&self, ui: &mut egui::Ui, palette: &Palette, doc: &OpenDocument<'_>) {
        let (headline, detail) = self.outcome(doc);
        let ready = self.readiness(doc).is_ok();

        card_frame(palette)
            .inner_margin(egui::Margin::symmetric(12, 9))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                    ui.painter().circle_filled(
                        rect.center(),
                        3.5,
                        if ready {
                            palette.accent
                        } else {
                            palette.text_dim
                        },
                    );
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new("WHAT YOU'LL GET")
                            .color(palette.text_dim)
                            .size(10.0)
                            .strong(),
                    );
                });
                ui.add_space(4.0);
                ui.label(
                    RichText::new(headline)
                        .color(palette.text)
                        .size(14.0)
                        .strong(),
                );
                ui.add_space(2.0);
                ui.label(RichText::new(detail).color(palette.text_dim).size(11.0));
            });
    }

    /// Draw the window and report what the user did.
    ///
    /// `thumbs` holds the previews the shell has decoded for the files in the
    /// list, keyed by path. They arrive from outside because decoding is IO and
    /// this module does none — a file with no preview yet simply draws its type
    /// badge until one is ready.
    pub fn show(
        &mut self,
        ctx: &Context,
        palette: &Palette,
        doc: &OpenDocument<'_>,
        preview: Option<&egui::TextureHandle>,
        thumbs: &HashMap<PathBuf, egui::TextureHandle>,
    ) -> Action {
        let mut action = Action::None;

        // The shared window frame: the export pane is a window like Settings
        // and About Us, and it is the same surface for the same reason.
        let frame = pdfreader_ui::window_frame(palette);

        let modal = egui::Modal::new(egui::Id::new("export-window"))
            .frame(frame)
            .backdrop_color(Color32::from_black_alpha(120))
            .show(ctx, |ui| {
                ui.set_width(WINDOW_WIDTH);

                // The header is measured as it is drawn rather than guessed at,
                // so the body can be given exactly the room that is left.
                let top = ui.min_rect().top();
                self.header(ui, palette);
                ui.add_space(14.0);
                ui.separator();
                ui.add_space(14.0);
                let header_height = ui.min_rect().bottom() - top;

                // Cap the body to what the viewport can actually show once the
                // header, the footer and the window's own margin are accounted
                // for. The pane scrolls inside that cap, so the window never
                // grows past the screen and the header and footer never move.
                let budget = body_budget(ctx.content_rect().height(), header_height);

                egui::ScrollArea::vertical()
                    .id_salt("export-body")
                    .max_height(budget)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            ui.set_min_height(budget);
                            ui.vertical(|ui| {
                                ui.set_width(STRIP_WIDTH);
                                self.task_strip(ui, palette, doc);
                            });
                            ui.add_space(18.0);
                            ui.vertical(|ui| {
                                ui.set_width(ui.available_width());
                                self.body(ui, palette, doc, preview, thumbs);
                            });
                        });
                    });

                ui.add_space(16.0);
                ui.separator();
                ui.add_space(12.0);
                self.footer(ui, palette, doc, &mut action);
            });

        // Escape and a click on the backdrop both mean "never mind".
        if modal.should_close() {
            return Action::Cancel;
        }
        // A picker asked for while drawing outranks anything else: it is the
        // most recent thing the user did.
        if let Some(field) = self.pending_browse.take() {
            return Action::Browse(field);
        }
        action
    }

    /// Title, the sentence that says what the whole window does, and the
    /// banner for the task in view.
    ///
    /// The intro line is deliberately about the window rather than the task:
    /// a first-time user has to learn that one pane covers all five jobs
    /// before the task name under it means anything.
    fn header(&self, ui: &mut egui::Ui, palette: &Palette) {
        ui.label(
            RichText::new("Export")
                .color(palette.text)
                .size(19.0)
                .strong(),
        );
        ui.add_space(2.0);
        ui.label(
            RichText::new(
                "Turn pages into pictures, build a PDF from pictures, or join PDFs into one.",
            )
            .color(palette.text_dim)
            .size(12.0),
        );

        ui.add_space(12.0);

        // The task banner: what is selected, and what it produces. Carried in a
        // soft accent card so the current choice is unmistakable from the header
        // alone, without reading down to the highlighted card.
        card_frame(palette)
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(self.task.label())
                            .color(palette.accent)
                            .size(14.0)
                            .strong(),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(self.task.blurb())
                            .color(palette.text_dim)
                            .size(12.0),
                    );
                });
            });
    }

    /// The chooser down the left-hand side: one card per export.
    fn task_strip(&mut self, ui: &mut egui::Ui, palette: &Palette, doc: &OpenDocument<'_>) {
        step_header(ui, palette, "1", "Choose an export");
        ui.add_space(8.0);

        // Only one task can be switched to per frame, so the change is applied
        // after the loop rather than while the list is being drawn.
        let mut chosen = None;
        for task in ExportTask::ALL {
            // A task that reads the open document cannot be offered when there
            // is none: a disabled card that still shows what it would do beats
            // one that silently does nothing.
            let usable = !task.needs_document() || doc.is_open();
            if task_card(ui, palette, task, task == self.task, usable) {
                chosen = Some(task);
            }
            ui.add_space(6.0);
        }

        if let Some(task) = chosen {
            self.set_task(task, doc);
        }
    }

    /// The settings for the current task.
    fn body(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        doc: &OpenDocument<'_>,
        preview: Option<&egui::TextureHandle>,
        thumbs: &HashMap<PathBuf, egui::TextureHandle>,
    ) {
        step_header(ui, palette, "2", "Set the options");
        ui.add_space(8.0);
        match self.task {
            ExportTask::PageImage => self.image_body(ui, palette, doc, preview, true),
            ExportTask::AllPageImages => self.image_body(ui, palette, doc, preview, false),
            ExportTask::PagePdf => self.pdf_body(ui, palette, doc, preview),
            ExportTask::ImagesToPdf => self.images_body(ui, palette, doc, thumbs),
            ExportTask::MergePdfs => self.merge_body(ui, palette, doc, thumbs),
        }
    }

    /// Settings for the two image exports.
    ///
    /// They share a body because they share every setting; only the destination
    /// (one file or a directory) and the sentence describing the result differ.
    fn image_body(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        doc: &OpenDocument<'_>,
        preview: Option<&egui::TextureHandle>,
        single: bool,
    ) {
        ui.horizontal_top(|ui| {
            preview_pane(ui, palette, doc, preview, single);
            ui.add_space(20.0);
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());

                section(ui, palette, "FORMAT");
                ui.add_space(6.0);
                let formats: Vec<&str> = ImageFormat::ALL.iter().map(|f| f.label()).collect();
                let current = ImageFormat::ALL
                    .iter()
                    .position(|f| *f == self.format)
                    .unwrap_or(0);
                if let Some(index) = segmented(ui, palette, current, &formats) {
                    self.format = ImageFormat::ALL[index];
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new(match self.format {
                        ImageFormat::Png => "Lossless — best for text and line art",
                        ImageFormat::Jpeg => "Lossy but far smaller — best for photos and scans",
                    })
                    .color(palette.text_dim)
                    .size(11.0),
                );

                ui.add_space(16.0);
                section(ui, palette, "RESOLUTION");
                ui.add_space(6.0);
                let dpis: Vec<&str> = ExportDpi::ALL.iter().map(|d| d.label()).collect();
                let current = ExportDpi::ALL
                    .iter()
                    .position(|d| *d == self.dpi)
                    .unwrap_or(0);
                if let Some(index) = segmented(ui, palette, current, &dpis) {
                    self.dpi = ExportDpi::ALL[index];
                }
                ui.add_space(4.0);
                let scale = self.dpi.scale();
                let detail = doc.page_size.map_or_else(
                    || self.dpi.blurb().to_string(),
                    |(width, height)| {
                        let (pixels_wide, pixels_high) = pixel_size(width, height, scale);
                        format!("{} · {pixels_wide} × {pixels_high} px", self.dpi.blurb())
                    },
                );
                ui.label(RichText::new(detail).color(palette.text_dim).size(11.0));

                ui.add_space(16.0);
                self.output_row(ui, palette, doc.title);
                ui.add_space(14.0);
                self.outcome_card(ui, palette, doc);
            });
        });
    }

    /// Settings for "a page range as a new PDF".
    fn pdf_body(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        doc: &OpenDocument<'_>,
        preview: Option<&egui::TextureHandle>,
    ) {
        ui.horizontal_top(|ui| {
            preview_pane(ui, palette, doc, preview, true);
            ui.add_space(20.0);
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());

                section(ui, palette, "PAGES TO INCLUDE");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("From").color(palette.text_dim).size(12.0));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.range_first)
                            .desired_width(52.0)
                            .hint_text("1"),
                    );
                    ui.label(RichText::new("to").color(palette.text_dim).size(12.0));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.range_last)
                            .desired_width(52.0)
                            .hint_text(doc.pages.to_string()),
                    );
                    ui.add_space(6.0);
                    if ui
                        .add(egui::Button::new(RichText::new("All pages").size(12.0)))
                        .clicked()
                    {
                        self.range_first.clear();
                        self.range_last.clear();
                    }
                    if ui
                        .add(egui::Button::new(RichText::new("This page").size(12.0)))
                        .clicked()
                    {
                        self.range_first = (doc.current_page + 1).to_string();
                        self.range_last.clone_from(&self.range_first);
                    }
                });

                ui.add_space(6.0);
                // The range is reported back the way the engine will read it,
                // so "from 3 to 3" cannot be mistaken for three pages.
                let summary = match self.typed_range(doc) {
                    Ok(range) => format!(
                        "{} page{} will be copied into the new PDF",
                        range.len(),
                        if range.len() == 1 { "" } else { "s" }
                    ),
                    Err(problem) => problem,
                };
                ui.label(RichText::new(summary).color(palette.text_dim).size(11.0));

                ui.add_space(16.0);
                self.output_row(ui, palette, doc.title);
                ui.add_space(14.0);
                self.outcome_card(ui, palette, doc);
            });
        });
    }

    /// Settings for "images become the pages of a new PDF".
    fn images_body(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        doc: &OpenDocument<'_>,
        thumbs: &HashMap<PathBuf, egui::TextureHandle>,
    ) {
        section(ui, palette, "PAGE SIZE");
        ui.add_space(6.0);
        let labels: Vec<&str> = ImagePageSize::ALL.iter().map(|s| s.label()).collect();
        let current = ImagePageSize::ALL
            .iter()
            .position(|s| *s == self.page_size)
            .unwrap_or(0);
        if let Some(index) = segmented(ui, palette, current, &labels) {
            self.page_size = ImagePageSize::ALL[index];
        }
        ui.add_space(4.0);
        ui.label(
            RichText::new(self.page_size.blurb())
                .color(palette.text_dim)
                .size(11.0),
        );

        ui.add_space(16.0);
        section(ui, palette, "IMAGES — ONE PAGE EACH, IN THIS ORDER");
        ui.add_space(6.0);
        self.source_list(
            ui,
            palette,
            thumbs,
            Field::AddImages,
            "Add images…",
            "image",
        );
        ui.add_space(16.0);
        self.output_row(ui, palette, None);
        ui.add_space(14.0);
        self.outcome_card(ui, palette, doc);
    }

    /// Settings for "join several PDFs into one".
    fn merge_body(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        doc: &OpenDocument<'_>,
        thumbs: &HashMap<PathBuf, egui::TextureHandle>,
    ) {
        section(ui, palette, "PDFS TO JOIN — IN THIS ORDER");
        ui.add_space(6.0);
        self.source_list(ui, palette, thumbs, Field::AddPdfs, "Add PDFs…", "document");
        ui.add_space(16.0);
        self.output_row(ui, palette, None);
        ui.add_space(14.0);
        self.outcome_card(ui, palette, doc);
    }

    /// The ordered file stack shared by the two composition tasks.
    ///
    /// Order is the whole job here, so it gets three ways in: drag a row by its
    /// handle, nudge it with the arrows, or set the whole list at once with
    /// Reverse and Sort. The strip under the list shows the result of all three
    /// instead of leaving the user to imagine it.
    fn source_list(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        thumbs: &HashMap<PathBuf, egui::TextureHandle>,
        field: Field,
        add_label: &str,
        noun: &str,
    ) {
        ui.horizontal(|ui| {
            if ui.add(primary_button(palette, add_label)).clicked() {
                self.pending_browse = Some(field);
            }

            // The whole-list actions sit next to Add so they read as operations
            // on the stack rather than on the row the pointer happens to be on.
            let many = self.sources.len() > 1;
            if icon_button(ui, palette, Icon::Reverse, many)
                .on_hover_text("Reverse the order")
                .clicked()
            {
                self.reverse_sources();
            }
            if icon_button(ui, palette, Icon::SortName, many)
                .on_hover_text("Sort by file name")
                .clicked()
            {
                self.sort_sources_by_name();
            }
            if icon_button(ui, palette, Icon::Remove, !self.sources.is_empty())
                .on_hover_text("Remove every file")
                .clicked()
            {
                self.clear_sources();
            }

            ui.add_space(4.0);
            ui.label(
                RichText::new(count_label(self.sources.len(), noun))
                    .color(palette.text_dim)
                    .size(11.0),
            );
        });

        ui.add_space(6.0);

        // The row actions are collected rather than applied: the list cannot be
        // mutated while it is being drawn, because every row reads the entries
        // around it.
        let mut actions = ListActions::default();

        card_frame(palette)
            .inner_margin(egui::Margin::same(6))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("export-source-list")
                    .max_height(LIST_HEIGHT)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        draw_source_rows(ui, palette, &self.sources, thumbs, noun, &mut actions);
                    });
            });

        if let Some(index) = actions.remove {
            self.remove_source(index);
        }
        if let Some(index) = actions.move_up {
            self.move_source(index, -1);
        }
        if let Some(index) = actions.move_down {
            self.move_source(index, 1);
        }
        if let Some((from, to)) = actions.reorder {
            self.move_source_to(from, to);
        }

        if !self.sources.is_empty() {
            ui.add_space(12.0);
            let caption = if field == Field::AddImages {
                format!(
                    "{} — one page each",
                    count_label(self.sources.len(), "image")
                )
            } else {
                format!(
                    "{} joined in this order",
                    count_label(self.sources.len(), "document")
                )
            };
            order_preview(ui, palette, &self.sources, thumbs, &caption);
        }
    }

    /// The destination row, labelled for what the task actually writes.
    ///
    /// `stem` is the document the suggestion was derived from, so the window can
    /// say where the guess came from. The composition tasks have no document to
    /// name and pass `None`.
    fn output_row(&mut self, ui: &mut egui::Ui, palette: &Palette, stem: Option<&str>) {
        let folder = self.task.writes_a_directory();
        section(
            ui,
            palette,
            if folder {
                "WRITE THE PAGES INTO"
            } else {
                "SAVE AS"
            },
        );
        ui.add_space(6.0);

        let mut browse = false;
        ui.horizontal(|ui| {
            let width = ui.available_width() - 96.0;
            path_field(ui, palette, &self.output, folder, width);
            if ui.add(secondary_button(palette, "Browse…")).clicked() {
                browse = true;
            }
        });

        if browse {
            self.pending_browse = Some(if folder {
                Field::OutputFolder
            } else {
                Field::OutputFile
            });
        }

        // A default is a guess, so it is labelled as one and can be taken back.
        if let Some(stem) = stem {
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!("Suggested from “{stem}” — Browse to change it."))
                    .color(palette.text_dim)
                    .size(11.0),
            );
        }
    }

    /// The bottom bar: why the task cannot run, and the two buttons.
    fn footer(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        doc: &OpenDocument<'_>,
        action: &mut Action,
    ) {
        let readiness = self.readiness(doc);

        ui.horizontal(|ui| {
            let mut cancel = false;
            if ui.add(secondary_button(palette, "Cancel")).clicked() {
                cancel = true;
            }

            // The message is the reason the button is off, or the summary of
            // what pressing it will do.
            let (message, colour) = match &readiness {
                Err(problem) => (problem.clone(), palette.danger),
                Ok(request) => (describe_request(request), palette.text_dim),
            };
            ui.add_space(pdfreader_ui::FOOTER_GAP);
            ui.label(RichText::new(message).color(colour).size(11.5));

            let enabled = readiness.is_ok();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let button = if enabled {
                    primary_button(palette, self.task.action_label())
                } else {
                    disabled_button(palette, self.task.action_label())
                };

                if ui.add_enabled(enabled, button).clicked()
                    && let Ok(request) = readiness
                {
                    *action = Action::Start(request);
                }
            });

            if cancel {
                *action = Action::Cancel;
            }
        });
    }
}

/// A one-line description of what a request will produce.
fn describe_request(request: &Request) -> String {
    match request {
        Request::PageImage { path, .. } => format!("Writes {}", file_label(path)),
        Request::AllPages { dir, format, .. } => {
            format!(
                "Writes one {} per page into {}",
                format.label(),
                dir.display()
            )
        }
        Request::PagesPdf { range, path } => format!(
            "{} page{} → {}",
            range.len(),
            if range.len() == 1 { "" } else { "s" },
            file_label(path)
        ),
        Request::ImagesToPdf { images, output, .. } => format!(
            "{} image{} → {}",
            images.len(),
            if images.len() == 1 { "" } else { "s" },
            file_label(output)
        ),
        Request::MergePdfs {
            sources, output, ..
        } => format!("{} documents → {}", sources.len(), file_label(output)),
    }
}

/// The file name of a path, for a message that has no room for the directory.
fn file_label(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// Parse a page number the user typed.
fn parse_page(text: &str) -> Result<u32, String> {
    match text.trim().parse::<u32>() {
        Ok(0) => Err("Pages are numbered from 1.".to_string()),
        Ok(page) => Ok(page),
        Err(_) => Err(format!("“{}” is not a page number.", text.trim())),
    }
}

/// What the window can say about a file without opening it.
fn describe_source(path: &Path) -> String {
    if is_image(path) {
        return match image::image_dimensions(path) {
            Ok((width, height)) => format!("{width} × {height} px"),
            Err(_) => "not a readable image".to_string(),
        };
    }

    match std::fs::metadata(path) {
        Ok(metadata) => human_size(metadata.len()),
        Err(_) => "missing".to_string(),
    }
}

/// Whether a path looks like an image the application can decode.
fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            IMAGE_EXTENSIONS
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
}

/// A file size in the shortest unit that keeps it readable.
///
/// `u64` to `f64` loses precision past 2^53 bytes, which is 9 petabytes: a file
/// that large cannot be described in a list row anyway.
#[allow(clippy::cast_precision_loss)]
fn human_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let bytes_f = bytes as f64;
    if bytes_f < KIB {
        return format!("{bytes} B");
    }
    if bytes_f < KIB * KIB {
        return format!("{:.0} KB", bytes_f / KIB);
    }
    format!("{:.1} MB", bytes_f / (KIB * KIB))
}

/// "3 images", with the noun pluralised.
fn count_label(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// A row of mutually exclusive choices.
///
/// Returns the index the user picked, if any. Built by hand rather than with
/// `SelectableLabel` so the selected option reads as a filled pill, which is
/// legible at this size in all seven themes.
fn segmented(
    ui: &mut egui::Ui,
    palette: &Palette,
    current: usize,
    labels: &[&str],
) -> Option<usize> {
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (index, label) in labels.iter().enumerate() {
            let selected = index == current;
            let text = RichText::new(*label).size(12.0).color(if selected {
                palette.text
            } else {
                palette.text_dim
            });
            let button = egui::Button::new(text)
                .fill(if selected {
                    palette.accent_soft
                } else {
                    palette.surface
                })
                .corner_radius(palette.radius);
            if ui.add(button).clicked() {
                picked = Some(index);
            }
        }
    });
    picked
}

/// The vertical room the body may use, given the viewport and the measured
/// header.
///
/// The whole point is that the window is a *modal sized by its content*, so a
/// tall task would otherwise push the header and the footer off the screen —
/// which is exactly what happened once the file stack, its preview and the
/// result card were added to the same column. Capping the body and letting it
/// scroll inside the cap keeps the two things a user always needs — what this
/// window is, and the button that finishes it — pinned in place.
///
/// Split out as a pure function so the rule is testable without a window.
fn body_budget(viewport_height: f32, header_height: f32) -> f32 {
    (viewport_height - 2.0 * MODAL_MARGIN - header_height - FOOTER_BLOCK)
        .clamp(MIN_BODY_HEIGHT, BODY_HEIGHT)
}

/// A numbered step heading: a filled badge with the step number and a title.
///
/// The export pane is a short flow — choose, set, save — so numbering the two
/// halves tells a first-time user where to start and that the right-hand column
/// is a consequence of the left, not a second, independent choice.
fn step_header(ui: &mut egui::Ui, palette: &Palette, number: &str, title: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(19.0), Sense::hover());
        ui.painter()
            .circle_filled(rect.center(), 9.5, palette.accent_soft);
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            number,
            FontId::proportional(11.0),
            palette.accent,
        );
        ui.add_space(3.0);
        ui.label(RichText::new(title).strong().color(palette.text).size(13.0));
    });
}

/// One card in the export chooser: a pictogram, the task name, and the sentence
/// saying what it makes.
///
/// Larger than the plain row it replaced because the chooser is the first thing
/// a user reads: a name alone ("Pages to images") leaves them guessing, while
/// the line under it says exactly what will come out. The whole card is the hit
/// target, so there is no small label to aim at.
fn task_card(
    ui: &mut egui::Ui,
    palette: &Palette,
    task: ExportTask,
    selected: bool,
    enabled: bool,
) -> bool {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, CARD_HEIGHT), Sense::click());
    let response = response.on_hover_cursor(if enabled {
        egui::CursorIcon::PointingHand
    } else {
        egui::CursorIcon::Default
    });

    let fill = if selected {
        palette.accent_soft
    } else if response.hovered() && enabled {
        palette.surface_hover
    } else {
        palette.surface
    };
    let radius = pdfreader_ui::card_radius(palette);
    ui.painter().rect_filled(rect, radius, fill);
    // A hairline keeps the card edge visible on themes where `surface` and
    // `panel_bg` are close; it brightens to the accent on the chosen card.
    ui.painter().rect_stroke(
        rect,
        radius,
        Stroke::new(
            1.0,
            if selected {
                palette.accent
            } else {
                palette.border
            },
        ),
        StrokeKind::Inside,
    );
    if selected {
        let bar = Rect::from_min_size(
            rect.min + Vec2::new(0.0, 6.0),
            Vec2::new(3.0, rect.height() - 12.0),
        );
        ui.painter().rect_filled(bar, 2.0, palette.accent);
    }

    let ink = match (enabled, selected) {
        (false, _) => palette.text_dim.gamma_multiply(0.6),
        (true, _) => palette.text,
    };

    // Pictogram, centred in its own column.
    task_glyph(
        ui.painter(),
        rect.min + Vec2::new(GLYPH_COLUMN / 2.0, rect.height() / 2.0),
        task,
        if enabled { palette.accent } else { ink },
    );

    // Text column, laid out with wrapping so a long sentence stays inside the
    // card instead of running under the next one.
    let text_left = rect.min.x + GLYPH_COLUMN + 2.0;
    let wrap = (rect.max.x - text_left - 12.0).max(48.0);
    let title =
        ui.painter()
            .layout_no_wrap(task.label().to_owned(), FontId::proportional(12.5), ink);
    let desc = ui.painter().layout(
        task.blurb().to_owned(),
        FontId::proportional(10.5),
        palette
            .text_dim
            .gamma_multiply(if enabled { 1.0 } else { 0.6 }),
        wrap,
    );
    let title_height = title.size().y;
    let content_height = title_height + 2.0 + desc.size().y;
    let mut y = rect.center().y - content_height / 2.0;
    let painter = ui.painter().with_clip_rect(rect);
    painter.galley(Pos2::new(text_left, y), title, ink);
    y += title_height + 2.0;
    painter.galley(Pos2::new(text_left, y), desc, palette.text_dim);

    enabled && response.clicked()
}

/// A small pictogram for one export task, drawn with the painter so it scales
/// with the UI and needs no icon font.
///
/// Each glyph is the shape of the job — a page, several pages, a folded page, a
/// photo becoming a page, two pages joining — and is only ever two or three
/// primitives, so it stays legible at 20 px.
fn task_glyph(painter: &egui::Painter, center: Pos2, task: ExportTask, color: Color32) {
    let stroke = Stroke::new(1.3, color);
    let page = |offset: Vec2| Rect::from_center_size(center + offset, Vec2::new(11.0, 15.0));

    match task {
        // One page with a picture on it.
        ExportTask::PageImage => {
            let sheet = page(Vec2::ZERO);
            painter.rect_stroke(sheet, 2.0, stroke, StrokeKind::Inside);
            photo_mark(painter, sheet, color);
        }
        // A page holding several pictures, stacked.
        ExportTask::AllPageImages => {
            let sheet = page(Vec2::ZERO);
            painter.rect_stroke(sheet, 2.0, stroke, StrokeKind::Inside);
            for offset in [0.0_f32, 4.6] {
                let mark = Rect::from_min_size(
                    sheet.min + Vec2::new(3.0, 3.5 + offset),
                    Vec2::new(5.0, 3.4),
                );
                painter.rect_filled(mark, 0.8, color);
            }
        }
        // A page with a folded corner and a text band: a document.
        ExportTask::PagePdf => {
            let sheet = page(Vec2::ZERO);
            painter.rect_stroke(sheet, 2.0, stroke, StrokeKind::Inside);
            let fold = 4.5;
            painter.add(Shape::convex_polygon(
                vec![
                    Pos2::new(sheet.right() - fold, sheet.top()),
                    Pos2::new(sheet.right(), sheet.top() + fold),
                    Pos2::new(sheet.right() - fold, sheet.top() + fold),
                ],
                color,
                Stroke::NONE,
            ));
            let band = Rect::from_min_size(
                sheet.min + Vec2::new(2.5, sheet.height() * 0.58),
                Vec2::new(sheet.width() - 5.0, 1.6),
            );
            painter.rect_filled(band, 0.8, color);
        }
        // A photo on the left turning into a page on the right.
        ExportTask::ImagesToPdf => {
            let photo = Rect::from_center_size(center + Vec2::new(-6.5, 1.0), Vec2::splat(7.0));
            painter.rect_stroke(photo, 1.5, stroke, StrokeKind::Inside);
            painter.circle_filled(photo.center() - Vec2::new(1.2, 1.2), 1.0, color);
            let y = center.y + 1.0;
            painter.line_segment(
                [Pos2::new(center.x - 1.4, y), Pos2::new(center.x + 2.2, y)],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(center.x + 2.2, y),
                    Pos2::new(center.x + 0.7, y - 1.4),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(center.x + 2.2, y),
                    Pos2::new(center.x + 0.7, y + 1.4),
                ],
                stroke,
            );
            let sheet = Rect::from_center_size(center + Vec2::new(7.0, 0.0), Vec2::new(8.0, 11.0));
            painter.rect_stroke(sheet, 1.5, stroke, StrokeKind::Inside);
        }
        // Two pages with a plus between them: joining.
        ExportTask::MergePdfs => {
            let left = Rect::from_center_size(center + Vec2::new(-6.5, 0.0), Vec2::new(8.0, 11.0));
            let right = Rect::from_center_size(center + Vec2::new(6.5, 0.0), Vec2::new(8.0, 11.0));
            painter.rect_stroke(left, 1.5, stroke, StrokeKind::Inside);
            painter.rect_stroke(right, 1.5, stroke, StrokeKind::Inside);
            let plus = Stroke::new(1.6, color);
            let s = 2.0;
            painter.line_segment(
                [
                    Pos2::new(center.x, center.y - s),
                    Pos2::new(center.x, center.y + s),
                ],
                plus,
            );
            painter.line_segment(
                [
                    Pos2::new(center.x - s, center.y),
                    Pos2::new(center.x + s, center.y),
                ],
                plus,
            );
        }
    }
}

/// The little "photo" mark inside a page glyph: a sun and a hill.
///
/// Shared by the page glyphs so a picture reads the same wherever it appears,
/// and drawn inside the sheet's lower half so it never collides with the fold.
fn photo_mark(painter: &egui::Painter, sheet: Rect, color: Color32) {
    let area = Rect::from_min_max(
        sheet.min + Vec2::new(2.5, sheet.height() * 0.42),
        sheet.max - Vec2::new(2.5, 2.5),
    );
    if area.width() <= 2.0 || area.height() <= 2.5 {
        return;
    }
    painter.circle_filled(
        area.left_top() + Vec2::new(area.width() * 0.26, area.height() * 0.26),
        1.1,
        color,
    );
    painter.add(Shape::convex_polygon(
        vec![
            Pos2::new(area.left(), area.bottom()),
            Pos2::new(area.center().x, area.top() + area.height() * 0.34),
            Pos2::new(area.right(), area.bottom()),
        ],
        color,
        Stroke::NONE,
    ));
}

/// The row controls the user pressed this frame.
///
/// Collected rather than applied so the list can be read while it is drawn.
#[derive(Default)]
struct ListActions {
    /// Index of the row whose remove button was pressed.
    remove: Option<usize>,
    /// Index of the row that asked to move up.
    move_up: Option<usize>,
    /// Index of the row that asked to move down.
    move_down: Option<usize>,
    /// A drag that was just released: the row it started on and where it landed.
    reorder: Option<(usize, usize)>,
}

/// Draw the ordered stack, empty state included.
fn draw_source_rows(
    ui: &mut egui::Ui,
    palette: &Palette,
    sources: &[SourceEntry],
    thumbs: &HashMap<PathBuf, egui::TextureHandle>,
    noun: &str,
    actions: &mut ListActions,
) {
    if sources.is_empty() {
        empty_stack(ui, palette, noun);
        return;
    }

    let last = sources.len() - 1;
    let mut row_rects: Vec<Rect> = Vec::with_capacity(sources.len());

    // One drop zone around the whole stack. The rows themselves are transparent
    // until hovered, so this zone supplies the list surface and turns to the
    // accent while a row is held over it — which is the whole "drop here" signal.
    let (_, dropped) = ui.dnd_drop_zone::<usize, _>(egui::Frame::NONE, |ui| {
        for (index, entry) in sources.iter().enumerate() {
            row_rects.push(source_row(
                ui,
                palette,
                index,
                entry,
                thumbs.get(&entry.path),
                index > 0,
                index < last,
                actions,
            ));
        }
    });

    if let (Some(from), Some(pointer)) = (dropped, ui.ctx().pointer_interact_pos())
        && let Some(to) = drop_target(&row_rects, pointer.y, *from)
    {
        actions.reorder = Some((*from, to));
    }
}

/// Where a row released at `pointer_y` should land.
///
/// The pointer is compared against row centres: releasing above a row's middle
/// means "before it", below means "after it". The gap index that falls out is
/// one too high for a downward move, because the entry is still occupying its
/// old slot while the pointer is being measured.
fn drop_target(rects: &[Rect], pointer_y: f32, from: usize) -> Option<usize> {
    if rects.is_empty() {
        return None;
    }
    let slot = rects
        .iter()
        .position(|rect| pointer_y < rect.center().y)
        .unwrap_or(rects.len());
    let to = if slot > from { slot - 1 } else { slot };
    Some(to.min(rects.len() - 1))
}

/// The empty state: what the stack is for, and the one gesture it supports.
fn empty_stack(ui: &mut egui::Ui, palette: &Palette, noun: &str) {
    ui.add_space(16.0);
    ui.vertical_centered(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::hover());
        draw_icon(
            ui.painter(),
            rect.center(),
            if noun == "image" {
                Icon::ImageFile
            } else {
                Icon::PdfFile
            },
            palette.text_dim.gamma_multiply(0.75),
        );
        ui.add_space(6.0);
        ui.label(
            RichText::new(format!("No {noun}s yet."))
                .color(palette.text_dim)
                .size(12.0),
        );
        ui.add_space(3.0);
        ui.label(
            RichText::new("Add some above, then drag the rows to set the order.")
                .color(palette.text_dim)
                .size(11.0),
        );
    });
    ui.add_space(16.0);
}

/// One row of the ordered stack: handle, position, preview, name, controls.
///
/// Returns the rect the row occupied, which the drop-zone hit test needs.
fn source_row(
    ui: &mut egui::Ui,
    palette: &Palette,
    index: usize,
    entry: &SourceEntry,
    thumb: Option<&egui::TextureHandle>,
    can_move_up: bool,
    can_move_down: bool,
    actions: &mut ListActions,
) -> Rect {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), Sense::hover());
    let radius = pdfreader_ui::card_radius(palette);

    // Transparent until the pointer is on it, so the drop zone's own fill — and
    // its drag-over highlight — shows through.
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, radius, palette.surface_hover);
    }
    ui.painter().rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, palette.border),
        StrokeKind::Inside,
    );

    let content = rect.shrink2(Vec2::new(ROW_PAD, 0.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| {
        ui.horizontal_centered(|ui| {
            // The handle is the only part of the row that starts a drag, so the
            // buttons beside it keep their clicks.
            ui.dnd_drag_source(Id::new(("export-source-drag", index)), index, |ui| {
                let (grip, _) = ui.allocate_exact_size(Vec2::splat(HANDLE_SIZE), Sense::hover());
                draw_icon(
                    ui.painter(),
                    grip.center(),
                    Icon::DragHandle,
                    palette.text_dim,
                );
            })
            .response
            .on_hover_text("Drag to reorder");

            ui.add_space(3.0);

            let (badge, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
            ui.painter()
                .circle_filled(badge.center(), 9.0, palette.accent_soft);
            ui.painter().text(
                badge.center(),
                Align2::CENTER_CENTER,
                (index + 1).to_string(),
                FontId::proportional(10.5),
                palette.accent,
            );

            ui.add_space(7.0);
            preview_tile(ui, palette, entry, thumb);
            ui.add_space(9.0);

            // The name is given whatever room the controls leave, so a long file
            // name cannot push the buttons off the row.
            let reserved = 3.0 * (BUTTON_SIZE + 2.0) + 8.0;
            let text_width = (ui.available_width() - reserved).max(80.0);
            ui.allocate_ui_with_layout(
                Vec2::new(text_width, ROW_HEIGHT),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.add_space(7.0);
                    ui.add(
                        egui::Label::new(
                            RichText::new(file_name(&entry.path))
                                .color(palette.text)
                                .size(12.0),
                        )
                        .truncate(),
                    );
                    ui.add(
                        egui::Label::new(
                            RichText::new(&entry.detail)
                                .color(palette.text_dim)
                                .size(10.5),
                        )
                        .truncate(),
                    );
                },
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_button(ui, palette, Icon::Remove, true)
                    .on_hover_text("Remove from the list")
                    .clicked()
                {
                    actions.remove = Some(index);
                }
                if icon_button(ui, palette, Icon::MoveDown, can_move_down)
                    .on_hover_text("Move down")
                    .clicked()
                {
                    actions.move_down = Some(index);
                }
                if icon_button(ui, palette, Icon::MoveUp, can_move_up)
                    .on_hover_text("Move up")
                    .clicked()
                {
                    actions.move_up = Some(index);
                }
            });
        });
    });

    rect
}

/// A compact icon button.
///
/// `enabled` is passed in rather than handled with `add_enabled_ui` because the
/// icon is painted straight onto the frame, not drawn from a label.
fn icon_button(ui: &mut egui::Ui, palette: &Palette, icon: Icon, enabled: bool) -> egui::Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(BUTTON_SIZE), sense);

    if enabled && response.hovered() {
        ui.painter().rect_filled(
            rect,
            pdfreader_ui::card_radius(palette),
            palette.surface_hover,
        );
    }
    let ink = if enabled {
        palette.text
    } else {
        palette.text_dim.gamma_multiply(0.45)
    };
    draw_icon(ui.painter(), rect.center(), icon, ink);
    response
}

/// The preview box on a row: the decoded thumbnail when there is one, and the
/// file's type badge until then — or for ever, for a file that cannot be read.
fn preview_tile(
    ui: &mut egui::Ui,
    palette: &Palette,
    entry: &SourceEntry,
    thumb: Option<&egui::TextureHandle>,
) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(TILE_SIZE), Sense::hover());
    tile_background(ui, palette, rect);

    if let Some(texture) = thumb {
        let size = fit_into(texture.size_vec2(), TILE_SIZE - 4.0);
        ui.painter().image(
            texture.id(),
            Rect::from_center_size(rect.center(), size),
            uv_full(),
            Color32::WHITE,
        );
    } else {
        draw_icon(
            ui.painter(),
            rect.center(),
            type_icon(entry),
            palette.text_dim,
        );
    }
}

/// The rounded, recessed box a preview sits in.
fn tile_background(ui: &mut egui::Ui, palette: &Palette, rect: Rect) {
    ui.painter().rect_filled(rect, 3.0, palette.window_bg);
    ui.painter().rect_stroke(
        rect,
        3.0,
        Stroke::new(1.0, palette.border),
        StrokeKind::Inside,
    );
}

/// The badge for a file: a picture or a document.
fn type_icon(entry: &SourceEntry) -> Icon {
    if is_image(&entry.path) {
        Icon::ImageFile
    } else {
        Icon::PdfFile
    }
}

/// The whole of a texture, for `Painter::image`.
fn uv_full() -> Rect {
    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0))
}

/// The file's own name, falling back to the whole path.
fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// The strip under the list showing the pages the job will produce, in order.
///
/// The list already carries the thumbnails; this is the *result* view — the
/// whole order at a glance, without scrolling, which is what makes a long stack
/// of scans checkable before it is written.
fn order_preview(
    ui: &mut egui::Ui,
    palette: &Palette,
    sources: &[SourceEntry],
    thumbs: &HashMap<PathBuf, egui::TextureHandle>,
    caption: &str,
) {
    card_frame(palette)
        .inner_margin(egui::Margin::symmetric(12, 9))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (dot, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                ui.painter()
                    .circle_filled(dot.center(), 3.5, palette.accent);
                ui.add_space(2.0);
                ui.label(
                    RichText::new("OUTPUT ORDER")
                        .color(palette.text_dim)
                        .size(10.0)
                        .strong(),
                );
                ui.add_space(6.0);
                ui.label(RichText::new(caption).color(palette.text_dim).size(10.0));
            });
            ui.add_space(6.0);
            egui::ScrollArea::horizontal()
                .id_salt("export-order-preview")
                .max_height(PREVIEW_TILE_H + 6.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for (index, entry) in sources.iter().enumerate() {
                            preview_page_tile(ui, palette, index, entry, thumbs.get(&entry.path));
                            ui.add_space(4.0);
                        }
                    });
                });
        });
}

/// One numbered page in the output-order strip.
fn preview_page_tile(
    ui: &mut egui::Ui,
    palette: &Palette,
    index: usize,
    entry: &SourceEntry,
    thumb: Option<&egui::TextureHandle>,
) {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(PREVIEW_TILE_W, PREVIEW_TILE_H), Sense::hover());
    tile_background(ui, palette, rect);

    let art = Rect::from_min_max(
        rect.min + Vec2::new(3.0, 3.0),
        Pos2::new(rect.max.x - 3.0, rect.max.y - 13.0),
    );
    if let Some(texture) = thumb {
        let size = fit_into(texture.size_vec2(), art.width().min(art.height()));
        ui.painter().image(
            texture.id(),
            Rect::from_center_size(art.center(), size),
            uv_full(),
            Color32::WHITE,
        );
    } else {
        draw_icon(
            ui.painter(),
            art.center(),
            type_icon(entry),
            palette.text_dim,
        );
    }

    ui.painter().text(
        Pos2::new(rect.center().x, rect.max.y - 7.0),
        Align2::CENTER_CENTER,
        (index + 1).to_string(),
        FontId::proportional(10.0),
        palette.text_dim,
    );
    response.on_hover_text(file_name(&entry.path));
}

/// The destination field: a read-only path that elides from the middle.
fn path_field(ui: &mut egui::Ui, palette: &Palette, path: &Path, folder: bool, width: f32) {
    let frame = field_frame(palette);
    let text = elide_path(path, folder);

    frame.show(ui, |ui| {
        ui.set_width(width);
        ui.add(
            egui::Label::new(RichText::new(text).color(palette.text).size(11.5))
                .truncate()
                .selectable(false),
        )
        .on_hover_text(path.display().to_string());
    });
}

/// Shorten a path from the middle so both ends stay recognisable.
///
/// The tail matters (the file name) and so does the head (which disk it is on),
/// while the directories in between are what a user can spare. A directory is
/// never shortened: its tail is the part that matters, and hiding which folder
/// is about to be written into would be the opposite of helpful.
fn elide_path(path: &Path, folder: bool) -> String {
    /// Length past which a path is shortened.
    const LIMIT: usize = 64;
    /// How much of the leading directories to keep.
    const HEAD: usize = 16;

    let text = path.display().to_string();
    if folder || text.chars().count() <= LIMIT {
        return text;
    }

    let (Some(parent), Some(name)) = (path.parent(), path.file_name().and_then(|n| n.to_str()))
    else {
        return text;
    };
    let parent = parent.display().to_string();
    if parent.chars().count() <= HEAD {
        return text;
    }

    let head: String = parent.chars().take(HEAD).collect();
    format!("{head}…{}{name}", std::path::MAIN_SEPARATOR)
}

/// The page preview beside the settings.
fn preview_pane(
    ui: &mut egui::Ui,
    palette: &Palette,
    doc: &OpenDocument<'_>,
    preview: Option<&egui::TextureHandle>,
    single: bool,
) {
    let frame = card_frame(palette).inner_margin(egui::Margin::same(8));

    frame.show(ui, |ui| {
        ui.set_width(PREVIEW_WIDTH);
        ui.vertical_centered(|ui| {
            match preview {
                Some(texture) => {
                    let size = fit_into(texture.size_vec2(), PREVIEW_WIDTH);
                    ui.add(egui::Image::new(texture).fit_to_exact_size(size));
                }
                None => {
                    // An empty box of the right height keeps the settings
                    // beside it from jumping when the preview arrives.
                    ui.allocate_ui(Vec2::new(PREVIEW_WIDTH, PREVIEW_WIDTH * 0.6), |ui| {
                        ui.centered_and_justified(|ui| {
                            ui.spinner();
                        });
                    });
                }
            }

            ui.add_space(8.0);
            let caption = if single {
                format!("Page {}", doc.current_page + 1)
            } else {
                format!("{} pages", doc.pages)
            };
            ui.label(RichText::new(caption).color(palette.text).size(11.5));
            if let Some((width, height)) = doc.page_size {
                ui.label(
                    RichText::new(format!("{width:.0} × {height:.0} pt"))
                        .color(palette.text_dim)
                        .size(10.5),
                );
            }
        });
    });
}

/// Scale a texture down to fit a square box, keeping its aspect ratio.
fn fit_into(size: Vec2, box_size: f32) -> Vec2 {
    if size.x <= 0.0 || size.y <= 0.0 {
        return Vec2::splat(box_size);
    }
    let scale = (box_size / size.x).min(box_size / size.y).min(1.0);
    size * scale
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(pages: u32, current: u32) -> OpenDocument<'static> {
        OpenDocument {
            title: Some("report"),
            folder: PathBuf::from("/docs"),
            pages,
            current_page: current,
            page_size: Some((595.276, 841.89)),
        }
    }

    /// A snapshot with nothing open, which is what the window sees on an empty
    /// desktop.
    fn no_doc() -> OpenDocument<'static> {
        OpenDocument::snapshot(None)
    }

    fn window(task: ExportTask) -> ExportWindow {
        ExportWindow::new(task, &doc(10, 4))
    }

    #[test]
    fn the_window_opens_ready_to_export_the_common_case() {
        // A suggested destination means the user can press the button without
        // choosing anything, which is the point of pre-filling it.
        let window = window(ExportTask::PageImage);
        let readiness = window.readiness(&doc(10, 4));

        let Ok(Request::PageImage {
            path,
            format,
            scale,
        }) = readiness
        else {
            panic!("expected a page image request, got {readiness:?}");
        };
        assert_eq!(path, PathBuf::from("/docs/report-page-0005.png"));
        assert_eq!(format, ImageFormat::Png);
        assert!((scale - ExportDpi::Print.scale()).abs() < f32::EPSILON);
    }

    #[test]
    fn all_pages_suggests_a_directory_named_after_the_document() {
        let window = window(ExportTask::AllPageImages);
        let Ok(Request::AllPages { dir, .. }) = window.readiness(&doc(3, 0)) else {
            panic!("expected an all-pages request");
        };
        assert_eq!(dir, PathBuf::from("/docs/report-pages"));
    }

    #[test]
    fn a_task_that_reads_the_document_refuses_an_empty_window() {
        let window = window(ExportTask::PageImage);
        let problem = window
            .readiness(&no_doc())
            .expect_err("an export with no document must be refused");
        assert!(problem.contains("Open a document"), "got {problem}");
    }

    #[test]
    fn composition_tasks_work_with_no_document_at_all() {
        for task in [ExportTask::ImagesToPdf, ExportTask::MergePdfs] {
            let mut window = ExportWindow::new(task, &no_doc());
            window.add_sources(vec![PathBuf::from("/a.pdf"), PathBuf::from("/b.pdf")]);
            assert!(
                window.readiness(&no_doc()).is_ok(),
                "{task:?} must not need an open document"
            );
        }
    }

    #[test]
    fn merging_needs_two_files_because_one_is_a_copy() {
        let mut window = window(ExportTask::MergePdfs);
        window.add_sources(vec![PathBuf::from("/a.pdf")]);
        let problem = window
            .readiness(&doc(0, 0))
            .expect_err("a single input must be refused");
        assert!(problem.contains("two"), "got {problem}");

        window.add_sources(vec![PathBuf::from("/b.pdf")]);
        assert!(window.readiness(&doc(0, 0)).is_ok());
    }

    #[test]
    fn picking_the_same_file_twice_adds_it_once() {
        let mut window = window(ExportTask::MergePdfs);
        window.add_sources(vec![PathBuf::from("/a.pdf"), PathBuf::from("/a.pdf")]);
        assert_eq!(window.sources.len(), 1);

        // A second pick of the same file among new ones is still ignored.
        window.add_sources(vec![PathBuf::from("/b.pdf"), PathBuf::from("/a.pdf")]);
        assert_eq!(window.sources.len(), 2);
    }

    #[test]
    fn entries_can_be_reordered_and_removed() {
        let mut window = window(ExportTask::MergePdfs);
        window.add_sources(vec![
            PathBuf::from("/a.pdf"),
            PathBuf::from("/b.pdf"),
            PathBuf::from("/c.pdf"),
        ]);

        window.move_source(0, 1);
        let order: Vec<&Path> = window.sources.iter().map(|e| e.path.as_path()).collect();
        assert_eq!(
            order,
            vec![
                Path::new("/b.pdf"),
                Path::new("/a.pdf"),
                Path::new("/c.pdf")
            ]
        );

        window.remove_source(1);
        let order: Vec<&Path> = window.sources.iter().map(|e| e.path.as_path()).collect();
        assert_eq!(order, vec![Path::new("/b.pdf"), Path::new("/c.pdf")]);
    }

    /// A move off either end must do nothing rather than panic or wrap around.
    #[test]
    fn moving_past_an_end_is_ignored() {
        let mut window = window(ExportTask::MergePdfs);
        window.add_sources(vec![PathBuf::from("/a.pdf"), PathBuf::from("/b.pdf")]);

        window.move_source(0, -1);
        window.move_source(1, 1);
        window.move_source(9, 1);

        let order: Vec<&Path> = window.sources.iter().map(|e| e.path.as_path()).collect();
        assert_eq!(order, vec![Path::new("/a.pdf"), Path::new("/b.pdf")]);
    }

    #[test]
    fn the_range_defaults_to_the_whole_document() {
        let window = window(ExportTask::PagePdf);
        assert_eq!(window.typed_range(&doc(10, 4)), Ok(PageRange::new(0, 9)));
    }

    #[test]
    fn the_typed_range_is_one_based_and_checked_against_the_document() {
        let mut window = window(ExportTask::PagePdf);

        window.range_first = "3".to_string();
        window.range_last = "5".to_string();
        assert_eq!(window.typed_range(&doc(10, 0)), Ok(PageRange::new(2, 4)));

        // A single page is a range of one, not a range of zero.
        window.range_last = "3".to_string();
        assert_eq!(window.typed_range(&doc(10, 0)), Ok(PageRange::single(2)));

        // Past the end, backwards, zero, and not a number at all.
        window.range_last = "11".to_string();
        assert!(window.typed_range(&doc(10, 0)).is_err());
        window.range_first = "6".to_string();
        window.range_last = "5".to_string();
        assert!(window.typed_range(&doc(10, 0)).is_err());
        window.range_first = "0".to_string();
        window.range_last = "5".to_string();
        assert!(window.typed_range(&doc(10, 0)).is_err());
        window.range_first = "two".to_string();
        assert!(window.typed_range(&doc(10, 0)).is_err());
    }

    #[test]
    fn an_empty_document_has_no_range_to_export() {
        let window = window(ExportTask::PagePdf);
        assert!(window.typed_range(&doc(0, 0)).is_err());
    }

    /// Switching tasks changes what kind of path the output is, so the old
    /// destination cannot be kept.
    #[test]
    fn switching_tasks_resets_the_destination() {
        let mut window = window(ExportTask::PageImage);
        assert_eq!(window.output, PathBuf::from("/docs/report-page-0005.png"));

        window.set_task(ExportTask::AllPageImages, &doc(10, 4));
        assert_eq!(window.output, PathBuf::from("/docs/report-pages"));

        window.set_task(ExportTask::MergePdfs, &doc(10, 4));
        assert_eq!(window.output, PathBuf::from("/docs/merged.pdf"));

        // Re-selecting the current task must not throw away a chosen path.
        window.set_output(PathBuf::from("/tmp/out.pdf"));
        window.set_task(ExportTask::MergePdfs, &doc(10, 4));
        assert_eq!(window.output, PathBuf::from("/tmp/out.pdf"));
    }

    #[test]
    fn a_chosen_destination_survives_a_format_change() {
        let mut window = window(ExportTask::PageImage);
        window.set_output(PathBuf::from("/tmp/mine.png"));
        window.format = ImageFormat::Jpeg;
        // The suggestion is only recomputed on a task switch, so a path the
        // user typed is never overwritten by a guess.
        assert_eq!(window.output, PathBuf::from("/tmp/mine.png"));
    }

    #[test]
    fn every_request_becomes_the_command_that_runs_it() {
        let range = PageRange::new(1, 3);
        assert_eq!(
            Request::PagesPdf {
                path: PathBuf::from("a.pdf"),
                range,
            }
            .into_command(),
            Command::ExportPagesPdf {
                path: PathBuf::from("a.pdf"),
                range,
            }
        );
        assert_eq!(
            Request::ImagesToPdf {
                images: vec![PathBuf::from("a.png")],
                output: PathBuf::from("o.pdf"),
                size: ImagePageSize::A4,
            }
            .into_command(),
            Command::ImagesToPdf {
                images: vec![PathBuf::from("a.png")],
                output: PathBuf::from("o.pdf"),
                size: ImagePageSize::A4,
            }
        );
    }

    #[test]
    fn readiness_always_names_the_next_step() {
        // Every blocked state must say what to do, not just that it is blocked.
        let mut window = ExportWindow::new(ExportTask::ImagesToPdf, &no_doc());
        let problem = window.readiness(&no_doc()).expect_err("no images yet");
        assert!(problem.contains("Add at least one image"), "got {problem}");

        // With the inputs in place it is ready, and it says what it will do.
        window.add_sources(vec![PathBuf::from("a.png")]);
        assert!(window.readiness(&no_doc()).is_ok());
    }

    #[test]
    fn file_sizes_are_reported_in_a_unit_that_fits() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn a_long_path_keeps_both_ends_when_it_is_shortened() {
        let path =
            Path::new("/a/very/long/directory/name/that/goes/on/and/on/and/on/report-pages.pdf");
        let elided = elide_path(path, false);
        assert!(elided.ends_with("report-pages.pdf"), "got {elided}");
        assert!(elided.contains('…'), "got {elided}");
        assert!(elided.chars().count() < 48, "got {elided}");

        // A path that already fits is left exactly as it is.
        let short = Path::new("/tmp/out.pdf");
        assert_eq!(elide_path(short, false), short.display().to_string());

        // A directory is shown whole: the tail is the part that matters.
        assert_eq!(elide_path(path, true), path.display().to_string());
    }

    #[test]
    fn the_preview_keeps_its_aspect_ratio_inside_the_box() {
        let wide = fit_into(Vec2::new(400.0, 100.0), 160.0);
        assert!((wide.x - 160.0).abs() < f32::EPSILON);
        assert!((wide.y - 40.0).abs() < f32::EPSILON);

        // A small texture is never blown up: a thumbnail shown larger than it
        // is would just be blurry.
        let small = fit_into(Vec2::new(40.0, 20.0), 160.0);
        assert!((small.x - 40.0).abs() < f32::EPSILON);
        assert!((small.y - 20.0).abs() < f32::EPSILON);
    }

    /// The summary card must name the result in plain words and carry the
    /// numbers a user checks it against — the pixel size for images, the page
    /// count for PDFs, the input count for the composition tasks.
    #[test]
    fn the_outcome_says_what_will_be_written() {
        // One page: the format, and A4 at the default 300 dpi.
        let (headline, detail) = window(ExportTask::PageImage).outcome(&doc(10, 4));
        assert_eq!(headline, "1 PNG image");
        assert!(detail.contains("2480 × 3508"), "got {detail}");
        assert!(detail.contains("300 dpi"), "got {detail}");

        // Every page: the count, pluralised both ways.
        let every = window(ExportTask::AllPageImages);
        assert_eq!(every.outcome(&doc(3, 0)).0, "3 image files");
        assert_eq!(every.outcome(&doc(1, 0)).0, "1 image file");

        // A range PDF: the length of the range, not the document.
        let mut range = window(ExportTask::PagePdf);
        range.range_first = "3".to_string();
        range.range_last = "5".to_string();
        assert_eq!(range.outcome(&doc(10, 0)).0, "A 3-page PDF");

        // A composition task: the number of inputs it will use.
        let mut merge = ExportWindow::new(ExportTask::MergePdfs, &no_doc());
        merge.add_sources(vec![PathBuf::from("/a.pdf"), PathBuf::from("/b.pdf")]);
        assert_eq!(merge.outcome(&no_doc()).0, "A PDF from 2 files");

        // Nothing chosen yet must still describe the empty result honestly.
        let empty = ExportWindow::new(ExportTask::ImagesToPdf, &no_doc());
        assert_eq!(empty.outcome(&no_doc()).0, "A 0-page PDF");
    }

    /// Order is the whole job on the composition tasks, so every way of changing
    /// it has to move the right file to the right place.
    #[test]
    fn the_stack_can_be_reordered_reversed_and_sorted() {
        let mut stack = ExportWindow::new(ExportTask::MergePdfs, &no_doc());
        stack.add_sources(vec![
            PathBuf::from("/a/c.pdf"),
            PathBuf::from("/a/a.pdf"),
            PathBuf::from("/a/b.pdf"),
        ]);

        // Drag the third row onto the first.
        stack.move_source_to(2, 0);
        assert_eq!(
            stack.source_paths(),
            vec![
                Path::new("/a/b.pdf"),
                Path::new("/a/c.pdf"),
                Path::new("/a/a.pdf")
            ]
        );

        // A move onto itself, or off either end, is a no-op rather than a panic
        // and rather than silently doing something else.
        stack.move_source_to(1, 1);
        stack.move_source_to(9, 0);
        stack.move_source_to(0, 9);
        assert_eq!(stack.source_paths().len(), 3);
        assert_eq!(stack.source_paths()[0], Path::new("/a/b.pdf"));

        // Reverse flips the whole list end to end.
        stack.reverse_sources();
        assert_eq!(
            stack.source_paths(),
            vec![
                Path::new("/a/a.pdf"),
                Path::new("/a/c.pdf"),
                Path::new("/a/b.pdf")
            ]
        );

        // Sort is by file name, not by the directories that contain it.
        stack.sort_sources_by_name();
        assert_eq!(
            stack.source_paths(),
            vec![
                Path::new("/a/a.pdf"),
                Path::new("/a/b.pdf"),
                Path::new("/a/c.pdf")
            ]
        );
    }

    /// The drop hit-test turns a pointer position into a destination index, and
    /// has to allow for the dragged row still occupying its old slot.
    #[test]
    fn a_drop_lands_where_the_pointer_points() {
        let rows = vec![
            Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(100.0, 46.0)),
            Rect::from_min_size(Pos2::new(0.0, 46.0), Vec2::new(100.0, 46.0)),
            Rect::from_min_size(Pos2::new(0.0, 92.0), Vec2::new(100.0, 46.0)),
        ];

        // Above the first row's middle: the top of the list.
        assert_eq!(drop_target(&rows, 5.0, 2), Some(0));
        // Below the last row's middle: the end of the list.
        assert_eq!(drop_target(&rows, 200.0, 0), Some(2));
        // Just past the second row's middle: down one slot.
        assert_eq!(drop_target(&rows, 80.0, 0), Some(1));
        // Just above the second row's middle: no move at all.
        assert_eq!(drop_target(&rows, 30.0, 0), Some(0));
        // Nothing to land on.
        assert_eq!(drop_target(&[], 10.0, 0), None);
    }

    #[test]
    fn an_image_is_described_by_its_pixel_size_and_a_pdf_by_its_size_on_disk() {
        assert!(is_image(Path::new("a.PNG")));
        assert!(is_image(Path::new("a.jpeg")));
        assert!(!is_image(Path::new("a.pdf")));

        // A missing file says so rather than reporting a size of zero.
        assert_eq!(describe_source(Path::new("nope.pdf")), "missing");
    }

    /// The window's decisions, pushed through the real reducer, must come out
    /// as the effect that does the work.
    ///
    /// This is the seam neither side can test alone: the window knows nothing
    /// about the reducer, and the reducer has never heard of the window. A
    /// field renamed on one side and not the other would still compile on both.
    #[test]
    fn a_window_request_reaches_the_reducer_as_the_effect_that_runs_it() {
        use pdfreader_core::{Document, DocumentId, Effect, Outline, PageGeometry, Store};

        fn store_with_a_document() -> Store {
            let mut store = Store::new();
            let effects = store.dispatch(Command::OpenPath(PathBuf::from("/docs/report.pdf")));
            let Effect::OpenDocument { tab, .. } = effects[0].clone() else {
                panic!("expected an open request, got {effects:?}");
            };
            store.dispatch(Command::DocumentOpened {
                tab,
                document: Document {
                    id: DocumentId::from_raw(0),
                    path: PathBuf::from("/docs/report.pdf"),
                    title: "report".to_string(),
                    pages: vec![PageGeometry::A4; 4],
                    encrypted: false,
                    can_edit_content: true,
                    outline: Outline::default(),
                },
            });
            store
        }

        // A document task: the reducer supplies the page, the window the rest.
        let mut store = store_with_a_document();
        store.dispatch(Command::GoToPage(1));
        let snapshot = OpenDocument::snapshot(store.state().active());
        let window = ExportWindow::new(ExportTask::PageImage, &snapshot);
        let request = window.readiness(&snapshot).expect("the window is ready");
        let effects = store.dispatch(request.into_command());

        let Effect::ExportPageImage {
            page,
            scale,
            format,
            path,
            ..
        } = &effects[0]
        else {
            panic!("expected a page export, got {effects:?}");
        };
        assert_eq!(*page, 1, "the page in view, not the first page");
        assert!((*scale - ExportDpi::Print.scale()).abs() < f32::EPSILON);
        assert_eq!(*format, ImageFormat::Png);
        assert_eq!(path, &PathBuf::from("/docs/report-page-0002.png"));

        // A composition task: no document anywhere, and the list goes through.
        let mut store = Store::new();
        let mut window = ExportWindow::new(ExportTask::MergePdfs, &no_doc());
        window.add_sources(vec![PathBuf::from("/a.pdf"), PathBuf::from("/b.pdf")]);
        window.set_output(PathBuf::from("/out/merged.pdf"));
        let request = window
            .readiness(&no_doc())
            .expect("merging needs no document");
        let effects = store.dispatch(request.into_command());

        assert_eq!(
            effects,
            vec![Effect::MergePdfs {
                sources: vec![PathBuf::from("/a.pdf"), PathBuf::from("/b.pdf")],
                output: PathBuf::from("/out/merged.pdf"),
            }]
        );
    }

    /// Draw every task on a real `egui::Context` with no backend attached.
    ///
    /// `Context::run_ui` lays out and paints into a `FullOutput` that is thrown
    /// away, so this walks the entire window — task strip, per-task settings,
    /// file list, preview and footer — without a window, a GPU or a screenshot.
    /// It is the only check that covers the drawing code itself, and it catches
    /// what a compile cannot: a panic inside a layout helper, or two widgets
    /// fighting over one id.

    #[test]
    fn every_task_draws_a_frame_without_panicking() {
        let palette = pdfreader_ui::Theme::from_id(pdfreader_core::ThemeId::Dark).palette;

        for task in ExportTask::ALL {
            // Empty and populated, because the file list and the footer's
            // readiness message are exactly what differs between the two.
            for sources in [
                Vec::new(),
                vec![PathBuf::from("/a/one.png"), PathBuf::from("/a/two.pdf")],
            ] {
                let mut window = ExportWindow::new(task, &doc(3, 1));
                window.add_sources(sources);

                let ctx = Context::default();
                let preview = ctx.load_texture(
                    "preview",
                    egui::ColorImage::filled([8, 8], Color32::GRAY),
                    egui::TextureOptions::LINEAR,
                );

                // Two passes and both preview states: a modal settles its size
                // on the first frame and is painted at that size on the second,
                // which is where a bad constraint would show up. The clock has
                // to move between frames or egui treats the second as a repeat.
                for frame in 0..2 {
                    let open = doc(3, 1);
                    let shown = if frame == 0 { None } else { Some(&preview) };
                    let input = egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1280.0, 900.0),
                        )),
                        max_texture_side: Some(2048),
                        time: Some(f64::from(frame) * 0.016),
                        ..egui::RawInput::default()
                    };
                    let output = ctx.run_ui(input, |ui| {
                        window.show(ui.ctx(), &palette, &open, shown, &HashMap::new());
                    });

                    // The window is a whole pane — strip, body and footer — so a
                    // frame that painted almost nothing means it silently drew
                    // nothing, which is the failure this test exists to catch.
                    // The emptiest layout measures 40 primitives; a frame that
                    // only drew the modal backdrop would be a handful, so this
                    // floor has room for egui to change without going vacuous.
                    if frame == 1 {
                        assert!(
                            output.shapes.len() > 25,
                            "{task:?} painted only {} shapes",
                            output.shapes.len()
                        );
                    }
                    output.drop_without_applying_deltas();
                }
            }
        }
    }

    /// The window must fit the viewport. The reported bug was the opposite: the
    /// pane grew past the screen, so the header and the footer could not be
    /// reached. Either the window fits, or the body hit its floor — and that is
    /// only allowed on a viewport too short to hold a usable pane at all.
    #[test]
    fn the_body_budget_keeps_the_window_on_screen() {
        for viewport in [600.0_f32, 700.0, 800.0, 900.0, 1080.0, 1440.0] {
            for header in [80.0_f32, 100.0, 140.0] {
                let budget = body_budget(viewport, header);
                let window_height = header + budget + FOOTER_BLOCK + 2.0 * MODAL_MARGIN;

                assert!(
                    budget <= BODY_HEIGHT,
                    "the body never exceeds its preference"
                );
                assert!(budget >= MIN_BODY_HEIGHT, "the body never collapses");
                assert!(
                    window_height <= viewport + 0.5 || budget <= MIN_BODY_HEIGHT + 0.5,
                    "viewport {viewport}, header {header}: the window is {window_height} px tall"
                );
            }
        }
    }

    /// A generous viewport must still get the whole preferred height, or the cap
    /// would quietly make every window small.
    #[test]
    fn a_tall_viewport_gets_the_full_preferred_body() {
        assert!((body_budget(1440.0, 100.0) - BODY_HEIGHT).abs() < f32::EPSILON);
    }

    /// The export window is a big pane: a task strip, per-task settings, a
    /// preview and a footer side by side, not a compact dialog. Measured off
    /// the painted shapes so a constant drifting back down fails here.
    #[test]
    fn export_window_is_a_large_pane() {
        let palette = pdfreader_ui::Theme::from_id(pdfreader_core::ThemeId::Dark).palette;
        let screen = egui::vec2(1280.0, 900.0);

        for task in ExportTask::ALL {
            let mut window = ExportWindow::new(task, &doc(3, 1));
            let ctx = Context::default();
            for frame in 0..2 {
                let open = doc(3, 1);
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, screen)),
                    max_texture_side: Some(2048),
                    time: Some(f64::from(frame) * 0.016),
                    ..egui::RawInput::default()
                };
                let output = ctx.run_ui(input, |ui| {
                    window.show(ui.ctx(), &palette, &open, None, &HashMap::new());
                });
                if frame == 0 {
                    output.drop_without_applying_deltas();
                    continue;
                }

                // The backdrop is painted across the whole viewport; everything
                // narrower than it belongs to the window itself.
                let rects: Vec<egui::Rect> = output
                    .shapes
                    .iter()
                    .map(|s| s.shape.visual_bounding_rect())
                    .filter(|r| r.is_finite() && r.width() < screen.x - 1.0)
                    .collect();
                let width = rects.iter().map(egui::Rect::width).fold(0.0, f32::max);
                let top = rects.iter().map(|r| r.min.y).fold(f32::MAX, f32::min);
                let bottom = rects.iter().map(|r| r.max.y).fold(0.0, f32::max);
                output.drop_without_applying_deltas();

                assert!(
                    width >= WINDOW_WIDTH,
                    "{task:?} painted only {width:.0} px wide, expected at least {WINDOW_WIDTH}"
                );
                assert!(
                    bottom - top >= BODY_HEIGHT,
                    "{task:?} painted only {:.0} px tall, expected at least {BODY_HEIGHT}",
                    bottom - top
                );
            }
        }
    }
}
