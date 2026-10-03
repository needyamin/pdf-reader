//! Interactive form fields as domain state.
//!
//! PDFium exposes form fields as *widget annotations* scattered across pages,
//! each of which may borrow a name from a parent field — a radio group is
//! several widgets sharing one name, and one logical field can have widgets on
//! more than one page. This module holds the flattened, engine-independent view
//! that the reducer stores and the UI renders.
//!
//! The types live in `core` rather than in the engine crate because the Forms
//! panel and the field overlay both need them, and neither may depend on
//! PDFium. Conversion from PDFium's widget tree happens in `pdfreader_pdf`.
//!
//! All rectangles are in unrotated PDF page space, so anything drawn on screen
//! must go through `pdfreader_render::hit::PageSpace::to_screen` first.

use serde::{Deserialize, Serialize};

use crate::rect::Rect;

/// Identifies one widget on one page.
///
/// A field's name is not unique — every widget in a radio group shares it — so
/// identity has to include the page and the widget's position in that page's
/// annotation list. That pair is also what the engine needs to find the widget
/// again when writing a value back.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub struct FieldId {
    /// Zero-based page the widget sits on.
    pub page: u32,
    /// Index of the widget within that page's annotation list.
    pub annot_index: u32,
}

impl FieldId {
    /// Build an id from a page and an annotation index.
    pub const fn new(page: u32, annot_index: u32) -> Self {
        Self { page, annot_index }
    }
}

/// Which flavour of interactive form the document carries.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub enum FormKind {
    /// The document has no interactive form.
    #[default]
    None,
    /// A conventional AcroForm.
    Acrobat,
    /// A full XFA form, where the AcroForm is only a compatibility shell.
    XfaFull,
    /// An XFA form drawn over an AcroForm.
    XfaForeground,
}

impl FormKind {
    /// Whether there is anything worth showing in a Forms panel.
    pub const fn is_interactive(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// The kind of widget a form field presents.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub enum FormFieldType {
    /// A push button: an action trigger with no persistent value.
    PushButton,
    /// A checkbox: one independent on/off widget.
    CheckBox,
    /// A radio button: one widget in a mutually exclusive group.
    RadioButton,
    /// A combo box: a drop-down, optionally editable.
    ComboBox,
    /// A list box: a scrollable choice list.
    ListBox,
    /// A text field, single or multi-line.
    Text,
    /// A digital signature field.
    Signature,
    /// A widget the engine could not classify.
    #[default]
    Unknown,
}

impl FormFieldType {
    /// Whether the field holds a value a user can type or pick.
    ///
    /// Push buttons and signature fields are excluded: a push button has no
    /// state to persist, and signing is not supported by this engine.
    pub const fn is_fillable(self) -> bool {
        matches!(
            self,
            Self::Text | Self::CheckBox | Self::RadioButton | Self::ComboBox | Self::ListBox
        )
    }

    /// Whether the field offers a fixed list of choices.
    pub const fn has_options(self) -> bool {
        matches!(self, Self::ComboBox | Self::ListBox)
    }

    /// Whether the value is a boolean rather than text.
    pub const fn is_boolean(self) -> bool {
        matches!(self, Self::CheckBox | Self::RadioButton)
    }

    /// Short label for the Forms panel.
    pub const fn label(self) -> &'static str {
        match self {
            Self::PushButton => "Button",
            Self::CheckBox => "Checkbox",
            Self::RadioButton => "Radio",
            Self::ComboBox => "Dropdown",
            Self::ListBox => "List",
            Self::Text => "Text",
            Self::Signature => "Signature",
            Self::Unknown => "Field",
        }
    }
}

/// The current value of a field.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum FieldValue {
    /// No value, or a widget type that carries none.
    #[default]
    Empty,
    /// Text entered into a text field.
    Text(String),
    /// Checked state of a checkbox or radio button.
    Checked(bool),
    /// The selected option of a combo or list box.
    Choice(Option<String>),
}

impl FieldValue {
    /// The value rendered as text, for display and CSV export.
    pub fn as_text(&self) -> String {
        match self {
            Self::Empty => String::new(),
            Self::Text(s) => s.clone(),
            Self::Checked(on) => if *on { "Yes" } else { "Off" }.to_string(),
            Self::Choice(Some(s)) => s.clone(),
            Self::Choice(None) => String::new(),
        }
    }

