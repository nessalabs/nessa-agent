//! The real `nessa env serve` binary over pipes, as `ssh` would run it: a
//! hello naming this build's lease protocol, a lease admitted, a harness started from the
//! host's own configuration, its bytes carried both ways, stopped and ended
//! with cleanup evidence, and every step in the host's ledger.
#![cfg(unix)]

use nessa_protocol::lease::{
    decode, encode, Cleanup, Data, FromEnvironment, GrantRefusal, ToEnvironment, Unavailability,
};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

const LEASE: &str = "lease-from-the-gateway";

/// The data directory a host's `nessa` reads, with `config.json` naming
/// `harness` as Claude's command.
fn host(root: &Path, harness: &str) {
    let namespace = root.join("ci");
    nessa_local_storage::create_directory(&namespace.join("auth")).unwrap();
    let workspace = root.join("workspace");
    nessa_local_storage::create_directory(&workspace).unwrap();
    let config = serde_json::json!({
        "agents": {
            "catalog": concat!(env!("CARGO_MANIFEST_DIR"), "/../nessa-sdk/data/models.json"),
            "workspace": workspace,
            "selected": "claude",
            "runtimes": {
                "claude": {
                    "command": harness,
                    "args": [],
                    "model": "claude-sonnet-5",
                    "toolsEnabled": false
                }
            }
        }
    });
    let mut file = nessa_local_storage::open(
        &namespace.join("config.json"),
        nessa_local_storage::OpenMode::CreateNew,
    )
    .unwrap();
    file.write_all(config.to_string().as_bytes()).unwrap();
    file.sync_all().unwrap();
}

struct Serving {
    child: Child,
    input: Option<ChildStdin>,
    frames: mpsc::Receiver<FromEnvironment>,
}

fn serve(root: &Path) -> Serving {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nessa"));
    command.args(["env", "serve"]).env_clear();
    serve_by(command, root)
}

fn serve_by(mut command: Command, root: &Path) -> Serving {
    let mut child = command
        .env("NESSA_DATA_DIR", root)
        .env("NESSA_STAGE", "ci")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let input = child.stdin.take();
    let output = child.stdout.take().unwrap();
    let (sender, frames) = mpsc::channel();
    std::thread::spawn(move || read_frames(output, sender));
    Serving {
        child,
        input,
        frames,
    }
}

fn read_frames(mut output: ChildStdout, sender: mpsc::Sender<FromEnvironment>) {
    loop {
        let mut length = [0; 4];
        if output.read_exact(&mut length).is_err() {
            return;
        }
        let mut body = vec![0; u32::from_be_bytes(length) as usize];
        if output.read_exact(&mut body).is_err() {
            return;
        }
        let frame = decode(&body).expect("the environment sends only lease frames");
        if sender.send(frame).is_err() {
            return;
        }
    }
}

impl Serving {
    fn send(&mut self, frame: ToEnvironment) {
        let input = self.input.as_mut().unwrap();
        input.write_all(&encode(&frame).unwrap()).unwrap();
        input.flush().unwrap();
    }

    fn next(&self) -> FromEnvironment {
        self.frames
            .recv_timeout(Duration::from_secs(20))
            .expect("the environment answers")
    }

    /// The gateway's stream ends; the environment ends what it held and exits.
    fn finish(mut self) {
        drop(self.input.take());
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "nessa env serve exits once its stream ends"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(status.success(), "{status:?}");
    }
}

