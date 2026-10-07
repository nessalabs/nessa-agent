//! Bounded loopback HTTP framing and candidate delivery. The domain owns
//! consent acceptance. Dropping the candidate receiver cancels the listener
//! and its scoped connections; an empty stream otherwise waits at most the
//! supplied whole-attempt deadline, including after a separate revoke.
use std::time::Duration;

use async_trait::async_trait;
use futures_util::stream::{FuturesUnordered, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{self, Sender};
use tokio::time::{timeout, timeout_at, Instant};

use crate::mcp_authorization::application::{CallbackBind, CallbackQuery, ConsentCallback};

const PAGE: &str = "HTTP/1.1 200 OK\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: 47\r\nconnection: close\r\n\r\n<!doctype html><p>You can close this page.</p>\n";
const HEADER_LIMIT: usize = 4096;
const CONNECTION_LIMIT: usize = 4;
const CONNECTION_BUDGET: Duration = Duration::from_secs(2);

pub struct LoopbackCallback;

#[async_trait]
impl ConsentCallback for LoopbackCallback {
    async fn listen(&self, wait_for: Duration) -> Result<CallbackBind, ()> {
        let listener = TcpListener::bind("127.0.0.1:0").await.map_err(|_| ())?;
        let port = listener.local_addr().map_err(|_| ())?.port();
        let (sender, candidates) = mpsc::channel(1);
        tokio::spawn(async move {
            let _ = timeout(wait_for, serve(listener, sender, CONNECTION_BUDGET)).await;
        });
        Ok(CallbackBind {
            redirect_uri: format!("http://127.0.0.1:{port}/mcp-oauth/callback"),
            candidates,
        })
    }
}

async fn serve(listener: TcpListener, sender: Sender<CallbackQuery>, budget: Duration) {
    let mut connections = FuturesUnordered::new();
    loop {
        tokio::select! {
            biased;
            _ = sender.closed() => return,
            _ = connections.next(), if !connections.is_empty() => {}
            accepted = listener.accept(), if connections.len() < CONNECTION_LIMIT => {
                let Ok((stream, _)) = accepted else { return };
                connections.push(deliver(stream, sender.clone(), budget));
            }
        }
    }
}

async fn deliver(stream: TcpStream, sender: Sender<CallbackQuery>, budget: Duration) {
    let query = connection(stream, budget).await;
    if let Some(query) = query {
        // A blocked send still owns one of the bounded connection slots, but
        // the socket is already closed. Whole-attempt timeout/receiver closure
        // cancels this future through the scoped supervisor.
        let _ = sender.send(query).await;
    }
}

async fn connection<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    budget: Duration,
) -> Option<CallbackQuery> {
    let deadline = Instant::now() + budget;
    let header = timeout_at(deadline, read_header(&mut stream))
        .await
        .ok()?
        .ok()?;
    let query = callback_query(&header);
    // Writing a fixed page is best effort and uses the same absolute budget.
    // A write failure must not replace an already framed callback candidate.
    let _ = timeout_at(deadline, stream.write_all(PAGE.as_bytes())).await;
    query
}

#[derive(Debug, PartialEq, Eq)]
enum HeaderFailure {
    Incomplete,
    Oversize,
    InvalidUtf8,
    Io,
}

async fn read_header<R: AsyncRead + Unpin>(reader: &mut R) -> Result<String, HeaderFailure> {
    let mut bytes = vec![0; HEADER_LIMIT];
    let mut used = 0;
    loop {
        let read = reader
            .read(&mut bytes[used..])
            .await
            .map_err(|_| HeaderFailure::Io)?;
        if read == 0 {
            return Err(HeaderFailure::Incomplete);
        }
        let scan = used.saturating_sub(3);
        used += read;
        if let Some(end) = bytes[scan..used]
            .windows(4)
            .position(|part| part == b"\r\n\r\n")
        {
            bytes.truncate(scan + end + 4);
            return String::from_utf8(bytes).map_err(|_| HeaderFailure::InvalidUtf8);
        }
        if used == HEADER_LIMIT {
            return Err(HeaderFailure::Oversize);
        }
    }
}

