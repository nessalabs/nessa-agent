//! Where the browser comes back to after Sign in with Boat: a listener on
//! this machine's loopback, on a port the system picks, for one sign-in.
//!
//! Boat admits `http://localhost` redirect URIs on any port for an app
//! without a secret, matching the rest exactly, so the app is registered
//! with `http://localhost/boat/callback` and each sign-in names its port.
//! The listener answers only `GET /boat/callback`; anything else is a 404.
//! It closes when the sign-in drops it.
use crate::workspaces::application::{
    CallbackPage, CallbackQuery, PendingCallback, SignInCallbacks,
};
use async_trait::async_trait;
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot},
};

/// The path Boat sends the browser back to.
pub const CALLBACK_PATH: &str = "/boat/callback";
/// A request head larger than this is not a browser coming back.
const HEAD_LIMIT: usize = 16 * 1024;
/// How long one connection may take to send its head.
const HEAD_DEADLINE: Duration = Duration::from_secs(10);
/// How long the browser waits for its page while the code is exchanged
/// and the connection saved.
const PAGE_DEADLINE: Duration = Duration::from_secs(90);

pub struct LoopbackCallbacks;

#[async_trait]
impl SignInCallbacks for LoopbackCallbacks {
    async fn open(&self) -> std::io::Result<Box<dyn PendingCallback>> {
        let v4 = TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = v4.local_addr()?.port();
        // `localhost` may resolve to ::1 first; listen there too when the
        // machine has it, on the same port.
        let v6 = TcpListener::bind(("::1", port)).await.ok();
        let (sender, arrivals) = mpsc::channel(4);
        let mut tasks = vec![tokio::spawn(serve(v4, sender.clone()))];
        if let Some(v6) = v6 {
            tasks.push(tokio::spawn(serve(v6, sender)));
        }
        Ok(Box::new(Loopback {
            port,
            arrivals,
            tasks,
        }))
    }
}

struct Loopback {
    port: u16,
    arrivals: mpsc::Receiver<(CallbackQuery, oneshot::Sender<CallbackPage>)>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for Loopback {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

#[async_trait]
impl PendingCallback for Loopback {
    fn redirect_uri(&self) -> String {
        format!("http://localhost:{}{CALLBACK_PATH}", self.port)
    }
    async fn arrived(&mut self) -> Option<(CallbackQuery, oneshot::Sender<CallbackPage>)> {
        self.arrivals.recv().await
    }
}

async fn serve(
    listener: TcpListener,
    arrivals: mpsc::Sender<(CallbackQuery, oneshot::Sender<CallbackPage>)>,
) {
    while let Ok((stream, _)) = listener.accept().await {
        let arrivals = arrivals.clone();
        tokio::spawn(async move {
            let _ = answer(stream, arrivals).await;
        });
    }
}

async fn answer(
    mut stream: TcpStream,
    arrivals: mpsc::Sender<(CallbackQuery, oneshot::Sender<CallbackPage>)>,
) -> std::io::Result<()> {
    let target = match tokio::time::timeout(HEAD_DEADLINE, read_target(&mut stream)).await {
        Ok(Ok(Some(target))) => target,
        _ => return write(&mut stream, "400 Bad Request", "").await,
    };
    let Some(query) = callback_query(&target) else {
        return write(&mut stream, "404 Not Found", "").await;
    };
    let (reply, page) = oneshot::channel();
    if arrivals.send((query, reply)).await.is_err() {
        return write(&mut stream, "410 Gone", &html(CallbackPage::Failed)).await;
    }
    let page = tokio::time::timeout(PAGE_DEADLINE, page)
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or(CallbackPage::Failed);
    write(&mut stream, "200 OK", &html(page)).await
}

/// The request target of a `GET`, once its whole head has arrived.
async fn read_target(stream: &mut TcpStream) -> std::io::Result<Option<String>> {
    let mut head = Vec::new();
    let mut buffer = [0u8; 2048];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut buffer).await?;
        if read == 0 || head.len() + read > HEAD_LIMIT {
            return Ok(None);
        }
        head.extend_from_slice(&buffer[..read]);
    }
    Ok(target(&head))
}