    /// Whether the field has been left blank.
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Empty | Self::Choice(None) => true,
            Self::Text(s) => s.is_empty(),
            Self::Choice(Some(s)) => s.is_empty(),
            // An unchecked box counts as empty for form-completeness checks.
            Self::Checked(on) => !on,
        }
    }
}

/// One selectable option of a combo or list box.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct FieldOption {
    /// Text shown to the user.
    pub label: String,
    /// Whether this option is currently selected.
    pub selected: bool,
}

/// A flattened form field: one widget on one page.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct FormFieldInfo {
    /// Which widget this is.
    pub id: FieldId,
    /// Fully qualified field name, empty when the PDF leaves it unnamed.
    pub name: String,
    /// Alternate name (tooltip), when the PDF supplies one.
    pub alternate_name: Option<String>,
    /// Widget rectangle in unrotated PDF page space.
    pub rect: Rect,
    /// What kind of widget this is.
    pub kind: FormFieldType,
    /// Current value.
    pub value: FieldValue,
    /// Choices, for combo and list boxes.
    pub options: Vec<FieldOption>,
    /// Whether the field is marked read-only.
    pub read_only: bool,
    /// Whether the field is marked required.
    pub required: bool,
    /// Whether a text field accepts more than one line.
    pub multiline: bool,
}

impl FormFieldInfo {
    /// Text to show when the field has no name of its own.
    const UNNAMED: &'static str = "Unnamed field";

    /// The name to display, falling back to a placeholder for unnamed widgets.
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() {
            Self::UNNAMED
        } else {
            &self.name
        }
    }

    /// The tooltip, falling back to the display name.
    pub fn description(&self) -> &str {
        self.alternate_name
            .as_deref()
            .unwrap_or_else(|| self.display_name())
    }

    /// Whether the user may edit this field.
    pub fn is_editable(&self) -> bool {
        self.kind.is_fillable() && !self.read_only
    }
}

/// The whole interactive form of a document.
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct FormInfo {
    /// Form flavour.
    pub kind: FormKind,
    /// Every widget, in page order.
    pub fields: Vec<FormFieldInfo>,
}

impl FormInfo {
    /// Whether there is a form at all.
    pub const fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// How many fields the user can actually fill in.
    pub fn fillable_count(&self) -> usize {
        self.fields.iter().filter(|f| f.is_editable()).count()
    }

    /// Number of fillable fields still left blank.
    ///
    /// Drives the "3 of 12 remaining" hint in the Forms panel.
    pub fn remaining_count(&self) -> usize {
        self.fields
            .iter()
            .filter(|f| f.is_editable() && f.value.is_empty())
            .count()
    }

    /// Look up a field by id.
    pub fn field(&self, id: FieldId) -> Option<&FormFieldInfo> {
        self.fields.iter().find(|f| f.id == id)
    }

    /// Look up a field by id, mutably.
    pub fn field_mut(&mut self, id: FieldId) -> Option<&mut FormFieldInfo> {
        self.fields.iter_mut().find(|f| f.id == id)
    }

