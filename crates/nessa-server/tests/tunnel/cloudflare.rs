//! The Cloudflare provider without Cloudflare: the pins, the checked
//! install, what it runs, and how it reads a ready tunnel's address.
use super::*;
use crate::agent_install::application::SourceFailure;
use crate::tunnel::application::TunnelToken;

/// Serves the bytes it was given for any URL.
struct Serving(Vec<u8>);
impl ArchiveSource for Serving {
    fn download(
        &self,
        _url: &str,
        at_most: u64,
        staged: &mut StagedArchive,
    ) -> Result<(), SourceFailure> {
        if self.0.len() as u64 > at_most {
            return Err(SourceFailure::TooLarge(at_most));
        }
        std::io::Write::write_all(staged.file_mut(), &self.0)
            .map_err(|error| SourceFailure::Unreachable(error.to_string()))
    }
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

const PROGRAM_BYTES: &[u8] = b"#!/bin/sh\nexit 0\n";

fn a_build(packaging: Packaging, archive: &[u8]) -> PinnedBuild {
    PinnedBuild {
        operating_system: "linux".into(),
        architecture: "x86_64".into(),
        url: "https://example.invalid/cloudflared".into(),
        packaging,
        archive_bytes: archive.len() as u64,
        archive_digest: sha256(archive),
        program_bytes: PROGRAM_BYTES.len() as u64,
        program_digest: sha256(PROGRAM_BYTES),
        version: "test".into(),
    }
}

fn tgz(member: &str, bytes: &[u8]) -> Vec<u8> {
    let mut archive = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    archive.append_data(&mut header, member, bytes).unwrap();
    archive.into_inner().unwrap().finish().unwrap()
}

#[test]
fn every_supported_platform_has_a_pin_and_others_have_none() {
    for (system, architecture) in [
        ("macos", "aarch64"),
        ("macos", "x86_64"),
        ("linux", "x86_64"),
        ("linux", "aarch64"),
    ] {
        let build = PinnedBuild::for_platform(system, architecture).expect("pinned");
        assert!(build
            .url
            .starts_with("https://github.com/cloudflare/cloudflared/releases/download/"));
        assert!(build.url.contains(&build.version));
        assert_eq!(build.archive_digest.len(), 64);
        assert_eq!(build.program_digest.len(), 64);
    }
    assert!(PinnedBuild::for_platform("windows", "x86_64").is_none());
}

#[test]
fn a_pinned_program_is_installed_runnable_and_kept() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let build = a_build(Packaging::Program, PROGRAM_BYTES);
    let program = install(directory.path(), &build, &Serving(PROGRAM_BYTES.to_vec())).unwrap();
    assert_eq!(std::fs::read(&program).unwrap(), PROGRAM_BYTES);
    assert_eq!(
        std::fs::metadata(&program).unwrap().permissions().mode() & 0o777,
        0o700
    );
    // Already there and matching: nothing is fetched.
    let refusing = Serving(b"anything else".to_vec());
    assert_eq!(
        install(directory.path(), &build, &refusing).unwrap(),
        program
    );
}

#[test]
fn a_program_from_its_archive_is_installed() {
    let directory = tempfile::tempdir().unwrap();
    let archive = tgz("cloudflared", PROGRAM_BYTES);
    let build = a_build(Packaging::Tgz, &archive);
    let program = install(directory.path(), &build, &Serving(archive)).unwrap();
    assert_eq!(std::fs::read(program).unwrap(), PROGRAM_BYTES);
}

