//! Domain model, commands and application state.
//!
//! This crate is deliberately free of any dependency on the PDF engine or the
//! UI toolkit. It owns: what a document is, what a tab is, what commands exist,
//! and how state changes in response to them.

pub mod annotation;
pub mod command;
pub mod document;
pub mod export;
pub mod form;
pub mod pageobj;
pub mod rect;
pub mod store;
pub mod view;

pub use annotation::{
    AnnotationId, AnnotationInfo, AnnotationKind, DEFAULT_LINE_WEIGHT, LINE_WEIGHT_PRESETS,
    MAX_LINE_WEIGHT, MIN_LINE_WEIGHT, NewAnnotation, Tool, clamp_line_weight, fitted_line_rect,
};
pub use command::{Command, Effect, GuideTopic, SidebarTab, TabId};
pub use document::{Document, DocumentId, Outline, OutlineNode, PageGeometry, Rotation};
pub use export::{
    EXPORT_SCALE, ExportDpi, ExportTarget, ExportTask, IMAGE_POINTS_PER_PIXEL, IMAGE_SOURCE_DPI,
    ImageFormat, ImagePageSize, PageRange, pixel_size,
};
pub use form::{
    FieldId, FieldOption, FieldValue, FormFieldInfo, FormFieldType, FormInfo, FormKind,
};
pub use pageobj::{PageObjectId, PageObjectInfo, PageObjectKind, RestackTarget, TextFormat};
pub use rect::Rect;
pub use store::{AppState, Store, Tab};
pub use view::{MAX_ZOOM, MIN_ZOOM, ThemeId, ViewMode, ViewState, ZoomMode, clamp_zoom};
