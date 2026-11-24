use std::path::PathBuf;

use apitester_ui::{ApiTesterApp, LaunchConfig};

fn parse_args() -> LaunchConfig {
    let mut cfg = LaunchConfig::default();
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--demo" | "--demo-mode" => cfg.demo_mode = true,
            "--data-dir" => {
                if let Some(dir) = args.next() {
                    cfg.data_dir = Some(PathBuf::from(dir));
                }
            }
            "--screenshot-tour" => {
                cfg.demo_mode = true;
                if let Some(dir) = args.next() {
                    cfg.screenshot_dir = Some(PathBuf::from(dir));
                }
            }
            _ => {}
        }
    }

    cfg
}

fn main() -> eframe::Result<()> {
    let config = parse_args();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to start tokio runtime");

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("APITester")
            .with_inner_size([1360.0, 860.0])
            .with_min_inner_size([960.0, 600.0]),
        ..Default::default()
    };

    eframe::run_native(
        "APITester",
        native_options,
        Box::new(move |cc| Ok(Box::new(ApiTesterApp::new(cc, runtime.handle().clone(), config)))),
    )
}
