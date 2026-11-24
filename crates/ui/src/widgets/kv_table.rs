use apitester_core::models::{FormField, FormFieldKind, KeyValue, Variable};
use egui::{Grid, Ui};

/// Renders an editable Key/Value/Description table. Returns `true` if modified.
pub fn kv_table_ui(ui: &mut Ui, id_salt: &str, items: &mut Vec<KeyValue>) -> bool {
    let mut changed = false;
    let mut remove_idx = None;

    Grid::new(id_salt)
        .num_columns(5)
        .striped(true)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.strong("On");
            ui.strong("Key");
            ui.strong("Value");
            ui.strong("Description");
            ui.strong("");
            ui.end_row();

            for (idx, item) in items.iter_mut().enumerate() {
                changed |= ui.checkbox(&mut item.enabled, "").changed();
                changed |= ui
                    .add_sized([180.0, 22.0], egui::TextEdit::singleline(&mut item.key))
                    .changed();
                changed |= ui
                    .add_sized([240.0, 22.0], egui::TextEdit::singleline(&mut item.value))
                    .changed();
                changed |= ui
                    .add_sized([200.0, 22.0], egui::TextEdit::singleline(&mut item.description))
                    .changed();
                if ui.small_button("x").on_hover_text("Remove row").clicked() {
                    remove_idx = Some(idx);
                }
                ui.end_row();
            }
        });

    if let Some(idx) = remove_idx {
        items.remove(idx);
        changed = true;
    }

    if ui.small_button("+ Add Row").clicked() {
        items.push(KeyValue::default());
        changed = true;
    }

    changed
}

/// Renders an editable multipart `FormData` table (Text or File fields).
pub fn form_data_table_ui(ui: &mut Ui, id_salt: &str, items: &mut Vec<FormField>) -> bool {
    let mut changed = false;
    let mut remove_idx = None;

    Grid::new(id_salt)
        .num_columns(5)
        .striped(true)
        .show(ui, |ui| {
            ui.strong("On");
            ui.strong("Key");
            ui.strong("Type");
            ui.strong("Value / File Path");
            ui.strong("");
            ui.end_row();

            for (idx, field) in items.iter_mut().enumerate() {
                changed |= ui.checkbox(&mut field.enabled, "").changed();
                changed |= ui
                    .add_sized([160.0, 22.0], egui::TextEdit::singleline(&mut field.key))
                    .changed();

                let is_file = matches!(field.kind, FormFieldKind::File { .. });
                let mut selected_file = is_file;
                egui::ComboBox::from_id_salt(format!("{id_salt}_kind_{idx}"))
                    .width(72.0)
                    .selected_text(if selected_file { "File" } else { "Text" })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut selected_file, false, "Text");
                        ui.selectable_value(&mut selected_file, true, "File");
                    });
                if selected_file != is_file {
                    field.kind = if selected_file {
                        FormFieldKind::File {
                            path: String::new(),
                            content_type: None,
                        }
                    } else {
                        FormFieldKind::Text {
                            value: String::new(),
                        }
                    };
                    changed = true;
                }

                match &mut field.kind {
                    FormFieldKind::Text { value } => {
                        changed |= ui
                            .add_sized([260.0, 22.0], egui::TextEdit::singleline(value))
                            .changed();
                    }
                    FormFieldKind::File { path, .. } => {
                        changed |= ui
                            .add_sized(
                                [260.0, 22.0],
                                egui::TextEdit::singleline(path).hint_text("/path/to/file"),
                            )
                            .changed();
                    }
                }

                if ui.small_button("x").clicked() {
                    remove_idx = Some(idx);
                }
                ui.end_row();
            }
        });

    if let Some(idx) = remove_idx {
        items.remove(idx);
        changed = true;
    }

    if ui.small_button("+ Add Field").clicked() {
        items.push(FormField::text("", ""));
        changed = true;
    }

    changed
}

/// Renders an editable `Variable` table with secret masking toggle.
pub fn variables_table_ui(
    ui: &mut Ui,
    id_salt: &str,
    vars: &mut Vec<Variable>,
    reveal_secrets: bool,
) -> bool {
    let mut changed = false;
    let mut remove_idx = None;

    Grid::new(id_salt)
        .num_columns(5)
        .striped(true)
        .show(ui, |ui| {
            ui.strong("On");
            ui.strong("Variable");
            ui.strong("Value");
            ui.strong("Secret");
            ui.strong("");
            ui.end_row();

            for (idx, var) in vars.iter_mut().enumerate() {
                changed |= ui.checkbox(&mut var.enabled, "").changed();
                changed |= ui
                    .add_sized([140.0, 22.0], egui::TextEdit::singleline(&mut var.key))
                    .changed();
                changed |= ui
                    .add_sized(
                        [180.0, 22.0],
                        egui::TextEdit::singleline(&mut var.value)
                            .password(var.secret && !reveal_secrets),
                    )
                    .changed();
                changed |= ui.checkbox(&mut var.secret, "").changed();
                if ui.small_button("x").clicked() {
                    remove_idx = Some(idx);
                }
                ui.end_row();
            }
        });

    if let Some(idx) = remove_idx {
        vars.remove(idx);
        changed = true;
    }

    if ui.small_button("+ Add Variable").clicked() {
        vars.push(Variable::default());
        changed = true;
    }

    changed
}
