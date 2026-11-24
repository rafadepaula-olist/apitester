//! Collection runner and native async load testing engine for apitester.

pub mod collection_runner;
pub mod error;
pub mod load_tester;

pub use apitester_core::models::Collection;
pub use collection_runner::{
    parse_csv_data, parse_json_data, run_collection, CollectionRunReport, RequestRunResult,
    RunnerOptions, RunnerProgressEvent,
};
pub use error::RunnerError;
pub use load_tester::{
    run_load_test, LoadProfile, LoadProgressUpdate, LoadTestConfig, LoadTestSummary,
    LoadTimelinePoint,
};
