use super::*;
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

use rustls::crypto::CryptoProvider;

use crate::agent_install::application::{InstallAgentRuntime, RuntimeStore};
use crate::agent_install::domain::AgentName;
use crate::agent_install::infrastructure::{host_platform, releases_for, ManagedRuntimes};
use crate::agent_install_test_support::{audit, delivery, request, temporary_root};

#[test]
fn building_a_client_names_the_tls_backend_this_process_uses() {
    // `reqwest` is taken here without one, so the client it builds either
    // finds a provider already installed or panics inside the library. This is
    // that install, asserted from the outside: once a client exists this
    // process has a default, and it is the implementation this module named
    // rather than whichever one the build graph happened to leave lying about.
    //
    // Compared against *ring*'s own provider rather than merely asserted to
    // exist, because "some backend is installed" is satisfied by the feature
    // that chooses `aws-lc-rs` for the whole build graph — which is the thing
    // this is here to keep out. The comparison is over the suites themselves,
    // which are statics, so it is the two providers being the same one and not
    // two lists that read alike.
    HttpsArchives::new().expect("an https client");

    let installed = CryptoProvider::get_default()
        .expect("this process has a tls backend")
        .clone();

    assert_eq!(
        installed.cipher_suites,
        rustls::crypto::ring::default_provider().cipher_suites,
        "the tls backend is not the one this module installs"
    );
}

/// A staged file in a temporary directory, so the tests below can write into
/// something real without the store's own rules getting in the way.
fn staged(directory: &std::path::Path) -> StagedArchive {
    let path = directory.join("download");
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .expect("a temporary staged file");
    StagedArchive::new(file, path)
}

/// A reader that hands over some bytes and then comes apart, standing in for a
/// transfer that stops part-way.
struct Interrupted {
    before: Vec<u8>,
}

impl Read for Interrupted {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.before.is_empty() {
            return Err(io::Error::new(io::ErrorKind::ConnectionReset, "reset"));
        }
        let taken = self.before.len().min(buffer.len());
        buffer[..taken].copy_from_slice(&self.before[..taken]);
        self.before.drain(..taken);
        Ok(taken)
    }
}

#[test]
fn a_status_that_says_no_carries_the_status() {
    // A 404 is a pin naming a release that has been withdrawn; a 503 is worth
    // trying again. Keeping the number is what lets them be told apart.
    assert_eq!(refusal(404), Some(SourceFailure::Refused(404)));
    assert_eq!(refusal(503), Some(SourceFailure::Refused(503)));
    assert_eq!(refusal(302), Some(SourceFailure::Refused(302)));
}

#[test]
fn a_success_is_not_a_refusal() {
    assert_eq!(refusal(200), None);
    assert_eq!(refusal(206), None);
}

#[test]
fn a_body_that_arrives_is_stored_whole() {
    let root = temporary_root();
    let mut staged = staged(root.path());
    let body = b"archive bytes".repeat(10_000);

    store(&mut body.as_slice(), &mut staged, 1_000_000).expect("the body is stored");

    let written = std::fs::read(staged.path()).expect("the staged file reads");
    assert_eq!(written, body);
}

#[test]
fn a_transfer_that_comes_apart_is_the_networks_doing() {
    let root = temporary_root();
    let mut staged = staged(root.path());
    let mut body = Interrupted {
        before: b"half an archive".to_vec(),
    };

    let failure = store(&mut body, &mut staged, 1_000_000).expect_err("an interrupted transfer");

    assert!(
        matches!(failure, SourceFailure::Unreachable(_)),
        "an interrupted transfer is not a refusal or a full disk: {failure:?}"
    );
}

