use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use apitester_core::models::{AuthConfig, ProxyType, ThemeMode};
use apitester_import_export::{
    export_curl, export_postman_collection, export_postman_environment, generate_code, import_curl,
    import_har, import_openapi, import_postman_collection, import_postman_environment,
    CodegenLanguage,
};
use apitester_proxy::{CaptureProxyServer, CertificateAuthority};
use apitester_runner::{
    parse_csv_data, parse_json_data, run_collection, run_load_test, LoadProfile, LoadTestConfig,
    LoadTimelinePoint, RunnerOptions,
};
use egui::{
    Color32, Frame, Margin, Pos2, Rect, RichText, Rounding, ScrollArea, Sense, SidePanel, Stroke,
    Ui, Vec2,
};
use uuid::Uuid;

use crate::app::ApiTesterApp;
use crate::state::{
    AppTab, ImportFormat, LoadProfileKind, RunnerModalTab, UiMessage, WorkbenchTab,
};
use crate::theme::{
    apply_theme, primary_button, section_card, status_badge, subtle_divider, underline_tab,
    Palette,
};
use crate::widgets::auth_editor::auth_editor_ui;
use crate::widgets::kv_table::variables_table_ui;

const CODEGEN_LANGS: &[(CodegenLanguage, &str)] = &[
    (CodegenLanguage::Curl, "cURL"),
    (CodegenLanguage::RustReqwest, "Rust"),
    (CodegenLanguage::PythonRequests, "Python"),
    (CodegenLanguage::JavaScriptFetch, "JS"),
    (CodegenLanguage::GoNetHttp, "Go"),
    (CodegenLanguage::PhpCurl, "PHP"),
];

/// Renders the collapsible Right Code Snippet Drawer (`SidePanel::right`).
pub fn render_code_drawer(ctx: &egui::Context, app: &mut ApiTesterApp) {
    if !app.code_drawer_open {
        return;
    }

    let is_dark = !matches!(app.settings.theme, ThemeMode::Light);
    let pal = Palette::for_mode(is_dark);

    SidePanel::right("code_drawer")
        .default_width(360.0)
        .min_width(280.0)
        .frame(
            Frame::none()
                .fill(pal.bg_sidebar)
                .inner_margin(Margin::same(12.0))
                .stroke(Stroke::new(1.0_f32, pal.border_subtle)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Code Snippet").strong().size(15.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("x").on_hover_text("Close (Ctrl+Shift+C)").clicked() {
                        app.code_drawer_open = false;
                    }
                });
            });
            ui.add_space(4.0);

            ui.horizontal_wrapped(|ui| {
                for &(lang, name) in CODEGEN_LANGS {
                    if underline_tab(ui, app.code_drawer_lang == lang, name, &pal).clicked() {
                        app.code_drawer_lang = lang;
                    }
                }
            });
            subtle_divider(ui, &pal);
            ui.add_space(6.0);

            let maybe_code = app.tabs.get(app.active_tab).and_then(|t| t.as_request()).map(|tab| {
                let vars = app.build_variable_context(tab.collection_id);
                generate_code(&tab.request, app.code_drawer_lang, &vars)
            });

            if let Some(mut code) = maybe_code {
                ui.horizontal(|ui| {
                    if ui.button("Copy Snippet").clicked() {
                        ui.output_mut(|o| o.copied_text = code.clone());
                    }
                });
                ui.add_space(6.0);
                ScrollArea::both().id_salt("code_drawer_scroll").show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut code)
                            .desired_width(f32::INFINITY)
                            .desired_rows(22)
                            .code_editor(),
                    );
                });
            } else {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        RichText::new("Select an HTTP or GraphQL request tab to generate code.")
                            .color(pal.text_muted),
                    );
                });
            }
        });
}

