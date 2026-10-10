//! Text geometry extracted from a page.
//!
//! Character boxes come from the engine in PDF page space. Words and lines are
//! derived once per page and cached; they are *not* keyed on zoom, because zoom
//! is an affine transform applied at draw time. Keying them on zoom is a
//! classic way to turn a 60fps scroll into a rebuild-per-frame.
//!
//! The word and line structure is what text selection hangs off: a click is
//! hit-tested to a character, then widened to that character's whole word, and
//! a drag becomes a range that is drawn as one rectangle per line.

use crate::geometry::Rect;

/// How much of the shorter box must overlap vertically for two characters to
/// count as being on the same line.
const LINE_OVERLAP: f32 = 0.5;

/// A horizontal gap wider than this fraction of the character height starts a
/// new word. PDFs that express spaces as gaps rather than as characters rely on
/// it; a real space character is handled separately.
const WORD_GAP_FACTOR: f32 = 0.35;

/// One character and the box it occupies, in PDF page points.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct CharBox {
    /// The character.
    pub ch: char,
    /// Character box in page space.
    pub rect: Rect,
    /// Index of the line this character belongs to.
    pub line: u32,
    /// Index of the word this character belongs to.
    pub word: u32,
}

/// A run of characters forming a word.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Span {
    /// Index of the first character in the page's character list.
    pub first_char: u32,
    /// Index one past the last character.
    pub last_char: u32,
    /// Bounding box of the whole span.
    pub rect: Rect,
    /// Index of the line containing this span.
    pub line: u32,
}

/// A line of text.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Line {
    /// Index of the first character.
    pub first_char: u32,
    /// Index one past the last character.
    pub last_char: u32,
    /// Bounding box of the whole line.
    pub rect: Rect,
}

/// All text geometry for one page, in reading order.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct TextPage {
    /// Every character on the page, in reading order.
    pub chars: Vec<CharBox>,
    /// Words, in reading order.
    pub words: Vec<Span>,
    /// Lines, in reading order.
    pub lines: Vec<Line>,
}

/// A selection: a half-open range of indices into [`TextPage::chars`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct CharRange {
    /// Index of the first selected character.
    pub first: u32,
    /// Index one past the last selected character.
    pub last: u32,
}

impl CharRange {
    /// The range covering both ends, whichever way the selection was dragged.
    ///
    /// Both ends are inclusive character indices, so dragging from a later
    /// character back to an earlier one selects the same text.
    pub fn spanning(a: u32, b: u32) -> Self {
        let (first, last) = if a <= b { (a, b) } else { (b, a) };
        Self {
            first,
            last: last + 1,
        }
    }

    /// Number of selected characters.
    pub fn len(self) -> u32 {
        self.last.saturating_sub(self.first)
    }

    /// Whether nothing is selected.
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// Whether the range covers a character index.
    pub fn contains(self, index: u32) -> bool {
        index >= self.first && index < self.last
    }
}

/// How much of the shorter box two boxes overlap vertically, from 0 to 1.
fn vertical_overlap(a: Rect, b: Rect) -> f32 {
    let overlap = a.max_y.min(b.max_y) - a.min_y.max(b.min_y);
    let shorter = a.height().min(b.height());
    if shorter <= 0.0 {
        // Degenerate boxes (spaces, zero-width glyphs) carry no vertical
        // information, so they never break a line.
        return 1.0;
    }
    (overlap / shorter).clamp(0.0, 1.0)
}

/// Turn engine characters into a page whose words and lines are derived.
///
/// Characters are expected in reading order, which is what PDFium yields. A
/// character moves to a new line when it stops overlapping the line so far
/// vertically, and into a new word after a space or a wide horizontal gap.
pub fn build(chars: Vec<CharBox>) -> TextPage {
    let mut chars = chars;
    let mut lines: Vec<Line> = Vec::new();
    let mut words: Vec<Span> = Vec::new();
    let mut line: Option<(u32, Rect)> = None;
    let mut word: Option<u32> = None;
    let mut previous_right: Option<f32> = None;
    let mut previous_height = 0.0_f32;

    for position in 0..chars.len() {
        let ch = chars[position].ch;
        let rect = chars[position].rect;
        let at = position as u32;

        // --- line -------------------------------------------------------
        let continues =
            line.is_some_and(|(_, line_rect)| vertical_overlap(line_rect, rect) >= LINE_OVERLAP);
        let line_index = if continues {
            line.map_or(0, |(index, _)| index)
        } else {
            let index = lines.len() as u32;
            lines.push(Line {
                first_char: at,
                last_char: at + 1,
                rect,
            });
            // A new line always starts a new word.
            word = None;
            previous_right = None;
            index
        };
        chars[position].line = line_index;
        let line_rect = {
            let entry = &mut lines[line_index as usize];
            entry.last_char = at + 1;
            entry.rect = entry.rect.union(rect);
            entry.rect
        };
        line = Some((line_index, line_rect));

        // --- word -------------------------------------------------------
        if ch.is_whitespace() {
            // A space belongs to the word it follows but is not part of it, so
            // a word's span never carries a trailing space.
            chars[position].word = word.unwrap_or(words.len() as u32);
            word = None;
            previous_right = Some(rect.max_x);
            continue;
        }

        let gap = previous_right.map_or(0.0_f32, |right| rect.min_x - right);
        let starts_word =
            word.is_none() || gap > WORD_GAP_FACTOR * previous_height.max(rect.height());

        let word_index = if starts_word {
            let index = words.len() as u32;
            words.push(Span {
                first_char: at,
                last_char: at + 1,
                rect,
                line: line_index,
            });
            word = Some(index);
            index
        } else {
            let index = word.unwrap_or(0);
            let entry = &mut words[index as usize];
            entry.last_char = at + 1;
            entry.rect = entry.rect.union(rect);
            index
        };
        chars[position].word = word_index;

        previous_right = Some(rect.max_x);
        if rect.height() > 0.0 {
            previous_height = rect.height();
        }
    }

    TextPage {
        chars,
        words,
        lines,
    }
}