/// Parse a complete origin-form GET with exactly one state and one code or
/// error. Unknown query fields are ignored; recognized duplicates are refused.
/// Percent decoding preserves literal `+` because the target is not a form.
fn callback_query(header: &str) -> Option<CallbackQuery> {
    let body = header.strip_suffix("\r\n\r\n")?;
    let mut lines = body.split("\r\n");
    let mut parts = lines.next()?.split(' ');
    let method = parts.next()?;
    let target = parts.next()?;
    let version = parts.next()?;
    if method != "GET" || !matches!(version, "HTTP/1.0" | "HTTP/1.1") || parts.next().is_some() {
        return None;
    }
    if !target.bytes().all(|byte| (0x21..=0x7e).contains(&byte)) || target.contains('#') {
        return None;
    }
    for line in lines {
        let (name, value) = line.split_once(':')?;
        if name.is_empty()
            || !name.bytes().all(header_name_byte)
            || !value
                .bytes()
                .all(|byte| byte == b'\t' || byte >= 0x20 && byte != 0x7f)
        {
            return None;
        }
    }
    let (path, query) = target.split_once('?')?;
    if path != "/mcp-oauth/callback" {
        return None;
    }
    let mut state = None;
    let mut code = None;
    let mut error = None;
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let name = percent_decode(name)?;
        let value = percent_decode(value)?;
        let slot = match name.as_str() {
            "state" => &mut state,
            "code" => &mut code,
            "error" => &mut error,
            _ => continue,
        };
        if slot.is_some() || value.is_empty() {
            return None;
        }
        *slot = Some(value);
    }
    let state = state?;
    if code.is_some() == error.is_some() {
        return None;
    }
    Some(CallbackQuery {
        state,
        code,
        denied: error.is_some(),
    })
}

