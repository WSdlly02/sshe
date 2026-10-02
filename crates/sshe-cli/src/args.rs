use clap::{Parser, Subcommand, ValueEnum};
use sshe_protocol::{MAX_EXEC_SECONDS, MAX_HISTORY_QUERY, ProbeKind};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    version,
    about = "Peer diagnostics and non-interactive remote control",
    subcommand_required = true
)]
pub(crate) struct Cli {
    /// Local alias prefixed with @; defaults to @self.
    #[arg(value_parser = parse_target)]
    pub(crate) target: Option<String>,
    #[arg(short, long, global = true)]
    pub(crate) config: Option<PathBuf>,
    /// Machine-readable output, including command stdout/stderr as byte arrays.
    #[arg(long, global = true)]
    pub(crate) json: bool,
    #[command(subcommand)]
    pub(crate) command: Action,
}

#[derive(Subcommand)]
pub(crate) enum Action {
    #[command(flatten)]
    Local(LocalAction),
    #[command(flatten)]
    Remote(RemoteAction),
}

/// Commands that only act on this machine; `@target` is rejected.
#[derive(Subcommand)]
pub(crate) enum LocalAction {
    /// Create identity and config.
    Init,
    /// Print this node's EndpointId.
    Id,
    /// Manage the local address book and inbound whitelist.
    Peer {
        #[command(subcommand)]
        action: PeerAction,
    },
    /// Run scheduled probes and serve peers.
    Daemon,
}

/// Commands that run on `@target`, or on this node by default.
#[derive(Subcommand)]
pub(crate) enum RemoteAction {
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
              value_parser = clap::value_parser!(u64).range(1..=MAX_HISTORY_QUERY as u64))]
        limit: u64,
    },
    /// Run a program without a shell; stdin closed, no PTY.
    Exec {
        /// Seconds before the process group is killed.
        #[arg(long, default_value_t = 30,
              value_parser = clap::value_parser!(u64).range(1..=MAX_EXEC_SECONDS))]
        timeout: u64,
        #[arg(required = true, last = true)]
        command: Vec<String>,
    },
}

#[derive(Subcommand)]
pub(crate) enum PeerAction {
    List,
    Add { alias: String, endpoint_id: String },
    Remove { alias: String },
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum Kind {
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
            Action::Remote(RemoteAction::Exec { command, .. }) => {
                assert_eq!(command, ["printf", "%s", "--json"])
            }
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
        assert!(Cli::try_parse_from(["sshe", "exec", "--timeout", "61", "--", "true"]).is_err());
        assert!(Cli::try_parse_from(["sshe", "peer", "list"]).is_ok());
        assert_eq!(
            Cli::try_parse_from(["sshe", "@self", "history", "--about", "vps"])
                .unwrap()
                .target
                .as_deref(),
            Some("self")
        );
    }
}
