//! Page-content objects: the text and images the editor can change.
//!
//! Annotations live in their own layer above the page; these are the objects
//! that make up the page itself. They are addressed positionally — a page is a
//! list, and an object is an index into it — because that is how PDFium hands
//! them out, and the same scheme [`crate::annotation::AnnotationId`] uses.
//!
//! The types live here rather than in the engine crate for the same reason
//! [`crate::rect::Rect`] does: the editor's selection is application state, and
//! `core` cannot depend on the engine that produces it.

use serde::{Deserialize, Serialize};

use crate::rect::Rect;

/// Size new text is drawn at, in points.
///
/// 12 pt is the conventional body size and matches the default the rest of the
/// reader uses for typed text. It lives here rather than in the engine because
/// the undo stack has to record it: redo re-creates the object from the store's
/// own data, with no engine round trip to ask.
pub const DEFAULT_INSERT_FONT_SIZE: f32 = 12.0;

/// How far two boxes may differ and still be the same object, in points.
///
/// A box survives a round trip through PDFium's floating-point matrices, but not
/// bit-for-bit, so undo's re-match needs a little slack. Half a point is far
/// below anything a user could have moved deliberately.
pub const OBJECT_MATCH_TOLERANCE: f32 = 0.5;

/// Which page object is being addressed.
///
/// Positional, and therefore only valid until the page's object list changes.
/// Deleting or inserting an object renumbers everything after it, so an id kept
/// across one of those has to be re-matched rather than trusted — see
/// [`PageObjectInfo::matches`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub struct PageObjectId {
    /// Page it sits on, zero-based.
    pub page: u32,
    /// Its index in that page's object list.
    ///
    /// The *real* index: a page also holds paths and shadings, which the editor
    /// does not offer, but which still occupy slots in the list. Reporting a
    /// compacted index would make every mutation address the wrong object.
    pub index: u32,
}

/// The kinds of page object the editor can change.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub enum PageObjectKind {
    /// A run of text.
    #[default]
    Text,
    /// A bitmapped image.
    Image,
}

impl PageObjectKind {
    /// Lower-case noun, for messages and tooltips.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Image => "image",
        }
    }
}

/// Where a restacked object lands in the page's paint order.
///
/// PDF paints a page's objects in list order: index 0 is the bottom, the last
/// index the top. The panel's "bring to front" / "send to back" move an object
/// to those ends; undo restores an exact index.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub enum RestackTarget {
    /// The top of the paint order — the last object painted.
    Front,
    /// The bottom — painted first, behind everything.
    Back,
    /// An exact index, which is how undo restores a restack.
    At(u32),
}

/// The presentational formatting of a text object.
///
/// A PDF text run has no "style" concept: bold is baked into the font or faked
/// with a heavier stroke, and there is no "paragraph" to reflow inside. This
/// record is the editor's view of those fakes, so the panel can show and toggle
/// them and undo can restore them. `None` on a field means "leave whatever the
/// PDF already has".
///
/// Italic is deliberately absent. The obvious fake — shearing the object's
/// matrix — climbs the baseline across the run (the far end of a 150 pt line
/// rises ~30 pt), which both looks wrong and cannot fit the run's original
/// area. True italic needs a font the run can be re-set in, which PDFium's
/// page-object API does not offer.
#[derive(Clone, Copy, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct TextFormat {
    /// Synthetic bold: render the glyphs filled *and* stroked.
    pub bold: bool,
    /// Override the fill colour (RGBA); `None` leaves the original ink.
    pub color: Option<[u8; 4]>,
    /// Override the effective point size; `None` keeps the size the object was
    /// read at. Applied exactly as asked — a deliberate size change is the
    /// user's call even if the run grows past its original ink area. Text
    /// *retypes* try to fit their original area and overflow it when they
    /// cannot (they never refuse).
    pub size: Option<f32>,
}