fn header_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return None;
            }
            let high = hex_value(bytes[index + 1])?;
            let low = hex_value(bytes[index + 2])?;
            out.push((high << 4) | low);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::io;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use super::*;
    use tokio::io::ReadBuf;

    const REQUEST: &[u8] =
        b"GET /mcp-oauth/callback?state=a%2Fb&code=c%20d+e HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";

    struct Chunks(VecDeque<Vec<u8>>);
    impl AsyncRead for Chunks {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            if let Some(mut chunk) = self.0.pop_front() {
                let count = chunk.len().min(buf.remaining());
                buf.put_slice(&chunk[..count]);
                if count != chunk.len() {
                    chunk.drain(..count);
                    self.0.push_front(chunk);
                }
            }
            Poll::Ready(Ok(()))
        }
    }

    fn request(query: &str) -> String {
        format!("GET /mcp-oauth/callback?{query} HTTP/1.1\r\nHost: localhost\r\n\r\n")
    }

    #[tokio::test]
    async fn a3a_fragmentation_at_every_byte_keeps_complete_values() {
        for split in 1..REQUEST.len() {
            let mut reader = Chunks(VecDeque::from([
                REQUEST[..split].to_vec(),
                REQUEST[split..].to_vec(),
            ]));
            let header = read_header(&mut reader).await.unwrap();
            let query = callback_query(&header).unwrap();
            assert_eq!(query.state, "a/b", "split {split}");
            assert_eq!(query.code.as_deref(), Some("c d+e"));
            assert!(!query.denied);
        }
    }

    #[tokio::test]
    async fn a3b_eof_limit_and_utf8_are_typed() {
        for bytes in [
            Vec::new(),
            b"GET /mcp-oauth/callback?state=s&code=c".to_vec(),
            b"GET /mcp-oauth/callback?state=s&code=c HTTP/1.1\r\nHost: x\r\n".to_vec(),
        ] {
            assert_eq!(
                read_header(&mut Chunks(VecDeque::from([bytes]))).await,
                Err(HeaderFailure::Incomplete)
            );
        }
        let mut invalid = REQUEST.to_vec();
        invalid[4] = 0xff;
        assert_eq!(
            read_header(&mut Chunks(VecDeque::from([invalid]))).await,
            Err(HeaderFailure::InvalidUtf8)
        );
        for len in [HEADER_LIMIT - 1, HEADER_LIMIT] {
            let mut bytes = REQUEST[..REQUEST.len() - 2].to_vec();
            bytes.extend_from_slice(b"x-pad: ");
            bytes.resize(len - 4, b'x');
            bytes.extend_from_slice(b"\r\n\r\n");
            assert!(callback_query(
                &read_header(&mut Chunks(VecDeque::from([bytes])))
                    .await
                    .unwrap()
            )
            .is_some());
        }
        let mut bytes = vec![b'x'; HEADER_LIMIT + 1];
        bytes.extend_from_slice(b"\r\n\r\n");
        assert_eq!(
            read_header(&mut Chunks(VecDeque::from([bytes]))).await,
            Err(HeaderFailure::Oversize)
        );
        let mut trailing = REQUEST.to_vec();
        trailing.extend_from_slice(b"another request\xff");
        assert!(callback_query(
            &read_header(&mut Chunks(VecDeque::from([trailing])))
                .await
                .unwrap()
        )
        .is_some());
    }

    #[test]
    fn a3b_query_shape_refuses_duplicates_and_keeps_valid_neighbors() {
        for query in [
            "state=s&state=s&code=c",
            "state=s&%73tate=s&code=c",
            "state=s&code=c&code=c",
            "state=s&error=no&error=no",
            "state=s&code=c&error=no",
            "code=c",
            "state=s",
            "state=&code=c",
            "state=s&code=",
            "state=s&error=",
            "state=%GG&code=c",
            "state=s&code=%",
            "state=s&code=%ff",
            "state=s&code=c#fragment",
        ] {
            assert_eq!(callback_query(&request(query)), None, "{query}");
        }
        for query in [
            "state=s&code=c",
            "state=s&code=c&unknown=x",
            "state=s&error=access_denied&error_description=No",
            "state=%E2%98%83&code=c+d",
        ] {
            assert!(callback_query(&request(query)).is_some(), "{query}");
        }
        for malformed in [
            "GET /favicon.ico HTTP/1.1\r\n\r\n",
            "POST /mcp-oauth/callback?state=s&code=c HTTP/1.1\r\n\r\n",
            "GET /mcp-oauth/callback?state=s&code=c\r\n\r\n",
            "GET /mcp-oauth/callback?state=s&code=c HTTP/2\r\n\r\n",
            "GET  /mcp-oauth/callback?state=s&code=c HTTP/1.1\r\n\r\n",
            "GET /mcp-oauth/callback?state=s&code=c HTTP/1.1\n\n",
            "GET /mcp-oauth/callback?state=s\n&code=c HTTP/1.1\r\n\r\n",
            "GET /mcp-oauth/callback?state=s&code=c HTTP/1.1\r\nbad header\r\n\r\n",
        ] {
            assert!(callback_query(malformed).is_none(), "{malformed:?}");
        }
        assert!(
            callback_query(&request("state=s&code=c").replace("HTTP/1.1", "HTTP/1.0")).is_some()
        );
    }

    async fn address(bind: &CallbackBind) -> String {
        let url = url::Url::parse(&bind.redirect_uri).unwrap();
        format!("127.0.0.1:{}", url.port().unwrap())
    }

    async fn send(host: &str, bytes: &[u8]) -> TcpStream {
        let mut stream = TcpStream::connect(host).await.unwrap();
        stream.write_all(bytes).await.unwrap();
        stream
    }

    #[tokio::test]
    async fn a3a_idle_first_and_incomplete_header_do_not_block_second() {
        let mut bind = LoopbackCallback
            .listen(Duration::from_secs(3))
            .await
            .unwrap();
        let host = address(&bind).await;
        let mut idle = TcpStream::connect(&host).await.unwrap();
        let mut partial = send(&host, &REQUEST[..REQUEST.len() - 2]).await;
        assert!(timeout(Duration::from_millis(30), bind.candidates.recv())
            .await
            .is_err());
        let mut valid = send(&host, REQUEST).await;
        let query = timeout(Duration::from_millis(300), bind.candidates.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(query.state, "a/b");
        let mut page = Vec::new();
        valid.read_to_end(&mut page).await.unwrap();
        assert_eq!(page, PAGE.as_bytes());
        drop(bind);
        let mut byte = [0];
        assert_eq!(
            timeout(Duration::from_millis(300), idle.read(&mut byte))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        assert_eq!(
            timeout(Duration::from_millis(300), partial.read(&mut byte))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        assert!(TcpStream::connect(&host).await.is_err());
    }

    #[tokio::test]
    async fn a3b_unrelated_malformed_and_eof_leave_listener_available() {
        let mut bind = LoopbackCallback
            .listen(Duration::from_secs(2))
            .await
            .unwrap();
        let host = address(&bind).await;
        for bytes in [
            b"GET /favicon.ico HTTP/1.1\r\n\r\n".as_slice(),
            request("state=s&code=c&code=d").as_bytes(),
            b"incomplete",
        ] {
            let mut stream = send(&host, bytes).await;
            stream.shutdown().await.unwrap();
            let mut reply = Vec::new();
            stream.read_to_end(&mut reply).await.unwrap();
        }
        let _valid = send(&host, REQUEST).await;
        assert_eq!(
            bind.candidates.recv().await.unwrap().code.as_deref(),
            Some("c d+e")
        );
    }

    #[tokio::test]
    async fn a3f_saturation_releases_slots_and_original_deadline_closes_stream() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let host = listener.local_addr().unwrap();
        let (sender, mut receiver) = mpsc::channel(1);
        let task =
            tokio::spawn(async move { serve(listener, sender, Duration::from_millis(200)).await });
        let mut idle = Vec::new();
        for _ in 0..CONNECTION_LIMIT {
            idle.push(TcpStream::connect(host).await.unwrap());
        }
        let _valid = send(&host.to_string(), REQUEST).await;
        assert!(timeout(Duration::from_millis(30), receiver.recv())
            .await
            .is_err());
        assert_eq!(
            timeout(Duration::from_millis(400), receiver.recv())
                .await
                .unwrap()
                .unwrap()
                .state,
            "a/b"
        );
        drop(receiver);
        timeout(Duration::from_millis(300), task)
            .await
            .unwrap()
            .unwrap();

        let mut bind = LoopbackCallback
            .listen(Duration::from_millis(100))
            .await
            .unwrap();
        let host = address(&bind).await;
        let _idle = TcpStream::connect(&host).await.unwrap();
        assert!(timeout(Duration::from_millis(400), bind.candidates.recv())
            .await
            .unwrap()
            .is_none());
        assert!(TcpStream::connect(&host).await.is_err());
    }

    #[tokio::test]
    async fn a3f_full_candidate_channel_cancels_scoped_handlers() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let host = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel(1);
        let task = tokio::spawn(serve(listener, sender, Duration::from_secs(1)));
        for _ in 0..CONNECTION_LIMIT + 1 {
            let mut stream = send(&host.to_string(), REQUEST).await;
            let mut page = Vec::new();
            stream.read_to_end(&mut page).await.unwrap();
            assert_eq!(page, PAGE.as_bytes());
        }
        drop(receiver);
        timeout(Duration::from_millis(300), task)
            .await
            .unwrap()
            .unwrap();
        assert!(TcpStream::connect(host).await.is_err());
    }

    struct BrokenWriter(Chunks);
    impl AsyncRead for BrokenWriter {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Pin::new(&mut self.0).poll_read(cx, buf)
        }
    }
    impl AsyncWrite for BrokenWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()))
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn a3g_response_failure_keeps_framed_candidate() {
        let stream = BrokenWriter(Chunks(VecDeque::from([REQUEST.to_vec()])));
        assert_eq!(
            connection(stream, Duration::from_secs(1))
                .await
                .unwrap()
                .state,
            "a/b"
        );
        let (client, mut peer) = tokio::io::duplex(1);
        let writer = tokio::spawn(async move {
            peer.write_all(REQUEST).await.unwrap();
            peer
        });
        let query = connection(client, Duration::from_millis(60)).await.unwrap();
        assert_eq!(query.state, "a/b");
        drop(writer.await.unwrap());
    }

    #[tokio::test]
    async fn a3f_trickle_does_not_reset_connection_budget() {
        let (client, mut peer) = tokio::io::duplex(128);
        let writer = tokio::spawn(async move {
            for chunk in REQUEST.chunks(8) {
                if peer.write_all(chunk).await.is_err() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
        let result = timeout(
            Duration::from_millis(160),
            connection(client, Duration::from_millis(60)),
        )
        .await
        .unwrap();
        assert!(result.is_none());
        writer.await.unwrap();
    }

    struct BrokenReader;
    impl AsyncRead for BrokenReader {
        fn poll_read(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(Err(io::ErrorKind::ConnectionReset.into()))
        }
    }

    #[tokio::test]
    async fn a3b_read_failure_is_typed() {
        assert_eq!(read_header(&mut BrokenReader).await, Err(HeaderFailure::Io));
    }
}
