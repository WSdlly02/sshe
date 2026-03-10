use clap::Parser;
use std::env;
use std::path::PathBuf;

/// sshe: 智能选择 SSH 目标地址的包装器
#[derive(Debug, Parser)]
#[command(
    name = "sshe",
    version,
    about = "A modern wrapper of ssh/sshd",
    long_about = None
)]
pub struct Args {
    /// 配置文件路径
    #[arg(short = 'c', long = "config-file", value_name = "PATH")]
    pub config_file: Option<PathBuf>,

    /// 输出更详细的日志
    #[arg(short = 'v', long = "verbose")]
    pub verbose: bool,

    /// 逻辑主机名
    #[arg(value_name = "HOST_NAME")]
    pub host_name: String,

    /// 传递给 ssh 的参数
    #[arg(value_name = "SSH_ARGS", trailing_var_arg = true)]
    pub ssh_args: Vec<String>,
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
                let home = env::var_os("HOME").ok_or_else(|| {
                    "HOME is not set, cannot determine default config path".to_string()
                })?;

                let default_path_1 = PathBuf::from(&home).join(".ssh/sshe.toml");
                let default_path_2 = PathBuf::from(&home).join(".config/sshe.toml");
                let default_path_3 = PathBuf::from(&home).join(".config/sshe/config.toml");

                if default_path_1.is_file() {
                    Ok(default_path_1)
                } else if default_path_2.is_file() {
                    Ok(default_path_2)
                } else if default_path_3.is_file() {
                    Ok(default_path_3)
                } else {
                    Err(format!(
                        "default config file does not exist: {} {} {}",
                        default_path_1.display(),
                        default_path_2.display(),
                        default_path_3.display()
                    ))
                }
            }
        }
    }
}