/// `GET <target> HTTP/1.x` from a request head.
pub fn target(head: &[u8]) -> Option<String> {
    let line = head.split(|byte| *byte == b'\r').next()?;
    let line = std::str::from_utf8(line).ok()?;
    let mut parts = line.split(' ');
    let (Some("GET"), Some(target), Some(version)) = (parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    version.starts_with("HTTP/1.").then(|| target.to_owned())
}

/// What a visit to the callback path carries; `None` for any other path.
pub fn callback_query(target: &str) -> Option<CallbackQuery> {
    let url = url::Url::parse(&format!("http://localhost{target}")).ok()?;
    if url.path() != CALLBACK_PATH {
        return None;
    }
    let mut query = CallbackQuery::default();
    for (key, value) in url.query_pairs() {
        let slot = match key.as_ref() {
            "code" => &mut query.code,
            "state" => &mut query.state,
            "iss" => &mut query.iss,
            "error" => &mut query.error,
            _ => continue,
        };
        // A parameter named twice is not Boat's redirect.
        if slot.replace(value.into_owned()).is_some() {
            return Some(CallbackQuery {
                error: Some("invalid_request".into()),
                ..CallbackQuery::default()
            });
        }
    }
    Some(query)
}

async fn write(stream: &mut TcpStream, status: &str, body: &str) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\ncache-control: no-store\r\nreferrer-policy: no-referrer\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

fn html(page: CallbackPage) -> String {
    let (title, line) = match page {
        CallbackPage::Connected => ("Nessa is connected to Boat", "You can close this tab."),
        CallbackPage::Failed => (
            "Nessa could not connect to Boat",
            "Nessa says why, and you can try again from there.",
        ),
    };
    format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content=\"width=device-width\"><title>{title}</title>\
<style>body{{font:15px/1.5 -apple-system,system-ui,sans-serif;display:grid;place-items:center;min-height:90vh;margin:0;color:#1d1d1f;background:#fbfbfa}}\
@media (prefers-color-scheme:dark){{body{{color:#ececec;background:#141414}}}}main{{max-width:28rem;padding:0 16px;text-align:center}}h1{{font-size:17px;font-weight:600;margin:0 0 4px}}p{{margin:0;opacity:.7}}\
a{{display:inline-block;margin-top:16px;padding:6px 14px;border-radius:8px;font-weight:500;text-decoration:none;color:#fbfbfa;background:#1d1d1f;transition:transform 160ms ease-out}}a:active{{transform:scale(.97)}}\
@media (prefers-color-scheme:dark){{a{{color:#141414;background:#ececec}}}}</style>\
<main><h1>{title}</h1><p>{line}</p><a href=\"nessa://open\">Open Nessa</a></main>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_pages_offer_to_open_nessa() {
        for page in [CallbackPage::Connected, CallbackPage::Failed] {
            assert!(html(page).contains(r#"<a href="nessa://open">Open Nessa</a>"#));
        }
    }

    #[test]
    fn only_a_get_head_names_a_target() {
        assert_eq!(
            target(b"GET /boat/callback?code=a HTTP/1.1\r\nHost: x\r\n\r\n").as_deref(),
            Some("/boat/callback?code=a")
        );
        assert_eq!(target(b"POST /boat/callback HTTP/1.1\r\n\r\n"), None);
        assert_eq!(target(b"GET /x SPDY\r\n\r\n"), None);
    }

    #[test]
    fn the_callback_reads_boats_redirect() {
        let query =
            callback_query("/boat/callback?code=c1&state=s1&iss=https%3A%2F%2Fboat.dev").unwrap();
        assert_eq!(query.code.as_deref(), Some("c1"));
        assert_eq!(query.state.as_deref(), Some("s1"));
        assert_eq!(query.iss.as_deref(), Some("https://boat.dev"));
        assert_eq!(callback_query("/favicon.ico"), None);
        assert_eq!(
            callback_query("/boat/callback?code=a&code=b")
                .unwrap()
                .error
                .as_deref(),
            Some("invalid_request")
        );
    }

    #[tokio::test]
    async fn a_browser_visit_arrives_and_gets_its_page() {
        let mut pending = LoopbackCallbacks.open().await.unwrap();
        let redirect = pending.redirect_uri();
        let port: u16 = redirect
            .trim_start_matches("http://localhost:")
            .split('/')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        let browser = tokio::spawn(async move {
            let mut other = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            other
                .write_all(b"GET /favicon.ico HTTP/1.1\r\n\r\n")
                .await
                .unwrap();
            let mut answer = String::new();
            other.read_to_string(&mut answer).await.unwrap();
            assert!(answer.starts_with("HTTP/1.1 404"));
            let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            stream
                .write_all(
                    b"GET /boat/callback?code=c&state=s&iss=x HTTP/1.1\r\nHost: localhost\r\n\r\n",
                )
                .await
                .unwrap();
            let mut answer = String::new();
            stream.read_to_string(&mut answer).await.unwrap();
            answer
        });
        let (query, reply) = pending.arrived().await.unwrap();
        assert_eq!(query.code.as_deref(), Some("c"));
        reply.send(CallbackPage::Connected).unwrap();
        let answer = browser.await.unwrap();
        assert!(answer.starts_with("HTTP/1.1 200"));
        assert!(answer.contains("Nessa is connected to Boat"));
    }
}