/// Renders the full-viewport Import / Export Workbench Tab.
pub fn render_import_export_tab(ui: &mut Ui, ctx: &egui::Context, app: &mut ApiTesterApp) {
    let is_dark = !matches!(app.settings.theme, ThemeMode::Light);
    let pal = Palette::for_mode(is_dark);

    // Extract current tab fields safely.
    let (mut format, mut file_path, mut input_text, mut status) =
        match app.tabs.get(app.active_tab) {
            Some(AppTab::ImportExport {
                format,
                file_path,
                input_text,
                status,
            }) => (*format, file_path.clone(), input_text.clone(), status.clone()),
            _ => return,
        };

    // Handle drag-and-drop files anywhere on the window while this tab is active.
    let dropped_paths: Vec<_> = ctx.input(|i| {
        i.raw
            .dropped_files
            .iter()
            .filter_map(|f| f.path.clone())
            .collect()
    });
    if let Some(path) = dropped_paths.first() {
        let (detected, text, msg) = import_from_file_path(app, path);
        format = detected;
        file_path = path.display().to_string();
        input_text = text;
        status = msg;
    }

    ScrollArea::vertical().id_salt("import_export_tab_scroll").show(ui, |ui| {
        ui.heading("Import & Export");
        ui.label(
            RichText::new("Import Postman v2.1 collections/environments, cURL, OpenAPI 3/Swagger, or HAR 1.2 files.")
                .color(pal.text_secondary),
        );
        ui.add_space(10.0);

        section_card(ui, &pal, |ui| {
            ui.horizontal_wrapped(|ui| {
                if primary_button(ui, "Choose File...", Vec2::new(118.0, 32.0)).clicked() {
                    status = "Opening file dialog...".into();
                    let tx = app.tx.clone();
                    let egui_ctx = ctx.clone();
                    app.rt.spawn(async move {
                        let picked = pick_import_file_async().await;
                        let _ = tx.send(UiMessage::ImportFilePicked(picked));
                        egui_ctx.request_repaint();
                    });
                }
                if ui
                    .add_enabled(
                        !input_text.trim().is_empty(),
                        egui::Button::new(RichText::new("Import Text").strong().color(Color32::WHITE))
                            .fill(pal.accent_primary)
                            .stroke(Stroke::NONE)
                            .rounding(Rounding::same(6.0))
                            .min_size(Vec2::new(104.0, 32.0)),
                    )
                    .clicked()
                {
                    status = run_import(app, format, &input_text);
                }
                if ui
                    .add_enabled(!input_text.is_empty(), egui::Button::new("Clear"))
                    .clicked()
                {
                    input_text.clear();
                    file_path.clear();
                    status.clear();
                }
                ui.label(
                    RichText::new("Tip: Drag & drop any .json / .yaml / .har file onto this window")
                        .small()
                        .color(pal.text_muted),
                );
            });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("File path:").color(pal.text_secondary));
                ui.add(
                    egui::TextEdit::singleline(&mut file_path)
                        .hint_text("/path/to/collection.postman_collection.json")
                        .desired_width((ui.available_width() - 80.0).max(200.0)),
                );
                if ui
                    .add_enabled(!file_path.trim().is_empty(), egui::Button::new("Load"))
                    .clicked()
                {
                    let p = Path::new(file_path.trim()).to_path_buf();
                    let (detected, text, msg) = import_from_file_path(app, &p);
                    format = detected;
                    input_text = text;
                    status = msg;
                }
            });

            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                for (fmt, label) in [
                    (ImportFormat::PostmanCollection, "Postman v2.1"),
                    (ImportFormat::PostmanEnvironment, "Postman Env"),
                    (ImportFormat::Curl, "cURL"),
                    (ImportFormat::OpenApi, "OpenAPI / Swagger"),
                    (ImportFormat::Har, "HAR 1.2"),
                ] {
                    if underline_tab(ui, format == fmt, label, &pal).clicked() {
                        format = fmt;
                    }
                }
            });
            subtle_divider(ui, &pal);
            ui.add_space(8.0);

            ScrollArea::vertical()
                .id_salt("import_export_editor_scroll")
                .max_height(280.0)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut input_text)
                            .desired_rows(13)
                            .desired_width(f32::INFINITY)
                            .hint_text("Paste Postman v2.1 JSON, cURL command, OpenAPI JSON/YAML, or HAR 1.2 JSON...")
                            .code_editor(),
                    );
                });

            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                if ui.button("Export Postman v2.1").clicked() {
                    if let Some(col) = app.collections.first() {
                        match export_postman_collection(col) {
                            Ok(json) => {
                                input_text = json;
                                status = format!("Exported '{}' as Postman v2.1 JSON.", col.name);
                            }
                            Err(e) => status = format!("Export failed: {e}"),
                        }
                    } else {
                        status = "No collections to export.".into();
                    }
                }
                if ui.button("Export Active Env").clicked() {
                    if let Some(env) = app
                        .settings
                        .active_environment_id
                        .and_then(|id| app.environments.iter().find(|e| e.id == id))
                        .or_else(|| app.environments.first())
                    {
                        match export_postman_environment(env) {
                            Ok(json) => {
                                input_text = json;
                                status = format!("Exported environment '{}'.", env.name);
                            }
                            Err(e) => status = format!("Export failed: {e}"),
                        }
                    } else {
                        status = "No environments to export.".into();
                    }
                }
                if ui.button("Export First Request as cURL").clicked() {
                    if let Some(tab) = app.tabs.iter().find_map(AppTab::as_request) {
                        let vars = app.build_variable_context(tab.collection_id);
                        input_text = export_curl(&tab.request, &vars);
                        status = format!("Exported '{}' as cURL.", tab.title);
                    }
                }
                if ui
                    .add_enabled(!input_text.trim().is_empty(), egui::Button::new("Save to File..."))
                    .clicked()
                {
                    let default_name = match format {
                        ImportFormat::PostmanEnvironment => "environment.postman_environment.json",
                        ImportFormat::Curl => "request.sh",
                        _ => "collection.postman_collection.json",
                    };
                    let content_to_save = input_text.clone();
                    let tx = app.tx.clone();
                    let egui_ctx = ctx.clone();
                    status = "Opening save dialog...".into();
                    app.rt.spawn(async move {
                        let res = save_export_file_async(default_name, content_to_save).await;
                        let _ = tx.send(UiMessage::ExportFileSaved(res));
                        egui_ctx.request_repaint();
                    });
                }
            });

            if !status.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new(&status).color(pal.info));
            }
        });
    });

    if let Some(AppTab::ImportExport {
        format: f,
        file_path: fp,
        input_text: it,
        status: st,
    }) = app.tabs.get_mut(app.active_tab)
    {
        *f = format;
        *fp = file_path;
        *it = input_text;
        *st = status;
    }
}

