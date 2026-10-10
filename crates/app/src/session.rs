//! Remembering the open tabs between runs.
//!
//! The reader reopens whatever was open last time — the same files, each at the
//! scroll position, zoom, page and rotation it was left at — until a tab is
//! closed, which drops it for good. Only the file path and the per-tab
//! [`ViewState`] are stored; the documents themselves are re-read from disk, so
//! a session file never goes stale.

use std::path::{Path, PathBuf};

use pdfreader_core::ViewState;
use serde::{Deserialize, Serialize};

/// One remembered tab.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct SessionTab {
    /// File the tab was showing.
    pub path: PathBuf,
    /// Where in the document it was left.
    pub view: ViewState,
}

/// The tabs to reopen on the next launch.
#[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
pub struct Session {
    /// Tabs in display order.
    pub tabs: Vec<SessionTab>,
    /// Index into [`Session::tabs`] of the tab that was active.
    pub active: Option<usize>,
}

impl Session {
    /// Drop tabs whose file has gone, so a deleted document does not come back
    /// as an error tab on every launch.
    pub fn retaining_existing_files(self) -> Self {
        self.retaining(|path| path.is_file())
    }

    /// Drop tabs for which `exists` is false, for the reason above.
    pub fn retaining(mut self, exists: impl Fn(&Path) -> bool) -> Self {
        self.tabs.retain(|tab| exists(&tab.path));
        // Keep the active index in range: a removed tab must not point past the
        // end, and an empty session has no active tab at all.
        if self.active.is_some_and(|index| index >= self.tabs.len()) {
            self.active = self.tabs.len().checked_sub(1);
        }
        self
    }
}

/// Where the session is stored, beside `settings.json`.
fn session_file() -> Option<PathBuf> {
    Some(crate::config_dir()?.join("session.json"))
}

/// Load the previous session. A missing or unreadable file is an empty session,
/// which is exactly what a first run wants.
pub fn load() -> Session {
    session_file().map_or_else(Session::default, |path| load_from(&path))
}

/// Write the session to disk (best effort).
pub fn save(session: &Session) {
    if let Some(path) = session_file() {
        save_to(&path, session);
    }
}

/// Load a session from `path`; anything unreadable is an empty session.
fn load_from(path: &Path) -> Session {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Session::default();
    };
    match serde_json::from_str(&text) {
        Ok(session) => session,
        Err(error) => {
            // A corrupt session is not worth keeping a copy of the way settings
            // are: it is only a list of files, and the next save replaces it.
            tracing::warn!("session file could not be parsed ({error}); starting fresh");
            Session::default()
        }
    }
}

/// Write `session` to `path`, creating the directory if needed.
fn save_to(path: &Path, session: &Session) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(session) {
        let _ = std::fs::write(path, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfreader_core::{Rotation, ViewMode, ZoomMode};

    fn tab(path: &str) -> SessionTab {
        SessionTab {
            path: PathBuf::from(path),
            view: ViewState::default(),
        }
    }

    #[test]
    fn a_session_round_trips_through_json() {
        let session = Session {
            tabs: vec![tab("a.pdf"), tab("b.pdf")],
            active: Some(1),
        };
        let text = serde_json::to_string(&session).expect("serialise");
        let back: Session = serde_json::from_str(&text).expect("deserialise");
        assert_eq!(back, session);
    }

    /// A remembered view must survive the round trip — that is the whole point
    /// of "open on the same location".
    #[test]
    fn a_remembered_view_survives() {
        let view = ViewState {
            zoom_mode: ZoomMode::Fixed(1.75),
            zoom: 1.75,
            scroll_x: 12.5,
            scroll_y: 340.0,
            rotation: Rotation::Cw90,
            mode: ViewMode::Single,
            current_page: 4,
            ..ViewState::default()
        };
        let session = Session {
            tabs: vec![SessionTab {
                path: PathBuf::from("a.pdf"),
                view,
            }],
            active: Some(0),
        };
        let text = serde_json::to_string(&session).expect("serialise");
        let back: Session = serde_json::from_str(&text).expect("deserialise");
        assert_eq!(back.tabs[0].view, view);
    }

    #[test]
    fn tabs_with_missing_files_are_dropped() {
        let session = Session {
            tabs: vec![tab("keep.pdf"), tab("gone.pdf")],
            active: Some(1),
        };
        let kept = session.retaining(|path| path == Path::new("keep.pdf"));
        assert_eq!(kept.tabs.len(), 1);
        assert_eq!(kept.tabs[0].path, PathBuf::from("keep.pdf"));
        // The active tab was the one that vanished; it falls back in range.
        assert_eq!(kept.active, Some(0));
    }

    #[test]
    fn an_empty_session_has_no_active_tab() {
        let session = Session {
            tabs: vec![tab("gone.pdf")],
            active: Some(0),
        };
        let kept = session.retaining(|_| false);
        assert!(kept.tabs.is_empty());
        assert_eq!(kept.active, None);
    }

    /// The whole point: what is written must come back exactly.
    #[test]
    fn a_session_survives_a_trip_through_a_file() {
        let dir = std::env::temp_dir().join("pdf-reader-session-test");
        let path = dir.join("session.json");
        let _ = std::fs::remove_dir_all(&dir);

        let session = Session {
            tabs: vec![tab("a.pdf"), tab("b.pdf")],
            active: Some(0),
        };
        save_to(&path, &session);
        assert_eq!(load_from(&path), session);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_session_file_loads_as_empty() {
        let path = std::env::temp_dir().join("pdf-reader-session-does-not-exist.json");
        let _ = std::fs::remove_file(&path);
        assert_eq!(load_from(&path), Session::default());
    }
}
