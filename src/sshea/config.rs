use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct SsheaConfig {
    pub server_addr: Option<String>,
    pub token_file: Option<String>,
    pub client_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FinalConfig {
    pub server_addr: String,
    pub token_file: PathBuf,
    pub client_name: String,
}

pub fn read_config_file(path: &Path) -> Result<SsheaConfig> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read config file {}", path.display()))?;

    toml::from_str::<SsheaConfig>(&content)
        .with_context(|| format!("failed to parse TOML config {}", path.display()))
}

impl SsheaConfig {
    pub fn resolve(self) -> Result<FinalConfig> {
        let token_file = match self.token_file {
            Some(path) => PathBuf::from(expand_tilde(&path)?),
            None => default_token_path()?,
        };

        Ok(FinalConfig {
            server_addr: self
                .server_addr
                .unwrap_or_else(|| "127.0.0.1:8022".to_string()),
            token_file,
            client_name: self.client_name.unwrap_or_else(|| "sshea".to_string()),
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
    use super::SsheaConfig;

    #[test]
    fn resolve_uses_defaults() {
        let config = SsheaConfig {
            server_addr: None,
            token_file: Some("/tmp/token".to_string()),
            client_name: None,
        };

        let resolved = config.resolve().expect("resolve should succeed");

        assert_eq!(resolved.server_addr, "127.0.0.1:8022");
        assert_eq!(resolved.client_name, "sshea");
    }
}
