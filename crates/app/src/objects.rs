//! Page-content objects, cached per page for the editor.
//!
//! Shaped like [`crate::lens::PageTextCache`], and for the same reasons: the
//! engine owns the list, the canvas needs it synchronously while drawing, and
//! asking per frame would put a `page.objects()` walk in every redraw.
//!
//! One deliberate difference: text does not change under the reader, so that
//! cache never expires an entry. Objects do — every edit invalidates the page it
//! touched — which is what [`PageObjectCache::invalidate`] is for.

use std::collections::{HashMap, HashSet};

use pdfreader_core::{DocumentId, PageObjectId, PageObjectInfo, TextFormat};

/// Page objects by document and page.
#[derive(Default)]
pub struct PageObjectCache {
    /// Object lists by document and page.
    pages: HashMap<(DocumentId, u32), Vec<PageObjectInfo>>,
    /// Requests sent to the engine but not answered yet.
    inflight: HashSet<(DocumentId, u32)>,
    /// The style each text run had the first time it was read.
    ///
    /// Later lists report the style after an edit. Reset and the first undo
    /// need the PDF's own bold, size, and ink, so this is written once and
    /// kept across those lists.
    originals: HashMap<(DocumentId, PageObjectId), TextFormat>,
}

impl PageObjectCache {
    /// An empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// The objects of a page, once they have been read.
    pub fn get(&self, doc: DocumentId, page: u32) -> Option<&[PageObjectInfo]> {
        self.pages.get(&(doc, page)).map(Vec::as_slice)
    }

    /// Mark a page as wanted.
    ///
    /// Returns `true` when the caller should send a request, which is only the
    /// first time a page is asked for — that is what stops a frame from
    /// re-requesting the list on every redraw.
    pub fn request(&mut self, doc: DocumentId, page: u32) -> bool {
        if self.pages.contains_key(&(doc, page)) {
            return false;
        }
        self.inflight.insert((doc, page))
    }

    /// Store a page's objects, clearing its in-flight marker.
    ///
    /// Text objects have no paragraph box in a PDF; their current box is just
    /// the ink bounds of the string they currently hold. Preserve the original
    /// box and size across a text-only change so the next edit still fits to the
    /// same layout instead of progressively shrinking its available area.
    /// A move/resize leaves the text alone but changes its box, so the fresh box
    /// becomes the new layout constraint.
    pub fn insert(&mut self, doc: DocumentId, page: u32, mut objects: Vec<PageObjectInfo>) {
        let key = (doc, page);
        if let Some(previous) = self.pages.get(&key) {
            for object in &mut objects {
                let Some(old) = previous
                    .iter()
                    .find(|old| old.id == object.id && old.kind == object.kind)
                else {
                    continue;
                };
                if object.kind == pdfreader_core::PageObjectKind::Text {
                    // If this read could not see the ink, keep the colour the
                    // shell already has so the panel does not flash back to black.
                    if object.format.color.is_none() {
                        object.format.color = old.format.color;
                    }
                    // A text-only change preserves the original box and size so
                    // the next edit fits to the same layout instead of shrinking.
                    if old.text != object.text {
                        object.layout_bounds = old.layout_bounds;
                        object.font_size = old.font_size;
                    }
                }
            }
        }
        for object in &objects {
            if object.kind == pdfreader_core::PageObjectKind::Text {
                // First sight only. A later list reports the edited style, and
                // Reset / undo need the PDF's own bold, size, and ink.
                self.originals
                    .entry((doc, object.id))
                    .or_insert(object.format);
            }
        }
        self.inflight.remove(&key);
        self.pages.insert(key, objects);
    }

    /// The style this text run had when it was first read from the PDF.
    pub fn original_format(&self, doc: DocumentId, id: PageObjectId) -> Option<TextFormat> {
        self.originals.get(&(doc, id)).copied()
    }

    /// Update the cached format of one object.
    ///
    /// The shell calls this the moment a formatting command is dispatched, before
    /// the engine's re-list arrives (which cannot report colour). The next
    /// [`Self::insert`] then preserves that colour from this cache entry.
    pub fn set_format(&mut self, doc: DocumentId, id: PageObjectId, format: TextFormat) {
        if let Some(objects) = self.pages.get_mut(&(doc, id.page)) {
            for object in objects {
                if object.id == id {
                    object.format = format;
                    break;
                }
            }
        }
    }

