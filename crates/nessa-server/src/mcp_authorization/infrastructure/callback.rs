//! `http://127.0.0.1:<port>/mcp-oauth/callback`. The response body is a fixed
//! page: it does not contain the code, the state, or the request target.
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::mcp_authorization::application::{CallbackBind, CallbackQuery, ConsentCallback};

const PAGE: &str = "HTTP/1.1 200 OK\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: 47\r\nconnection: close\r\n\r\n<!doctype html><p>You can close this page.</p>\n";

pub struct LoopbackCallback;

#[async_trait]
impl ConsentCallback for LoopbackCallback {
    async fn listen(&self, wait_for: Duration) -> Result<CallbackBind, ()> {
        let listener = TcpListener::bind("127.0.0.1:0").await.map_err(|_| ())?;
        let port = listener.local_addr().map_err(|_| ())?.port();
        let (sender, accepted) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let accepted = tokio::time::timeout(wait_for, one_callback(listener)).await;
            let query = accepted.ok().and_then(Result::ok);
            if let Some(query) = query {
                let _ = sender.send(query);
            }
        });
        Ok(CallbackBind {
            redirect_uri: format!("http://127.0.0.1:{port}/mcp-oauth/callback"),
            accepted,
        })
    }
}

async fn one_callback(listener: TcpListener) -> Result<CallbackQuery, ()> {
    loop {
        let (mut stream, _) = listener.accept().await.map_err(|_| ())?;
        let mut bytes = vec![0; 4096];
        let read = match stream.read(&mut bytes).await {
            Ok(0) | Err(_) => continue,
            Ok(read) => read,
        };
        let _ = stream.write_all(PAGE.as_bytes()).await;
        let request = String::from_utf8_lossy(&bytes[..read]);
        if let Some(query) = callback_query(&request) {
            return Ok(query);
        }
    }
}

/// A GET of `/mcp-oauth/callback` that carries state, a code, or an error.
/// Query values are percent-decoded. `+` stays `+`: this is a URI query,
/// not a form body.
fn callback_query(request: &str) -> Option<CallbackQuery> {
    let request_line = request.lines().next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?;
    let target = parts.next()?;
    if method != "GET" {
        return None;
    }
    let (path, query) = target.split_once('?')?;
    if path != "/mcp-oauth/callback" {
        return None;
    }
    let query = query
        .split_once('#')
        .map(|(query, _)| query)
        .unwrap_or(query);
    let mut state = None;
    let mut code = None;
    let mut denied = false;
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        let name = percent_decode(name)?;
        let value = percent_decode(value)?;
        match name.as_str() {
            "state" => state = Some(value),
            "code" => code = Some(value),
            "error" => denied = true,
            _ => {}
        }
    }
    if state.is_none() && code.is_none() && !denied {
        return None;
    }
    Some(CallbackQuery {
        state: state.unwrap_or_default(),
        code,
        denied,
    })
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
    use std::time::Duration;

    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    #[tokio::test]
    async fn a_wrong_first_connection_does_not_consume_the_redirect() {
        let bind = LoopbackCallback
            .listen(Duration::from_secs(2))
            .await
            .unwrap();
        let host = bind
            .redirect_uri
            .strip_prefix("http://")
            .unwrap()
            .split('/')
            .next()
            .unwrap();
        let mut wrong = TcpStream::connect(host).await.unwrap();
        wrong
            .write_all(b"GET /favicon.ico HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .await
            .unwrap();
        let mut page = [0_u8; 256];
        let read = wrong.read(&mut page).await.unwrap();
        let page = String::from_utf8_lossy(&page[..read]);
        assert!(!page.contains("c%20d"));
        assert!(!page.contains("a/b"));
        let mut empty = TcpStream::connect(host).await.unwrap();
        empty.shutdown().await.unwrap();
        let mut callback = TcpStream::connect(host).await.unwrap();
        callback
            .write_all(
                b"GET /mcp-oauth/callback?state=a%2Fb&code=c%20d+e HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
            )
            .await
            .unwrap();
        let query = tokio::time::timeout(Duration::from_secs(2), bind.accepted)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(query.state, "a/b");
        assert_eq!(query.code.as_deref(), Some("c d+e"));
        assert!(!query.denied);
    }
}
