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
const POPUP_WIDTH: f32 = 320.0;

/// How much of the selected text the header shows before it is cut short.
const SELECTION_CLAMP: usize = 120;

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
        let mut translate = None;

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

                        // --- header: language, speak, close ---------------
                        // Each section carries its own padding so the separators
                        // can run edge to edge. The actions are laid out
                        // right-to-left, so the language label keeps all the
                        // space it needs instead of a fixed magic gap.
                        egui::Frame::new()
                            .inner_margin(Margin::symmetric(12, 8))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(match &self.answer {
                                            Answer::Ready(lookup) => match lookup.detected {
                                                pdfreader_lens::Lang::En => "English → Bangla",
                                                pdfreader_lens::Lang::Bn => "Bangla → English",
                                            },
                                            _ => "LingoLearn",
                                        })
                                        .color(palette.text_dim)
                                        .size(11.5),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui
                                                .add(
                                                    egui::Button::new(
                                                        RichText::new("×").size(12.0),
                                                    )
                                                    .fill(egui::Color32::TRANSPARENT),
                                                )
                                                .on_hover_text("Close")
                                                .clicked()
                                            {
                                                close = true;
                                            }
                                            if speak_enabled
                                                && ui
                                                    .add(
                                                        egui::Button::new(
                                                            RichText::new("Speak").size(11.0),
                                                        )
                                                        .fill(egui::Color32::TRANSPARENT),
                                                    )
                                                    .on_hover_text("Pronounce")
                                                    .clicked()
                                            {
                                                let lang = match &self.answer {
                                                    Answer::Ready(lookup) => lookup.detected.code(),
                                                    _ => "en",
                                                };
                                                speak = Some((self.text.clone(), lang));
                                            }
                                        },
                                    );
                                });
                            });
                        ui.separator();

                        // --- body -----------------------------------------
                        egui::Frame::new()
                            .inner_margin(Margin::symmetric(12, 10))
                            .show(ui, |ui| {
                                let selected = if self.text.chars().count() > SELECTION_CLAMP {
                                    let cut: String =
                                        self.text.chars().take(SELECTION_CLAMP).collect();
                                    format!("{cut}…")
                                } else {
                                    self.text.clone()
                                };
                                ui.label(
                                    RichText::new(selected).color(palette.text_dim).size(12.0),
                                );

                                match &self.answer {
                                    Answer::Loading => {
                                        ui.label(
                                            RichText::new("Loading dictionaries…")
                                                .color(palette.text_dim)
                                                .size(13.0),
                                        );
                                    }
                                    Answer::Missing(reason) => {
                                        ui.label(
                                            RichText::new(reason).color(palette.danger).size(13.0),
                                        );
                                    }
                                    Answer::Ready(lookup) => {
                                        if !lookup.translation.is_empty() {
                                            ui.label(
                                                RichText::new(&lookup.translation)
                                                    .color(palette.text)
                                                    .size(16.0),
                                            );
                                        }
                                        if !lookup.ipa.is_empty() {
                                            ui.label(
                                                RichText::new(format!("/{}/", lookup.ipa))
                                                    .color(palette.text_dim)
                                                    .size(12.0),
                                            );
                                        }
                                        if lookup.partial {
                                            ui.label(
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
                                                        pdfreader_ui::chip_frame(palette).show(
                                                            ui,
                                                            |ui| {
                                                                // Single line, so a
                                                                // chip that does not
                                                                // fit wraps as a whole
                                                                // instead of breaking
                                                                // mid-word.
                                                                ui.add(
                                                                    egui::Label::new(
                                                                        RichText::new(term)
                                                                            .color(palette.text)
                                                                            .size(13.0),
                                                                    )
                                                                    .wrap_mode(
                                                                        egui::TextWrapMode::Extend,
                                                                    ),
                                                                );
                                                            },
                                                        );
                                                    }
                                                });
                                            }
                                        }
                                    }
                                }
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

        if let Some((text, lang)) = speak {
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
}
