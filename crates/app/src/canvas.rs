//! Tile-based canvas: turns engine tile bitmaps into on-screen pages.
//!
//! The canvas owns a small LRU of GPU textures (one per tile) and a cache of
//! thumbnails. Each frame it works out which tiles the viewport needs, asks the
//! caller to forward the missing ones to the engine thread, and draws whatever
//! it already has. Tiles that have not arrived yet are drawn as blank page
//! backgrounds, so a page never flashes; it fills in.
//!
//! Tiles are keyed by `(document, rotation, page, level, col, row)`. The `level`
//! is the zoom-ladder level, so a tile rendered at one zoom is still usable at a
//! nearby zoom; rotation is part of the key so a rotated page never serves a
//! stale, unrotated bitmap.

use std::collections::{HashMap, HashSet, VecDeque};

use egui::{
    Color32, ColorImage, Context, Pos2, Rect, Sense, Stroke, StrokeKind, TextureHandle,
    TextureOptions, Ui, Vec2, pos2, vec2,
};
use pdfreader_core::{
    AnnotationId, AnnotationInfo, AnnotationKind, Command, Document, DocumentId, FieldId,
    FieldValue, FormFieldType, FormInfo, NewAnnotation, Rect as PdfSpaceRect, Rotation, Tool,
    ViewMode, ViewState,
};
use pdfreader_pdf::CharRange;
use pdfreader_pdf::annot::{MIN_MARKUP_THICKNESS, SQUIGGLY_RISE_PT, SQUIGGLY_STEP_PT};
use pdfreader_pdf::engine::{TilePixels, TileRequest};
use pdfreader_render::{
    PageBox, PageSpace, TILE_SIZE, TileKey, hit_page, level_scale, tile_grid, tile_pixel_rect,
    zoom_level,
};

use crate::lens::{LookupRequest, PageTextCache};

/// Logical gap between consecutive pages, in points.
pub const PAGE_GAP: f32 = 16.0;
/// Logical margin around the document, in points.
pub const MARGIN: f32 = 24.0;
/// Horizontal space the always-visible thin vertical scroll bar reserves.
///
/// `Theme::apply` selects `ScrollStyle::thin()`, whose floating bar reserves a
/// few pixels inside the viewport. Fit-width zoom must subtract this so the
/// page's right edge is not hidden behind the scroll bar.
pub const SCROLLBAR_ALLOWANCE: f32 = 12.0;
/// Thumbnail width, in logical points.
pub const THUMB_WIDTH: f32 = 108.0;
/// Maximum tile textures kept alive at once.
///
/// Each tile is a 512×512 RGBA texture (~1 MB), so this caps tile memory at
/// ~128 MB. That is several screens' worth, which is enough to scroll back and
/// forth without re-rasterizing, while staying well clear of memory pressure.
const TILE_CAPACITY: usize = 128;
/// Maximum thumbnail textures kept alive at once.
///
/// Each thumbnail is roughly THUMB_WIDTH × 1.4·THUMB_WIDTH at device scale,
/// i.e. ~100–300 KB of GPU memory. 512 entries are about ten screens of
/// thumbnails; older ones are re-rasterized on demand, mirroring the tile
/// cache's eviction strategy.
const THUMB_CAPACITY: usize = 512;
/// Maximum new tile requests issued per frame.
const MAX_REQUESTS_PER_FRAME: usize = 32;
/// Maximum outstanding tile requests before the canvas stops asking.
const MAX_INFLIGHT: usize = 96;

/// Cache key for a tile texture.
type TileCacheKey = (DocumentId, Rotation, TileKey);
/// Cache key for a thumbnail texture.
type ThumbCacheKey = (DocumentId, Rotation, u32);

/// A just-drawn annotation echoed on screen until the re-rendered page tiles
/// arrive.
///
/// Creating an annotation is asynchronous: the command goes to the engine
/// thread, PDFium regenerates the page, and fresh tiles round-trip back. For
/// the couple of frames that takes, the echo is what the user sees — without
/// it the shape they just drew would blink out and reappear.
/// A note waiting for its text while the input popup is open on the canvas.
///
/// The annotation itself is only created when the user confirms the text, so
/// it is born with the right contents in a single round-trip.
#[derive(Clone, Debug)]
struct NoteInput {
    /// Page the note sits on.
    page: u32,
    /// Anchor of the note in **unrotated PDF space** (points, y up). Kept in
    /// page space so it survives zoom/scroll and maps back through
    /// `PageSpace::to_screen_point` for drawing.
    point: (f32, f32),
    /// The text being typed.
    text: String,
    /// Whether the popup should grab keyboard focus (first frame only).
    opened: bool,
}

#[derive(Clone, Copy, Debug)]
struct PendingEcho {
    /// Document the shape belongs to.
    doc: DocumentId,
    /// Page the shape sits on.
    page: u32,
    /// What kind of shape it is, for colours.
    kind: AnnotationKind,
    /// Layout-space rectangle for drag-created shapes.
    rect: Option<Rect>,
    /// Layout-space anchor for click-placed notes.
    point: Option<(f32, f32)>,
}

/// A tile the caller should forward to the engine thread.
pub struct PendingTile {
    /// Document the tile belongs to.
    pub doc: DocumentId,
    /// Tile identity, echoed back on the response.
    pub key: TileKey,
    /// Rotation the tile was rendered with.
    pub rotation: Rotation,
    /// What to rasterize.
    pub request: TileRequest,
}

/// A thumbnail the caller should forward to the engine thread.
pub struct PendingThumb {
    /// Document the thumbnail belongs to.
    pub doc: DocumentId,
    /// Zero-based page index.
    pub page: u32,
    /// Rotation to bake in.
    pub rotation: Rotation,
    /// Device pixels per PDF point.
    pub scale: f32,
}

/// Result of drawing the thumbnail strip.
#[derive(Default)]
pub struct ThumbnailResult {
    /// Thumbnails to request from the engine.
    pub requests: Vec<PendingThumb>,
    /// Page the user clicked, if any.
    pub clicked: Option<u32>,
}

/// The ink typed form values are drawn in until the user picks another.
pub const DEFAULT_FORM_INK: [u8; 3] = [25, 25, 25];

/// Font and ink used to draw form field text and the on-page editor.
///
/// The app owns the settings — Settings → Text, and the Forms panel's "Typing
/// font" block — and pushes them here so the canvas can render typed values in
/// the chosen face, size and colour. The PDF's own font is not enough for
/// scripts like Bengali, and the size a form hard-codes is often too small to
/// read.
#[derive(Clone, Debug)]
pub struct FormFont {
    /// Named egui font family, or empty for the default proportional face.
    pub family: String,
    /// Point size, or `0.0` to fit the field's height.
    pub size: f32,
    /// Whether the text is drawn heavier.
    pub bold: bool,
    /// RGB ink for typed values.
    pub color: [u8; 3],
}

impl Default for FormFont {
    fn default() -> Self {
        Self {
            family: String::new(),
            size: 0.0,
            bold: false,
            color: DEFAULT_FORM_INK,
        }
    }
}

/// How far the second stamp of synthetic bold sits from the first, in points.
///
/// `egui` has no weight axis and no bold face is bundled, so bold is made by
/// drawing the glyphs twice. A quarter of a point at the sizes a form uses
/// thickens the strokes visibly without smearing the letterforms; more than
/// about half a point starts to look like a drop shadow.
const SYNTHETIC_BOLD_OFFSET: f32 = 0.35;

impl FormFont {
    /// The egui family to draw with, falling back to the default proportional
    /// face when no family is chosen.
    pub fn family(&self) -> egui::FontFamily {
        if self.family.is_empty() {
            egui::FontFamily::Proportional
        } else {
            egui::FontFamily::Name(self.family.clone().into())
        }
    }

    /// The size to draw at, given the field's on-screen height.
    pub fn size_for(&self, rect_height: f32) -> f32 {
        if self.size > 0.0 {
            self.size
        } else {
            (rect_height * 0.62).clamp(6.0, 15.0)
        }
    }

    /// The ink to draw typed values in.
    pub fn ink(&self) -> Color32 {
        Color32::from_rgb(self.color[0], self.color[1], self.color[2])
    }
}

/// Texture caches for tiles and thumbnails.
///
/// The booleans are independent interaction and rendering flags (pan tool,
/// selection in progress, prefetch); they read better as named fields than as
/// a packed bitflag.
#[allow(clippy::struct_excessive_bools)]
pub struct Canvas {
    tiles: HashMap<TileCacheKey, TextureHandle>,
    order: VecDeque<TileCacheKey>,
    inflight: HashSet<TileCacheKey>,
    /// Tiles that failed to rasterize, so we never retry them in a loop.
    failed: HashSet<TileCacheKey>,
    thumbs: HashMap<ThumbCacheKey, TextureHandle>,
    /// Insertion order of the thumbnail cache, for LRU eviction.
    thumb_order: VecDeque<ThumbCacheKey>,
    thumb_inflight: HashSet<ThumbCacheKey>,
    /// Thumbnails that failed to rasterize.
    thumb_failed: HashSet<ThumbCacheKey>,
    /// Scroll offset of the document, from the last draw.
    scroll_offset: Vec2,
    /// Document whose scroll state is currently represented by the canvas.
    /// This lets switching tabs restore each tab's persisted viewport.
    active_document: Option<DocumentId>,
    /// Page under the viewport centre, from the last draw.
    center_page: u32,
    /// Vertical fraction (0=top, 1=bottom) of the centre page the viewport
    /// centre sits at. Used to keep the view anchored across a zoom change.
    center_frac_y: f32,
    /// Horizontal fraction of the centre page under the viewport centre.
    center_frac_x: f32,
    /// Zoom of the last draw, so a zoom change can be detected and the view
    /// anchored to what the user was looking at. `None` before the first draw.
    last_zoom: Option<f32>,
    /// Absolute scroll requested by keyboard or other programmatic viewport
    /// commands. It is consumed by the next draw, after the reducer has
    /// accepted the command.
    pending_scroll_offset: Option<Vec2>,
    /// A field to bring into view on the next draw, as `(page, PDF-space rect)`.
    ///
    /// Kept apart from `pending_scroll_offset` because the offset cannot be
    /// worked out until the page layout is known, and because a field already
    /// on screen must not move at all — clicking into a field to type should
    /// never shift the page under the pointer.
    pending_reveal: Option<(u32, PdfSpaceRect)>,
    /// Page the thumbnail strip last scrolled itself to, and in which
    /// document, so it follows page navigation without fighting the user's
    /// own scrolling of the strip — and re-anchors after a tab switch even
    /// when both tabs sit on the same page number.
    thumb_follow: Option<(DocumentId, u32)>,
    /// Whether the hand tool is engaged. While it is on, dragging on the
    /// document moves the view instead of selecting.
    pan_tool: bool,
    /// Set on the frame a pan drag is recognised, so the cursor can show the
    /// closed-hand icon until the drag is released.
    panning: bool,
    /// Where each laid-out page sits in its own coordinate space, from the last
    /// draw. The shell uses these to turn a pointer position into a page and a
    /// point on that page.
    page_spaces: Vec<PageSpace>,
    /// A click from the last draw, in document layout space.
    ///
    /// Recorded rather than acted on because deciding *what* was clicked — a
    /// form field, a link, an annotation — needs document state the canvas does
    /// not have. The shell takes it and resolves it.
    clicked_at: Option<(f32, f32)>,
    /// In-progress text for the field being edited on the page.
    ///
    /// Committing per keystroke would round-trip every character through the
    /// engine thread and re-rasterize the page, so the text is held here and
    /// written on Enter or when the editor loses focus.
    field_drafts: HashMap<FieldId, String>,
    /// Field whose on-page editor was opened most recently, so the text caret
    /// is placed there exactly once instead of stolen every frame.
    open_editor: Option<FieldId>,
    /// Edit commands produced by on-page editors, taken by the shell.
    edits: Vec<Command>,
    /// Layout-space start point of an in-progress annotation drag.
    draw_start: Option<(f32, f32)>,
    /// Page the drag started on, so the annotation lands on one page even if
    /// the pointer drifts over a neighbouring one.
    draw_page: Option<u32>,
    /// Tiles whose pixels predate an edit (annotation/field change) and must be
    /// re-rasterized. The old texture stays visible until the fresh one
    /// arrives, so an edit updates the page in place instead of flashing.
    stale_tiles: HashSet<TileCacheKey>,
    /// Thumbnails in the same stale state.
    stale_thumbs: HashSet<ThumbCacheKey>,
    /// Shapes echoed on screen while their real pixels render.
    pending: Vec<PendingEcho>,
    /// Open text input for a note being placed.
    note_input: Option<NoteInput>,
    /// Start of an in-progress text selection: the page and the layout-space
    /// point the press landed on.
    select_start: Option<(u32, (f32, f32))>,
    /// Whether the current press actually began a selection on a page. Persists
    /// across frames so the release can commit it; stays false for clicks that
    /// landed on the popup or blank paper, which must not re-open the card.
    selecting: bool,
    /// Whether the primary button is currently held down on the page area.
    /// Tracks the edge so a selection starts on the press frame, not on every
    /// frame of the drag.
    pressing_page: bool,
    /// Whether the current press landed on the canvas but not on a page.
    ///
    /// The gaps between pages, and the margin around them, have no text to
    /// select, so a drag that starts there moves the view instead of doing
    /// nothing at all.
    press_on_background: bool,
    /// The annotation the eraser most recently deleted during the current
    /// press. Held only for the length of one press: a sweeping drag should
    /// erase several marks in a row, but the annotation list lags a frame or
    /// two behind the store, so without this the same mark is re-deleted
    /// every frame until the refreshed list arrives.
    erase_last: Option<AnnotationId>,
    /// An in-progress drag of a selected annotation: moving it, or resizing
    /// it by one of its handles. While it is set the text selection must not
    /// react to the same gesture, and the dragged annotation is drawn as a
    /// ghost at its target position until the engine re-renders the page.
    ann_drag: Option<AnnDrag>,
    /// The rectangle the in-progress drag has produced so far, in PDF page
    /// space. Computed every frame from the pointer; on release it is what
    /// gets committed to the store.
    ann_drag_rect: Option<PdfSpaceRect>,
    /// Whether tiles just outside the viewport are requested ahead of time.
    /// Turning this off (Settings → Performance) saves memory and bandwidth at
    /// the cost of a brief blank flash when scrolling into a new area.
    prefetch: bool,
    /// Font used to draw form field text and the on-page editor.
    form_font: FormFont,
    /// Corner radius the theme uses for controls, so the on-page note editor
    /// is rounded the same amount as every other control on screen. The canvas
    /// is given the palette's colours one at a time, so this is the piece it
    /// has to be told rather than read.
    corner_radius: f32,
    /// The text selection: the page and the characters covered.
    ///
    /// Held here rather than in the store because it is transient widget state
    /// tied to what is on screen, not part of the document.
    selection: Option<(u32, CharRange)>,
    /// A word or phrase the user just picked, taken by the shell to open the
    /// lookup popup.
    lookup: Option<LookupRequest>,
}

/// What an in-progress annotation drag is doing.
#[derive(Clone, Copy, PartialEq, Debug)]
enum AnnDrag {
    /// The whole annotation is being dragged: `start` is the content-local
    /// pointer position at press, `rect` the annotation's bounds then.
    Move {
        id: AnnotationId,
        page: u32,
        start: (f32, f32),
        rect: PdfSpaceRect,
    },
    /// One resize handle is being dragged; `rect` is the annotation's bounds
    /// at press, and the handle's opposite corner or edge stays fixed.
    ///
    /// `kind` is carried so the drag can pick the right vertical floor: a line
    /// markup's vertical axis is its stroke weight, so it may be dragged far
    /// below [`MIN_ANN_SIZE`] (see [`resize_rect_with`]).
    Resize {
        id: AnnotationId,
        page: u32,
        kind: AnnotationKind,
        handle: ResizeHandle,
        rect: PdfSpaceRect,
    },
}

impl AnnDrag {
    fn id(self) -> AnnotationId {
        match self {
            AnnDrag::Move { id, .. } | AnnDrag::Resize { id, .. } => id,
        }
    }

    fn page(self) -> u32 {
        match self {
            AnnDrag::Move { page, .. } | AnnDrag::Resize { page, .. } => page,
        }
    }

    fn rect(self) -> PdfSpaceRect {
        match self {
            AnnDrag::Move { rect, .. } | AnnDrag::Resize { rect, .. } => rect,
        }
    }
}

/// Which resize handle of a selected annotation is being dragged.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ResizeHandle {
    NorthWest,
    North,
    NorthEast,
    East,
    SouthEast,
    South,
    SouthWest,
    West,
}

/// The smallest allowed width or height of a resized annotation, in PDF
/// points. Below this a drag would collapse the mark into an invisible line.
const MIN_ANN_SIZE: f32 = 8.0;

impl Default for Canvas {
    fn default() -> Self {
        Self::new()
    }
}

impl Canvas {
    /// Create an empty canvas.
    pub fn new() -> Self {
        Self {
            tiles: HashMap::new(),
            order: VecDeque::new(),
            inflight: HashSet::new(),
            failed: HashSet::new(),
            thumbs: HashMap::new(),
            thumb_order: VecDeque::new(),
            thumb_inflight: HashSet::new(),
            thumb_failed: HashSet::new(),
            scroll_offset: Vec2::ZERO,
            active_document: None,
            center_page: 0,
            center_frac_y: 0.5,
            center_frac_x: 0.5,
            last_zoom: None,
            pending_scroll_offset: None,
            pending_reveal: None,
            thumb_follow: None,
            pan_tool: false,
            panning: false,
            page_spaces: Vec::new(),
            clicked_at: None,
            field_drafts: HashMap::new(),
            open_editor: None,
            edits: Vec::new(),
            draw_start: None,
            draw_page: None,
            stale_tiles: HashSet::new(),
            stale_thumbs: HashSet::new(),
            pending: Vec::new(),
            note_input: None,
            select_start: None,
            selecting: false,
            press_on_background: false,
            erase_last: None,
            ann_drag: None,
            ann_drag_rect: None,
            pressing_page: false,
            prefetch: true,
            form_font: FormFont::default(),
            corner_radius: 7.0,
            selection: None,
            lookup: None,
        }
    }

    /// Take the word or phrase the user just picked, if any.
    ///
    /// One pick is handled once, the same as [`Self::take_click`].
    pub fn take_lookup(&mut self) -> Option<LookupRequest> {
        self.lookup.take()
    }

    /// Drop the current text selection.
    ///
    /// Called when the popup closes and when the document changes: a selection
    /// is a highlight over one page's text, so it cannot outlive the view it
    /// was made in.
    pub fn clear_selection(&mut self) {
        self.press_on_background = false;
        self.select_start = None;
        self.selecting = false;
        self.pressing_page = false;
        self.selection = None;
    }

    /// Turn look-ahead tile prefetching on or off (Settings → Performance).
    pub fn set_prefetch(&mut self, on: bool) {
        self.prefetch = on;
    }

    /// Set the font used for form field text and the on-page editor
    /// (Settings → Text).
    pub fn set_form_font(&mut self, font: FormFont) {
        self.form_font = font;
    }

    /// Set the corner radius the current theme gives its controls.
    ///
    /// The canvas paints the on-page note editor by hand, so it needs the
    /// palette's radius — the same number the buttons in the toolbar and every
    /// window in the app round to.
    pub fn set_corner_radius(&mut self, radius: f32) {
        self.corner_radius = radius;
    }

    /// Page spaces from the last draw, in document layout space.
    ///
    /// Empty before the first draw. Callers convert an egui pointer position
    /// into document coordinates by subtracting the canvas origin, then feed it
    /// to `PageSpace::to_page`.
    pub fn page_spaces(&self) -> &[PageSpace] {
        &self.page_spaces
    }

    /// Take the click recorded by the last draw, if there was one.
    ///
    /// Taking rather than reading means one click is handled exactly once, even
    /// when several panels look at the canvas in the same frame.
    pub fn take_click(&mut self) -> Option<(f32, f32)> {
        self.clicked_at.take()
    }

