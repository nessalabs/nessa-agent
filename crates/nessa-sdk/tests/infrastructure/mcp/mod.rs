//! The MCP client against fixture servers. Each test is a row of the tables in
//! `docs/design/mcp-connections.md`, named after it.
//!
//! ```text
//! protocol -> McpSession -> Connection -> fixture (in process, manual clock)
//! stand_in -> McpSession::serve -> Connection -> fixture
//! forwarded -> McpSession::serve -> ForwardedResults (the grant's)
//! sessions -> McpServers::open / tool_ui / stop -> FixtureLauncher
//! process  -> McpServers -> ProcessLauncher -> fixtures/server.py
//! ```
//! Arrows show what each file drives.
mod apps;
mod fixture;
mod forwarded;
mod process;
mod protocol;
mod sessions;
mod stand_in;

use super::{McpOwner, McpServers, McpSession};
use crate::domain::agent_execution::sessions::SessionId;
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

/// The SDK session (a conversation's) test sessions belong to.
fn conversation() -> SessionId {
    SessionId::new("conversation").unwrap()
}

/// Whose test sessions are: [`conversation`], each under a grant of its own.
fn owner() -> McpOwner {
    McpOwner::new(conversation())
}

/// A session on `fixture` with `behaviour`, with the servers and launcher.
async fn session(
    behaviour: Behaviour,
) -> (
    McpSession,
    McpServers,
    Arc<FixtureLauncher>,
    Arc<ManualClock>,
) {
    // Listed, as a harness lists before it calls, unless the test holds lists back.
    let lists = !behaviour.silent.contains("tools/list");
    let (servers, launcher, clock) = servers(behaviour);
    let session = servers.open("fixture", owner()).await.unwrap();
    if lists {
        // A test whose lists fail on purpose sees that in its own list.
        let _ = session.list_tools().await;
    }
    (session, servers, launcher, clock)
}

/// A harness's end of a stand-in.
struct Harness {
    lines: Lines<BufReader<ReadHalf<DuplexStream>>>,
    write: WriteHalf<DuplexStream>,
}
impl Harness {
    /// Serve `session` to a new harness, in the background.
    fn attach(session: McpSession) -> Self {
        Self::attach_with_buffer(session, 64 * 1024)
    }
    /// As [`Self::attach`], with `bytes` of room between the stand-in and
    /// the harness: a small one makes the stand-in wait on a harness that
    /// does not read.
    fn attach_with_buffer(session: McpSession, bytes: usize) -> Self {
        let (harness, served) = tokio::io::duplex(bytes);
        let (input, output) = tokio::io::split(served);
        tokio::spawn(session.serve(input, output));
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
