mod app;
mod proxy;
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    match app::run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("Error: {err:#}");
            ExitCode::FAILURE
        }
    }
}
