//! Canonical gateway setup, with the real native listener and a device paired
//! through it, and separate client process support.
#[path = "fixtures/client.rs"]
mod client;
#[path = "fixtures/gateway.rs"]
mod gateway;
#[path = "fixtures/semantic.rs"]
mod semantic;
pub(super) use client::{command, Frame, WatchChild, WireClient};
use nessa_local_storage::OpenMode;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use uuid::Uuid;

const GATEWAY: &str = "composition::read_only_online_tests::fixtures::gateway::gateway_child";
const CLIENT: &str = "composition::read_only_online_tests::fixtures::client::client_child";
#[derive(Serialize, Deserialize)]
pub(super) struct Setup {
    pub(super) gateway: String,
    pub(super) organization: String,
    pub(super) owner: String,
    /// The paired device's issued credential id.
    pub(super) credential: String,
    pub(super) receiver: String,
    pub(super) epoch: u64,
    pub(super) conversation: String,
    pub(super) empty: String,
    /// The second paired device's receiver; `live` mode only.
    pub(super) second_receiver: Option<String>,
    /// The second paired device's issued credential; `live` mode only.
    pub(super) second_credential: Option<String>,
}
/// A profile for this gateway's paired device whose cache is `cache`: the
/// fixture's own profile with that one field, in a new private file.
pub(super) fn profile_for(root: &Path, cache: &Path) -> String {
    profile_for_device(root, "profile.json", cache)
}
/// The same for the device whose fixture profile is `device` under `root`
/// (`profile-b.json` is live mode's second device).
pub(super) fn profile_for_device(root: &Path, device: &str, cache: &Path) -> String {
    let mut profile: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join(device)).unwrap()).unwrap();
    profile["cache"] = serde_json::json!(cache);
    let path = root.join(format!("profile-{}.json", uuid()));
    private_write(&path, &serde_json::to_vec(&profile).unwrap());
    path.to_string_lossy().into_owned()
}
pub(super) fn uuid() -> String {
    Uuid::new_v4().to_string()
}
pub(super) fn private_write(path: &Path, bytes: &[u8]) {
    nessa_local_storage::open(path, OpenMode::CreateNew)
        .unwrap()
        .write_all(bytes)
        .unwrap();
}
pub(super) struct Gateway {
    child: Child,
    // Retained so a live gateway can answer later control lines; the other
    // modes print nothing after readiness.
    output: BufReader<ChildStdout>,
    control: Option<ChildStdin>,
    stderr: StderrTail,
}
impl Gateway {
    pub(super) fn start(root: &Path) -> Self {
        Self::start_mode(root, "")
    }
    pub(super) fn start_mode(root: &Path, mode: &str) -> Self {
        Self::spawn(root, mode, Stdio::null())
    }
    /// A gateway that also commits and revokes on request, from inside its own
    /// process, so its process-local watch producer sees the change.
    pub(super) fn start_live(root: &Path) -> Self {
        Self::spawn(root, "live", Stdio::piped())
    }
    /// Actual Agent-produced semantic prefix and controlled terminal suffix.
    pub(super) fn start_semantic(root: &Path) -> Self {
        Self::spawn(root, "semantic", Stdio::piped())
    }
    fn spawn(root: &Path, mode: &str, input: Stdio) -> Self {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", GATEWAY, "--nocapture"])
            .env("NESSA_ONLINE_GATEWAY", root)
            .env("NESSA_ONLINE_GATE", mode)
            .stdin(input)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let control = child.stdin.take();
        let mut stderr = StderrTail::spawn(child.stderr.take().unwrap());
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            if output.read_line(&mut line).unwrap() == 0 {
                let status = child.wait().unwrap();
                panic!("gateway failed ({status}): {}", stderr.finish());
            }
            if line.trim() == "ONLINE_READY" {
                break;
            }
        }
        Self {
            child,
            output,
            control,
            stderr,
        }
    }
    /// Ask a live gateway to act and wait until it reports the effect done.
    pub(super) fn act(&mut self, action: &str) {
        let control = self.control.as_mut().expect("live gateway control");
        writeln!(control, "{action}").unwrap();
        control.flush().unwrap();
        let mut line = String::new();
        loop {
            line.clear();
            if self.output.read_line(&mut line).unwrap() == 0 {
                let status = self.child.wait().unwrap();
                panic!(
                    "gateway ended during {action} ({status}): {}",
                    self.stderr.finish()
                );
            }
            if line.trim() == format!("DONE {action}") {
                return;
            }
        }
    }
}

/// The child's stderr, kept to a tail so a full pipe cannot stall it and a
/// death still names the panic that closed stdout.
struct StderrTail {
    text: Arc<Mutex<String>>,
    reader: Option<JoinHandle<()>>,
}
impl StderrTail {
    fn spawn(stderr: ChildStderr) -> Self {
        let text = Arc::new(Mutex::new(String::new()));
        let shared = text.clone();
        let reader = std::thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let mut tail = shared.lock().unwrap();
                        tail.push_str(&line);
                        const LIMIT: usize = 8 * 1024;
                        if tail.len() > LIMIT {
                            let excess = tail.len() - LIMIT;
                            tail.drain(..excess);
                        }
                    }
                }
            }
        });
        Self {
            text,
            reader: Some(reader),
        }
    }

    fn finish(&mut self) -> String {
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        self.text.lock().unwrap().clone()
    }
}
impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = self.stderr.finish();
    }
}
