//! Where a ChatGPT sign-in's browser comes back to: an HTTP listener on
//! `127.0.0.1`, port 1455 when it is free (OpenAI's example), else any free
//! port, for one attempt. It reads only `GET /auth/callback?…`, answers
//! every other request 404, and answers the callback once the sign-in says
//! how it went.
use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};

use crate::logins::chatgpt::ports::{
    Callback, CallbackListener, CallbackReply, CallbackServer, ListenFailed,
};

/// The port tried first.
pub const PREFERRED_PORT: u16 = 1455;
/// The path OpenAI sends the browser back to; only the port may vary.
pub const CALLBACK_PATH: &str = "/auth/callback";
/// The most of a request read: a request line with a code, state and a
/// few parameters, and the browser's headers.
const MOST_REQUEST_BYTES: usize = 16 * 1024;
/// How long a connection may take to send its request.
const REQUEST_WAIT: Duration = Duration::from_secs(10);
/// How long the browser waits for the sign-in to finish before it is told
/// to look in Nessa.
const REPLY_WAIT: Duration = Duration::from_secs(60);

pub struct LoopbackListener {
    preferred: u16,
}

impl LoopbackListener {
    pub fn new() -> Self {
        Self {
            preferred: PREFERRED_PORT,
        }
    }

    /// Tries `preferred` first (a test's free port).
    pub fn preferring(preferred: u16) -> Self {
        Self { preferred }
    }
}

impl Default for LoopbackListener {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CallbackListener for LoopbackListener {
    async fn listen(&self) -> Result<Box<dyn CallbackServer>, ListenFailed> {
        let listener = match TcpListener::bind(("127.0.0.1", self.preferred)).await {
            Ok(listener) => listener,
            Err(_) => TcpListener::bind(("127.0.0.1", 0))
                .await
                .map_err(|_| ListenFailed)?,
        };
        let port = listener.local_addr().map_err(|_| ListenFailed)?.port();
        let (sender, callbacks) = mpsc::channel(4);
        let accepting = tokio::spawn(accept(listener, sender));
        Ok(Box::new(LoopbackServer {
            port,
            callbacks,
            accepting,
        }))
    }
}

struct LoopbackServer {
    port: u16,
    callbacks: mpsc::Receiver<Callback>,
    accepting: tokio::task::JoinHandle<()>,
}

impl Drop for LoopbackServer {
    fn drop(&mut self) {
        // The port is let go with the attempt.
        self.accepting.abort();
    }
}

#[async_trait]
impl CallbackServer for LoopbackServer {
    fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}{CALLBACK_PATH}", self.port)
    }

    async fn next(&mut self) -> Option<Callback> {
        self.callbacks.recv().await
    }
}

async fn accept(listener: TcpListener, callbacks: mpsc::Sender<Callback>) {
    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(serve(stream, callbacks.clone()));
    }
}

async fn serve(mut stream: TcpStream, callbacks: mpsc::Sender<Callback>) {
    let Ok(Some(target)) = tokio::time::timeout(REQUEST_WAIT, request_target(&mut stream))
        .await
        .map(|read| read.ok().flatten())
    else {
        return;
    };
    let Some(params) = callback_params(&target) else {
        let _ = respond(&mut stream, 404, "Not found.").await;
        return;
    };
    let (reply, answer) = oneshot::channel();
    if callbacks.send(Callback { params, reply }).await.is_err() {
        let _ = respond(&mut stream, 410, PAGE_GONE).await;
        return;
    }
    let (status, page) = match tokio::time::timeout(REPLY_WAIT, answer).await {
        Ok(Ok(CallbackReply::SignedIn)) => (200, PAGE_SIGNED_IN),
        Ok(Ok(CallbackReply::Failed)) => (200, PAGE_FAILED),
        Ok(Ok(CallbackReply::NotThis)) | Ok(Err(_)) => (400, PAGE_GONE),
        Err(_) => (200, PAGE_FAILED),
    };
    let _ = respond(&mut stream, status, page).await;
}

const PAGE_SIGNED_IN: &str = "Signed in to ChatGPT. You can close this tab and go back to Nessa.";
const PAGE_FAILED: &str = "The ChatGPT sign-in didn’t finish. Go back to Nessa to see why.";
const PAGE_GONE: &str = "This sign-in is no longer waiting. Start it again from Nessa.";

/// The request target of a `GET`, once the request's head is read.
async fn request_target(stream: &mut TcpStream) -> std::io::Result<Option<String>> {
    let mut head = Vec::new();
    let mut buffer = [0_u8; 2048];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut buffer).await?;
        if read == 0 || head.len() + read > MOST_REQUEST_BYTES {
            return Ok(None);
        }
        head.extend_from_slice(&buffer[..read]);
    }
    Ok(request_line_target(&head))
}

/// The target of `GET <target> HTTP/1.x`, the head's first line.
pub fn request_line_target(head: &[u8]) -> Option<String> {
    let line = head.split(|byte| *byte == b'\n').next()?;
    let line = std::str::from_utf8(line).ok()?.trim_end_matches('\r');
    let mut parts = line.split(' ');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("GET"), Some(target), Some(version), None) if version.starts_with("HTTP/1.") => {
            Some(target.to_owned())
        }
        _ => None,
    }
}

/// The query of a request for [`CALLBACK_PATH`]; `None` for any other path.
/// A name given twice keeps its first value.
pub fn callback_params(target: &str) -> Option<BTreeMap<String, String>> {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != CALLBACK_PATH {
        return None;
    }
    let mut params = BTreeMap::new();
    for (name, value) in url::form_urlencoded::parse(query.as_bytes()) {
        params
            .entry(name.into_owned())
            .or_insert_with(|| value.into_owned());
    }
    Some(params)
}

async fn respond(stream: &mut TcpStream, status: u16, text: &str) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Gone",
    };
    let body = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>Nessa</title>\
         <body style=\"font:16px system-ui;margin:40px\"><p>{text}</p></body>"
    );
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}
