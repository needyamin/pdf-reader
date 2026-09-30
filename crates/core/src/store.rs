//! Application state and the reducer that advances it.

use std::path::PathBuf;

use crate::command::{Command, Effect, SidebarTab, TabId};
use crate::document::{Document, DocumentId};
use crate::form::FormInfo;
use crate::view::{ThemeId, ViewState, ZoomMode, clamp_zoom};

/// Multiplicative step for the zoom in and zoom out commands.
const ZOOM_STEP: f32 = 1.25;

/// One reversible annotation operation.
///
/// Ids are positional, so an *add* is recorded with its id unresolved and
/// filled in when the engine's fresh list arrives; a *delete* records the full
/// info so it can be re-created verbatim.
#[derive(Clone, PartialEq, Debug)]
pub enum UndoAction {
    /// An annotation was created; undo means deleting it.
    Add {
        /// Page it was drawn on.
        page: u32,
        /// Resolved once the fresh list arrives.
        id: Option<crate::annotation::AnnotationId>,
        /// The list before the add, for identity.
        before: Vec<crate::annotation::AnnotationInfo>,
    },
    /// An annotation was deleted; undo means re-creating it.
    Delete {
        /// Everything needed to rebuild it.
        info: crate::annotation::AnnotationInfo,
    },
    /// Annotation text was rewritten; undo means restoring the old text.
    Contents {
        /// Which annotation.
        id: crate::annotation::AnnotationId,
        /// The text before the edit.
        previous: Option<String>,
    },
}

/// A single open document and how it is being viewed.
#[derive(Clone, PartialEq, Debug)]
pub struct Tab {
    /// Tab identity.
    pub id: TabId,
    /// File this tab was opened from. Kept so a tab can still show which file
    /// it refers to while loading, or after a failed load.
    pub path: PathBuf,
    /// The document, present once loading has succeeded.
    pub document: Option<Document>,
    /// The document's interactive form, once the engine has reported it.
    ///
    /// `None` means "not read yet"; an empty [`FormInfo`] means "read, and this
    /// document has no form". The UI needs to tell those apart, otherwise every
    /// document would flicker through a "no form fields" state on load.
    pub form: Option<FormInfo>,
    /// How the document is being viewed.
    pub view: ViewState,
    /// True while the engine is opening the file.
    pub loading: bool,
    /// Failure reason, when the load failed.
    pub error: Option<String>,
    /// Whether the document has changes that have not been written to disk.
    ///
    /// Every mutating command sets this; saving clears it. Session state is
    /// not covered — persisting the viewport is not an edit.
    pub dirty: bool,
    /// The document's annotations, once the engine has listed them.
    ///
    /// `None` means "not read yet"; an empty vec means "read, and there are
    /// none". The Comments panel distinguishes the two the same way the Forms
    /// panel does.
    pub annotations: Option<Vec<crate::annotation::AnnotationInfo>>,
    /// Reversible annotation operations, newest last. Undo pops one.
    pub undo_stack: Vec<UndoAction>,
}

impl Tab {
    /// Number of pages, or 0 while still loading.
    pub fn page_count(&self) -> u32 {
        self.document.as_ref().map_or(0, Document::page_count)
    }

    /// The document's form, if it has been read and is non-empty.
    pub fn form(&self) -> Option<&FormInfo> {
        self.form.as_ref().filter(|f| !f.is_empty())
    }

    /// The currently selected field, if the selection still resolves.
    pub fn selected_field(&self) -> Option<&crate::form::FormFieldInfo> {
        let id = self.view.selected_field?;
        self.form.as_ref()?.field(id)
    }
}

/// The whole application state. Serializable, so session restore is a load.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct AppState {
    /// Open tabs, in display order.
    pub tabs: Vec<Tab>,
    /// The active tab, if any.
    pub active_tab: Option<TabId>,
    /// Active colour theme.
    pub theme: ThemeId,
    /// Whether the sidebar is shown.
    pub sidebar_visible: bool,
    /// Which sidebar panel is selected.
    pub sidebar_tab: SidebarTab,
    /// Whether the window is fullscreen.
    pub fullscreen: bool,
    /// The active annotation tool. Not persisted: a reader opens in select
    /// mode, and a tool chosen last session means nothing this session.
    pub tool: crate::annotation::Tool,
}

impl AppState {
    /// Borrow the active tab.
    pub fn active(&self) -> Option<&Tab> {
        let id = self.active_tab?;
        self.tabs.iter().find(|t| t.id == id)
    }

    /// Mutably borrow the active tab.
    pub fn active_mut(&mut self) -> Option<&mut Tab> {
        let id = self.active_tab?;
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    /// Borrow a tab by id.
    pub fn tab(&self, id: TabId) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == id)
    }
}

/// Owns [`AppState`] and is the only thing allowed to change it.
/// How many annotation operations stay reversible.
const UNDO_LIMIT: usize = 64;

#[derive(Clone, Debug, Default)]
pub struct Store {
    state: AppState,
    next_tab_id: u64,
    next_doc_id: u64,
}

impl Store {
    /// Create an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Read the current state.
    pub const fn state(&self) -> &AppState {
        &self.state
    }

    /// Allocate the next document id.
    fn next_document_id(&mut self) -> DocumentId {
        self.next_doc_id += 1;
        DocumentId::from_raw(self.next_doc_id)
    }

