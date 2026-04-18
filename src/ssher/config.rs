use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Deserialize)]
pub struct SsherConfig {
    pub global: Option<GlobalConfig>,
    pub hosts: BTreeMap<String, HostConfig>,
}

#[derive(Debug, Deserialize)]
pub struct GlobalConfig {
    pub probe_timeout_ms: Option<u64>,
    pub cache_ttl_sec: Option<u64>,
    pub cache_path: Option<String>,
    pub selection_mode: Option<SelectionMode>,
}

#[derive(Debug, Deserialize)]
pub struct HostConfig {
    pub probe_timeout_ms: Option<u64>,
    pub selection_mode: Option<SelectionMode>,
    pub endpoints: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct FinalConfig {
    pub host_alias: String,
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
    pub probe_timeout_ms: u64,
    pub selection_mode: SelectionMode,
    pub endpoints: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SelectionMode {
    LowestIcmpLatency,
    LowestTcpLatency,
}
pub fn read_config_file(path: &Path) -> Result<SsherConfig> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read config file {}: ", path.display()))?;

    toml::from_str::<SsherConfig>(&content)
        .with_context(|| format!("failed to parse TOML config {}: ", path.display()))
}

impl SsherConfig {
    pub fn resolve_host(&self, host: &str) -> Result<FinalConfig> {
        let host_config = self
            .hosts
            .get(host)
            .with_context(|| format!("no configuration found for host '{}'", host))?;

        let global = self.global.as_ref();
        let cache_ttl_sec = global.and_then(|cfg| cfg.cache_ttl_sec).unwrap_or(300);
        let cache_path = resolve_cache_path(global.and_then(|cfg| cfg.cache_path.as_deref()))?;

        Ok(FinalConfig {
            host_alias: host.to_string(),
            host: merge_host_config(global, host_config)?,
            cache: CacheConfig {
                ttl_sec: cache_ttl_sec,
                path: cache_path,
            },
        })
    }

    pub fn validate(&self) -> Result<()> {
        match &self.global {
            Some(global) => {
                if let Some(timeout) = global.probe_timeout_ms {
                    if timeout == 0 {
                        bail!("global probe_timeout_ms must be greater than 0");
                    }
                }
                if let Some(ttl) = global.cache_ttl_sec {
                    if ttl == 0 {
                        bail!("global cache_ttl_sec must be greater than 0");
                    }
                }
            }
            None => {}
        }
        match &self.hosts {
            hosts if hosts.is_empty() => {
                bail!("at least one host configuration is required");
            }
            hosts => {
                for (name, host) in hosts {
                    if let Some(timeout) = host.probe_timeout_ms {
                        if timeout == 0 {
                            bail!("host '{}' probe_timeout_ms must be greater than 0", name);
                        }
                    }
                    if host.endpoints.is_empty() {
                        bail!("host '{}' must have at least one endpoint", name);
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
) -> Result<FinalHostConfig> {
    Ok(FinalHostConfig {
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

fn resolve_cache_path(path: Option<&str>) -> Result<PathBuf> {
    match path {
        Some(path) => Ok(PathBuf::from(expand_tilde(path)?)),
        None => default_cache_path(),
    }
}

fn default_cache_path() -> Result<PathBuf> {
    let output = Command::new("id")
        .arg("-u")
        .output()
        .context("failed to execute 'id -u' to get current user ID")?;
    let uid = String::from_utf8(output.stdout)
        .map_err(|_| anyhow!("failed to parse user ID"))?
        .trim()
        .to_string();

    Ok(PathBuf::from(format!(
        "/run/user/{uid}/sshe/ssher_cache.toml"
    )))
}

fn expand_tilde(path: &str) -> Result<String> {
    if path == "~" {
        let home = env::var("HOME").context("HOME is not set")?;
        return Ok(home);
    }

    if let Some(stripped) = path.strip_prefix("~/") {
        let home = env::var("HOME").context("HOME is not set")?;
        return Ok(format!("{home}/{stripped}"));
    }

    Ok(path.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        GlobalConfig, HostConfig, SelectionMode, SsherConfig, default_cache_path,
        merge_host_config, read_config_file,
    };
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn merge_uses_defaults_without_global() {
        let host = HostConfig {
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

    #[test]
    fn default_cache_path_uses_run_user_uid() {
        let path = default_cache_path().expect("path should resolve");
        assert!(path.to_string_lossy().contains("/run/user/"));
        assert!(path.to_string_lossy().ends_with("/sshe/ssher_cache.toml"));
    }

    #[test]
    fn read_config_file_fails_for_invalid_toml() {
        let path = temp_test_path("invalid-config", "toml");
        fs::write(&path, "this is not valid toml = [").expect("failed to write test config");

        let err = read_config_file(&path).expect_err("invalid TOML should fail");
        assert!(err.to_string().contains("failed to parse TOML config"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn validate_rejects_zero_global_cache_ttl() {
        let config = SsherConfig {
            global: Some(GlobalConfig {
                probe_timeout_ms: None,
                cache_ttl_sec: Some(0),
                cache_path: None,
                selection_mode: None,
            }),
            hosts: hosts_with_single_endpoint(),
        };

        let err = config
            .validate()
            .expect_err("zero cache_ttl_sec should fail");
        assert!(
            err.to_string()
                .contains("global cache_ttl_sec must be greater than 0")
        );
    }

    #[test]
    fn validate_rejects_zero_host_probe_timeout() {
        let mut hosts = BTreeMap::new();
        hosts.insert(
            "my-pc".to_string(),
            HostConfig {
                probe_timeout_ms: Some(0),
                selection_mode: None,
                endpoints: vec!["127.0.0.1".to_string()],
            },
        );

        let config = SsherConfig {
            global: None,
            hosts,
        };

        let err = config
            .validate()
            .expect_err("zero host probe_timeout_ms should fail");
        assert!(
            err.to_string()
                .contains("host 'my-pc' probe_timeout_ms must be greater than 0")
        );
    }

    #[test]
    fn validate_rejects_empty_endpoints() {
        let mut hosts = BTreeMap::new();
        hosts.insert(
            "my-pc".to_string(),
            HostConfig {
                probe_timeout_ms: None,
                selection_mode: None,
                endpoints: Vec::new(),
            },
        );

        let config = SsherConfig {
            global: None,
            hosts,
        };

        let err = config.validate().expect_err("empty endpoints should fail");
        assert!(
            err.to_string()
                .contains("host 'my-pc' must have at least one endpoint")
        );
    }

    fn temp_test_path(prefix: &str, ext: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be monotonic enough for tests")
            .as_nanos();
        std::env::temp_dir().join(format!("ssher-{prefix}-{nanos}.{ext}"))
    }

    fn hosts_with_single_endpoint() -> BTreeMap<String, HostConfig> {
        let mut hosts = BTreeMap::new();
        hosts.insert(
            "my-pc".to_string(),
            HostConfig {
                probe_timeout_ms: None,
                selection_mode: None,
                endpoints: vec!["127.0.0.1".to_string()],
            },
        );
        hosts
    }
}
