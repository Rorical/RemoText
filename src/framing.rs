use anyhow::{Context, Result, bail};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::protocol::{MAX_FRAME_LEN, Message};

pub async fn write_message<W>(writer: &mut W, message: &Message) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let bytes = postcard::to_stdvec(message).context("serialize protocol message")?;
    if bytes.len() > MAX_FRAME_LEN {
        bail!("protocol frame is too large: {} bytes", bytes.len());
    }

    writer
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .await
        .context("write protocol frame length")?;
    writer
        .write_all(&bytes)
        .await
        .context("write protocol frame body")?;
    writer.flush().await.context("flush protocol frame")?;

    Ok(())
}

pub async fn read_message<R>(reader: &mut R) -> Result<Message>
where
    R: AsyncRead + Unpin,
{
    let mut len = [0u8; 4];
    reader
        .read_exact(&mut len)
        .await
        .context("read protocol frame length")?;
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME_LEN {
        bail!("protocol frame exceeds limit: {len} bytes");
    }

    let mut bytes = vec![0u8; len];
    reader
        .read_exact(&mut bytes)
        .await
        .context("read protocol frame body")?;
    postcard::from_bytes(&bytes).context("deserialize protocol message")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ClientHello, Message};

    #[tokio::test]
    async fn frame_roundtrip() {
        let message = Message::ClientHello(ClientHello {
            version: 1,
            credential_request: vec![7; 32],
        });
        let mut bytes = Vec::new();
        write_message(&mut bytes, &message).await.unwrap();

        let mut cursor = std::io::Cursor::new(bytes);
        let decoded = read_message(&mut cursor).await.unwrap();
        assert_eq!(decoded, message);
    }
}