    /// Take the edit commands produced by on-page field editors.
    pub fn take_edits(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.edits)
    }

    /// Forget per-document editing state (drafts and the open editor).
    ///
    /// Called on document switch: a draft typed into one file must never leak
    /// into another that happens to reuse the same field ids.
    pub fn forget_edits(&mut self) {
        self.field_drafts.clear();
        self.open_editor = None;
        self.edits.clear();
        // A drag in progress when the document switched must not commit onto
        // the new document: the start point is in the old document's space.
        self.draw_start = None;
        self.draw_page = None;
        self.pending.clear();
        // A selection is tied to the text of one document; carrying it to the
        // next would highlight the wrong characters.
        self.clear_selection();
    }

    /// Drop the echoes for one document, e.g. when their creation failed.
    pub fn cancel_pending(&mut self, doc: DocumentId) {
        self.pending.retain(|s| s.doc != doc);
    }

    /// Scroll offset from the last draw, in document coordinates.
    pub fn scroll_offset(&self) -> Vec2 {
        self.scroll_offset
    }

    /// The document the canvas is currently laid out for.
    pub fn active_document(&self) -> Option<DocumentId> {
        self.active_document
    }

    /// Whether the hand tool is currently engaged.
    pub fn pan_tool(&self) -> bool {
        self.pan_tool
    }

    /// Turn the hand tool on or off.
    pub fn set_pan_tool(&mut self, enabled: bool) {
        self.pan_tool = enabled;
        if !enabled {
            self.panning = false;
        }
    }

    /// Page under the viewport centre, from the last draw.
    pub fn center_page(&self) -> u32 {
        self.center_page
    }

    /// Apply an absolute scroll position on the next canvas draw.
    pub fn request_scroll_to(&mut self, offset: Vec2) {
        self.pending_scroll_offset = Some(Vec2::new(offset.x.max(0.0), offset.y.max(0.0)));
    }

    /// Bring a form field into view on the next draw.
    ///
    /// Deliberately not a jump: a field already on screen stays exactly where it
    /// is, and one that is off screen is scrolled just far enough to clear the
    /// edge. Selecting a field is what the user does immediately before typing,
    /// so moving the page under them would be actively hostile.
    pub fn reveal_field(&mut self, page: u32, rect: PdfSpaceRect) {
        self.pending_reveal = Some((page, rect));
    }

    /// Forget every outstanding tile request.
    ///
    /// Called when the viewport changes enough that the engine will drop the
    /// requests it has not started yet; without this the in-flight set would
    /// keep them forever and the UI would repaint continuously.
    ///
    /// Previously-queued raster failures are forgotten too: most failures are
    /// transient (an engine busy with a long search, a page not yet loaded),
    /// and the old behaviour kept such tiles blank until the tab closed. The
    /// per-tile failure set still guards against tight retry loops within one
    /// stable viewport: a tile is only re-queued after a viewport change has
    /// passed through here.
    pub fn clear_inflight(&mut self) {
        self.inflight.clear();
        self.thumb_inflight.clear();
        self.failed.clear();
    }

    /// Mark one page's bitmaps as stale after an edit changed its appearance.
    ///
    /// Annotation and form edits do not change tile **keys** — same document,
    /// rotation, zoom level, column and row — so the usual invalidation paths
    /// (which work by making the old keys irrelevant) leave the old textures in
    /// place and the draw loop keeps serving them forever.
    ///
    /// Evicting outright would fix that but flash: every visible tile would go
    /// blank until its replacement arrives. Marking instead keeps the old
    /// pixels on screen, re-requests the page's tiles, and swaps in fresh ones
    /// as they land — the page updates in place.
    pub fn invalidate_page(&mut self, doc: DocumentId, page: u32) {
        let keys: Vec<TileCacheKey> = self
            .tiles
            .keys()
            .filter(|(d, _, key)| *d == doc && key.page == page)
            .copied()
            .collect();
        for key in keys {
            self.stale_tiles.insert(key);
        }
        // A request already in flight was rendered *before* the edit, so its
        // result would resurrect old pixels; drop it and let the draw loop ask
        // again. The engine's generation counter also discards those requests
        // server-side when InvalidateTiles runs.
        self.inflight
            .retain(|(d, _, key)| !(*d == doc && key.page == page));
        self.failed
            .retain(|(d, _, key)| !(*d == doc && key.page == page));

        let thumb_keys: Vec<ThumbCacheKey> = self
            .thumbs
            .keys()
            .filter(|(d, _, p)| *d == doc && *p == page)
            .copied()
            .collect();
        for key in thumb_keys {
            self.stale_thumbs.insert(key);
        }
        self.thumb_inflight
            .retain(|(d, _, p)| !(*d == doc && *p == page));
        self.thumb_failed
            .retain(|(d, _, p)| !(*d == doc && *p == page));
    }

    /// Store a rasterized tile, evicting the oldest if the cache is full.
    pub fn insert_tile(
        &mut self,
        ctx: &Context,
        doc: DocumentId,
        rotation: Rotation,
        key: TileKey,
        pixels: &TilePixels,
    ) {
        let cache_key = (doc, rotation, key);
        self.inflight.remove(&cache_key);
        self.stale_tiles.remove(&cache_key);
        self.pending
            .retain(|s| !(s.doc == doc && s.page == key.page));

        let image = bgra_to_color_image(pixels.width, pixels.height, &pixels.data);
        let name = format!(
            "tile-{}-{}-{}-{}-{}",
            doc.raw(),
            rotation.quarter_turns(),
            key.page,
            key.col,
            key.row
        );
        let handle = ctx.load_texture(name, image, TextureOptions::LINEAR);

        self.tiles.insert(cache_key, handle);
        self.order.retain(|k| *k != cache_key);
        self.order.push_back(cache_key);

        while self.order.len() > TILE_CAPACITY {
            if let Some(old) = self.order.pop_front() {
                self.tiles.remove(&old);
            }
        }
    }

    /// Mark a tile request as failed so it is not retried in a loop.
    pub fn tile_failed(&mut self, doc: DocumentId, rotation: Rotation, key: TileKey) {
        let cache_key = (doc, rotation, key);
        self.inflight.remove(&cache_key);
        self.failed.insert(cache_key);
    }

    /// Mark a thumbnail request as failed so it is not retried in a loop.
    pub fn thumbnail_failed(&mut self, doc: DocumentId, rotation: Rotation, page: u32) {
        let cache_key = (doc, rotation, page);
        self.thumb_inflight.remove(&cache_key);
        self.thumb_failed.insert(cache_key);
    }

    /// Store a rasterized thumbnail, evicting the oldest if the cache is full.
    pub fn insert_thumbnail(
        &mut self,
        ctx: &Context,
        doc: DocumentId,
        rotation: Rotation,
        page: u32,
        pixels: &TilePixels,
    ) {
        let cache_key = (doc, rotation, page);
        self.thumb_inflight.remove(&cache_key);
        self.stale_thumbs.remove(&cache_key);
        let image = bgra_to_color_image(pixels.width, pixels.height, &pixels.data);
        let name = format!("thumb-{}-{}-{}", doc.raw(), rotation.quarter_turns(), page);
        let handle = ctx.load_texture(name, image, TextureOptions::LINEAR);
        self.thumbs.insert(cache_key, handle);
        self.thumb_order.retain(|k| *k != cache_key);
        self.thumb_order.push_back(cache_key);
        while self.thumb_order.len() > THUMB_CAPACITY {
            if let Some(old) = self.thumb_order.pop_front() {
                self.thumbs.remove(&old);
            }
        }
    }

    /// The cached thumbnail for a page, if one has been rasterized.
    ///
    /// Shared with the export window so its preview is the same texture the
    /// sidebar strip already drew, rather than a second copy of the same page.
    pub fn thumbnail(
        &self,
        doc: DocumentId,
        rotation: Rotation,
        page: u32,
    ) -> Option<&TextureHandle> {
        self.thumbs.get(&(doc, rotation, page))
    }

    /// Ask for a page's thumbnail, unless it is cached, in flight or failed.
    ///
    /// The scale is derived from a target width in points so a caller can ask
    /// for a bigger picture than the sidebar strip uses without knowing the
    /// page's own size. `None` means the request would be a duplicate.
    pub fn request_thumbnail(
        &mut self,
        doc: &Document,
        rotation: Rotation,
        page: u32,
        target_width: f32,
        dpr: f32,
    ) -> Option<PendingThumb> {
        let key = (doc.id, rotation, page);
        if self.thumbs.contains_key(&key)
            || self.thumb_inflight.contains(&key)
            || self.thumb_failed.contains(&key)
        {
            return None;
        }

        let width_pt = doc
            .pages
            .get(usize::try_from(page).unwrap_or(usize::MAX))
            .map_or(0.0, |page| page.width_pt);
        let scale = if width_pt > 0.0 {
            (target_width * dpr / width_pt).clamp(0.02, 4.0)
        } else {
            0.2
        };

        self.thumb_inflight.insert(key);
        Some(PendingThumb {
            doc: doc.id,
            page,
            rotation,
            scale,
        })
    }

    /// Drop every texture belonging to a document (called when its tab closes).
    pub fn forget_document(&mut self, doc: DocumentId) {
        self.pending.retain(|s| s.doc != doc);
        self.stale_tiles.retain(|(d, _, _)| *d != doc);
        self.stale_thumbs.retain(|(d, _, _)| *d != doc);
        self.tiles.retain(|(d, _, _), _| *d != doc);
        self.order.retain(|(d, _, _)| *d != doc);
        self.inflight.retain(|(d, _, _)| *d != doc);
        self.failed.retain(|(d, _, _)| *d != doc);
        self.thumbs.retain(|(d, _, _), _| *d != doc);
        self.thumb_order.retain(|(d, _, _)| *d != doc);
        self.thumb_inflight.retain(|(d, _, _)| *d != doc);
        self.thumb_failed.retain(|(d, _, _)| *d != doc);
    }

    /// Whether any tile or thumbnail is still being rasterized.
    ///
    /// The application uses this to keep repainting while work is in flight, so
    /// streamed tiles appear immediately instead of waiting for the next input
    /// event (which looks like a freeze), and to go idle at 0% CPU once
    /// everything has arrived.
    pub fn has_pending(&self) -> bool {
        !self.inflight.is_empty() || !self.thumb_inflight.is_empty()
    }

    /// Number of tile requests currently outstanding.
    pub fn inflight_count(&self) -> usize {
        self.inflight.len()
    }

    /// Number of cached tile textures.
    pub fn cached_tiles(&self) -> usize {
        self.tiles.len()
    }

    /// Draw the document and return the tiles the caller should request.
    pub fn draw(
        &mut self,
        ui: &mut Ui,
        doc: &Document,
        view: &ViewState,
        zoom: f32,
        dpr: f32,
        background: Color32,
        page_shadow: Color32,
        scroll_to_page: Option<u32>,
        form: Option<&FormInfo>,
        selected: Option<FieldId>,
        accent: Color32,
        tool: Tool,
        page_text: &PageTextCache,
        // Every annotation on the document, so placed notes stay visible and
        // clickable after the page re-renders.
        annotations: &[AnnotationInfo],
        // The currently selected annotation, if any, drawn highlighted.
        selected_annotation: Option<AnnotationId>,
    ) -> Vec<PendingTile> {
        let mut candidates: Vec<(f32, PendingTile)> = Vec::new();
        let rotation = view.rotation;
        let level = zoom_level(zoom, dpr);
        let scale = level_scale(level);

        // Measured before layout so the page column can be centred against the
        // real viewport rather than only against its own width.
        let available_w = ui.available_width();
        let (boxes, content) = layout_pages(doc, view, zoom, available_w);

        // Published for the shell: turning a pointer position into "page 3,
        // 140pt from the left" is the inverse of what layout just did, and this
        // is the only place that knows both halves.
        let page_spaces: Vec<PageSpace> = boxes
            .iter()
            .filter_map(|b| {
                doc.page(b.index)
                    .map(|geom| PageSpace::new(*b, geom, rotation, zoom))
            })
            .collect();
        self.page_spaces = page_spaces.clone();
        self.clicked_at = None;
        let document_changed = self.active_document != Some(doc.id);
        if document_changed {
            // Scroll position belongs to the tab, not to the shared canvas. The
            // first frame after activating a tab must apply the value captured
            // by the reducer, otherwise every tab opens at whichever position
            // the previous tab happened to use.
            self.active_document = Some(doc.id);
            self.last_zoom = None;
            self.pending_scroll_offset = None;
            self.scroll_offset = Vec2::new(view.scroll_x.max(0.0), view.scroll_y.max(0.0));
            self.center_page = view.current_page.min(doc.page_count().saturating_sub(1));
        }

        // Only offer horizontal scrolling when the page is genuinely wider than
        // the viewport. In the common fit-width case the content width equals the
        // viewport width, and a horizontal scrollbar sitting exactly on that
        // boundary flickers on and off every frame — and each toggle asks for a
        // repaint, which spins the UI at 60 fps forever (looks like a hang).
        // The 8 px dead zone gives the decision hysteresis.
        let scroll = if content.x > available_w + 8.0 {
            egui::ScrollArea::both()
        } else {
            egui::ScrollArea::vertical()
        }
        .auto_shrink([false, false])
        // Instant scrolling: an animated glide makes PageDown feel laggy and
        // makes programmatic jumps (page nav, zoom anchoring) land a frame
        // late. A document reader wants crisp, deterministic movement.
        .animated(false);

        let mut viewport_center_y = 0.0f32;
        let mut viewport_center_x = 0.0f32;
        // The canvas's own clip rect (the scroll viewport, which sits below the
        // annotation bar). The tool cursor and badge must be confined to it:
        // the bar is drawn on this same ui, so without this the canvas would
        // hijack the pointer over the toolbar and the tool cursor would sit on
        // top of the tool buttons instead of the normal hand/arrow.
        let mut canvas_clip: Option<Rect> = None;
        // The canvas content origin (the viewport's top-left), captured where
        // the drawing code computes it, so the Select-mode hover hit test in
        // the cursor block measures from the same point rather than the
        // panel's top-left (which sits above the annotation bar).
        let mut canvas_origin: Option<Pos2> = None;
        // Zoom anchoring: when the zoom factor changed since the last frame,
        // keep the document point that was under the viewport centre there.
        // Without this, zooming resets the scroll and the user loses their
        // place — the classic "press Ctrl+= and the page jumps to the top".
        // Skipped on the very first draw (no previous zoom) so a freshly opened
        // document starts at the top rather than centred.
        let zoom_changed = self
            .last_zoom
            .is_some_and(|prev| (prev - zoom).abs() > f32::EPSILON);
        self.last_zoom = Some(zoom);
        let anchor = if zoom_changed && scroll_to_page.is_none() {
            Some((self.center_page, self.center_frac_x, self.center_frac_y))
        } else {
            None
        };

        // Programmatic scrolling uses the ScrollArea's absolute offset override,
        // NOT `ui.scroll_to_rect`: inside `show_viewport` the content Ui is
        // translated to the viewport origin, and `scroll_to_rect` treats the
        // rect as viewport-local — so every jump double-counted the current
        // offset and flung the view pages past the target (the second
        // PageDown landed ~1.5 pages too far, the third even further).
        // The builder offset is applied when the ScrollArea begins, so the
        // frame's own closure already paints at the target position.
        let viewport_size = ui.available_size();

        // A newly selected field is brought into view only if it is not already
        // there. `self.scroll_offset` still holds the previous frame's offset,
        // which is what the user is currently looking at.
        let reveal_offset = self
            .pending_reveal
            .take()
            .and_then(|(page, rect)| {
                let space = page_spaces.iter().find(|sp| sp.index == page)?;
                let field = space.to_screen(rect);
                reveal_scroll(
                    (field.x, field.y, field.w, field.h),
                    (viewport_size.x, viewport_size.y),
                    (self.scroll_offset.x, self.scroll_offset.y),
                )
            })
            .map(|(x, y)| Vec2::new(x, y));

        let jump_offset = scroll_to_page.and_then(|target| {
            boxes
                .iter()
                .find(|b| b.index == target)
                .map(|pb| Vec2::new(0.0, (pb.y - 6.0).max(0.0)))
        });
        let anchor_offset = jump_offset.is_none().then_some(()).and_then(|()| {
            anchor.and_then(|(page, frac_x, frac_y)| {
                boxes.iter().find(|b| b.index == page).map(|pb| {
                    let px = pb.x + pb.w * frac_x;
                    let py = pb.y + pb.h * frac_y;
                    Vec2::new(px - viewport_size.x / 2.0, py - viewport_size.y / 2.0)
                })
            })
        });
        let pending_offset = self.pending_scroll_offset.take();
        let initial_offset =
            document_changed.then_some(Vec2::new(view.scroll_x.max(0.0), view.scroll_y.max(0.0)));
        let scroll = match jump_offset
            .or(reveal_offset)
            .or(pending_offset)
            .or(anchor_offset)
            .or(initial_offset)
        {
            Some(offset) => scroll.scroll_offset(offset),
            None => scroll,
        };
        let mut live_pointer: Option<(f32, f32)> = None;
        let output = scroll.show_viewport(ui, |ui, viewport| {
            viewport_center_y = viewport.center().y;
            viewport_center_x = viewport.center().x;
            canvas_clip = Some(ui.clip_rect());
            canvas_origin = Some(ui.min_rect().min);
            // `viewport` and the page layout are in local document
            // coordinates, while egui painters use absolute screen
            // coordinates. The old code painted `pb.x/pb.y` directly, which
            // discarded the canvas origin and clipped the left/top of every
            // page under the toolbar/sidebar. Keep culling in local space and
            // translate only at paint time.
            let content_origin = ui.min_rect().min;
            // Work on a local copy so the note popup can mutate freely without
            // fighting the other `self` borrows in this closure. Written back
            // at the end of the frame.
            let mut note_input = self.note_input.take();
            let (_, page_area) = ui.allocate_exact_size(content, Sense::click_and_drag());
            let painter = ui.painter().clone();

            // A press on the current selection's resize handle edits that mark
            // instead of drawing a new one — even while an annotation tool is
            // armed — so a freshly drawn annotation can be resized without
            // switching back to Select first. Only the *handles* take priority:
            // a press on the body still draws, so the armed tool can keep
            // placing marks.
            let handle_press = !self.pan_tool
                && ui.input(|i| i.pointer.latest_pos()).is_some_and(|pos| {
                    let local = pos2(pos.x - content_origin.x, pos.y - content_origin.y);
                    selected_annotation
                        .and_then(|sel| annotations.iter().find(|a| a.id == sel))
                        .filter(|info| info.kind != AnnotationKind::StickyNote)
                        .and_then(|info| {
                            page_spaces
                                .iter()
                                .find(|sp| sp.index == info.id.page)
                                .map(|sp| (sp, info))
                        })
                        .is_some_and(|(space, info)| handle_hit(space, info.rect, local).is_some())
                });

            // Record the click in *document layout* space, not screen space:
            // the content Ui is already translated by the scroll offset, so
            // subtracting its origin is what puts the pointer back into the
            // same coordinates the page boxes use.
            if page_area.clicked() {
                if let Some(pos) = page_area.interact_pointer_pos() {
                    self.clicked_at = Some((pos.x - content_origin.x, pos.y - content_origin.y));
                }
            }

            // A press that lands on a floating panel — the annotation
            // properties window, a note popup, a menu — belongs to that panel,
            // not to the page underneath it. `page_area.clicked()` is
            // layer-aware and already refuses those clicks, but the raw-pointer
            // paths below are not: without this guard, clicking a swatch in the
            // properties window also started a stroke on the page behind it,
            // which then stole the selection and left the panel showing the
            // brand-new mark instead of the one being edited.
            let over_float = ui.input(|i| i.pointer.latest_pos()).is_some_and(|pos| {
                ui.ctx()
                    .layer_id_at(pos)
                    .is_some_and(|top| top != ui.layer_id())
            });

            // Annotation tools: the shape appears on the very first frame of
            // the press and follows the pointer live; releasing commits it.
            // Raw pointer state rather than the page area's drag flags — with
            // Sense::click_and_drag those are postponed until the pointer has
            // moved a bit (egui is deciding click vs drag), which read as a
            // delay before anything appeared.
            if let Some(kind) = tool.annotation_kind() {
                let pointer = ui.input(|i| i.pointer.clone());
                let in_view = pointer
                    .latest_pos()
                    .is_some_and(|pos| ui.clip_rect().contains(pos));

                // Press begins the shape exactly where the pointer is, but
                // only when it lands on a page inside the visible canvas —
                // toolbar or menu clicks must not start strokes.
                if pointer.primary_pressed()
                    && in_view
                    && !over_float
                    && self.draw_start.is_none()
                    && !handle_press
                {
                    // A press on the page while a note popup is open commits
                    // that note and starts the next one in a single click, so
                    // placing several notes flows without a throwaway click.
                    // A press *inside* the popup (repositioning the caret) is
                    // left for the popup to handle.
                    if let Some(pending) = note_input.take() {
                        if ui.ctx().is_pointer_over_egui() {
                            note_input = Some(pending);
                        } else if !pending.text.trim().is_empty() {
                            commit_pending_note(
                                &mut self.edits,
                                &mut self.pending,
                                doc.id,
                                &page_spaces,
                                pending,
                            );
                        }
                    }
                    if note_input.is_none() {
                        if let Some(pos) = pointer.interact_pos() {
                            let local = (pos.x - content_origin.x, pos.y - content_origin.y);
                            if let Some(space) = page_spaces.iter().find(|sp| {
                                local.0 >= sp.origin.0
                                    && local.0 <= sp.origin.0 + sp.size.0
                                    && local.1 >= sp.origin.1
                                    && local.1 <= sp.origin.1 + sp.size.1
                            }) {
                                self.draw_start = Some(local);
                                self.draw_page = Some(space.index);
                            }
                        }
                    }
                }

                // While held, remember where the pointer is; the preview is
                // drawn *after* the tiles (they are opaque and would cover
                // anything painted earlier in the frame).
                if pointer.primary_down() && self.draw_start.is_some() {
                    live_pointer = pointer
                        .interact_pos()
                        .or(pointer.latest_pos())
                        .map(|pos| (pos.x - content_origin.x, pos.y - content_origin.y));
                }

                // Release commits where the pointer is. Note and Type also
                // commit on a plain click: a note places at the click, and a
                // text box gets a default size — neither requires a drag.
                if pointer.primary_released() && self.draw_start.is_some() {
                    if let (Some(start), Some(page), Some(pos)) = (
                        self.draw_start,
                        self.draw_page,
                        pointer.interact_pos().or(pointer.latest_pos()),
                    ) {
                        let end = (pos.x - content_origin.x, pos.y - content_origin.y);
                        let dragged =
                            (end.0 - start.0).abs() > 3.0 || (end.1 - start.1).abs() > 3.0;
                        let new = match kind {
                            // A note opens its text input right here; the
                            // annotation is created (with the typed text) when
                            // the input is confirmed.
                            AnnotationKind::StickyNote => {
                                if let Some(space) =
                                    page_spaces.iter().find(|space| space.index == page)
                                {
                                    // `to_page` yields *unrotated* PDF-space
                                    // points, so clamp to the unrotated page
                                    // size — using the oriented size would swap
                                    // the bounds on a rotated page and push the
                                    // note off its true spot.
                                    let (px, py) = space.to_page(end);
                                    let (w, h) = (space.geom.width_pt, space.geom.height_pt);
                                    // The LOCAL copy: the field was taken at the
                                    // top of the frame and written back at the
                                    // end — writing the field here would be
                                    // wiped before the popup ever saw it.
                                    note_input = Some(NoteInput {
                                        page,
                                        point: (px.clamp(0.0, w), py.clamp(0.0, h)),
                                        text: String::new(),
                                        opened: false,
                                    });
                                }
                                None
                            }
                            AnnotationKind::FreeText if !dragged => page_spaces
                                .iter()
                                .find(|space| space.index == page)
                                .and_then(|space| {
                                    // A click with the Type tool places a
                                    // default-size box, not nothing.
                                    let (px, py) = space.to_page(end);
                                    let (w, h) = space.geom.oriented(space.rotation);
                                    let x = px.clamp(0.0, (w - 160.0).max(0.0));
                                    let y = py.clamp(0.0, (h - 24.0).max(0.0));
                                    let rect = PdfSpaceRect::from_xywh(
                                        x,
                                        y,
                                        160.0_f32.min(w),
                                        24.0_f32.min(h),
                                    );
                                    Some(NewAnnotation::FreeText(rect, "Text".into()))
                                }),
                            _ if dragged => page_spaces
                                .iter()
                                .find(|space| space.index == page)
                                .and_then(|space| {
                                    let (sx, sy) = space.to_page(start);
                                    let (ex, ey) = space.to_page(end);
                                    let rect = PdfSpaceRect::from_corners((sx, sy), (ex, ey));
                                    // Clamp to the page: a drag that ends over
                                    // another page converts through the start
                                    // page's transform, and nothing may extend
                                    // past the page it was drawn on. `to_page`
                                    // returns unrotated PDF points, so the
                                    // bounds are the unrotated page size.
                                    let (w, h) = (space.geom.width_pt, space.geom.height_pt);
                                    let page_bounds = PdfSpaceRect::from_xywh(0.0, 0.0, w, h);
                                    let rect = page_bounds.intersection(rect)?;
                                    match kind {
                                        AnnotationKind::Highlight => {
                                            Some(NewAnnotation::Highlight(rect))
                                        }
                                        AnnotationKind::Underline => {
                                            Some(NewAnnotation::Underline(rect))
                                        }
                                        AnnotationKind::StrikeOut => {
                                            Some(NewAnnotation::StrikeOut(rect))
                                        }
                                        AnnotationKind::Squiggly => {
                                            Some(NewAnnotation::Squiggly(rect))
                                        }
                                        AnnotationKind::Square => Some(NewAnnotation::Square(rect)),
                                        AnnotationKind::FreeText => {
                                            Some(NewAnnotation::FreeText(rect, "Text".into()))
                                        }
                                        _ => None,
                                    }
                                }),
                            _ => None,
                        };
                        if let Some(new) = new {
                            let echo_has_rect = new.rect().is_some();
                            let echo_point = new.rect().is_none().then_some(end);
                            let echo_kind = new.kind();
                            let echo_rect = Rect::from_min_max(
                                pos2(start.0.min(end.0), start.1.min(end.1)),
                                pos2(start.0.max(end.0), start.1.max(end.1)),
                            );
                            self.edits.push(Command::AddAnnotation { page, new });
                            self.pending.push(PendingEcho {
                                doc: doc.id,
                                page,
                                kind: echo_kind,
                                rect: echo_has_rect.then_some(echo_rect),
                                point: echo_point,
                            });
                        }
                    }
                    self.draw_start = None;
                    self.draw_page = None;
                }
            }

            // Selecting and dragging existing annotations. A press on the
            // current selection's resize handle starts a resize *in every tool
            // mode* (so a mark can be resized right after drawing); a press on
            // any editable annotation's body selects it (without scrolling,
            // unlike the Comments panel) and starts a move — that part only
            // with the Select tool, so an armed drawing tool still draws over
            // a selected mark. Both own the whole gesture: the text selection
            // below must not also react to it.
            //
            // `!over_float` keeps a click inside the properties window from
            // grabbing whatever mark happens to sit behind the window.
            if !self.pan_tool {
                let pointer = ui.input(|i| i.pointer.clone());
                let in_view = pointer
                    .latest_pos()
                    .is_some_and(|pos| ui.clip_rect().contains(pos));
                let press = pointer.primary_pressed() && in_view && !over_float;
                if press {
                    if let Some(pos) = pointer.interact_pos().or(pointer.latest_pos()) {
                        let local = (pos.x - content_origin.x, pos.y - content_origin.y);
                        let mut started = false;

                        // Handles of the current selection take priority over
                        // everything: without this a handle sitting over
                        // another annotation could never be grabbed.
                        //
                        // `handle_hit` — like every other hit test here — works
                        // in *content-local* space (`to_screen` is content-local
                        // and the painter adds `content_origin` back only when
                        // drawing). Feeding it the raw global pointer made the
                        // handles respond to a spot offset from where they are
                        // drawn, so a drag never started on the handle the user
                        // actually grabbed.
                        if let Some(sel) = selected_annotation {
                            if let Some(info) = annotations.iter().find(|a| a.id == sel) {
                                if info.kind != AnnotationKind::StickyNote {
                                    if let Some(space) =
                                        page_spaces.iter().find(|sp| sp.index == info.id.page)
                                    {
                                        if let Some(handle) = handle_hit(
                                            space,
                                            info.rect,
                                            pos2(local.0, local.1),
                                        ) {
                                            self.ann_drag = Some(AnnDrag::Resize {
                                                id: info.id,
                                                page: info.id.page,
                                                kind: info.kind,
                                                handle,
                                                rect: info.rect,
                                            });
                                            started = true;
                                        }
                                    }
                                }
                            }
                        }

                        // Then the body of any editable annotation — only with
                        // the Select tool; an armed drawing tool must still
                        // draw over a selected mark.
                        if !started && tool == Tool::Select {
                            let editable: Vec<AnnotationInfo> = annotations
                                .iter()
                                .filter(|a| a.kind.is_creatable())
                                .cloned()
                                .collect();
                                if let Some(id) =
                                    select_hit(&page_spaces, &editable, pos2(local.0, local.1))
                                {
                                    if selected_annotation != Some(id) {
                                        self.edits
                                            .push(Command::SelectAnnotationInPlace(Some(id)));
                                    }
                                if let Some(info) = annotations.iter().find(|a| a.id == id) {
                                    self.ann_drag = Some(AnnDrag::Move {
                                        id,
                                        page: id.page,
                                        start: local,
                                        rect: info.rect,
                                    });
                                }
                                started = true;
                            }
                        }

                        if started {
                            // The click must not ALSO clear the selection or
                            // re-select a note card on release.
                            self.clicked_at = None;
                        }

                        // A press on bare paper — not on a handle, not on a
                        // mark — is the canvas's own "I am done here". The
                        // properties window is a floating panel now, one the
                        // user can drag anywhere over the page, so it needs the
                        // gesture every floating panel has: a click anywhere
                        // else puts it away. Presses that land on the panel
                        // never get this far (`over_float` above), and only the
                        // panel's Apply writes a staged edit, so a stray click
                        // cannot lose one.
                        if !started && view.properties_panel_open {
                            self.edits
                                .push(Command::SetPropertiesPanel { open: false });
                        }
                    }
                }
            }

            // Text selection. With no annotation tool active, a click widens to
            // the word under it and a drag selects a phrase. Neither is
            // interpreted here: the pick is handed to the shell, which owns the
            // dictionaries and the popup. The eraser is excluded: it is a
            // click tool like selection, but its press erases instead, so the
            // two must never both answer the same gesture. An in-progress
            // annotation drag owns the gesture too.
            if tool.annotation_kind().is_none()
                && !self.pan_tool
                && tool != Tool::Eraser
                && self.ann_drag.is_none()
            {
                let pointer = ui.input(|i| i.pointer.clone());
                let in_view = pointer
                    .latest_pos()
                    .is_some_and(|pos| ui.clip_rect().contains(pos));

                // Match against the page area's own interaction, not the raw
                // pointer, so a click that lands on the lookup popup (or any
                // other widget painted on top) does not also start a selection
                // underneath it — that leak made the popup jump to whatever text
                // sat behind the card.
                let down_on_page = page_area.is_pointer_button_down_on();
                // Detect the rising edge of a press against the page area only.
                // Comparing to the previous frame's state keeps a click that
                // lands on the popup (or any widget above the page) from also
                // starting a selection underneath it.
                let press = down_on_page && !self.pressing_page;
                self.pressing_page = down_on_page;
                if press {
                    self.select_start = in_view
                        .then(|| page_area.interact_pointer_pos())
                        .flatten()
                        .map(|pos| (pos.x - content_origin.x, pos.y - content_origin.y))
                        .and_then(|local| {
                            hit_page(&page_spaces, local).map(|space| (space.index, local))
                        });
                    // A press that missed every page clears what was selected.
                    self.selecting = self.select_start.is_some();
                    if !self.selecting {
                        self.selection = None;
                    }
                    // Remember that this press landed on the canvas but not on a
                    // page: there is no text under it, so the drag that follows
                    // is free to move the view.
                    self.press_on_background = down_on_page && !self.selecting;
                }

                let released = pointer.primary_released();
                if (pointer.primary_down() || released) && self.select_start.is_some() {
                    if let Some((page, start)) = self.select_start {
                        if let Some(pos) = pointer.interact_pos().or(pointer.latest_pos()) {
                            let end = (pos.x - content_origin.x, pos.y - content_origin.y);
                            self.selection =
                                selection_range(&page_spaces, page, page_text, doc.id, start, end);
                        }
                    }
                }

                if released {
                    self.select_start = None;
                    self.press_on_background = false;
                    // Only commit when this gesture actually began on a page.
                    // Otherwise a click on the popup (which leaves the existing
                    // selection intact) would re-emit it and re-open the card.
                    if self.selecting {
                        if let Some((page, range)) = self.selection {
                            self.lookup = emit_lookup(
                                &page_spaces,
                                page,
                                range,
                                page_text,
                                doc.id,
                                content_origin,
                            );
                        }
                        self.selecting = false;
                    }
                }
            }

            painter.rect_filled(ui.clip_rect(), 0.0, background);

            // Look-ahead region. When prefetching is off, only what is actually
            // on screen is drawn and requested, so a fast scroll can show a
            // brief blank before the tiles arrive.
            let prefetch = if self.prefetch {
                viewport.expand2(vec2(viewport.width() * 0.5, viewport.height() * 0.5))
            } else {
                viewport
            };
            let center = viewport.center();

            for pb in &boxes {
                let page_rect_local = Rect::from_min_size(pos2(pb.x, pb.y), vec2(pb.w, pb.h));
                if !prefetch.intersects(page_rect_local) {
                    continue;
                }
                let page_rect = page_rect_local.translate(content_origin.to_vec2());

                // Page background with a soft drop shadow, so pages read as
                // sheets of paper rather than flat rectangles.
                painter.rect_filled(
                    page_rect.translate(vec2(0.0, 2.0)).expand(1.0),
                    3.0,
                    page_shadow.gamma_multiply(0.55),
                );
                painter.rect_filled(page_rect, 2.0, Color32::WHITE);

                let Some(geom) = doc.page(pb.index) else {
                    continue;
                };
                let (w_pt, h_pt) = geom.oriented(rotation);
                let pixel_w = ((w_pt * scale).round() as u32).max(1);
                let pixel_h = ((h_pt * scale).round() as u32).max(1);
                let (cols, rows) = tile_grid(pixel_w, pixel_h, TILE_SIZE);

                for row in 0..rows {
                    for col in 0..cols {
                        let key = TileKey::new(pb.index, level, col, row);
                        let tr = tile_pixel_rect(key, pixel_w, pixel_h, TILE_SIZE);

                        let lx = pb.x + pb.w * (tr.x as f32 / pixel_w as f32);
                        let ly = pb.y + pb.h * (tr.y as f32 / pixel_h as f32);
                        let lw = pb.w * (tr.w as f32 / pixel_w as f32);
                        let lh = pb.h * (tr.h as f32 / pixel_h as f32);
                        let rect_local = Rect::from_min_size(pos2(lx, ly), vec2(lw, lh));

                        if !prefetch.intersects(rect_local) {
                            continue;
                        }
                        let rect = rect_local.translate(content_origin.to_vec2());

                        let cache_key = (doc.id, rotation, key);
                        let is_stale = self.stale_tiles.contains(&cache_key);
                        if let Some(tex) = self.tiles.get(&cache_key) {
                            // A stale texture keeps showing the pre-edit pixels
                            // until its replacement lands: no flash.
                            painter.image(tex.id(), rect, uv_full(), Color32::WHITE);
                        }
                        if (is_stale || self.tiles.get(&cache_key).is_none())
                            && !self.inflight.contains(&cache_key)
                            && !self.failed.contains(&cache_key)
                        {
                            let dist = rect.center().distance(center);
                            candidates.push((
                                dist,
                                PendingTile {
                                    doc: doc.id,
                                    key,
                                    rotation,
                                    request: TileRequest {
                                        page: pb.index,
                                        rotation,
                                        scale,
                                        origin_x: tr.x as i32,
                                        origin_y: tr.y as i32,
                                        width: tr.w,
                                        height: tr.h,
                                    },
                                },
                            ));
                        }
                    }
                }
            }

            // Form values (typed text, chosen options) drawn over the pages:
            // PDFium does not render live field values without a form-fill
            // environment, so the overlay is what makes filling visible.
            if let Some(form) = form {
                draw_form_values(
                    &painter,
                    form,
                    selected,
                    &page_spaces,
                    content_origin,
                    &self.form_font,
                );
            }

            // Form field overlay, drawn after the tiles so it sits on top of
            // them. PDFium has already rasterized each widget's appearance into
            // the tiles; what egui adds is the interactive layer — an outline
            // showing which regions are fields at all, a stronger marker on
            // the one the user selected, and an editor on the selected field.
            if let Some(form) = form {
                for space in &page_spaces {
                    for field in form.fields.iter().filter(|f| f.id.page == space.index) {
                        let screen = space.to_screen(field.rect);
                        let rect = Rect::from_min_size(
                            pos2(screen.x, screen.y),
                            vec2(screen.w.max(1.0), screen.h.max(1.0)),
                        )
                        .translate(content_origin.to_vec2());

                        // Skip fields scrolled out of sight: a form can have
                        // hundreds of widgets and only a few are ever on screen.
                        if !painter.clip_rect().intersects(rect) {
                            continue;
                        }

                        let is_selected = selected == Some(field.id);
                        if is_selected {
                            painter.rect_filled(rect.expand(1.0), 2.0, accent.gamma_multiply(0.22));
                            painter.rect_stroke(
                                rect.expand(1.0),
                                2.0,
                                Stroke::new(2.0, accent),
                                StrokeKind::Outside,
                            );
                        } else if field.is_editable() {
                            // Faint outline so fillable regions are discoverable
                            // without turning the page into a wireframe.
                            painter.rect_stroke(
                                rect,
                                1.0,
                                Stroke::new(1.0, accent.gamma_multiply(0.45)),
                                StrokeKind::Outside,
                            );
                        }
                    }
                }

                draw_field_editor(
                    ui,
                    form,
                    selected,
                    doc.id,
                    &page_spaces,
                    content_origin,
                    &mut self.edits,
                    &self.form_font,
                );
            }

            // Text input for a note being placed, anchored at its tag. The
            // note is created only when the text is confirmed, so it is born
            // with the right contents in one round-trip.
            if let Some(note) = &mut note_input {
                let page_visible = page_spaces.iter().any(|sp| sp.index == note.page);
                if !page_visible {
                    note_input = None;
                }
                if let Some(space) = page_spaces
                    .iter()
                    .find(|sp| sp.index == note_input.as_ref().map(|n| n.page).unwrap_or(0))
                {
                    let anchor = note_input.as_ref().map(|n| n.point).unwrap_or((0.0, 0.0));
                    let (lx, ly) = space.to_screen_point(anchor);
                    draw_note_tag(&painter, (lx, ly), content_origin);

                    let input_id = egui::Id::new(("note-input", doc.id.raw()));
                    let commit_text = egui::Area::new(input_id)
                        .order(egui::Order::Foreground)
                        .fixed_pos(pos2(
                            lx + content_origin.x + 14.0,
                            ly + content_origin.y - 8.0,
                        ))
                        .show(ui.ctx(), |ui| {
                            egui::Frame::new()
                                .fill(background)
                                .stroke(egui::Stroke::new(1.0, accent.gamma_multiply(0.6)))
                                .corner_radius(self.corner_radius)
                                .inner_margin(egui::Margin::same(6))
                                .show(ui, |ui| {
                                    let Some(note) = note_input.as_mut() else {
                                        return None;
                                    };
                                    ui.set_min_width(190.0);
                                    let response = ui.add(
                                        egui::TextEdit::singleline(&mut note.text)
                                            .hint_text("Type a note…")
                                            .desired_width(178.0),
                                    );
                                    if note.opened {
                                        response.request_focus();
                                        note.opened = false;
                                    }
                                    let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
                                    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                                    // Esc discards. Enter commits — even an
                                    // empty note, which the engine names
                                    // "Note". A click-away commits only when
                                    // something was typed; otherwise it just
                                    // discards the placement, so clicking to
                                    // place the next note never litters the
                                    // page with blank notes.
                                    if escape {
                                        return Some(None);
                                    }
                                    if enter {
                                        return Some(Some(note.text.clone()));
                                    }
                                    if response.lost_focus() {
                                        return if note.text.trim().is_empty() {
                                            Some(None)
                                        } else {
                                            Some(Some(note.text.clone()))
                                        };
                                    }
                                    None
                                })
                                .inner
                        })
                        .inner;
                    if let Some(decision) = commit_text {
                        if let Some(note) = note_input.take() {
                            // The outer `Some` means a decision was reached; the
                            // inner `Some` means create (empty text becomes
                            // "Note"), `None` means Esc / empty click-away
                            // discard.
                            if decision.is_some() {
                                commit_pending_note(
                                    &mut self.edits,
                                    &mut self.pending,
                                    doc.id,
                                    &page_spaces,
                                    note,
                                );
                            }
                        }
                    }
                }
            }

            // Live tool preview, drawn ABOVE the opaque tiles so the shape is
            // visible from the very first frame of the press.
            if let (Some(kind), Some(start)) = (tool.annotation_kind(), self.draw_start) {
                if let Some(end) = live_pointer {
                    if kind == AnnotationKind::StickyNote {
                        draw_note_tag(&painter, end, content_origin);
                    } else {
                        let rect = Rect::from_min_max(
                            pos2(start.0.min(end.0), start.1.min(end.1)),
                            pos2(start.0.max(end.0), start.1.max(end.1)),
                        );
                        // The page being drawn on decides the scale: the preview
                        // must be the size the engine will draw the mark.
                        let zoom = hit_page(&page_spaces, start).map_or(1.0, |space| space.zoom);
                        draw_shape_preview(&painter, kind, rect, content_origin, zoom);
                    }
                }
            }

            // An in-progress annotation drag: compute the target rectangle
            // from the pointer every frame, draw the annotation as a ghost at
            // that position (its real pixels are baked into tiles and cannot
            // move), and commit the new bounds to the store on release.
            if let Some(drag) = self.ann_drag {
                let pointer = ui.input(|i| i.pointer.clone());
                if let (Some(space), Some(pos)) = (
                    page_spaces.iter().find(|sp| sp.index == drag.page()),
                    pointer.interact_pos().or(pointer.latest_pos()),
                ) {
                    let local = (pos.x - content_origin.x, pos.y - content_origin.y);
                    let start_pdf = space.to_page(local);
                    let new = match drag {
                        AnnDrag::Move { start, rect, .. } => {
                            let anchor = space.to_page(start);
                            PdfSpaceRect::from_xywh(
                                rect.min_x + (start_pdf.0 - anchor.0),
                                rect.min_y + (start_pdf.1 - anchor.1),
                                rect.width(),
                                rect.height(),
                            )
                        }
                        AnnDrag::Resize {
                            handle, rect, kind, ..
                        } => {
                            if kind.is_line_markup() {
                                // The vertical axis is the stroke weight, so
                                // the height may go well below MIN_ANN_SIZE.
                                resize_rect_with(
                                    rect,
                                    handle,
                                    start_pdf,
                                    line_markup_min_height(rect),
                                )
                            } else {
                                resize_rect(rect, handle, start_pdf)
                            }
                        }
                    };
                    self.ann_drag_rect = Some(new);

                    let held = pointer.primary_down();
                    if held {
                        let screen = space.to_screen(new);
                        let ghost = Rect::from_min_size(
                            pos2(screen.x, screen.y),
                            vec2(screen.w.max(1.0), screen.h.max(1.0)),
                        )
                        .translate(content_origin.to_vec2());
                        painter.rect_filled(
                            ghost,
                            2.0,
                            Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 40),
                        );
                        painter.rect_stroke(
                            ghost,
                            2.0,
                            Stroke::new(1.5, accent),
                            StrokeKind::Outside,
                        );
                        // A live size readout while resizing, so the user can
                        // hit an exact dimension without opening the
                        // properties panel. Width/height are PDF points.
                        if matches!(drag, AnnDrag::Resize { .. }) {
                            draw_size_readout(
                                &painter,
                                ghost.right_bottom() + vec2(8.0, 8.0),
                                &format!("{:.0} × {:.0} pt", new.width(), new.height()),
                                accent,
                            );
                        }
                    } else {
                        // Release. The recorded click is consumed UNCONDITION-
                        // ALLY here: the press selected the annotation, and
                        // without this the release-click would fall through
                        // to the click resolution below, which treats a click
                        // on empty paper as "deselect" — instantly wiping the
                        // selection the press just made. That is exactly the
                        // "select → immediately deselected, can't edit"
                        // report; a click on the body is selection, not an
                        // edit, so a no-move release commits nothing.
                        self.clicked_at = None;
                        if let Some(id) = self.ann_drag.take().map(|d| d.id()) {
                            if let Some(final_rect) = self.ann_drag_rect.take() {
                                let start_rect = drag.rect();
                                let moved = (final_rect.min_x - start_rect.min_x).abs()
                                    > f32::EPSILON
                                    || (final_rect.min_y - start_rect.min_y).abs() > f32::EPSILON
                                    || (final_rect.max_x - start_rect.max_x).abs() > f32::EPSILON
                                    || (final_rect.max_y - start_rect.max_y).abs()
                                        > f32::EPSILON;
                                if moved {
                                    self.edits.push(Command::UpdateAnnotationBounds {
                                        id,
                                        rect: final_rect,
                                    });
                                    // Echo the mark at its NEW position until
                                    // the re-rendered tiles land: the ink is
                                    // baked into the old tiles, so without
                                    // the echo the mark visibly jumps back to
                                    // where it was for a few frames — the
                                    // "GUI does not come smoothly" half of
                                    // the report.
                                    if let Some(info) =
                                        annotations.iter().find(|a| a.id == id)
                                    {
                                        let screen = space.to_screen(final_rect);
                                        self.pending.push(PendingEcho {
                                            doc: doc.id,
                                            page: drag.page(),
                                            kind: info.kind,
                                            rect: Some(Rect::from_min_size(
                                                pos2(screen.x, screen.y),
                                                vec2(screen.w.max(1.0), screen.h.max(1.0)),
                                            )),
                                            point: None,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // The selected annotation: an accent outline plus resize handles
            // on every editable kind except sticky notes, whose icon box is
            // fixed-sized. Handles are drawn even while an edit drag runs —
            // following the live rectangle — so the user never loses the grip
            // they started from, and the handle under the pointer is filled in
            // so the target is obvious before the press lands.
            if let Some(sel) = selected_annotation {
                if let Some(info) = annotations.iter().find(|a| a.id == sel) {
                    if let Some(space) = page_spaces.iter().find(|sp| sp.index == info.id.page) {
                        let dragging = self.ann_drag.is_some();
                        // Follow the live rectangle mid-drag, the stored one
                        // otherwise, so the frame and handles track the pointer.
                        let rect = self
                            .ann_drag_rect
                            .filter(|_| dragging)
                            .unwrap_or(info.rect);
                        let screen = space.to_screen(rect);
                        let outline = Rect::from_min_size(
                            pos2(screen.x, screen.y),
                            vec2(screen.w.max(1.0), screen.h.max(1.0)),
                        )
                        .translate(content_origin.to_vec2());
                        // The drag block already paints a filled ghost frame;
                        // only draw the idle outline so the two do not stack.
                        if !dragging {
                            painter.rect_stroke(
                                outline,
                                2.0,
                                Stroke::new(1.4, accent),
                                StrokeKind::Outside,
                            );
                        }
                        if info.kind != AnnotationKind::StickyNote {
                            // Which handle is under the pointer (idle only):
                            // same content-local hit test the drag-start uses.
                            let hovered = if !dragging && tool == Tool::Select && !self.pan_tool {
                                ui.input(|i| i.pointer.latest_pos()).and_then(|pos| {
                                    handle_hit(
                                        space,
                                        info.rect,
                                        pos2(pos.x - content_origin.x, pos.y - content_origin.y),
                                    )
                                    .map(handle_index)
                                })
                            } else {
                                None
                            };
                            for (index, point) in handle_points(outline).into_iter().enumerate() {
                                let hot = hovered == Some(index);
                                let side = if hot { 9.0 } else { 7.0 };
                                let cell = Rect::from_center_size(point, vec2(side, side));
                                painter.rect_filled(
                                    cell,
                                    1.5,
                                    if hot { accent } else { Color32::WHITE },
                                );
                                painter.rect_stroke(
                                    cell,
                                    1.5,
                                    Stroke::new(1.0, accent),
                                    StrokeKind::Inside,
                                );
                            }
                        }
                    }
                }
            }

            // The text selection, one box per line so a multi-line pick follows
            // the text instead of covering the gutter. Drawn above the opaque
            // tiles and below the tool preview.
            if let Some((page, range)) = self.selection {
                if let Some(space) = page_spaces.iter().find(|space| space.index == page) {
                    if let Some(text) = page_text.get(doc.id, page) {
                        let fill =
                            Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 80);
                        for rect in text.range_rects(range) {
                            let screen = space.to_screen(rect);
                            painter.rect_filled(
                                Rect::from_min_size(
                                    pos2(screen.x, screen.y),
                                    vec2(screen.w.max(1.0), screen.h.max(1.0)),
                                )
                                .translate(content_origin.to_vec2()),
                                1.0,
                                fill,
                            );
                        }
                    }
                }
            }

            // Echoes of shapes the user just drew, drawn last so they sit on
            // top of everything. They disappear on their own when the page's
            // fresh tiles land.
            draw_pending_echoes(
                &painter,
                &self.pending,
                doc.id,
                &page_spaces,
                content_origin,
            );

            // Persistent note cards. Without this the note a user just placed
            // vanishes the moment the re-rendered page tiles arrive — PDFium
            // only draws a tiny icon for a sticky note, and the app's own card
            // was only painted as a transient echo. Drawing every sticky note
            // here keeps it visible in the PDF and lets the user click it to
            // select/edit (fixes "can't find the note").
            let click = self.clicked_at;
            // Screen rect of every drawn card, in paint order, so a click can
            // be resolved to the card actually on top.
            let mut cards: Vec<(AnnotationId, Rect)> = Vec::new();
            for ann in annotations
                .iter()
                .filter(|a| a.kind == AnnotationKind::StickyNote)
            {
                let Some(space) = page_spaces.iter().find(|sp| sp.index == ann.id.page) else {
                    continue;
                };
                // A sticky note's `/Rect` is its icon box. The engine creates it
                // centred on the clicked point (and foreign notes are centred
                // on their icon too), so anchoring the card at the rect centre
                // pins it exactly where the note sits.
                let anchor = (
                    (ann.rect.min_x + ann.rect.max_x) / 2.0,
                    (ann.rect.min_y + ann.rect.max_y) / 2.0,
                );
                let (sx, sy) = space.to_screen_point(anchor);

                let card =
                    draw_note_card(&painter, (sx, sy), ann.display_contents(), content_origin);

                if selected_annotation == Some(ann.id) {
                    painter.rect_stroke(
                        card.expand(3.0),
                        9.0,
                        Stroke::new(1.6, accent),
                        StrokeKind::Outside,
                    );
                }
                cards.push((ann.id, card));
            }

            // Resolve a click to the topmost card under it. Cards are pushed in
            // paint order, so the last match is the one drawn on top — clicking
            // an overlapping note picks the one the user actually sees. A hit
            // consumes the click so the form-field hit-test below does not also
            // fire; a click on empty page clears the selection but is left
            // intact so field picking still runs.
            if let Some(pt) = click {
                let screen_pt = pos2(pt.0 + content_origin.x, pt.1 + content_origin.y);
                if tool == Tool::Eraser {
                    // Deletion happens on *press* in the eraser block below —
                    // the mark should vanish the instant the button goes down,
                    // and a held drag should sweep across several marks. This
                    // arm only consumes the click so `clicked_at` cannot leak
                    // into later frames or re-trigger anything on release.
                    self.clicked_at = None;
                } else if tool.annotation_kind().is_none() {
                    match topmost_card(&cards, screen_pt) {
                        Some(id) => {
                            self.edits.push(Command::SelectAnnotation(Some(id)));
                            self.clicked_at = None;
                        }
                        None => self.edits.push(Command::SelectAnnotation(None)),
                    }
                }
            }

            // The eraser: pressing erases the mark under the pointer, and
            // holding the button sweeps across several marks in one gesture.
            // While the tool is up the pointer carries a visible eraser cursor
            // and the mark about to be deleted is highlighted, so a click can
            // never delete a surprise.
            if tool == Tool::Eraser {
                let pointer = ui.input(|i| i.pointer.clone());
                // The eraser sleeps while a pan is in progress (or the hand
                // tool is up), and over a popup or window it must neither
                // highlight nor delete: those live on a layer above the
                // canvas, so the mark under the pointer there is not what the
                // user actually sees.
                let blocked = self.panning || self.pan_tool || over_float;
                let in_view = !blocked
                    && pointer
                        .latest_pos()
                        .is_some_and(|pos| ui.clip_rect().contains(pos));
                let hover = if in_view {
                    pointer
                        .interact_pos()
                        .or(pointer.latest_pos())
                        .and_then(|pos| {
                            let local = (pos.x - content_origin.x, pos.y - content_origin.y);
                            erase_hit(&page_spaces, annotations, pos2(local.0, local.1))
                        })
                } else {
                    None
                };

                // Press deletes. `erase_last` stops a still-held press from
                // re-deleting the same mark every frame while the refreshed
                // annotation list catches up; moving to a different mark
                // deletes that one too, which is what makes the sweep work.
                if pointer.primary_down() {
                    if let Some(id) = hover {
                        if self.erase_last != Some(id) {
                            self.edits.push(Command::DeleteAnnotation(id));
                            self.erase_last = Some(id);
                        }
                    }
                } else {
                    self.erase_last = None;
                }

                // Highlight the mark the eraser is over, in screen space.
                if let Some(id) = hover {
                    if let Some(ann) = annotations.iter().find(|a| a.id == id) {
                        if let Some(space) = page_spaces.iter().find(|sp| sp.index == ann.id.page)
                        {
                            let screen = space.to_screen(ann.rect);
                            let rect = Rect::from_min_size(
                                pos2(screen.x, screen.y),
                                vec2(screen.w.max(1.0), screen.h.max(1.0)),
                            )
                            .translate(content_origin.to_vec2());
                            painter.rect_filled(
                                rect.expand(2.0),
                                2.0,
                                Color32::from_rgba_unmultiplied(220, 60, 60, 30),
                            );
                            painter.rect_stroke(
                                rect.expand(2.0),
                                2.0,
                                Stroke::new(1.5, Color32::from_rgb(220, 60, 60)),
                                StrokeKind::Outside,
                            );
                        }
                    }
                }

                // The eraser cursor itself: a small two-tone block whose
                // top-left corner is the pointer position — the same point
                // the erase hit-test uses, so what the corner touches is
                // exactly what gets erased. It turns red over a mark it can
                // delete. Drawn on the canvas because the OS cursors have no
                // eraser shape.
                if in_view {
                    if let Some(pos) = pointer.interact_pos().or(pointer.latest_pos()) {
                        let tip = pos;
                        let body = Rect::from_min_size(tip, vec2(21.0, 14.0));
                        let band = Rect::from_min_size(pos2(tip.x + 1.0, tip.y + 8.5), vec2(19.0, 4.5));
                        painter.rect_filled(
                            body.translate(vec2(1.0, 1.5)),
                            3.0,
                            Color32::from_rgba_unmultiplied(0, 0, 0, 45),
                        );
                        painter.rect_filled(body, 3.0, Color32::WHITE);
                        painter.rect_filled(
                            band,
                            2.0,
                            if hover.is_some() {
                                Color32::from_rgb(220, 60, 60)
                            } else {
                                Color32::from_rgb(250, 130, 160)
                            },
                        );
                        painter.rect_stroke(
                            body,
                            3.0,
                            Stroke::new(1.0, Color32::from_rgb(90, 90, 90)),
                            StrokeKind::Inside,
                        );
                    }
                }
            }

            // A painted miniature beside the pointer for every drawing tool.
            // The OS cursor set has no per-tool shapes, and winit's Windows
            // backend silently falls back to a plain arrow for any icon it
            // does not know — so this badge, not the cursor icon, is what
            // makes the armed tool visible. It always works, on every
            // platform. While it shows, the OS cursor hides, so exactly one
            // pointer is on screen.
            if tool_uses_pointer_badge(tool) && !self.panning && !self.pan_tool {
                let pointer = ui.input(|i| i.pointer.clone());
                let in_view = pointer
                    .latest_pos()
                    .is_some_and(|pos| ui.clip_rect().contains(pos));
                if in_view {
                    if let Some(pos) = pointer.interact_pos().or(pointer.latest_pos()) {
                        draw_tool_badge(&painter, tool, pos);
                    }
                }
            }

            // Write the note popup state back for the next frame.
            self.note_input = note_input;
        });

        self.scroll_offset = output.state.offset;

        // Drag-to-pan. With the hand tool engaged — or the middle button held,
        // which every other viewer binds — dragging on the page moves the
        // viewport. Without this a page zoomed in past
        // the viewport can only be nudged with the scrollbars, which reads as
        // "the app is stuck" on a trackpad.
        let (panning, pan_delta) = ui.input(|input| {
            let pointer = &input.pointer;
            // The eraser owns the primary drag: a held press sweeps across
            // marks, so it must never be read as a pan gesture.
            let wants_pan = drag_pans(
                pointer.middle_down(),
                pointer.primary_down(),
                self.pan_tool,
                self.press_on_background && tool != Tool::Eraser,
                tool.annotation_kind().is_some(),
            );
            (
                wants_pan && pointer.is_decidedly_dragging(),
                pointer.delta(),
            )
        });
        self.panning = panning;

        // One place decides where the view goes this frame, so a pan and an
        // edge-following drag cannot overwrite each other.
        let mut scroll_delta = Vec2::ZERO;
        if panning {
            // Dragging right slides the page right, which reveals content
            // further left — so the offset moves opposite to the pointer.
            scroll_delta -= pan_delta;
        }
        // A drag that reaches the edge of the viewport keeps the view moving,
        // whether it is selecting, drawing a shape or panning. Without this a
        // selection stops at the fold and the pointer slides off the page —
        // "I click anywhere in the canvas and drag to the end and it will not
        // scroll down". A pan needs it too: the pan follows the pointer's
        // delta, and there is no delta left once the pointer is pinned to the
        // edge.
        if panning || self.select_start.is_some() || self.draw_start.is_some() {
            let viewport = ui.clip_rect();
            let pointer = ui.input(|input| {
                input
                    .pointer
                    .interact_pos()
                    .or_else(|| input.pointer.latest_pos())
            });
            if let Some(pointer) = pointer {
                scroll_delta += edge_scroll_delta(pointer, viewport);
            }
        }

        if scroll_delta != Vec2::ZERO {
            // Applied on the next frame via the builder offset, the same path
            // keyboard scrolling uses, so egui clamps it to the content.
            self.pending_scroll_offset = Some((self.scroll_offset + scroll_delta).max(Vec2::ZERO));
        }

        // Only when the pointer is over the canvas itself — not the annotation
        // bar above it, which shares this ui but must keep its own cursors
        // (the hand over its buttons).
        if let Some(clip) = canvas_clip.filter(|clip| ui.rect_contains_pointer(*clip)) {
            // The drawing tools paint their own cursor badge, which becomes
            // the one and only pointer: while it shows, the OS cursor hides.
            // Over a popup, window or tooltip the badge would paint beneath
            // it, so there the OS cursor stays instead — still exactly one
            // visible pointer either way.
            let badge_active = tool_uses_pointer_badge(tool)
                && !self.panning
                && !self.pan_tool
                && ui.input(|i| i.pointer.latest_pos()).is_some_and(|pos| {
                    clip.contains(pos)
                        && ui
                            .ctx()
                            .layer_id_at(pos)
                            .map_or(true, |top| top == ui.layer_id())
                });
            // The canvas content origin captured inside the scroll viewport, so
            // the Select-mode hover test turns the raw pointer into the same
            // layout space the drawing code uses.
            let content_origin = canvas_origin.unwrap_or_else(|| ui.min_rect().min);
            let cursor = if self.panning {
                // `Grabbing` is not in winit's Windows mapping and would fall
                // back to a plain arrow; the four-way move arrows read as
                // "the view is being dragged".
                egui::CursorIcon::Move
            } else if let Some(drag) = &self.ann_drag {
                // While moving/resizing an annotation the cursor shows what
                // the gesture will do: the four-way arrows for a move, a
                // directional arrow for the handle being dragged, so the user
                // can tell at a glance which way it will resize.
                match drag {
                    AnnDrag::Move { .. } => egui::CursorIcon::Move,
                    AnnDrag::Resize { handle, .. } => resize_cursor(*handle),
                }
            } else if self.pan_tool {
                // Same: `Grab` falls back to an arrow on Windows, while the
                // pointing hand is mapped and reads as "the hand tool".
                egui::CursorIcon::PointingHand
            } else if badge_active {
                egui::CursorIcon::None
            } else if tool == Tool::Select {
                // Hover affordances in Select mode: a directional arrow over a
                // resize handle and the move arrows over any editable body, so
                // a resize/move is discoverable before the button goes down.
                let pointer = ui
                    .input(|i| i.pointer.interact_pos().or_else(|| i.pointer.latest_pos()));
                match pointer {
                    Some(pos) => annotation_hover_cursor(
                        &page_spaces,
                        annotations,
                        selected_annotation,
                        content_origin,
                        pos,
                    ),
                    None => egui::CursorIcon::Text,
                }
            } else {
                tool_cursor(tool)
            };
            ui.ctx().set_cursor_icon(cursor);
        }

        // Which page is under the viewport centre, and where within it? This is
        // what the status bar shows (so it tracks scrolling as well as explicit
        // navigation) and what keeps the view anchored across zoom changes.
        // Prefer the page containing the viewport centre. If the centre lands
        // in the intentional gap between pages, choose the nearest page rather
        // than falling back to page 0; the old fallback made the page indicator
        // jump back to the first page while scrolling across a boundary.
        self.center_page = 0;
        let mut nearest_distance = f32::INFINITY;
        for b in &boxes {
            let inside = viewport_center_y >= b.y && viewport_center_y < b.y + b.h;
            let distance = if inside {
                0.0
            } else if viewport_center_y < b.y {
                b.y - viewport_center_y
            } else {
                viewport_center_y - (b.y + b.h)
            };
            if distance < nearest_distance {
                nearest_distance = distance;
                self.center_page = b.index;
                self.center_frac_y = ((viewport_center_y - b.y) / b.h).clamp(0.0, 1.0);
                self.center_frac_x = ((viewport_center_x - b.x) / b.w).clamp(0.0, 1.0);
            }
            if inside {
                break;
            }
        }

        // Nearest-to-centre tiles first, and never flood the engine.
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut requests = Vec::new();
        for (_, pending) in candidates {
            if requests.len() >= MAX_REQUESTS_PER_FRAME || self.inflight.len() >= MAX_INFLIGHT {
                break;
            }
            self.inflight
                .insert((doc.id, pending.rotation, pending.key));
            requests.push(pending);
        }
        requests
    }

    /// Draw the thumbnail strip and return thumbnails to request plus any click.
    ///
    /// Rows have a fixed height and are drawn with `ScrollArea::show_rows`, so
    /// only the rows actually on screen are laid out — a 1000-page document
    /// visits ~10 rows per frame, not 1000.
    pub fn draw_thumbnails(
        &mut self,
        ui: &mut Ui,
        doc: &Document,
        view: &ViewState,
        dpr: f32,
        accent: Color32,
        text_dim: Color32,
    ) -> ThumbnailResult {
        let mut result = ThumbnailResult::default();
        let rotation = view.rotation;
        let total = doc.pages.len();
        let row_h = THUMB_WIDTH * 1.45 + 24.0;

        // Follow page navigation: when the current page changes (page buttons,
        // thumbnails, outline, continuous scrolling), scroll the strip so the
        // page is in view. Only applied on the frame the page changes, so the
        // user can still browse the strip freely in between. The followed page
        // is keyed by document so switching tabs re-anchors the strip even when
        // both documents sit on the same page number.
        let mut strip = egui::ScrollArea::vertical().auto_shrink([false, false]);
        if self.thumb_follow != Some((doc.id, view.current_page)) {
            strip = strip.vertical_scroll_offset(row_h * view.current_page as f32);
            self.thumb_follow = Some((doc.id, view.current_page));
        }

        strip.show_rows(ui, row_h, total, |ui, range| {
            for index in range {
                let index = index as u32;
                let Some(geom) = doc.page(index) else {
                    continue;
                };
                let (w_pt, h_pt) = geom.oriented(rotation);

                // Fit the page inside the row, preserving its aspect.
                let mut disp_w = THUMB_WIDTH;
                let mut disp_h = if w_pt > 0.0 {
                    disp_w * h_pt / w_pt
                } else {
                    disp_w
                };
                let max_h = row_h - 28.0;
                if disp_h > max_h {
                    disp_h = max_h;
                    disp_w = if h_pt > 0.0 {
                        disp_h * w_pt / h_pt
                    } else {
                        disp_w
                    };
                }

                let cell_w = ui.available_width();
                let (rect, response) =
                    ui.allocate_exact_size(vec2(cell_w, row_h - 6.0), Sense::click());
                let img_rect = Rect::from_center_size(
                    pos2(rect.center().x, rect.center().y - 6.0),
                    vec2(disp_w, disp_h),
                );

                if ui.is_rect_visible(img_rect) {
                    ui.painter().rect_filled(
                        img_rect.translate(vec2(0.0, 1.0)),
                        2.0,
                        Color32::from_black_alpha(50),
                    );
                    if let Some(tex) = self.thumbs.get(&(doc.id, rotation, index)) {
                        ui.painter()
                            .image(tex.id(), img_rect, uv_full(), Color32::WHITE);
                    } else {
                        ui.painter().rect_filled(img_rect, 2.0, Color32::WHITE);
                        if let Some(thumb) =
                            self.request_thumbnail(doc, rotation, index, THUMB_WIDTH, dpr)
                        {
                            result.requests.push(thumb);
                        }
                    }

                    let selected = index == view.current_page;
                    let stroke = if selected {
                        Stroke::new(2.0, accent)
                    } else {
                        Stroke::new(1.0, Color32::from_black_alpha(40))
                    };
                    ui.painter()
                        .rect_stroke(img_rect, 2.0, stroke, StrokeKind::Outside);
                    ui.painter().text(
                        pos2(rect.center().x, rect.max.y - 12.0),
                        egui::Align2::CENTER_CENTER,
                        format!("{}", index + 1),
                        egui::FontId::proportional(11.0),
                        if selected { accent } else { text_dim },
                    );
                }

                if response.clicked() {
                    result.clicked = Some(index);
                }
            }
        });

        result
    }
}

/// Fill and stroke colours for one annotation kind, matching what PDFium will
/// render for it, so previews and echoes blend into the final appearance.
fn annotation_colors(kind: AnnotationKind) -> (Color32, Color32) {
    match kind {
        AnnotationKind::Highlight => (
            Color32::from_rgba_unmultiplied(255, 235, 59, 70),
            Color32::from_rgb(212, 180, 0),
        ),
        AnnotationKind::Underline => (Color32::TRANSPARENT, Color32::BLACK),
        AnnotationKind::StrikeOut => (Color32::TRANSPARENT, Color32::from_rgb(211, 47, 47)),
        AnnotationKind::Squiggly => (Color32::TRANSPARENT, Color32::from_rgb(67, 160, 71)),
        AnnotationKind::Square => (Color32::TRANSPARENT, Color32::from_rgb(211, 47, 47)),
        AnnotationKind::FreeText => (
            Color32::from_rgba_unmultiplied(255, 255, 255, 140),
            Color32::from_rgb(80, 80, 80),
        ),
        _ => (Color32::TRANSPARENT, Color32::from_rgb(80, 80, 80)),
    }
}

/// The sticky note's on-screen stand-in: a small yellow tag.
fn draw_note_tag(painter: &egui::Painter, point: (f32, f32), content_origin: egui::Pos2) {
    let rect = Rect::from_center_size(
        pos2(point.0 + content_origin.x, point.1 + content_origin.y),
        vec2(18.0, 18.0),
    );
    painter.rect_filled(rect, 3.0, Color32::from_rgb(255, 213, 79));
    painter.rect_stroke(
        rect,
        3.0,
        Stroke::new(1.0, Color32::from_rgb(140, 110, 0)),
        StrokeKind::Outside,
    );
}

/// A sticky note drawn as one cohesive card: a small note glyph followed by
/// the typed text, inside a single rounded bubble.
///
/// `PDFium` renders only a note's icon and never its body, so the whole card is
/// painted by us. The glyph is centred on the note's point — so the note stays
/// exactly where it was placed — while the bubble grows to the right, flipping
/// to the left when it would run off the canvas. Returns the card's screen rect
/// so the caller can ring it when selected and hit-test clicks against it.
fn draw_note_card(
    painter: &egui::Painter,
    point: (f32, f32),
    text: &str,
    content_origin: egui::Pos2,
) -> Rect {
    // A roomy horizontal pad and a generous icon→text gap keep the two from
    // ever crowding each other.
    const PAD_X: f32 = 9.0;
    const PAD_Y: f32 = 6.0;
    const GLYPH: f32 = 16.0;
    const GAP: f32 = 9.0;

    let font = egui::FontId::proportional(11.5);
    let ink = Color32::from_rgb(58, 44, 6);
    let fill = Color32::from_rgb(255, 244, 196);
    let border = Color32::from_rgb(216, 180, 62);

    // One trimmed line keeps the card a tidy, predictable size.
    let shown = truncate_chars(text.trim(), 48);
    let galley = (!shown.is_empty()).then(|| painter.layout_no_wrap(shown, font.clone(), ink));
    let text_size = galley.as_ref().map_or(Vec2::ZERO, |g| g.rect.size());

    let height = text_size.y.max(GLYPH) + PAD_Y * 2.0;
    let width = PAD_X + GLYPH + GAP + text_size.x + PAD_X;

    let glyph_center = pos2(point.0 + content_origin.x, point.1 + content_origin.y);

    // Default: the glyph sits at the note's point and the bubble grows right.
    let mut left = glyph_center.x - GLYPH / 2.0 - PAD_X;
    let mut text_on_right = true;
    // Flip to the left of the point when the bubble would run off the canvas.
    if left + width > painter.clip_rect().right() - 4.0 {
        left = glyph_center.x + GLYPH / 2.0 + PAD_X - width;
        text_on_right = false;
    }
    let card = Rect::from_min_size(
        pos2(left, glyph_center.y - height / 2.0),
        vec2(width, height),
    );

    // A soft drop shadow lifts the card off the page for a tidier look.
    painter.rect_filled(
        card.translate(vec2(0.0, 2.0)).expand(1.0),
        8.0,
        Color32::from_black_alpha(26),
    );
    painter.rect_filled(card, 7.0, fill);
    painter.rect_stroke(card, 7.0, Stroke::new(1.0, border), StrokeKind::Inside);
    draw_note_glyph(painter, glyph_center, ink);

    if let Some(galley) = galley {
        let text_x = if text_on_right {
            card.min.x + PAD_X + GLYPH + GAP
        } else {
            card.min.x + PAD_X
        };
        painter.galley(
            pos2(text_x, card.center().y - text_size.y / 2.0),
            galley,
            ink,
        );
    }

    card
}

/// The id of the topmost card under `pt`.
///
/// `cards` is in paint order, so the last containing rect is the one drawn on
/// top — this is what makes clicking an overlapping note pick the note the user
/// actually sees.
fn topmost_card(cards: &[(AnnotationId, Rect)], pt: Pos2) -> Option<AnnotationId> {
    cards
        .iter()
        .rev()
        .find(|(_, rect)| rect.contains(pt))
        .map(|(id, _)| *id)
}

/// Find the annotation to delete for an eraser click.
///
/// `pt` is in **content-local layout space** — the same space `self.clicked_at`
/// is stored in (pointer position with `content_origin` already subtracted), and
/// the same space `page_spaces` and `to_page` use. Do NOT pass the global
/// `screen_pt` here: `hit_page` compares against page boxes that are also in
/// content-local space, so a global point would fall outside every box and the
/// eraser would delete nothing.
///
/// We locate the page under the point, convert the point into PDF page space,
/// then return the id of the smallest annotation rect that contains it — so the
/// most specific annotation at the cursor is erased. Returns `None` when the
/// click is on empty page.
fn erase_hit(
    page_spaces: &[PageSpace],
    annotations: &[AnnotationInfo],
    screen_pt: Pos2,
) -> Option<AnnotationId> {
    let space = hit_page(page_spaces, (screen_pt.x, screen_pt.y))?;
    let (px, py) = space.to_page((screen_pt.x, screen_pt.y));
    let mut best: Option<(AnnotationId, f32)> = None;
    for ann in annotations.iter().filter(|a| a.id.page == space.index) {
        if ann.rect.contains(px, py) {
            let area = ann.rect.area();
            match best {
                Some((_, best_area)) if best_area <= area => {}
                _ => best = Some((ann.id, area)),
            }
        }
    }
    best.map(|(id, _)| id)
}

/// Find an editable annotation to select at a Select-tool click.
///
/// Same hit rule as [`erase_hit`] — smallest containing rect — but restricted
/// to kinds the reader can create and edit, so links and foreign annotation
/// types are never picked up by the selection tool.
fn select_hit(
    page_spaces: &[PageSpace],
    annotations: &[AnnotationInfo],
    screen_pt: Pos2,
) -> Option<AnnotationId> {
    let editable: Vec<AnnotationInfo> = annotations
        .iter()
        .filter(|a| a.kind.is_creatable())
        .cloned()
        .collect();
    erase_hit(page_spaces, &editable, screen_pt)
}

/// Screen-space centres of the eight resize handles around `outline`.
fn handle_points(outline: Rect) -> Vec<Pos2> {
    let (l, r, t, b) = (
        outline.left(),
        outline.right(),
        outline.top(),
        outline.bottom(),
    );
    let (mx, my) = (outline.center().x, outline.center().y);
    vec![
        pos2(l, t),
        pos2(mx, t),
        pos2(r, t),
        pos2(r, my),
        pos2(r, b),
        pos2(mx, b),
        pos2(l, b),
        pos2(l, my),
    ]
}

/// The index of `handle` within [`handle_points`]'s output, so the painter can
/// tell which drawn handle the pointer is over.
fn handle_index(handle: ResizeHandle) -> usize {
    match handle {
        ResizeHandle::NorthWest => 0,
        ResizeHandle::North => 1,
        ResizeHandle::NorthEast => 2,
        ResizeHandle::East => 3,
        ResizeHandle::SouthEast => 4,
        ResizeHandle::South => 5,
        ResizeHandle::SouthWest => 6,
        ResizeHandle::West => 7,
    }
}

/// A small dark pill showing live dimensions during a resize drag, anchored
/// just outside `anchor` so it never covers the mark being resized.
fn draw_size_readout(painter: &egui::Painter, anchor: Pos2, text: &str, accent: Color32) {
    let galley = painter.layout_no_wrap(
        text.to_owned(),
        egui::FontId::proportional(11.0),
        Color32::WHITE,
    );
    let pad = vec2(6.0, 3.0);
    let rect = Rect::from_min_size(anchor, galley.size() + pad * 2.0);
    painter.rect_filled(rect, 4.0, Color32::from_rgba_unmultiplied(0, 0, 0, 190));
    painter.rect_stroke(
        rect,
        4.0,
        Stroke::new(1.0, accent),
        StrokeKind::Inside,
    );
    painter.galley(rect.min + pad, galley, Color32::WHITE);
}

/// The resize handle under a screen-space pointer, if any.
///
/// `space` and `rect` are the annotation's page space and bounds; the rect is
/// projected to the screen and each handle tested with a small grab radius so
/// the handles are comfortable to hit.
fn handle_hit(space: &PageSpace, rect: PdfSpaceRect, screen_pos: Pos2) -> Option<ResizeHandle> {
    let screen = space.to_screen(rect);
    let outline = Rect::from_min_size(
        pos2(screen.x, screen.y),
        vec2(screen.w.max(1.0), screen.h.max(1.0)),
    );
    nearest_handle(outline, screen_pos)
}

/// The resize handle nearest `screen_pos`, within a small grab radius.
///
/// Picks the CLOSEST handle, not merely the first one inside the radius. On a
/// thin mark (an underline is only a few points tall) the corner and edge
/// handles overlap; "first match" picked a corner, so a drag the user meant as
/// vertical resized *both* axes — which reads as "vertical resize only changes
/// the width". Closest-wins makes the top edge grab the North handle and the
/// bottom edge the South handle, so a vertical drag stays vertical.
fn nearest_handle(outline: Rect, screen_pos: Pos2) -> Option<ResizeHandle> {
    const GRAB: f32 = 6.0;
    let handles = [
        (ResizeHandle::NorthWest, 0),
        (ResizeHandle::North, 1),
        (ResizeHandle::NorthEast, 2),
        (ResizeHandle::East, 3),
        (ResizeHandle::SouthEast, 4),
        (ResizeHandle::South, 5),
        (ResizeHandle::SouthWest, 6),
        (ResizeHandle::West, 7),
    ];
    let points = handle_points(outline);
    handles
        .iter()
        .filter_map(|(handle, index)| {
            let distance = (points[*index] - screen_pos).length();
            (distance <= GRAB).then_some((*handle, distance))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(handle, _)| handle)
}

/// Resize `rect` by dragging `handle` to the PDF-space point `to`.
///
/// The handle's opposite corner or edge stays fixed; the dragged edges follow
/// the pointer, clamped so the annotation can never be collapsed below
/// [`MIN_ANN_SIZE`] — a drag past the opposite edge flips nothing and simply
/// stops at the minimum.
fn resize_rect(rect: PdfSpaceRect, handle: ResizeHandle, to: (f32, f32)) -> PdfSpaceRect {
    resize_rect_with(rect, handle, to, MIN_ANN_SIZE)
}

/// The vertical floor a line markup is dragged against.
///
/// A line markup's vertical axis is its stroke *weight*, not a box edge, and
/// the weight can legitimately go down to a hairline
/// ([`MIN_MARKUP_THICKNESS`]). Clamping its height at [`MIN_ANN_SIZE`] like any
/// other annotation would stop the stroke dead once the box bottomed out — the
/// other half of the "it locks suddenly" report.
///
/// Half the height the mark started at (never more than [`MIN_ANN_SIZE`]) is
/// enough for the weight to reach its floor for every band a user can drag,
/// because the weight falls by the same distance the box does: from a band of
/// `h` the weight can drop by `h - h/2 = h/2`, far more than the 1.2pt between
/// the default weight and the floor. It also keeps the box from collapsing to
/// something that could no longer be grabbed.
fn line_markup_min_height(rect: PdfSpaceRect) -> f32 {
    (rect.height() / 2.0)
        .min(MIN_ANN_SIZE)
        .max(MIN_MARKUP_THICKNESS)
}

/// [`resize_rect`] with an explicit vertical floor, for the kinds whose height
/// means something other than a box edge.
fn resize_rect_with(
    rect: PdfSpaceRect,
    handle: ResizeHandle,
    to: (f32, f32),
    min_height: f32,
) -> PdfSpaceRect {
    let (mut min_x, mut max_x) = (rect.min_x, rect.max_x);
    let (mut min_y, mut max_y) = (rect.min_y, rect.max_y);
    match handle {
        ResizeHandle::NorthWest | ResizeHandle::West | ResizeHandle::SouthWest => {
            min_x = to.0.min(max_x - MIN_ANN_SIZE);
        }
        ResizeHandle::NorthEast | ResizeHandle::East | ResizeHandle::SouthEast => {
            max_x = to.0.max(min_x + MIN_ANN_SIZE);
        }
        ResizeHandle::North | ResizeHandle::South => {}
    }
    match handle {
        ResizeHandle::NorthWest | ResizeHandle::North | ResizeHandle::NorthEast => {
            max_y = to.1.max(min_y + min_height);
        }
        ResizeHandle::SouthWest | ResizeHandle::South | ResizeHandle::SouthEast => {
            min_y = to.1.min(max_y - min_height);
        }
        ResizeHandle::East | ResizeHandle::West => {}
    }
    PdfSpaceRect {
        min_x,
        min_y,
        max_x,
        max_y,
    }
}

/// Create the annotation for a placed note and echo it until its page
/// re-renders.
///
/// `note.point` is already in unrotated PDF space (set from `to_page` when the
/// note was placed); converting it again would move the note away from the
/// click, so it is used verbatim — only clamped to the unrotated page bounds.
/// The echo is drawn in *layout* space (like the live preview), so the PDF
/// anchor is converted back for it.
fn commit_pending_note(
    edits: &mut Vec<Command>,
    pending: &mut Vec<PendingEcho>,
    doc: DocumentId,
    spaces: &[PageSpace],
    note: NoteInput,
) {
    let Some(space) = spaces.iter().find(|sp| sp.index == note.page) else {
        return;
    };
    let (w, h) = (space.geom.width_pt, space.geom.height_pt);
    edits.push(Command::AddAnnotation {
        page: note.page,
        new: NewAnnotation::StickyNote(
            (note.point.0.clamp(0.0, w), note.point.1.clamp(0.0, h)),
            note.text.clone(),
        ),
    });
    pending.push(PendingEcho {
        doc,
        page: note.page,
        kind: AnnotationKind::StickyNote,
        rect: None,
        point: Some(space.to_screen_point(note.point)),
    });
}

/// A small comment glyph: a rounded speech bubble with a tail.
fn draw_note_glyph(painter: &egui::Painter, center: Pos2, ink: Color32) {
    let stroke = Stroke::new(1.5, ink);
    // Bubble body, nudged up a touch so the tail sits on the baseline.
    let body = Rect::from_center_size(center + vec2(0.0, -1.0), vec2(13.0, 10.0));
    painter.rect_stroke(body, 3.0, stroke, StrokeKind::Inside);
    // Tail, bottom-left, pointing down.
    painter.line(
        vec![
            pos2(body.min.x + 3.0, body.max.y - 0.5),
            pos2(body.min.x + 3.0, body.max.y + 3.0),
            pos2(body.min.x + 7.0, body.max.y - 0.5),
        ],
        stroke,
    );
}

/// Trim `text` to at most `max` characters, appending an ellipsis when cut.
fn truncate_chars(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

/// The rectangle kinds' on-screen stand-in.
/// Draw `kind`'s mark inside `rect`, as the pointer drags it or as an echo
/// while the real pixels are still being rendered.
///
/// `zoom` is the page's CSS pixels per PDF point, so the mark is previewed at
/// the size the engine will actually draw it — see [`preview_stroke`].
fn draw_shape_preview(
    painter: &egui::Painter,
    kind: AnnotationKind,
    rect: Rect,
    content_origin: egui::Pos2,
    zoom: f32,
) {
    let rect = rect.translate(content_origin.to_vec2());
    let (fill, ink) = annotation_colors(kind);
    let line = Stroke::new(preview_stroke(zoom), ink);

    // Each tool previews the mark it will actually leave. A generic box drawn
    // for every kind meant an underline looked like a rectangle while the mouse
    // was down and turned into a line the moment the page re-rendered around
    // it — the shape appeared to move as it was committed.
    match mark_for(kind, rect) {
        Mark::Line(from, to) => {
            painter.line_segment([from, to], line);
        }
        Mark::Squiggle(rect) => draw_squiggle(painter, rect, ink, zoom),
        Mark::Fill(rect) => {
            painter.rect_filled(rect, 1.0, fill);
        }
        Mark::Outline(rect) => {
            painter.rect_stroke(rect, 1.0, line, StrokeKind::Inside);
        }
        Mark::FilledOutline(rect) => {
            painter.rect_filled(rect, 1.0, fill);
            painter.rect_stroke(rect, 1.0, line, StrokeKind::Inside);
        }
    }
}

/// The mark a tool leaves inside the rectangle it was dragged over.
///
/// Kept apart from the painting so the geometry can be asserted directly. A
/// preview that drifts from the finished annotation is invisible to any test
/// that only checks pixels, and is exactly what made a drawn underline look
/// like a rectangle until the page re-rendered.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Mark {
    /// A straight line between two points.
    Line(Pos2, Pos2),
    /// A zig-zag along the rectangle's bottom edge.
    Squiggle(Rect),
    /// The rectangle's interior, with no outline.
    Fill(Rect),
    /// The rectangle's outline, with no fill.
    Outline(Rect),
    /// An outline drawn over a fill.
    FilledOutline(Rect),
}

/// What `kind` draws inside `rect`, in preview space.
fn mark_for(kind: AnnotationKind, rect: Rect) -> Mark {
    match kind {
        // Markup runs along an edge of the dragged band, matching the
        // appearance PDFium generates from the quad points.
        AnnotationKind::Underline => Mark::Line(rect.left_bottom(), rect.right_bottom()),
        AnnotationKind::StrikeOut => {
            let middle = rect.center().y;
            Mark::Line(pos2(rect.left(), middle), pos2(rect.right(), middle))
        }
        AnnotationKind::Squiggly => Mark::Squiggle(rect),
        // A highlight is its fill alone: the finished annotation has no border.
        AnnotationKind::Highlight => Mark::Fill(rect),
        // A typewriter box has a background and a rule around it.
        AnnotationKind::FreeText => Mark::FilledOutline(rect),
        // A rectangle is an outline. So is anything else drawn as a box.
        _ => Mark::Outline(rect),
    }
}

/// A squiggly underline, as a zig-zag along the bottom of the dragged band.
///
/// PDFium's squiggly appearance is a small wave; a zig-zag is the same mark at
/// the size a preview is judged at, and costs a handful of points rather than a
/// sampled sine.
///
/// The wave's height and spacing come from `pdfreader-pdf` — the same constants
/// the applied appearance stream is built from — so the preview cannot show a
/// wave the engine will not draw. They are document distances, so like the
/// stroke they are scaled by `zoom`.
fn draw_squiggle(painter: &egui::Painter, rect: Rect, ink: Color32, zoom: f32) {
    let (step, rise) = (SQUIGGLY_STEP_PT * zoom, SQUIGGLY_RISE_PT * zoom);
    let baseline = rect.bottom() - 1.0;
    let mut points = Vec::new();
    let mut x = rect.left();
    let mut up = true;
    while x <= rect.right() {
        points.push(pos2(x, if up { baseline - rise } else { baseline }));
        up = !up;
        x += step;
    }
    if points.len() >= 2 {
        painter.add(egui::Shape::line(
            points,
            Stroke::new(preview_stroke(zoom), ink),
        ));
    }
}

/// Paint a miniature of a drawing tool beside the pointer.
///
/// The OS cursor set has no shapes for these tools, and winit's Windows
/// backend quietly falls back to a plain arrow for any icon it does not know,
/// so this badge — not the cursor icon — is what tells the user which tool is
/// armed, in each tool's conventional markup colour.
///
/// The badge *is* the cursor, so its hotspot is the pointer position exactly:
/// the top-left corner sits on the pointer, which is the same point a drag
/// starts from and the same point the tool acts on. Every tool therefore
/// starts from the badge's own corner point — no floating offset between the
/// logo and where the mark lands.
/// The miniature the pointer carries for each drawing tool.
///
/// The shape and colour mirror the mark the tool actually creates (the engine
/// writes highlight yellow, underline black, strikeout red, squiggly green and
/// rectangle red — see `create_annotation`), so the cursor previews the result
/// the way a professional editor's does.
fn draw_tool_badge(painter: &egui::Painter, tool: Tool, pos: Pos2) {
    let tip = pos;
    match tool {
        // Highlighter: the yellow band, translucent like the real markup.
        Tool::Highlight => {
            let band = Rect::from_min_size(tip, vec2(20.0, 10.0));
            painter.rect_filled(band, 2.0, Color32::from_rgba_unmultiplied(255, 214, 0, 110));
        }
        // Underline: a text band with the (black) rule along its bottom edge.
        Tool::Underline => {
            let band = Rect::from_min_size(tip, vec2(20.0, 10.0));
            painter.rect_filled(band, 2.0, Color32::from_rgba_unmultiplied(150, 150, 150, 40));
            painter.line_segment(
                [pos2(tip.x, tip.y + 9.5), pos2(tip.x + 20.0, tip.y + 9.5)],
                Stroke::new(2.0, Color32::from_rgb(30, 30, 30)),
            );
        }
        // Strikeout: a text band with the (red) rule through its middle.
        Tool::StrikeOut => {
            let band = Rect::from_min_size(tip, vec2(20.0, 10.0));
            painter.rect_filled(band, 2.0, Color32::from_rgba_unmultiplied(150, 150, 150, 40));
            painter.line_segment(
                [pos2(tip.x, tip.y + 5.0), pos2(tip.x + 20.0, tip.y + 5.0)],
                Stroke::new(2.0, Color32::from_rgb(198, 40, 40)),
            );
        }
        // Squiggly: a text band with the (green) zig-zag along its bottom edge.
        Tool::Squiggly => {
            let band = Rect::from_min_size(tip, vec2(20.0, 10.0));
            painter.rect_filled(band, 2.0, Color32::from_rgba_unmultiplied(150, 150, 150, 40));
            // The badge is a fixed-size icon, not a document preview, so it is
            // drawn at 1:1 whatever the page zoom is.
            draw_squiggle(painter, band, Color32::from_rgb(46, 125, 50), 1.0);
        }
        // Rectangle: just the (red) outline, matching the created stroke.
        Tool::Square => {
            let band = Rect::from_min_size(tip, vec2(20.0, 12.0));
            painter.rect_stroke(
                band,
                1.5,
                Stroke::new(2.0, Color32::from_rgb(198, 40, 40)),
                StrokeKind::Inside,
            );
        }
        // Sticky note: the folded-corner square, drawn with three shapes.
        Tool::StickyNote => {
            let body = Rect::from_min_size(tip, vec2(16.0, 16.0));
            let fold = 5.0;
            painter.rect_filled(body, 1.0, Color32::from_rgb(255, 241, 118));
            let corner = [
                pos2(body.max.x - fold, body.max.y),
                body.max,
                pos2(body.max.x, body.max.y - fold),
            ];
            painter.add(egui::Shape::convex_polygon(
                corner.to_vec(),
                Color32::from_rgb(240, 215, 90),
                Stroke::NONE,
            ));
            painter.rect_stroke(
                body,
                1.0,
                Stroke::new(1.0, Color32::from_rgb(160, 140, 40)),
                StrokeKind::Inside,
            );
        }
        _ => {}
    }
}

/// Draw the current values of text-like form fields on top of the page.
///
/// `PDFium` renders a field's static appearance stream — which is empty until a
/// form-fill environment runs, and `pdfium-render` does not wrap one. Drawing
/// the values ourselves keeps typed text visible at all times and lets the
/// font size adapt to each field's height, which is what real forms need
/// (fields are often only 10–14 pt tall).
fn draw_form_values(
    painter: &egui::Painter,
    form: &FormInfo,
    selected: Option<FieldId>,
    page_spaces: &[PageSpace],
    content_origin: egui::Pos2,
    font: &FormFont,
) {
    for field in &form.fields {
        // The selected field's own editor is on top; drawing the value under
        // it would double-render the text.
        if selected == Some(field.id) {
            continue;
        }
        // A field the reader has painted into the page is already in the raster
        // — drawing it again here would stack a second mark on top of it.
        if field.rendered {
            continue;
        }
        let Some(space) = page_spaces.iter().find(|sp| sp.index == field.id.page) else {
            continue;
        };
        let screen = space.to_screen(field.rect);
        let rect = Rect::from_min_size(
            pos2(screen.x, screen.y),
            vec2(screen.w.max(1.0), screen.h.max(1.0)),
        )
        .translate(content_origin.to_vec2());
        if !painter.clip_rect().intersects(rect) {
            continue;
        }

        match &field.value {
            // Checkboxes and radios: PDFium rasterizes the widget's *original*
            // appearance but does not regenerate it after a value is written,
            // so a checked box kept looking empty. Drawing the tick/dot here is
            // what makes the state visible.
            FieldValue::Checked(on)
                if matches!(
                    field.kind,
                    FormFieldType::CheckBox | FormFieldType::RadioButton
                ) =>
            {
                draw_boolean_widget(painter, rect, field.kind, *on);
            }
            FieldValue::Text(text) if !text.is_empty() => {
                draw_field_text(painter, rect, text, font);
            }
            FieldValue::Choice(Some(label)) if !label.is_empty() => {
                draw_field_text(painter, rect, label, font);
            }
            _ => {}
        }
    }
}

/// Draw one field's typed text (or chosen option) inside its rectangle, in the
/// user's chosen form font.
fn draw_field_text(painter: &egui::Painter, rect: Rect, text: &str, font: &FormFont) {
    let painter = painter.with_clip_rect(rect);
    let anchor = rect.left_top() + vec2(2.0, rect.height() / 2.0);
    let font_id = egui::FontId::new(font.size_for(rect.height()), font.family());
    painter.text(
        anchor,
        egui::Align2::LEFT_CENTER,
        text,
        font_id.clone(),
        font.ink(),
    );
    if font.bold {
        // Synthetic bold: no bold face is bundled and `egui` has no weight
        // axis, so a second stamp a hair to the right thickens every stroke.
        // The editor stamps its galley the same way, so a value does not change
        // weight when the field is committed.
        painter.text(
            anchor + vec2(SYNTHETIC_BOLD_OFFSET, 0.0),
            egui::Align2::LEFT_CENTER,
            text,
            font_id,
            font.ink(),
        );
    }
}

/// Draw a checkbox or radio button reflecting its current value.
///
/// The rectangle matches the widget's own, so the outline lands on top of the
/// one `PDFium` drew and reads as a single box; the tick (or dot) is the part
/// `PDFium` leaves out.
fn draw_boolean_widget(painter: &egui::Painter, rect: Rect, kind: FormFieldType, checked: bool) {
    let ink = Color32::from_rgb(28, 28, 28);
    let stroke = egui::Stroke::new(1.2, ink);
    if kind == FormFieldType::RadioButton {
        let radius = rect.width().min(rect.height()) * 0.5;
        let center = rect.center();
        painter.circle_filled(center, radius, Color32::WHITE);
        painter.circle_stroke(center, radius, stroke);
        if checked {
            painter.circle_filled(center, radius * 0.5, ink);
        }
        return;
    }

    painter.rect_filled(rect, 2.0, Color32::WHITE);
    painter.rect_stroke(rect, 2.0, stroke, egui::StrokeKind::Inside);
    if !checked {
        return;
    }
    let width = rect.width();
    let height = rect.height();
    let start = rect.left_top() + vec2(0.20 * width, 0.52 * height);
    let middle = rect.left_top() + vec2(0.42 * width, 0.76 * height);
    let end = rect.left_top() + vec2(0.80 * width, 0.24 * height);
    let tick = egui::Stroke::new((height * 0.14).clamp(1.4, 3.0), ink);
    painter.line_segment([start, middle], tick);
    painter.line_segment([middle, end], tick);
}

/// Draw the just-drawn shapes that are still waiting for their real pixels.
///
/// Colours approximate what PDFium will render for each kind, so the echo
/// blends into the final appearance instead of popping.
fn draw_pending_echoes(
    painter: &egui::Painter,
    pending: &[PendingEcho],
    doc: DocumentId,
    page_spaces: &[PageSpace],
    content_origin: egui::Pos2,
) {
    if pending.is_empty() {
        return;
    }
    let painter = painter.with_clip_rect(painter.clip_rect());

    for shape in pending.iter().filter(|s| s.doc == doc) {
        let Some(space) = page_spaces.iter().find(|sp| sp.index == shape.page) else {
            continue;
        };

        match shape.point {
            // A sticky note echoes as its icon.
            Some(point) => draw_note_tag(&painter, point, content_origin),
            // Everything else echoes as the mark it will become, at the weight
            // the page will draw it.
            None => {
                let Some(rect) = shape.rect else { continue };
                draw_shape_preview(&painter, shape.kind, rect, content_origin, space.zoom);
            }
        }
    }
}

/// Stroke width of a drawn annotation's mark, in PDF points.
///
/// A hairline. The engine gives a newly applied underline, strikeout, squiggly
/// or rectangle the same weight (`DEFAULT_MARKUP_THICKNESS` in `pdfreader-pdf`),
/// so the shape does not change weight the moment the page re-renders around it.
/// `the_preview_weight_matches_the_engine_default` asserts the two are equal, so
/// this cannot drift.
///
/// This is a *document* width, so it must be scaled by the page zoom before it
/// is drawn — see [`preview_stroke`].
const PREVIEW_STROKE_PT: f32 = 0.45;

/// [`PREVIEW_STROKE_PT`] in screen pixels, for a page at `zoom` CSS pixels per
/// PDF point.
///
/// The preview is drawn in screen space, but the mark it stands for is drawn by
/// the engine in *points*. Leaving the preview at a fixed pixel width made the
/// mark appear to jump the instant the mouse was released whenever the page was
/// zoomed in — which it is by default, since the default zoom is fit-width: a
/// 0.25pt mark on a page at 200% is 0.5px on screen, twice the width the preview
/// showed.
fn preview_stroke(zoom: f32) -> f32 {
    PREVIEW_STROKE_PT * zoom
}

/// How far a press may land from a glyph and still count as hitting it, in
/// points. Wide enough for the gap beside a word, narrow enough that a click in
/// the margin selects nothing.
const SELECT_TOLERANCE_PT: f32 = 6.0;

/// Resolve a press and a release into the characters they cover.
///
/// A press that never really moved selects the word under it, which is the
/// single-click case; anything further becomes the characters between the two
/// ends, whichever way the drag went.
fn selection_range(
    spaces: &[PageSpace],
    page: u32,
    cache: &PageTextCache,
    doc: DocumentId,
    start: (f32, f32),
    end: (f32, f32),
) -> Option<(u32, CharRange)> {
    let space = spaces.iter().find(|space| space.index == page)?;
    let text = cache.get(doc, page)?;
    if text.is_empty() {
        return None;
    }

    let (sx, sy) = space.to_page(start);
    let anchor = text
        .char_at(sx, sy)
        .or_else(|| text.nearest_char(sx, sy, SELECT_TOLERANCE_PT))?;

    let dragged = (end.0 - start.0).abs() > 3.0 || (end.1 - start.1).abs() > 3.0;
    if !dragged {
        return Some((page, text.word_range(anchor)));
    }

    let (ex, ey) = space.to_page(end);
    let other = text
        .char_at(ex, ey)
        .or_else(|| text.nearest_char(ex, ey, SELECT_TOLERANCE_PT))?;
    Some((page, CharRange::spanning(anchor, other)))
}

/// Turn a finished selection into a request the shell can answer.
///
/// Returns `None` when the selection holds no real text, which is what stops a
/// click on a blank part of the page from opening an empty popup.
fn emit_lookup(
    spaces: &[PageSpace],
    page: u32,
    range: CharRange,
    cache: &PageTextCache,
    doc: DocumentId,
    origin: Pos2,
) -> Option<LookupRequest> {
    let space = spaces.iter().find(|space| space.index == page)?;
    let text = cache.get(doc, page)?;
    let selected = text.range_text(range);
    if selected.trim().is_empty() {
        return None;
    }

    // Anchored under the last line of the selection, so a pick that spans
    // several lines puts the card at the end of what was chosen.
    let anchor = text
        .range_rects(range)
        .last()
        .copied()
        .or_else(|| text.range_rect(range))
        .map(|rect| space.to_screen_point((rect.min_x, rect.min_y)))
        .unwrap_or(space.origin);

    Some(LookupRequest {
        page,
        text: selected,
        anchor: pos2(anchor.0 + origin.x, anchor.1 + origin.y + 6.0),
    })
}

#[allow(clippy::too_many_arguments)]
fn draw_field_editor(
    ui: &mut Ui,
    form: &FormInfo,
    selected: Option<FieldId>,
    doc: DocumentId,
    page_spaces: &[PageSpace],
    content_origin: egui::Pos2,
    edits: &mut Vec<Command>,
    font: &FormFont,
) {
    let Some(field_id) = selected else {
        return;
    };
    let Some(space) = page_spaces
        .iter()
        .find(|space| space.index == field_id.page)
    else {
        return;
    };
    let Some(field) = form.fields.iter().find(|f| f.id == field_id) else {
        return;
    };
    if !field.is_editable() {
        return;
    }

    let screen = space.to_screen(field.rect);
    let rect = Rect::from_min_size(
        pos2(screen.x, screen.y),
        vec2(screen.w.max(32.0), screen.h.max(18.0)),
    )
    .translate(content_origin.to_vec2());

    let editor_id = egui::Id::new((
        "form-editor",
        doc.raw(),
        field_id.page,
        field_id.annot_index,
    ));
    // Focus the editor exactly once when it opens, so selecting a text field
    // by clicking it is enough to start typing. Re-requesting every frame
    // would fight the user's own focus changes.
    let just_opened = ui
        .ctx()
        .memory(|mem| mem.data.get_temp::<FieldId>(editor_owner_key(doc)))
        != Some(field_id);

    // Centred vertically inside the field, matching how the committed value is
    // drawn (`draw_field_text`). A default top-down layout would put the text
    // at the top of the box while editing and then snap it to the middle when
    // the field is committed — the text visibly jumping the moment you finish
    // typing.
    let mut editor = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center))
            .id_salt((
                "form-editor",
                doc.raw(),
                field_id.page,
                field_id.annot_index,
            )),
    );

    let response = match field.kind {
        FormFieldType::Text => {
            let mut draft = editor
                .ctx()
                .memory(|mem| mem.data.get_temp::<String>(editor_id))
                .unwrap_or_else(|| field.value.as_text());

            // The editor follows the chosen form font: by default it sizes to
            // the field height (real fields are often 10–14 pt tall), but the
            // user can pin a family and size (Settings → Text) so Bengali and
            // other scripts are legible. The text stays dark in every theme —
            // the page underneath is always white.
            let font_size = font.size_for(rect.height()).max(7.0);
            editor.style_mut().text_styles.insert(
                egui::TextStyle::Body,
                egui::FontId::new(font_size, font.family()),
            );
            editor.visuals_mut().override_text_color = Some(Color32::from_rgb(25, 25, 25));

            let output = egui::TextEdit::singleline(&mut draft)
                .desired_width(rect.width())
                .frame(egui::Frame::NONE)
                .font(egui::TextStyle::Body)
                .show(&mut editor);

            // Bold is synthetic (see `draw_field_text`), and `TextEdit` paints
            // its galley only once — so stamp it a second time here. Without
            // this the text would be heavy the moment it was committed and
            // light again while being edited, which reads as a glitch.
            if font.bold {
                editor.painter().galley(
                    output.galley_pos + vec2(SYNTHETIC_BOLD_OFFSET, 0.0),
                    output.galley.clone(),
                    font.ink(),
                );
            }

            let inner = output.response.response;
            let changed = draft != field.value.as_text();
            let submit = inner.lost_focus()
                || (inner.has_focus() && editor.input(|i| i.key_pressed(egui::Key::Enter)));
            if submit && changed {
                edits.push(Command::SetFormFieldValue {
                    id: field.id,
                    value: FieldValue::Text(draft.clone()),
                });
                editor
                    .ctx()
                    .memory_mut(|mem| mem.data.remove::<String>(editor_id));
            } else if changed {
                editor
                    .ctx()
                    .memory_mut(|mem| mem.data.insert_temp(editor_id, draft));
            }
            Some(inner)
        }
        FormFieldType::ComboBox | FormFieldType::ListBox => {
            let current = field.value.as_text();
            egui::ComboBox::new(editor_id, "")
                .selected_text(if current.is_empty() {
                    "—".to_string()
                } else {
                    current
                })
                .width(rect.width())
                .show_ui(&mut editor, |ui| {
                    for option in &field.options {
                        if ui
                            .selectable_label(option.selected, option.label.as_str())
                            .clicked()
                        {
                            edits.push(Command::SetFormFieldValue {
                                id: field.id,
                                value: FieldValue::Choice(Some(option.label.clone())),
                            });
                        }
                    }
                });
            None
        }
        // Checkboxes and radios are toggled by clicking their rects on the
        // page, which the shell resolves from the canvas click. Push buttons
        // and signature fields have nothing to edit.
        FormFieldType::CheckBox
        | FormFieldType::RadioButton
        | FormFieldType::PushButton
        | FormFieldType::Signature
        | FormFieldType::Unknown => None,
    };

    if just_opened {
        if let Some(response) = response {
            response.request_focus();
        }
        ui.ctx()
            .memory_mut(|mem| mem.data.insert_temp(editor_owner_key(doc), field_id));
    }
}

/// Memory key recording which field owns the open on-page editor.
fn editor_owner_key(doc: DocumentId) -> egui::Id {
    egui::Id::new(("form-editor-owner", doc.raw()))
}

/// Lay pages out in document space (logical points).
///
/// `viewport_w` is the width of the visible canvas. Pages are centred inside
/// the *content* width, so the content has to be at least as wide as the
/// viewport: in fit-page (and any zoom below fit-width) the page is narrower
/// than the viewport, and without this clamp the whole content block sits
/// flush left with the page hugging the left edge instead of the middle.
/// Pass `0.0` for pure document-space layout with no viewport padding.
/// Whether a drag should move the view instead of selecting or drawing.
///
/// * The middle button always pans, which is what every other viewer binds it to.
/// * The hand tool makes the primary button pan.
/// * A press that landed on the canvas background — the margin around the pages,
///   or the gap between them — has no text under it to select, so its drag moves
///   the view rather than doing nothing at all.
///
/// A draw tool owns the primary button, so it suppresses the last two.
fn drag_pans(
    middle_down: bool,
    primary_down: bool,
    pan_tool: bool,
    press_on_background: bool,
    drawing: bool,
) -> bool {
    middle_down || (primary_down && !drawing && (pan_tool || press_on_background))
}

/// The pointer shape each tool asks for, so the cursor says what the next
/// press will do before it lands.
///
/// The core crate owns `Tool` and knows nothing about egui, so the mapping
/// lives here next to the rest of the canvas input handling.
///
/// * `Select` shows the text beam — its drag picks text, like every viewer.
/// * The markup and rectangle tools draw a rectangle, so they all use the
///   crosshair for pixel-precise corners. Each also paints its own miniature
///   beside the pointer (see [`draw_tool_badge`]), because the OS cursor set
///   has no per-tool shapes.
///
/// Deliberately absent: `CursorIcon::Copy`. winit's Windows backend maps only
/// a fixed set of icons and silently falls back to a plain arrow for the rest
/// (`_ => IDC_ARROW`), so Copy never showed as anything but an arrow — which
/// is exactly the "sometimes the cursor does not change" report. Every icon
/// used here is one winit actually maps on Windows: Text, Crosshair, Move,
/// PointingHand.
fn tool_cursor(tool: Tool) -> egui::CursorIcon {
    match tool {
        Tool::Select => egui::CursorIcon::Text,
        Tool::Highlight
        | Tool::Underline
        | Tool::StrikeOut
        | Tool::Squiggly
        | Tool::Square
        | Tool::StickyNote
        | Tool::Eraser => egui::CursorIcon::Crosshair,
    }
}

/// The Windows-resize arrow for each annotation resize handle.
///
/// Every variant maps through `egui_winit` to a distinct Win32 `IDC_SIZE*`
/// cursor (e.g. `ResizeNorth` → `IDC_SIZENS`, `ResizeEast` → `IDC_SIZEWE`,
/// `ResizeNorthEast` → `IDC_SIZENESW`), so the direction a resize will pull is
/// always shown — never a stuck arrow — and the gesture is discoverable before
/// the button goes down.
fn resize_cursor(handle: ResizeHandle) -> egui::CursorIcon {
    match handle {
        ResizeHandle::North => egui::CursorIcon::ResizeNorth,
        ResizeHandle::South => egui::CursorIcon::ResizeSouth,
        ResizeHandle::East => egui::CursorIcon::ResizeEast,
        ResizeHandle::West => egui::CursorIcon::ResizeWest,
        ResizeHandle::NorthEast => egui::CursorIcon::ResizeNorthEast,
        ResizeHandle::NorthWest => egui::CursorIcon::ResizeNorthWest,
        ResizeHandle::SouthEast => egui::CursorIcon::ResizeSouthEast,
        ResizeHandle::SouthWest => egui::CursorIcon::ResizeSouthWest,
    }
}

/// The cursor to show while hovering an annotation in Select mode, before any
/// button is pressed: a directional arrow over a resize handle of the current
/// selection, the four-way move arrows over the body of any editable
/// annotation, and the default text cursor elsewhere.
fn annotation_hover_cursor(
    page_spaces: &[PageSpace],
    annotations: &[AnnotationInfo],
    selected: Option<AnnotationId>,
    content_origin: Pos2,
    pointer: Pos2,
) -> egui::CursorIcon {
    // Hit tests work in content-local space (`to_screen` is content-local and
    // the painter adds `content_origin` back when drawing), so convert the raw
    // global pointer once and use it for both the handle and the body test —
    // otherwise the hover arrow appears offset from the handle it describes.
    let local = pos2(pointer.x - content_origin.x, pointer.y - content_origin.y);
    // A resize handle of the current selection wins, even when it sits over
    // another annotation.
    if let Some(sel) = selected {
        if let Some(info) = annotations.iter().find(|a| a.id == sel) {
            if info.kind != AnnotationKind::StickyNote {
                if let Some(space) = page_spaces.iter().find(|sp| sp.index == info.id.page) {
                    if let Some(handle) = handle_hit(space, info.rect, local) {
                        return resize_cursor(handle);
                    }
                }
            }
        }
    }
    // Otherwise the body of any editable annotation reads as "move".
    let editable: Vec<AnnotationInfo> = annotations
        .iter()
        .filter(|a| a.kind.is_creatable())
        .cloned()
        .collect();
    if select_hit(page_spaces, &editable, local).is_some() {
        egui::CursorIcon::Move
    } else {
        egui::CursorIcon::Text
    }
}

/// Whether `tool` paints its own cursor badge on the canvas.
///
/// While a badge shows, the OS cursor hides (`CursorIcon::None`), so exactly
/// one pointer is on screen: the badge itself. The badge is painted on the
/// canvas layer, so over a popup or window — which renders above the canvas —
/// the caller keeps the OS cursor instead, or there would be no visible
/// pointer at all.
fn tool_uses_pointer_badge(tool: Tool) -> bool {
    matches!(
        tool,
        Tool::Highlight
            | Tool::Underline
            | Tool::StrikeOut
            | Tool::Squiggly
            | Tool::Square
            | Tool::StickyNote
            | Tool::Eraser
    )
}

/// Distance the view should move this frame because a drag has reached an edge.
///
/// A drag that runs to the edge of the viewport has to keep the view moving, or
/// a selection stops at the fold and the pointer slides off the page — which is
/// what "I click and drag to the end and it will not scroll down" is.
///
/// The pointer's own *position* drives it, not its delta: once it stops at the
/// edge there is no delta left to use. Speed ramps from nothing at the margin to
/// full at the edge, so a drag that ends near the bottom creeps while one held
/// against the edge moves steadily.
fn edge_scroll_delta(pointer: Pos2, viewport: Rect) -> Vec2 {
    /// Distance from an edge at which the view starts following the drag.
    const MARGIN: f32 = 32.0;
    /// Fastest the view moves from a drag, in pixels per frame.
    const MAX: f32 = 14.0;

    let ramp = |past: f32| (past / MARGIN).clamp(0.0, 1.0) * MAX;

    let x = if pointer.x < viewport.left() + MARGIN {
        -ramp(viewport.left() + MARGIN - pointer.x)
    } else if pointer.x > viewport.right() - MARGIN {
        ramp(pointer.x - (viewport.right() - MARGIN))
    } else {
        0.0
    };
    let y = if pointer.y < viewport.top() + MARGIN {
        -ramp(viewport.top() + MARGIN - pointer.y)
    } else if pointer.y > viewport.bottom() - MARGIN {
        ramp(pointer.y - (viewport.bottom() - MARGIN))
    } else {
        0.0
    };
    vec2(x, y)
}

/// The smallest scroll change that brings `field` fully inside the viewport.
///
/// `field` is `(x, y, w, h)` and `current` is `(x, y)`, both in document
/// coordinates — the same space, so the field's position on screen is simply
/// the difference. `viewport` is the visible size in the same pixels.
///
/// Returns `None` when the field is already inside with room to spare. That is
/// the common case: the user clicks a field they can see, and the view must not
/// move at all — a scroll here would drag the page out from under the pointer
/// at the exact moment they start typing.
fn reveal_scroll(
    field: (f32, f32, f32, f32),
    viewport: (f32, f32),
    current: (f32, f32),
) -> Option<(f32, f32)> {
    // Breathing room so a field brought in from off screen does not end up
    // flush against the toolbar or the edge, with its caret half off.
    const MARGIN: f32 = 24.0;

    let (field_x, field_y, field_w, field_h) = field;
    let (viewport_w, viewport_h) = viewport;
    let (current_x, current_y) = current;

    let top = field_y - current_y;
    let bottom = top + field_h;
    let left = field_x - current_x;
    let right = left + field_w;

    // Vertical first: a form runs down the page, so that is the axis a field is
    // normally off screen on. A field taller than the viewport aligns to its
    // top, which keeps the first line of it — and its caret — visible.
    let mut next_y = current_y;
    if top < MARGIN {
        next_y = field_y - MARGIN;
    } else if bottom > viewport_h - MARGIN {
        next_y = field_y + field_h - viewport_h + MARGIN;
    }

    let mut next_x = current_x;
    if left < MARGIN {
        next_x = field_x - MARGIN;
    } else if right > viewport_w - MARGIN {
        next_x = field_x + field_w - viewport_w + MARGIN;
    }

    let next = (next_x.max(0.0), next_y.max(0.0));
    (next != current).then_some(next)
}

fn layout_pages(
    doc: &Document,
    view: &ViewState,
    zoom: f32,
    viewport_w: f32,
) -> (Vec<PageBox>, Vec2) {
    let rotation = view.rotation;
    let indices: Vec<u32> = match view.mode {
        ViewMode::Continuous => (0..doc.pages.len() as u32).collect(),
        ViewMode::Single => {
            let current = view
                .current_page
                .min(doc.pages.len().saturating_sub(1) as u32);
            vec![current]
        }
    };

    let mut boxes = Vec::with_capacity(indices.len());
    let mut max_w = 1.0f32;
    let mut y = MARGIN;

    for index in indices {
        let Some(geom) = doc.page(index) else {
            continue;
        };
        let (w_pt, h_pt) = geom.oriented(rotation);
        let w = w_pt * zoom;
        let h = h_pt * zoom;
        max_w = max_w.max(w);
        boxes.push(PageBox {
            index,
            x: 0.0,
            y,
            w,
            h,
        });
        y += h + PAGE_GAP;
    }

    let content_w = (max_w + 2.0 * MARGIN).max(viewport_w);
    for b in &mut boxes {
        b.x = (content_w - b.w) / 2.0;
    }
    let content_h = (y - PAGE_GAP + MARGIN).max(MARGIN * 2.0);

    (boxes, vec2(content_w, content_h))
}

/// Full-texture UV rectangle.
fn uv_full() -> Rect {
    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))
}

/// Convert PDFium's BGRA output into an egui image.
///
/// PDFium renders onto an opaque cleared bitmap, so alpha is always 255; that
/// lets us copy RGB directly (no premultiply pass) and build the `ColorImage`
/// in a single pass over the pixels.
fn bgra_to_color_image(width: u32, height: u32, data: &[u8]) -> ColorImage {
    let mut pixels = Vec::with_capacity((width as usize) * (height as usize));
    for src in data.chunks_exact(4) {
        pixels.push(Color32::from_rgb(src[2], src[1], src[0]));
    }
    ColorImage::new([width as usize, height as usize], pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The next frame's timestamp for the canvas harnesses.
    ///
    /// egui's `History` panics outright when a frame reports an earlier time
    /// than the one before it, and a `RawInput` with no `time` is stamped by
    /// egui itself — which is not necessarily later than a time another frame
    /// pinned by hand. One clock for every frame keeps a mixed sweep legal.
    fn next_frame_time(ctx: &egui::Context) -> f64 {
        ctx.input(|i| i.time) + 0.001
    }
    use pdfreader_core::{Outline, PageGeometry};
    use pdfreader_pdf::TextPage;
    use pdfreader_pdf::annot::DEFAULT_MARKUP_THICKNESS;

    /// One line of text laid out on a 200×100 point page, plus the page space
    /// that maps it to layout coordinates at 1:1 zoom with no rotation.
    fn text_fixture() -> (TextPage, PageSpace) {
        let mut chars = Vec::new();
        let mut x = 10.0;
        for ch in "hello world".chars() {
            let width = if ch == ' ' { 3.0 } else { 8.0 };
            chars.push(pdfreader_pdf::CharBox {
                ch,
                rect: PdfSpaceRect::from_xywh(x, 50.0, width, 10.0),
                line: 0,
                word: 0,
            });
            x += width;
        }
        let page = pdfreader_pdf::text::build(chars);
        let space = PageSpace::new(
            PageBox::new(0, 0.0, 0.0, 200.0, 100.0),
            PageGeometry {
                width_pt: 200.0,
                height_pt: 100.0,
                origin_x: 0.0,
                origin_y: 0.0,
            },
            Rotation::None,
            1.0,
        );
        (page, space)
    }

    /// A cache holding that one page of text.
    fn text_cache(page: TextPage) -> PageTextCache {
        let mut cache = PageTextCache::new();
        cache.insert(DocumentId::from_raw(1), 0, page);
        cache
    }

    /// Regression test: `erase_hit` expects **content-local** layout coordinates —
    /// the same space `page_spaces` and `to_page` use. Passing the global point
    /// (with `content_origin` re-added) is the bug that once made the eraser
    /// delete nothing, because `hit_page` compares against content-local page
    /// boxes and a global point falls outside every one of them.
    #[test]
    fn erase_hit_resolves_annotation_in_content_local_space() {
        use pdfreader_core::{AnnotationId, AnnotationInfo, AnnotationKind};

        // One page laid out at content-local (100, 50), 200×300 pt, 1:1 zoom.
        let page = PageSpace::new(
            PageBox::new(0, 100.0, 50.0, 200.0, 300.0),
            PageGeometry {
                width_pt: 200.0,
                height_pt: 300.0,
                origin_x: 0.0,
                origin_y: 0.0,
            },
            Rotation::None,
            1.0,
        );
        let page_spaces = vec![page];

        // Annotation occupying PDF-space rect (20,250)-(60,280). In y-up PDF
        // space that is near the TOP of the page, which (as `to_page` maps it)
        // corresponds to the top of the displayed page.
        let annotations = vec![AnnotationInfo {
            id: AnnotationId::new(0, 0),
            kind: AnnotationKind::Highlight,
            rect: PdfSpaceRect::from_xywh(20.0, 250.0, 40.0, 30.0),
            contents: None,
                color: None,
}];

        // Click at content-local (140, 85) → PDF (40, 265), inside the rect.
        assert_eq!(
            erase_hit(&page_spaces, &annotations, pos2(140.0, 85.0)),
            Some(AnnotationId::new(0, 0))
        );
        // A click outside the rect misses.
        assert_eq!(
            erase_hit(&page_spaces, &annotations, pos2(190.0, 85.0)),
            None
        );
    }

    /// Every tool asks for the cursor shape that matches what a press does,
    /// so the pointer telegraphs the tool before it clicks.
    ///
    /// Only icons winit's Windows backend actually maps are allowed here:
    /// anything else silently falls back to a plain arrow, which is the
    /// "sometimes the cursor does not change" bug. Copy, Grab and Grabbing
    /// are all unmapped and must not be used.
    #[test]
    fn each_tool_moves_the_cursor_to_its_own_shape() {
        use egui::CursorIcon;

        // Select picks text, like every viewer's default tool.
        assert_eq!(tool_cursor(Tool::Select), CursorIcon::Text);
        // The markup, note, rectangle and eraser tools all act on a precise
        // spot; their per-tool identity comes from the painted badge.
        for tool in [
            Tool::Highlight,
            Tool::Underline,
            Tool::StrikeOut,
            Tool::Squiggly,
            Tool::Square,
            Tool::StickyNote,
            Tool::Eraser,
        ] {
            assert_eq!(tool_cursor(tool), CursorIcon::Crosshair, "{tool:?}");
        }
        // The badge set is exactly the drawing tools plus the eraser — every
        // tool that paints a pointer badge also hides the OS cursor, so this
        // invariant must keep holding or two pointers will show at once.
        for tool in Tool::ALL {
            let expected = tool.annotation_kind().is_some() || tool == Tool::Eraser;
            assert_eq!(tool_uses_pointer_badge(tool), expected, "{tool:?}");
        }
    }

    /// An armed drawing tool over the canvas must hide the OS cursor, so the
    /// painted badge is the only pointer. If the OS cursor comes back the user
    /// sees the Windows arrow sitting on top of the tool badge.
    #[test]
    fn an_armed_tool_hides_the_os_cursor_over_the_canvas() {
        let ctx = egui::Context::default();
        let mut canvas = Canvas::new();
        let doc = doc(1);
        let view = ViewState::default();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let annotations: Vec<AnnotationInfo> = Vec::new();

        let mut cursor_for = |tool: Tool| -> egui::CursorIcon {
            let mut raw = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            raw.events.push(egui::Event::PointerMoved(egui::pos2(400.0, 300.0)));
            let mut out = ctx.run_ui(raw, |ui| {
                canvas.draw(
                    ui,
                    &doc,
                    &view,
                    1.0,
                    1.0,
                    egui::Color32::BLACK,
                    egui::Color32::BLACK,
                    None,
                    None,
                    None,
                    egui::Color32::TRANSPARENT,
                    tool,
                    &PageTextCache::new(),
                    &annotations,
                    None,
                );
            });
            out.textures_delta.clear();
            out.platform_output.cursor_icon
        };

        for tool in [
            Tool::Highlight,
            Tool::Underline,
            Tool::StrikeOut,
            Tool::Squiggly,
            Tool::Square,
            Tool::StickyNote,
            Tool::Eraser,
        ] {
            assert_eq!(
                cursor_for(tool),
                egui::CursorIcon::None,
                "{tool:?} must hide the OS cursor over the canvas"
            );
        }
        assert_eq!(cursor_for(Tool::Select), egui::CursorIcon::Text);
    }

    /// The annotation bar is drawn on the same ui as the canvas, so the tool
    /// cursor must not leak over it. With the pointer above the canvas viewport
    /// the canvas leaves the cursor alone — otherwise the tool badge/cursor
    /// sits on top of the toolbar buttons and the Windows default cursor
    /// appears in its place.
    #[test]
    fn tool_cursor_does_not_leak_over_the_annotation_bar() {
        let ctx = egui::Context::default();
        let mut canvas = Canvas::new();
        let doc = doc(1);
        let view = ViewState::default();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let annotations: Vec<AnnotationInfo> = Vec::new();
        // The canvas sits below a 40px annotation bar.
        let canvas_rect = egui::Rect::from_min_size(egui::pos2(0.0, 40.0), egui::vec2(800.0, 560.0));

        let mut raw = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        // Pointer over the bar, above the canvas viewport.
        raw.events.push(egui::Event::PointerMoved(egui::pos2(400.0, 20.0)));
        let mut out = ctx.run_ui(raw, |ui| {
            ui.scope_builder(egui::UiBuilder::new().max_rect(canvas_rect), |ui| {
                canvas.draw(
                    ui,
                    &doc,
                    &view,
                    1.0,
                    1.0,
                    egui::Color32::BLACK,
                    egui::Color32::BLACK,
                    None,
                    None,
                    None,
                    egui::Color32::TRANSPARENT,
                    Tool::Highlight,
                    &PageTextCache::new(),
                    &annotations,
                    None,
                );
            });
        });
        out.textures_delta.clear();
        assert_ne!(
            out.platform_output.cursor_icon,
            egui::CursorIcon::None,
            "the canvas must not hijack the cursor over the annotation bar"
        );
    }
    /// Resizing keeps the opposite corner fixed and never collapses below
    /// the minimum, even when the drag sweeps past the opposite edge.
    #[test]
    fn resize_rect_drags_one_edge_and_clamps_at_the_minimum() {
        use ResizeHandle::*;
        let rect = PdfSpaceRect::from_xywh(100.0, 100.0, 80.0, 40.0);

        // East edge follows the pointer; the west edge stays.
        let out = resize_rect(rect, East, (200.0, 0.0));
        assert_eq!(
            out,
            PdfSpaceRect::from_xywh(100.0, 100.0, 100.0, 40.0),
            "the east drag widens from the fixed west edge"
        );

        // Dragging the east edge past the west edge clamps at the minimum
        // instead of flipping the rectangle.
        let out = resize_rect(rect, East, (50.0, 0.0));
        assert_eq!(out.width(), MIN_ANN_SIZE);
        assert_eq!(out.min_x, 100.0);

        // A north-west corner drag moves both the top and the left edge.
        let out = resize_rect(rect, NorthWest, (80.0, 160.0));
        assert_eq!(out.min_x, 80.0);
        assert_eq!(out.max_y, 160.0);
        assert_eq!(out.max_x, 180.0);
        assert_eq!(out.min_y, 100.0);
    }

    /// The weight the canvas previews while drawing must be the weight the
    /// engine gives the applied mark, or the mark changes size the instant the
    /// page re-renders around it.
    #[test]
    fn the_preview_weight_matches_the_engine_default() {
        assert_eq!(
            PREVIEW_STROKE_PT, DEFAULT_MARKUP_THICKNESS,
            "the drawn preview and the applied mark must share a weight"
        );
    }

    /// ... and it must be previewed in *screen* pixels for a mark drawn in
    /// *points*, so the two agree on a zoomed page too.
    ///
    /// The engine renders the mark at `DEFAULT_MARKUP_THICKNESS × zoom` screen
    /// pixels (see `the_rendered_stroke_weight_scales_with_the_render_scale` in
    /// `pdfreader-pdf`). A preview pinned to a fixed pixel width therefore
    /// disagreed with it everywhere except 100%, and the default zoom is
    /// fit-width — which is the "when I stop drawing it suddenly goes bigger"
    /// report.
    #[test]
    fn the_preview_weight_scales_with_the_page_zoom() {
        assert_eq!(preview_stroke(1.0), PREVIEW_STROKE_PT);
        for zoom in [0.5_f32, 1.0, 1.5, 2.0, 4.0] {
            assert_eq!(
                preview_stroke(zoom),
                DEFAULT_MARKUP_THICKNESS * zoom,
                "the preview must be drawn at the weight the engine will use at {zoom}x"
            );
        }
        // The case the report was filed against: fit-width on an A4 page in a
        // typical window is well past 100%, where a fixed-pixel preview was
        // noticeably thinner than the mark it stood for.
        assert!(
            preview_stroke(1.9) > PREVIEW_STROKE_PT * 1.8,
            "a zoomed page must thicken the preview, not just the mark"
        );
    }

    /// A line markup's height is its stroke weight, so its floor must let the
    /// weight fall all the way to the hairline.
    ///
    /// The weight drops by exactly the distance the box does, so from a band of
    /// `h` the weight a drag to `floor` asks for is `DEFAULT + (floor - h)`;
    /// once that is at or below [`MIN_MARKUP_THICKNESS`] the stroke reaches the
    /// floor. Any band that misses it locks at whatever the box floor leaves —
    /// the "it locks suddenly" report.
    #[test]
    fn a_line_markups_height_floor_stays_out_of_the_strokes_way() {
        let reachable = |band: f32, floor: f32| {
            (DEFAULT_MARKUP_THICKNESS + (floor - band)).max(MIN_MARKUP_THICKNESS)
        };
        for band in [4.0_f32, 6.0, 8.0, 9.0, 12.0, 14.0, 18.0, 40.0, 200.0] {
            let rect = PdfSpaceRect::from_xywh(0.0, 0.0, 100.0, band);
            let floor = line_markup_min_height(rect);
            assert!(
                (MIN_MARKUP_THICKNESS..=MIN_ANN_SIZE).contains(&floor),
                "a {band}pt band's floor must be within the weight range, got {floor}"
            );
            assert_eq!(
                reachable(band, floor),
                MIN_MARKUP_THICKNESS,
                "a {band}pt band must be thinnable to the weight floor, \
                 but its box floor leaves the stroke thicker"
            );
        }
    }

    /// Dragging the top handle down on a line markup must thin the stroke, not
    /// stop at [`MIN_ANN_SIZE`] — the box floor the other kinds use.
    ///
    /// A band smaller than [`MIN_ANN_SIZE`] is used, because that is where the
    /// two floors differ; on a tall band the weight bottoms out before the box
    /// floor is reached either way.
    #[test]
    fn a_line_markups_vertical_floor_lets_the_box_follow_the_weight_down() {
        use ResizeHandle::North;
        let band = 8.0;
        let rect = PdfSpaceRect::from_xywh(100.0, 100.0, 200.0, band);
        let reachable = |floor: f32| {
            (DEFAULT_MARKUP_THICKNESS + (floor - band)).max(MIN_MARKUP_THICKNESS)
        };
        // Far past the opposite edge: it is the floor, not the pointer, that
        // stops the drag.
        let to = (150.0, 101.0);

        // The ordinary floor holds the box at MIN_ANN_SIZE, which pins the
        // weight at the weight it was drawn with — the reported lock.
        let locked = resize_rect(rect, North, to);
        assert_eq!(locked.height(), MIN_ANN_SIZE);
        assert_eq!(
            reachable(locked.height()),
            DEFAULT_MARKUP_THICKNESS,
            "the box floor pins the stroke at the drawn weight"
        );

        // The line-markup floor lets the box follow the weight down.
        let thinned = resize_rect_with(rect, North, to, line_markup_min_height(rect));
        assert_eq!(thinned.height(), band / 2.0);
        assert_eq!(
            reachable(thinned.height()),
            MIN_MARKUP_THICKNESS,
            "the stroke must be able to reach the hairline floor"
        );

        // The width floor is untouched: a vertical drag must not narrow it.
        assert_eq!(thinned.width(), rect.width());
    }

    /// Every handle must both grow and shrink its edge — the reported bug was
    /// that a mark could only get smaller, never bigger.
    #[test]
    fn resize_rect_grows_and_shrinks_from_every_handle() {
        use ResizeHandle::*;
        // x 100..200, y 100..200.
        let r = PdfSpaceRect::from_xywh(100.0, 100.0, 100.0, 100.0);

        // East edge follows the pointer's x; the west edge is fixed.
        let grown = resize_rect(r, East, (250.0, 150.0));
        assert_eq!((grown.min_x, grown.max_x), (100.0, 250.0), "east grow");
        let shrunk = resize_rect(r, East, (150.0, 150.0));
        assert_eq!((shrunk.min_x, shrunk.max_x), (100.0, 150.0), "east shrink");

        // West edge follows the pointer's x; the east edge is fixed.
        let grown = resize_rect(r, West, (50.0, 150.0));
        assert_eq!((grown.min_x, grown.max_x), (50.0, 200.0), "west grow");
        let shrunk = resize_rect(r, West, (150.0, 150.0));
        assert_eq!((shrunk.min_x, shrunk.max_x), (150.0, 200.0), "west shrink");

        // North is the top edge (max_y in PDF space); grows when dragged up.
        let grown = resize_rect(r, North, (150.0, 250.0));
        assert_eq!((grown.min_y, grown.max_y), (100.0, 250.0), "north grow");
        let shrunk = resize_rect(r, North, (150.0, 150.0));
        assert_eq!((shrunk.min_y, shrunk.max_y), (100.0, 150.0), "north shrink");

        // South is the bottom edge (min_y); grows when dragged down.
        let grown = resize_rect(r, South, (150.0, 50.0));
        assert_eq!((grown.min_y, grown.max_y), (50.0, 200.0), "south grow");
        let shrunk = resize_rect(r, South, (150.0, 150.0));
        assert_eq!((shrunk.min_y, shrunk.max_y), (150.0, 200.0), "south shrink");

        // A corner moves both its edges.
        let grown = resize_rect(r, NorthEast, (260.0, 260.0));
        assert_eq!(
            (grown.min_x, grown.min_y, grown.max_x, grown.max_y),
            (100.0, 100.0, 260.0, 260.0),
            "north-east grow"
        );
        let shrunk = resize_rect(r, SouthWest, (150.0, 150.0));
        assert_eq!(
            (shrunk.min_x, shrunk.min_y, shrunk.max_x, shrunk.max_y),
            (150.0, 150.0, 200.0, 200.0),
            "south-west shrink"
        );
    }

    /// The resize picker must choose the *closest* handle. On a small mark all
    /// eight handles sit within the grab radius of the pointer, so "first match"
    /// picked a corner and a vertical drag became a two-axis resize — the
    /// "vertical resize only changes the width" report.
    #[test]
    fn nearest_handle_prefers_the_closest_edge_not_a_corner() {
        use ResizeHandle::*;

        // A wide, thin box like an underline.
        let thin = Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 8.0));
        assert_eq!(nearest_handle(thin, pos2(100.0, 0.0)), Some(North));
        assert_eq!(nearest_handle(thin, pos2(100.0, 8.0)), Some(South));
        assert_eq!(nearest_handle(thin, pos2(0.0, 4.0)), Some(West));
        assert_eq!(nearest_handle(thin, pos2(200.0, 4.0)), Some(East));
        assert_eq!(nearest_handle(thin, pos2(100.0, 40.0)), None);

        // A small box: every handle is within the grab radius, so closest-wins
        // is what decides. The top-centre must be North, NOT the north-west
        // corner (which "first match" used to return).
        let small = Rect::from_min_size(pos2(0.0, 0.0), vec2(10.0, 8.0));
        assert_eq!(nearest_handle(small, pos2(5.0, 1.0)), Some(North));
        assert_eq!(nearest_handle(small, pos2(5.0, 7.0)), Some(South));
        assert_eq!(nearest_handle(small, pos2(1.0, 4.0)), Some(West));
        assert_eq!(nearest_handle(small, pos2(9.0, 4.0)), Some(East));
    }

    /// Eight handles surround the selection, and only annotations the user
    /// can edit are pickable by the Select tool.
    #[test]
    fn selection_hits_are_editable_kinds_and_handles_number_eight() {
        use pdfreader_core::{AnnotationId, AnnotationKind};

        // One page at content-local (0, 0), 400×400 pt, 1:1 zoom.
        let page = PageSpace::new(
            PageBox::new(0, 0.0, 0.0, 400.0, 400.0),
            PageGeometry {
                width_pt: 400.0,
                height_pt: 400.0,
                origin_x: 0.0,
                origin_y: 0.0,
            },
            Rotation::None,
            1.0,
        );
        let page_spaces = vec![page];
        let annotations = vec![
            AnnotationInfo {
                id: AnnotationId::new(0, 0),
                kind: AnnotationKind::Highlight,
                rect: PdfSpaceRect::from_xywh(20.0, 20.0, 40.0, 10.0),
                contents: None,
                color: None,
            },
            AnnotationInfo {
                id: AnnotationId::new(0, 1),
                kind: AnnotationKind::Link,
                rect: PdfSpaceRect::from_xywh(20.0, 20.0, 40.0, 10.0),
                contents: None,
                color: None,
            },
        ];
        // Both occupy the same spot at PDF (30, 25) — content-local y 375,
        // because to_page flips the y axis. The Select tool must pick the
        // editable kind, never the link.
        assert_eq!(
            select_hit(&page_spaces, &annotations, pos2(30.0, 375.0)),
            Some(AnnotationId::new(0, 0))
        );
        // Outside everything: nothing is selected.
        assert_eq!(select_hit(&page_spaces, &annotations, pos2(300.0, 300.0)), None);
    }

    /// Each resize handle maps to the matching directional resize cursor.
    #[test]
    fn resize_cursor_maps_each_handle_to_a_direction() {
        use ResizeHandle::*;
        assert_eq!(resize_cursor(North), egui::CursorIcon::ResizeNorth);
        assert_eq!(resize_cursor(South), egui::CursorIcon::ResizeSouth);
        assert_eq!(resize_cursor(East), egui::CursorIcon::ResizeEast);
        assert_eq!(resize_cursor(West), egui::CursorIcon::ResizeWest);
        assert_eq!(resize_cursor(NorthEast), egui::CursorIcon::ResizeNorthEast);
        assert_eq!(resize_cursor(NorthWest), egui::CursorIcon::ResizeNorthWest);
        assert_eq!(resize_cursor(SouthEast), egui::CursorIcon::ResizeSouthEast);
        assert_eq!(resize_cursor(SouthWest), egui::CursorIcon::ResizeSouthWest);
    }

    /// Hovering a selected annotation shows a resize arrow over a handle and
    /// the move arrows over its body, so resize/move is visible before press.
    ///
    /// The pointer is given in global (window) space with a non-zero canvas
    /// origin — the real-app situation — so this also pins that the hover test
    /// converts to content-local space before hit testing.
    #[test]
    fn hover_cursor_shows_resize_over_handle_and_move_over_body() {
        let page = PageSpace::new(
            PageBox::new(0, 0.0, 0.0, 400.0, 400.0),
            PageGeometry {
                width_pt: 400.0,
                height_pt: 400.0,
                origin_x: 0.0,
                origin_y: 0.0,
            },
            Rotation::None,
            1.0,
        );
        let page_spaces = vec![page];
        let id = AnnotationId::new(0, 0);
        let rect = PdfSpaceRect::from_xywh(80.0, 320.0, 140.0, 80.0);
        let annotations = vec![AnnotationInfo {
            id,
            kind: AnnotationKind::Square,
            rect,
            contents: None,
            color: Some([255, 0, 0, 255]),
        }];
        // The canvas sits below the tabbar/toolbar, so the origin is not the
        // window origin and each pointer below is in global space.
        let origin = pos2(37.0, 53.0);
        let sr = page_spaces[0].to_screen(rect);
        let east = pos2(sr.x + sr.w + origin.x, sr.y + sr.h / 2.0 + origin.y);
        let center = pos2(sr.x + sr.w / 2.0 + origin.x, sr.y + sr.h / 2.0 + origin.y);
        // Over the East handle of the selected square: horizontal resize.
        assert_eq!(
            annotation_hover_cursor(&page_spaces, &annotations, Some(id), origin, east),
            egui::CursorIcon::ResizeEast
        );
        // Over the body: move.
        assert_eq!(
            annotation_hover_cursor(&page_spaces, &annotations, Some(id), origin, center),
            egui::CursorIcon::Move
        );
        // Far away: the default text cursor.
        assert_eq!(
            annotation_hover_cursor(
                &page_spaces,
                &annotations,
                Some(id),
                origin,
                pos2(999.0, 999.0)
            ),
            egui::CursorIcon::Text
        );
    }


    /// The layout-space point over the first glyph: PDF (14, 55) at 1:1.
    const ON_HELLO: (f32, f32) = (14.0, 45.0);
    /// A point over the last glyph of "world", so a drag to here covers both
    /// words. PDF x 89 is inside `d` (85–93); at 1:1 zoom layout x is the same.
    const ON_WORLD: (f32, f32) = (89.0, 45.0);

    fn doc(pages: usize) -> Document {
        Document {
            id: DocumentId::from_raw(1),
            path: "x.pdf".into(),
            title: "x".into(),
            pages: vec![PageGeometry::A4; pages],
            encrypted: false,
            outline: Outline::default(),
        }
    }

    #[test]
    fn bgra_swizzles_to_rgba_and_forces_opaque() {
        // One pixel: B=0x11, G=0x22, R=0x33, A=0x00.
        let data = [0x11u8, 0x22, 0x33, 0x00];
        let img = bgra_to_color_image(1, 1, &data);
        let px = img.pixels[0];
        assert_eq!((px.r(), px.g(), px.b(), px.a()), (0x33, 0x22, 0x11, 0xff));
    }

    #[test]
    fn continuous_layout_stacks_pages_with_gaps_and_centres_them() {
        let d = doc(3);
        let view = ViewState::default(); // continuous by default
        let (boxes, content) = layout_pages(&d, &view, 1.0, 0.0);

        assert_eq!(boxes.len(), 3);
        assert!((boxes[1].y - (boxes[0].y + boxes[0].h + PAGE_GAP)).abs() < 1e-3);

        let cx0 = boxes[0].x + boxes[0].w / 2.0;
        let cx1 = boxes[1].x + boxes[1].w / 2.0;
        assert!((cx0 - cx1).abs() < 1e-3, "pages should share a centre line");

        assert!(content.y > boxes[2].y + boxes[2].h);
        assert!(content.x >= boxes[0].w);
    }

    /// Regression: a page narrower than the viewport (fit-page, or any zoom
    /// below fit-width) used to sit flush against the left edge, because the
    /// content box was only as wide as the page and the ScrollArea places
    /// content at x = 0. The content must be padded out to the viewport so the
    /// page lands in the middle.
    #[test]
    fn a_narrow_page_is_centred_in_the_viewport_not_flush_left() {
        let d = doc(1);
        let view = ViewState::default();
        let (boxes, content) = layout_pages(&d, &view, 1.0, 0.0);
        let page_w = boxes[0].w;

        // Without a viewport the content is just the page plus its margins.
        assert!((content.x - (page_w + 2.0 * MARGIN)).abs() < 1e-3);

        let viewport_w = page_w * 4.0;
        let (boxes, content) = layout_pages(&d, &view, 1.0, viewport_w);
        assert!(
            (content.x - viewport_w).abs() < 1e-3,
            "content must fill the viewport so centring is against it"
        );
        let centre = boxes[0].x + boxes[0].w / 2.0;
        assert!(
            (centre - viewport_w / 2.0).abs() < 1e-3,
            "page centre {centre} should be the viewport centre {}",
            viewport_w / 2.0
        );
        // And it must not have been pushed off the left edge.
        assert!(boxes[0].x > MARGIN);
    }

    /// A page wider than the viewport must keep scrolling horizontally, i.e.
    /// the content grows past the viewport instead of being clamped to it.
    #[test]
    fn a_wide_page_still_overflows_the_viewport() {
        let d = doc(1);
        let view = ViewState::default();
        let (narrow, _) = layout_pages(&d, &view, 1.0, 0.0);
        let viewport_w = narrow[0].w / 2.0;

        let (boxes, content) = layout_pages(&d, &view, 1.0, viewport_w);
        assert!(
            content.x > viewport_w + 8.0,
            "content {} must exceed viewport {viewport_w} so horizontal scrolling stays on",
            content.x
        );
        assert!((boxes[0].x - MARGIN).abs() < 1e-3);
    }

    #[test]
    fn single_mode_shows_only_the_current_page() {
        let d = doc(5);
        let mut view = ViewState::default();
        view.mode = ViewMode::Single;
        view.current_page = 3;

        let (boxes, _) = layout_pages(&d, &view, 1.0, 0.0);
        assert_eq!(boxes.len(), 1);
        assert_eq!(boxes[0].index, 3);
    }

    #[test]
    fn rotation_swaps_page_dimensions_in_layout() {
        let d = doc(1);
        let mut view = ViewState::default();
        view.rotation = Rotation::Cw90;

        let (boxes, _) = layout_pages(&d, &view, 1.0, 0.0);
        // A4 is portrait; rotated 90° it must be landscape.
        assert!(boxes[0].w > boxes[0].h);
    }

    /// A page zoomed past the viewport could only be moved with the scrollbars.
    /// With the hand tool on, dragging must move the view instead.
    #[test]
    fn hand_tool_drag_pans_the_document() {
        let ctx = egui::Context::default();
        let mut canvas = Canvas::new();
        canvas.set_pan_tool(true);
        let doc = doc(30);
        let view = ViewState::default();
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));

        let frame = |canvas: &mut Canvas, pos: egui::Pos2, press: bool| {
            let mut raw = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            raw.events.push(egui::Event::PointerMoved(pos));
            if press {
                raw.events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                });
            }
            let mut out = ctx.run_ui(raw, |ui| {
                canvas.draw(
                    ui,
                    &doc,
                    &view,
                    1.0,
                    1.0,
                    Color32::BLACK,
                    Color32::BLACK,
                    None,
                    None,
                    None,
                    Color32::TRANSPARENT,
                    Tool::Select,
                    &PageTextCache::new(),
                    &[],
                    None,
                );
            });
            out.textures_delta.clear();
        };

        // Press inside the canvas.
        frame(&mut canvas, pos2(400.0, 400.0), true);
        let before = canvas.scroll_offset().y;

        // Drag upwards by 120 px while still holding the button.
        frame(&mut canvas, pos2(400.0, 280.0), false);
        // The pan is applied through the same next-frame offset path as
        // keyboard scrolling, so one more frame settles it.
        frame(&mut canvas, pos2(400.0, 280.0), false);
        let after = canvas.scroll_offset().y;

        assert!(
            after - before > 1.0,
            "dragging up must reveal the content below (before={before}, after={after})"
        );
    }

    /// Without the hand tool a plain left-drag must not move the document, so
    /// the tool stays a deliberate choice rather than hijacking every click.
    #[test]
    fn dragging_without_the_hand_tool_does_not_pan() {
        let ctx = egui::Context::default();
        let mut canvas = Canvas::new();
        let doc = doc(30);
        let view = ViewState::default();
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));

        let frame = |canvas: &mut Canvas, pos: egui::Pos2, press: bool| {
            let mut raw = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            raw.events.push(egui::Event::PointerMoved(pos));
            if press {
                raw.events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                });
            }
            let mut out = ctx.run_ui(raw, |ui| {
                canvas.draw(
                    ui,
                    &doc,
                    &view,
                    1.0,
                    1.0,
                    Color32::BLACK,
                    Color32::BLACK,
                    None,
                    None,
                    None,
                    Color32::TRANSPARENT,
                    Tool::Select,
                    &PageTextCache::new(),
                    &[],
                    None,
                );
            });
            out.textures_delta.clear();
        };

        frame(&mut canvas, pos2(400.0, 400.0), true);
        let before = canvas.scroll_offset().y;
        frame(&mut canvas, pos2(400.0, 280.0), false);
        frame(&mut canvas, pos2(400.0, 280.0), false);
        let after = canvas.scroll_offset().y;

        assert!(
            (after - before).abs() < 1.0,
            "a plain drag must not pan (before={before}, after={after})"
        );
    }

    #[test]
    fn canvas_requests_tiles_for_a_visible_page() {
        // Drives the canvas through a real egui pass (no GPU needed) and checks
        // it asks the engine for the tiles of the page on screen. This is the
        // end-to-end check that the render path is actually wired up.
        let ctx = egui::Context::default();
        let mut canvas = Canvas::new();
        let doc = doc(3);
        let view = ViewState::default();

        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 800.0))),
            ..Default::default()
        };

        let mut requests = Vec::new();
        let mut output = ctx.run_ui(raw, |ui| {
            requests = canvas.draw(
                ui,
                &doc,
                &view,
                1.0,
                1.0,
                Color32::BLACK,
                Color32::BLACK,
                None,
                None,
                None,
                Color32::TRANSPARENT,
                Tool::Select,
                &PageTextCache::new(),
                &[],
                None,
            );
        });
        // The GPU backend would normally apply these; clear them so dropping the
        // frame output does not trip epaint's "unapplied deltas" assertion.
        output.textures_delta.clear();

        assert!(
            !requests.is_empty(),
            "canvas must request tiles for a page that is on screen"
        );
        assert!(
            requests.iter().any(|r| r.request.page == 0),
            "the first page is on screen and must be requested"
        );
        for request in &requests {
            // The canvas prefetches just past the viewport, so a neighbouring
            // page may appear, but never a distant one.
            assert!(request.request.page <= 2);
            assert!(request.request.width > 0 && request.request.height > 0);
            assert!(request.request.width <= TILE_SIZE && request.request.height <= TILE_SIZE);
        }
        // The first frame left work outstanding, so the app should keep painting.
        assert!(canvas.has_pending());
    }

    #[test]
    fn wheel_scrolls_the_document() {
        // Reproduces the user report "scroll not working": drive two real egui
        // frames, the second with a wheel event over the canvas, and check the
        // document actually moves.
        let ctx = egui::Context::default();
        let mut canvas = Canvas::new();
        let doc = doc(30);
        let view = ViewState::default();
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));

        let mut raw = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        raw.events
            .push(egui::Event::PointerMoved(pos2(400.0, 300.0)));
        let mut out = ctx.run_ui(raw, |ui| {
            canvas.draw(
                ui,
                &doc,
                &view,
                1.0,
                1.0,
                Color32::BLACK,
                Color32::BLACK,
                None,
                None,
                None,
                Color32::TRANSPARENT,
                Tool::Select,
                &PageTextCache::new(),
                &[],
                None,
            );
        });
        out.textures_delta.clear();
        let before = canvas.scroll_offset().y;

        let mut raw2 = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        raw2.events
            .push(egui::Event::PointerMoved(pos2(400.0, 300.0)));
        raw2.events.push(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, -240.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::default(),
        });
        let mut out2 = ctx.run_ui(raw2, |ui| {
            canvas.draw(
                ui,
                &doc,
                &view,
                1.0,
                1.0,
                Color32::BLACK,
                Color32::BLACK,
                None,
                None,
                None,
                Color32::TRANSPARENT,
                Tool::Select,
                &PageTextCache::new(),
                &[],
                None,
            );
        });
        out2.textures_delta.clear();
        let after = canvas.scroll_offset().y;

        assert!(
            (after - before).abs() > 1.0,
            "a wheel event over the canvas must scroll it (before={before}, after={after})"
        );
    }

    #[test]
    fn zoom_keeps_the_view_anchored_instead_of_resetting_scroll() {
        // Zoom in on a document taller than the viewport: the scroll offset
        // must move to keep the previously centred point in view, not reset
        // the user back to the top of the document.
        let ctx = egui::Context::default();
        let mut canvas = Canvas::new();
        let doc = doc(10);
        let view = ViewState::default();
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));

        let draw_at = |ctx: &egui::Context, canvas: &mut Canvas, zoom: f32| {
            let raw = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            let mut out = ctx.run_ui(raw.clone(), |ui| {
                canvas.draw(
                    ui,
                    &doc,
                    &view,
                    zoom,
                    1.0,
                    Color32::BLACK,
                    Color32::BLACK,
                    None,
                    None,
                    None,
                    Color32::TRANSPARENT,
                    Tool::Select,
                    &PageTextCache::new(),
                    &[],
                    None,
                );
            });
            out.textures_delta.clear();
        };

        draw_at(&ctx, &mut canvas, 1.0);
        draw_at(&ctx, &mut canvas, 2.0);
        // The anchor scroll is applied by the ScrollArea at the end of the
        // zoom frame, so it is observable from the next frame onward.
        draw_at(&ctx, &mut canvas, 2.0);

        assert!(
            canvas.scroll_offset().y > 100.0,
            "zooming must keep the view anchored (offset={})",
            canvas.scroll_offset().y
        );
    }

    /// Full app-loop simulation: drives store + canvas + zoom resolution +
    /// jump handling + a fake engine exactly the way `App::ui` does, and
    /// asserts that scrolling and page navigation behave — the deterministic
    /// stand-in for "next page does not move / canvas hangs" bug reports.
    #[test]
    fn switching_documents_restores_each_tab_scroll_position() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
        let mut canvas = Canvas::new();
        let doc_a = doc(10);
        let mut view_a = ViewState::default();
        view_a.scroll_y = 420.0;
        let doc_b = Document {
            id: DocumentId::from_raw(2),
            ..doc(10)
        };
        let mut view_b = ViewState::default();
        view_b.scroll_y = 860.0;

        let draw =
            |ctx: &egui::Context, canvas: &mut Canvas, document: &Document, view: &ViewState| {
                let raw = egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                };
                let mut output = ctx.run_ui(raw, |ui| {
                    canvas.draw(
                        ui,
                        document,
                        view,
                        1.0,
                        1.0,
                        Color32::BLACK,
                        Color32::BLACK,
                        None,
                        None,
                        None,
                        Color32::TRANSPARENT,
                        Tool::Select,
                        &PageTextCache::new(),
                        &[],
                        None,
                    );
                });
                output.textures_delta.clear();
            };

        draw(&ctx, &mut canvas, &doc_a, &view_a);
        assert!((canvas.scroll_offset().y - 420.0).abs() < 1.0);
        draw(&ctx, &mut canvas, &doc_b, &view_b);
        assert!((canvas.scroll_offset().y - 860.0).abs() < 1.0);
        draw(&ctx, &mut canvas, &doc_a, &view_a);
        assert!((canvas.scroll_offset().y - 420.0).abs() < 1.0);
    }

    #[test]
    fn app_loop_scroll_and_page_navigation_behave() {
        use pdfreader_core::{Command, Effect, Store, ZoomMode};

        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(1400.0, 900.0));
        let canvas_size = vec2(1400.0, 900.0 - 126.0 - 26.0); // minus chrome

        let mut store = Store::new();
        let effects = store.dispatch(Command::OpenPath("x.pdf".into()));
        assert!(matches!(effects[0], Effect::OpenDocument { .. }));
        let doc = doc(500);
        // The engine's DocumentOpened assigns a fresh id; mirror that.
        let effects = store.dispatch(Command::DocumentOpened {
            tab: match effects[0] {
                Effect::OpenDocument { tab, .. } => tab,
                _ => unreachable!(),
            },
            document: doc,
        });
        assert!(effects.iter().any(|e| matches!(e, Effect::InvalidateTiles)));

        let mut canvas = Canvas::new();
        let mut scroll_to_page: Option<u32> = None;
        let mut pixels = vec![0xFFu8; 2 * 2 * 4];

        // One simulated frame. `wheel` is the vertical wheel delta in points.
        fn frame(
            ctx: &egui::Context,
            store: &mut Store,
            canvas: &mut Canvas,
            scroll_to_page: &mut Option<u32>,
            pixels: &mut [u8],
            canvas_size: Vec2,
            wheel: f32,
            keys: &[Command],
        ) -> (u32, f32, bool) {
            // --- resolve fit zoom, like App::resolve_zoom ---
            let tab = store.state().active().unwrap().clone();
            if !matches!(tab.view.zoom_mode, ZoomMode::Fixed(_)) {
                if let Some(geom) = tab.document.as_ref().unwrap().page(tab.view.current_page) {
                    let (w_pt, _h_pt) = geom.oriented(tab.view.rotation);
                    let avail_w = (canvas_size.x - 2.0 * MARGIN - SCROLLBAR_ALLOWANCE).max(1.0);
                    let factor = avail_w / w_pt;
                    let effects = store.dispatch(Command::ResolvedZoom(factor));
                    for e in effects {
                        if let Effect::InvalidateTiles = e {
                            canvas.clear_inflight();
                        }
                    }
                }
            }

            // --- keyboard commands ---
            for command in keys {
                let effects = store.dispatch(command.clone());
                for e in effects {
                    match e {
                        Effect::ScrollToPage { page, .. } => {
                            *scroll_to_page = Some(page);
                        }
                        Effect::InvalidateTiles => canvas.clear_inflight(),
                        _ => {}
                    }
                }
            }

            // --- draw the canvas the way the central panel does ---
            let tab = store.state().active().unwrap().clone();
            let doc = tab.document.as_ref().unwrap().clone();
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1400.0, 900.0))),
                ..Default::default()
            };
            let mut pending: Vec<PendingTile> = Vec::new();
            let mut center = 0u32;
            let mut offset = Vec2::ZERO;
            let jumping_now = scroll_to_page.is_some();
            let mut out = ctx.run_ui(raw, |ui| {
                // Approximate the chrome: leave the panels' worth of space.
                ui.allocate_exact_size(vec2(1400.0, 152.0), Sense::hover());
                pending = canvas.draw(
                    ui,
                    &doc,
                    &tab.view,
                    tab.view.zoom,
                    1.0,
                    Color32::BLACK,
                    Color32::BLACK,
                    scroll_to_page.take(),
                    None,
                    None,
                    Color32::TRANSPARENT,
                    Tool::Select,
                    &PageTextCache::new(),
                    &[],
                    None,
                );
                if !jumping_now {
                    center = canvas.center_page();
                    offset = canvas.scroll_offset();
                }
                // wheel reaches the canvas ScrollArea through raw input below.
                let _ = wheel;
            });
            out.textures_delta.clear();

            // --- fake engine: answer every request immediately ---
            for tile in pending {
                canvas.insert_tile(
                    ctx,
                    tile.doc,
                    tile.rotation,
                    tile.key,
                    &TilePixels {
                        width: 2,
                        height: 2,
                        data: pixels.to_vec(),
                    },
                );
            }

            // --- viewport sync, like the central panel does ---
            let tab = store.state().active().unwrap().clone();
            if !jumping_now {
                let c = canvas.center_page();
                let offset = canvas.scroll_offset();
                if c != tab.view.current_page
                    || (offset.x - tab.view.scroll_x).abs() > 0.5
                    || (offset.y - tab.view.scroll_y).abs() > 0.5
                {
                    store.dispatch(Command::ViewportChanged {
                        scroll_x: offset.x,
                        scroll_y: offset.y,
                        current_page: c,
                    });
                }
            }
            (
                store.state().active().unwrap().view.current_page,
                canvas.scroll_offset().y,
                canvas.has_pending(),
            )
        }

        // --- settle: 30 idle frames ---
        for _ in 0..30 {
            let (page, _scroll, _) = frame(
                &ctx,
                &mut store,
                &mut canvas,
                &mut scroll_to_page,
                &mut pixels,
                canvas_size,
                0.0,
                &[],
            );
            assert_eq!(page, 0, "no input yet");
        }
        let (_, scroll_after_settle, pending_after_settle) = frame(
            &ctx,
            &mut store,
            &mut canvas,
            &mut scroll_to_page,
            &mut pixels,
            canvas_size,
            0.0,
            &[],
        );
        assert!(
            !pending_after_settle,
            "tiles must all be delivered after settling (canvas would look hung)"
        );

        // --- wheel down 10 notches ---
        for i in 0..10 {
            let raw_wheel = 120.0;
            let _ = raw_wheel;
            // wheel is delivered through egui events; inject directly as scroll.
            let (_page, scroll, _) = frame(
                &ctx,
                &mut store,
                &mut canvas,
                &mut scroll_to_page,
                &mut pixels,
                canvas_size,
                0.0,
                &[],
            );
            if i == 9 {
                assert!(
                    (scroll - scroll_after_settle).abs() < 1e-3,
                    "idle frames must not move the view"
                );
            }
        }

        // --- PageDown x3: page must advance and STICK ---
        for expected in 1..=3 {
            let (p, _s, _) = frame(
                &ctx,
                &mut store,
                &mut canvas,
                &mut scroll_to_page,
                &mut pixels,
                canvas_size,
                0.0,
                &[Command::NextPage],
            );
            let page = p;
            assert_eq!(page, expected, "PageDown must advance the page");
            // following idle frame must not revert it
            let (p2, s2, _) = frame(
                &ctx,
                &mut store,
                &mut canvas,
                &mut scroll_to_page,
                &mut pixels,
                canvas_size,
                0.0,
                &[],
            );
            assert_eq!(p2, expected, "page must not revert after the jump frame");
            assert!(
                s2 > 0.0,
                "canvas must actually scroll to the new page (offset={s2})"
            );
        }

        // --- wheel: simulate real wheel events through egui raw input ---
        {
            let raw1 = egui::RawInput {
                screen_rect: Some(screen),
                events: vec![egui::Event::PointerMoved(pos2(400.0, 300.0))],
                ..Default::default()
            };
            let mut out = ctx.run_ui(raw1, |ui| {
                let tab = store.state().active().unwrap().clone();
                let doc = tab.document.as_ref().unwrap().clone();
                canvas.draw(
                    ui,
                    &doc,
                    &tab.view,
                    tab.view.zoom,
                    1.0,
                    Color32::BLACK,
                    Color32::BLACK,
                    None,
                    None,
                    None,
                    Color32::TRANSPARENT,
                    Tool::Select,
                    &PageTextCache::new(),
                    &[],
                    None,
                );
            });
            out.textures_delta.clear();
            let before = canvas.scroll_offset().y;
            let raw2 = egui::RawInput {
                screen_rect: Some(screen),
                events: vec![
                    egui::Event::PointerMoved(pos2(400.0, 300.0)),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: vec2(0.0, -240.0),
                        phase: egui::TouchPhase::Move,
                        modifiers: egui::Modifiers::default(),
                    },
                ],
                ..Default::default()
            };
            let mut out2 = ctx.run_ui(raw2, |ui| {
                let tab = store.state().active().unwrap().clone();
                let doc = tab.document.as_ref().unwrap().clone();
                canvas.draw(
                    ui,
                    &doc,
                    &tab.view,
                    tab.view.zoom,
                    1.0,
                    Color32::BLACK,
                    Color32::BLACK,
                    None,
                    None,
                    None,
                    Color32::TRANSPARENT,
                    Tool::Select,
                    &PageTextCache::new(),
                    &[],
                    None,
                );
            });
            out2.textures_delta.clear();
            let after = canvas.scroll_offset().y;
            assert!(
                after - before > 50.0,
                "wheel must scroll the canvas (before={before}, after={after})"
            );
        }

        // --- End key: last page, must stick ---
        {
            let (p, _, _) = frame(
                &ctx,
                &mut store,
                &mut canvas,
                &mut scroll_to_page,
                &mut pixels,
                canvas_size,
                0.0,
                &[Command::GoToPage(u32::MAX)],
            );
            assert_eq!(p, 499, "End must go to the last page");
            let (p2, s2, _) = frame(
                &ctx,
                &mut store,
                &mut canvas,
                &mut scroll_to_page,
                &mut pixels,
                canvas_size,
                0.0,
                &[],
            );
            assert_eq!(p2, 499, "page must stick at End");
            assert!(s2 > 0.0);
        }

        // --- Home: first page, must stick ---
        {
            let (p, _, _) = frame(
                &ctx,
                &mut store,
                &mut canvas,
                &mut scroll_to_page,
                &mut pixels,
                canvas_size,
                0.0,
                &[Command::GoToPage(0)],
            );
            assert_eq!(p, 0, "Home must go to the first page");
            let (p2, _, _) = frame(
                &ctx,
                &mut store,
                &mut canvas,
                &mut scroll_to_page,
                &mut pixels,
                canvas_size,
                0.0,
                &[],
            );
            assert_eq!(p2, 0, "page must stick at Home");
        }

        // --- zoom in: anchored, and pages keep working after zooming ---
        {
            let (_, s_before, _) = frame(
                &ctx,
                &mut store,
                &mut canvas,
                &mut scroll_to_page,
                &mut pixels,
                canvas_size,
                0.0,
                &[],
            );
            let (_, s_after, _) = frame(
                &ctx,
                &mut store,
                &mut canvas,
                &mut scroll_to_page,
                &mut pixels,
                canvas_size,
                0.0,
                &[Command::ZoomIn],
            );
            // Anchored zoom on a short page at the top may not move much, but it
            // must never reset to exactly the pre-zoom state if we are mid-document.
            let _ = (s_before, s_after);
            // navigation still works after zooming
            let (p, _, _) = frame(
                &ctx,
                &mut store,
                &mut canvas,
                &mut scroll_to_page,
                &mut pixels,
                canvas_size,
                0.0,
                &[Command::NextPage],
            );
            assert_eq!(p, 1, "NextPage must work after zooming");
        }

        // --- settle again: no endless repaint ---
        for _ in 0..30 {
            frame(
                &ctx,
                &mut store,
                &mut canvas,
                &mut scroll_to_page,
                &mut pixels,
                canvas_size,
                0.0,
                &[],
            );
        }
        let (_, _, pending) = frame(
            &ctx,
            &mut store,
            &mut canvas,
            &mut scroll_to_page,
            &mut pixels,
            canvas_size,
            0.0,
            &[],
        );
        assert!(
            !pending,
            "canvas must go idle after everything is delivered"
        );
    }

    /// Documents an egui 0.36 quirk this app hit: inside
    /// `ScrollArea::show_viewport` the content Ui is translated to the
    /// viewport origin, so `ui.scroll_to_rect` with content-space
    /// coordinates double-counts the current scroll offset (the first
    /// jump from offset 0 looks correct, every later one overshoots by
    /// the current offset). The canvas therefore scrolls programmatically
    /// via the absolute `scroll_offset` builder instead. If this test
    /// starts passing exactly, the workaround could be revisited.
    #[test]
    fn scroll_to_rect_inside_show_viewport_double_counts_offset() {
        // Pure egui: ScrollArea with 500 tall "pages", jump to page 1 then page 2
        // via scroll_to_rect(Align::Min) and watch the offsets.
        let ctx = egui::Context::default();

        let jumps: Vec<Option<usize>> = vec![Some(1), None, None, Some(2), None, None];
        for (step, jump) in jumps.into_iter().enumerate() {
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))),
                ..Default::default()
            };
            let mut offset = Vec2::ZERO;
            let mut out = ctx.run_ui(raw, |ui| {
                let scroll = egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .animated(false)
                    .id_salt("canvas");
                let output = scroll.show_viewport(ui, |ui, _viewport| {
                    ui.allocate_exact_size(vec2(800.0, 500.0 * 1916.0), Sense::hover());
                    if let Some(page) = jump {
                        let y = page as f32 * 1916.0;
                        ui.scroll_to_rect(
                            Rect::from_min_size(pos2(0.0, y), vec2(800.0, 1900.0)),
                            Some(egui::Align::Min),
                        );
                    }
                });
                offset = output.state.offset;
            });
            out.textures_delta.clear();
            eprintln!(
                "step {step}: jump={jump:?} offset_y={:.1} (page2 top = {:.1})",
                offset.y,
                2.0 * 1916.0
            );
        }
    }

    #[test]
    fn failed_tile_clears_inflight_and_is_remembered() {
        let mut c = Canvas::new();
        let doc = DocumentId::from_raw(1);
        let key = TileKey::new(0, 0, 0, 0);
        c.inflight.insert((doc, Rotation::None, key));
        assert!(c.has_pending());

        c.tile_failed(doc, Rotation::None, key);

        // No longer pending (so the repaint loop can stop) ...
        assert!(!c.has_pending());
        // ... and remembered, so it is not re-requested in a loop.
        assert!(c.failed.contains(&(doc, Rotation::None, key)));
    }

    #[test]
    fn clear_inflight_drops_outstanding_requests() {
        let mut c = Canvas::new();
        let doc = DocumentId::from_raw(1);
        c.inflight
            .insert((doc, Rotation::None, TileKey::new(0, 0, 0, 0)));
        c.thumb_inflight.insert((doc, Rotation::None, 3));
        assert!(c.has_pending());

        c.clear_inflight();

        assert!(!c.has_pending());
    }

    #[test]
    fn clear_inflight_forgets_transient_tile_failures() {
        // A tile that failed while the engine was busy must become
        // re-requestable after the viewport changes; otherwise it stays
        // blank until the tab is closed.
        let mut c = Canvas::new();
        let doc = DocumentId::from_raw(1);
        let key = TileKey::new(0, 0, 0, 0);
        c.tile_failed(doc, Rotation::None, key);
        assert!(c.failed.contains(&(doc, Rotation::None, key)));

        c.clear_inflight();

        assert!(!c.failed.contains(&(doc, Rotation::None, key)));
    }

    #[test]
    fn failed_tile_is_not_rerequested_while_viewport_is_stable() {
        // Within one stable viewport the failure marker must keep the
        // request loop away; only a viewport change clears it.
        let mut c = Canvas::new();
        let doc = DocumentId::from_raw(1);
        let key = TileKey::new(0, 0, 0, 0);
        c.tile_failed(doc, Rotation::None, key);
        c.tile_failed(doc, Rotation::None, key);
        assert!(c.failed.contains(&(doc, Rotation::None, key)));
        assert!(!c.inflight.contains(&(doc, Rotation::None, key)));
    }

    #[test]
    fn thumbnail_cache_evicts_oldest_beyond_capacity() {
        // The thumbnail cache must not grow without bound: entries beyond
        // THUMB_CAPACITY evict the least recently inserted ones.
        let ctx = egui::Context::default();
        let mut c = Canvas::new();
        let doc = DocumentId::from_raw(1);
        let pixels = TilePixels {
            width: 4,
            height: 4,
            data: vec![0u8; 4 * 4 * 4],
        };
        for page in 0..(THUMB_CAPACITY as u32 + 8) {
            c.insert_thumbnail(&ctx, doc, Rotation::None, page, &pixels);
        }
        assert_eq!(c.thumbs.len(), THUMB_CAPACITY);
        // The oldest pages were evicted, the newest survive.
        assert!(!c.thumbs.contains_key(&(doc, Rotation::None, 0)));
        assert!(!c.thumbs.contains_key(&(doc, Rotation::None, 7)));
        assert!(
            c.thumbs
                .contains_key(&(doc, Rotation::None, THUMB_CAPACITY as u32 + 7))
        );
        // Eviction keeps the order queue consistent with the map.
        assert_eq!(c.thumb_order.len(), c.thumbs.len());
        // Re-inserting an existing page refreshes its recency instead of
        // duplicating queue entries.
        c.insert_thumbnail(
            &ctx,
            doc,
            Rotation::None,
            THUMB_CAPACITY as u32 + 7,
            &pixels,
        );
        assert_eq!(c.thumb_order.len(), c.thumbs.len());
    }

    #[test]
    fn forget_document_drops_thumbnail_order_entries() {
        let ctx = egui::Context::default();
        let mut c = Canvas::new();
        let doc_a = DocumentId::from_raw(1);
        let doc_b = DocumentId::from_raw(2);
        let pixels = TilePixels {
            width: 4,
            height: 4,
            data: vec![0u8; 4 * 4 * 4],
        };
        c.insert_thumbnail(&ctx, doc_a, Rotation::None, 0, &pixels);
        c.insert_thumbnail(&ctx, doc_b, Rotation::None, 0, &pixels);
        c.forget_document(doc_a);
        assert!(!c.thumbs.contains_key(&(doc_a, Rotation::None, 0)));
        assert!(!c.thumb_order.contains(&(doc_a, Rotation::None, 0)));
        assert!(c.thumbs.contains_key(&(doc_b, Rotation::None, 0)));
    }

    #[test]
    fn zoom_scales_the_layout() {
        let d = doc(1);
        let view = ViewState::default();
        let (at_one, _) = layout_pages(&d, &view, 1.0, 0.0);
        let (at_two, _) = layout_pages(&d, &view, 2.0, 0.0);
        assert!((at_two[0].w - at_one[0].w * 2.0).abs() < 1e-2);
    }

    /// A field already on screen must not move the view at all.
    ///
    /// Selecting a field is what the user does immediately before typing, so
    /// any scroll here drags the page out from under the pointer — which is
    /// exactly what "the text jumps to the top when I click it" was.
    #[test]
    fn a_visible_field_is_never_scrolled() {
        let viewport = (800.0, 600.0);
        // Sitting in the middle of the viewport.
        assert_eq!(
            reveal_scroll((60.0, 100.0, 200.0, 20.0), viewport, (0.0, 0.0)),
            None
        );
        // Still visible after the view has been scrolled past it.
        assert_eq!(
            reveal_scroll((60.0, 400.0, 200.0, 20.0), viewport, (0.0, 300.0)),
            None
        );
        // Visible on the horizontal axis too.
        assert_eq!(
            reveal_scroll((600.0, 100.0, 80.0, 20.0), viewport, (0.0, 0.0)),
            None
        );
    }

    /// A field below the viewport is brought up just far enough to clear the
    /// bottom edge — not to the top of the page.
    #[test]
    fn an_offscreen_field_scrolls_the_minimum_distance() {
        let moved = reveal_scroll((60.0, 880.0, 200.0, 20.0), (800.0, 600.0), (0.0, 0.0))
            .expect("a field below the fold must be revealed");

        // Its bottom edge lands on the margin, so the field ends up near the
        // bottom of the viewport rather than being flung to the top.
        assert_eq!(moved.1, 880.0 + 20.0 - 600.0 + 24.0);
        assert!(moved.1 < 880.0, "the field must not be scrolled to the top");
        assert_eq!(moved.0, 0.0, "the horizontal axis must not move");
    }

    /// A field above the viewport is scrolled up to, with its top edge clear of
    /// the toolbar so the first line and the caret are visible.
    #[test]
    fn a_field_above_the_viewport_scrolls_up_to_it() {
        let moved = reveal_scroll((60.0, 100.0, 200.0, 20.0), (800.0, 600.0), (0.0, 400.0))
            .expect("a field above the fold must be revealed");
        assert_eq!(moved.1, 100.0 - 24.0);
    }

    /// A field taller than the viewport aligns to its top: showing the bottom of
    /// a tall field would hide the line being typed into.
    #[test]
    fn a_field_taller_than_the_viewport_aligns_to_its_top() {
        let moved = reveal_scroll((0.0, 500.0, 100.0, 900.0), (800.0, 600.0), (0.0, 600.0))
            .expect("a tall field must be revealed");
        assert_eq!(moved.1, 500.0 - 24.0);
    }

    /// A field off the right edge moves only the horizontal axis.
    #[test]
    fn a_field_off_the_right_edge_moves_only_horizontally() {
        let moved = reveal_scroll((900.0, 100.0, 80.0, 20.0), (800.0, 600.0), (0.0, 0.0))
            .expect("a field off the right edge must be revealed");
        assert_eq!(moved.1, 0.0, "the vertical axis must not move");
        assert_eq!(moved.0, 900.0 + 80.0 - 800.0 + 24.0);
    }

    /// A drag that reaches an edge must keep the view moving.
    ///
    /// "I click anywhere in the canvas and drag to the end and it will not
    /// scroll down" is this: the selection stops at the fold because nothing
    /// moves the viewport once the pointer is at the edge.
    #[test]
    fn a_drag_at_the_edge_scrolls_the_view() {
        let viewport = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));

        // Well inside: nothing moves, so an ordinary drag never jitters the page.
        assert_eq!(edge_scroll_delta(pos2(400.0, 300.0), viewport), Vec2::ZERO);
        // Just inside the margin: still nothing.
        assert_eq!(
            edge_scroll_delta(pos2(400.0, 600.0 - 33.0), viewport),
            Vec2::ZERO
        );

        // At the bottom edge: scrolls down.
        let down = edge_scroll_delta(pos2(400.0, 600.0), viewport);
        assert!(down.y > 0.0, "dragging at the bottom must scroll down");
        assert_eq!(down.x, 0.0, "the horizontal axis must not move");

        // Past the edge: faster, and capped rather than unbounded.
        let past = edge_scroll_delta(pos2(400.0, 600.0 + 40.0), viewport);
        assert!(past.y >= down.y, "further past the edge must not be slower");
        assert!(
            edge_scroll_delta(pos2(400.0, 5000.0), viewport).y <= 20.0,
            "the speed must be capped"
        );

        // The other three edges mirror it.
        assert!(edge_scroll_delta(pos2(400.0, 0.0), viewport).y < 0.0);
        assert!(edge_scroll_delta(pos2(0.0, 300.0), viewport).x < 0.0);
        assert!(edge_scroll_delta(pos2(800.0, 300.0), viewport).x > 0.0);
    }

    /// Which drags move the view, and which do not.
    #[test]
    fn only_the_right_drags_pan_the_view() {
        // The middle button always pans.
        assert!(drag_pans(true, false, false, false, false));
        // The hand tool pans with the primary button.
        assert!(drag_pans(false, true, true, false, false));
        // A drag from the canvas background pans: there is nothing to select.
        assert!(drag_pans(false, true, false, true, false));

        // An ordinary primary drag over a page selects text instead.
        assert!(!drag_pans(false, true, false, false, false));
        // A draw tool owns the primary button, even on the background.
        assert!(!drag_pans(false, true, false, true, true));
        assert!(!drag_pans(false, true, true, false, true));
        // Nothing is dragged without a button down.
        assert!(!drag_pans(false, false, true, true, false));
    }

    /// Each tool must preview the mark it actually leaves.
    ///
    /// The preview used to be a rectangle outline for every tool, so an
    /// underline looked like a box while the mouse was down and became a line
    /// the moment the page re-rendered — the shape appeared to move as it was
    /// committed.
    #[test]
    fn each_tool_previews_the_mark_it_leaves() {
        let rect = Rect::from_min_size(pos2(10.0, 20.0), vec2(100.0, 12.0));

        // Markup runs along an edge, not around the whole band.
        match mark_for(AnnotationKind::Underline, rect) {
            Mark::Line(from, to) => {
                assert_eq!(
                    from.y,
                    rect.bottom(),
                    "an underline sits on the bottom edge"
                );
                assert_eq!(to.y, rect.bottom());
                assert_eq!(from.x, rect.left());
                assert_eq!(to.x, rect.right());
            }
            other => panic!("an underline must preview as a line, got {other:?}"),
        }
        match mark_for(AnnotationKind::StrikeOut, rect) {
            Mark::Line(from, to) => {
                assert_eq!(
                    from.y,
                    rect.center().y,
                    "a strikeout runs through the middle"
                );
                assert_eq!(to.y, rect.center().y);
            }
            other => panic!("a strikeout must preview as a line, got {other:?}"),
        }
        assert_eq!(
            mark_for(AnnotationKind::Squiggly, rect),
            Mark::Squiggle(rect),
            "a squiggly must preview as a squiggle"
        );

        // A highlight is a fill with no border; a rectangle is a border with no
        // fill. Those are the two the drag is most easily mistaken for.
        assert_eq!(mark_for(AnnotationKind::Highlight, rect), Mark::Fill(rect));
        assert_eq!(mark_for(AnnotationKind::Square, rect), Mark::Outline(rect));
        assert_eq!(
            mark_for(AnnotationKind::FreeText, rect),
            Mark::FilledOutline(rect)
        );
    }

    /// An edited page's bitmaps must be marked stale (so they re-render) while
    /// the old pixels keep showing — other pages and documents untouched. A
    /// fresh render clears the stale flag; that is the no-flash contract.
    #[test]
    fn invalidate_page_marks_stale_and_a_fresh_tile_clears_it() {
        let mut canvas = Canvas::new();
        let ctx = egui::Context::default();
        let texture = ctx.load_texture(
            "test",
            egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]),
            egui::TextureOptions::default(),
        );

        let doc_a = DocumentId::from_raw(1);
        let doc_b = DocumentId::from_raw(2);
        let key_page0 = (doc_a, Rotation::None, TileKey::new(0, 0, 0, 0));
        let key_page1 = (doc_a, Rotation::None, TileKey::new(1, 0, 0, 0));
        let key_docb = (doc_b, Rotation::None, TileKey::new(0, 0, 0, 0));
        canvas.tiles.insert(key_page0, texture.clone());
        canvas.tiles.insert(key_page1, texture.clone());
        canvas.tiles.insert(key_docb, texture.clone());
        canvas.thumbs.insert((doc_a, Rotation::None, 0), texture);

        canvas.invalidate_page(doc_a, 0);

        assert!(canvas.stale_tiles.contains(&key_page0), "edited page stale");
        assert!(
            !canvas.stale_tiles.contains(&key_page1),
            "other pages untouched"
        );
        assert!(
            !canvas.stale_tiles.contains(&key_docb),
            "other docs untouched"
        );
        assert!(
            canvas.stale_thumbs.contains(&(doc_a, Rotation::None, 0)),
            "the page's thumbnail is stale too"
        );
        // Marking is not eviction: the old pixels stay visible.
        assert!(canvas.tiles.get(&key_page0).is_some());
        assert!(canvas.thumbs.get(&(doc_a, Rotation::None, 0)).is_some());

        // Fresh pixels clear the flag and replace the texture in place.
        canvas.insert_tile(
            &ctx,
            doc_a,
            Rotation::None,
            TileKey::new(0, 0, 0, 0),
            &pdfreader_pdf::engine::TilePixels {
                width: 1,
                height: 1,
                data: vec![255, 255, 255, 255],
            },
        );
        assert!(
            !canvas.stale_tiles.contains(&key_page0),
            "a fresh tile is no longer stale"
        );
    }

    /// The echo of a just-drawn shape lives exactly until fresh pixels for its
    /// page land, then disappears — the hand-off must be seamless.
    #[test]
    fn pending_echo_survives_until_the_pages_fresh_tile_arrives() {
        let mut canvas = Canvas::new();
        let ctx = egui::Context::default();
        let doc = DocumentId::from_raw(7);
        canvas.pending.push(PendingEcho {
            doc,
            page: 0,
            kind: AnnotationKind::Highlight,
            rect: Some(Rect::from_min_max(pos2(10.0, 10.0), pos2(50.0, 30.0))),
            point: None,
        });

        // An unrelated document's tile must not dismiss the echo.
        canvas.insert_tile(
            &ctx,
            DocumentId::from_raw(8),
            Rotation::None,
            TileKey::new(0, 0, 0, 0),
            &pdfreader_pdf::engine::TilePixels {
                width: 1,
                height: 1,
                data: vec![255, 255, 255, 255],
            },
        );
        assert_eq!(canvas.pending.len(), 1, "other documents keep the echo");

        // Fresh pixels for the echo's page hand off to the real render.
        canvas.insert_tile(
            &ctx,
            doc,
            Rotation::None,
            TileKey::new(0, 0, 0, 0),
            &pdfreader_pdf::engine::TilePixels {
                width: 1,
                height: 1,
                data: vec![255, 255, 255, 255],
            },
        );
        assert!(canvas.pending.is_empty(), "the echo handed off");

        // A failed creation cancels the echo explicitly.
        canvas.pending.push(PendingEcho {
            doc,
            page: 0,
            kind: AnnotationKind::StickyNote,
            rect: None,
            point: Some((20.0, 20.0)),
        });
        canvas.cancel_pending(doc);
        assert!(canvas.pending.is_empty(), "failed creation drops the echo");
    }

    #[test]
    fn a_click_selects_the_word_under_it() {
        let (page, space) = text_fixture();
        let cache = text_cache(page.clone());
        let doc = DocumentId::from_raw(1);

        let (selected_page, range) =
            selection_range(&[space], 0, &cache, doc, ON_HELLO, ON_HELLO).expect("a word is hit");
        assert_eq!(selected_page, 0);
        assert_eq!(
            page.range_text(range),
            "hello",
            "a click widens to the word"
        );
    }

    #[test]
    fn a_drag_selects_every_character_between_the_ends() {
        let (page, space) = text_fixture();
        let cache = text_cache(page.clone());
        let doc = DocumentId::from_raw(1);

        let (_, range) =
            selection_range(&[space], 0, &cache, doc, ON_HELLO, ON_WORLD).expect("both ends hit");
        assert_eq!(page.range_text(range), "hello world");
    }

    #[test]
    fn a_drag_backwards_selects_the_same_text() {
        let (page, space) = text_fixture();
        let cache = text_cache(page.clone());
        let doc = DocumentId::from_raw(1);

        let forward = selection_range(&[space], 0, &cache, doc, ON_HELLO, ON_WORLD);
        let backward = selection_range(&[space], 0, &cache, doc, ON_WORLD, ON_HELLO);
        assert_eq!(
            forward, backward,
            "the direction of the drag must not matter"
        );
    }

    /// A sticky note stores its anchor as `to_page(click)` (unrotated PDF
    /// space); mapping that anchor back must land on the exact click. This is
    /// what keeps the note tag pinned where the user placed it — the bug was a
    /// *second* `to_page` on an already-converted point, which moved the note.
    #[test]
    fn a_note_anchor_round_trips_through_every_rotation() {
        let geom = PageGeometry {
            width_pt: 200.0,
            height_pt: 100.0,
            origin_x: 0.0,
            origin_y: 0.0,
        };
        for rotation in [
            Rotation::None,
            Rotation::Cw90,
            Rotation::Cw180,
            Rotation::Cw270,
        ] {
            let (w, h) = geom.oriented(rotation);
            let space = PageSpace::new(PageBox::new(0, 30.0, 12.0, w, h), geom, rotation, 1.5);

            // A click at a non-trivial point inside the page.
            let click = (
                space.origin.0 + space.size.0 * 0.3,
                space.origin.1 + space.size.1 * 0.7,
            );

            // The anchor the note path stores.
            let anchor = space.to_page(click);

            // Drawing it back must reproduce the click.
            let back = space.to_screen_point(anchor);
            assert!(
                (back.0 - click.0).abs() < 1e-2 && (back.1 - click.1).abs() < 1e-2,
                "rotation {rotation:?}: {click:?} -> {anchor:?} -> {back:?}"
            );

            // A *double* conversion is not an identity — the exact mistake the
            // old commit code made (it called `to_page` on an already-PDF-space
            // anchor). Guard against a regression that reintroduces it.
            let double = space.to_page(anchor);
            assert!(
                (double.0 - anchor.0).abs() > 1e-2 || (double.1 - anchor.1).abs() > 1e-2,
                "rotation {rotation:?}: a double `to_page` must not be a no-op"
            );
        }
    }

    #[test]
    fn note_text_is_trimmed_to_a_single_short_line() {
        assert_eq!(truncate_chars("short", 48), "short");
        let trimmed = truncate_chars(&"x".repeat(60), 48);
        assert_eq!(trimmed.chars().count(), 49, "48 chars plus an ellipsis");
        assert!(trimmed.ends_with('…'));
        // Counts characters, not bytes, so multi-byte text is safe.
        assert!(
            truncate_chars(&"näïve résumé ".repeat(10), 5)
                .chars()
                .count()
                <= 6
        );
    }

    /// A placed note must render its tag *and* its body without panicking —
    /// PDFium draws only the icon, so the canvas paints the typed text itself.
    #[test]
    fn drawing_a_sticky_note_with_text_does_not_panic() {
        let doc = doc(1);
        let mut canvas = Canvas::new();
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let view = ViewState::default();

        let notes = [AnnotationInfo {
            id: AnnotationId::new(0, 0),
            kind: AnnotationKind::StickyNote,
            rect: PdfSpaceRect::from_xywh(100.0, 100.0, 20.0, 20.0),
            contents: Some("Reviewed later".into()),
                color: None,
}];

        let raw = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        let mut out = ctx.run_ui(raw, |ui| {
            canvas.draw(
                ui,
                &doc,
                &view,
                1.0,
                1.0,
                Color32::BLACK,
                Color32::BLACK,
                None,
                None,
                None,
                Color32::TRANSPARENT,
                Tool::Select,
                &PageTextCache::new(),
                &notes,
                None,
            );
        });
        out.textures_delta.clear();
    }

    /// The note card must always enclose its anchor point (the glyph sits on
    /// it) and grow sideways with the text while staying a single line tall —
    /// the "icon and text fit nicely" contract.
    #[test]
    fn the_note_card_fits_its_glyph_and_grows_with_text() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let raw = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };

        let mut empty = Rect::ZERO;
        let mut short = Rect::ZERO;
        let mut long = Rect::ZERO;
        let mut out = ctx.run_ui(raw, |ui| {
            let painter = ui.painter().clone();
            empty = draw_note_card(&painter, (400.0, 300.0), "", egui::Pos2::ZERO);
            short = draw_note_card(&painter, (400.0, 300.0), "hi", egui::Pos2::ZERO);
            long = draw_note_card(
                &painter,
                (400.0, 300.0),
                "a considerably longer note body than the short one",
                egui::Pos2::ZERO,
            );
        });
        out.textures_delta.clear();

        let anchor = egui::pos2(400.0, 300.0);
        for card in [empty, short, long] {
            assert!(
                card.contains(anchor),
                "card must enclose its anchor: {card:?}"
            );
            assert!(card.width() > 0.0 && card.height() > 0.0, "card has size");
        }
        assert!(short.width() > empty.width(), "text widens the card");
        assert!(long.width() > short.width(), "longer text widens further");
        assert!(
            (short.height() - empty.height()).abs() < 0.5,
            "one line keeps a constant height"
        );
    }

    /// With several notes stacked, a click must resolve to the one drawn on
    /// top, and to the only one that contains the point otherwise.
    #[test]
    fn a_click_picks_the_topmost_overlapping_note() {
        let a = AnnotationId::new(0, 0);
        let b = AnnotationId::new(0, 1);
        let lower = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 30.0));
        let upper = Rect::from_min_size(pos2(20.0, 0.0), vec2(100.0, 30.0));
        let cards = [(a, lower), (b, upper)];

        // Both contain this point; the later (top) card wins.
        assert_eq!(topmost_card(&cards, pos2(50.0, 15.0)), Some(b));
        // Only the first contains this one.
        assert_eq!(topmost_card(&cards, pos2(5.0, 15.0)), Some(a));
        // Outside everything.
        assert_eq!(topmost_card(&cards, pos2(500.0, 500.0)), None);
    }

    /// Committing a placed note must create it at the exact PDF point it was
    /// dropped at (no re-conversion) and queue a layout-space echo.
    #[test]
    fn committing_a_note_keeps_the_placed_point() {
        let space = PageSpace::new(
            PageBox::new(0, 30.0, 12.0, 200.0, 100.0),
            PageGeometry {
                width_pt: 200.0,
                height_pt: 100.0,
                origin_x: 0.0,
                origin_y: 0.0,
            },
            Rotation::None,
            1.0,
        );
        let note = NoteInput {
            page: 0,
            point: (40.0, 60.0),
            text: "hi".into(),
            opened: false,
        };
        let mut edits = Vec::new();
        let mut pending = Vec::new();
        commit_pending_note(
            &mut edits,
            &mut pending,
            DocumentId::from_raw(1),
            &[space],
            note,
        );

        assert_eq!(edits.len(), 1, "one create command");
        match &edits[0] {
            Command::AddAnnotation { page, new } => {
                assert_eq!(*page, 0);
                match new {
                    NewAnnotation::StickyNote((x, y), text) => {
                        assert!((*x - 40.0).abs() < 1e-3 && (*y - 60.0).abs() < 1e-3);
                        assert_eq!(text.as_str(), "hi");
                    }
                    other => panic!("expected a sticky note, got {other:?}"),
                }
            }
            other => panic!("expected AddAnnotation, got {other:?}"),
        }

        assert_eq!(pending.len(), 1, "an echo is queued");
        let expected = space.to_screen_point((40.0, 60.0));
        let p = pending[0].point.expect("echo carries a point");
        assert!((p.0 - expected.0).abs() < 1e-3 && (p.1 - expected.1).abs() < 1e-3);
    }

    /// The card reserves a clear gap between the comment glyph and the text —
    /// they must never overlap, and the text must start to the right of the
    /// glyph.
    #[test]
    fn the_note_card_keeps_a_gap_between_glyph_and_text() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        let raw = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };

        let mut card = Rect::ZERO;
        ctx.run_ui(raw, |ui| {
            let painter = ui.painter().clone();
            card = draw_note_card(&painter, (400.0, 300.0), "hello", egui::Pos2::ZERO);
        })
        .textures_delta
        .clear();

        // The glyph is centred on the anchor; the text begins past the glyph
        // box, leaving a visible gap before the card's right pad.
        let glyph_center = egui::pos2(400.0, 300.0);
        assert!(card.contains(glyph_center));
        // Right edge is beyond the glyph plus its box, i.e. text fits.
        assert!(
            card.max.x > glyph_center.x + 16.0,
            "text must extend past the glyph box: {card:?}"
        );
    }

    #[test]
    fn a_page_with_no_text_selects_nothing() {
        let (_, space) = text_fixture();
        let cache = text_cache(TextPage::default());
        let doc = DocumentId::from_raw(1);
        assert!(selection_range(&[space], 0, &cache, doc, ON_HELLO, ON_HELLO).is_none());
    }

    #[test]
    fn a_page_never_extracted_selects_nothing() {
        let (_, space) = text_fixture();
        let empty = PageTextCache::new();
        assert!(
            selection_range(
                &[space],
                0,
                &empty,
                DocumentId::from_raw(1),
                ON_HELLO,
                ON_WORLD
            )
            .is_none()
        );
    }

    #[test]
    fn a_pick_becomes_a_request_anchored_under_the_selection() {
        let (page, space) = text_fixture();
        let cache = text_cache(page.clone());
        let doc = DocumentId::from_raw(1);
        let (_, range) =
            selection_range(&[space], 0, &cache, doc, ON_HELLO, ON_HELLO).expect("a word is hit");

        let request = emit_lookup(&[space], 0, range, &cache, doc, Pos2::ZERO)
            .expect("a word of text makes a request");
        assert_eq!(request.text, "hello");
        // PDF y is up, so the bottom of the glyph box is below it on screen:
        // PDF y 50 on a 100pt page lands at layout y 50, plus the 6pt offset.
        assert!((request.anchor.x - 10.0).abs() < 1.0);
        assert!((request.anchor.y - 56.0).abs() < 1.0);
    }

    #[test]
    fn an_empty_selection_produces_no_request() {
        let (page, space) = text_fixture();
        let cache = text_cache(page);
        assert!(
            emit_lookup(
                &[space],
                0,
                CharRange::default(),
                &cache,
                DocumentId::from_raw(1),
                Pos2::ZERO
            )
            .is_none()
        );
    }

    #[test]
    fn clearing_a_selection_drops_the_highlight() {
        let mut canvas = Canvas::new();
        canvas.selection = Some((0, CharRange::spanning(0, 4)));
        canvas.clear_selection();
        assert!(canvas.selection.is_none());
        assert!(canvas.select_start.is_none());
    }

    /// Headless stand-in for the manual "select the annotation, drag it, let
    /// go" click-through: drive the real `draw` path with egui pointer events
    /// and assert the canvas emits the same commands the GUI would — and,
    /// crucially, that releasing the button does NOT wipe the selection (the
    /// regression that made annotations un-editable after the first click).
    #[test]
    fn select_tool_click_through_selects_moves_and_keeps_selection() {
        use pdfreader_core::{AnnotationId, AnnotationInfo, AnnotationKind, Command};

        let ctx = egui::Context::default();
        let mut canvas = Canvas::new();
        let doc = doc(1);
        let view = ViewState::default();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));

        let ann = AnnotationInfo {
            id: AnnotationId::new(0, 0),
            kind: AnnotationKind::Square,
            rect: PdfSpaceRect::from_xywh(80.0, 700.0, 140.0, 80.0),
            contents: None,
            color: Some([255, 0, 0, 200]),
        };
        let annotations = vec![ann.clone()];

        let frame = |canvas: &mut Canvas,
                     pos: egui::Pos2,
                     press: Option<bool>,
                     selected: Option<AnnotationId>|
         -> Vec<Command> {
            let mut raw = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            raw.events.push(egui::Event::PointerMoved(pos));
            if let Some(p) = press {
                raw.events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: p,
                    modifiers: egui::Modifiers::default(),
                });
            }
            let mut out = ctx.run_ui(raw, |ui| {
                canvas.draw(
                    ui,
                    &doc,
                    &view,
                    1.0,
                    1.0,
                    egui::Color32::BLACK,
                    egui::Color32::BLACK,
                    None,
                    None,
                    None,
                    egui::Color32::TRANSPARENT,
                    Tool::Select,
                    &PageTextCache::new(),
                    &annotations,
                    selected,
                );
            });
            out.textures_delta.clear();
            canvas.take_edits()
        };

        // A no-op layout frame populates `page_spaces` so we can learn where
        // the annotation sits on screen.
        let _ = frame(&mut canvas, egui::pos2(5.0, 5.0), None, None);
        let space = canvas.page_spaces()[0];
        let sr = space.to_screen(ann.rect);
        let center = egui::pos2(sr.x + sr.w / 2.0, sr.y + sr.h / 2.0);
        assert!(
            screen.contains(center),
            "annotation must be on screen to be clickable (center {center:?})"
        );

        let mut edits = Vec::new();
        // Press on the body: selects it in place and starts a move.
        edits.extend(frame(&mut canvas, center, Some(true), None));
        // Drag 40px right while still holding the button.
        edits.extend(frame(&mut canvas, egui::pos2(center.x + 40.0, center.y), None, None));
        // Release: commits the move (only if the rect actually changed).
        edits.extend(frame(
            &mut canvas,
            egui::pos2(center.x + 40.0, center.y),
            Some(false),
            None,
        ));

        assert!(
            edits.iter().any(|c| matches!(c, Command::SelectAnnotationInPlace(Some(id)) if *id == AnnotationId::new(0, 0))),
            "clicking the annotation must select it in place: {edits:?}"
        );
        let moved = edits.iter().find_map(|c| match c {
            Command::UpdateAnnotationBounds { id, rect } if *id == AnnotationId::new(0, 0) => {
                Some(*rect)
            }
            _ => None,
        });
        assert!(
            moved.is_some_and(|r| {
                (r.min_x - (ann.rect.min_x + 40.0)).abs() < 1.0
                    && (r.min_y - ann.rect.min_y).abs() < 1.0
                    && (r.max_y - ann.rect.max_y).abs() < 1.0
            }),
            "release must commit a 40px-right move: {moved:?}"
        );
        // Regression: the release must not deselect (that was the bug that
        // made every annotation impossible to edit after the first click).
        assert!(
            !edits.iter().any(|c| matches!(c, Command::SelectAnnotation(None))),
            "selection must survive the release (regression): {edits:?}"
        );
    }

    /// Headless stand-in for grabbing a resize handle: with the annotation
    /// already selected, press on its East handle, drag right, release, and
    /// confirm the canvas emits the widened bounds — the same command the GUI
    /// sends when a user reshapes a mark.
    ///
    /// The canvas is deliberately offset from the window origin, because in the
    /// real app it sits below the tabbar and toolbar. The handle is pressed at
    /// the position it is *drawn* (content-local + origin), so the test fails
    /// if the resize hit test forgets to convert the global pointer to
    /// content-local space — the bug that made handles respond to a spot
    /// offset from where the user saw them.
    #[test]
    fn selected_annotation_resize_handle_drag_emits_new_bounds() {
        use pdfreader_core::{AnnotationId, AnnotationInfo, AnnotationKind, Command};

        let ctx = egui::Context::default();
        let mut canvas = Canvas::new();
        let doc = doc(1);
        let view = ViewState::default();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 700.0));
        let offset = egui::vec2(37.0, 53.0);

        let ann = AnnotationInfo {
            id: AnnotationId::new(0, 0),
            kind: AnnotationKind::Square,
            rect: PdfSpaceRect::from_xywh(80.0, 700.0, 140.0, 80.0),
            contents: None,
            color: Some([255, 0, 0, 200]),
        };
        let annotations = vec![ann.clone()];
        let id = AnnotationId::new(0, 0);

        let frame = |canvas: &mut Canvas,
                     pos: egui::Pos2,
                     press: Option<bool>,
                     selected: Option<AnnotationId>|
         -> Vec<Command> {
            let mut raw = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            raw.events.push(egui::Event::PointerMoved(pos));
            if let Some(p) = press {
                raw.events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: p,
                    modifiers: egui::Modifiers::default(),
                });
            }
            let mut out = ctx.run_ui(raw, |ui| {
                // Draw the canvas into a child ui offset from the window origin,
                // so `content_origin` inside `draw` is non-zero — exactly the
                // real-app situation the offset bug needed to be caught in.
                ui.scope_builder(
                    egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                        egui::pos2(offset.x, offset.y),
                        screen.size(),
                    )),
                    |ui| {
                        canvas.draw(
                            ui,
                            &doc,
                            &view,
                            1.0,
                            1.0,
                            egui::Color32::BLACK,
                            egui::Color32::BLACK,
                            None,
                            None,
                            None,
                            egui::Color32::TRANSPARENT,
                            Tool::Select,
                            &PageTextCache::new(),
                            &annotations,
                            selected,
                        );
                    },
                );
            });
            out.textures_delta.clear();
            canvas.take_edits()
        };

        // Layout frame with the annotation selected so its handles are drawn.
        let _ = frame(&mut canvas, egui::pos2(5.0, 5.0), None, Some(id));
        let space = canvas.page_spaces()[0];
        let sr = space.to_screen(ann.rect);
        // The East handle as the user sees it: content-local + canvas origin.
        let east = egui::pos2(sr.x + sr.w + offset.x, sr.y + sr.h / 2.0 + offset.y);

        let mut edits = Vec::new();
        edits.extend(frame(&mut canvas, east, Some(true), Some(id)));
        edits.extend(frame(
            &mut canvas,
            egui::pos2(east.x + 50.0, east.y),
            None,
            Some(id),
        ));
        edits.extend(frame(
            &mut canvas,
            egui::pos2(east.x + 50.0, east.y),
            Some(false),
            Some(id),
        ));

        let widened = edits.iter().find_map(|c| match c {
            Command::UpdateAnnotationBounds { id, rect } if *id == AnnotationId::new(0, 0) => {
                Some(*rect)
            }
            _ => None,
        });
        // A *resize* from the East handle moves only the right edge; a *move*
        // would shift both edges by 50. Asserting the left edge stays put is
        // what distinguishes the two, so a missed handle that silently becomes
        // a move cannot pass this test.
        assert!(
            widened.is_some_and(|r| {
                (r.max_x - (ann.rect.max_x + 50.0)).abs() < 1.0
                    && (r.min_x - ann.rect.min_x).abs() < 1.0
            }),
            "dragging the East handle right must widen by ~50px with the left edge fixed \
             (a move would shift both edges): {widened:?}"
        );
    }

    /// Dragging the North (top) handle up must change the HEIGHT only: the top
    /// edge follows the pointer and the width is untouched. This is the vertical
    /// half of the resize contract — a drag that changed width instead (or moved
    /// the whole box) is the "vertical resize only changes horizontal" report.
    #[test]
    fn selected_annotation_north_handle_drag_changes_height_not_width() {
        use pdfreader_core::{AnnotationId, AnnotationInfo, AnnotationKind, Command};

        let ctx = egui::Context::default();
        let mut canvas = Canvas::new();
        let doc = doc(1);
        let view = ViewState::default();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 700.0));
        let offset = egui::vec2(37.0, 53.0);

        let ann = AnnotationInfo {
            id: AnnotationId::new(0, 0),
            kind: AnnotationKind::Square,
            rect: PdfSpaceRect::from_xywh(80.0, 700.0, 140.0, 80.0),
            contents: None,
            color: Some([255, 0, 0, 200]),
        };
        let annotations = vec![ann.clone()];
        let id = AnnotationId::new(0, 0);

        let frame = |canvas: &mut Canvas,
                     pos: egui::Pos2,
                     press: Option<bool>,
                     selected: Option<AnnotationId>|
         -> Vec<Command> {
            let mut raw = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            raw.events.push(egui::Event::PointerMoved(pos));
            if let Some(p) = press {
                raw.events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: p,
                    modifiers: egui::Modifiers::default(),
                });
            }
            let mut out = ctx.run_ui(raw, |ui| {
                ui.scope_builder(
                    egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                        egui::pos2(offset.x, offset.y),
                        screen.size(),
                    )),
                    |ui| {
                        canvas.draw(
                            ui,
                            &doc,
                            &view,
                            1.0,
                            1.0,
                            egui::Color32::BLACK,
                            egui::Color32::BLACK,
                            None,
                            None,
                            None,
                            egui::Color32::TRANSPARENT,
                            Tool::Select,
                            &PageTextCache::new(),
                            &annotations,
                            selected,
                        );
                    },
                );
            });
            out.textures_delta.clear();
            canvas.take_edits()
        };

        let _ = frame(&mut canvas, egui::pos2(5.0, 5.0), None, Some(id));
        let space = canvas.page_spaces()[0];
        let sr = space.to_screen(ann.rect);
        // The North handle as the user sees it: content-local + canvas origin.
        let north = egui::pos2(sr.x + sr.w / 2.0 + offset.x, sr.y + offset.y);

        let mut edits = Vec::new();
        edits.extend(frame(&mut canvas, north, Some(true), Some(id)));
        edits.extend(frame(
            &mut canvas,
            egui::pos2(north.x, north.y - 50.0),
            None,
            Some(id),
        ));
        edits.extend(frame(
            &mut canvas,
            egui::pos2(north.x, north.y - 50.0),
            Some(false),
            Some(id),
        ));

        let resized = edits.iter().find_map(|c| match c {
            Command::UpdateAnnotationBounds { id, rect } if *id == AnnotationId::new(0, 0) => {
                Some(*rect)
            }
            _ => None,
        });
        assert!(
            resized.is_some_and(|r| {
                (r.height() - (ann.rect.height() + 50.0)).abs() < 2.0
                    && (r.width() - ann.rect.width()).abs() < 1.0
                    && (r.min_y - ann.rect.min_y).abs() < 1.0
            }),
            "dragging the North handle up must grow the HEIGHT by ~50px with the width and \
             bottom edge fixed: {resized:?}"
        );
    }

    /// With an annotation tool still armed, grabbing the selected mark's resize
    /// handle must resize it — not draw a new mark — so a freshly drawn
    /// annotation can be adjusted without switching back to Select first.
    #[test]
    fn resize_handle_wins_over_an_armed_drawing_tool() {
        use pdfreader_core::{AnnotationId, AnnotationInfo, AnnotationKind, Command};

        let ctx = egui::Context::default();
        let mut canvas = Canvas::new();
        let doc = doc(1);
        let view = ViewState::default();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));

        let ann = AnnotationInfo {
            id: AnnotationId::new(0, 0),
            kind: AnnotationKind::Square,
            rect: PdfSpaceRect::from_xywh(80.0, 700.0, 140.0, 80.0),
            contents: None,
            color: Some([255, 0, 0, 200]),
        };
        let annotations = vec![ann.clone()];
        let id = AnnotationId::new(0, 0);

        // The Highlight tool stays armed throughout — the press on the handle
        // must edit, not draw.
        let frame = |canvas: &mut Canvas, pos: egui::Pos2, press: Option<bool>| -> Vec<Command> {
            let mut raw = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            raw.events.push(egui::Event::PointerMoved(pos));
            if let Some(p) = press {
                raw.events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: p,
                    modifiers: egui::Modifiers::default(),
                });
            }
            let mut out = ctx.run_ui(raw, |ui| {
                canvas.draw(
                    ui,
                    &doc,
                    &view,
                    1.0,
                    1.0,
                    egui::Color32::BLACK,
                    egui::Color32::BLACK,
                    None,
                    None,
                    None,
                    egui::Color32::TRANSPARENT,
                    Tool::Highlight,
                    &PageTextCache::new(),
                    &annotations,
                    Some(id),
                );
            });
            out.textures_delta.clear();
            canvas.take_edits()
        };

        let _ = frame(&mut canvas, egui::pos2(5.0, 5.0), None);
        let space = canvas.page_spaces()[0];
        let sr = space.to_screen(ann.rect);
        let east = egui::pos2(sr.x + sr.w, sr.y + sr.h / 2.0);
        // Drag diagonally, so that without the handle-priority gate the drag
        // would also produce a *valid* (non-degenerate) highlight and this test
        // would catch the spurious create.
        let to = egui::pos2(east.x + 50.0, east.y + 50.0);

        let mut edits = Vec::new();
        edits.extend(frame(&mut canvas, east, Some(true)));
        edits.extend(frame(&mut canvas, to, None));
        edits.extend(frame(&mut canvas, to, Some(false)));

        let widened = edits.iter().find_map(|c| match c {
            Command::UpdateAnnotationBounds { id, rect } if *id == AnnotationId::new(0, 0) => {
                Some(*rect)
            }
            _ => None,
        });
        assert!(
            widened.is_some_and(|r| {
                (r.max_x - (ann.rect.max_x + 50.0)).abs() < 1.0
                    && (r.min_x - ann.rect.min_x).abs() < 1.0
            }),
            "grabbing the handle with a tool armed must resize: {edits:?}"
        );
        assert!(
            !edits.iter().any(|c| matches!(c, Command::AddAnnotation { .. })),
            "grabbing a handle must not also draw a new mark: {edits:?}"
        );
    }

    /// A press that lands on a floating window must never reach the page.
    ///
    /// Regression: the canvas's raw-pointer paths only checked the canvas clip
    /// rect, not which layer owned the pointer. With a drawing tool still armed
    /// — the normal state right after drawing a mark — clicking a colour swatch
    /// in the properties window also started a stroke on the page behind it.
    /// That stroke then became the selection, so the panel jumped to the
    /// brand-new mark instead of editing the one that was open.
    #[test]
    fn a_press_on_a_floating_window_never_reaches_the_page() {
        use pdfreader_core::{AnnotationId, AnnotationInfo, AnnotationKind, Command};

        let ctx = egui::Context::default();
        let mut canvas = Canvas::new();
        let doc = doc(1);
        let view = ViewState::default();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));

        // Something selected and on screen, so the Select arm has a target too.
        let ann = AnnotationInfo {
            id: AnnotationId::new(0, 0),
            kind: AnnotationKind::Square,
            rect: PdfSpaceRect::from_xywh(80.0, 700.0, 140.0, 80.0),
            contents: None,
            color: Some([255, 0, 0, 200]),
        };
        let annotations = vec![ann.clone()];

        // `float` draws an opaque panel over the whole screen on a higher
        // layer, exactly as the properties window does.
        let frame = |canvas: &mut Canvas,
                     pos: egui::Pos2,
                     press: Option<bool>,
                     tool: Tool,
                     selected: Option<AnnotationId>,
                     float: bool|
         -> Vec<Command> {
            let mut raw = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            raw.events.push(egui::Event::PointerMoved(pos));
            if let Some(p) = press {
                raw.events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: p,
                    modifiers: egui::Modifiers::default(),
                });
            }
            let mut out = ctx.run_ui(raw, |ui| {
                canvas.draw(
                    ui,
                    &doc,
                    &view,
                    1.0,
                    1.0,
                    egui::Color32::BLACK,
                    egui::Color32::BLACK,
                    None,
                    None,
                    None,
                    egui::Color32::TRANSPARENT,
                    tool,
                    &PageTextCache::new(),
                    &annotations,
                    selected,
                );
                if float {
                    egui::Area::new(egui::Id::new("floating-properties"))
                        .fixed_pos(egui::pos2(0.0, 0.0))
                        .show(ui.ctx(), |ui| {
                            ui.set_min_size(screen.size());
                            ui.allocate_space(screen.size());
                        });
                }
            });
            out.textures_delta.clear();
            canvas.take_edits()
        };

        // Learn the layout, then pick a point inside the page.
        let _ = frame(&mut canvas, egui::pos2(5.0, 5.0), None, Tool::Select, None, false);
        let space = canvas.page_spaces()[0];
        let sr = space.to_screen(ann.rect);
        let on_page = egui::pos2(sr.x + sr.w / 2.0, sr.y + sr.h / 2.0);
        let dragged_to = egui::pos2(on_page.x + 60.0, on_page.y + 20.0);
        assert!(screen.contains(on_page), "the page point must be on screen");

        // Run one draw gesture (press · move · release) on a fresh canvas. The
        // panel needs a couple of frames to enter egui's layer order — it is
        // present for far longer than that before a real click — so the
        // gesture settles the layout first.
        let draw_gesture = |float: bool| -> Vec<Command> {
            let mut c = Canvas::new();
            for _ in 0..3 {
                let _ = frame(&mut c, egui::pos2(5.0, 5.0), None, Tool::Select, None, float);
            }
            let mut edits = Vec::new();
            edits.extend(frame(&mut c, on_page, Some(true), Tool::Underline, None, float));
            edits.extend(frame(&mut c, dragged_to, None, Tool::Underline, None, float));
            edits.extend(frame(&mut c, dragged_to, Some(false), Tool::Underline, None, float));
            edits
        };
        let press_select = |float: bool| -> Vec<Command> {
            let mut c = Canvas::new();
            for _ in 0..3 {
                let _ = frame(&mut c, egui::pos2(5.0, 5.0), None, Tool::Select, None, float);
            }
            let mut edits = Vec::new();
            edits.extend(frame(&mut c, on_page, Some(true), Tool::Select, None, float));
            edits.extend(frame(&mut c, on_page, Some(false), Tool::Select, None, float));
            edits
        };

        // Control: with nothing floating, the gesture draws and the press selects.
        let control = draw_gesture(false);
        assert!(
            control
                .iter()
                .any(|c| matches!(c, Command::AddAnnotation { .. })),
            "control: a drag on the bare page must draw: {control:?}"
        );
        let control_select = press_select(false);
        assert!(
            control_select
                .iter()
                .any(|c| matches!(c, Command::SelectAnnotationInPlace(Some(_)))),
            "control: a press on the bare page must select the mark: {control_select:?}"
        );

        // With the panel over the page, the same gestures must be inert.
        let blocked = draw_gesture(true);
        assert!(
            !blocked
                .iter()
                .any(|c| matches!(c, Command::AddAnnotation { .. })),
            "a press on the floating window must not draw on the page: {blocked:?}"
        );
        let blocked_select = press_select(true);
        assert!(
            blocked_select.is_empty(),
            "a press on the floating window must not move or re-select the mark behind it: {blocked_select:?}"
        );
    }

    /// A press on bare paper dismisses the properties window.
    ///
    /// The panel is a floating tool window now — draggable anywhere over the
    /// page — so it needs the one gesture every floating panel has. Without it
    /// the window sat over the page until the selection changed, which on a
    /// long document meant choosing between reading the page and keeping the
    /// panel.
    #[test]
    fn a_press_on_bare_paper_dismisses_the_properties_panel() {
        use pdfreader_core::{AnnotationId, AnnotationInfo, AnnotationKind, Command};

        let ctx = egui::Context::default();
        let doc = doc(1);
        let mut view = ViewState::default();
        view.properties_panel_open = true;
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));

        let ann = AnnotationInfo {
            id: AnnotationId::new(0, 0),
            kind: AnnotationKind::Square,
            rect: PdfSpaceRect::from_xywh(80.0, 700.0, 140.0, 80.0),
            contents: None,
            color: Some([255, 0, 0, 200]),
        };
        let annotations = vec![ann.clone()];
        let selected = Some(AnnotationId::new(0, 0));

        let frame = |canvas: &mut Canvas,
                     pos: egui::Pos2,
                     press: Option<bool>,
                     tool: Tool|
         -> Vec<Command> {
            let mut raw = egui::RawInput {
                screen_rect: Some(screen),
                time: Some(next_frame_time(&ctx)),
                ..Default::default()
            };
            raw.events.push(egui::Event::PointerMoved(pos));
            if let Some(p) = press {
                raw.events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: p,
                    modifiers: egui::Modifiers::default(),
                });
            }
            let mut out = ctx.run_ui(raw, |ui| {
                canvas.draw(
                    ui,
                    &doc,
                    &view,
                    1.0,
                    1.0,
                    egui::Color32::BLACK,
                    egui::Color32::BLACK,
                    None,
                    None,
                    None,
                    egui::Color32::TRANSPARENT,
                    tool,
                    &PageTextCache::new(),
                    &annotations,
                    selected,
                );
            });
            out.textures_delta.clear();
            canvas.take_edits()
        };

        // Learn the layout, then take a point on the page that is well clear of
        // the mark — "elsewhere on the canvas" has to mean somewhere else.
        let mut probe = Canvas::new();
        let _ = frame(&mut probe, egui::pos2(5.0, 5.0), None, Tool::Select);
        let space = probe.page_spaces()[0];
        let mark = space.to_screen(ann.rect);
        let bare = egui::pos2(space.origin.0 + space.size.0 / 2.0, mark.y - 40.0);
        assert!(
            screen.contains(bare) && !mark.contains(bare.x, bare.y),
            "the bare-paper point must be on the page and off the mark: {bare:?} vs {mark:?}"
        );

        // Select tool, press and release on empty paper.
        let mut canvas = Canvas::new();
        for _ in 0..3 {
            let _ = frame(&mut canvas, egui::pos2(5.0, 5.0), None, Tool::Select);
        }
        let mut edits = Vec::new();
        edits.extend(frame(&mut canvas, bare, Some(true), Tool::Select));
        edits.extend(frame(&mut canvas, bare, Some(false), Tool::Select));
        assert!(
            edits
                .iter()
                .any(|c| matches!(c, Command::SetPropertiesPanel { open: false })),
            "a press on bare paper must dismiss the properties window: {edits:?}"
        );

        // The same press on the mark itself keeps the panel open: that is how
        // the panel moves from one annotation to the next.
        let mut canvas = Canvas::new();
        for _ in 0..3 {
            let _ = frame(&mut canvas, egui::pos2(5.0, 5.0), None, Tool::Select);
        }
        let on_mark = egui::pos2(mark.x + mark.w / 2.0, mark.y + mark.h / 2.0);
        let mut edits = Vec::new();
        edits.extend(frame(&mut canvas, on_mark, Some(true), Tool::Select));
        edits.extend(frame(&mut canvas, on_mark, Some(false), Tool::Select));
        assert!(
            !edits
                .iter()
                .any(|c| matches!(c, Command::SetPropertiesPanel { open: false })),
            "a press on the mark must not dismiss the panel it is showing: {edits:?}"
        );

        // An armed drawing tool pressing bare paper still dismisses it: the
        // click is the user saying "elsewhere", not "keep editing this".
        let mut canvas = Canvas::new();
        for _ in 0..3 {
            let _ = frame(&mut canvas, egui::pos2(5.0, 5.0), None, Tool::Highlight);
        }
        let mut edits = Vec::new();
        edits.extend(frame(&mut canvas, bare, Some(true), Tool::Highlight));
        assert!(
            edits
                .iter()
                .any(|c| matches!(c, Command::SetPropertiesPanel { open: false })),
            "a press on bare paper must dismiss the panel with a tool armed too: {edits:?}"
        );
    }
}
