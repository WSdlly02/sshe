use crate::{Error, MAX_FRAME, Result};
use serde::{Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
pub async fn read_frame<R: AsyncRead + Unpin, T: DeserializeOwned>(r: &mut R) -> Result<T> {
    let len = r.read_u32().await? as usize;
    if len > MAX_FRAME {
        return Err(Error::FrameTooLarge);
    }
    let mut bytes = vec![0; len];
    r.read_exact(&mut bytes).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub async fn write_frame<W: AsyncWrite + Unpin, T: Serialize>(w: &mut W, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_FRAME {
        return Err(Error::FrameTooLarge);
    }
    w.write_u32(bytes.len() as u32).await?;
    w.write_all(&bytes).await?;
    w.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Request;
    #[tokio::test]
    async fn refuses_oversized_frame_before_payload() {
        let mut bytes = ((MAX_FRAME + 1) as u32).to_be_bytes().as_slice().to_vec();
        assert!(
            read_frame::<_, Request>(&mut bytes.as_slice())
                .await
                .is_err()
        );
        bytes.clear();
    }
}