    /// Record that reading failed.
    ///
    /// An empty list rather than an absent entry, so the page is not waited for
    /// again and the canvas reads it as "nothing here to edit" — the honest
    /// answer for a page the engine could not walk.
    pub fn failed(&mut self, doc: DocumentId, page: u32) {
        self.insert(doc, page, Vec::new());
    }

    /// Forget a page, so the next frame asks for it again.
    ///
    /// Called after every mutation. Without it the canvas would keep hit-testing
    /// against the boxes things had *before* they were moved, and a second drag
    /// would start from the wrong place.
    pub fn invalidate(&mut self, doc: DocumentId, page: u32) {
        self.pages.remove(&(doc, page));
        self.inflight.remove(&(doc, page));
    }

    /// Forget a closed document's objects.
    pub fn drop_document(&mut self, doc: DocumentId) {
        self.pages.retain(|(id, _), _| *id != doc);
        self.inflight.retain(|(id, _)| *id != doc);
        self.originals.retain(|(id, _), _| *id != doc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfreader_core::{PageObjectId, PageObjectKind, Rect, TextFormat};

    fn object(index: u32) -> PageObjectInfo {
        PageObjectInfo {
            id: PageObjectId { page: 0, index },
            kind: PageObjectKind::Text,
            bounds: Rect::from_xywh(0.0, 0.0, 10.0, 10.0),
            layout_bounds: Rect::from_xywh(0.0, 0.0, 10.0, 10.0),
            text: Some("x".to_string()),
            font_size: Some(12.0),
            format: TextFormat::default(),
        }
    }

    #[test]
    fn a_page_is_asked_for_once_until_it_is_answered() {
        let mut cache = PageObjectCache::new();
        let doc = DocumentId::from_raw(1);

        assert!(cache.request(doc, 3), "the first ask sends a request");
        assert!(!cache.request(doc, 3), "a second ask does not");
        assert!(cache.get(doc, 3).is_none(), "nothing is known yet");

        cache.insert(doc, 3, vec![object(0)]);
        assert!(!cache.request(doc, 3), "an answered page is not re-asked");
        assert_eq!(cache.get(doc, 3).map(<[_]>::len), Some(1));
    }

    #[test]
    fn invalidating_a_page_makes_it_ask_again() {
        let mut cache = PageObjectCache::new();
        let doc = DocumentId::from_raw(1);
        cache.request(doc, 0);
        cache.insert(doc, 0, vec![object(0), object(1)]);

        cache.invalidate(doc, 0);
        assert!(cache.get(doc, 0).is_none(), "the stale list is gone");
        assert!(cache.request(doc, 0), "and the page is wanted again");
    }

    #[test]
    fn a_failed_page_is_not_waited_for() {
        let mut cache = PageObjectCache::new();
        let doc = DocumentId::from_raw(1);
        cache.request(doc, 2);
        cache.failed(doc, 2);

        assert_eq!(cache.get(doc, 2).map(<[_]>::len), Some(0));
        assert!(!cache.request(doc, 2), "a failed page is not re-asked");
    }

    #[test]
    fn the_first_style_is_kept_after_the_run_is_restyled() {
        let mut cache = PageObjectCache::new();
        let doc = DocumentId::from_raw(1);
        let mut original = object(0);
        original.format = TextFormat {
            bold: false,
            color: Some([10, 20, 30, 255]),
            size: Some(11.0),
        };
        original.font_size = Some(11.0);
        cache.insert(doc, 0, vec![original.clone()]);

        let mut edited = original.clone();
        edited.format = TextFormat {
            bold: true,
            color: Some([255, 0, 0, 255]),
            size: Some(24.0),
        };
        edited.font_size = Some(24.0);
        cache.insert(doc, 0, vec![edited]);

        assert_eq!(
            cache.original_format(doc, original.id),
            Some(original.format),
            "reset and undo must still see the style the PDF was set in"
        );
    }

    #[test]
    fn closing_a_document_forgets_only_its_pages() {
        let mut cache = PageObjectCache::new();
        let (a, b) = (DocumentId::from_raw(1), DocumentId::from_raw(2));
        cache.insert(a, 0, vec![object(0)]);
        cache.insert(b, 0, vec![object(0)]);

        cache.drop_document(a);
        assert!(cache.get(a, 0).is_none());
        assert!(cache.get(b, 0).is_some(), "the other document is untouched");
    }
}
