//! Exercise real socket flags and request framing without a native window.

use std::io::Cursor;
use std::net::Shutdown;
use std::sync::mpsc;

use super::*;

#[test]
fn an_inherited_nonblocking_stream_waits_for_the_request() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (accepted, ready) = mpsc::channel();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        // Force Darwin's accepted-stream mode on every test platform. Do not
        // send any request until that mode has been established.
        stream.set_nonblocking(true).unwrap();
        accepted.send(()).unwrap();
        serve(&mut stream, &AtomicBool::new(false))
    });
    let mut client = TcpStream::connect(address).unwrap();
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    client
        .set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    let error = client.read(&mut [0]).unwrap_err();
    assert!(matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    ));
    client
        .write_all(b"GET /control.html HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut response = String::new();
    client.read_to_string(&mut response).unwrap();
    assert!(worker.join().unwrap().unwrap());
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
}

#[test]
fn fragmented_headers_do_not_receive_an_early_success() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        serve(&mut stream, &AtomicBool::new(false))
    });
    let mut client = TcpStream::connect(address).unwrap();
    client.write_all(b"GET /con").unwrap();
    client
        .set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    let error = client.read(&mut [0]).unwrap_err();
    assert!(matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    ));
    client
        .write_all(b"trol.html HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut response = String::new();
    client.read_to_string(&mut response).unwrap();
    assert!(worker.join().unwrap().unwrap());
    assert!(response.contains("Native control loaded."));
}

#[test]
fn only_a_completed_control_request_is_recorded_as_served() {
    let server = Server::start().unwrap();
    for (path, expected) in [
        ("/other.html", "404 Not Found"),
        ("/control.html", "200 OK"),
    ] {
        let mut client = TcpStream::connect("127.0.0.1:1420").unwrap();
        write!(client, "GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        assert!(response.starts_with(&format!("HTTP/1.1 {expected}\r\n")));
        assert_eq!(
            server.served.load(Ordering::Acquire),
            path == "/control.html"
        );
    }
    assert_eq!(CONTROL_URL, "http://127.0.0.1:1420/control.html");
}

#[test]
fn incomplete_and_oversized_headers_cannot_receive_a_control_page() {
    for (request, kind) in [
        (
            b"GET /control.html HTTP/1.1\r\n".to_vec(),
            io::ErrorKind::UnexpectedEof,
        ),
        (vec![b'A'; 8192], io::ErrorKind::InvalidData),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            serve(&mut stream, &AtomicBool::new(false))
        });
        let mut client = TcpStream::connect(address).unwrap();
        client.write_all(&request).unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        assert_eq!(worker.join().unwrap().unwrap_err().kind(), kind);
        assert!(response.is_empty());
    }
}

const CONTROL: &[u8] = b"GET /control.html HTTP/1.1\r\nHost: 127.0.0.1:1420\r\n\r\n";

fn exchange(request: &[u8]) -> (io::Result<bool>, bool, Vec<u8>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let observed = AtomicBool::new(false);
        let result = serve(&mut stream, &observed);
        (result, observed.load(Ordering::Acquire))
    });
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream.write_all(request).unwrap();
    stream.shutdown(Shutdown::Write).unwrap();
    let mut response = Vec::new();
    let read = stream.read_to_end(&mut response);
    if let Err(error) = read {
        assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    }
    let (result, observed) = worker.join().unwrap();
    (result, observed, response)
}

#[test]
fn complete_control_records_the_request_before_success() {
    for request in [
        CONTROL,
        b"GET /control.html HTTP/1.0\r\nHost: localhost\r\n\r\n",
    ] {
        let (result, observed, response) = exchange(request);
        assert!(result.unwrap());
        assert!(observed);
        assert!(response.starts_with(b"HTTP/1.1 200 OK\r\n"));
    }
}

#[test]
fn fragmented_control_does_not_receive_an_early_response() {
    let mut chunks = Cursor::new(CONTROL);
    let mut calls = 0;
    assert!(control_request(|buffer, remaining| {
        assert!(remaining <= Duration::from_secs(1));
        calls += 1;
        if calls == 1 {
            thread::sleep(Duration::from_millis(20));
        } else if calls == 2 {
            assert!(
                remaining < Duration::from_millis(990),
                "a partial read reset the budget"
            );
        }
        // Every read delivers just one byte, including the initial G.
        chunks.read(&mut buffer[..1])
    })
    .unwrap());
    assert_eq!(calls, CONTROL.len());
}

#[test]
fn a_complete_header_returned_after_the_deadline_is_refused() {
    let result = control_request(|buffer, remaining| {
        thread::sleep(remaining + Duration::from_millis(20));
        buffer[..CONTROL.len()].copy_from_slice(CONTROL);
        Ok(CONTROL.len())
    });
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
}

#[test]
fn other_requests_do_not_publish_a_control_witness() {
    for request in [
        b"GET /other HTTP/1.1\r\n\r\n".as_slice(),
        b"POST /control.html HTTP/1.1\r\n\r\n".as_slice(),
        b"GET /control.html?other HTTP/1.1\r\n\r\n".as_slice(),
    ] {
        let (result, observed, response) = exchange(request);
        result.unwrap();
        assert!(!observed);
        assert!(response.starts_with(b"HTTP/1.1 404 Not Found\r\n"));
    }
}

#[test]
fn incomplete_and_oversized_headers_cannot_publish_success() {
    for request in [b"G".as_slice(), &[b'G'; 8192]] {
        let (result, observed, response) = exchange(request);
        assert!(result.is_err());
        assert!(!observed);
        assert!(response.is_empty());
    }
}

#[test]
fn a_header_at_the_request_bound_can_still_be_complete() {
    let mut header = b"GET /control.html HTTP/1.1\r\nX: ".to_vec();
    header.resize(8192 - 4, b'a');
    header.extend_from_slice(b"\r\n\r\n");
    let (result, observed, response) = exchange(&header);
    result.unwrap();
    assert!(observed);
    assert!(response.starts_with(b"HTTP/1.1 200 OK\r\n"));
}

#[test]
fn stalled_header_keeps_the_original_read_budget() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let observed = AtomicBool::new(false);
        let result = serve(&mut stream, &observed);
        assert!(!observed.load(Ordering::Acquire));
        let error = result.unwrap_err();
        assert!(matches!(
            error.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
        ));
    });
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream.write_all(b"G").unwrap();
    let start = Instant::now();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    assert!(response.is_empty());
    assert!(start.elapsed() < Duration::from_secs(2));
    worker.join().unwrap();
}
