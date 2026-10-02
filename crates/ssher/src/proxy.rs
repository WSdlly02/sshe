use anyhow::{Context, Result, anyhow};
use tokio::{
    io::{self, AsyncWriteExt},
    net::TcpStream,
    time::{self, Duration},
};

pub(crate) async fn proxy_tcp_stdio(endpoint: &str, port: u16, timeout_ms: u64) -> Result<()> {
    let address = format!("{endpoint}:{port}");
    let stream = time::timeout(
        Duration::from_millis(timeout_ms),
        TcpStream::connect(&address),
    )
    .await
    .map_err(|_| anyhow!("connect timeout: {address}"))?
    .with_context(|| format!("connect failed for {address}"))?;

    let (mut reader, mut writer) = stream.into_split();
    let mut stdin = io::stdin();
    let mut stdout = io::stdout();

    let stdin_to_socket = async {
        io::copy(&mut stdin, &mut writer)
            .await
            .context("stdin->socket copy failed")?;
        writer.shutdown().await.context("socket shutdown failed")?;
        Ok::<(), anyhow::Error>(())
    };

    let socket_to_stdout = async {
        io::copy(&mut reader, &mut stdout)
            .await
            .context("socket->stdout copy failed")?;
        stdout.flush().await.context("stdout flush failed")?;
        Ok::<(), anyhow::Error>(())
    };

    let (left, right) = tokio::join!(stdin_to_socket, socket_to_stdout);
    left?;
    right?;
    Ok(())
}
