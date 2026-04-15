use clap::Parser;
use std::env;
use std::path::PathBuf;

/// sshe: 智能选择 SSH 目标地址的包装器
#[derive(Debug, Parser)]
#[command(
    name = "sshed",
    version,
    about = "sshe's daemon mode for ssh command execution",
    long_about = None
)]
pub struct Args {
    /// 配置文件路径
    #[arg(short = 'c', long = "config-file", value_name = "PATH")]
    pub config_file: Option<PathBuf>,

    /// 输出更详细的日志
    #[arg(short = 'v', long = "verbose")]
    pub verbose: bool,
}

impl Args {
    pub fn resolve_config_path(&self) -> Result<PathBuf, String> {
        match self.config_file.as_ref() {
            Some(path) => {
                if path.is_file() {
                    return Ok(path.clone());
                } else {
                    return Err(format!(
                        "config file does not exist or is not a regular file: {}",
                        path.display()
                    ));
                }
            }
            None => {
                let home = match env::var("HOME") {
                    Ok(path) => PathBuf::from(path),
                    Err(_) => {
                        return Err(
                            "HOME is not set, cannot determine default config path".to_string()
                        );
                    }
                };

                let default_paths = [
                    home.join(".ssh/sshed.toml"),
                    home.join(".config/sshed.toml"),
                    home.join(".config/sshe/sshed_config.toml"),
                ];

                if let Some(default_path) = default_paths.iter().find(|p| p.is_file()) {
                    Ok(default_path.clone())
                } else {
                    Err(format!(
                        "default config file does not exist: {}",
                        default_paths
                            .iter()
                            .filter_map(|p| p.to_str())
                            .collect::<Vec<&str>>()
                            .join(", ")
                    ))
                }
            }
        }
    }
}
