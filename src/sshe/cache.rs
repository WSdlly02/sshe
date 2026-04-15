use crate::sshe::config::{CacheConfig, FinalHostConfig, SelectionMode};
use crate::sshe::selector::{ProbeResult, ProbeSource};
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
) -> Result<Option<ProbeResult>, String> {
    let cache_file = read_cache_file(&cache.path)?;
    let now = current_unix_ts()?;

    let Some(entry) = cache_file.entries.get(host_alias) else {
        return Ok(None);
    };

    if entry.expires_at_unix <= now {
        return Ok(None);
    }

    if entry.port != host.port || entry.selection_mode != host.selection_mode {
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
    result: &ProbeResult,
) -> Result<(), String> {
    let mut cache_file = read_cache_file(&cache.path)?;
    let now = current_unix_ts()?;

    cache_file.entries.insert(
        host_alias.to_string(),
        CacheEntry {
            endpoint: result.endpoint.clone(),
            latency_ms: result.latency_ms.min(u64::MAX as u128) as u64,
            expires_at_unix: now.saturating_add(cache.ttl_sec),
            port: host.port,
            selection_mode: host.selection_mode,
        },
    );

    write_cache_file(&cache.path, &cache_file)
}

fn read_cache_file(path: &Path) -> Result<CacheFile, String> {
    match fs::read_to_string(path) {
        Ok(content) => toml::from_str(&content)
            .map_err(|err| format!("failed to parse cache file {}: {err}", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(CacheFile::default()),
        Err(err) => Err(format!(
            "failed to read cache file {}: {err}",
            path.display()
        )),
    }
}

fn write_cache_file(path: &Path, cache: &CacheFile) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("cache path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent).map_err(|err| {
        format!(
            "failed to create cache directory {}: {err}",
            parent.display()
        )
    })?;

    let content = toml::to_string(cache)
        .map_err(|err| format!("failed to serialize cache file {}: {err}", path.display()))?;
    fs::write(path, content)
        .map_err(|err| format!("failed to write cache file {}: {err}", path.display()))
}

fn current_unix_ts() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|err| format!("system clock error: {err}"))
}
