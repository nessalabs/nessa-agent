//! The real allowed-origin control, owned only by this process.

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;

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
                    if let Err(error) = serve(&mut stream, &observed) {
                        eprintln!("page_load_acceptance: control request failed: {error}");
                    }
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

fn serve(stream: &mut TcpStream, observed: &AtomicBool) -> io::Result<()> {
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    let control = control_request(|buffer, remaining| {
        stream.set_read_timeout(Some(remaining))?;
        stream.read(buffer)
    })?;
    let (status, body) = if control {
        // Publish the request witness before its success can reach WebKit.
        observed.store(true, Ordering::Release);
        (
            "200 OK",
            "<!doctype html><title>Allowed origin</title><p>Native control loaded.</p>",
        )
    } else {
        ("404 Not Found", "Not found")
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes())
}

/// The fixture's read seam retains one deadline across partial TCP reads.
fn control_request(
    mut read: impl FnMut(&mut [u8], Duration) -> io::Result<usize>,
) -> io::Result<bool> {
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut request = [0; 8192];
    let mut used = 0;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let length = read(&mut request[used..], remaining)?;
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        if length == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        used += length;
        if request[..used].windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            let line_end = request[..used]
                .windows(2)
                .position(|bytes| bytes == b"\r\n")
                .ok_or(io::ErrorKind::InvalidData)?;
            return Ok(&request[..line_end] == b"GET /control.html HTTP/1.1");
        }
        if used == request.len() {
            return Err(io::ErrorKind::InvalidData.into());
        }
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