/// Renders the full-viewport Collection Runner & Native Load Tester Workbench Tab.
pub fn render_runner_tab(ui: &mut Ui, app: &mut ApiTesterApp) {
    let is_dark = !matches!(app.settings.theme, ThemeMode::Light);
    let pal = Palette::for_mode(is_dark);

    ui.horizontal(|ui| {
        ui.heading("Runner & Load Tester");
        ui.add_space(16.0);
        if underline_tab(
            ui,
            app.runner_modal_tab == RunnerModalTab::CollectionRunner,
            "Collection Runner",
            &pal,
        )
        .clicked()
        {
            app.runner_modal_tab = RunnerModalTab::CollectionRunner;
        }
        if underline_tab(
            ui,
            app.runner_modal_tab == RunnerModalTab::LoadTester,
            "Load Tester",
            &pal,
        )
        .clicked()
        {
            app.runner_modal_tab = RunnerModalTab::LoadTester;
        }
    });
    subtle_divider(ui, &pal);
    ui.add_space(8.0);

    match app.runner_modal_tab {
        RunnerModalTab::CollectionRunner => render_collection_runner_view(ui, app, &pal),
        RunnerModalTab::LoadTester => render_load_tester_view(ui, app, &pal),
    }
}

/// Renders the full-viewport Local Capture Proxy Workbench Tab.
pub fn render_capture_proxy_tab(ui: &mut Ui, app: &mut ApiTesterApp) {
    let is_dark = !matches!(app.settings.theme, ThemeMode::Light);
    let pal = Palette::for_mode(is_dark);

    ui.heading("Local Capture Proxy");
    ui.label(
        RichText::new("Inspect HTTP/HTTPS traffic locally and open any captured exchange directly as a Request tab.")
            .color(pal.text_secondary),
    );
    ui.add_space(10.0);

    section_card(ui, &pal, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label("Port:");
            ui.add(egui::DragValue::new(&mut app.proxy_port).range(1024..=65535));
            if app.proxy_server.is_some() {
                if ui
                    .button(RichText::new("Stop Proxy").strong().color(pal.error))
                    .clicked()
                {
                    if let Some(srv) = app.proxy_server.take() {
                        srv.stop();
                    }
                    app.proxy_status = "Stopped".into();
                }
                ui.label(RichText::new(&app.proxy_status).strong().color(pal.success));
            } else if primary_button(ui, "Start Proxy", Vec2::new(104.0, 30.0)).clicked() {
                let addr: SocketAddr = ([127, 0, 0, 1], app.proxy_port).into();
                let start_res = app.rt.block_on(CaptureProxyServer::start(addr));
                match start_res {
                    Ok((srv, bound_addr, mut rx)) => {
                        app.proxy_status = format!("Listening on {bound_addr}");
                        app.proxy_server = Some(srv);
                        let tx = app.tx.clone();
                        app.rt.spawn(async move {
                            while let Ok(exchange) = rx.recv().await {
                                let _ = tx.send(UiMessage::ProxyCaptured(exchange));
                            }
                        });
                    }
                    Err(e) => {
                        app.proxy_status = format!("Start failed: {e}");
                    }
                }
            }

            if ui.button("Export Root CA").clicked() {
                let ca_dir = app.json_store.root_dir().join("ca");
                let _ = std::fs::create_dir_all(&ca_dir);
                let cert_path = ca_dir.join("apitester-ca.crt.pem");
                let key_path = ca_dir.join("apitester-ca.key.pem");
                match CertificateAuthority::load_or_generate(&cert_path, &key_path) {
                    Ok(_) => {
                        app.proxy_status = format!("CA saved to {}", cert_path.display());
                    }
                    Err(e) => {
                        app.proxy_status = format!("CA error: {e}");
                    }
                }
            }

            if ui.button("Clear").clicked() {
                app.captured_exchanges.clear();
                if let Some(srv) = &app.proxy_server {
                    srv.clear_history();
                }
            }
        });
    });

    ui.add_space(10.0);
    let mut open_idx = None;
    section_card(ui, &pal, |ui| {
        if app.captured_exchanges.is_empty() {
            ui.label(
                RichText::new("No captured exchanges yet. Start the proxy and route HTTP traffic through it.")
                    .color(pal.text_muted),
            );
        } else {
            ScrollArea::vertical().id_salt("proxy_table_scroll").show(ui, |ui| {
                egui::Grid::new("proxy_captured_grid")
                    .num_columns(5)
                    .striped(true)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.strong("Method");
                        ui.strong("Status");
                        ui.strong("URL");
                        ui.strong("Duration");
                        ui.strong("");
                        ui.end_row();
                        for (idx, ex) in app.captured_exchanges.iter().enumerate() {
                            ui.strong(&ex.method);
                            status_badge(ui, ex.status, "", is_dark);
                            ui.monospace(&ex.url);
                            ui.label(
                                RichText::new(format!("{} ms", ex.duration_ms))
                                    .color(pal.text_secondary),
                            );
                            if ui.small_button("Open in Request Tab").clicked() {
                                open_idx = Some(idx);
                            }
                            ui.end_row();
                        }
                    });
            });
        }
    });

    if let Some(idx) = open_idx {
        let req = app.captured_exchanges[idx].to_request_item();
        app.tabs.push(AppTab::Request(Box::new(WorkbenchTab::from_request(req, None))));
        app.active_tab = app.tabs.len() - 1;
    }
}

