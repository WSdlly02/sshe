use anyhow::{Result, bail};
use clap::Parser;
use std::env;
use std::path::PathBuf;

/// sshea: client for AI-agent-oriented sshed
#[derive(Debug, Parser)]
#[command(version, about = "Connect to sshed and negotiate capabilities", long_about = None)]
pub struct Args {
    /// Config file path
    #[arg(short = 'c', long = "config-file", value_name = "PATH")]
    pub config_file: Option<PathBuf>,

    /// Override server address
    #[arg(long = "server-addr", value_name = "ADDR")]
    pub server_addr: Option<String>,

    /// Override token file path
    #[arg(long = "token-file", value_name = "PATH")]
    pub token_file: Option<PathBuf>,

    /// Output more diagnostics
    #[arg(short = 'v', long = "verbose")]
    pub verbose: bool,
}

impl Args {
    pub fn resolve_config_path(&self) -> Result<PathBuf> {
        match self.config_file.as_ref() {
            Some(path) => {
                if path.is_file() {
                    Ok(path.clone())
                } else {
                    bail!(
                        "config file does not exist or is not a regular file: {}",
                        path.display()
                    )
                }
            }
            None => {
                let home = match env::var("HOME") {
                    Ok(path) => PathBuf::from(path),
                    Err(_) => bail!("HOME is not set, cannot determine default config path"),
                };

                let default_paths = [
                    home.join(".ssh/sshea.toml"),
                    home.join(".config/sshea.toml"),
                    home.join(".config/sshe/sshea_config.toml"),
                ];

                if let Some(default_path) = default_paths.iter().find(|p| p.is_file()) {
                    Ok(default_path.clone())
                } else {
                    bail!(
                        "default config file does not exist: {}",
                        default_paths
                            .iter()
                            .filter_map(|p| p.to_str())
                            .collect::<Vec<&str>>()
                            .join(", ")
                    )
                }
            }
        }
    }
}