#[test]
fn a_body_that_cannot_be_written_is_this_machines_doing() {
    // A full disk reported as an unreachable registry sends somebody to check
    // their connection when the problem is in front of them.
    let root = temporary_root();
    let path = root.path().join("download");
    std::fs::write(&path, b"").expect("an empty staged file");
    let readable = File::open(&path).expect("a handle that cannot be written");
    let mut staged = StagedArchive::new(readable, path);

    let failure =
        store(&mut b"archive bytes".as_slice(), &mut staged, 1_000_000).expect_err("no write");

    assert!(
        matches!(failure, SourceFailure::NotStored(_)),
        "a write that could not complete is not a network failure: {failure:?}"
    );
}

#[test]
fn a_body_that_does_not_stop_is_refused_rather_than_kept() {
    // Nothing has been hashed at this point, so the digest cannot help: without
    // a bound, a server that answers forever fills the disk for as long as the
    // transfer timeout allows.
    let root = temporary_root();
    let mut staged = staged(root.path());
    let mut endless = io::repeat(b'x');

    let failure = store(&mut endless, &mut staged, 4096).expect_err("an endless body");

    assert_eq!(failure, SourceFailure::TooLarge(4096));
    let kept = std::fs::metadata(staged.path())
        .expect("the staged file exists")
        .len();
    assert!(
        kept <= 4096 + TRANSFER_CHUNK as u64,
        "an endless body kept writing past the bound: {kept} bytes"
    );
}

#[test]
fn a_body_exactly_at_the_bound_is_kept() {
    let root = temporary_root();
    let mut staged = staged(root.path());
    let body = vec![b'x'; 4096];

    store(&mut body.as_slice(), &mut staged, 4096).expect("a body at the bound is not over it");
}

/// A one-shot HTTP server on loopback, answering the first request with `body`.
///
/// Returns the port and a handle that finishes once that request has been
/// served. Nothing here is asserted on directly; it exists so that the test
/// below is asking about the client's own rule rather than about whether a
/// connection could be made.
fn one_plain_http_reply(body: &'static [u8]) -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let port = listener.local_addr().expect("the bound address").port();
    let served = thread::spawn(move || {
        let Ok((mut connection, _)) = listener.accept() else {
            return;
        };
        // Read just the request line and headers, so the client is not left
        // waiting on a response to something half-read.
        let mut request = Vec::new();
        let mut byte = [0u8; 1];
        while !request.ends_with(b"\r\n\r\n") {
            match connection.read(&mut byte) {
                Ok(0) | Err(_) => return,
                Ok(_) => request.push(byte[0]),
            }
        }
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/gzip\r\n\r\n",
            body.len()
        );
        let _ = connection
            .write_all(header.as_bytes())
            .and_then(|()| connection.write_all(body))
            .and_then(|()| connection.flush());
    });
    (port, served)
}

#[test]
fn a_plain_http_url_is_never_fetched() {
    // The pin's own rule is that an archive comes over https, and the digest is
    // only checked after the bytes have arrived. This is the client refusing to
    // be the place that rule stops holding.
    //
    // The url points at a real server answering 200 with a real body, so the
    // fetch would plainly succeed if the rule were dropped. Aiming at a closed
    // port instead would fail for the wrong reason, and go on passing after
    // somebody removed `https_only`.
    //
    // What this cannot reach is the other half of the same rule: an https url
    // that redirects to http. Following that needs a server presenting a
    // certificate this client will accept, which is a TLS fixture rather than a
    // unit test. Both halves are the one `https_only` setting, so this covers
    // the setting; it does not cover the redirect path separately.
    let archive = b"archive bytes that would be stored if http were fetched";
    let (port, served) = one_plain_http_reply(archive);

    let root = temporary_root();
    let mut staged = staged(root.path());
    let source = HttpsArchives::new().expect("an https client");

    let failure = source
        .download(
            &format!("http://127.0.0.1:{port}/runtime.tgz"),
            archive.len() as u64,
            &mut staged,
        )
        .expect_err("plain http is not fetched");

    assert!(
        matches!(failure, SourceFailure::Unreachable(_)),
        "{failure:?}"
    );
    assert_eq!(
        std::fs::metadata(staged.path())
            .expect("the staged file exists")
            .len(),
        0,
        "nothing may be written for a url that was never fetched"
    );

    // The server is still waiting to be asked, which is the point: the request
    // never left this process. Asking it once releases the accept so the thread
    // ends with the test rather than outliving it.
    assert!(
        TcpStream::connect(("127.0.0.1", port)).is_ok(),
        "the server was listening the whole time"
    );
    served.join().expect("the one-shot server finishes");
}

