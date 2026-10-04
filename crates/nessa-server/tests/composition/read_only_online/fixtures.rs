//! Canonical gateway setup, with the real native listener and a device paired
//! through it, and separate client process support.
#[path = "fixtures/client.rs"]
mod client;
#[path = "fixtures/gateway.rs"]
mod gateway;
pub(super) use client::{command, WireClient};
use nessa_local_storage::OpenMode;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
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
    /// The paired device's issued credential id.
    pub(super) credential: String,
    pub(super) receiver: String,
    pub(super) epoch: u64,
    pub(super) conversation: String,
    pub(super) empty: String,
}
/// A profile for this gateway's paired device whose cache is `cache`: the
/// fixture's own profile with that one field, in a new private file.
pub(super) fn profile_for(root: &Path, cache: &Path) -> String {
    let mut profile: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("profile.json")).unwrap()).unwrap();
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
pub(super) struct Gateway(Child);
impl Gateway {
    pub(super) fn start(root: &Path) -> Self {
        Self::start_mode(root, "")
    }
    pub(super) fn start_mode(root: &Path, mode: &str) -> Self {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", GATEWAY, "--nocapture"])
            .env("NESSA_ONLINE_GATEWAY", root)
            .env("NESSA_ONLINE_GATE", mode)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            if lines.read_line(&mut line).unwrap() == 0 {
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
        Self(child)
    }
}
impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
