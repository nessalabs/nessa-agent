use super::*;
use std::{
    io::Write,
    os::{fd::AsRawFd, unix::net::UnixStream},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn byte_then_eof(mut input: BackgroundIo, mut writer: impl Write) {
    writer.write_all(b"x").unwrap();
    let mut byte = [0];
    input.read_exact(&mut byte).await.unwrap();
    assert_eq!(&byte, b"x");
    drop(writer);
    assert_eq!(input.read(&mut byte).await.unwrap(), 0);
}

#[tokio::test]
async fn background_input_reads_a_pipe_and_its_eof() {
    let (reader, writer) = std::io::pipe().unwrap();
    byte_then_eof(BackgroundIo::from_fd(reader.into()).unwrap(), writer).await;
}

#[tokio::test]
async fn background_input_reads_a_socket_and_its_eof() {
    let (reader, writer) = UnixStream::pair().unwrap();
    byte_then_eof(BackgroundIo::from_fd(reader.into()).unwrap(), writer).await;
}

#[tokio::test]
async fn background_output_waits_for_socket_backpressure() {
    let (writer, reader) = UnixStream::pair().unwrap();
    let capacity: libc::c_int = 4096;
    // SAFETY: the socket and the integer option remain valid for this call.
    assert_eq!(
        unsafe {
            libc::setsockopt(
                writer.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                (&capacity as *const libc::c_int).cast(),
                std::mem::size_of_val(&capacity) as libc::socklen_t,
            )
        },
        0
    );
    let mut output = BackgroundIo::from_fd(writer.into()).unwrap();
    reader.set_nonblocking(true).unwrap();
    let mut input = tokio::net::UnixStream::from_std(reader).unwrap();
    let bytes = vec![b'x'; 128 * 1024];
    let writing = output.write_all(&bytes);
    tokio::pin!(writing);
    assert!(futures_util::poll!(writing.as_mut()).is_pending());
    let mut received = vec![0; bytes.len()];
    tokio::time::timeout(Duration::from_secs(5), async {
        let (sent, read) = tokio::join!(writing, input.read_exact(&mut received));
        sent.unwrap();
        read.unwrap();
    })
    .await
    .unwrap();
    assert_eq!(received, bytes);
}