/// Renders the full-viewport Settings Workbench Tab.
pub fn render_settings_tab(ui: &mut Ui, ctx: &egui::Context, app: &mut ApiTesterApp) {
    let is_dark = !matches!(app.settings.theme, ThemeMode::Light);
    let pal = Palette::for_mode(is_dark);

    ScrollArea::vertical().id_salt("settings_tab_scroll").show(ui, |ui| {
        ui.heading("Settings");
        ui.add_space(10.0);

        section_card(ui, &pal, |ui| {
            ui.label(RichText::new("Appearance").strong().size(15.0));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("Theme:");
                if underline_tab(ui, app.settings.theme != ThemeMode::Light, "Dark", &pal).clicked() {
                    app.settings.theme = ThemeMode::Dark;
                    apply_theme(ctx, app.settings.theme);
                    let _ = app.sqlite_store.save_settings(&app.settings);
                }
                if underline_tab(ui, app.settings.theme == ThemeMode::Light, "Light", &pal).clicked() {
                    app.settings.theme = ThemeMode::Light;
                    apply_theme(ctx, app.settings.theme);
                    let _ = app.sqlite_store.save_settings(&app.settings);
                }
            });
        });

        ui.add_space(10.0);
        section_card(ui, &pal, |ui| {
            ui.label(RichText::new("Network & TLS").strong().size(15.0));
            ui.add_space(6.0);
            ui.checkbox(&mut app.settings.verify_ssl, "Verify SSL certificates");
            ui.checkbox(&mut app.settings.follow_redirects, "Follow HTTP redirects");
            ui.horizontal(|ui| {
                ui.label("Max Redirects:");
                ui.add(egui::DragValue::new(&mut app.settings.max_redirects).range(0..=50));
                ui.add_space(12.0);
                ui.label("Timeout (ms):");
                ui.add(egui::DragValue::new(&mut app.settings.timeout_ms).range(0..=300_000));
            });
        });

        ui.add_space(10.0);
        section_card(ui, &pal, |ui| {
            ui.label(RichText::new("Outbound Proxy").strong().size(15.0));
            ui.add_space(6.0);
            ui.checkbox(&mut app.settings.proxy.enabled, "Enable outbound proxy");
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("settings_proxy_type")
                    .selected_text(format!("{:?}", app.settings.proxy.proxy_type))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut app.settings.proxy.proxy_type, ProxyType::Http, "HTTP");
                        ui.selectable_value(&mut app.settings.proxy.proxy_type, ProxyType::Https, "HTTPS");
                        ui.selectable_value(&mut app.settings.proxy.proxy_type, ProxyType::Socks5, "SOCKS5");
                    });
                ui.add(egui::TextEdit::singleline(&mut app.settings.proxy.host).desired_width(200.0));
                ui.add(egui::DragValue::new(&mut app.settings.proxy.port).range(1..=65535));
            });
        });

        ui.add_space(12.0);
        if primary_button(ui, "Save Settings", Vec2::new(124.0, 32.0)).clicked() {
            let _ = app.sqlite_store.save_settings(&app.settings);
            apply_theme(ctx, app.settings.theme);
        }
    });
}

/// Renders the full-viewport Collection Auth & Variables Workbench Tab.
pub fn render_collection_settings_tab(ui: &mut Ui, app: &mut ApiTesterApp, collection_id: Uuid) {
    let is_dark = !matches!(app.settings.theme, ThemeMode::Light);
    let pal = Palette::for_mode(is_dark);

    let mut save = false;
    ScrollArea::vertical().id_salt("col_settings_tab_scroll").show(ui, |ui| {
        if let Some(col) = app.collections.iter_mut().find(|c| c.id == collection_id) {
            ui.heading(format!("Collection Settings — {}", col.name));
            ui.add_space(10.0);

            section_card(ui, &pal, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Collection Name:");
                    ui.add(egui::TextEdit::singleline(&mut col.name).desired_width(280.0));
                });
            });

            ui.add_space(10.0);
            section_card(ui, &pal, |ui| {
                ui.label(RichText::new("Collection Authorization").strong().size(15.0));
                ui.add_space(6.0);
                auth_editor_ui(ui, "col_auth_editor", &mut col.auth);
            });

            ui.add_space(10.0);
            section_card(ui, &pal, |ui| {
                ui.label(RichText::new("Collection Variables").strong().size(15.0));
                ui.add_space(6.0);
                variables_table_ui(ui, "col_vars_editor", &mut col.variables, app.reveal_secrets);
            });

            ui.add_space(12.0);
            if primary_button(ui, "Save Collection", Vec2::new(132.0, 32.0)).clicked() {
                save = true;
            }
        } else {
            ui.label(RichText::new("Collection not found.").color(pal.text_muted));
        }
    });

    if save {
        if let Some(col) = app.collections.iter().find(|c| c.id == collection_id) {
            let _ = app.json_store.save_collection(col);
        }
    }
}

/// Opens a non-blocking async file picker.
/// On Linux, prefers native `zenity` -> `kdialog` (instant, reliable on Wayland/X11) and only
/// falls back to `rfd::AsyncFileDialog` if neither is installed.
async fn pick_import_file_async() -> Result<Option<std::path::PathBuf>, String> {
    #[cfg(target_os = "linux")]
    {
        match pick_file_via_zenity(false, None).await {
            Ok(path) => return Ok(path),
            Err(ZenityError::NotFound) => {}
            Err(ZenityError::Failed(msg)) => return Err(msg),
        }
        match pick_file_via_kdialog(false, None).await {
            Ok(path) => return Ok(path),
            Err(ZenityError::NotFound) => {}
            Err(ZenityError::Failed(msg)) => return Err(msg),
        }
    }

    let portal_fut = tokio::spawn(async {
        rfd::AsyncFileDialog::new()
            .add_filter("API Files", &["json", "yaml", "yml", "har", "txt"])
            .pick_file()
            .await
            .map(|h| h.path().to_path_buf())
    });

    match tokio::time::timeout(std::time::Duration::from_secs(30), portal_fut).await {
        Ok(Ok(path)) => Ok(path),
        Ok(Err(e)) => Err(format!("file dialog failed: {e}")),
        Err(_) => Err("file dialog timed out; use the File path field instead".into()),
    }
}

