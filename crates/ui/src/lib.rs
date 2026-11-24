//! Native GPU-accelerated desktop GUI (`eframe`/`egui`) for APITester.

pub mod app;
pub mod demo;
pub mod screenshot_tour;
pub mod state;
pub mod theme;
pub mod widgets;

pub use app::{ApiTesterApp, LaunchConfig};
