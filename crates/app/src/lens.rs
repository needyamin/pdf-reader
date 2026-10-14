//! Word lookup inside the document.
//!
//! Three pieces live here:
//!
//! * [`PageTextCache`] — the page text the canvas needs to turn a click into a
//!   character, filled by the engine thread and kept for the session.
//! * [`LookupRequest`] — what the canvas hands the shell when a word or phrase
//!   is picked.
//! * [`LensPopup`] — the floating card that shows the answer.
//!
//! The lookup itself is answered by `pdfreader_lens`, offline, from the bundled
//! dictionaries. This module only carries the result to the screen.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crossbeam_channel::Receiver;
use egui::{Color32, Id, Margin, Pos2, RichText};
use pdfreader_core::DocumentId;
use pdfreader_lens::{Dictionaries, Lookup, LookupError, translate_link};
use pdfreader_pdf::TextPage;
use pdfreader_platform::Speaker;
use pdfreader_ui::Palette;

/// Width of the popup card, in logical points.
const POPUP_WIDTH: f32 = 380.0;

/// Tallest the scrolling body is allowed to grow. A long translation scrolls
/// inside this; a short one shrinks the card instead of leaving an empty well.
const BODY_MAX_HEIGHT: f32 = 480.0;

/// Header row. Speak and the close mark are placed in this strip, sharing
/// its vertical centre, so neither sits above the title or past the card edge.
const HEADER_HEIGHT: f32 = 36.0;

/// Square click target for the close mark.
const CLOSE_SIDE: f32 = 28.0;

/// Width reserved for the speaker icon plus the Speak label.
const SPEAK_WIDTH: f32 = 88.0;

/// Gap between the title, Speak, and the close mark.
const HEADER_GAP: f32 = 8.0;

/// Text extracted from pages, kept so a click can be hit-tested to a character.
///
/// Only pages that have been on screen are held; a long document does not fill
/// memory with text nobody looked at.
#[derive(Default)]
pub struct PageTextCache {
    /// Text geometry by document and page.
    pages: HashMap<(DocumentId, u32), TextPage>,
    /// Requests sent to the engine but not answered yet.
    inflight: HashSet<(DocumentId, u32)>,
}

impl PageTextCache {
    /// An empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// The text of a page, when it has been extracted.
    pub fn get(&self, doc: DocumentId, page: u32) -> Option<&TextPage> {
        self.pages.get(&(doc, page))
    }

    /// Mark a page as wanted.
    ///
    /// Returns `true` when the caller should send a request to the engine,
    /// which is only the case the first time a page is asked for — that is what
    /// stops a frame from re-requesting text on every redraw.
    pub fn request(&mut self, doc: DocumentId, page: u32) -> bool {
        if self.pages.contains_key(&(doc, page)) {
            return false;
        }
        self.inflight.insert((doc, page))
    }

    /// Store extracted text, clearing its in-flight marker.
    pub fn insert(&mut self, doc: DocumentId, page: u32, text: TextPage) {
        self.inflight.remove(&(doc, page));
        self.pages.insert((doc, page), text);
    }

    /// Record that extraction failed, so the page is not waited for again.
    ///
    /// The page stays absent, which the canvas reads as "no selectable text" —
    /// the honest answer for a page PDFium could not read.
    pub fn failed(&mut self, doc: DocumentId, page: u32) {
        self.inflight.remove(&(doc, page));
        self.pages.insert((doc, page), TextPage::default());
    }

    /// Forget a closed document's text.
    pub fn drop_document(&mut self, doc: DocumentId) {
        self.pages.retain(|(id, _), _| *id != doc);
        self.inflight.retain(|(id, _)| *id != doc);
    }
}

/// A word or phrase the user picked on the page.
#[derive(Clone, PartialEq, Debug)]
pub struct LookupRequest {
    /// Zero-based page the text came from.
    pub page: u32,
    /// The selected text, as it appears on the page.
    pub text: String,
    /// Where to anchor the popup, in screen coordinates.
    ///
    /// Captured at pick time so the card stays next to the word it answers
    /// rather than following the pointer around afterwards.
    pub anchor: Pos2,
}

