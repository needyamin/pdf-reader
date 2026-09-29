//! Text geometry extracted from a page.
//!
//! Character boxes come from the engine in PDF page space. Words and lines are
//! derived once per page and cached; they are *not* keyed on zoom, because zoom
//! is an affine transform applied at draw time. Keying them on zoom is a
//! classic way to turn a 60fps scroll into a rebuild-per-frame.

/// One character and the box it occupies, in PDF page points.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct CharBox {
    /// The character.
    pub ch: char,
    /// Character box in page space.
    pub rect: crate::geometry::Rect,
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
    pub rect: crate::geometry::Rect,
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
    pub rect: crate::geometry::Rect,
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
}