impl TextPage {
    /// Whether the page has no extractable text (typically a scanned page).
    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    /// Concatenate the characters in a range into a string.
    pub fn text_in(&self, first: u32, last: u32) -> String {
        self.chars
            .iter()
            .skip(first as usize)
            .take((last.saturating_sub(first)) as usize)
            .map(|c| c.ch)
            .collect()
    }

    /// The full page text.
    pub fn text(&self) -> String {
        self.chars.iter().map(|c| c.ch).collect()
    }

    /// The character whose box contains a page-space point, if any.
    ///
    /// Later characters win, so the last-drawn box is picked when glyphs
    /// overlap.
    pub fn char_at(&self, x: f32, y: f32) -> Option<u32> {
        self.chars
            .iter()
            .enumerate()
            .rev()
            .find(|(_, character)| !character.rect.is_degenerate() && character.rect.contains(x, y))
            .map(|(index, _)| index as u32)
    }

    /// The character nearest a page-space point, within `max_distance` points.
    ///
    /// Used when a click lands beside a glyph rather than inside it. Distance
    /// is measured to the box, so a point inside a box is zero away.
    pub fn nearest_char(&self, x: f32, y: f32, max_distance: f32) -> Option<u32> {
        let mut best: Option<(u32, f32)> = None;
        for (index, character) in self.chars.iter().enumerate() {
            if character.rect.is_degenerate() {
                continue;
            }
            let dx = (character.rect.min_x - x)
                .max(x - character.rect.max_x)
                .max(0.0);
            let dy = (character.rect.min_y - y)
                .max(y - character.rect.max_y)
                .max(0.0);
            let distance = dx.hypot(dy);
            if distance > max_distance {
                continue;
            }
            if best.is_none_or(|(_, best_distance)| distance < best_distance) {
                best = Some((index as u32, distance));
            }
        }
        best.map(|(index, _)| index)
    }

    /// The word at a page-space point: the nearest character, widened to its
    /// whole word.
    ///
    /// `max_distance` is in points; callers pass a fraction of the line height
    /// so the tolerance scales with the text.
    pub fn word_at(&self, x: f32, y: f32, max_distance: f32) -> Option<CharRange> {
        let index = self
            .char_at(x, y)
            .or_else(|| self.nearest_char(x, y, max_distance))?;
        Some(self.word_range(index))
    }

    /// The range covering the whole word a character belongs to.
    pub fn word_range(&self, index: u32) -> CharRange {
        let Some(word) = self
            .chars
            .get(index as usize)
            .map(|character| character.word)
        else {
            return CharRange::default();
        };
        match self.words.get(word as usize) {
            Some(span) => CharRange {
                first: span.first_char,
                last: span.last_char,
            },
            // An unmapped character, such as a stray space, selects itself.
            None => CharRange {
                first: index,
                last: index + 1,
            },
        }
    }

    /// The range covering the whole line a character belongs to.
    pub fn line_range(&self, index: u32) -> CharRange {
        let Some(line) = self
            .chars
            .get(index as usize)
            .map(|character| character.line)
        else {
            return CharRange::default();
        };
        match self.lines.get(line as usize) {
            Some(line) => CharRange {
                first: line.first_char,
                last: line.last_char,
            },
            None => CharRange {
                first: index,
                last: index + 1,
            },
        }
    }

    /// The text of a range.
    pub fn range_text(&self, range: CharRange) -> String {
        self.text_in(range.first, range.last)
    }

