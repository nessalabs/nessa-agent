//! The MCP client against fixture servers. Each test is a row of the tables in
//! `docs/design/mcp-connections.md`, named after it.
//!
//! ```text
//! protocol  -> McpServers -> Connection -> fixture (in process, manual clock)
//! stand_in  -> StandIn::serve -> Connection -> fixture
//! lifecycle -> McpServers generations -> FixtureLauncher
//! process   -> McpServers -> ProcessLauncher -> fixtures/server.py
//! ```
//! Arrows show what each file drives.
mod fixture;
mod lifecycle;
mod process;
mod protocol;
mod stand_in;

use super::{McpServers, StandIn};
use crate::infrastructure::clock::manual::ManualClock;
use fixture::{launch, Behaviour, FixtureLauncher};
use serde_json::Value;
use std::sync::Arc;
use tokio::io::{
    AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf, WriteHalf,
};

/// One server, `fixture`, with `behaviour`, on a manual clock.
fn servers(behaviour: Behaviour) -> (McpServers, Arc<FixtureLauncher>, Arc<ManualClock>) {
    let launcher = FixtureLauncher::new(behaviour);
    let clock = Arc::new(ManualClock::default());
    let servers =
        McpServers::with_launcher(vec![launch("fixture")], clock.clone(), launcher.clone())
            .unwrap();
    (servers, launcher, clock)
}

/// A harness's end of a stand-in.
struct Harness {
    lines: Lines<BufReader<ReadHalf<DuplexStream>>>,
    write: WriteHalf<DuplexStream>,
}
impl Harness {
    /// Serve `stand_in` to a new harness, in the background.
    fn attach(stand_in: StandIn) -> Self {
        let (harness, served) = tokio::io::duplex(64 * 1024);
        let (input, output) = tokio::io::split(served);
        tokio::spawn(stand_in.serve(input, output));
        let (read, write) = tokio::io::split(harness);
        Self {
            lines: BufReader::new(read).lines(),
            write,
        }
    }
    async fn send(&mut self, message: Value) {
        let mut bytes = serde_json::to_vec(&message).unwrap();
        bytes.push(b'\n');
        self.write.write_all(&bytes).await.unwrap();
    }
    async fn send_raw(&mut self, bytes: &[u8]) {
        self.write.write_all(bytes).await.unwrap();
    }
    /// The next frame, or `None` once the stand-in closed.
    async fn next(&mut self) -> Option<Value> {
        let line = tokio::time::timeout(std::time::Duration::from_secs(5), self.lines.next_line())
            .await
            .expect("the stand-in answers or closes within 5 s")
            .ok()??;
        Some(serde_json::from_str(&line).unwrap())
    }
}
