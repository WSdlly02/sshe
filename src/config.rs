use serde::Deserialize;
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct SsheConfig {
    pub global: Option<GlobalConfig>,
    pub hosts: BTreeMap<String, HostConfig>,
}

#[derive(Debug, Deserialize)]
pub struct GlobalConfig {
    pub ssh_bin: Option<String>,
    pub probe_timeout_ms: Option<u64>,
    pub probe_concurrency: Option<usize>,
    #[allow(dead_code)]
    pub cache_ttl_sec: Option<u64>,
    #[allow(dead_code)]
    pub cache_path: Option<String>,
    pub selection_mode: Option<SelectionMode>,
}

#[derive(Debug, Deserialize)]
pub struct HostConfig {
    pub user: String,
    pub port: u16,
    pub identity_file: String,
    pub probe_timeout_ms: Option<u64>,
    pub selection_mode: Option<SelectionMode>,
    pub endpoints: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct FinalConfig {
    pub ssh_bin: String,
    pub host: FinalHostConfig,
}

#[derive(Debug, Clone)]
pub struct FinalHostConfig {
    pub user: String,
    pub port: u16,
    pub identity_file: String,
    pub probe_timeout_ms: u64,
    pub selection_mode: SelectionMode,
    pub endpoints: Vec<String>,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum SelectionMode {
    LowestIcmpLatency,
    LowestTcpLatency,
}
pub fn read_config_file(path: &Path) -> Result<SsheConfig, String> {
    let content = fs::read_to_string(path)
        .map_err(|e| format!("failed to read config file {}: {}", path.display(), e))?;

    toml::from_str::<SsheConfig>(&content)
        .map_err(|e| format!("failed to parse TOML config {}: {}", path.display(), e))
}

impl SsheConfig {
    pub fn resolve_host(&self, host: &str) -> Result<FinalConfig, String> {
        let host_config = self
            .hosts
            .get(host)
            .ok_or_else(|| format!("no configuration found for host '{}'", host))?;

        let global = self.global.as_ref();
        let ssh_bin = global
            .and_then(|cfg| cfg.ssh_bin.clone())
            .unwrap_or_else(|| "ssh".to_string());

        Ok(FinalConfig {
            ssh_bin,
            host: merge_host_config(global, host_config)?,
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        match &self.global {
            Some(global) => {
                if let Some(timeout) = global.probe_timeout_ms {
                    if timeout == 0 {
                        return Err("global probe_timeout_ms must be greater than 0".to_string());
                    }
                }
                if let Some(concurrency) = global.probe_concurrency {
                    if concurrency == 0 {
                        return Err("global probe_concurrency must be greater than 0".to_string());
                    }
                }
            }
            None => {}
        }
        match &self.hosts {
            hosts if hosts.is_empty() => {
                return Err("at least one host configuration is required".to_string());
            }
            hosts => {
                for (name, host) in hosts {
                    if host.user.trim().is_empty() {
                        return Err(format!("host '{}' has empty user", name));
                    }
                    match host.port {
                        1..65534 => {}
                        _ => return Err(format!("host '{}' has invalid port {}", name, host.port)),
                    }
                    if host.identity_file.trim().is_empty() {
                        return Err(format!("host '{}' has empty identity_file", name));
                    }
                    if let Some(timeout) = host.probe_timeout_ms {
                        if timeout == 0 {
                            return Err(format!(
                                "host '{}' probe_timeout_ms must be greater than 0",
                                name
                            ));
                        }
                    }
                    if host.endpoints.is_empty() {
                        return Err(format!("host '{}' must have at least one endpoint", name));
                    }
                }
            }
        }
        Ok(())
    }
}

pub fn merge_host_config(
    global: Option<&GlobalConfig>,
    host: &HostConfig,
) -> Result<FinalHostConfig, String> {
    let identity_file = expand_tilde(&host.identity_file)?;

    Ok(FinalHostConfig {
        user: host.user.clone(),
        port: host.port,
        identity_file,
        probe_timeout_ms: host
            .probe_timeout_ms
            .or(global.and_then(|cfg| cfg.probe_timeout_ms))
            .unwrap_or(500),
        selection_mode: host
            .selection_mode
            .or(global.and_then(|cfg| cfg.selection_mode))
            .unwrap_or(SelectionMode::LowestTcpLatency),
        endpoints: host.endpoints.clone(),
    })
}

fn expand_tilde(path: &str) -> Result<String, String> {
    if path == "~" {
        let home = env::var("HOME").map_err(|_| "HOME is not set".to_string())?;
        return Ok(home);
    }

    if let Some(stripped) = path.strip_prefix("~/") {
        let home = env::var("HOME").map_err(|_| "HOME is not set".to_string())?;
        return Ok(format!("{home}/{stripped}"));
    }

    Ok(path.to_string())
}

#[cfg(test)]
mod tests {
    use super::{HostConfig, SelectionMode, merge_host_config};

    #[test]
    fn merge_uses_defaults_without_global() {
        let host = HostConfig {
            user: "alice".to_string(),
            port: 22,
            identity_file: "/tmp/id_rsa".to_string(),
            probe_timeout_ms: None,
            selection_mode: None,
            endpoints: vec!["127.0.0.1".to_string()],
        };

        let merged = merge_host_config(None, &host).expect("merge should succeed");

        assert_eq!(merged.probe_timeout_ms, 500);
        assert!(matches!(
            merged.selection_mode,
            SelectionMode::LowestTcpLatency
        ));
    }
}