/// The answer a lookup produced, or why it produced none.
#[derive(Clone, PartialEq, Debug)]
pub enum Answer {
    /// The dictionaries are still loading on the background thread.
    Loading,
    /// A meaning, pronunciation and part-of-speech groups.
    Ready(Lookup),
    /// Nothing was found, or the dictionaries could not be read at all.
    Missing(String),
}

impl Answer {
    /// Turn a lookup result into an answer.
    pub fn from_lookup(result: Result<Lookup, LookupError>) -> Self {
        match result {
            Ok(lookup) => Self::Ready(lookup),
            Err(error) => Self::Missing(error.to_string()),
        }
    }
}

/// The floating card that shows a lookup.
///
/// Drawn as a top-level `Area` so it is never clipped by the canvas or the
/// scroll area — a popup that gets cut off at the edge of the viewport is worse
/// than no popup at all.
pub struct LensPopup {
    /// Whether the card is on screen.
    open: bool,
    /// The text that was selected.
    text: String,
    /// The answer to show.
    answer: Answer,
    /// Where the card is anchored.
    anchor: Pos2,
    /// Owns the system voice, so starting a new word stops the old one.
    speaker: Speaker,
}

impl Default for LensPopup {
    fn default() -> Self {
        Self::new()
    }
}

impl LensPopup {
    /// A closed popup.
    pub fn new() -> Self {
        Self {
            open: false,
            text: String::new(),
            answer: Answer::Loading,
            anchor: Pos2::ZERO,
            speaker: Speaker::new(),
        }
    }

    /// Show the card for a selection.
    ///
    /// When `auto_speak` is set (Settings → Language tools → "Speak on
    /// selection"), the text is read aloud the moment the card opens, so a
    /// learner hears the word without a second click. The language comes from
    /// the detected script, exactly as the Speak button uses it.
    pub fn open(&mut self, request: LookupRequest, answer: Answer, auto_speak: bool) {
        self.open = true;
        self.text = request.text;
        self.answer = answer;
        self.anchor = request.anchor;

        if auto_speak {
            let lang = match &self.answer {
                Answer::Ready(lookup) => lookup.detected.code(),
                _ => "en",
            };
            if let Err(error) = self.speaker.speak(&self.text, lang) {
                tracing::warn!("could not auto-speak the selection ({error})");
            }
        }
    }

    /// Take the card off screen and stop anything being spoken.
    pub fn close(&mut self) {
        self.open = false;
        self.speaker.stop();
    }

