use std::path::PathBuf;

use apitester_core::models::ThemeMode;
use apitester_import_export::{generate_code, CodegenLanguage};
use egui::{Event, ViewportCommand};

use crate::app::ApiTesterApp;
use crate::state::{
    ImportFormat, ModalState, RequestSubTab, ResponseSubTab, RunnerModalTab, SidebarTab,
};

const TOUR_STEPS: &[&str] = &[
    "01_http_workbench_dark.png",
    "02_http_workbench_light.png",
    "03_auth_and_scripts.png",
    "04_runner_and_load_tester.png",
    "05_import_export_codegen.png",
    "06_capture_proxy.png",
    "07_realtime_protocols.png",
    "08_import_export_modal.png",
];

/// State machine driving automated `--screenshot-tour <dir>` captures.
pub struct ScreenshotTour {
    output_dir: PathBuf,
    step: usize,
    phase: TourPhase,
    wait_frames: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TourPhase {
    Configure,
    Settle,
    WaitingForImage,
    Done,
}

impl ScreenshotTour {
    pub fn new(output_dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&output_dir);
        Self {
            output_dir,
            step: 0,
            phase: TourPhase::Configure,
            wait_frames: 0,
        }
    }

    pub fn is_done(&self) -> bool {
        self.phase == TourPhase::Done
    }

    /// Advances the screenshot tour state machine by one frame.
    pub fn tick(app: &mut ApiTesterApp, ctx: &egui::Context) {
        let Some(mut tour) = app.screenshot_tour.take() else {
            return;
        };

        // Check for arrived screenshot event first.
        let arrived_image = ctx.input(|i| {
            i.events.iter().find_map(|ev| match ev {
                Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });

        match tour.phase {
            TourPhase::Configure => {
                if tour.step >= TOUR_STEPS.len() {
                    tour.phase = TourPhase::Done;
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                } else {
                    configure_step(app, tour.step);
                    tour.phase = TourPhase::Settle;
                    tour.wait_frames = 4;
                    ctx.request_repaint();
                }
            }
            TourPhase::Settle => {
                if tour.wait_frames > 0 {
                    tour.wait_frames -= 1;
                    ctx.request_repaint();
                } else {
                    ctx.send_viewport_cmd(ViewportCommand::Screenshot);
                    tour.phase = TourPhase::WaitingForImage;
                    tour.wait_frames = 120;
                    ctx.request_repaint();
                }
            }
            TourPhase::WaitingForImage => {
                if let Some(img) = arrived_image {
                    let filename = TOUR_STEPS[tour.step];
                    let path = tour.output_dir.join(filename);
                    let raw_rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                    let _ = image::save_buffer(
                        &path,
                        &raw_rgba,
                        img.width() as u32,
                        img.height() as u32,
                        image::ColorType::Rgba8,
                    );
                    tour.step += 1;
                    tour.phase = TourPhase::Configure;
                    ctx.request_repaint();
                } else if tour.wait_frames > 0 {
                    tour.wait_frames -= 1;
                    ctx.request_repaint();
                } else {
                    // Timed out waiting for backend screenshot; advance to avoid hanging.
                    tour.step += 1;
                    tour.phase = TourPhase::Configure;
                    ctx.request_repaint();
                }
            }
            TourPhase::Done => {
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
        }

        app.screenshot_tour = Some(tour);
    }
}

fn configure_step(app: &mut ApiTesterApp, step: usize) {
    app.modal = ModalState::None;
    app.sidebar_open = true;
    app.sidebar_tab = SidebarTab::Collections;
    app.console_open = false;

    match step {
        0 => {
            // 01_http_workbench_dark.png
            app.settings.theme = ThemeMode::Dark;
            app.active_tab = 0;
            if let Some(tab) = app.tabs.get_mut(0) {
                tab.req_sub_tab = RequestSubTab::Params;
                tab.resp_sub_tab = ResponseSubTab::Body;
            }
        }
        1 => {
            // 02_http_workbench_light.png
            app.settings.theme = ThemeMode::Light;
            app.active_tab = 0;
            if let Some(tab) = app.tabs.get_mut(0) {
                tab.req_sub_tab = RequestSubTab::Params;
                tab.resp_sub_tab = ResponseSubTab::Body;
            }
        }
        2 => {
            // 03_auth_and_scripts.png
            app.settings.theme = ThemeMode::Dark;
            app.active_tab = 0;
            app.console_open = true;
            if let Some(tab) = app.tabs.get_mut(0) {
                tab.req_sub_tab = RequestSubTab::Tests;
                tab.resp_sub_tab = ResponseSubTab::TestResults;
            }
        }
        3 => {
            // 04_runner_and_load_tester.png
            app.settings.theme = ThemeMode::Dark;
            app.runner_modal_tab = RunnerModalTab::LoadTester;
            app.modal = ModalState::RunnerAndLoadTester;
        }
        4 => {
            // 05_import_export_codegen.png
            app.settings.theme = ThemeMode::Dark;
            let code = if let Some(tab) = app.tabs.first() {
                let vars = app.build_variable_context(tab.collection_id);
                generate_code(&tab.request, CodegenLanguage::RustReqwest, &vars)
            } else {
                String::new()
            };
            app.modal = ModalState::CodeSnippet {
                language: CodegenLanguage::RustReqwest,
                code,
            };
        }
        5 => {
            // 06_capture_proxy.png
            app.settings.theme = ThemeMode::Dark;
            app.modal = ModalState::CaptureProxy;
        }
        6 => {
            // 07_realtime_protocols.png
            app.settings.theme = ThemeMode::Dark;
            if app.tabs.len() > 2 {
                app.active_tab = 2; // WebSocket demo tab
            }
        }
        7 => {
            // 08_import_export_modal.png — populate a large multi-hundred-line JSON to verify scroll containment
            app.settings.theme = ThemeMode::Dark;
            app.active_tab = 0;
            let large_json = app
                .collections
                .first()
                .and_then(|c| apitester_import_export::export_postman_collection(c).ok())
                .map(|s| format!("{s}\n{s}\n{s}"))
                .unwrap_or_default();
            app.modal = ModalState::ImportExport {
                format: ImportFormat::PostmanCollection,
                input_text: large_json,
                status: "Ready to import (choose a file, drop one here, or click Import).".into(),
            };
        }
        _ => {}
    }
}