/// Opens a non-blocking async save dialog and writes `content` to disk.
async fn save_export_file_async(
    default_name: &str,
    content: String,
) -> Result<Option<std::path::PathBuf>, String> {
    let chosen = {
        #[cfg(target_os = "linux")]
        {
            match pick_file_via_zenity(true, Some(default_name)).await {
                Ok(path) => Some(path),
                Err(ZenityError::NotFound) => {
                    match pick_file_via_kdialog(true, Some(default_name)).await {
                        Ok(path) => Some(path),
                        Err(ZenityError::NotFound) => None,
                        Err(ZenityError::Failed(msg)) => return Err(msg),
                    }
                }
                Err(ZenityError::Failed(msg)) => return Err(msg),
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            None
        }
    };

    let path = match chosen {
        Some(p) => p,
        None => {
            let name_owned = default_name.to_string();
            let portal_fut = tokio::spawn(async move {
                rfd::AsyncFileDialog::new()
                    .set_file_name(&name_owned)
                    .save_file()
                    .await
                    .map(|h| h.path().to_path_buf())
            });
            match tokio::time::timeout(std::time::Duration::from_secs(30), portal_fut).await {
                Ok(Ok(p)) => p,
                Ok(Err(e)) => return Err(format!("save dialog failed: {e}")),
                Err(_) => return Err("save dialog timed out".into()),
            }
        }
    };

    if let Some(path) = path {
        tokio::fs::write(&path, content)
            .await
            .map_err(|e| e.to_string())?;
        Ok(Some(path))
    } else {
        Ok(None)
    }
}

#[cfg(target_os = "linux")]
enum ZenityError {
    NotFound,
    Failed(String),
}

#[cfg(target_os = "linux")]
async fn pick_file_via_zenity(
    save: bool,
    default_name: Option<&str>,
) -> Result<Option<std::path::PathBuf>, ZenityError> {
    let mut cmd = tokio::process::Command::new("zenity");
    cmd.arg("--file-selection");
    if save {
        cmd.arg("--save")
            .arg("--confirm-overwrite")
            .arg("--title=Save Export");
        if let Some(name) = default_name {
            cmd.arg(format!("--filename={name}"));
        }
    } else {
        cmd.arg("--title=Import API Collection")
            .arg("--file-filter=API Files (*.json *.yaml *.yml *.har *.txt) | *.json *.yaml *.yml *.har *.txt")
            .arg("--file-filter=All Files | *");
    }

    let output = match cmd.output().await {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(ZenityError::NotFound),
        Err(e) => return Err(ZenityError::Failed(e.to_string())),
    };

    if output.status.success() {
        let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if s.is_empty() {
            Ok(None)
        } else {
            Ok(Some(std::path::PathBuf::from(s)))
        }
    } else {
        Ok(None)
    }
}

#[cfg(target_os = "linux")]
async fn pick_file_via_kdialog(
    save: bool,
    default_name: Option<&str>,
) -> Result<Option<std::path::PathBuf>, ZenityError> {
    let mut cmd = tokio::process::Command::new("kdialog");
    if save {
        cmd.arg("--getsavefilename")
            .arg(default_name.unwrap_or("collection.postman_collection.json"));
    } else {
        cmd.arg("--getopenfilename")
            .arg(".")
            .arg("*.json *.yaml *.yml *.har *.txt|API Files");
    }

    let output = match cmd.output().await {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(ZenityError::NotFound),
        Err(e) => return Err(ZenityError::Failed(e.to_string())),
    };

    if output.status.success() {
        let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if s.is_empty() {
            Ok(None)
        } else {
            Ok(Some(std::path::PathBuf::from(s)))
        }
    } else {
        Ok(None)
    }
}

/// Infers the most likely [`ImportFormat`] from a file extension and content snippet.
pub fn detect_import_format(path: &Path, content: &str) -> ImportFormat {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    if ext == "har" {
        return ImportFormat::Har;
    }
    if ext == "yaml" || ext == "yml" {
        return ImportFormat::OpenApi;
    }

    let trimmed = content.trim_start();
    if trimmed.starts_with("curl ") {
        return ImportFormat::Curl;
    }

    if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if val.get("openapi").is_some() || val.get("swagger").is_some() {
            return ImportFormat::OpenApi;
        }
        if val.get("log").and_then(|l| l.get("entries")).is_some() {
            return ImportFormat::Har;
        }
        if val.get("_postman_variable_scope").is_some()
            || (val.get("values").and_then(|v| v.as_array()).is_some() && val.get("item").is_none())
        {
            return ImportFormat::PostmanEnvironment;
        }
    }

    ImportFormat::PostmanCollection
}

/// Reads an API file from disk, auto-detects its format, and imports it into `app`.
pub fn import_from_file_path(
    app: &mut ApiTesterApp,
    path: &Path,
) -> (ImportFormat, String, String) {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let format = detect_import_format(path, &content);
            let status = run_import(app, format, &content);
            (format, content, status)
        }
        Err(e) => (
            ImportFormat::PostmanCollection,
            String::new(),
            format!("Failed to read {}: {e}", path.display()),
        ),
    }
}

