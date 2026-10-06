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
    let (mut stream, _) = listener.accept().await.map_err(|_| ())?;
    let mut bytes = vec![0; 4096];
    let read = stream.read(&mut bytes).await.map_err(|_| ())?;
    let request = String::from_utf8_lossy(&bytes[..read]);
    let query = query_of(&request);
    let _ = stream.write_all(PAGE.as_bytes()).await;
    Ok(query)
}

fn query_of(request: &str) -> CallbackQuery {
    let target = request.split_whitespace().nth(1).unwrap_or("");
    let query = target.split_once('?').map(|(_, query)| query).unwrap_or("");
    let mut state = String::new();
    let mut code = None;
    let mut denied = false;
    for pair in query.split('&') {
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
        match name {
            "state" => state = value.to_owned(),
            "code" => code = Some(value.to_owned()),
            "error" => denied = true,
            _ => {}
        }
    }
    CallbackQuery {
        state,
        code,
        denied,
    }
}
