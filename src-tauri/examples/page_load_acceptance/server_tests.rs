use super::*;
use std::io::Cursor;
use std::net::Shutdown;

const CONTROL: &[u8] = b"GET /control.html HTTP/1.1\r\nHost: 127.0.0.1:1420\r\n\r\n";

fn exchange(request: &[u8]) -> (io::Result<()>, bool, Vec<u8>) {
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
    let (result, observed, response) = exchange(CONTROL);
    result.unwrap();
    assert!(observed);
    assert!(response.starts_with(b"HTTP/1.1 200 OK\r\n"));
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
