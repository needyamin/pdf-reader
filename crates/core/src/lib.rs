//! Domain model, commands and application state.
//!
//! This crate is deliberately free of any dependency on the PDF engine or the
//! UI toolkit. It owns: what a document is, what a tab is, what commands exist,
//! and how state changes in response to them.

pub mod annotation;
pub mod command;
pub mod document;
pub mod form;
pub mod rect;
pub mod store;
pub mod view;

pub use annotation::{AnnotationId, AnnotationInfo, AnnotationKind, NewAnnotation, Tool};
pub use command::{Command, Effect, SidebarTab, TabId};
pub use document::{Document, DocumentId, Outline, OutlineNode, PageGeometry, Rotation};
pub use form::{
    FieldId, FieldOption, FieldValue, FormFieldInfo, FormFieldType, FormInfo, FormKind,
};
pub use rect::Rect;
pub use store::{AppState, Store, Tab};
pub use view::{MAX_ZOOM, MIN_ZOOM, ThemeId, ViewMode, ViewState, ZoomMode, clamp_zoom};
