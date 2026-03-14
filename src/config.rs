use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    pub cache_ttl_sec: Option<u64>,
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
    pub host_alias: String,
    pub ssh_bin: String,
    pub host: FinalHostConfig,
    pub cache: CacheConfig,
}

#[derive(Debug, Clone)]
pub struct CacheConfig {
    pub ttl_sec: u64,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct FinalHostConfig {
    pub user: String,
    pub port: u16,
    pub identity_file: String,
    pub probe_timeout_ms: u64,
    pub probe_concurrency: usize,
    pub selection_mode: SelectionMode,
    pub endpoints: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
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
        let cache_ttl_sec = global.and_then(|cfg| cfg.cache_ttl_sec).unwrap_or(300);
        let cache_path = resolve_cache_path(global.and_then(|cfg| cfg.cache_path.as_deref()))?;

        Ok(FinalConfig {
            host_alias: host.to_string(),
            ssh_bin,
            host: merge_host_config(global, host_config)?,
            cache: CacheConfig {
                ttl_sec: cache_ttl_sec,
                path: cache_path,
            },
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
                if let Some(ttl) = global.cache_ttl_sec {
                    if ttl == 0 {
                        return Err("global cache_ttl_sec must be greater than 0".to_string());
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
                        1..=65535 => {}
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
        probe_concurrency: global.and_then(|cfg| cfg.probe_concurrency).unwrap_or(4),
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

fn resolve_cache_path(path: Option<&str>) -> Result<PathBuf, String> {
    match path {
        Some(path) => Ok(PathBuf::from(expand_tilde(path)?)),
        None => default_cache_path(),
    }
}

fn default_cache_path() -> Result<PathBuf, String> {
    let output = Command::new("id")
        .arg("-u")
        .output()
        .map_err(|e| format!("failed to execute 'id -u' to get current user ID: {}", e))?;
    let uid = String::from_utf8(output.stdout)
        .map_err(|_| "failed to parse user ID".to_string())?
        .trim()
        .to_string();

    Ok(PathBuf::from(format!("/run/user/{uid}/sshe/cache.toml")))
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
    use super::{HostConfig, SelectionMode, default_cache_path, merge_host_config};

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
        assert_eq!(merged.probe_concurrency, 4);
        assert!(matches!(
            merged.selection_mode,
            SelectionMode::LowestTcpLatency
        ));
    }

    #[test]
    fn default_cache_path_uses_run_user_uid() {
        let path = default_cache_path().expect("path should resolve");
        assert!(path.to_string_lossy().contains("/run/user/"));
        assert!(path.to_string_lossy().ends_with("/sshe/cache.toml"));
    }
}