/// One page object, as the editor sees it.
///
/// Doubles as the fingerprint undo re-matches on: it carries everything that
/// survives a renumbering, and nothing that does not.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct PageObjectInfo {
    /// Where it is.
    pub id: PageObjectId,
    /// What it is.
    pub kind: PageObjectKind,
    /// Its current ink box, in unrotated PDF page space.
    pub bounds: Rect,
    /// The fixed box the text was first laid out in, and which edited text must
    /// stay inside. This is identical to `bounds` when first read; the shell's
    /// object cache preserves it across retyping, because PDF objects do not
    /// have a paragraph box of their own to reflow inside.
    pub layout_bounds: Rect,
    /// Text objects: the string they hold. `None` for an image.
    pub text: Option<String>,
    /// Text objects: effective point size to use as the fit's starting point.
    /// The shell's object cache preserves it across retyping so undo restores
    /// the original size rather than progressively shrinking the line.
    pub font_size: Option<f32>,
    /// The presentational formatting the editor last applied. Read back from the
    /// object when it can be (bold via render mode, size directly), and
    /// preserved by the cache for the colour, which PDFium cannot report.
    #[serde(default)]
    pub format: TextFormat,
}

impl PageObjectInfo {
    /// Whether two records describe the same object, allowing for a shifted
    /// index.
    ///
    /// Compared on the things a renumbering cannot change, because the index is
    /// exactly what is in doubt.
    ///
    /// Text objects are identified by their **words**. A re-created one is
    /// placed from a baseline origin, so its ink box is not the box the original
    /// rendered at — comparing boxes would fail to find the very object the
    /// record describes. Images have no words, so their box is what identifies
    /// them, and that box is stable because an image is never re-created: its
    /// pixels cannot be restored, so it has no undo step to re-match for.
    pub fn matches(&self, other: &Self, tolerance: f32) -> bool {
        if self.kind != other.kind {
            return false;
        }

        if let (Some(mine), Some(theirs)) = (&self.text, &other.text) {
            return mine == theirs;
        }

        (self.bounds.min_x - other.bounds.min_x).abs() <= tolerance
            && (self.bounds.min_y - other.bounds.min_y).abs() <= tolerance
            && (self.bounds.max_x - other.bounds.max_x).abs() <= tolerance
            && (self.bounds.max_y - other.bounds.max_y).abs() <= tolerance
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(index: u32, kind: PageObjectKind, text: Option<&str>) -> PageObjectInfo {
        PageObjectInfo {
            id: PageObjectId { page: 2, index },
            kind,
            bounds: Rect::from_xywh(10.0, 20.0, 100.0, 12.0),
            layout_bounds: Rect::from_xywh(10.0, 20.0, 100.0, 12.0),
            text: text.map(ToString::to_string),
            font_size: Some(12.0),
            format: TextFormat::default(),
        }
    }

    #[test]
    fn a_renumbered_object_still_matches() {
        let before = info(3, PageObjectKind::Text, Some("Hello"));
        let after = info(5, PageObjectKind::Text, Some("Hello"));
        assert!(before.matches(&after, 0.5), "only the index moved");
    }

    #[test]
    fn a_different_object_does_not_match() {
        let text = info(3, PageObjectKind::Text, Some("Hello"));
        assert!(!text.matches(&info(3, PageObjectKind::Image, None), 0.5));
        assert!(!text.matches(&info(3, PageObjectKind::Text, Some("Goodbye")), 0.5));
    }

    #[test]
    fn a_moved_box_only_matches_within_the_tolerance() {
        let before = info(3, PageObjectKind::Image, None);
        let mut nudged = before.clone();
        nudged.bounds = Rect::from_xywh(10.2, 20.0, 100.0, 12.0);
        assert!(
            before.matches(&nudged, 0.5),
            "a rounding wobble is not a move"
        );

        nudged.bounds = Rect::from_xywh(30.0, 20.0, 100.0, 12.0);
        assert!(!before.matches(&nudged, 0.5), "20pt is a different object");
    }

    /// A text object is identified by its words, not its box.
    ///
    /// A re-created text object is placed from a baseline origin, so its ink box
    /// is not the box the original rendered at — matching on the box would fail
    /// to find the very object the record describes, and the redo would silently
    /// do nothing.
    #[test]
    fn a_text_object_matches_on_its_words_wherever_it_sits() {
        let before = info(3, PageObjectKind::Text, Some("Hello"));
        let mut recreated = before.clone();
        recreated.id = PageObjectId { page: 2, index: 9 };
        recreated.bounds = Rect::from_xywh(11.0, 18.5, 100.0, 15.0);

        assert!(
            before.matches(&recreated, 0.5),
            "the words are what identifies a text object"
        );

        // Different words, same box: not the same object.
        let mut other = before.clone();
        other.text = Some("Goodbye".to_string());
        assert!(!before.matches(&other, 0.5));
    }
}
