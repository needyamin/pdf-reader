//! Per-tab viewing state: how a document is currently being looked at.

use serde::{Deserialize, Serialize};

use crate::document::Rotation;
use crate::form::FieldId;

/// Whether pages are shown one at a time or as a continuous column.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub enum ViewMode {
    /// One page fills the viewport; scrolling moves between pages.
    Single,
    /// All pages stacked in a single scrollable column.
    #[default]
    Continuous,
}

/// How the current zoom factor was arrived at.
///
/// `Fixed` means the user picked a scale. `FitWidth` and `FitPage` are derived
/// from the viewport, so the resolved factor changes when the window resizes.
#[derive(Clone, Copy, PartialEq, Debug, Default, Serialize, Deserialize)]
pub enum ZoomMode {
    /// An explicit scale factor, where 1.0 is 100%.
    Fixed(f32),
    /// Scale so the page width matches the viewport width.
    ///
    /// This is the default: a page that fills the width is what people expect
    /// from a PDF reader, and it is what "full width" means in practice.
    #[default]
    FitWidth,
    /// Scale so the whole page fits in the viewport.
    FitPage,
}

impl ZoomMode {
    /// The scale implied by this mode, if it does not depend on the viewport.
    pub fn fixed_factor(self) -> Option<f32> {
        match self {
            Self::Fixed(f) => Some(f),
            Self::FitWidth | Self::FitPage => None,
        }
    }
}

/// Zoom, scroll and rotation state for one tab.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct ViewState {
    /// The user's zoom intent.
    pub zoom_mode: ZoomMode,
    /// Resolved scale factor. For fit modes this is recomputed on resize.
    pub zoom: f32,
    /// Horizontal scroll offset in document space, in points.
    pub scroll_x: f32,
    /// Vertical scroll offset in document space, in points.
    pub scroll_y: f32,
    /// Page rotation applied to every page.
    pub rotation: Rotation,
    /// Single page or continuous column.
    pub mode: ViewMode,
    /// Zero-based index of the page currently under the viewport centre.
    pub current_page: u32,
    /// The form field the user last selected, if any.
    ///
    /// Selection is shared by the Forms panel and the on-page overlay: clicking
    /// a row in one highlights the widget in the other, so it has to live in
    /// state rather than in either widget.
    pub selected_field: Option<FieldId>,
    /// The annotation the user last selected, if any.
    pub selected_annotation: Option<crate::annotation::AnnotationId>,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            zoom_mode: ZoomMode::FitWidth,
            zoom: 1.0,
            scroll_x: 0.0,
            scroll_y: 0.0,
            rotation: Rotation::None,
            mode: ViewMode::Continuous,
            current_page: 0,
            selected_field: None,
            selected_annotation: None,
        }
    }
}

/// Named colour themes carried over from the previous version.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub enum ThemeId {
    /// Adobe Acrobat Pro DC–style dark chrome with a red accent.
    #[default]
    Acrobat,
    /// Dark.
    Dark,
    /// Light.
    Light,
    /// Midnight.
    Midnight,
    /// Rose.
    Rose,
    /// Forest.
    Forest,
    /// Sunset.
    Sunset,
}

impl ThemeId {
    /// Every theme, in display order.
    pub const ALL: [ThemeId; 7] = [
        ThemeId::Acrobat,
        ThemeId::Dark,
        ThemeId::Light,
        ThemeId::Midnight,
        ThemeId::Rose,
        ThemeId::Forest,
        ThemeId::Sunset,
    ];

    /// Stable name used for persistence.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Acrobat => "acrobat",
            Self::Dark => "dark",
            Self::Light => "light",
            Self::Midnight => "midnight",
            Self::Rose => "rose",
            Self::Forest => "forest",
            Self::Sunset => "sunset",
        }
    }

    /// Look up a theme by its persisted name.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|t| t.name() == name)
    }
}

/// Zoom bounds. Deliberately wider than the old app's 0.25..5 range.
pub const MIN_ZOOM: f32 = 0.1;
/// Zoom bounds. Deliberately wider than the old app's 0.25..5 range.
pub const MAX_ZOOM: f32 = 32.0;

/// Clamp a raw scale factor into the supported range.
pub fn clamp_zoom(zoom: f32) -> f32 {
    zoom.clamp(MIN_ZOOM, MAX_ZOOM)
}
