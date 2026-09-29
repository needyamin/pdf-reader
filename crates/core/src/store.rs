//! Application state and the reducer that advances it.

use std::path::PathBuf;

use crate::command::{Command, Effect, SidebarTab, TabId};
use crate::document::{Document, DocumentId};
use crate::form::FormInfo;
use crate::view::{ThemeId, ViewState, ZoomMode, clamp_zoom};

/// Multiplicative step for the zoom in and zoom out commands.
const ZOOM_STEP: f32 = 1.25;

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

            Command::CloseTab(id) => {
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
        let effects = s.dispatch(Command::CloseTab(t2));
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
        let effects = s.dispatch(Command::CloseTab(tab));
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
        s.dispatch(Command::CloseTab(tab));

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
