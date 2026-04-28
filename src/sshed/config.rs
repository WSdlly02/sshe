use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct SshedConfig {
    pub listen_addr: Option<String>,
    pub token_file: Option<String>,
    pub capabilities: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct FinalConfig {
    pub listen_addr: String,
    pub token_file: PathBuf,
    pub capabilities: Vec<String>,
}

pub fn read_config_file(path: &Path) -> Result<SshedConfig> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read config file {}", path.display()))?;

    toml::from_str::<SshedConfig>(&content)
        .with_context(|| format!("failed to parse TOML config {}", path.display()))
}

impl SshedConfig {
    pub fn resolve(self) -> Result<FinalConfig> {
        let token_file = match self.token_file {
            Some(path) => PathBuf::from(expand_tilde(&path)?),
            None => default_token_path()?,
        };

        Ok(FinalConfig {
            listen_addr: self
                .listen_addr
                .unwrap_or_else(|| "127.0.0.1:8022".to_string()),
            token_file,
            capabilities: self
                .capabilities
                .unwrap_or_else(|| vec!["hello.v1".to_string()]),
        })
    }
}

pub fn read_token(path: &Path) -> Result<String> {
    let token = fs::read_to_string(path)
        .with_context(|| format!("failed to read token file {}", path.display()))?;
    let token = token.trim().to_string();
    if token.is_empty() {
        bail!("token file {} is empty", path.display());
    }
    Ok(token)
}

fn default_token_path() -> Result<PathBuf> {
    let home = env::var("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".config/sshe/sshed.token"))
}

fn expand_tilde(path: &str) -> Result<String> {
    if path == "~" {
        return env::var("HOME").context("HOME is not set");
    }

    if let Some(stripped) = path.strip_prefix("~/") {
        let home = env::var("HOME").context("HOME is not set")?;
        return Ok(format!("{home}/{stripped}"));
    }

    Ok(path.to_string())
}

#[cfg(test)]
mod tests {
    use super::{SshedConfig, read_token};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn resolve_uses_defaults() {
        let config = SshedConfig {
            listen_addr: None,
            token_file: Some("/tmp/token".to_string()),
            capabilities: None,
        };

        let resolved = config.resolve().expect("resolve should succeed");

        assert_eq!(resolved.listen_addr, "127.0.0.1:8022");
        assert_eq!(resolved.capabilities, vec!["hello.v1".to_string()]);
    }

    #[test]
    fn read_token_rejects_empty_file() {
        let path = temp_test_path("empty-token");
        fs::write(&path, " \n").expect("failed to write token file");

        let err = read_token(&path).expect_err("empty token should fail");
        assert!(err.to_string().contains("is empty"));

        let _ = fs::remove_file(path);
    }

    fn temp_test_path(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be monotonic enough for tests")
            .as_nanos();
        std::env::temp_dir().join(format!("sshed-{prefix}-{nanos}.txt"))
    }
}