    /// Apply a command and return the side effects the shell must run.
    ///
    /// Pure with respect to observable state: no IO happens here.
    pub fn dispatch(&mut self, command: Command) -> Vec<Effect> {
        match command {
            // The shell handles the dialog and search; neither changes reducer
            // state directly. Search runs against the active engine handle.
            Command::ShowOpenDialog | Command::Search(_) => Vec::new(),

            Command::FormFieldsLoaded { doc, form } => {
                if let Some(tab) = self
                    .state
                    .tabs
                    .iter_mut()
                    .find(|t| t.document.as_ref().is_some_and(|d| d.id == doc))
                {
                    tab.form = Some(form);
                }
                Vec::new()
            }

            Command::SelectFormField(id) => {
                let Some(tab) = self.state.active_mut() else {
                    return Vec::new();
                };
                tab.view.selected_field = id;

                // Selecting a field should bring it into view, the same way
                // clicking an outline entry jumps to its page.
                let Some(field_id) = id else {
                    return Vec::new();
                };
                let Some(page) = tab
                    .form
                    .as_ref()
                    .and_then(|form| form.field(field_id))
                    .map(|field| field.id.page)
                else {
                    return Vec::new();
                };
                self.dispatch(Command::GoToPage(page))
            }

            Command::SetFormFieldValue { id, value } => {
                let Some(tab) = self.state.active_mut() else {
                    return Vec::new();
                };
                let Some(doc) = tab.document.as_ref().map(|d| d.id) else {
                    return Vec::new();
                };
                // Radio buttons can be selected but not cleared (the engine has
                // no uncheck), and push buttons carry no value: filter both out
                // here so the UI never emits a write that must fail.
                let writable = tab
                    .form
                    .as_ref()
                    .and_then(|form| form.field(id))
                    .is_some_and(|field| match value {
                        crate::form::FieldValue::Checked(false) => {
                            field.kind != crate::form::FormFieldType::RadioButton
                        }
                        crate::form::FieldValue::Checked(true)
                        | crate::form::FieldValue::Text(_)
                        | crate::form::FieldValue::Choice(_)
                        | crate::form::FieldValue::Empty => field.is_editable(),
                    });
                if !writable {
                    return Vec::new();
                }

                let changed = tab
                    .form
                    .as_mut()
                    .is_some_and(|form| form.set_value(id, value.clone()));
                if !changed {
                    return Vec::new();
                }
                tab.dirty = true;
                vec![
                    Effect::SetField {
                        doc,
                        id,
                        value,
                    },
                    Effect::InvalidateTiles,
                ]
            }

            Command::SetTool(tool) => {
                self.state.tool = tool;
                Vec::new()
            }

            Command::AnnotationsLoaded { doc, annotations } => {
                if let Some(tab) = self
                    .state
                    .tabs
                    .iter_mut()
                    .find(|t| t.document.as_ref().is_some_and(|d| d.id == doc))
                {
                    let still_there = tab
                        .view
                        .selected_annotation
                        .is_none_or(|sel| annotations.iter().any(|a| a.id == sel));
                    if !still_there {
                        tab.view.selected_annotation = None;
                    }
                    // A just-created annotation has no id until now: the last
                    // listed annotation on the edited page is it (PDFium
                    // appends; popups are hidden from this list).
                    if let Some(last) = tab.undo_stack.last_mut() {
                        if let UndoAction::Add { page, id, .. } = last {
                            if id.is_none() {
                                *id = annotations
                                    .iter()
                                    .filter(|a| a.id.page == *page)
                                    .map(|a| a.id.annot_index)
                                    .max()
                                    .map(|idx| crate::annotation::AnnotationId::new(*page, idx));
                            }
                        }
                    }
                    tab.annotations = Some(annotations);
                }
                Vec::new()
            }

            Command::AnnotationsLoadFailed { doc } => {
                // Only the never-read state is a stuck panel; a failed edit on
                // an existing list must not wipe what the user is looking at.
                if let Some(tab) = self
                    .state
                    .tabs
                    .iter_mut()
                    .find(|t| t.document.as_ref().is_some_and(|d| d.id == doc))
                {
                    if tab.annotations.is_none() {
                        tab.annotations = Some(Vec::new());
                    }
                }
                Vec::new()
            }

            Command::SelectAnnotation(id) => {
                let Some(tab) = self.state.active_mut() else {
                    return Vec::new();
                };
                let Some(annotation_id) = id else {
                    tab.view.selected_annotation = None;
                    return Vec::new();
                };
                // Ids are positional and shift after a delete: a selection of
                // an id that no longer resolves is stale, not an error.
                let resolves = tab
                    .annotations
                    .as_ref()
                    .is_some_and(|list| list.iter().any(|a| a.id == annotation_id));
                if !resolves {
                    return Vec::new();
                }
                tab.view.selected_annotation = id;
                let Some(page) = tab
                    .annotations
                    .as_ref()
                    .and_then(|list| list.iter().find(|a| a.id == annotation_id))
                    .map(|a| a.id.page)
                else {
                    return Vec::new();
                };
                self.dispatch(Command::GoToPage(page))
            }

            // Creation is not optimistic about the list: the engine answers
            // with a fresh one, and that answer is where the new id comes from.
            Command::AddAnnotation { page, new } => {
                let Some(tab) = self.state.active() else {
                    return Vec::new();
                };
                let Some(doc) = tab.document.as_ref().map(|d| d.id) else {
                    return Vec::new();
                };
                if let Some(tab) = self.state.active_mut() {
                    tab.dirty = true;
                    tab.view.selected_annotation = None;
                    // The id is positional and unknown until the fresh list
                    // arrives; `AnnotationsLoaded` fills it in by diffing.
                    tab.undo_stack.push(UndoAction::Add {
                        page,
                        id: None,
                        before: tab.annotations.clone().unwrap_or_default(),
                    });
                    if tab.undo_stack.len() > UNDO_LIMIT {
                        tab.undo_stack.remove(0);
                    }
                }
                // Without invalidation the annotation exists only in PDFium's
                // memory: cached tiles keep the old pixels until a scroll.
                vec![
                    Effect::AddAnnotation { doc, page, new },
                    Effect::InvalidateTiles,
                ]
            }

            Command::DeleteAnnotation(id) => {
                let Some(tab) = self.state.active() else {
                    return Vec::new();
                };
                let Some(doc) = tab.document.as_ref().map(|d| d.id) else {
                    return Vec::new();
                };
                if let Some(tab) = self.state.active_mut() {
                    tab.dirty = true;
                    tab.view.selected_annotation = None;
                    if let Some(list) = tab.annotations.as_mut() {
                        if let Some(info) = list.iter().find(|a| a.id == id) {
                            tab.undo_stack.push(UndoAction::Delete { info: info.clone() });
                            if tab.undo_stack.len() > UNDO_LIMIT {
                                tab.undo_stack.remove(0);
                            }
                        }
                        list.retain(|a| a.id != id);
                    }
                }
                vec![
                    Effect::DeleteAnnotation { doc, id },
                    Effect::InvalidateTiles,
                ]
            }

            Command::SetAnnotationContents { id, contents } => {
                let Some(tab) = self.state.active_mut() else {
                    return Vec::new();
                };
                let Some(doc) = tab.document.as_ref().map(|d| d.id) else {
                    return Vec::new();
                };
                // A stale id after a delete must not write text into whichever
                // annotation shifted into its slot.
                let resolved = tab
                    .annotations
                    .as_mut()
                    .and_then(|list| list.iter_mut().find(|a| a.id == id));
                let Some(a) = resolved else {
                    return Vec::new();
                };
                let previous = a.contents.clone();
                a.contents = (!contents.is_empty()).then_some(contents.clone());
                tab.dirty = true;
                tab.undo_stack.push(UndoAction::Contents { id, previous });
                if tab.undo_stack.len() > UNDO_LIMIT {
                    tab.undo_stack.remove(0);
                }
                vec![
                    Effect::SetAnnotationContents {
                        doc,
                        id,
                        contents,
                    },
                    Effect::InvalidateTiles,
                ]
            }

            Command::Undo => {
                let Some(tab) = self.state.active_mut() else {
                    return Vec::new();
                };
                let Some(doc) = tab.document.as_ref().map(|d| d.id) else {
                    return Vec::new();
                };
                let Some(action) = tab.undo_stack.pop() else {
                    return Vec::new();
                };
                match action {
                    // Undoing a create = delete it. That delete records its own
                    // undo entry, so a second Undo re-creates the annotation.
                    UndoAction::Add { id: Some(id), .. } => {
                        self.dispatch(Command::DeleteAnnotation(id))
                    }
                    UndoAction::Add { id: None, .. } => {
                        // The fresh list never arrived; nothing to reverse.
                        Vec::new()
                    }
                    // Undoing a delete = re-create it verbatim.
                    UndoAction::Delete { info } => {
                        match crate::annotation::NewAnnotation::from_info(&info) {
                            Some(new) => {
                                if let Some(tab) = self.state.active_mut() {
                                    tab.dirty = true;
                                }
                                vec![
                                    Effect::AddAnnotation {
                                        doc,
                                        page: info.id.page,
                                        new,
                                    },
                                    Effect::InvalidateTiles,
                                ]
                            }
                            None => Vec::new(),
                        }
                    }
                    // Undoing a text edit = restore the previous text.
                    UndoAction::Contents { id, previous } => self.dispatch(
                        Command::SetAnnotationContents {
                            id,
                            contents: previous.unwrap_or_default(),
                        },
                    ),
                }
            }

            Command::SaveDocument => {
                let Some(tab) = self.state.active() else {
                    return Vec::new();
                };
                let Some(doc) = tab.document.as_ref().map(|d| d.id) else {
                    return Vec::new();
                };
                vec![Effect::SaveDocument {
                    doc,
                    path: tab.path.clone(),
                    flatten: false,
                }]
            }

            Command::SaveDocumentAs => Vec::new(),

            // Only the tab holding that document clears its flag, so a save
            // racing a tab switch cannot mark the wrong tab clean.
            Command::DocumentSaved { doc } => {
                if let Some(tab) = self
                    .state
                    .tabs
                    .iter_mut()
                    .find(|t| t.document.as_ref().is_some_and(|d| d.id == doc))
                {
                    tab.dirty = false;
                }
                Vec::new()
            }

            Command::SaveDocumentTo(path) => {
                let Some(tab) = self.state.active() else {
                    return Vec::new();
                };
                let Some(doc) = tab.document.as_ref().map(|d| d.id) else {
                    return Vec::new();
                };
                vec![Effect::SaveDocument {
                    doc,
                    path,
                    flatten: false,
                }]
            }

            Command::FlattenDocument => {
                let Some(tab) = self.state.active() else {
                    return Vec::new();
                };
                let Some(doc) = tab.document.as_ref().map(|d| d.id) else {
                    return Vec::new();
                };
                let path = tab.path.clone();
                if let Some(tab) = self.state.active_mut() {
                    tab.dirty = true;
                }
                vec![Effect::SaveDocument {
                    doc,
                    path,
                    flatten: true,
                }]
            }

            // A dirty tab asks before closing; a clean one closes directly, so
            // the common case does not grow a prompt.
            Command::RequestCloseTab(id) => match self.state.tab(id).is_some_and(|t| t.dirty) {
                true => vec![Effect::ConfirmClose { tab: id }],
                false => self.dispatch(Command::ConfirmCloseTab(id)),
            },

            Command::ConfirmCloseTab(id) => {
                let doc_id = self
                    .state
                    .tab(id)
                    .and_then(|t| t.document.as_ref())
                    .map(|d| d.id);
                self.state.tabs.retain(|t| t.id != id);
                if self.state.active_tab == Some(id) {
                    self.state.active_tab = self.state.tabs.last().map(|t| t.id);
                }
                match doc_id {
                    Some(doc) => vec![Effect::CloseDocument { doc }, Effect::PersistSession],
                    None => vec![Effect::PersistSession],
                }
            }

            Command::OpenPath(path) => {
                self.next_tab_id += 1;
                let id = TabId::from_raw(self.next_tab_id);
                self.state.tabs.push(Tab {
                    id,
                    path: path.clone(),
                    document: None,
                    form: None,
                    view: ViewState::default(),
                    loading: true,
                    error: None,
                    dirty: false,
                    annotations: None,
                    undo_stack: Vec::new(),
                });
                self.state.active_tab = Some(id);
                vec![Effect::OpenDocument { tab: id, path }]
            }

            Command::DocumentOpened { tab, mut document } => {
                document.id = self.next_document_id();
                if let Some(t) = self.state.tab_mut(tab) {
                    t.document = Some(document);
                    t.loading = false;
                    t.error = None;
                }
                vec![Effect::PersistSession, Effect::InvalidateTiles]
            }

            Command::DocumentFailed { tab, reason } => {
                if let Some(t) = self.state.tab_mut(tab) {
                    t.loading = false;
                    t.error = Some(reason);
                }
                Vec::new()
            }

            Command::DismissError => {
                if let Some(t) = self.state.active_mut() {
                    t.error = None;
                    t.loading = false;
                }
                Vec::new()
            }

            Command::ActivateTab(id) => {
                self.state.active_tab = Some(id);
                vec![Effect::InvalidateTiles]
            }

            Command::SetViewMode(mode) => self.with_view(|v| v.mode = mode),

            Command::SetZoom(mode) => self.with_view(|v| {
                v.zoom_mode = mode;
                if let Some(f) = mode.fixed_factor() {
                    v.zoom = clamp_zoom(f);
                }
            }),

            Command::ZoomIn => self.with_view(|v| {
                v.zoom = clamp_zoom(v.zoom * ZOOM_STEP);
                v.zoom_mode = ZoomMode::Fixed(v.zoom);
            }),

            Command::ZoomOut => self.with_view(|v| {
                v.zoom = clamp_zoom(v.zoom / ZOOM_STEP);
                v.zoom_mode = ZoomMode::Fixed(v.zoom);
            }),

            Command::ZoomReset => self.with_view(|v| {
                v.zoom = 1.0;
                v.zoom_mode = ZoomMode::Fixed(1.0);
            }),

            Command::ResolvedZoom(factor) => {
                let target = clamp_zoom(factor);
                match self.state.active_mut() {
                    // Only invalidate tiles when the factor actually moved, so a
                    // steady window does not churn the renderer every frame.
                    Some(tab) if (tab.view.zoom - target).abs() > 1e-4 => {
                        tab.view.zoom = target;
                        vec![Effect::InvalidateTiles]
                    }
                    _ => Vec::new(),
                }
            }

            Command::ViewportChanged {
                scroll_x,
                scroll_y,
                current_page,
            } => {
                if let Some(t) = self.state.active_mut() {
                    t.view.scroll_x = scroll_x.max(0.0);
                    t.view.scroll_y = scroll_y.max(0.0);
                    t.view.current_page = current_page;
                }
                Vec::new()
            }

            Command::ScrollBy { dx, dy } => {
                if let Some(t) = self.state.active_mut() {
                    t.view.scroll_x = (t.view.scroll_x + dx).max(0.0);
                    t.view.scroll_y = (t.view.scroll_y + dy).max(0.0);
                }
                Vec::new()
            }

            Command::RotateCw => self.with_view(|v| v.rotation = v.rotation.rotate_cw()),

            Command::RotateCcw => self.with_view(|v| v.rotation = v.rotation.rotate_ccw()),

            Command::GoToPage(page) => {
                let max = self.state.active().map_or(0, Tab::page_count);
                let target = if max == 0 { 0 } else { page.min(max - 1) };
                let mut effects = Vec::new();
                if let Some(t) = self.state.active_mut() {
                    t.view.current_page = target;
                }
                if let Some(id) = self.state.active_tab {
                    effects.push(Effect::ScrollToPage {
                        tab: id,
                        page: target,
                    });
                }
                effects
            }

            Command::NextPage => self.step_page(1),
            Command::PrevPage => self.step_page(-1),

            Command::ToggleSidebar => {
                self.state.sidebar_visible = !self.state.sidebar_visible;
                vec![Effect::PersistSession]
            }

            Command::SetSidebarTab(tab) => {
                self.state.sidebar_tab = tab;
                if !self.state.sidebar_visible {
                    self.state.sidebar_visible = true;
                }
                Vec::new()
            }

            Command::SetTheme(theme) => {
                self.state.theme = theme;
                vec![Effect::PersistSession]
            }

            Command::ToggleFullscreen => {
                self.state.fullscreen = !self.state.fullscreen;
                Vec::new()
            }
        }
    }

