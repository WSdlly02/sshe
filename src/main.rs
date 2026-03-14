mod args;
mod cache;
mod config;
mod selector;

use args::Args;
use cache::{load_cached_result, store_cached_result};
use clap::Parser;
use config::{SsheConfig, read_config_file};
use selector::{ProbeSource, select_best_endpoint};
use std::process::{Command, ExitCode};

#[tokio::main]
async fn main() -> ExitCode {
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
    if let Err(err) = config.validate() {
        eprintln!("Error: invalid config: {err}");
        return ExitCode::FAILURE;
    }

    let final_config = match config.resolve_host(&args.host_name) {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("Error: {err}");
            return ExitCode::FAILURE;
        }
    };

    let best = if args.refresh_cache {
        let probed = match select_best_endpoint(&final_config.host).await {
            Ok(result) => result,
            Err(err) => {
                eprintln!("Error: failed to select endpoint: {err}");
                return ExitCode::FAILURE;
            }
        };

        if let Err(err) = store_cached_result(
            &final_config.cache,
            &final_config.host_alias,
            &final_config.host,
            &probed,
        ) {
            eprintln!("Warning: failed to update cache: {err}");
        }

        probed
    } else {
        match load_cached_result(
            &final_config.cache,
            &final_config.host_alias,
            &final_config.host,
        ) {
            Ok(Some(result)) => result,
            Ok(None) => {
                let probed = match select_best_endpoint(&final_config.host).await {
                    Ok(result) => result,
                    Err(err) => {
                        eprintln!("Error: failed to select endpoint: {err}");
                        return ExitCode::FAILURE;
                    }
                };

                if let Err(err) = store_cached_result(
                    &final_config.cache,
                    &final_config.host_alias,
                    &final_config.host,
                    &probed,
                ) {
                    eprintln!("Warning: failed to update cache: {err}");
                }

                probed
            }
            Err(err) => {
                eprintln!("Warning: failed to read cache: {err}");
                let probed = match select_best_endpoint(&final_config.host).await {
                    Ok(result) => result,
                    Err(probe_err) => {
                        eprintln!("Error: failed to select endpoint: {probe_err}");
                        return ExitCode::FAILURE;
                    }
                };

                if let Err(store_err) = store_cached_result(
                    &final_config.cache,
                    &final_config.host_alias,
                    &final_config.host,
                    &probed,
                ) {
                    eprintln!("Warning: failed to update cache: {store_err}");
                }

                probed
            }
        }
    };

    if args.verbose {
        let source = match best.source {
            ProbeSource::Cache => "cache",
            ProbeSource::Probe => "probe",
        };
        eprintln!("Using config: {}", config_path.display());
        if args.refresh_cache {
            eprintln!("Cache policy: refresh requested, skipping cached entry");
        }
        eprintln!(
            "Selected endpoint for '{}': {} ({} ms, mode: {:?}, source: {})",
            args.host_name,
            best.endpoint,
            best.latency_ms,
            final_config.host.selection_mode,
            source
        );
        eprintln!("Cache path: {}", final_config.cache.path.display());
    }

    let destination = format!("{}@{}", final_config.host.user, best.endpoint);
    let mut command = Command::new(&final_config.ssh_bin);
    command
        .arg("-i")
        .arg(&final_config.host.identity_file)
        .arg("-p")
        .arg(&final_config.host.port.to_string())
        .arg(&destination)
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
