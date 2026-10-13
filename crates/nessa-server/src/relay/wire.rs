//! Reading preamble frames off an async stream, for the relay and for a
//! gateway that serves through one. A frame is read exactly, so the first
//! TLS bytes after `Joined` stay on the stream for whoever reads next.
use nessa_protocol::relay::{
    parse_body, parse_header, RelayFrame, RelayFrameError, HEADER, MAX_FRAME,
};
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf};

/// Read exactly one frame and not a byte more.
pub(crate) async fn read_frame(stream: &mut (impl AsyncRead + Unpin)) -> io::Result<RelayFrame> {
    let mut header = [0; HEADER];
    stream.read_exact(&mut header).await?;
    let header = parse_header(header).map_err(invalid)?;
    let mut body = [0; MAX_FRAME - HEADER];
    let body = &mut body[..header.body()];
    stream.read_exact(body).await?;
    parse_body(header, body).map_err(invalid)
}

/// [`read_frame`] as a future that owns its half, so a serve loop can keep
/// one read going across its other waits without losing a byte of it.
pub(crate) async fn next_frame<S: AsyncRead>(
    mut reader: ReadHalf<S>,
) -> (ReadHalf<S>, io::Result<RelayFrame>) {
    let frame = read_frame(&mut reader).await;
    (reader, frame)
}

/// Write one frame and flush it.
pub(crate) async fn write_frame(
    stream: &mut (impl AsyncWrite + Unpin),
    frame: RelayFrame,
) -> io::Result<()> {
    stream.write_all(&frame.encode()).await?;
    stream.flush().await
}

fn invalid(error: RelayFrameError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}
