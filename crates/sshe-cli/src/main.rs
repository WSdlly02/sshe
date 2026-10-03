mod app;
mod args;
mod output;
use args::{Action, Cli, LocalAction};
use clap::Parser;
use std::process::ExitCode;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    init_logging(matches!(cli.command, Action::Local(LocalAction::Daemon)));
    match app::run(cli).await {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// Logs go to stderr; `RUST_LOG` overrides the default. The daemon also shows
/// iroh warnings; one-shot commands keep stderr for results and their own errors.
/// Under systemd, journald adds timestamps and keeps the history, so ours are left out.
fn init_logging(daemon: bool) {
    let default = if daemon {
        "warn,sshe_node=info"
    } else {
        "off,sshe_node=warn"
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    let logs = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false);
    if std::env::var_os("JOURNAL_STREAM").is_some() {
        logs.without_time().with_ansi(false).init();
    } else {
        logs.init();
    }
}
