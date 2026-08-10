use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub async fn read_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
    max_bytes: usize,
) -> io::Result<Option<Vec<u8>>> {
    let mut frame = Vec::with_capacity(max_bytes.min(8 * 1024));
    let mut buffer = [0u8; 8 * 1024];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return if frame.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "JSON frame is missing a newline terminator",
                ))
            };
        }
        if let Some(newline) = buffer[..read].iter().position(|byte| *byte == b'\n') {
            let needed = newline;
            if frame.len().saturating_add(needed) > max_bytes {
                return Err(frame_too_large());
            }
            frame.extend_from_slice(&buffer[..newline]);
            return Ok(Some(frame));
        }
        if frame.len().saturating_add(read) > max_bytes {
            return Err(frame_too_large());
        }
        frame.extend_from_slice(&buffer[..read]);
    }
}

pub async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, frame: &[u8]) -> io::Result<()> {
    writer.write_all(frame).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await
}

pub fn frame_too_large() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "JSON frame exceeds byte limit")
}

#[cfg(test)]
mod tests {
    use tokio::io::AsyncWriteExt;

    use super::*;

    #[test]
    fn oversized_and_partial_frames_are_rejected_without_unbounded_reads() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let (mut writer, mut reader) = tokio::io::duplex(32);
            writer.write_all(b"12345\n").await.unwrap();
            assert_eq!(
                read_frame(&mut reader, 4).await.unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );

            let (mut writer, mut reader) = tokio::io::duplex(32);
            writer.write_all(b"{}").await.unwrap();
            writer.shutdown().await.unwrap();
            assert_eq!(
                read_frame(&mut reader, 4).await.unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
        });
    }
}