fn run_import(app: &mut ApiTesterApp, format: ImportFormat, input: &str) -> String {
    match format {
        ImportFormat::PostmanCollection => match import_postman_collection(input) {
            Ok(col) => {
                let name = col.name.clone();
                let count = col.items.len();
                let _ = app.json_store.save_collection(&col);
                app.collections.push(col);
                format!("Imported Postman v2.1 collection '{name}' ({count} top-level items).")
            }
            Err(e) => format!("Postman import failed: {e}"),
        },
        ImportFormat::PostmanEnvironment => match import_postman_environment(input) {
            Ok(env) => {
                let name = env.name.clone();
                let _ = app.json_store.save_environment(&env);
                app.environments.push(env);
                format!("Imported Postman environment '{name}'.")
            }
            Err(e) => format!("Environment import failed: {e}"),
        },
        ImportFormat::Curl => match import_curl(input) {
            Ok(req) => {
                let url = req.url.clone();
                app.tabs
                    .push(AppTab::Request(Box::new(WorkbenchTab::from_request(req, None))));
                app.active_tab = app.tabs.len() - 1;
                format!("Imported cURL request for '{url}' into a new tab.")
            }
            Err(e) => format!("cURL import failed: {e}"),
        },
        ImportFormat::OpenApi => match import_openapi(input) {
            Ok(col) => {
                let name = col.name.clone();
                let _ = app.json_store.save_collection(&col);
                app.collections.push(col);
                format!("Imported OpenAPI/Swagger spec as collection '{name}'.")
            }
            Err(e) => format!("OpenAPI import failed: {e}"),
        },
        ImportFormat::Har => match import_har(input) {
            Ok(col) => {
                let name = col.name.clone();
                let _ = app.json_store.save_collection(&col);
                app.collections.push(col);
                format!("Imported HAR 1.2 recording as collection '{name}'.")
            }
            Err(e) => format!("HAR import failed: {e}"),
        },
    }
}

fn render_collection_runner_view(ui: &mut Ui, app: &mut ApiTesterApp, pal: &Palette) {
    section_card(ui, pal, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label("Iterations:");
            ui.add(egui::DragValue::new(&mut app.runner_iterations).range(1..=1000));
            ui.label("Delay (ms):");
            ui.add(egui::DragValue::new(&mut app.runner_delay_ms).range(0..=60_000));
            ui.checkbox(&mut app.runner_stop_on_error, "Stop on error");
        });

        ui.add_space(6.0);
        ui.collapsing("Iteration Data (CSV or JSON array)", |ui| {
            ScrollArea::vertical()
                .id_salt("runner_data_scroll")
                .max_height(130.0)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut app.runner_data_input)
                            .desired_rows(4)
                            .desired_width(f32::INFINITY)
                            .hint_text("id,name\n1,Ada\n2,Grace  -- or JSON [{\"id\":\"1\"}]")
                            .code_editor(),
                    );
                });
        });

        ui.add_space(8.0);
        let mut start_col_idx = None;
        ui.horizontal_wrapped(|ui| {
            for (idx, col) in app.collections.iter().enumerate() {
                if ui
                    .add_enabled(
                        !app.runner_in_flight,
                        egui::Button::new(
                            RichText::new(format!("Run '{}'", col.name))
                                .strong()
                                .color(Color32::WHITE),
                        )
                        .fill(pal.accent_primary)
                        .stroke(Stroke::NONE)
                        .rounding(Rounding::same(6.0)),
                    )
                    .clicked()
                {
                    start_col_idx = Some(idx);
                }
            }
            if app.runner_in_flight {
                ui.spinner();
                ui.label("Running collection...");
            }
        });

        if let Some(idx) = start_col_idx {
            let col = app.collections[idx].clone();
            let vars = app.build_variable_context(Some(col.id));
            let trimmed = app.runner_data_input.trim();
            let data_rows = if trimmed.is_empty() {
                Vec::new()
            } else if trimmed.starts_with('[') {
                parse_json_data(trimmed).unwrap_or_default()
            } else {
                parse_csv_data(trimmed).unwrap_or_default()
            };

            let opts = RunnerOptions {
                iterations: app.runner_iterations,
                delay_ms: app.runner_delay_ms,
                data_rows,
                stop_on_error: app.runner_stop_on_error,
                script_timeout_ms: 5_000,
                settings: app.settings.clone(),
            };

            app.runner_in_flight = true;
            app.runner_live_results.clear();
            app.runner_report = None;

            let tx = app.tx.clone();
            app.rt.spawn(async move {
                let (ptx, mut prx) = tokio::sync::mpsc::unbounded_channel();
                let tx_prog = tx.clone();
                let fwd = tokio::spawn(async move {
                    while let Some(ev) = prx.recv().await {
                        let _ = tx_prog.send(UiMessage::RunnerProgress(ev));
                    }
                });
                let report = run_collection(&col, vars, opts, Some(ptx)).await;
                let _ = fwd.await;
                let _ = tx.send(UiMessage::RunnerFinished(report));
            });
        }
    });

    ui.add_space(10.0);

    if let Some(report) = &app.runner_report {
        ui.horizontal_wrapped(|ui| {
            metric_badge(ui, "Collection", &report.collection_name, pal.text_primary, pal);
            metric_badge(ui, "Requests", &format!("{}", report.total_requests), pal.info, pal);
            metric_badge(ui, "Passed Tests", &format!("{}", report.passed_tests), pal.success, pal);
            metric_badge(
                ui,
                "Failed Tests",
                &format!("{}", report.failed_tests),
                if report.failed_tests > 0 || report.failed_requests > 0 {
                    pal.error
                } else {
                    pal.text_muted
                },
                pal,
            );
            metric_badge(ui, "Duration", &format!("{:.1} ms", report.total_duration_ms), pal.text_secondary, pal);
        });
        ui.add_space(8.0);
    }

    section_card(ui, pal, |ui| {
        ScrollArea::vertical().id_salt("runner_results_scroll").show(ui, |ui| {
            egui::Grid::new("runner_results_grid")
                .num_columns(5)
                .striped(true)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.strong("Iter");
                    ui.strong("Request");
                    ui.strong("Status");
                    ui.strong("Time");
                    ui.strong("Tests");
                    ui.end_row();
                    for r in &app.runner_live_results {
                        ui.label(format!("#{}", r.iteration));
                        ui.label(format!("{} {}", r.method, r.request_name));
                        match r.status {
                            Some(code) => status_badge(ui, code, "", pal.is_dark),
                            None => {
                                ui.colored_label(pal.error, r.error.as_deref().unwrap_or("ERR"));
                            }
                        }
                        ui.label(RichText::new(format!("{:.1} ms", r.duration_ms)).color(pal.text_secondary));
                        let passed = r.test_results.iter().filter(|t| t.passed).count();
                        ui.label(format!("{passed}/{}", r.test_results.len()));
                        ui.end_row();
                    }
                });
        });
    });
}

