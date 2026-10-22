//! The real allowed-origin control, owned only by this process.

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

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
                    match serve(&mut stream) {
                        Ok(true) => {
                            observed.store(true, Ordering::Release);
                        }
                        Ok(false) => {}
                        Err(error) => {
                            eprintln!("page_load_acceptance: control HTTP fixture: {error}");
                        }
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

fn serve(stream: &mut TcpStream) -> io::Result<bool> {
    // Darwin inherits the listener's nonblocking flag on accept. A read timeout
    // does not clear it: the first read could fail before WebKit sends its GET.
    // `an_inherited_nonblocking_stream_waits_for_the_request` covers this seam.
    stream.set_nonblocking(false)?;
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut request = [0; 8192];
    let mut length = 0;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "request deadline"));
        }
        stream.set_read_timeout(Some(remaining))?;
        let read = stream.read(&mut request[length..])?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "request headers",
            ));
        }
        length += read;
        if request[..length]
            .windows(4)
            .any(|bytes| bytes == b"\r\n\r\n")
        {
            break;
        }
        if length == request.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request exceeds 8192 bytes",
            ));
        }
    }
    let control = request[..length].starts_with(b"GET /control.html HTTP/1.1\r\n")
        || request[..length].starts_with(b"GET /control.html HTTP/1.0\r\n");
    let (status, body) = if control {
        (
            "200 OK",
            "<!doctype html><title>Allowed origin</title><p>Native control loaded.</p>",
        )
    } else {
        ("404 Not Found", "No control page at this request target.")
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    stream.write_all(response.as_bytes())?;
    Ok(control)
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
#[path = "server/tests.rs"]
mod tests;
