use crate::ssher::config::{CacheConfig, FinalHostConfig, SelectionMode};
use crate::ssher::selector::{ProbeResult, ProbeSource};
use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CacheFile {
    pub entries: BTreeMap<String, CacheEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CacheEntry {
    pub endpoint: String,
    pub latency_ms: u64,
    pub expires_at_unix: u64,
    pub port: u16,
    pub selection_mode: SelectionMode,
}

pub fn load_cached_result(
    cache: &CacheConfig,
    host_alias: &str,
    host: &FinalHostConfig,
    port: u16,
) -> Result<Option<ProbeResult>> {
    let cache_file = read_cache_file(&cache.path)?;
    let now = current_unix_ts()?;

    let Some(entry) = cache_file.entries.get(host_alias) else {
        return Ok(None);
    };

    if entry.expires_at_unix <= now {
        return Ok(None);
    }

    if entry.port != port || entry.selection_mode != host.selection_mode {
        return Ok(None);
    }

    if !host
        .endpoints
        .iter()
        .any(|endpoint| endpoint == &entry.endpoint)
    {
        return Ok(None);
    }

    Ok(Some(ProbeResult {
        endpoint: entry.endpoint.clone(),
        latency_ms: entry.latency_ms as u128,
        source: ProbeSource::Cache,
    }))
}

pub fn store_cached_result(
    cache: &CacheConfig,
    host_alias: &str,
    host: &FinalHostConfig,
    port: u16,
    result: &ProbeResult,
) -> Result<()> {
    let mut cache_file = read_cache_file(&cache.path)?;
    let now = current_unix_ts()?;

    cache_file.entries.insert(
        host_alias.to_string(),
        CacheEntry {
            endpoint: result.endpoint.clone(),
            latency_ms: result.latency_ms.min(u64::MAX as u128) as u64,
            expires_at_unix: now.saturating_add(cache.ttl_sec),
            port,
            selection_mode: host.selection_mode,
        },
    );

    write_cache_file(&cache.path, &cache_file)
}

fn read_cache_file(path: &Path) -> Result<CacheFile> {
    match fs::read_to_string(path) {
        Ok(content) => toml::from_str(&content)
            .with_context(|| format!("failed to parse cache file {}", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(CacheFile::default()),
        Err(err) => Err(anyhow!(
            "failed to read cache file {}: {}",
            path.display(),
            err
        )),
    }
}

fn write_cache_file(path: &Path, cache: &CacheFile) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("cache path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create cache directory {}", parent.display()))?;

    let content = toml::to_string(cache)
        .with_context(|| format!("failed to serialize cache file {}", path.display()))?;
    fs::write(path, content)
        .with_context(|| format!("failed to write cache file {}", path.display()))
}

fn current_unix_ts() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|err| anyhow!("system clock error: {}", err))
}

#[cfg(test)]
mod tests {
    use super::load_cached_result;
    use crate::ssher::config::{CacheConfig, FinalHostConfig, SelectionMode};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn load_cached_result_fails_for_invalid_toml() {
        let path = temp_test_path("invalid-cache", "toml");
        fs::write(&path, "entries = [broken").expect("failed to write invalid cache file");

        let err = load_cached_result(
            &CacheConfig {
                ttl_sec: 300,
                path: path.clone(),
            },
            "my-pc",
            &test_host(),
            22,
        )
        .expect_err("invalid cache TOML should fail");

        assert!(err.to_string().contains("failed to parse cache file"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn load_cached_result_returns_none_when_cache_file_missing() {
        let path = temp_test_path("missing-cache", "toml");
        let _ = fs::remove_file(&path);

        let result = load_cached_result(
            &CacheConfig { ttl_sec: 300, path },
            "my-pc",
            &test_host(),
            22,
        )
        .expect("missing cache file should be treated as empty cache");

        assert!(result.is_none());
    }

    fn test_host() -> FinalHostConfig {
        FinalHostConfig {
            probe_timeout_ms: 500,
            selection_mode: SelectionMode::LowestTcpLatency,
            endpoints: vec!["127.0.0.1".to_string()],
        }
    }

    fn temp_test_path(prefix: &str, ext: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be monotonic enough for tests")
            .as_nanos();
        std::env::temp_dir().join(format!("ssher-{prefix}-{nanos}.{ext}"))
    }
}