    /// One bounding box per text line covered by a range, in reading order.
    ///
    /// Drawing a selection as one box per line is what makes a range spanning
    /// several lines follow the text instead of covering the gutter.
    pub fn range_rects(&self, range: CharRange) -> Vec<Rect> {
        let start = (range.first as usize).min(self.chars.len());
        let end = (range.last as usize).min(self.chars.len());
        let mut rects: Vec<(u32, Rect)> = Vec::new();

        for character in &self.chars[start..end] {
            if character.rect.is_degenerate() {
                continue;
            }
            match rects.last_mut() {
                Some((line, rect)) if *line == character.line => {
                    *rect = rect.union(character.rect);
                }
                _ => rects.push((character.line, character.rect)),
            }
        }

        rects.into_iter().map(|(_, rect)| rect).collect()
    }

    /// The single bounding box of a range, or `None` when it selects nothing.
    pub fn range_rect(&self, range: CharRange) -> Option<Rect> {
        let start = (range.first as usize).min(self.chars.len());
        let end = (range.last as usize).min(self.chars.len());
        self.chars[start..end]
            .iter()
            .filter(|character| !character.rect.is_degenerate())
            .fold(None, |acc: Option<Rect>, character| {
                Some(acc.map_or(character.rect, |rect| rect.union(character.rect)))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn character(ch: char, x: f32, y: f32) -> CharBox {
        let width = if ch == ' ' { 3.0 } else { 8.0 };
        CharBox {
            ch,
            rect: Rect::from_xywh(x, y, width, 12.0),
            line: 0,
            word: 0,
        }
    }

    /// Lay a string out left to right at a given baseline.
    fn line(text: &str, y: f32) -> Vec<CharBox> {
        let mut x = 10.0;
        text.chars()
            .map(|ch| {
                let boxed = character(ch, x, y);
                x += boxed.rect.width();
                boxed
            })
            .collect()
    }

    fn page() -> TextPage {
        let mut chars = line("hello world", 100.0);
        chars.extend(line("second line", 80.0));
        build(chars)
    }

    #[test]
    fn lines_and_words_are_derived() {
        let page = page();
        assert_eq!(page.lines.len(), 2);
        assert_eq!(page.words.len(), 4);
        assert_eq!(page.text(), "hello worldsecond line");
    }

    #[test]
    fn a_word_span_excludes_its_trailing_space() {
        let page = page();
        let hello = &page.words[0];
        assert_eq!(page.text_in(hello.first_char, hello.last_char), "hello");
        let world = &page.words[1];
        assert_eq!(page.text_in(world.first_char, world.last_char), "world");
    }

    #[test]
    fn a_wide_gap_starts_a_new_word_without_a_space_character() {
        let mut chars = line("ab", 100.0);
        // Same baseline, but far enough right to be its own word.
        chars.push(character('c', 100.0, 100.0));
        assert_eq!(
            build(chars).words.len(),
            2,
            "the gap alone should split the words"
        );
    }

    #[test]
    fn a_point_inside_a_glyph_hits_it() {
        let page = page();
        let hit = page.char_at(14.0, 104.0).expect("inside the first glyph");
        assert_eq!(page.chars[hit as usize].ch, 'h');
    }

    #[test]
    fn a_point_beside_a_glyph_hits_the_nearest_one() {
        let page = page();
        let near = page
            .nearest_char(95.0, 106.0, 40.0)
            .expect("within tolerance");
        assert_eq!(page.chars[near as usize].ch, 'd');
        assert!(page.nearest_char(500.0, 104.0, 10.0).is_none());
    }

    #[test]
    fn a_click_widens_to_the_whole_word() {
        let page = page();
        let range = page.word_at(14.0, 104.0, 20.0).expect("on the word hello");
        assert_eq!(page.range_text(range), "hello");
    }

    #[test]
    fn a_range_spanning_two_lines_gives_one_rect_per_line() {
        let page = page();
        let range = CharRange {
            first: page.words[0].first_char,
            last: page.words[3].last_char,
        };
        let rects = page.range_rects(range);
        assert_eq!(rects.len(), 2, "one box per line");
        // PDF space has y up, so the first box is the upper line.
        assert!(rects[0].min_y > rects[1].min_y);
    }

    #[test]
    fn a_range_rect_bounds_every_selected_glyph() {
        let page = page();
        let rect = page
            .range_rect(CharRange {
                first: page.words[0].first_char,
                last: page.words[1].last_char,
            })
            .expect("a non-empty range has a box");
        assert!(rect.contains(10.0, 100.0));
        assert!(rect.max_x >= 72.0);
    }

    #[test]
    fn ranges_are_ordered_whichever_way_they_were_dragged() {
        let forward = CharRange::spanning(2, 5);
        let backward = CharRange::spanning(5, 2);
        assert_eq!(forward, backward);
        assert_eq!(forward.len(), 4);
        assert!(forward.contains(3));
        assert!(!forward.contains(6));
    }

    #[test]
    fn an_empty_range_selects_nothing() {
        assert!(CharRange::default().is_empty());
    }

    #[test]
    fn an_empty_page_hits_nothing() {
        let page = TextPage::default();
        assert!(page.is_empty());
        assert!(page.char_at(0.0, 0.0).is_none());
        assert!(page.range_rects(CharRange::default()).is_empty());
    }
}
