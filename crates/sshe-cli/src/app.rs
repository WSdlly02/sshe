use crate::args::{Action, Cli, LocalAction, PeerAction, RemoteAction};
use anyhow::{Context, Result, bail};
use sshe_node::config;
use sshe_protocol::Request;
use std::path::Path;

pub(crate) async fn run(cli: Cli) -> Result<u8> {
    let path = cli.config.map(Ok).unwrap_or_else(config::default_path)?;
    let path = std::path::absolute(path)?;
    match cli.command {
        Action::Local(action) => {
            if cli.target.as_deref().is_some_and(|t| t != "self") {
                bail!("this command is local-only");
            }
            run_local(action, &path).await?;
            Ok(0)
        }
        Action::Remote(action) => {
            let response = sshe_node::invoke(&path, cli.target, request(action))
                .await
                .context("sshe request failed")?;
            crate::output::render(response, cli.json)
        }
    }
}

fn request(action: RemoteAction) -> Request {
    match action {
        RemoteAction::Probe { kind } => Request::Probe { kind: kind.into() },
        RemoteAction::History { kind, about, limit } => Request::History {
            kind: kind.map(Into::into),
            about,
            limit: limit as usize,
        },
        RemoteAction::Exec { timeout, command } => Request::Exec {
            program: command[0].clone(),
            args: command[1..].to_vec(),
            timeout_secs: timeout,
        },
    }
}

async fn run_local(action: LocalAction, path: &Path) -> Result<()> {
    match action {
        LocalAction::Init => println!(
            "{}",
            config::init(path).context("initialize identity and config")?
        ),
        LocalAction::Id => println!("{}", sshe_node::endpoint_id(&config::read(path)?)?),
        LocalAction::Peer { action } => peer(action, path)?,
        LocalAction::Daemon => sshe_node::daemon(path).await?,
    }
    Ok(())
}

fn peer(action: PeerAction, path: &Path) -> Result<()> {
    let mut cfg = config::read(path)?;
    match action {
        PeerAction::List => {
            println!("{}", serde_json::to_string_pretty(&cfg.peers)?);
            return Ok(());
        }
        PeerAction::Add { alias, endpoint_id } => {
            config::validate_alias(&alias)?;
            if cfg.peers.contains_key(&alias) {
                bail!("alias already exists; remove it explicitly before replacing");
            }
            let id = endpoint_id.parse().context("invalid EndpointId")?;
            cfg.peers.insert(
                alias,
                config::Peer {
                    id,
                    addrs: Vec::new(),
                },
            );
        }
        PeerAction::Remove { alias } => {
            if cfg.peers.remove(&alias).is_none() {
                bail!("unknown peer: {alias}");
            }
        }
    }
    config::save(path, &cfg)?;
    eprintln!("saved {}; restart daemon to apply", path.display());
    Ok(())
}
