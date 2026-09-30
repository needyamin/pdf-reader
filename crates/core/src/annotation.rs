//! Annotation model as domain state.
//!
//! Like form fields, the types live in `core` so the Comments panel and the
//! canvas can use them without depending on PDFium; `pdfreader_pdf` converts
//! PDFium's annotation objects into these. All rectangles are in unrotated PDF
//! page space (y up), so drawing must go through
//! `pdfreader_render::hit::PageSpace::to_screen`.

use serde::{Deserialize, Serialize};

use crate::rect::Rect;

/// Identifies one annotation: the page it sits on and its index in that page's
/// annotation list, the same identity scheme form fields use.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub struct AnnotationId {
    /// Zero-based page the annotation sits on.
    pub page: u32,
    /// Index of the annotation within that page's annotation list.
    pub annot_index: u32,
}

impl AnnotationId {
    /// Build an id from a page and an annotation index.
    pub const fn new(page: u32, annot_index: u32) -> Self {
        Self { page, annot_index }
    }
}

/// The annotation types this reader supports working with.
///
/// Every other type found in a document is reported as [`AnnotationKind::Other`]
/// rather than dropped, so the Comments panel still shows something exists.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub enum AnnotationKind {
    /// Text markup that fills the quad area.
    Highlight,
    /// Text markup drawing a line under the quad.
    Underline,
    /// Text markup striking through the quad.
    StrikeOut,
    /// Text markup drawing a squiggly line under the quad.
    Squiggly,
    /// A rectangle (or ellipse) outline.
    Square,
    /// A pop-up note anchored to a point.
    StickyNote,
    /// Text drawn on the page in a box.
    FreeText,
    /// A stamp.
    Stamp,
    /// A hyperlink or destination.
    Link,
    /// Any type this reader does not handle specially.
    #[default]
    Other,
}

impl AnnotationKind {
    /// Whether the user can create this kind with a tool.
    pub const fn is_creatable(self) -> bool {
        matches!(
            self,
            Self::Highlight
                | Self::Underline
                | Self::StrikeOut
                | Self::Squiggly
                | Self::Square
                | Self::StickyNote
                | Self::FreeText
        )
    }

    /// Whether the annotation is created by dragging a rectangle.
    ///
    /// Sticky notes and typewriter text are placed by clicking instead.
    pub const fn is_drag_created(self) -> bool {
        matches!(
            self,
            Self::Highlight
                | Self::Underline
                | Self::StrikeOut
                | Self::Squiggly
                | Self::Square
                | Self::FreeText
        )
    }

    /// Short label for tool buttons and the Comments panel.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Highlight => "Highlight",
            Self::Underline => "Underline",
            Self::StrikeOut => "Strikeout",
            Self::Squiggly => "Squiggly",
            Self::Square => "Rectangle",
            Self::StickyNote => "Note",
            Self::FreeText => "Typewriter",
            Self::Stamp => "Stamp",
            Self::Link => "Link",
            Self::Other => "Annotation",
        }
    }
}

/// A description of a user tool: what kind of annotation it creates.
///
/// `Select` is the default: dragging pans/selects instead of drawing.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub enum Tool {
    /// No annotation tool active; drags do not draw.
    #[default]
    Select,
    /// Highlight markup over a dragged rectangle.
    Highlight,
    /// Underline markup over a dragged rectangle.
    Underline,
    /// Strikeout markup over a dragged rectangle.
    StrikeOut,
    /// Squiggly markup over a dragged rectangle.
    Squiggly,
    /// Rectangle outline over a dragged rectangle.
    Square,
    /// Sticky note placed by a click.
    StickyNote,
    /// Typewriter text box placed by a drag.
    FreeText,
}

impl Tool {
    /// Every tool, in toolbar order.
    pub const ALL: [Tool; 8] = [
        Tool::Select,
        Tool::Highlight,
        Tool::Underline,
        Tool::StrikeOut,
        Tool::Squiggly,
        Tool::Square,
        Tool::StickyNote,
        Tool::FreeText,
    ];

    /// The annotation kind this tool creates, if any.
    pub const fn annotation_kind(self) -> Option<AnnotationKind> {
        match self {
            Self::Select => None,
            Self::Highlight => Some(AnnotationKind::Highlight),
            Self::Underline => Some(AnnotationKind::Underline),
            Self::StrikeOut => Some(AnnotationKind::StrikeOut),
            Self::Squiggly => Some(AnnotationKind::Squiggly),
            Self::Square => Some(AnnotationKind::Square),
            Self::StickyNote => Some(AnnotationKind::StickyNote),
            Self::FreeText => Some(AnnotationKind::FreeText),
        }
    }