impl Drop for Serving {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn ledger(root: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(root.join("ci").join("environment").join("leases.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn a_lease_runs_its_harness_on_the_host_and_ends_with_evidence_in_the_ledger() {
    let root = tempfile::tempdir().unwrap();
    // `cat` stands in for the harness: what the gateway writes comes back.
    host(root.path(), "/bin/cat");
    let mut serving = serve(root.path());
    match serving.next() {
        FromEnvironment::Hello {
            build,
            protocol,
            workspace,
        } => {
            assert_eq!(build, env!("CARGO_PKG_VERSION"));
            assert_eq!(protocol, nessa_server::env::LEASE_PROTOCOL);
            assert_eq!(
                Path::new(&workspace),
                root.path().join("workspace").as_path()
            );
        }
        other => panic!("the hello comes first: {other:?}"),
    }
    serving.send(ToEnvironment::Grant {
        lease: LEASE.into(),
        agent: "claude".into(),
    });
    assert_eq!(
        serving.next(),
        FromEnvironment::Granted {
            lease: LEASE.into()
        }
    );
    // A second grant of the same lease is refused: one live lease per id.
    serving.send(ToEnvironment::Grant {
        lease: LEASE.into(),
        agent: "claude".into(),
    });
    assert_eq!(
        serving.next(),
        FromEnvironment::Refused {
            lease: LEASE.into(),
            reason: GrantRefusal::Duplicate
        }
    );
    serving.send(ToEnvironment::Start {
        lease: LEASE.into(),
        channel: 1,
        environment: BTreeMap::new(),
    });
    serving.send(ToEnvironment::Input {
        lease: LEASE.into(),
        channel: 1,
        data: Data(b"ping\n".to_vec()),
    });
    let mut echoed = Vec::new();
    while echoed != b"ping\n" {
        match serving.next() {
            FromEnvironment::Output {
                lease,
                channel: 1,
                data,
            } if lease == LEASE => echoed.extend(data.0),
            other => panic!("only the harness's output: {other:?}"),
        }
    }
    serving.send(ToEnvironment::Stop {
        lease: LEASE.into(),
        channel: 1,
        grace_ms: 2_000,
        kill_ms: 5_000,
    });
    let stopped = loop {
        match serving.next() {
            FromEnvironment::OutputClosed { .. } => continue,
            other => break other,
        }
    };
    assert!(
        matches!(
            stopped,
            FromEnvironment::Stopped {
                channel: 1,
                cleanup: Cleanup::Confirmed { .. },
                ..
            }
        ),
        "{stopped:?}"
    );
    serving.send(ToEnvironment::End {
        lease: LEASE.into(),
    });
    assert!(matches!(
        serving.next(),
        FromEnvironment::Ended {
            cleanup: Cleanup::Confirmed { .. },
            ..
        }
    ));
    serving.finish();
    let kinds: Vec<_> = ledger(root.path())
        .iter()
        .map(|entry| entry["kind"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(kinds.first().map(String::as_str), Some("granted"));
    assert!(kinds.iter().any(|kind| kind == "ended"), "{kinds:?}");
}

#[test]
fn a_lease_held_when_the_gateways_stream_ends_is_ended_as_lost_and_accounted_after() {
    let root = tempfile::tempdir().unwrap();
    host(root.path(), "/bin/cat");
    let mut serving = serve(root.path());
    assert!(matches!(serving.next(), FromEnvironment::Hello { .. }));
    serving.send(ToEnvironment::Grant {
        lease: LEASE.into(),
        agent: "claude".into(),
    });
    assert!(matches!(serving.next(), FromEnvironment::Granted { .. }));
    serving.send(ToEnvironment::Start {
        lease: LEASE.into(),
        channel: 1,
        environment: BTreeMap::new(),
    });
    serving.finish();
    assert!(ledger(root.path())
        .iter()
        .any(|entry| entry["kind"] == "ended" && entry["lost"] == true));
    // The next connection asks what became of it.
    let mut serving = serve(root.path());
    assert!(matches!(serving.next(), FromEnvironment::Hello { .. }));
    serving.send(ToEnvironment::Account {
        lease: LEASE.into(),
    });
    assert!(
        matches!(
            serving.next(),
            FromEnvironment::Accounted {
                cleanup: Cleanup::Confirmed { .. },
                ..
            }
        ),
        "the host recorded stopping it"
    );
    serving.finish();
}

#[test]
fn a_host_with_no_agents_configured_says_so_after_its_hello() {
    let root = tempfile::tempdir().unwrap();
    nessa_local_storage::create_directory(&root.path().join("ci").join("auth")).unwrap();
    let serving = serve(root.path());
    assert!(matches!(
        serving.next(),
        FromEnvironment::Hello { protocol, .. } if protocol == nessa_server::env::LEASE_PROTOCOL
    ));
    assert_eq!(
        serving.next(),
        FromEnvironment::Unavailable {
            reason: Unavailability::NotConfigured
        }
    );
    serving.finish();
}

/// Run `command` as `sshd` hands it to a login shell, in `home`.
fn on_host(home: &Path, command: &str) -> Command {
    let mut shell = Command::new("sh");
    shell
        .arg("-c")
        .arg(command)
        .env_clear()
        .env("HOME", home)
        .env("PATH", std::env::var_os("PATH").unwrap());
    shell
}

/// First use (#703), with this real binary and this machine's own `sh` and
/// tools — Linux and macOS each in CI: a home without it probes absent on a
/// platform this build runs on; the binary sent with its digest installs;
/// the probe then finds it; and the gateway's serve command starts exactly
/// that copy, whose hello names this build's lease protocol.
#[test]
fn this_build_installs_on_a_host_without_it_and_then_serves_there() {
    use nessa_server::env::LEASE_PROTOCOL;
    use nessa_server::env_serve::install::{
        probe_command, serve_command, upload_command, Platform, Probe, Upload,
    };
    use sha2::{Digest, Sha256};
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    host(root.path(), "/bin/cat");
    let probe = |home: &Path| {
        let output = on_host(home, &probe_command(LEASE_PROTOCOL))
            .output()
            .unwrap();
        Probe::parse(&String::from_utf8(output.stdout).unwrap()).unwrap()
    };
    match probe(home.path()) {
        Probe::Absent(platform) => assert!(Platform::this_build().runs_on(&platform), "{platform}"),
        Probe::Present => panic!("nothing is installed in an empty home"),
    }
    let binary = Path::new(env!("CARGO_BIN_EXE_nessa"));
    let digest = format!("{:x}", Sha256::digest(std::fs::read(binary).unwrap()));
    let output = on_host(home.path(), &upload_command(LEASE_PROTOCOL, &digest))
        .stdin(std::fs::File::open(binary).unwrap())
        .output()
        .unwrap();
    let answer = String::from_utf8(output.stdout).unwrap();
    assert_eq!(Upload::parse(&answer), Some(Upload::Installed), "{answer}");
    assert_eq!(probe(home.path()), Probe::Present);
    let serving = serve_by(
        on_host(home.path(), &serve_command(LEASE_PROTOCOL)),
        root.path(),
    );
    match serving.next() {
        FromEnvironment::Hello { protocol, .. } => assert_eq!(protocol, LEASE_PROTOCOL),
        other => panic!("the hello comes first: {other:?}"),
    }
    serving.finish();
}

/// The binary says the lease protocol it speaks, which is what an install
/// checks a copy against before putting it in place.
#[test]
fn env_protocol_prints_this_builds_lease_protocol() {
    let output = Command::new(env!("CARGO_BIN_EXE_nessa"))
        .args(["env", "protocol"])
        .env_clear()
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("{}\n", nessa_server::env::LEASE_PROTOCOL)
    );
}