    /// Apply a change to the active tab's view state, invalidating tiles.
    fn with_view(&mut self, f: impl FnOnce(&mut ViewState)) -> Vec<Effect> {
        if let Some(tab) = self.state.active_mut() {
            f(&mut tab.view);
            vec![Effect::InvalidateTiles]
        } else {
            Vec::new()
        }
    }

    /// Move the active tab's page by `delta`, clamped to the document.
    fn step_page(&mut self, delta: i32) -> Vec<Effect> {
        let Some(tab) = self.state.active() else {
            return Vec::new();
        };
        let count = tab.page_count();
        if count == 0 {
            return Vec::new();
        }
        let current = i64::from(tab.view.current_page);
        let target = (current + i64::from(delta)).clamp(0, i64::from(count - 1)) as u32;
        self.dispatch(Command::GoToPage(target))
    }
}

impl AppState {
    /// Mutably borrow a tab by id.
    fn tab_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Document, Outline, PageGeometry, Rotation};
    use crate::form::{FieldId, FieldValue, FormFieldInfo, FormFieldType, FormKind};
    use crate::rect::Rect;
    use crate::view::ViewMode;
    use std::path::PathBuf;

    fn doc_with_pages(pages: u32) -> Document {
        Document {
            id: DocumentId::from_raw(0),
            path: PathBuf::from("x.pdf"),
            title: "x".into(),
            pages: vec![PageGeometry::A4; pages as usize],
            encrypted: false,
            outline: Outline::default(),
        }
    }

    fn opened_store(pages: u32) -> (Store, TabId) {
        let mut s = Store::new();
        let effects = s.dispatch(Command::OpenPath(PathBuf::from("x.pdf")));
        let Effect::OpenDocument { tab, .. } = effects[0].clone() else {
            panic!("expected OpenDocument");
        };
        s.dispatch(Command::DocumentOpened {
            tab,
            document: doc_with_pages(pages),
        });
        (s, tab)
    }

    #[test]
    fn opening_a_file_creates_a_loading_tab_and_asks_the_shell_to_load() {
        let mut s = Store::new();
        let effects = s.dispatch(Command::OpenPath(PathBuf::from("a.pdf")));
        assert_eq!(s.state().tabs.len(), 1);
        assert!(s.state().active().unwrap().loading);
        assert!(matches!(effects[0], Effect::OpenDocument { .. }));
    }

    #[test]
    fn document_opened_assigns_a_unique_id_and_stops_loading() {
        let (s, _) = opened_store(3);
        let tab = s.state().active().unwrap();
        assert!(!tab.loading);
        assert_eq!(tab.page_count(), 3);
        assert_eq!(tab.document.as_ref().unwrap().id.raw(), 1);
    }

    #[test]
    fn zoom_is_clamped() {
        let (mut s, _) = opened_store(1);
        for _ in 0..100 {
            s.dispatch(Command::ZoomIn);
        }
        assert!(s.state().active().unwrap().view.zoom <= 32.0);
        for _ in 0..200 {
            s.dispatch(Command::ZoomOut);
        }
        assert!(s.state().active().unwrap().view.zoom >= 0.1);
    }

    #[test]
    fn go_to_page_is_clamped_to_the_document() {
        let (mut s, _) = opened_store(5);
        s.dispatch(Command::GoToPage(99));
        assert_eq!(s.state().active().unwrap().view.current_page, 4);
    }

    #[test]
    fn next_and_prev_page_do_not_run_past_the_ends() {
        let (mut s, _) = opened_store(5);
        s.dispatch(Command::GoToPage(0));
        s.dispatch(Command::PrevPage);
        assert_eq!(s.state().active().unwrap().view.current_page, 0);
        s.dispatch(Command::GoToPage(4));
        s.dispatch(Command::NextPage);
        assert_eq!(s.state().active().unwrap().view.current_page, 4);
    }

    #[test]
    fn rotation_wraps_after_four_turns() {
        let (mut s, _) = opened_store(1);
        for _ in 0..4 {
            s.dispatch(Command::RotateCw);
        }
        assert_eq!(s.state().active().unwrap().view.rotation, Rotation::None);
        s.dispatch(Command::RotateCcw);
        assert_eq!(s.state().active().unwrap().view.rotation, Rotation::Cw270);
    }

    #[test]
    fn closing_the_active_tab_falls_back_to_another_tab() {
        let mut s = Store::new();
        let e1 = s.dispatch(Command::OpenPath(PathBuf::from("a.pdf")));
        let e2 = s.dispatch(Command::OpenPath(PathBuf::from("b.pdf")));
        let Effect::OpenDocument { tab: t1, .. } = e1[0].clone() else {
            panic!()
        };
        let Effect::OpenDocument { tab: t2, .. } = e2[0].clone() else {
            panic!()
        };
        assert_eq!(s.state().active_tab, Some(t2));
        let effects = s.dispatch(Command::ConfirmCloseTab(t2));
        assert_eq!(s.state().active_tab, Some(t1));
        assert!(effects.contains(&Effect::PersistSession));
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::CloseDocument { .. }))
        );
    }

    #[test]
    fn closing_a_loaded_tab_releases_its_document() {
        let (mut s, tab) = opened_store(2);
        let effects = s.dispatch(Command::ConfirmCloseTab(tab));
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::CloseDocument { .. }))
        );
        assert!(s.state().tabs.is_empty());
    }

    #[test]
    fn view_mode_and_theme_changes_are_recorded() {
        let (mut s, _) = opened_store(2);
        s.dispatch(Command::SetViewMode(ViewMode::Single));
        assert_eq!(s.state().active().unwrap().view.mode, ViewMode::Single);
        s.dispatch(Command::SetTheme(ThemeId::Forest));
        assert_eq!(s.state().theme, ThemeId::Forest);
    }

    #[test]
    fn resolved_zoom_updates_the_factor_and_only_invalidates_on_change() {
        let (mut s, _) = opened_store(2);

        let effects = s.dispatch(Command::ResolvedZoom(1.5));
        assert!((s.state().active().unwrap().view.zoom - 1.5).abs() < 1e-6);
        assert!(effects.iter().any(|e| matches!(e, Effect::InvalidateTiles)));

        // Same factor again: no work, no tile invalidation.
        let effects = s.dispatch(Command::ResolvedZoom(1.5));
        assert!(effects.is_empty());
    }

    #[test]
    fn resolved_zoom_is_clamped() {
        let (mut s, _) = opened_store(1);
        s.dispatch(Command::ResolvedZoom(1000.0));
        assert!(s.state().active().unwrap().view.zoom <= 32.0);
        s.dispatch(Command::ResolvedZoom(0.0001));
        assert!(s.state().active().unwrap().view.zoom >= 0.1);
    }

    #[test]
    fn viewport_changes_do_not_invalidate_tiles() {
        let (mut s, _) = opened_store(2);
        let effects = s.dispatch(Command::ViewportChanged {
            scroll_x: 24.0,
            scroll_y: 120.0,
            current_page: 1,
        });
        assert!(effects.is_empty());
        assert_eq!(s.state().active().unwrap().view.scroll_x, 24.0);
        assert_eq!(s.state().active().unwrap().view.scroll_y, 120.0);
        assert_eq!(s.state().active().unwrap().view.current_page, 1);
    }

    /// A form arrives keyed by document id, because the engine answers long
    /// after the request was made and the tab may have moved on.
    #[test]
    fn form_fields_are_attached_to_the_tab_holding_that_document() {
        let (mut s, tab) = opened_store(2);
        let doc = s.state().tab(tab).unwrap().document.as_ref().unwrap().id;
        assert!(s.state().tab(tab).unwrap().form.is_none());

        let form = FormInfo {
            kind: FormKind::Acrobat,
            fields: vec![sample_field(0)],
        };
        s.dispatch(Command::FormFieldsLoaded { doc, form });
        assert_eq!(s.state().tab(tab).unwrap().form().unwrap().fields.len(), 1);
    }

    #[test]
    fn a_late_form_response_for_a_closed_tab_is_ignored() {
        let (mut s, tab) = opened_store(1);
        let doc = s.state().tab(tab).unwrap().document.as_ref().unwrap().id;
        s.dispatch(Command::ConfirmCloseTab(tab));

        let form = FormInfo {
            kind: FormKind::Acrobat,
            fields: vec![sample_field(0)],
        };
        // Must not panic or resurrect the tab.
        s.dispatch(Command::FormFieldsLoaded { doc, form });
        assert!(s.state().tabs.is_empty());
    }

    /// An empty form is a real answer, not "not read yet" — the UI shows
    /// different copy for each.
    #[test]
    fn an_empty_form_is_distinguished_from_an_unread_one() {
        let (mut s, tab) = opened_store(1);
        let doc = s.state().tab(tab).unwrap().document.as_ref().unwrap().id;
        assert!(s.state().tab(tab).unwrap().form.is_none());

        s.dispatch(Command::FormFieldsLoaded {
            doc,
            form: FormInfo::default(),
        });
        // Read, but empty: `form()` reports None while `form` is Some.
        assert!(s.state().tab(tab).unwrap().form.is_some());
        assert!(s.state().tab(tab).unwrap().form().is_none());
    }

    #[test]
    fn selecting_a_field_records_it_and_brings_its_page_into_view() {
        let (mut s, tab) = opened_store(5);
        let doc = s.state().tab(tab).unwrap().document.as_ref().unwrap().id;
        let mut field = sample_field(3);
        field.id = FieldId::new(3, 1);
        s.dispatch(Command::FormFieldsLoaded {
            doc,
            form: FormInfo {
                kind: FormKind::Acrobat,
                fields: vec![field],
            },
        });

        let effects = s.dispatch(Command::SelectFormField(Some(FieldId::new(3, 1))));
        let tab = s.state().active().unwrap();
        assert_eq!(tab.view.selected_field, Some(FieldId::new(3, 1)));
        assert_eq!(tab.view.current_page, 3);
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::ScrollToPage { page: 3, .. }
        )));

        // Clearing the selection keeps the page where it is.
        s.dispatch(Command::SelectFormField(None));
        assert_eq!(s.state().active().unwrap().view.selected_field, None);
        assert_eq!(s.state().active().unwrap().view.current_page, 3);
    }

    fn sample_field(page: u32) -> FormFieldInfo {
        FormFieldInfo {
            id: FieldId::new(page, 0),
            name: "name".into(),
            alternate_name: None,
            rect: Rect::from_xywh(10.0, 10.0, 100.0, 20.0),
            kind: FormFieldType::Text,
            value: FieldValue::Empty,
            options: Vec::new(),
            read_only: false,
            required: false,
            multiline: false,
        }
    }

    #[test]
    fn setting_a_field_value_updates_the_form_and_marks_the_tab_dirty() {
        let (mut s, tab) = opened_store(2);
        let doc = s.state().tab(tab).unwrap().document.as_ref().unwrap().id;
        let mut field = sample_field(0);
        field.id = FieldId::new(0, 0);
        s.dispatch(Command::FormFieldsLoaded {
            doc,
            form: FormInfo {
                kind: FormKind::Acrobat,
                fields: vec![field],
            },
        });

        let effects = s.dispatch(Command::SetFormFieldValue {
            id: FieldId::new(0, 0),
            value: FieldValue::Text("typed".into()),
        });
        let state = s.state();
        assert_eq!(
            state.tab(tab).unwrap().form().unwrap().field(FieldId::new(0, 0)).unwrap().value,
            FieldValue::Text("typed".into())
        );
        assert!(state.tab(tab).unwrap().dirty);
        assert!(effects.iter().any(|e| matches!(e, Effect::SetField { .. })));
        assert!(effects.iter().any(|e| matches!(e, Effect::InvalidateTiles)));

        // The same value again changes nothing: no engine write, no dirty flag
        // churn while the user merely re-renders the panel.
        let effects = s.dispatch(Command::SetFormFieldValue {
            id: FieldId::new(0, 0),
            value: FieldValue::Text("typed".into()),
        });
        assert!(effects.is_empty());
    }

    #[test]
    fn a_dirty_tab_asks_before_closing_and_a_clean_one_does_not() {
        let (mut s, tab) = opened_store(1);
        s.dispatch(Command::DocumentOpened {
            tab,
            document: doc_with_pages(1),
        });

        // Clean tab: closes immediately, no prompt.
        let effects = s.dispatch(Command::RequestCloseTab(tab));
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::ConfirmClose { .. })),
            "a clean tab must not prompt"
        );
        assert!(s.state().tabs.is_empty());

        // Dirty tab: prompt instead of closing; only the confirm closes it.
        let (mut s, tab) = opened_store(1);
        s.dispatch(Command::DocumentOpened {
            tab,
            document: doc_with_pages(1),
        });
        if let Some(t) = s.state.tabs.iter_mut().find(|t| t.id == tab) {
            t.dirty = true;
        }
        let effects = s.dispatch(Command::RequestCloseTab(tab));
        assert!(effects.iter().any(|e| matches!(e, Effect::ConfirmClose { .. })));
        assert_eq!(s.state().tabs.len(), 1, "the tab must still be open");

        s.dispatch(Command::ConfirmCloseTab(tab));
        assert!(s.state().tabs.is_empty());
    }

    #[test]
    fn document_saved_clears_only_the_tab_holding_that_document() {
        let (mut s, tab) = opened_store(1);
        let doc = s.state().tab(tab).unwrap().document.as_ref().unwrap().id;
        s.state.tabs[0].dirty = true;

        s.dispatch(Command::DocumentSaved { doc });
        assert!(!s.state.tab(tab).unwrap().dirty);

        // A save for some other document must not clean this tab.
        s.state.tabs[0].dirty = true;
        s.dispatch(Command::DocumentSaved {
            doc: DocumentId::from_raw(999),
        });
        assert!(s.state.tab(tab).unwrap().dirty);
    }

    /// The annotation tool appeared dead because mutations never invalidated
    /// the tile cache: PDFium updated, the page pixels did not. Locked in.
    #[test]
    fn annotation_mutations_invalidate_tiles() {
        let (mut s, tab) = opened_store(1);
        let doc = s.state().tab(tab).unwrap().document.as_ref().unwrap().id;
        s.state.tabs[0].annotations = Some(Vec::new());

        let effects = s.dispatch(Command::AddAnnotation {
            page: 0,
            new: crate::annotation::NewAnnotation::Highlight(Rect::from_xywh(
                0.0, 0.0, 10.0, 10.0,
            )),
        });
        assert!(
            effects.iter().any(|e| matches!(e, Effect::InvalidateTiles)),
            "AddAnnotation must invalidate tiles"
        );

        s.state.tabs[0].annotations = Some(vec![crate::annotation::AnnotationInfo {
            id: crate::annotation::AnnotationId::new(0, 0),
            kind: crate::annotation::AnnotationKind::Highlight,
            rect: Rect::from_xywh(0.0, 0.0, 10.0, 10.0),
            contents: None,
        }]);
        let effects = s.dispatch(Command::SetAnnotationContents {
            id: crate::annotation::AnnotationId::new(0, 0),
            contents: "hello".into(),
        });
        assert!(
            effects.iter().any(|e| matches!(e, Effect::InvalidateTiles)),
            "SetAnnotationContents must invalidate tiles"
        );

        let effects = s.dispatch(Command::DeleteAnnotation(crate::annotation::AnnotationId::new(
            0, 0,
        )));
        assert!(
            effects.iter().any(|e| matches!(e, Effect::InvalidateTiles)),
            "DeleteAnnotation must invalidate tiles"
        );
    }

    /// A stale id must never select or write into whichever annotation shifted
    /// into its slot after a delete.
    #[test]
    fn stale_annotation_ids_are_ignored() {
        let (mut s, tab) = opened_store(1);
        let doc = s.state().tab(tab).unwrap().document.as_ref().unwrap().id;
        let annotations = vec![
            crate::annotation::AnnotationInfo {
                id: crate::annotation::AnnotationId::new(0, 0),
                kind: crate::annotation::AnnotationKind::Highlight,
                rect: Rect::from_xywh(0.0, 0.0, 10.0, 10.0),
                contents: None,
            },
            crate::annotation::AnnotationInfo {
                id: crate::annotation::AnnotationId::new(0, 1),
                kind: crate::annotation::AnnotationKind::StickyNote,
                rect: Rect::from_xywh(20.0, 20.0, 30.0, 30.0),
                contents: Some("note".into()),
            },
        ];
        s.state.tabs[0].annotations = Some(annotations);

        // Selecting an id that does not resolve is a no-op, not a selection.
        s.dispatch(Command::SelectAnnotation(Some(crate::annotation::AnnotationId::new(
            0, 99,
        ))));
        assert_eq!(s.state.active().unwrap().view.selected_annotation, None);

        // A valid selection sticks.
        s.dispatch(Command::SelectAnnotation(Some(crate::annotation::AnnotationId::new(0, 1))));
        assert_eq!(
            s.state.active().unwrap().view.selected_annotation,
            Some(crate::annotation::AnnotationId::new(0, 1))
        );

        // Editing a stale id writes nothing and emits nothing.
        let effects = s.dispatch(Command::SetAnnotationContents {
            id: crate::annotation::AnnotationId::new(0, 99),
            contents: "wrong target".into(),
        });
        assert!(effects.is_empty());
        assert_eq!(
            s.state.active().unwrap().annotations.as_ref().unwrap()[1]
                .contents
                .as_deref(),
            Some("note"),
            "the live annotation must be untouched"
        );

        // A fresh list that no longer contains the selection clears it, so a
        // delete can never leave the highlight on the wrong row.
        let shifted = vec![crate::annotation::AnnotationInfo {
            id: crate::annotation::AnnotationId::new(0, 0),
            kind: crate::annotation::AnnotationKind::StickyNote,
            rect: Rect::from_xywh(20.0, 20.0, 30.0, 30.0),
            contents: Some("note".into()),
        }];
        s.dispatch(Command::AnnotationsLoaded {
            doc,
            annotations: shifted,
        });
        assert_eq!(s.state.active().unwrap().view.selected_annotation, None);
    }

    /// An annotation failure before the first read must not leave the Comments
    /// panel stuck on "Reading annotations…" forever.
    #[test]
    fn a_failed_annotation_read_unsticks_the_panel() {
        let (mut s, tab) = opened_store(1);
        let doc = s.state().tab(tab).unwrap().document.as_ref().unwrap().id;
        assert!(s.state.tab(tab).unwrap().annotations.is_none());

        s.dispatch(Command::AnnotationsLoadFailed { doc });
        assert_eq!(
            s.state.tab(tab).unwrap().annotations,
            Some(Vec::new()),
            "never-read must become read-but-empty"
        );

        // A failure on an existing list must NOT wipe it.
        s.state.tabs[0].annotations = Some(vec![crate::annotation::AnnotationInfo {
            id: crate::annotation::AnnotationId::new(0, 0),
            kind: crate::annotation::AnnotationKind::Highlight,
            rect: Rect::from_xywh(0.0, 0.0, 10.0, 10.0),
            contents: None,
        }]);
        s.dispatch(Command::AnnotationsLoadFailed { doc });
        assert_eq!(
            s.state.tab(tab).unwrap().annotations.as_ref().unwrap().len(),
            1,
            "an existing list survives a failed operation"
        );
    }

    /// Undo of a create deletes it; the id is resolved when the fresh list
    /// arrives, not when the undo entry is recorded.
    #[test]
    fn undo_deletes_the_annotation_that_was_just_added() {
        let (mut s, tab) = opened_store(1);
        let doc = s.state().tab(tab).unwrap().document.as_ref().unwrap().id;
        s.state.tabs[0].annotations = Some(Vec::new());

        s.dispatch(Command::AddAnnotation {
            page: 0,
            new: crate::annotation::NewAnnotation::Highlight(Rect::from_xywh(
                0.0, 0.0, 10.0, 10.0,
            )),
        });
        assert_eq!(s.state.tab(tab).unwrap().undo_stack.len(), 1);
        assert!(
            matches!(
                s.state.tab(tab).unwrap().undo_stack[0],
                UndoAction::Add { id: None, .. }
            ),
            "the id is not known yet"
        );

        // The fresh list arrives: the new annotation resolves onto the entry.
        let annotations = vec![crate::annotation::AnnotationInfo {
            id: crate::annotation::AnnotationId::new(0, 0),
            kind: crate::annotation::AnnotationKind::Highlight,
            rect: Rect::from_xywh(0.0, 0.0, 10.0, 10.0),
            contents: None,
        }];
        s.dispatch(Command::AnnotationsLoaded {
            doc,
            annotations: annotations.clone(),
        });
        let resolved = match s.state.tab(tab).unwrap().undo_stack[0] {
            UndoAction::Add { id: Some(id), .. } => Some(id),
            _ => None,
        };
        assert_eq!(
            resolved,
            Some(crate::annotation::AnnotationId::new(0, 0)),
            "the id resolved onto the entry"
        );

        let effects = s.dispatch(Command::Undo);
        let deleted = effects.iter().find_map(|e| match e {
            Effect::DeleteAnnotation { id, .. } => Some(*id),
            _ => None,
        });
        assert_eq!(
            deleted,
            Some(crate::annotation::AnnotationId::new(0, 0)),
            "undo of an add must delete it, got {effects:?}"
        );
        // The delete recorded its own undo entry: a second Undo re-creates.
        assert!(matches!(
            s.state.tab(tab).unwrap().undo_stack.last(),
            Some(UndoAction::Delete { .. })
        ));
    }

    /// Undo of a delete re-creates the exact annotation.
    #[test]
    fn undo_recreates_a_deleted_annotation() {
        let (mut s, tab) = opened_store(1);
        s.state.tabs[0].annotations = Some(vec![crate::annotation::AnnotationInfo {
            id: crate::annotation::AnnotationId::new(0, 3),
            kind: crate::annotation::AnnotationKind::StickyNote,
            rect: Rect::from_xywh(30.0, 40.0, 48.0, 48.0),
            contents: Some("kept text".into()),
        }]);

        let effects = s.dispatch(Command::DeleteAnnotation(crate::annotation::AnnotationId::new(
            0, 3,
        )));
        assert!(s.state.tab(tab).unwrap().annotations.as_ref().unwrap().is_empty());

        let effects = s.dispatch(Command::Undo);
        let recreated = effects.iter().find_map(|e| match e {
            Effect::AddAnnotation { page, new, .. } => Some((*page, new.clone())),
            _ => None,
        });
        assert_eq!(
            recreated,
            Some((
                0,
                crate::annotation::NewAnnotation::StickyNote(
                    (30.0, 40.0),
                    "kept text".into()
                )
            )),
            "undo of a delete must re-create it with its text, got {effects:?}"
        );
    }

    /// Undo of a text edit restores the previous text.
    #[test]
    fn undo_restores_previous_contents() {
        let (mut s, _tab) = opened_store(1);
        s.state.tabs[0].annotations = Some(vec![crate::annotation::AnnotationInfo {
            id: crate::annotation::AnnotationId::new(0, 0),
            kind: crate::annotation::AnnotationKind::FreeText,
            rect: Rect::from_xywh(0.0, 0.0, 100.0, 20.0),
            contents: Some("before".into()),
        }]);

        s.dispatch(Command::SetAnnotationContents {
            id: crate::annotation::AnnotationId::new(0, 0),
            contents: "after".into(),
        });
        assert_eq!(
            s.state.active().unwrap().annotations.as_ref().unwrap()[0]
                .contents
                .as_deref(),
            Some("after")
        );

        let effects = s.dispatch(Command::Undo);
        let restored = effects.iter().find_map(|e| match e {
            Effect::SetAnnotationContents { contents, .. } => Some(contents.clone()),
            _ => None,
        });
        assert_eq!(
            restored,
            Some("before".to_string()),
            "undo must restore the previous text, got {effects:?}"
        );
        assert_eq!(
            s.state.active().unwrap().annotations.as_ref().unwrap()[0]
                .contents
                .as_deref(),
            Some("before"),
            "the optimistic state is restored too"
        );
    }

    #[test]
    fn keyboard_scroll_is_clamped_and_recorded() {
        let (mut s, _) = opened_store(2);
        s.dispatch(Command::ScrollBy { dx: 12.0, dy: 96.0 });
        assert_eq!(s.state().active().unwrap().view.scroll_x, 12.0);
        assert_eq!(s.state().active().unwrap().view.scroll_y, 96.0);
        s.dispatch(Command::ScrollBy {
            dx: -100.0,
            dy: -200.0,
        });
        assert_eq!(s.state().active().unwrap().view.scroll_x, 0.0);
        assert_eq!(s.state().active().unwrap().view.scroll_y, 0.0);
    }
}
