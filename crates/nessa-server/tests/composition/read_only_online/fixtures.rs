//! Canonical gateway setup and separate client process support.
#[path = "fixtures/client.rs"]
mod client;
#[path = "fixtures/gateway.rs"]
mod gateway;
pub(super) use client::{command, Frame, WireClient};
use nessa_local_storage::OpenMode;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use uuid::Uuid;

const GATEWAY: &str =
    "composition::read_only_example::online::tests::fixtures::gateway::gateway_child";
const CLIENT: &str =
    "composition::read_only_example::online::tests::fixtures::client::client_child";
#[derive(Serialize, Deserialize)]
pub(super) struct Setup {
    pub(super) gateway: String,
    pub(super) organization: String,
    pub(super) owner: String,
    pub(super) reader: String,
    pub(super) credential: String,
    pub(super) membership: String,
    pub(super) receiver: String,
    pub(super) epoch: u64,
    pub(super) conversation: String,
    pub(super) empty: String,
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
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            if output.read_line(&mut line).unwrap() == 0 {
                let output = child.wait_with_output().unwrap();
                panic!(
                    "gateway failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            if line.trim() == "ONLINE_READY" {
                break;
            }
        }
        Self {
            child,
            output,
            control,
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
            assert_ne!(
                self.output.read_line(&mut line).unwrap(),
                0,
                "gateway ended"
            );
            if line.trim() == format!("DONE {action}") {
                return;
            }
        }
    }
}
impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
