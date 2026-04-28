use anyhow::{Context, Result, bail};
use clap::Parser;
use sshe::protocol::{ClientFrame, PROTOCOL_VERSION, ServerFrame, read_frame, write_frame};
use sshe::sshea;
use tokio::net::TcpStream;

#[tokio::main]
async fn main() -> Result<()> {
    let args = sshea::args::Args::parse();
    let config_path = args.resolve_config_path()?;
    let mut config = sshea::config::read_config_file(&config_path)?.resolve()?;

    if let Some(server_addr) = args.server_addr {
        config.server_addr = server_addr;
    }
    if let Some(token_file) = args.token_file {
        config.token_file = token_file;
    }

    let token = sshea::config::read_token(&config.token_file)?;
    let mut stream = TcpStream::connect(&config.server_addr)
        .await
        .with_context(|| format!("failed to connect {}", config.server_addr))?;

    write_frame(
        &mut stream,
        &ClientFrame::Hello {
            protocol_version: PROTOCOL_VERSION,
            client_name: config.client_name.clone(),
            token,
        },
    )
    .await
    .context("failed to send hello")?;

    let response: ServerFrame = read_frame(&mut stream)
        .await
        .context("failed to read hello response")?;

    match response {
        ServerFrame::HelloAccepted {
            protocol_version,
            capabilities,
        } => {
            if protocol_version != PROTOCOL_VERSION {
                bail!("server returned unsupported protocol version {protocol_version}");
            }
            if args.verbose {
                eprintln!("using config {}", config_path.display());
                eprintln!("connected to {}", config.server_addr);
            }
            println!("{}", capabilities.join(","));
        }
        ServerFrame::Error { code, message } => {
            bail!("server rejected hello: {code}: {message}");
        }
    }

    Ok(())
}