fn render_load_tester_view(ui: &mut Ui, app: &mut ApiTesterApp, pal: &Palette) {
    section_card(ui, pal, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Profile:").color(pal.text_secondary));
            for (kind, label) in [
                (LoadProfileKind::Constant, "Constant"),
                (LoadProfileKind::Ramp, "Ramp"),
                (LoadProfileKind::Spike, "Spike"),
                (LoadProfileKind::Peak, "Peak"),
            ] {
                if underline_tab(ui, app.load_profile_kind == kind, label, pal).clicked() {
                    app.load_profile_kind = kind;
                }
            }
        });

        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.label("VUs:");
            ui.add(egui::DragValue::new(&mut app.load_vus).range(1..=500));
            if app.load_profile_kind != LoadProfileKind::Constant {
                ui.label("Peak/End VUs:");
                ui.add(egui::DragValue::new(&mut app.load_peak_vus).range(1..=500));
            }
            ui.label("Duration (s):");
            ui.add(egui::DragValue::new(&mut app.load_duration_secs).range(1..=300));

            if app.load_in_flight {
                if ui.button(RichText::new("Stop").strong().color(pal.error)).clicked() {
                    if let Some(flag) = &app.load_cancel {
                        flag.store(true, Ordering::Relaxed);
                    }
                }
                ui.spinner();
            } else if primary_button(ui, "Start Load Test", Vec2::new(128.0, 32.0)).clicked() {
                let target_tab = app
                    .tabs
                    .iter()
                    .find_map(AppTab::as_request);
                if let Some(tab) = target_tab {
                    let req = tab.request.clone();
                    let vars = app.build_variable_context(tab.collection_id);
                    let auth = tab
                        .collection_id
                        .and_then(|cid| app.collections.iter().find(|c| c.id == cid))
                        .map(|c| c.auth.clone())
                        .unwrap_or(AuthConfig::NoAuth);
                    let effective_auth = match &req.auth {
                        AuthConfig::Inherit => auth,
                        other => other.clone(),
                    };

                    let profile = match app.load_profile_kind {
                        LoadProfileKind::Constant => LoadProfile::Constant { vus: app.load_vus },
                        LoadProfileKind::Ramp => LoadProfile::Ramp {
                            start_vus: app.load_vus,
                            end_vus: app.load_peak_vus,
                        },
                        LoadProfileKind::Spike => LoadProfile::Spike {
                            base_vus: app.load_vus,
                            spike_vus: app.load_peak_vus,
                            spike_at_pct: 0.4,
                            spike_duration_pct: 0.25,
                        },
                        LoadProfileKind::Peak => LoadProfile::Peak {
                            base_vus: app.load_vus,
                            peak_vus: app.load_peak_vus,
                        },
                    };

                    let cancel = Arc::new(AtomicBool::new(false));
                    app.load_cancel = Some(cancel.clone());
                    app.load_in_flight = true;
                    app.load_timeline.clear();
                    app.load_summary = None;

                    let cfg = LoadTestConfig {
                        duration_secs: app.load_duration_secs,
                        profile,
                        settings: app.settings.clone(),
                        cancel,
                    };
                    let tx = app.tx.clone();
                    app.rt.spawn(async move {
                        let (ptx, mut prx) = tokio::sync::mpsc::unbounded_channel();
                        let tx_prog = tx.clone();
                        let fwd = tokio::spawn(async move {
                            while let Some(up) = prx.recv().await {
                                let _ = tx_prog.send(UiMessage::LoadProgress(up));
                            }
                        });
                        let summary =
                            run_load_test(req, vars, effective_auth, cfg, Some(ptx)).await;
                        let _ = fwd.await;
                        let _ = tx.send(UiMessage::LoadFinished(summary));
                    });
                }
            }
        });
    });

    ui.add_space(10.0);

    if let Some(summary) = &app.load_summary {
        ui.horizontal_wrapped(|ui| {
            metric_badge(ui, "RPS", &format!("{:.1}", summary.rps), pal.success, pal);
            metric_badge(ui, "Requests", &format!("{}", summary.total_requests), pal.info, pal);
            metric_badge(
                ui,
                "Error Rate",
                &format!("{:.2}%", summary.error_rate * 100.0),
                if summary.error_count > 0 { pal.error } else { pal.success },
                pal,
            );
            metric_badge(ui, "p50", &format!("{:.1} ms", summary.p50_ms), pal.text_primary, pal);
            metric_badge(ui, "p90", &format!("{:.1} ms", summary.p90_ms), pal.warning, pal);
            metric_badge(ui, "p95", &format!("{:.1} ms", summary.p95_ms), pal.accent_indicator, pal);
            metric_badge(ui, "p99", &format!("{:.1} ms", summary.p99_ms), pal.error, pal);
        });
        ui.add_space(10.0);
        section_card(ui, pal, |ui| {
            draw_load_chart(ui, &summary.timeline, pal);
        });
    } else if !app.load_timeline.is_empty() {
        section_card(ui, pal, |ui| {
            draw_load_chart(ui, &app.load_timeline, pal);
        });
    } else {
        ui.label(
            RichText::new("Select a load profile and click Start Load Test to benchmark the active request.")
                .color(pal.text_muted),
        );
    }
}

