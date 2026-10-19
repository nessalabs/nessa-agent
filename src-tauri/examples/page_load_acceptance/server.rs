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
                    let mut request = [0; 8192];
                    if let Ok(length) = stream.read(&mut request) {
                        if request[..length].starts_with(b"GET /control.html ") {
                            observed.store(true, Ordering::Release);
                        }
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

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
