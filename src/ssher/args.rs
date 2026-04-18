use anyhow::{Result, bail};
use clap::Parser;
use std::env;
use std::path::PathBuf;

/// ssher: 供 OpenSSH ProxyCommand 调用的地址选择器
#[derive(Debug, Parser)]
#[command(
    version,
    about = "Resolve the best endpoint and proxy TCP stdio for OpenSSH",
    long_about = None
)]
pub struct Args {
    /// 配置文件路径
    #[arg(short = 'c', long = "config-file", value_name = "PATH")]
    pub config_file: Option<PathBuf>,

    /// 忽略缓存并强制重新测速
    #[arg(long = "refresh-cache")]
    pub refresh_cache: bool,

    /// 输出更详细的日志
    #[arg(short = 'v', long = "verbose")]
    pub verbose: bool,

    /// 逻辑主机名，建议在 ssh_config 中传入 %n
    #[arg(long = "host", value_name = "HOST", required = true)]
    pub host: String,

    /// ssh_config 中传入的 %p
    #[arg(long = "port", value_name = "PORT", required = true)]
    pub port: u16,
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
                    home.join(".ssh/ssher.toml"),
                    home.join(".config/ssher.toml"),
                    home.join(".config/sshe/ssher_config.toml"),
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