fn metric_badge(ui: &mut Ui, label: &str, value: &str, color: Color32, pal: &Palette) {
    Frame::none()
        .fill(pal.bg_surface)
        .stroke(Stroke::new(1.0_f32, pal.border_subtle))
        .rounding(Rounding::same(6.0))
        .inner_margin(Margin::symmetric(12.0, 8.0))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(label).small().color(pal.text_muted));
                ui.label(RichText::new(value).strong().size(15.0).color(color));
            });
        });
}

fn draw_load_chart(ui: &mut Ui, points: &[LoadTimelinePoint], pal: &Palette) {
    if points.is_empty() {
        return;
    }
    ui.label(
        RichText::new("Throughput (RPS bars) & p95 Latency (burnt-orange line)")
            .small()
            .color(pal.text_secondary),
    );
    ui.add_space(4.0);
    let desired = Vec2::new(ui.available_width().max(320.0), 220.0);
    let (rect, _) = ui.allocate_exact_size(desired, Sense::hover());
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, 6.0, pal.bg_input);
    painter.rect_stroke(rect, 6.0, Stroke::new(1.0_f32, pal.border_subtle));

    let pad = 16.0_f32;
    let plot = Rect::from_min_max(
        Pos2::new(rect.min.x + pad, rect.min.y + pad),
        Pos2::new(rect.max.x - pad, rect.max.y - pad),
    );

    let max_rps = points
        .iter()
        .map(|p| p.requests)
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    let max_p95 = points
        .iter()
        .map(|p| p.p95_latency_ms as f32)
        .fold(1.0_f32, f32::max);

    let n = points.len() as f32;
    let bar_w = (plot.width() / n).max(4.0);
    let mut line_pts = Vec::with_capacity(points.len());

    for (idx, pt) in points.iter().enumerate() {
        let x0 = plot.min.x + idx as f32 * bar_w + 3.0;
        let x1 = (x0 + bar_w - 6.0).max(x0 + 2.0);
        let h_rps = (pt.requests as f32 / max_rps) * plot.height();
        let bar_rect = Rect::from_min_max(
            Pos2::new(x0, plot.max.y - h_rps),
            Pos2::new(x1, plot.max.y),
        );
        painter.rect_filled(
            bar_rect,
            3.0,
            Color32::from_rgba_unmultiplied(pal.success.r(), pal.success.g(), pal.success.b(), 140),
        );

        let cx = (x0 + x1) * 0.5;
        let cy = plot.max.y - (pt.p95_latency_ms as f32 / max_p95) * plot.height();
        line_pts.push(Pos2::new(cx, cy));
    }

    if line_pts.len() >= 2 {
        painter.add(egui::Shape::line(
            line_pts,
            Stroke::new(2.0_f32, pal.accent_indicator),
        ));
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::{tempdir, NamedTempFile};

    use super::*;
    use crate::app::LaunchConfig;

    #[test]
    fn import_from_file_path_loads_postman_v21_collection() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let data_dir = tempdir().unwrap();
        let mut app = ApiTesterApp::new_headless(
            rt.handle().clone(),
            LaunchConfig {
                demo_mode: false,
                data_dir: Some(data_dir.path().to_path_buf()),
                screenshot_dir: None,
            },
        );
        assert!(app.collections.is_empty());

        let mut tmp = NamedTempFile::new().unwrap();
        let postman_json = r#"{
            "info": {
                "name": "File Imported API",
                "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"
            },
            "item": [
                {
                    "name": "Health Check",
                    "request": {
                        "method": "GET",
                        "url": "https://api.example.com/health"
                    }
                }
            ]
        }"#;
        tmp.write_all(postman_json.as_bytes()).unwrap();

        let (format, content, status) = import_from_file_path(&mut app, tmp.path());
        assert_eq!(format, ImportFormat::PostmanCollection);
        assert!(!content.is_empty());
        assert!(status.contains("File Imported API"), "status: {status}");
        assert_eq!(app.collections.len(), 1);
        assert_eq!(app.collections[0].name, "File Imported API");
        assert_eq!(app.collections[0].items.len(), 1);
    }
}
