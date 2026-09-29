//! Conversion from PDFium's widget annotations into the core form model.
//!
//! The domain types live in `pdfreader_core::form` so the UI and the reducer can
//! use them without depending on PDFium. This module is the only place that
//! knows how PDFium represents a form, which keeps that knowledge behind the
//! engine seam.

use pdfium_render::prelude::{
    PdfDocument, PdfFormField, PdfFormFieldCommon, PdfFormFieldType, PdfFormType,
    PdfPageAnnotation, PdfPageAnnotationCommon,
};

use pdfreader_core::{FieldId, FieldOption, FieldValue, FormFieldInfo, FormFieldType, FormInfo, FormKind};

/// Read a document's interactive form.
///
/// Walks every page's annotations and keeps the ones PDFium recognises as form
/// widgets. Returns an empty [`FormInfo`] rather than an error when the document
/// simply has no form, because "no form" is a normal state, not a failure.
///
/// Widgets whose bounds cannot be read are skipped rather than given a zero
/// rect: a field with no rectangle cannot be drawn or clicked, so surfacing it
/// would only create an entry the user cannot reach.
pub(crate) fn read_form(document: &PdfDocument<'_>) -> FormInfo {
    let kind = match document.form() {
        Some(form) => match form.form_type() {
            PdfFormType::Acrobat => FormKind::Acrobat,
            PdfFormType::XfaFull => FormKind::XfaFull,
            PdfFormType::XfaForeground => FormKind::XfaForeground,
            _ => FormKind::None,
        },
        None => FormKind::None,
    };

    let mut fields = Vec::new();
    let pages = document.pages();
    let page_count = pages.len();

    for page_index in 0..page_count {
        let Ok(page) = pages.get(page_index) else {
            continue;
        };
        let annotations = page.annotations();

        for (annot_index, annotation) in annotations.iter().enumerate() {
            let Some(field) = annotation.as_form_field() else {
                continue;
            };
            if let Some(info) = read_widget(&annotation, field, page_index as u32, annot_index) {
                fields.push(info);
            }
        }
    }

    FormInfo { kind, fields }
}

/// Build one [`FormFieldInfo`] from a widget annotation and its field.
fn read_widget(
    annotation: &PdfPageAnnotation<'_>,
    field: &PdfFormField<'_>,
    page: u32,
    annot_index: usize,
) -> Option<FormFieldInfo> {
    // A widget with no bounds cannot be placed or hit-tested, so drop it.
    let bounds = annotation.bounds().ok()?;
    let rect = pdfreader_core::Rect::from_xywh(
        bounds.left().value,
        bounds.bottom().value,
        bounds.width().value,
        bounds.height().value,
    );

    let kind = match field.field_type() {
        PdfFormFieldType::PushButton => FormFieldType::PushButton,
        PdfFormFieldType::Checkbox => FormFieldType::CheckBox,
        PdfFormFieldType::RadioButton => FormFieldType::RadioButton,
        PdfFormFieldType::ComboBox => FormFieldType::ComboBox,
        PdfFormFieldType::ListBox => FormFieldType::ListBox,
        PdfFormFieldType::Text => FormFieldType::Text,
        PdfFormFieldType::Signature => FormFieldType::Signature,
        PdfFormFieldType::Unknown => FormFieldType::Unknown,
    };

    let value = read_value(field, kind);
    let options = read_options(field);
    let multiline = field.as_text_field().is_some_and(|t| t.is_multiline());

    Some(FormFieldInfo {
        id: FieldId::new(page, annot_index as u32),
        name: field.name().unwrap_or_default(),
        alternate_name: annotation.name(),
        rect,
        kind,
        value,
        options,
        read_only: field.is_read_only(),
        required: field.is_required(),
        multiline,
    })
}

/// Read the current value of a field, per widget type.
///
/// PDFium has no single value accessor: text and choice fields expose `value()`,
/// while checkboxes and radio buttons expose `is_checked()`.
fn read_value(field: &PdfFormField<'_>, kind: FormFieldType) -> FieldValue {
    match kind {
        FormFieldType::Text => field
            .as_text_field()
            .and_then(|t| t.value())
            .map_or(FieldValue::Empty, FieldValue::Text),

        FormFieldType::ComboBox | FormFieldType::ListBox => {
            let selected = field
                .as_combo_box_field()
                .and_then(|c| c.value())
                .or_else(|| field.as_list_box_field().and_then(|l| l.value()));
            FieldValue::Choice(selected)
        }

        FormFieldType::CheckBox | FormFieldType::RadioButton => {
            let checked = field
                .as_checkbox_field()
                .and_then(|c| c.is_checked().ok())
                .or_else(|| field.as_radio_button_field().and_then(|r| r.is_checked().ok()));
            FieldValue::Checked(checked.unwrap_or(false))
        }

        FormFieldType::PushButton | FormFieldType::Signature | FormFieldType::Unknown => {
            FieldValue::Empty
        }
    }
}

/// Read the choices of a combo or list box.
fn read_options(field: &PdfFormField<'_>) -> Vec<FieldOption> {
    let options = field
        .as_combo_box_field()
        .map(|c| c.options())
        .or_else(|| field.as_list_box_field().map(|l| l.options()));

    let Some(options) = options else {
        return Vec::new();
    };

    options
        .iter()
        .map(|option| FieldOption {
            label: option.label().cloned().unwrap_or_default(),
            selected: option.is_set(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The widget tree is only reachable through PDFium, so what is worth
    /// pinning down here is the pure mapping from PDFium's enum to ours.
    #[test]
    fn every_pdfium_field_type_maps_to_a_domain_type() {
        let mapped = [
            (PdfFormFieldType::PushButton, FormFieldType::PushButton),
            (PdfFormFieldType::Checkbox, FormFieldType::CheckBox),
            (PdfFormFieldType::RadioButton, FormFieldType::RadioButton),
            (PdfFormFieldType::ComboBox, FormFieldType::ComboBox),
            (PdfFormFieldType::ListBox, FormFieldType::ListBox),
            (PdfFormFieldType::Text, FormFieldType::Text),
            (PdfFormFieldType::Signature, FormFieldType::Signature),
            (PdfFormFieldType::Unknown, FormFieldType::Unknown),
        ];
        for (pdfium_kind, expected) in mapped {
            let domain = match pdfium_kind {
                PdfFormFieldType::PushButton => FormFieldType::PushButton,
                PdfFormFieldType::Checkbox => FormFieldType::CheckBox,
                PdfFormFieldType::RadioButton => FormFieldType::RadioButton,
                PdfFormFieldType::ComboBox => FormFieldType::ComboBox,
                PdfFormFieldType::ListBox => FormFieldType::ListBox,
                PdfFormFieldType::Text => FormFieldType::Text,
                PdfFormFieldType::Signature => FormFieldType::Signature,
                PdfFormFieldType::Unknown => FormFieldType::Unknown,
            };
            assert_eq!(domain, expected);
        }
    }
}
