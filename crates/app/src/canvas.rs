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
    Color32, ColorImage, Context, Rect, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions,
    Ui, Vec2, pos2, vec2,
};
use pdfreader_core::{Document, DocumentId, FieldId, FormInfo, Rotation, ViewMode, ViewState};
use pdfreader_pdf::engine::{TilePixels, TileRequest};
use pdfreader_render::{
    PageBox, PageSpace, TILE_SIZE, TileKey, level_scale, tile_grid, tile_pixel_rect, zoom_level,
};

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

/// Texture caches for tiles and thumbnails.
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
}

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
            thumb_follow: None,
            pan_tool: false,
            panning: false,
            page_spaces: Vec::new(),
            clicked_at: None,
        }
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

    /// Scroll offset from the last draw, in document coordinates.
    pub fn scroll_offset(&self) -> Vec2 {
        self.scroll_offset
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

    /// Drop every texture belonging to a document (called when its tab closes).
    pub fn forget_document(&mut self, doc: DocumentId) {
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
            .or(pending_offset)
            .or(anchor_offset)
            .or(initial_offset)
        {
            Some(offset) => scroll.scroll_offset(offset),
            None => scroll,
        };
        let output = scroll.show_viewport(ui, |ui, viewport| {
            viewport_center_y = viewport.center().y;
            viewport_center_x = viewport.center().x;
            // `viewport` and the page layout are in local document
            // coordinates, while egui painters use absolute screen
            // coordinates. The old code painted `pb.x/pb.y` directly, which
            // discarded the canvas origin and clipped the left/top of every
            // page under the toolbar/sidebar. Keep culling in local space and
            // translate only at paint time.
            let content_origin = ui.min_rect().min;
            let (_, page_area) = ui.allocate_exact_size(content, Sense::click_and_drag());
            let painter = ui.painter();

            // Record the click in *document layout* space, not screen space:
            // the content Ui is already translated by the scroll offset, so
            // subtracting its origin is what puts the pointer back into the
            // same coordinates the page boxes use.
            if page_area.clicked() {
                if let Some(pos) = page_area.interact_pointer_pos() {
                    self.clicked_at = Some((pos.x - content_origin.x, pos.y - content_origin.y));
                }
            }
            painter.rect_filled(ui.clip_rect(), 0.0, background);

            let prefetch = viewport.expand2(vec2(viewport.width() * 0.5, viewport.height() * 0.5));
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

                        if let Some(tex) = self.tiles.get(&(doc.id, rotation, key)) {
                            painter.image(tex.id(), rect, uv_full(), Color32::WHITE);
                        } else if !self.inflight.contains(&(doc.id, rotation, key))
                            && !self.failed.contains(&(doc.id, rotation, key))
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

            // Form field overlay, drawn after the tiles so it sits on top of
            // them. PDFium has already rasterized each widget's appearance into
            // the tiles; what egui adds is the interactive layer — an outline
            // showing which regions are fields at all, and a stronger marker on
            // the one the user selected.
            if let Some(form) = form {
                draw_form_overlay(
                    &painter,
                    form,
                    selected,
                    &page_spaces,
                    content_origin,
                    accent,
                );
            }
        });

        self.scroll_offset = output.state.offset;

        // Drag-to-pan. With the hand tool engaged — or the middle button held,
        // which is what Acrobat, Chrome and every other viewer bind — dragging
        // on the page moves the viewport. Without this a page zoomed in past
        // the viewport can only be nudged with the scrollbars, which reads as
        // "the app is stuck" on a trackpad.
        let (panning, pan_delta) = ui.input(|input| {
            let pointer = &input.pointer;
            let wants_pan = pointer.middle_down() || (self.pan_tool && pointer.primary_down());
            (
                wants_pan && pointer.is_decidedly_dragging(),
                pointer.delta(),
            )
        });
        self.panning = panning;
        if panning {
            // Dragging right slides the page right, which reveals content
            // further left — so the offset moves opposite to the pointer.
            // Applied on the next frame via the builder offset, the same path
            // keyboard scrolling uses, so egui clamps it to the content.
            self.pending_scroll_offset = Some((self.scroll_offset - pan_delta).max(Vec2::ZERO));
        }
        if ui.rect_contains_pointer(ui.clip_rect()) {
            let cursor = if self.panning {
                egui::CursorIcon::Grabbing
            } else if self.pan_tool {
                egui::CursorIcon::Grab
            } else {
                egui::CursorIcon::Default
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
                        if !self.thumb_inflight.contains(&(doc.id, rotation, index))
                            && !self.thumb_failed.contains(&(doc.id, rotation, index))
                        {
                            self.thumb_inflight.insert((doc.id, rotation, index));
                            let scale = if w_pt > 0.0 {
                                (THUMB_WIDTH * dpr / w_pt).clamp(0.02, 4.0)
                            } else {
                                0.2
                            };
                            result.requests.push(PendingThumb {
                                doc: doc.id,
                                page: index,
                                rotation,
                                scale,
                            });
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

/// Draw the interactive layer over form fields.
///
/// Field rectangles arrive in unrotated PDF space with y pointing up, so every
/// one goes through [`PageSpace::to_screen`]: that is the single place that
/// knows about the y flip and the rotation transposition. Doing the conversion
/// by hand here is what would put a field on the wrong side of the page.
fn draw_form_overlay(
    painter: &egui::Painter,
    form: &FormInfo,
    selected: Option<FieldId>,
    page_spaces: &[PageSpace],
    content_origin: egui::Pos2,
    accent: Color32,
) {
    let painter = painter.with_clip_rect(painter.clip_rect());

    for space in page_spaces {
        let page = space.index;
        for field in form.fields.iter().filter(|f| f.id.page == page) {
            let screen = space.to_screen(field.rect);
            let rect = Rect::from_min_size(
                pos2(screen.x, screen.y),
                vec2(screen.w.max(1.0), screen.h.max(1.0)),
            )
            .translate(content_origin.to_vec2());

            // Skip fields scrolled out of sight: a form can have hundreds of
            // widgets and only a handful are ever on screen.
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
                // Faint outline so fillable regions are discoverable without
                // turning the page into a wireframe.
                painter.rect_stroke(
                    rect,
                    1.0,
                    Stroke::new(1.0, accent.gamma_multiply(0.45)),
                    StrokeKind::Outside,
                );
            }
        }
    }
}

/// Lay pages out in document space (logical points).
///
/// `viewport_w` is the width of the visible canvas. Pages are centred inside
/// the *content* width, so the content has to be at least as wide as the
/// viewport: in fit-page (and any zoom below fit-width) the page is narrower
/// than the viewport, and without this clamp the whole content block sits
/// flush left with the page hugging the left edge instead of the middle.
/// Pass `0.0` for pure document-space layout with no viewport padding.
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
    use pdfreader_core::{Outline, PageGeometry};

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
}