/// Install the real pinned Opencode release, over the real network.
///
/// Ignored by default: it fetches around fifty megabytes and depends on the npm
/// registry being reachable, neither of which belongs in a suite that runs on
/// every change. It is checked in because it is the only test that exercises
/// the whole path against the artifact Nessa actually ships a pin for — a fake
/// archive cannot tell you that the pinned digest is right, that the real
/// tarball puts its executable where the pin says, or that what comes out is a
/// binary this machine will run.
///
/// Run it after changing the pin:
///
/// ```text
/// cargo test -p nessa-server --lib -- --ignored installs_the_pinned_release
/// ```
#[test]
#[ignore = "downloads the real release archive"]
fn installs_the_pinned_release() {
    let platform = host_platform();
    let agent = AgentName::parse("opencode").expect("a plain agent name");
    let pinned = releases_for(&agent).expect("the pinned releases parse");
    let Some(release) = pinned
        .into_iter()
        .find(|release| release.runs_on(&platform))
    else {
        // Not a failure: the pin covers the platforms Nessa installs on, and a
        // developer on another one should not see a red test for it.
        eprintln!("no opencode release pinned for {platform}; nothing to install");
        return;
    };

    let root = temporary_root();
    let store = ManagedRuntimes::new(root.path());
    let source = HttpsArchives::new().expect("an https client");

    let installed = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: audit(),
        delivery: delivery(),
    }
    .execute(&agent, &release, &platform, &request())
    .expect("the pinned release installs");

    assert!(installed.downloaded);
    assert_eq!(&installed.version, release.version());
    assert!(installed.executable.is_file());

    // The pinned digest is only worth having if it matches what the registry
    // actually serves today. Installing at all proves it did.
    let size = installed
        .executable
        .metadata()
        .expect("reading the installed runtime")
        .len();
    assert!(
        size > 1_000_000,
        "an agent runtime should be a real binary, got {size} bytes"
    );

    // The point of a pin is that the version Nessa tested is the version that
    // runs. Nothing so far has asked the binary itself: the digest proves the
    // archive was the measured one, not that what came out of it launches or
    // agrees about what it is. Running it closes both gaps at once — and on
    // macOS it is also what would catch a binary the system refuses to execute.
    let reported = std::process::Command::new(&installed.executable)
        .arg("--version")
        .output()
        .expect("the installed runtime can be launched");
    let version = String::from_utf8_lossy(&reported.stdout);
    assert!(
        version.contains(release.version().as_str()),
        "the installed runtime reports {version:?}, not the pinned {}",
        release.version()
    );

    // Asking again must be free. This is what stops a first-run surface from
    // downloading a hundred megabytes because somebody pressed the button twice.
    let again = InstallAgentRuntime {
        source: &source,
        store: &store,
        audit: audit(),
        delivery: delivery(),
    }
    .execute(&agent, &release, &platform, &request())
    .expect("an installed runtime is reported as installed");
    assert!(!again.downloaded);
    assert_eq!(again.executable, installed.executable);

    // And the record survives a fresh store over the same directory, which is
    // what happens on the next launch of the app.
    let reopened = ManagedRuntimes::new(root.path());
    let executable = reopened
        .installed(&agent, &release)
        .expect("the store can be read")
        .expect("the runtime is still installed");
    assert_eq!(executable, installed.executable);
}