    /// Draw the card, if it is open.
    ///
    /// `target` is the language code the Google Translate button links to. It
    /// does not change the offline answer, which is always English→Bangla.
    /// `speak_enabled` and `translate_enabled` come from the settings menu and
    /// hide the matching button/footer when the user has turned a tool off.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        palette: &Palette,
        target: &str,
        speak_enabled: bool,
        translate_enabled: bool,
    ) -> Option<egui::Rect> {
        if !self.open {
            return None;
        }

        let mut close = false;
        let mut speak: Option<(String, &'static str)> = None;
        let mut stop_speech = false;
        let mut translate = None;
        // Polled every frame the card is open. While a voice is running the
        // icon is the sounding speaker, and a repaint is booked so it falls
        // back to the quiet speaker as soon as the utterance ends.
        let speaking = self.speaker.is_speaking();
        if speaking {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }

        let area = egui::Area::new(Id::new("lens-popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(self.anchor)
            .constrain(true)
            .interactable(true)
            .show(ctx, |ui| {
                pdfreader_ui::window_frame(palette)
                    .inner_margin(Margin::same(0))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 4],
                        blur: 16,
                        spread: 0,
                        color: Color32::from_black_alpha(46),
                    })
                    .show(ui, |ui| {
                        ui.set_min_width(POPUP_WIDTH);
                        ui.set_max_width(POPUP_WIDTH);

                        // --- header: language on the left, Speak and the
                        // close mark on the right. Both controls are placed
                        // from the row's right edge so they share one centre
                        // line and stay inside the card.
                        egui::Frame::new()
                            .inner_margin(Margin::symmetric(12, 6))
                            .show(ui, |ui| {
                                let row_width = ui.available_width();
                                let (_, row_response) = ui.allocate_exact_size(
                                    egui::vec2(row_width, HEADER_HEIGHT),
                                    egui::Sense::empty(),
                                );
                                let slots = header_slots(row_response.rect, speak_enabled);
                                let title = match &self.answer {
                                    Answer::Ready(lookup) => match lookup.detected {
                                        pdfreader_lens::Lang::En => "English → Bangla",
                                        pdfreader_lens::Lang::Bn => "Bangla → English",
                                    },
                                    _ => "LingoLearn",
                                };
                                ui.scope_builder(
                                    egui::UiBuilder::new().max_rect(slots.title),
                                    |ui| {
                                        ui.set_min_size(slots.title.size());
                                        ui.with_layout(
                                            egui::Layout::left_to_right(egui::Align::Center),
                                            |ui| {
                                                ui.add(
                                                    egui::Label::new(
                                                        RichText::new(title)
                                                            .color(palette.text_dim)
                                                            .size(12.0),
                                                    )
                                                    .wrap_mode(egui::TextWrapMode::Truncate),
                                                );
                                            },
                                        );
                                    },
                                );
                                if let Some(speak_rect) = slots.speak {
                                    let response = header_hit(ui, speak_rect, "lens-speak");
                                    if response.hovered() {
                                        ui.painter().rect_filled(
                                            speak_rect,
                                            4.0,
                                            palette.surface_hover,
                                        );
                                    }
                                    paint_speak_control(ui, speak_rect, speaking, palette.text);
                                    let hint = if speaking { "Stop" } else { "Pronounce" };
                                    if response.on_hover_text(hint).clicked() {
                                        if speaking {
                                            stop_speech = true;
                                        } else {
                                            let lang = match &self.answer {
                                                Answer::Ready(lookup) => lookup.detected.code(),
                                                _ => "en",
                                            };
                                            speak = Some((self.text.clone(), lang));
                                        }
                                    }
                                }
                                let close_response = header_hit(ui, slots.close, "lens-close");
                                if close_response.hovered() {
                                    ui.painter().rect_filled(
                                        slots.close,
                                        4.0,
                                        palette.surface_hover,
                                    );
                                }
                                pdfreader_ui::draw_icon(
                                    ui.painter(),
                                    slots.close.center(),
                                    pdfreader_ui::Icon::Close,
                                    palette.text,
                                );
                                if close_response.on_hover_text("Close").clicked() {
                                    close = true;
                                }
                            });
                        ui.separator();

                        // --- body -----------------------------------------
                        // The body scrolls. A sentence-long translation used
                        // to be clipped by the window edge; it now wraps to
                        // the card width and scrolls inside it.
                        egui::Frame::new()
                            .inner_margin(Margin::symmetric(12, 10))
                            .show(ui, |ui| {
                                let width = ui.available_width();
                                // Leave a gutter so wrapped lines are not hidden
                                // under the scrollbar, and cap the viewport so a
                                // long translation scrolls inside the card
                                // instead of running off the screen.
                                let wrap_width = (width - 16.0).max(40.0);
                                let max_height = body_max_height(
                                    ui.ctx().content_rect().height(),
                                    translate_enabled,
                                );
                                egui::ScrollArea::vertical()
                                    .id_salt("lens-body")
                                    .max_height(max_height)
                                    .auto_shrink([false, true])
                                    .scroll_bar_visibility(
                                        egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded,
                                    )
                                    .show(ui, |ui| {
                                        ui.set_width(wrap_width);
                                        wrapped(
                                            ui,
                                            RichText::new(self.text.as_str())
                                                .color(palette.text_dim)
                                                .size(12.0),
                                        );

                                        match &self.answer {
                                            Answer::Loading => {
                                                wrapped(
                                                    ui,
                                                    RichText::new("Loading dictionaries…")
                                                        .color(palette.text_dim)
                                                        .size(13.0),
                                                );
                                            }
                                            Answer::Missing(reason) => {
                                                wrapped(
                                                    ui,
                                                    RichText::new(reason)
                                                        .color(palette.danger)
                                                        .size(13.0),
                                                );
                                            }
                                            Answer::Ready(lookup) => {
                                                if !lookup.translation.is_empty() {
                                                    ui.add_space(4.0);
                                                    wrapped(
                                                        ui,
                                                        RichText::new(&lookup.translation)
                                                            .color(palette.text)
                                                            .size(16.0),
                                                    );
                                                }
                                                if !lookup.ipa.is_empty() {
                                                    wrapped(
                                                        ui,
                                                        RichText::new(format!("/{}/", lookup.ipa))
                                                            .color(palette.text_dim)
                                                            .size(12.0),
                                                    );
                                                }
                                                if lookup.partial {
                                                    wrapped(
                                                        ui,
                                                        RichText::new("word-by-word, approximate")
                                                            .color(palette.text_dim)
                                                            .size(11.0),
                                                    );
                                                }
                                                if !lookup.groups.is_empty() {
                                                    ui.add_space(6.0);
                                                    for group in &lookup.groups {
                                                        ui.horizontal_wrapped(|ui| {
                                                            if !group.pos.is_empty() {
                                                                ui.label(
                                                                    RichText::new(&group.pos)
                                                                        .color(palette.text_dim)
                                                                        .size(11.0)
                                                                        .italics(),
                                                                );
                                                            }
                                                            for term in &group.terms {
                                                                pdfreader_ui::chip_frame(palette)
                                                                    .show(ui, |ui| {
                                                                        ui.set_max_width(
                                                                            width - 24.0,
                                                                        );
                                                                        wrapped(
                                                                            ui,
                                                                            RichText::new(term)
                                                                                .color(palette.text)
                                                                                .size(13.0),
                                                                        );
                                                                    });
                                                            }
                                                        });
                                                    }
                                                }
                                            }
                                        }
                                    });
                            });

                        // --- footer ---------------------------------------
                        if translate_enabled {
                            ui.separator();
                            egui::Frame::new()
                                .inner_margin(Margin::symmetric(12, 6))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    RichText::new("Google Translate").size(10.0),
                                                )
                                                .fill(egui::Color32::TRANSPARENT),
                                            )
                                            .on_hover_text("Open in the browser")
                                            .clicked()
                                        {
                                            translate = Some(self.text.clone());
                                        }
                                    });
                                });
                        }
                    });
            });

        if stop_speech {
            self.speaker.stop();
        } else if let Some((text, lang)) = speak {
            if let Err(error) = self.speaker.speak(&text, lang) {
                tracing::warn!("could not speak the selection ({error})");
            }
        }
        if let Some(text) = translate {
            let link = translate_link(&text, target);
            if let Err(error) = pdfreader_platform::url::open(&link) {
                tracing::warn!("could not open the translate link ({error})");
            }
        }

        // Escape closes, which is what every other popup in the app does.
        if close || ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.close();
        }

        // Hand back the card's screen rect so the caller can tell when the
        // pointer is over it — a floating area swallows the wheel, and the
        // shell uses this to keep the canvas scrolling underneath it.
        self.open.then_some(area.response.rect)
    }
}

