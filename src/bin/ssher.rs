use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use sshe::ssher;
use std::process::ExitCode;
use tokio::io::{self, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{self, Duration};

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("Error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<()> {
    let args = ssher::args::Args::parse();

    if args.port == 0 {
        bail!("--port must be between 1 and 65535");
    }

    let config_path = args.resolve_config_path()?;
    let config = ssher::config::read_config_file(&config_path)?;
    config.validate().context("invalid config")?;
    let final_config = config.resolve_host(&args.host)?;

    let best = if args.refresh_cache {
        let probed = ssher::selector::select_best_endpoint(&final_config.host, args.port)
            .await
            .context("failed to select endpoint")?;

        if let Err(err) = ssher::cache::store_cached_result(
            &final_config.cache,
            &final_config.host_alias,
            &final_config.host,
            args.port,
            &probed,
        ) {
            eprintln!("Warning: failed to update cache: {err:#}");
        }

        probed
    } else {
        match ssher::cache::load_cached_result(
            &final_config.cache,
            &final_config.host_alias,
            &final_config.host,
            args.port,
        ) {
            Ok(Some(result)) => result,
            Ok(None) => {
                let probed = ssher::selector::select_best_endpoint(&final_config.host, args.port)
                    .await
                    .context("failed to select endpoint")?;

                if let Err(err) = ssher::cache::store_cached_result(
                    &final_config.cache,
                    &final_config.host_alias,
                    &final_config.host,
                    args.port,
                    &probed,
                ) {
                    eprintln!("Warning: failed to update cache: {err:#}");
                }

                probed
            }
            Err(err) => {
                eprintln!("Warning: failed to read cache: {err:#}");
                let probed = ssher::selector::select_best_endpoint(&final_config.host, args.port)
                    .await
                    .context("failed to select endpoint")?;

                if let Err(store_err) = ssher::cache::store_cached_result(
                    &final_config.cache,
                    &final_config.host_alias,
                    &final_config.host,
                    args.port,
                    &probed,
                ) {
                    eprintln!("Warning: failed to update cache: {store_err:#}");
                }

                probed
            }
        }
    };

    if args.verbose {
        let source = match best.source {
            ssher::selector::ProbeSource::Cache => "cache",
            ssher::selector::ProbeSource::Probe => "probe",
        };
        eprintln!("Using config: {}", config_path.display());
        if args.refresh_cache {
            eprintln!("Cache policy: refresh requested, skipping cached entry");
        }
        eprintln!(
            "Selected endpoint for '{}': {}:{} ({} ms, mode: {:?}, source: {})",
            args.host,
            best.endpoint,
            args.port,
            best.latency_ms,
            final_config.host.selection_mode,
            source
        );
        eprintln!("Cache path: {}", final_config.cache.path.display());
    }

    proxy_tcp_stdio(
        &best.endpoint,
        args.port,
        final_config.host.probe_timeout_ms,
    )
    .await
    .context("failed to proxy TCP stream")
}

async fn proxy_tcp_stdio(endpoint: &str, port: u16, timeout_ms: u64) -> Result<()> {
    let address = format!("{endpoint}:{port}");
    let stream = time::timeout(
        Duration::from_millis(timeout_ms),
        TcpStream::connect(&address),
    )
    .await
    .map_err(|_| anyhow!("connect timeout: {address}"))?
    .with_context(|| format!("connect failed for {address}"))?;

    let (mut reader, mut writer) = stream.into_split();
    let mut stdin = io::stdin();
    let mut stdout = io::stdout();

    let stdin_to_socket = async {
        io::copy(&mut stdin, &mut writer)
            .await
            .context("stdin->socket copy failed")?;
        writer.shutdown().await.context("socket shutdown failed")?;
        Ok::<(), anyhow::Error>(())
    };

    let socket_to_stdout = async {
        io::copy(&mut reader, &mut stdout)
            .await
            .context("socket->stdout copy failed")?;
        stdout.flush().await.context("stdout flush failed")?;
        Ok::<(), anyhow::Error>(())
    };

    let (left, right) = tokio::join!(stdin_to_socket, socket_to_stdout);
    left?;
    right?;
    Ok(())
}
