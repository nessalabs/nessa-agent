//! A stand-in for cloudflared, as the stress check's `fake-cloudflared.mjs`
//! is: the gateway starts it as it would the real program
//! (`NESSA_TUNNEL_PROGRAM`, debug builds only) and asks its metrics address
//! `/ready` and `/quicktunnel`. Its public side copies each TCP connection
//! to the ingress `--url` names, so a WebSocket opened there reaches the
//! gateway's real tunnel ingress, as it would through Cloudflare's edge.
//!
//! ```text
//! dialer ── ws://127.0.0.1:<edge>/native ──▶ this program ──▶ the gateway's ingress
//! ```
//!
//! Everything it prints goes to `<tunnel>/cloudflared.log`: its pid and
//! public origin once it serves, then `connection N` for each one taken.
use super::CLOUDFLARED;
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// The stand-in as the gateway runs it, and the log it writes.
pub(in crate::composition::read_only_online_tests) struct Edge {
    program: PathBuf,
}
impl Edge {
    /// A script in `directory` that runs this test binary's stand-in. The
    /// gateway clears the program's environment, so the script carries its
    /// arguments across in one of its own.
    pub(in crate::composition::read_only_online_tests) fn new(directory: &Path) -> Self {
        use std::os::unix::fs::PermissionsExt;
        let program = directory.join("cloudflared");
        let exe = std::env::current_exe().unwrap();
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\nNESSA_FAKE_CLOUDFLARED=\"$*\" exec '{}' --exact {CLOUDFLARED} --nocapture\n",
                exe.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self { program }
    }
    /// The program to name as `NESSA_TUNNEL_PROGRAM`.
    pub(in crate::composition::read_only_online_tests) fn program(&self) -> &Path {
        &self.program
    }
    /// The pid and public origin it printed once serving for the gateway
    /// at `root`.
    pub(in crate::composition::read_only_online_tests) fn serving(root: &Path) -> (u32, String) {
        let log = root.join("tunnel").join("cloudflared.log");
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let text = std::fs::read_to_string(&log).unwrap_or_default();
            if let Some(line) = text
                .lines()
                .find(|line| line.starts_with("fake cloudflared "))
            {
                let mut words = line.split(' ').skip(2);
                let pid = words.next().unwrap().parse().unwrap();
                let origin = words.nth(1).unwrap().to_owned();
                return (pid, origin);
            }
            assert!(Instant::now() < deadline, "the tunnel's program serves");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    /// How many connections it has taken for the gateway at `root`.
    pub(in crate::composition::read_only_online_tests) fn connections(root: &Path) -> usize {
        std::fs::read_to_string(root.join("tunnel").join("cloudflared.log"))
            .unwrap_or_default()
            .lines()
            .filter(|line| line.starts_with("connection "))
            .count()
    }
}

/// Ends a process when dropped: the gateway leaves its tunnel's program
/// running when it stops, as it does across restarts.
pub(in crate::composition::read_only_online_tests) struct EndsProcess(pub u32);
impl Drop for EndsProcess {
    fn drop(&mut self) {
        // SAFETY: kill takes plain integers; the pid is the test's own
        // stand-in, which nothing else waits on.
        unsafe {
            libc::kill(self.0 as libc::pid_t, libc::SIGKILL);
        }
    }
}

fn argument<'a>(args: &'a [&str], name: &str) -> &'a str {
    let at = args.iter().position(|arg| *arg == name).unwrap();
    args[at + 1]
}

fn copy(mut from: TcpStream, mut to: TcpStream) {
    let _ = io::copy(&mut from, &mut to);
    let _ = to.shutdown(std::net::Shutdown::Both);
    let _ = from.shutdown(std::net::Shutdown::Both);
}

#[test]
fn fake_cloudflared_child() {
    let Ok(args) = std::env::var("NESSA_FAKE_CLOUDFLARED") else {
        return;
    };
    let args: Vec<&str> = args.split(' ').collect();
    let metrics = argument(&args, "--metrics").to_owned();
    let ingress = argument(&args, "--url")
        .strip_prefix("http://")
        .unwrap()
        .to_owned();
    let edge = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://{}", edge.local_addr().unwrap());
    let quick = format!("{{\"hostname\":\"{origin}\"}}");
    let answers = TcpListener::bind(&metrics).unwrap();
    std::thread::spawn(move || {
        for request in answers.incoming() {
            let Ok(mut request) = request else { continue };
            let mut line = String::new();
            let _ = BufReader::new(request.try_clone().unwrap()).read_line(&mut line);
            let (status, body) = match line.split(' ').nth(1) {
                Some("/ready") => ("200 OK", "{\"status\":200,\"readyConnections\":1}"),
                Some("/quicktunnel") => ("200 OK", quick.as_str()),
                _ => ("404 Not Found", ""),
            };
            let _ = write!(
                request,
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let mut rest = [0u8; 1024];
            let _ = request.set_read_timeout(Some(Duration::from_millis(100)));
            let _ = request.read(&mut rest);
        }
    });
    println!("fake cloudflared {} at {origin}", std::process::id());
    for (taken, socket) in edge.incoming().enumerate() {
        let Ok(socket) = socket else { continue };
        println!("connection {}", taken + 1);
        let Ok(upstream) = TcpStream::connect(&ingress) else {
            continue;
        };
        let (back, up) = (socket.try_clone().unwrap(), upstream.try_clone().unwrap());
        std::thread::spawn(move || copy(socket, upstream));
        std::thread::spawn(move || copy(up, back));
    }
}