#[test]
fn nothing_that_does_not_match_its_pin_is_made_runnable() {
    let directory = tempfile::tempdir().unwrap();
    // A download that is not the pinned archive.
    let pinned = a_build(Packaging::Program, PROGRAM_BYTES);
    assert_eq!(
        install(
            directory.path(),
            &pinned,
            &Serving(b"#!/bin/sh\nexit 1\n".to_vec())
        ),
        Err(ServiceFailure::Unavailable("unverified"))
    );
    assert!(!installed(directory.path(), &pinned).exists());
    // The pinned archive, holding no program by that name.
    let archive = tgz("elsewhere", PROGRAM_BYTES);
    let build = a_build(Packaging::Tgz, &archive);
    assert_eq!(
        install(directory.path(), &build, &Serving(archive)),
        Err(ServiceFailure::Unavailable("unverified"))
    );
    // The pinned archive, holding a program that is not the pinned one.
    let archive = tgz("cloudflared", b"#!/bin/sh\nexit 2\n");
    let build = a_build(Packaging::Tgz, &archive);
    assert_eq!(
        install(directory.path(), &build, &Serving(archive)),
        Err(ServiceFailure::Unavailable("unverified"))
    );
    assert!(!installed(directory.path(), &build).exists());
}

#[test]
fn a_kept_program_that_no_longer_matches_is_fetched_again() {
    let directory = tempfile::tempdir().unwrap();
    let build = a_build(Packaging::Program, PROGRAM_BYTES);
    let program = install(directory.path(), &build, &Serving(PROGRAM_BYTES.to_vec())).unwrap();
    std::fs::write(&program, b"changed").unwrap();
    install(directory.path(), &build, &Serving(PROGRAM_BYTES.to_vec())).unwrap();
    assert_eq!(std::fs::read(program).unwrap(), PROGRAM_BYTES);
}

