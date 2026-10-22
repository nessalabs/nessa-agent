//! Exercise real socket flags and request framing without a native window.

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
        serve(&mut stream)
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
        serve(&mut stream)
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
            serve(&mut stream)
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
