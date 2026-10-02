use crate::proxy::proxy_tcp_stdio;
use anyhow::{Context, Result, bail};
use clap::Parser;
use ssher::ssher;
use ssher::{config::FinalConfig, selector::ProbeResult};

pub(crate) async fn run() -> Result<()> {
    let args = ssher::args::Args::parse();

    if args.port == 0 {
        bail!("--port must be between 1 and 65535");
    }

    let config_path = args.resolve_config_path()?;
    let config = ssher::config::read_config_file(&config_path)?;
    config.validate().context("invalid config")?;
    let final_config = config.resolve_host(&args.host)?;

    let best = if args.refresh_cache {
        probe_and_store(&final_config, args.port).await?
    } else {
        match ssher::cache::load_cached_result(
            &final_config.cache,
            &final_config.host_alias,
            &final_config.host,
            args.port,
        ) {
            Ok(Some(result)) => result,
            Ok(None) => probe_and_store(&final_config, args.port).await?,
            Err(err) => {
                eprintln!("Warning: failed to read cache: {err:#}");
                probe_and_store(&final_config, args.port).await?
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

/// Probes now and refreshes the cache; a cache write failure only warns.
async fn probe_and_store(config: &FinalConfig, port: u16) -> Result<ProbeResult> {
    let probed = ssher::selector::select_best_endpoint(&config.host, port)
        .await
        .context("failed to select endpoint")?;
    if let Err(err) = ssher::cache::store_cached_result(
        &config.cache,
        &config.host_alias,
        &config.host,
        port,
        &probed,
    ) {
        eprintln!("Warning: failed to update cache: {err:#}");
    }
    Ok(probed)
}