    /// Replace a field's value in place, reporting whether anything changed.
    ///
    /// Returns `false` when the field is unknown or already holds that value,
    /// so callers can skip the tile invalidation that follows a real edit.
    pub fn set_value(&mut self, id: FieldId, value: FieldValue) -> bool {
        match self.field_mut(id) {
            Some(field) if field.value != value => {
                field.value = value;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(kind: FormFieldType, read_only: bool, value: FieldValue) -> FormFieldInfo {
        FormFieldInfo {
            id: FieldId::new(0, 0),
            name: "a".into(),
            alternate_name: None,
            rect: Rect::from_xywh(0.0, 0.0, 10.0, 10.0),
            kind,
            value,
            options: Vec::new(),
            read_only,
            required: false,
            multiline: false,
        }
    }

    #[test]
    fn only_value_bearing_widgets_are_fillable() {
        assert!(FormFieldType::Text.is_fillable());
        assert!(FormFieldType::CheckBox.is_fillable());
        assert!(FormFieldType::RadioButton.is_fillable());
        assert!(FormFieldType::ComboBox.is_fillable());
        assert!(FormFieldType::ListBox.is_fillable());
        // A push button has no state to persist; signing is unsupported.
        assert!(!FormFieldType::PushButton.is_fillable());
        assert!(!FormFieldType::Signature.is_fillable());
    }

    #[test]
    fn only_choice_fields_have_options() {
        assert!(FormFieldType::ComboBox.has_options());
        assert!(FormFieldType::ListBox.has_options());
        assert!(!FormFieldType::Text.has_options());
    }

    #[test]
    fn boolean_fields_are_checks_and_radios() {
        assert!(FormFieldType::CheckBox.is_boolean());
        assert!(FormFieldType::RadioButton.is_boolean());
        assert!(!FormFieldType::Text.is_boolean());
    }

    #[test]
    fn field_values_render_as_exportable_text() {
        assert_eq!(FieldValue::Text("hello".into()).as_text(), "hello");
        assert_eq!(FieldValue::Checked(true).as_text(), "Yes");
        assert_eq!(FieldValue::Checked(false).as_text(), "Off");
        assert_eq!(FieldValue::Choice(Some("b".into())).as_text(), "b");
        assert_eq!(FieldValue::Choice(None).as_text(), "");
        assert_eq!(FieldValue::Empty.as_text(), "");
    }

    #[test]
    fn emptiness_covers_blank_and_unset_values() {
        assert!(FieldValue::Empty.is_empty());
        assert!(FieldValue::Text(String::new()).is_empty());
        assert!(FieldValue::Choice(None).is_empty());
        assert!(FieldValue::Choice(Some(String::new())).is_empty());
        assert!(!FieldValue::Text("x".into()).is_empty());
        assert!(!FieldValue::Checked(true).is_empty());
        // An unchecked box counts as empty for form-completeness checks.
        assert!(FieldValue::Checked(false).is_empty());
    }

    #[test]
    fn unnamed_fields_get_a_display_placeholder() {
        let mut f = field(FormFieldType::Text, false, FieldValue::Empty);
        f.name = String::new();
        assert_eq!(f.display_name(), "Unnamed field");
        // With a name, the tooltip falls back to it.
        assert_eq!(f.description(), "Unnamed field");
        f.alternate_name = Some("Full name".into());
        assert_eq!(f.description(), "Full name");
    }

    #[test]
    fn read_only_fields_are_not_editable() {
        let mut f = field(FormFieldType::Text, true, FieldValue::Empty);
        assert!(!f.is_editable());
        f.read_only = false;
        assert!(f.is_editable());
        // A push button is never editable even when writable.
        f.kind = FormFieldType::PushButton;
        assert!(!f.is_editable());
    }

    #[test]
    fn counts_ignore_buttons_and_read_only_fields() {
        let info = FormInfo {
            kind: FormKind::Acrobat,
            fields: vec![
                field(FormFieldType::Text, false, FieldValue::Empty),
                field(FormFieldType::Text, false, FieldValue::Text("x".into())),
                field(FormFieldType::PushButton, false, FieldValue::Empty),
                field(FormFieldType::CheckBox, true, FieldValue::Empty),
            ],
        };
        assert_eq!(info.fillable_count(), 2);
        assert_eq!(info.remaining_count(), 1);
        assert!(!info.is_empty());
        assert!(FormInfo::default().is_empty());
    }

    #[test]
    fn set_value_reports_whether_anything_changed() {
        let mut info = FormInfo {
            kind: FormKind::Acrobat,
            fields: vec![field(FormFieldType::Text, false, FieldValue::Empty)],
        };
        let id = FieldId::new(0, 0);

        assert!(info.set_value(id, FieldValue::Text("x".into())));
        assert_eq!(info.field(id).unwrap().value.as_text(), "x");

        // Same value again: no change, so no redraw is needed.
        assert!(!info.set_value(id, FieldValue::Text("x".into())));
        // Unknown field: no change either.
        assert!(!info.set_value(FieldId::new(9, 9), FieldValue::Empty));
    }
}