/// The Speak control: a speaker glyph and the word, centred as one group.
///
/// `speaking` swaps in the glyph with sound waves. The word stays put.
fn paint_speak_control(ui: &egui::Ui, rect: egui::Rect, speaking: bool, color: Color32) {
    let galley = egui::WidgetText::from(RichText::new("Speak").size(13.0).color(color))
        .into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Button,
        );
    let icon_side = 16.0;
    let gap = 4.0;
    let total = icon_side + gap + galley.size().x;
    let left = rect.center().x - total * 0.5;
    pdfreader_ui::draw_icon(
        ui.painter(),
        egui::pos2(left + icon_side * 0.5, rect.center().y),
        if speaking {
            pdfreader_ui::Icon::Speaking
        } else {
            pdfreader_ui::Icon::Speak
        },
        color,
    );
    ui.painter().galley(
        egui::pos2(
            left + icon_side + gap,
            rect.center().y - galley.size().y * 0.5,
        ),
        galley,
        color,
    );
}

/// A label that wraps to the card width instead of running past it.
fn wrapped(ui: &mut egui::Ui, text: RichText) {
    ui.add(egui::Label::new(text).wrap_mode(egui::TextWrapMode::Wrap));
}

/// Where the title, Speak, and the close mark sit inside the header row.
struct HeaderSlots {
    title: egui::Rect,
    speak: Option<egui::Rect>,
    close: egui::Rect,
}

