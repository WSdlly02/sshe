use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, time::Duration};
pub const MAX_PROBE_TIMEOUT_SECS: u64 = 10;
pub const MAX_PEER_TIMEOUT_SECS: u64 = 45;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProbeConfig {
    /// Per-check limit for DNS, TCP and service checks; checks in a group run concurrently.
    pub timeout_secs: u64,
    /// Limit for a peer health check, cold dial included. Kept short so an outage
    /// shows as a failure instead of a slow success; rescue requests wait longer.
    pub peer_timeout_secs: u64,
    pub wan: NetworkTargets,
    pub lan: NetworkTargets,
    pub services: BTreeMap<String, String>,
}

impl Default for ProbeConfig {
    fn default() -> Self {
        Self {
            timeout_secs: 5,
            peer_timeout_secs: 15,
            wan: Default::default(),
            lan: Default::default(),
            services: Default::default(),
        }
    }
}

impl ProbeConfig {
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs)
    }
    pub fn peer_timeout(&self) -> Duration {
        Duration::from_secs(self.peer_timeout_secs)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkTargets {
    pub domains: Vec<String>,
    /// Explicit host:port targets; IP addresses avoid DNS dependence.
    pub tcp: Vec<String>,
}
