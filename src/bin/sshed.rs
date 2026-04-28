use anyhow::{Context, Result};
use clap::Parser;
use sshe::protocol::{ClientFrame, PROTOCOL_VERSION, ServerFrame, read_frame, write_frame};
use sshe::sshed;
use tokio::net::{TcpListener, TcpStream};

#[tokio::main]
async fn main() -> Result<()> {
    let args = sshed::args::Args::parse();
    let config_path = args.resolve_config_path()?;
    let config = sshed::config::read_config_file(&config_path)?.resolve()?;
    let token = sshed::config::read_token(&config.token_file)?;

    let listener = TcpListener::bind(&config.listen_addr)
        .await
        .with_context(|| format!("failed to bind {}", config.listen_addr))?;
    if args.verbose {
        eprintln!("sshed listening on {}", config.listen_addr);
        eprintln!("using config {}", config_path.display());
    }

    loop {
        let (stream, peer_addr) = listener.accept().await.context("failed to accept client")?;
        let token = token.clone();
        let capabilities = config.capabilities.clone();
        let verbose = args.verbose;

        tokio::spawn(async move {
            if let Err(err) = handle_client(stream, token, capabilities).await {
                if verbose {
                    eprintln!("client {peer_addr} failed: {err:#}");
                }
            } else if verbose {
                eprintln!("client {peer_addr} authenticated");
            }
        });
    }
}

async fn handle_client(
    mut stream: TcpStream,
    expected_token: String,
    capabilities: Vec<String>,
) -> Result<()> {
    let frame: ClientFrame = read_frame(&mut stream)
        .await
        .context("failed to read hello")?;
    match frame {
        ClientFrame::Hello {
            protocol_version,
            client_name: _,
            token,
        } => {
            if protocol_version != PROTOCOL_VERSION {
                write_frame(
                    &mut stream,
                    &ServerFrame::Error {
                        code: "unsupported_protocol".to_string(),
                        message: format!("expected protocol version {PROTOCOL_VERSION}"),
                    },
                )
                .await?;
                return Ok(());
            }

            if token != expected_token {
                write_frame(
                    &mut stream,
                    &ServerFrame::Error {
                        code: "unauthorized".to_string(),
                        message: "invalid token".to_string(),
                    },
                )
                .await?;
                return Ok(());
            }

            write_frame(
                &mut stream,
                &ServerFrame::HelloAccepted {
                    protocol_version: PROTOCOL_VERSION,
                    capabilities,
                },
            )
            .await?;
        }
    }

    Ok(())
}