/// Place Speak and the close mark on the right of `row`, sharing its centre.
///
/// The close mark is a square centred in the row so the X lines up with the
/// Speak label. The title takes whatever width is left.
fn header_slots(row: egui::Rect, speak_enabled: bool) -> HeaderSlots {
    let close = egui::Rect::from_center_size(
        egui::pos2(row.right() - CLOSE_SIDE * 0.5, row.center().y),
        egui::vec2(CLOSE_SIDE, CLOSE_SIDE),
    );
    let speak = speak_enabled.then(|| {
        egui::Rect::from_min_max(
            egui::pos2(close.left() - HEADER_GAP - SPEAK_WIDTH, row.top()),
            egui::pos2(close.left() - HEADER_GAP, row.bottom()),
        )
    });
    let title_right = speak
        .map(|rect| rect.left() - HEADER_GAP)
        .unwrap_or_else(|| close.left() - HEADER_GAP);
    let title = egui::Rect::from_min_max(
        row.left_top(),
        egui::pos2(title_right.max(row.left()), row.bottom()),
    );
    HeaderSlots {
        title,
        speak,
        close,
    }
}

/// Click target painted later by the caller. An empty sense on the row would
/// otherwise take the click before this rect is tested.
fn header_hit(ui: &egui::Ui, rect: egui::Rect, salt: &str) -> egui::Response {
    ui.interact(rect, ui.id().with(salt), egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// How tall the scrolling body may be on a screen of `screen_height`.
///
/// The chrome (header, footer, margins) is reserved first, so the card itself
/// fits on the screen and a long translation scrolls inside the body.
fn body_max_height(screen_height: f32, translate_enabled: bool) -> f32 {
    let footer = if translate_enabled { 44.0 } else { 12.0 };
    let chrome = HEADER_HEIGHT + 28.0 + footer;
    (screen_height - chrome - 16.0).clamp(140.0, BODY_MAX_HEIGHT)
}

/// Load the dictionaries on a background thread.
///
/// Reading and parsing the three files takes a moment, and doing it on the UI
/// thread would stall the first frames of the window. The receiver yields once,
/// with either the dictionaries or the reason they could not be read.
pub fn spawn_load(dir: Option<&Path>) -> Receiver<Result<Dictionaries, String>> {
    let (tx, rx) = crossbeam_channel::bounded(1);
    let dir = dir.map(Path::to_path_buf);
    std::thread::spawn(move || {
        let result = match dir.as_deref() {
            Some(dir) => pdfreader_lens::load::load(dir).map_err(|error| error.to_string()),
            None => Err("no dictionary folder found".to_owned()),
        };
        let _ = tx.send(result);
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfreader_core::DocumentId;

    fn page() -> TextPage {
        pdfreader_pdf::text::build(Vec::new())
    }

    /// The popup must report its screen rect while open so the shell can keep
    /// the canvas scrolling underneath it — a floating card otherwise swallows
    /// the wheel, which is what broke scrolling right after selecting text.
    #[test]
    fn an_open_popup_reports_its_rect() {
        let ctx = egui::Context::default();
        let palette =
            pdfreader_ui::theme::Theme::from_id(pdfreader_core::ThemeId::default()).palette;
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            ..Default::default()
        };

        let mut popup = LensPopup::new();
        let mut out = ctx.run_ui(raw.clone(), |ui| {
            assert!(
                popup
                    .show(ui.ctx(), &palette, "zh-CN", true, true)
                    .is_none(),
                "a closed card has no rect"
            );
        });
        out.textures_delta.clear();

        popup.open(
            LookupRequest {
                page: 0,
                text: "hello".into(),
                anchor: egui::pos2(120.0, 120.0),
            },
            Answer::Loading,
            false,
        );
        let mut out = ctx.run_ui(raw, |ui| {
            assert!(
                popup
                    .show(ui.ctx(), &palette, "zh-CN", true, true)
                    .is_some(),
                "an open card reports its rect"
            );
        });
        out.textures_delta.clear();
    }

    #[test]
    fn a_page_is_requested_once() {
        let mut cache = PageTextCache::new();
        let doc = DocumentId::from_raw(1);
        assert!(cache.request(doc, 0), "the first ask should send a request");
        assert!(!cache.request(doc, 0), "a repeat ask should not");
        cache.insert(doc, 0, page());
        assert!(!cache.request(doc, 0), "a cached page needs no request");
        assert!(cache.get(doc, 0).is_some());
    }

    #[test]
    fn a_failed_page_is_not_waited_for_again() {
        let mut cache = PageTextCache::new();
        let doc = DocumentId::from_raw(1);
        assert!(cache.request(doc, 3));
        cache.failed(doc, 3);
        assert!(!cache.request(doc, 3));
        assert!(
            cache.get(doc, 3).is_some(),
            "a failed page reads as empty text"
        );
    }

    #[test]
    fn closing_a_document_drops_its_text() {
        let mut cache = PageTextCache::new();
        let doc = DocumentId::from_raw(7);
        cache.insert(doc, 0, page());
        cache.insert(DocumentId::from_raw(8), 0, page());
        cache.drop_document(doc);
        assert!(cache.get(doc, 0).is_none());
        assert!(cache.get(DocumentId::from_raw(8), 0).is_some());
    }

    #[test]
    fn answers_carry_lookup_errors_as_text() {
        let answer = Answer::from_lookup(Err(LookupError::NoMeaning("zzz".to_owned())));
        assert_eq!(
            answer,
            Answer::Missing("no offline meaning found for \"zzz\"".to_owned())
        );
    }

    #[test]
    fn only_language_codes_reach_the_translate_link() {
        assert!(pdfreader_lens::is_valid_target("bn"));
        assert!(!pdfreader_lens::is_valid_target("javascript"));
        assert!(translate_link("abbey", "bad code").contains("tl=bn"));
    }

    #[test]
    fn speak_and_close_share_the_header_centre() {
        let row = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(356.0, HEADER_HEIGHT));
        let slots = header_slots(row, true);
        let speak = slots.speak.expect("speak is shown");
        assert!(
            (speak.center().y - slots.close.center().y).abs() < 0.5,
            "speak and the close mark must share a centre line"
        );
        assert!(
            (slots.close.right() - row.right()).abs() < 0.5,
            "the close mark must sit on the right edge"
        );
        assert!(
            speak.right() <= slots.close.left() - HEADER_GAP + 0.5,
            "speak must sit just left of the close mark"
        );
        assert!(
            slots.title.right() <= speak.left(),
            "the title must not run into Speak"
        );
        assert!(row.contains_rect(speak));
        assert!(row.contains_rect(slots.close));

        let hidden = header_slots(row, false);
        assert!(hidden.speak.is_none());
        assert!(hidden.title.right() <= hidden.close.left());
        assert!(row.contains_rect(hidden.close));
    }

    /// A long answer has to wrap and scroll inside the card. An uncapped body
    /// would be far taller than the screen; a one-line clip would stay as
    /// short as a single word.
    #[test]
    fn a_long_translation_scrolls_inside_the_card() {
        let screen = egui::vec2(800.0, 900.0);
        let short = card_rect("হ্যালো", screen);
        let long = card_rect(&"অনুবাদ ".repeat(300), screen);
        assert!(
            long.height() > short.height() + 200.0,
            "a long translation must wrap and open the card (short {short:?}, long {long:?})"
        );
        assert!(
            long.bottom() <= screen.y + 1.0 && long.right() <= screen.x + 1.0,
            "the card must stay on screen: {long:?}"
        );
        assert!(
            long.height() < 700.0,
            "the card must cap and scroll instead of growing with the text: {long:?}"
        );
    }

    fn card_rect(translation: &str, screen: egui::Vec2) -> egui::Rect {
        let ctx = egui::Context::default();
        let palette =
            pdfreader_ui::theme::Theme::from_id(pdfreader_core::ThemeId::default()).palette;
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, screen)),
            ..Default::default()
        };
        let mut popup = LensPopup::new();
        popup.open(
            LookupRequest {
                page: 0,
                text: "a selected sentence that is itself long enough to wrap".into(),
                anchor: egui::pos2(40.0, 48.0),
            },
            Answer::Ready(pdfreader_lens::Lookup {
                translation: translation.to_owned(),
                ..Default::default()
            }),
            false,
        );
        // The scroll area learns its content size on the first frame and
        // applies the cap on the second.
        let mut out = ctx.run_ui(raw.clone(), |ui| {
            popup.show(ui.ctx(), &palette, "bn", true, true);
        });
        out.textures_delta.clear();
        let mut rect = None;
        let mut out = ctx.run_ui(raw, |ui| {
            rect = popup.show(ui.ctx(), &palette, "bn", true, true);
        });
        out.textures_delta.clear();
        rect.expect("an open card reports its rect")
    }
}
