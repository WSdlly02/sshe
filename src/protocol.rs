use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_FRAME_LEN: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientFrame {
    Hello {
        protocol_version: u32,
        client_name: String,
        token: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerFrame {
    HelloAccepted {
        protocol_version: u32,
        capabilities: Vec<String>,
    },
    Error {
        code: String,
        message: String,
    },
}

pub async fn read_frame<R, T>(reader: &mut R) -> Result<T>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let len = reader
        .read_u32()
        .await
        .context("failed to read frame length")? as usize;
    if len > MAX_FRAME_LEN {
        bail!("frame length {len} exceeds max {MAX_FRAME_LEN}");
    }

    let mut buf = vec![0; len];
    reader
        .read_exact(&mut buf)
        .await
        .context("failed to read frame payload")?;

    serde_json::from_slice(&buf).context("failed to decode JSON frame")
}

pub async fn write_frame<W, T>(writer: &mut W, frame: &T) -> Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let payload = serde_json::to_vec(frame).context("failed to encode JSON frame")?;
    if payload.len() > MAX_FRAME_LEN {
        return Err(anyhow!(
            "frame length {} exceeds max {}",
            payload.len(),
            MAX_FRAME_LEN
        ));
    }

    writer
        .write_u32(payload.len() as u32)
        .await
        .context("failed to write frame length")?;
    writer
        .write_all(&payload)
        .await
        .context("failed to write frame payload")?;
    writer.flush().await.context("failed to flush frame")
}

#[cfg(test)]
mod tests {
    use super::{ClientFrame, PROTOCOL_VERSION, ServerFrame, read_frame, write_frame};
    use tokio::io::duplex;

    #[tokio::test]
    async fn round_trips_client_hello_frame() {
        let (mut client, mut server) = duplex(4096);
        let frame = ClientFrame::Hello {
            protocol_version: PROTOCOL_VERSION,
            client_name: "sshea".to_string(),
            token: "secret".to_string(),
        };

        write_frame(&mut client, &frame)
            .await
            .expect("write should succeed");
        let decoded: ClientFrame = read_frame(&mut server).await.expect("read should succeed");

        assert_eq!(decoded, frame);
    }

    #[tokio::test]
    async fn round_trips_server_capabilities_frame() {
        let (mut client, mut server) = duplex(4096);
        let frame = ServerFrame::HelloAccepted {
            protocol_version: PROTOCOL_VERSION,
            capabilities: vec!["exec.v1".to_string()],
        };

        write_frame(&mut client, &frame)
            .await
            .expect("write should succeed");
        let decoded: ServerFrame = read_frame(&mut server).await.expect("read should succeed");

        assert_eq!(decoded, frame);
    }
}