    /// Label for the tool button.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Select => "Select",
            Self::Highlight => "Highlight",
            Self::Underline => "Underline",
            Self::StrikeOut => "Strikeout",
            Self::Squiggly => "Squiggly",
            Self::Square => "Rectangle",
            Self::StickyNote => "Note",
            Self::FreeText => "Type",
        }
    }
}

/// A description of a new annotation to create.
///
/// Markup kinds (highlight and friends) take the rectangle the user dragged;
/// the engine turns it into the quad points those annotations need. Sticky
/// notes take the anchor point. Free text takes a rectangle to draw into.
#[derive(Clone, PartialEq, Debug)]
pub enum NewAnnotation {
    /// Highlight over a rectangle.
    Highlight(Rect),
    /// Underline over a rectangle.
    Underline(Rect),
    /// Strikeout over a rectangle.
    StrikeOut(Rect),
    /// Squiggly over a rectangle.
    Squiggly(Rect),
    /// Rectangle outline.
    Square(Rect),
    /// Sticky note anchored at a point.
    StickyNote((f32, f32)),
    /// Free text drawn into a rectangle.
    FreeText(Rect, String),
}

impl NewAnnotation {
    /// The page-space rectangle this annotation covers, when it has one.
    pub fn rect(&self) -> Option<Rect> {
        match self {
            Self::Highlight(r)
            | Self::Underline(r)
            | Self::StrikeOut(r)
            | Self::Squiggly(r)
            | Self::Square(r)
            | Self::FreeText(r, _) => Some(*r),
            Self::StickyNote(_) => None,
        }
    }

    /// The kind of annotation this describes.
    pub fn kind(&self) -> AnnotationKind {
        match self {
            Self::Highlight(_) => AnnotationKind::Highlight,
            Self::Underline(_) => AnnotationKind::Underline,
            Self::StrikeOut(_) => AnnotationKind::StrikeOut,
            Self::Squiggly(_) => AnnotationKind::Squiggly,
            Self::Square(_) => AnnotationKind::Square,
            Self::StickyNote(_) => AnnotationKind::StickyNote,
            Self::FreeText(_, _) => AnnotationKind::FreeText,
        }
    }
}

/// A flattened annotation: one annotation on one page.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct AnnotationInfo {
    /// Which annotation this is.
    pub id: AnnotationId,
    /// What kind of annotation it is.
    pub kind: AnnotationKind,
    /// Bounding rectangle in unrotated PDF page space.
    pub rect: Rect,
    /// Author-supplied text (note body, free-text contents), when present.
    pub contents: Option<String>,
}

impl AnnotationInfo {
    /// The contents to show in the Comments panel, or a fallback.
    pub fn display_contents(&self) -> &str {
        self.contents.as_deref().unwrap_or("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_tools_and_click_tools_are_distinct() {
        for tool in Tool::ALL {
            if matches!(tool, Tool::StickyNote | Tool::Select) {
                assert!(!tool.annotation_kind().is_some_and(AnnotationKind::is_drag_created));
            }
        }
        // The drag tools all report drag-created markup kinds.
        assert!(Tool::Highlight.annotation_kind().is_some_and(AnnotationKind::is_drag_created));
        assert!(Tool::Square.annotation_kind().is_some_and(AnnotationKind::is_drag_created));
        // Sticky notes are placed by click, not drag.
        assert!(!AnnotationKind::StickyNote.is_drag_created());
    }

    #[test]
    fn new_annotation_reports_its_kind_and_rect() {
        let rect = Rect::from_xywh(0.0, 0.0, 10.0, 10.0);
        assert_eq!(NewAnnotation::Highlight(rect).kind(), AnnotationKind::Highlight);
        assert_eq!(NewAnnotation::Square(rect).rect(), Some(rect));
        assert_eq!(NewAnnotation::StickyNote((5.0, 5.0)).rect(), None);
    }

    #[test]
    fn every_tool_has_a_unique_label() {
        let mut labels: Vec<&str> = Tool::ALL.iter().map(|t| t.label()).collect();
        labels.sort_unstable();
        let count = labels.len();
        labels.dedup();
        assert_eq!(labels.len(), count, "tool labels must be unique");
    }
}
