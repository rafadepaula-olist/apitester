use std::process::ExitCode;

use apitester_cli::{execute, Cli};
use clap::Parser;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match execute(cli).await {
        Ok(out) => {
            if !out.stdout.is_empty() {
                print!("{}", out.stdout);
            }
            if out.exit_code == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(out.exit_code as u8)
            }
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
