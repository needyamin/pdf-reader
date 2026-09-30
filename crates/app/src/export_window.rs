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

use std::path::{Path, PathBuf};

use egui::{Color32, Context, RichText, Stroke, Vec2};
use pdfreader_core::{
    Command, ExportDpi, ExportTask, ImageFormat, ImagePageSize, PageRange, Tab, pixel_size,
};
use pdfreader_ui::Palette;

/// Image extensions the reader can decode.
///
/// Kept in step with the `image` features the workspace enables: a codec added
/// there but not here would hide readable files from the picker.
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg"];

/// Width of the whole window, in points.
const WINDOW_WIDTH: f32 = 812.0;

/// Width of the task strip on the left.
const STRIP_WIDTH: f32 = 186.0;

/// Height of the scrollable file list for the composition tasks.
const LIST_HEIGHT: f32 = 232.0;

/// Size of the square the page preview is fitted into.
///
/// Public because the shell has to ask the engine for a thumbnail at least this
/// wide; keeping the two numbers in one place is what stops the preview from
/// arriving blurry after someone enlarges the box.
pub const PREVIEW_WIDTH: f32 = 168.0;

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

    /// Draw the window and report what the user did.
    pub fn show(
        &mut self,
        ctx: &Context,
        palette: &Palette,
        doc: &OpenDocument<'_>,
        preview: Option<&egui::TextureHandle>,
    ) -> Action {
        let mut action = Action::None;

        let frame = egui::Frame::new()
            .fill(palette.panel_bg)
            .stroke(Stroke::new(1.0, palette.border))
            .corner_radius(8.0)
            .inner_margin(egui::Margin::symmetric(20, 18));

        let modal = egui::Modal::new(egui::Id::new("export-window"))
            .frame(frame)
            .backdrop_color(Color32::from_black_alpha(120))
            .show(ctx, |ui| {
                ui.set_width(WINDOW_WIDTH);
                self.header(ui, palette);
                ui.add_space(14.0);
                ui.separator();
                ui.add_space(14.0);

                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(STRIP_WIDTH);
                        self.task_strip(ui, palette, doc);
                    });
                    ui.add_space(18.0);
                    ui.vertical(|ui| {
                        ui.set_width(ui.available_width());
                        self.body(ui, palette, doc, preview);
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

    /// Title and the sentence that says what the task produces.
    fn header(&self, ui: &mut egui::Ui, palette: &Palette) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Export")
                    .color(palette.text)
                    .size(17.0)
                    .strong(),
            );
            ui.add_space(6.0);
            ui.label(RichText::new("·").color(palette.text_dim).size(15.0));
            ui.add_space(6.0);
            ui.label(
                RichText::new(self.task.label())
                    .color(palette.accent)
                    .size(15.0),
            );
        });
        ui.add_space(3.0);
        ui.label(
            RichText::new(self.task.blurb())
                .color(palette.text_dim)
                .size(12.0),
        );
    }

    /// The list of tasks down the left-hand side.
    fn task_strip(&mut self, ui: &mut egui::Ui, palette: &Palette, doc: &OpenDocument<'_>) {
        section(ui, palette, "WHAT TO MAKE");
        ui.add_space(6.0);

        // Only one task can be switched to per frame, so the change is applied
        // after the loop rather than while the list is being drawn.
        let mut chosen = None;
        for task in ExportTask::ALL {
            // A task that reads the open document cannot be offered when there
            // is none: a disabled row that explains itself beats a row that
            // silently does nothing.
            let usable = !task.needs_document() || doc.is_open();
            if task_row(ui, palette, task.label(), task == self.task, usable) {
                chosen = Some(task);
            }
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
    ) {
        match self.task {
            ExportTask::PageImage => self.image_body(ui, palette, doc, preview, true),
            ExportTask::AllPageImages => self.image_body(ui, palette, doc, preview, false),
            ExportTask::PagePdf => self.pdf_body(ui, palette, doc, preview),
            ExportTask::ImagesToPdf => self.images_body(ui, palette),
            ExportTask::MergePdfs => self.merge_body(ui, palette),
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
            });
        });
    }

    /// Settings for "images become the pages of a new PDF".
    fn images_body(&mut self, ui: &mut egui::Ui, palette: &Palette) {
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
        self.source_list(ui, palette, Field::AddImages, "Add images…", "image");
        ui.add_space(16.0);
        self.output_row(ui, palette, None);
    }

    /// Settings for "join several PDFs into one".
    fn merge_body(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        section(ui, palette, "PDFS TO JOIN — IN THIS ORDER");
        ui.add_space(6.0);
        self.source_list(ui, palette, Field::AddPdfs, "Add PDFs…", "document");
        ui.add_space(16.0);
        self.output_row(ui, palette, None);
    }

    /// The ordered file list shared by the two composition tasks.
    fn source_list(
        &mut self,
        ui: &mut egui::Ui,
        palette: &Palette,
        field: Field,
        add_label: &str,
        noun: &str,
    ) {
        let mut browse = false;

        ui.horizontal(|ui| {
            if ui
                .add(
                    egui::Button::new(RichText::new(add_label).size(12.0))
                        .fill(palette.accent_soft)
                        .corner_radius(7.0),
                )
                .clicked()
            {
                browse = true;
            }
            ui.add_enabled_ui(!self.sources.is_empty(), |ui| {
                if ui
                    .add(
                        egui::Button::new(RichText::new("Clear").size(12.0))
                            .fill(palette.surface)
                            .corner_radius(7.0),
                    )
                    .clicked()
                {
                    self.clear_sources();
                }
            });
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

        let frame = egui::Frame::new()
            .fill(palette.surface)
            .stroke(Stroke::new(1.0, palette.border))
            .corner_radius(6.0)
            .inner_margin(egui::Margin::same(6));

        frame.show(ui, |ui| {
            egui::ScrollArea::vertical()
                .max_height(LIST_HEIGHT)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    draw_source_rows(ui, palette, &self.sources, noun, &mut actions);
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
        if browse {
            // Handled by the caller through the returned action.
            self.pending_browse = Some(field);
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
            if ui
                .add(
                    egui::Button::new(RichText::new("Browse…").size(12.0))
                        .fill(palette.surface)
                        .corner_radius(7.0),
                )
                .clicked()
            {
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
            if ui
                .add(
                    egui::Button::new(RichText::new("Cancel").size(12.5))
                        .fill(palette.surface)
                        .corner_radius(7.0),
                )
                .clicked()
            {
                cancel = true;
            }

            // The message is the reason the button is off, or the summary of
            // what pressing it will do.
            let (message, colour) = match &readiness {
                Err(problem) => (problem.clone(), palette.danger),
                Ok(request) => (describe_request(request), palette.text_dim),
            };
            ui.add_space(10.0);
            ui.label(RichText::new(message).color(colour).size(11.5));

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let enabled = readiness.is_ok();
                let button =
                    egui::Button::new(RichText::new(self.task.action_label()).size(12.5).color(
                        if enabled {
                            palette.text
                        } else {
                            palette.text_dim
                        },
                    ))
                    .fill(if enabled {
                        palette.accent_soft
                    } else {
                        palette.surface
                    })
                    .corner_radius(7.0);

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

/// A small dim heading above a group of controls.
fn section(ui: &mut egui::Ui, palette: &Palette, text: &str) {
    ui.label(
        RichText::new(text)
            .color(palette.text_dim)
            .size(10.0)
            .strong(),
    );
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
                .corner_radius(6.0);
            if ui.add(button).clicked() {
                picked = Some(index);
            }
        }
    });
    picked
}

/// One row of the task strip.
fn task_row(
    ui: &mut egui::Ui,
    palette: &Palette,
    label: &str,
    selected: bool,
    enabled: bool,
) -> bool {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 30.0), egui::Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);

    let fill = if selected {
        palette.accent_soft
    } else if response.hovered() && enabled {
        palette.surface_hover
    } else {
        palette.surface
    };
    ui.painter().rect_filled(rect, 5.0, fill);

    if selected {
        let bar = egui::Rect::from_min_size(
            rect.min + Vec2::new(0.0, 5.0),
            Vec2::new(3.0, rect.height() - 10.0),
        );
        ui.painter().rect_filled(bar, 2.0, palette.accent);
    }

    let colour = match (enabled, selected) {
        (false, _) => palette.text_dim.gamma_multiply(0.55),
        (true, true) => palette.text,
        (true, false) => palette.text_dim,
    };
    ui.painter().text(
        rect.min + Vec2::new(13.0, rect.height() / 2.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(12.5),
        colour,
    );

    enabled && response.clicked()
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
}

/// Draw the contents of the file list, empty state included.
fn draw_source_rows(
    ui: &mut egui::Ui,
    palette: &Palette,
    sources: &[SourceEntry],
    noun: &str,
    actions: &mut ListActions,
) {
    if sources.is_empty() {
        ui.add_space(18.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new(format!("No {noun}s yet."))
                    .color(palette.text_dim)
                    .size(12.0),
            );
            ui.add_space(3.0);
            ui.label(
                RichText::new("Use the button above to add some.")
                    .color(palette.text_dim)
                    .size(11.0),
            );
        });
        ui.add_space(18.0);
        return;
    }

    let last = sources.len() - 1;
    for (index, entry) in sources.iter().enumerate() {
        let name = entry.path.file_name().map_or_else(
            || entry.path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        source_row(
            ui,
            palette,
            index,
            &name,
            &entry.detail,
            index > 0,
            index < last,
            actions,
        );
    }
}

/// One row of the file list: position, name, detail, and the row controls.
fn source_row(
    ui: &mut egui::Ui,
    palette: &Palette,
    index: usize,
    name: &str,
    detail: &str,
    can_move_up: bool,
    can_move_down: bool,
    actions: &mut ListActions,
) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{:>2}", index + 1))
                .color(palette.text_dim)
                .size(11.0)
                .monospace(),
        );
        ui.add_space(2.0);

        // The name is given whatever room is left after the controls, so a long
        // path cannot push the buttons off the row.
        let reserved = 108.0;
        let width = (ui.available_width() - reserved).max(60.0);
        ui.allocate_ui_with_layout(
            Vec2::new(width, 20.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.add(
                    egui::Label::new(RichText::new(name).color(palette.text).size(12.0)).truncate(),
                );
            },
        );
        ui.label(RichText::new(detail).color(palette.text_dim).size(11.0));

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(
                    true,
                    egui::Button::new(RichText::new("✕").size(11.0)).frame(false),
                )
                .on_hover_text("Remove from the list")
                .clicked()
            {
                actions.remove = Some(index);
            }
            if ui
                .add_enabled(
                    can_move_down,
                    egui::Button::new(RichText::new("▼").size(9.0)).frame(false),
                )
                .on_hover_text("Move down")
                .clicked()
            {
                actions.move_down = Some(index);
            }
            if ui
                .add_enabled(
                    can_move_up,
                    egui::Button::new(RichText::new("▲").size(9.0)).frame(false),
                )
                .on_hover_text("Move up")
                .clicked()
            {
                actions.move_up = Some(index);
            }
        });
    });
}

/// The destination field: a read-only path that elides from the middle.
fn path_field(ui: &mut egui::Ui, palette: &Palette, path: &Path, folder: bool, width: f32) {
    let frame = egui::Frame::new()
        .fill(palette.window_bg)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(9, 6));

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
    let frame = egui::Frame::new()
        .fill(palette.window_bg)
        .stroke(Stroke::new(1.0, palette.border))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::same(8));

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
                        window.show(ui.ctx(), &palette, &open, shown);
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
}
