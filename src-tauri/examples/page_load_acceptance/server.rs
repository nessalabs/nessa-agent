//! The real allowed-origin control, owned only by this process.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub const CONTROL_URL: &str = "http://127.0.0.1:1420/control.html";

pub struct Server {
    stop: Arc<AtomicBool>,
    pub served: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Server {
    pub fn start() -> Result<Self, String> {
        // The policy owns port 1420. A collision fails rather than widening it.
        let listener = TcpListener::bind("127.0.0.1:1420")
            .map_err(|error| format!("bind allowed-origin fixture: {error}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let served = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let observed = served.clone();
        let worker = thread::spawn(move || {
            while !stopped.load(Ordering::Acquire) {
                if let Ok((mut stream, _)) = listener.accept() {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
                    // WebKit can open a connection before it has a request
                    // for it. Answering one that sent nothing would hand the
                    // control page over a socket the fixture never saw asked,
                    // so the whole request head is read first, and a
                    // connection that sent none is closed unanswered.
                    let Some(request) = request_head(&mut stream) else {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    };
                    if request.starts_with(b"GET /control.html ") {
                        observed.store(true, Ordering::Release);
                    }
                    let body =
                        "<!doctype html><title>Allowed origin</title><p>Native control loaded.</p>";
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                }
                thread::sleep(Duration::from_millis(5));
            }
        });
        Ok(Self {
            stop,
            served,
            worker: Some(worker),
        })
    }
}

/// The request head `stream` sent, up to its blank line; `None` when it sent
/// nothing before closing or going quiet.
fn request_head(stream: &mut std::net::TcpStream) -> Option<Vec<u8>> {
    let mut head = Vec::new();
    let mut chunk = [0; 4096];
    while !head.windows(4).any(|end| end == b"\r\n\r\n") && head.len() < 8192 {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(length) => head.extend_from_slice(&chunk[..length]),
        }
    }
    (!head.is_empty()).then_some(head)
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
