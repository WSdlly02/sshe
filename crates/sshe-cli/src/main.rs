use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use sshe_node::config;
use sshe_protocol::{ProbeKind, Request, Response};
use std::{io::Write, path::PathBuf, process::ExitCode};

#[derive(Parser)]
#[command(
    version,
    about = "Peer diagnostics and non-interactive remote control",
    subcommand_required = true
)]
struct Cli {
    /// Local alias prefixed with @; defaults to @self.
    #[arg(value_parser = parse_target)]
    target: Option<String>,
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,
    /// Machine-readable output, including command stdout/stderr as byte arrays.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Action,
}
#[derive(Subcommand)]
enum Action {
    /// Create identity and config (local only).
    Init,
    /// Print this node's EndpointId (local only).
    Id,
    /// Manage the local address book and inbound whitelist (local only).
    Peer {
        #[command(subcommand)]
        action: PeerAction,
    },
    /// Run scheduled probes and serve peers (local only).
    Daemon,
    /// Run one probe now.
    Probe {
        #[arg(value_enum)]
        kind: Kind,
    },
    /// Show results recorded by the daemon's scheduled probes; never probes.
    History {
        #[arg(long, value_enum)]
        kind: Option<Kind>,
        /// Only records about this target or label, such as a peer alias
        /// as named on the queried node.
        #[arg(long)]
        about: Option<String>,
        /// Most recent records to show.
        #[arg(short = 'n', long, default_value_t = 50,
              value_parser = clap::value_parser!(u64).range(1..=sshe_protocol::MAX_HISTORY_QUERY as u64))]
        limit: u64,
    },
    /// Run a program without a shell; stdin closed, no PTY.
    Exec {
        #[arg(long, default_value_t = 30)]
        timeout: u64,
        #[arg(required = true, last = true)]
        command: Vec<String>,
    },
}
#[derive(Subcommand)]
enum PeerAction {
    List,
    Add { alias: String, endpoint_id: String },
    Remove { alias: String },
}
#[derive(Clone, Copy, ValueEnum)]
enum Kind {
    Host,
    Lan,
    Wan,
    Services,
    Peers,
}
impl From<Kind> for ProbeKind {
    fn from(kind: Kind) -> Self {
        match kind {
            Kind::Host => ProbeKind::Host,
            Kind::Lan => ProbeKind::Lan,
            Kind::Wan => ProbeKind::Wan,
            Kind::Services => ProbeKind::Services,
            Kind::Peers => ProbeKind::Peers,
        }
    }
}
fn parse_target(value: &str) -> std::result::Result<String, String> {
    value
        .strip_prefix('@')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| "target must be @alias or @self".into())
}
#[tokio::main]
async fn main() -> ExitCode {
    match run(Cli::parse()).await {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
async fn run(cli: Cli) -> Result<u8> {
    let path = cli.config.map(Ok).unwrap_or_else(config::default_path)?;
    let path = std::path::absolute(path)?;
    let remote = cli.target.as_deref().is_some_and(|t| t != "self");
    let request = match cli.command {
        Action::Probe { kind } => Request::Probe { kind: kind.into() },
        Action::History { kind, about, limit } => Request::History {
            kind: kind.map(Into::into),
            about,
            limit: limit as usize,
        },
        Action::Exec { timeout, command } => {
            if !(1..=sshe_protocol::MAX_EXEC_SECONDS).contains(&timeout) {
                bail!("timeout must be 1..=60 seconds");
            }
            Request::Exec {
                program: command[0].clone(),
                args: command[1..].to_vec(),
                timeout_secs: timeout,
            }
        }
        action => {
            if remote {
                bail!("this command is local-only");
            }
            match action {
                Action::Init => println!(
                    "{}",
                    config::init(&path).context("initialize identity and config")?
                ),
                Action::Id => println!(
                    "{}",
                    config::load_key(&config::read(&path)?.identity)?.public()
                ),
                Action::Peer {
                    action: PeerAction::List,
                } => println!(
                    "{}",
                    serde_json::to_string_pretty(&config::read(&path)?.peers)?
                ),
                Action::Peer { action } => {
                    let mut cfg = config::read(&path)?;
                    match action {
                        PeerAction::List => unreachable!(),
                        PeerAction::Add { alias, endpoint_id } => {
                            config::validate_alias(&alias)?;
                            if cfg.peers.contains_key(&alias) {
                                bail!(
                                    "alias already exists; remove it explicitly before replacing"
                                );
                            }
                            cfg.peers.insert(
                                alias,
                                config::Peer {
                                    id: endpoint_id.parse().context("invalid EndpointId")?,
                                },
                            );
                        }
                        PeerAction::Remove { alias } => {
                            if cfg.peers.remove(&alias).is_none() {
                                bail!("unknown peer: {alias}");
                            }
                        }
                    }
                    config::save(&path, &cfg)?;
                    eprintln!("saved {}; restart daemon to apply", path.display());
                }
                Action::Daemon => sshe_node::daemon(&path).await?,
                _ => unreachable!(),
            }
            return Ok(0);
        }
    };
    match sshe_node::invoke(&path, cli.target, request)
        .await
        .context("sshe request failed")?
    {
        Response::Probe(report) => println!("{}", serde_json::to_string_pretty(&report)?),
        Response::History(report) => println!("{}", serde_json::to_string_pretty(&report)?),
        Response::Health(_) => bail!("unexpected health response"),
        Response::Error(message) => bail!("{message}"),
        Response::Exec(result) => {
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                std::io::stdout().write_all(&result.stdout)?;
                std::io::stderr().write_all(&result.stderr)?;
            }
            return Ok(result
                .exit_code
                .unwrap_or_else(|| 128 + result.signal.unwrap_or(1))
                .clamp(0, 255) as u8);
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn target_and_exec_arguments_are_unambiguous() {
        let cli = Cli::try_parse_from(["sshe", "@desktop", "exec", "--", "printf", "%s", "--json"])
            .unwrap();
        assert_eq!(cli.target.as_deref(), Some("desktop"));
        assert!(!cli.json);
        match cli.command {
            Action::Exec { command, .. } => assert_eq!(command, ["printf", "%s", "--json"]),
            _ => panic!(),
        }
        assert!(
            Cli::try_parse_from(["sshe", "probe", "host"])
                .unwrap()
                .target
                .is_none()
        );
        assert!(Cli::try_parse_from(["sshe", "status"]).is_err());
        assert!(Cli::try_parse_from(["sshe", "history", "-n", "0"]).is_err());
        assert_eq!(
            Cli::try_parse_from(["sshe", "@self", "history", "--about", "vps"])
                .unwrap()
                .target
                .as_deref(),
            Some("self")
        );
    }
}
