mod args;
mod config;
mod selector;

use args::Args;
use clap::Parser;
use config::{SsheConfig, read_config_file};
use selector::select_best_endpoint;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let args = Args::parse();

    let config_path = match args.resolve_config_path() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("Error: {err}");
            return ExitCode::FAILURE;
        }
    };

    let config: SsheConfig = match read_config_file(&config_path) {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("Error: {err}");
            return ExitCode::FAILURE;
        }
    };
    config.validate().unwrap_or_else(|err| {
        eprintln!("Error: invalid config: {err}");
        std::process::exit(1);
    });

    let final_config = match config.resolve_host(&args.host_name) {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("Error: {err}");
            return ExitCode::FAILURE;
        }
    };

    let best = match select_best_endpoint(&final_config.host) {
        Ok(result) => result,
        Err(err) => {
            eprintln!("Error: failed to select endpoint: {err}");
            return ExitCode::FAILURE;
        }
    };

    if args.verbose {
        eprintln!("Using config: {}", config_path.display());
        eprintln!(
            "Selected endpoint for '{}': {} ({} ms, mode: {:?})",
            args.host_name, best.endpoint, best.latency_ms, final_config.host.selection_mode
        );
    }

    let destination = format!("{}@{}", final_config.host.user, best.endpoint);
    let mut command = Command::new(&final_config.ssh_bin);
    command
        .arg("-i")
        .arg(&final_config.host.identity_file)
        .arg("-p")
        .arg(final_config.host.port.to_string())
        .arg(destination)
        .args(&args.ssh_args);

    let status = match command.status() {
        Ok(status) => status,
        Err(err) => {
            eprintln!(
                "Error: failed to execute ssh binary '{}': {err}",
                final_config.ssh_bin
            );
            return ExitCode::FAILURE;
        }
    };

    match status.code() {
        Some(code) => ExitCode::from(code as u8),
        None => ExitCode::FAILURE,
    }
}
