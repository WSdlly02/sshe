use crate::{Error, Result, identity, layout::Layout};
use iroh::{EndpointAddr, EndpointId, TransportAddr};
use serde::{Deserialize, Serialize};
use sshe_core::{MAX_PEER_TIMEOUT_SECS, MAX_PROBE_TIMEOUT_SECS, ProbeConfig};
use sshe_protocol::ProbeKind;
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Write,
    net::SocketAddr,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub identity: PathBuf,
    #[serde(default)]
    pub peers: BTreeMap<String, Peer>,
    #[serde(default)]
    pub probe: ProbeConfig,
    #[serde(default)]
    pub daemon: DaemonConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DaemonConfig {
    /// Seconds from completion to the next probe of that kind, including manual rounds.
    pub interval_secs: u64,
    /// Records kept per (kind, method, target); older ones are dropped.
    pub history_size: usize,
    /// Probe kinds to schedule automatically; manual probes of other kinds are still recorded.
    pub probes: Vec<ProbeKind>,
    /// Concurrent requests per source (peers, local CLI); excess ones get a busy reply.
    pub max_concurrent: usize,
    /// Fixed UDP port for IPv4 and IPv6, so a firewall can allow it; random when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind_port: Option<u16>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            interval_secs: 60,
            history_size: 64,
            probes: vec![ProbeKind::Wan, ProbeKind::Peers],
            max_concurrent: 16,
            bind_port: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Peer {
    pub id: EndpointId,
    /// Known direct addresses, tried alongside address lookup so dialing does
    /// not depend on it alone (e.g. a VPS's public IP and `bind_port`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub addrs: Vec<SocketAddr>,
}

impl Peer {
    pub fn addr(&self) -> EndpointAddr {
        EndpointAddr::from_parts(self.id, self.addrs.iter().copied().map(TransportAddr::Ip))
    }
}

pub fn default_path() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .ok_or_else(|| Error::Invalid("HOME or XDG_CONFIG_HOME required".into()))?;
    Ok(base.join("sshe/config.toml"))
}

pub fn read(path: &Path) -> Result<Config> {
    let mut config: Config = toml::from_str(&fs::read_to_string(path)?)?;
    config.validate()?;
    if config.identity.is_relative() {
        config.identity = Layout::new(path).dir().join(&config.identity);
    }
    Ok(config)
}

impl Config {
    pub fn peer(&self, alias: &str) -> Result<&Peer> {
        self.peers
            .get(alias)
            .ok_or_else(|| Error::Invalid(format!("unknown peer: {alias}")))
    }
    pub fn validate(&self) -> Result<()> {
        for alias in self.peers.keys() {
            validate_alias(alias)?;
        }
        let targets = [&self.probe.wan, &self.probe.lan];
        if targets.iter().any(|t| t.domains.len() + t.tcp.len() > 16)
            || self.probe.services.len() > 16
        {
            return Err(Error::Invalid("at most 16 checks per probe group".into()));
        }
        let checks: [(&str, u64, std::ops::RangeInclusive<u64>); 5] = [
            (
                "probe.peer_timeout_secs",
                self.probe.peer_timeout_secs,
                1..=MAX_PEER_TIMEOUT_SECS,
            ),
            (
                "probe.timeout_secs",
                self.probe.timeout_secs,
                1..=MAX_PROBE_TIMEOUT_SECS,
            ),
            ("daemon.interval_secs", self.daemon.interval_secs, 5..=3600),
            (
                "daemon.history_size",
                self.daemon.history_size as u64,
                1..=1024,
            ),
            (
                "daemon.max_concurrent",
                self.daemon.max_concurrent as u64,
                1..=256,
            ),
        ];
        for (name, value, range) in checks {
            if !range.contains(&value) {
                return Err(Error::Invalid(format!(
                    "{name} must be in {}..={}",
                    range.start(),
                    range.end()
                )));
            }
        }
        for service in self.probe.services.values() {
            if !service.starts_with("tcp:") && !service.starts_with("systemd:") {
                return Err(Error::Invalid("service must use tcp: or systemd:".into()));
            }
        }
        Ok(())
    }
}

pub fn validate_alias(alias: &str) -> Result<()> {
    if alias == "self"
        || alias.is_empty()
        || !alias
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
    {
        return Err(Error::Invalid(
            "peer alias must contain letters, digits, _ or -; self is reserved".into(),
        ));
    }
    Ok(())
}

pub fn save(path: &Path, config: &Config) -> Result<()> {
    config.validate()?;
    let layout = Layout::new(path);
    let parent = layout.dir();
    let mut stored = config.clone();
    if let Ok(relative) = stored.identity.strip_prefix(parent) {
        stored.identity = relative.to_path_buf();
    }
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(toml::to_string_pretty(&stored)?.as_bytes())?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| Error::Io(e.error))?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

pub fn init(path: &Path) -> Result<EndpointId> {
    if path.exists() {
        return Err(Error::Invalid(
            "config already exists; refusing to overwrite".into(),
        ));
    }
    let layout = Layout::new(path);
    let dir = layout.dir();
    if !dir.exists() {
        fs::create_dir_all(dir)?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    let key = identity::initialize(&layout.default_identity)?;
    let config = Config {
        identity: layout
            .default_identity
            .file_name()
            .ok_or_else(|| Error::Invalid("identity filename".into()))?
            .into(),
        peers: BTreeMap::new(),
        probe: ProbeConfig::default(),
        daemon: DaemonConfig::default(),
    };
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    file.write_all(toml::to_string_pretty(&config)?.as_bytes())?;
    file.as_file().sync_all()?;
    file.persist_noclobber(path)
        .map_err(|e| Error::Io(e.error))?;
    File::open(dir)?.sync_all()?;
    Ok(key.public())
}