#[test]
fn a_quick_answer_names_an_https_address_or_a_loopback_stand_in() {
    let address = |body: &str, stand_in: bool| {
        quick_address(body.as_bytes(), stand_in).map(|address| address.as_str().to_owned())
    };
    assert_eq!(
        address(r#"{"hostname":"a-b-c.trycloudflare.com"}"#, false).as_deref(),
        Some("https://a-b-c.trycloudflare.com")
    );
    // Whatever answers the metrics port names only a quick tunnel's address.
    for refused in [
        r#"{"hostname":"https://evil.example"}"#,
        r#"{"hostname":"evil.example"}"#,
        r#"{"hostname":"http://127.0.0.1:5"}"#,
        r#"{"hostname":".trycloudflare.com"}"#,
        r#"{"hostname":"a.b.trycloudflare.com"}"#,
        r#"{"hostname":"a/b.trycloudflare.com"}"#,
    ] {
        assert_eq!(address(refused, false), None, "{refused}");
    }
    assert_eq!(
        address(r#"{"hostname":"http://127.0.0.1:5"}"#, true).as_deref(),
        Some("http://127.0.0.1:5")
    );
    assert_eq!(address(r#"{"hostname":"http://example.com"}"#, true), None);
    assert_eq!(address(r#"{"hostname":""}"#, true), None);
    assert_eq!(address("not json", true), None);
}

#[test]
fn an_answer_is_its_status_and_body() {
    assert_eq!(
        parse_answer(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok"),
        Some((200, b"ok".to_vec()))
    );
    assert_eq!(
        parse_answer(b"HTTP/1.1 503 Not Ready\r\n\r\n"),
        Some((503, vec![]))
    );
    assert_eq!(parse_answer(b"garbage"), None);
}

fn service(directory: &Path, tunnel: CloudflareTunnel) -> CloudflareService {
    CloudflareService::new(
        TunnelFiles::new(directory),
        Program::Given("/bin/true".into()),
        Arc::new(|| Some(Box::new(Serving(vec![])) as Box<dyn ArchiveSource>)),
        tunnel,
        "127.0.0.1:47651".parse().unwrap(),
    )
}

fn named(token: &str) -> CloudflareTunnel {
    CloudflareTunnel::Named {
        address: TunnelAddress::try_from("https://gateway.example.com".to_owned()).unwrap(),
        token: TunnelToken::new(token.into()).unwrap(),
    }
}

#[test]
fn a_quick_tunnel_forwards_to_the_ingress_and_reports_on_loopback() {
    let directory = tempfile::tempdir().unwrap();
    let launch = service(directory.path(), CloudflareTunnel::Quick)
        .launch()
        .unwrap();
    let args: Vec<_> = launch
        .args
        .iter()
        .map(|arg| arg.to_str().unwrap())
        .collect();
    assert_eq!(
        &args[..5],
        [
            "tunnel",
            "--no-autoupdate",
            "--loglevel",
            "warn",
            "--metrics"
        ]
    );
    assert!(args[5].starts_with("127.0.0.1:"));
    assert_eq!(&args[6..], ["--url", "http://127.0.0.1:47651"]);
    let home = directory.path().join("tunnel");
    assert_eq!(launch.environment, [("HOME".into(), home.clone().into())]);
    assert_eq!(launch.directory, home);
}

#[test]
fn a_named_tunnel_reads_its_token_from_the_file_never_the_command_line() {
    let directory = tempfile::tempdir().unwrap();
    let launch = service(directory.path(), named("secret-token"))
        .launch()
        .unwrap();
    let args: Vec<_> = launch
        .args
        .iter()
        .map(|arg| arg.to_str().unwrap())
        .collect();
    let token_file = directory.path().join("tunnel").join("token");
    assert_eq!(
        &args[6..],
        ["run", "--token-file", token_file.to_str().unwrap()]
    );
    assert!(args.iter().all(|arg| !arg.contains("secret-token")));
}

#[test]
fn the_key_tells_settings_apart_and_never_holds_the_token() {
    let directory = tempfile::tempdir().unwrap();
    let quick = service(directory.path(), CloudflareTunnel::Quick)
        .key()
        .unwrap();
    let one = service(directory.path(), named("one")).key().unwrap();
    let two = service(directory.path(), named("two")).key().unwrap();
    assert_ne!(quick, one);
    assert_ne!(one, two);
    assert!(!one.contains("one ") && !one.ends_with("one"));
    // The same setting, the same key, whatever port each launch takes.
    assert_eq!(one, service(directory.path(), named("one")).key().unwrap());
}

/// A metrics server answering `/ready` with `ready` and `/quicktunnel` with
/// `hostname`, for as many requests as come.
async fn metrics(ready: u16, hostname: &'static str) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut request = vec![0; 1024];
            let read = stream.read(&mut request).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&request[..read]).into_owned();
            let (status, body) = if request.starts_with("GET /ready ") {
                (ready, String::new())
            } else {
                (200, format!(r#"{{"hostname":"{hostname}"}}"#))
            };
            let _ = stream
                .write_all(
                    format!(
                        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await;
        }
    });
    port
}

fn launched_on(port: u16) -> Launch {
    Launch {
        program: "/bin/true".into(),
        args: vec![
            "tunnel".into(),
            "--metrics".into(),
            format!("127.0.0.1:{port}").into(),
        ],
        environment: vec![],
        directory: "/".into(),
    }
}

#[tokio::test]
async fn a_taken_back_quick_tunnel_is_asked_on_its_own_metrics_port() {
    let directory = tempfile::tempdir().unwrap();
    let service = service(directory.path(), CloudflareTunnel::Quick);
    let port = metrics(200, "q.trycloudflare.com").await;
    service.resume(&launched_on(port));
    assert_eq!(
        service
            .ready()
            .await
            .map(|address| address.as_str().to_owned())
            .as_deref(),
        Some("https://q.trycloudflare.com")
    );
}

#[tokio::test]
async fn a_tunnel_not_ready_reports_nothing_and_a_named_one_its_own_address() {
    let directory = tempfile::tempdir().unwrap();
    let quick = service(directory.path(), CloudflareTunnel::Quick);
    quick.resume(&launched_on(metrics(503, "q.trycloudflare.com").await));
    assert!(quick.ready().await.is_none());
    let named = service(directory.path(), named("token"));
    named.resume(&launched_on(metrics(200, "ignored.example").await));
    assert_eq!(
        named
            .ready()
            .await
            .map(|address| address.as_str().to_owned())
            .as_deref(),
        Some("https://gateway.example.com")
    );
    // A launch naming no loopback metrics port is never asked.
    let elsewhere = service(directory.path(), CloudflareTunnel::Quick);
    elsewhere.resume(&Launch {
        args: vec!["--metrics".into(), "10.0.0.1:80".into()],
        ..launched_on(0)
    });
    assert!(elsewhere.ready().await.is_none());
}
