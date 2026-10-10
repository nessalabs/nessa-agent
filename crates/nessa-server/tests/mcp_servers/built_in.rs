//! A server the gateway serves itself, reached through the relay as a
//! configured server's stand-in reaches its server: admitted by its digest
//! and the conversation's grant, answering MCP, and stopping its calls when
//! a call is cancelled, its stand-in goes, or its grant is revoked.
use super::super::{
    built_in_digest, read_line, write_line, Answer, ConversationGrants, Hello, OsTokens, Refusal,
    Relay,
};
use super::MAX_BUILT_IN_LINE_BYTES;
use crate::mcp_servers::application::{BuiltInFuture, BuiltInServer};
use crate::mcp_servers::domain::ConfigurationKey;
use nessa_sdk::domain::agent_execution::sessions::SessionId;
use nessa_sdk::infrastructure::acp::sessions::StandInGrants;
use nessa_sdk::infrastructure::{clock::RuntimeClock, mcp::McpServers};
use serde_json::{json, Value};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::{AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};
use tokio::sync::watch;

/// `echo` answers its arguments; `wait` answers once stopped. Says which
/// session each call was for, and which calls were stopped.
#[derive(Default)]
struct Fake {
    sessions: Mutex<Vec<String>>,
    stopped: Arc<Mutex<Vec<String>>>,
    revision: Mutex<String>,
}
impl BuiltInServer for Fake {
    fn name(&self) -> &str {
        "built-in"
    }
    fn configuration(&self) -> Vec<String> {
        vec![self.revision.lock().unwrap().clone()]
    }
    fn tools(&self) -> Vec<Value> {
        vec![json!({"name": "echo", "inputSchema": {"type": "object"}})]
    }
    fn call(
        &self,
        session: &SessionId,
        call: String,
        tool: String,
        arguments: Value,
        mut stop: watch::Receiver<bool>,
    ) -> BuiltInFuture {
        self.sessions
            .lock()
            .unwrap()
            .push(session.as_str().to_owned());
        let stopped = self.stopped.clone();
        Box::pin(async move {
            if tool == "wait" {
                let _ = stop.wait_for(|stop| *stop).await;
                stopped
                    .lock()
                    .unwrap()
                    .push(arguments["name"].as_str().unwrap().into());
                return json!({"content": [], "isError": true});
            }
            json!({"content": [{"type": "text", "text": arguments.to_string()}], "call": call})
        })
    }
}

fn key() -> ConfigurationKey {
    ConfigurationKey::new([3; 32])
}

struct Opened {
    fake: Arc<Fake>,
    grants: ConversationGrants,
    grant: Option<nessa_sdk::infrastructure::acp::sessions::StandInGrant>,
    token: String,
    relay: Arc<Relay>,
}

fn opened() -> Opened {
    let servers = McpServers::new(Vec::new(), Arc::new(RuntimeClock::new())).unwrap();
    let grants = ConversationGrants::new(servers.clone(), Arc::new(OsTokens));
    let grant = grants.grant(&SessionId::new("conversation").unwrap());
    let token = grant.environment()[0].1.clone();
    let fake = Arc::new(Fake::default());
    let relay = Arc::new(Relay::new(servers, grants.clone(), key()).with_built_in(fake.clone()));
    Opened {
        fake,
        grants,
        grant: Some(grant),
        token,
        relay,
    }
}

struct Client {
    from: BufReader<ReadHalf<DuplexStream>>,
    to: WriteHalf<DuplexStream>,
    served: tokio::task::JoinHandle<()>,
}

impl Opened {
    /// A stand-in's connection, its hello said with `configuration` and
    /// `token`, and the relay's answer.
    async fn connect(&self, configuration: &str, token: &str) -> (Client, Answer) {
        let (client, server) = tokio::io::duplex(1 << 20);
        let relay = self.relay.clone();
        let served = tokio::spawn(async move { relay.serve(server).await });
        let (from, mut to) = tokio::io::split(client);
        let mut from = BufReader::new(from);
        let hello = Hello {
            server: "built-in".into(),
            configuration: configuration.into(),
            session: token.into(),
        };
        write_line(&mut to, &hello).await.unwrap();
        let answer = read_line::<Answer>(&mut from).await.unwrap();
        (Client { from, to, served }, answer)
    }

    async fn admitted(&self) -> Client {
        let digest = built_in_digest(&key(), self.fake.as_ref());
        let (client, answer) = self.connect(&digest, &self.token).await;
        assert_eq!(answer, Answer::Accepted);
        client
    }
}

impl Client {
    async fn send(&mut self, message: Value) {
        write_line(&mut self.to, &message).await.unwrap();
    }
    async fn next(&mut self) -> Option<Value> {
        tokio::time::timeout(Duration::from_secs(5), read_line::<Value>(&mut self.from))
            .await
            .expect("an answer in time")
    }
    async fn ask(&mut self, message: Value) -> Value {
        self.send(message).await;
        self.next().await.expect("an answer")
    }
}

async fn stopped(fake: &Fake, names: &[&str]) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let mut stopped = fake.stopped.lock().unwrap().clone();
            stopped.sort();
            if stopped == names {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the calls are stopped");
}

#[tokio::test]
async fn a_built_in_server_answers_mcp_for_the_conversation_its_grant_names() {
    let opened = opened();
    let mut client = opened.admitted().await;
    let initialized = client
        .ask(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26"}}))
        .await;
    assert_eq!(initialized["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(initialized["result"]["serverInfo"]["name"], "built-in");
    assert!(initialized["result"]["capabilities"]["tools"].is_object());
    let listed = client
        .ask(json!({"jsonrpc": "2.0", "id": "list", "method": "tools/list"}))
        .await;
    assert_eq!(listed["id"], "list");
    assert_eq!(listed["result"]["tools"][0]["name"], "echo");
    let called = client
        .ask(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "echo", "arguments": {"say": "hi"}}}))
        .await;
    assert_eq!(called["id"], 2);
    assert_eq!(called["result"]["content"][0]["text"], r#"{"say":"hi"}"#);
    assert_eq!(*opened.fake.sessions.lock().unwrap(), ["conversation"]);
    let unknown = client
        .ask(json!({"jsonrpc": "2.0", "id": 3, "method": "resources/list"}))
        .await;
    assert_eq!(unknown["error"]["code"], -32601);
    let garbled = {
        client.to.write_all(b"not json\n").await.unwrap();
        client.next().await.unwrap()
    };
    assert_eq!(garbled["error"]["code"], -32700);
    let batch = client
        .ask(json!([{"jsonrpc": "2.0", "id": 4, "method": "ping"}]))
        .await;
    assert_eq!(batch["error"]["code"], -32600);
}

#[tokio::test]
async fn a_cancelled_call_is_stopped_and_not_answered() {
    let opened = opened();
    let mut client = opened.admitted().await;
    client
        .send(json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": "wait", "arguments": {"name": "seven"}}}))
        .await;
    client
        .send(json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 7}}))
        .await;
    stopped(&opened.fake, &["seven"]).await;
    // The next answer is the ping's: the cancelled call has none.
    let pong = client
        .ask(json!({"jsonrpc": "2.0", "id": 8, "method": "ping"}))
        .await;
    assert_eq!(pong["id"], 8);
}

#[tokio::test]
async fn a_stand_in_gone_or_a_grant_revoked_stops_every_call_and_ends_the_connection() {
    let mut opened = opened();
    let mut gone = opened.admitted().await;
    gone.send(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "wait", "arguments": {"name": "gone"}}}))
        .await;
    let _ = gone
        .ask(json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}))
        .await;
    // The harness closing its input: the stand-in passes that on.
    gone.to.shutdown().await.unwrap();
    stopped(&opened.fake, &["gone"]).await;

    let mut revoked = opened.admitted().await;
    revoked
        .send(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "wait", "arguments": {"name": "revoked"}}}))
        .await;
    let _ = revoked
        .ask(json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}))
        .await;
    drop(opened.grant.take());
    stopped(&opened.fake, &["gone", "revoked"]).await;
    assert_eq!(revoked.next().await, None, "the connection ends");
    tokio::time::timeout(Duration::from_secs(5), revoked.served)
        .await
        .expect("served to its end")
        .unwrap();
    // A revoked grant admits nothing more.
    let digest = built_in_digest(&key(), opened.fake.as_ref());
    let (_, answer) = opened.connect(&digest, &opened.token).await;
    assert!(matches!(
        answer,
        Answer::Refused {
            reason: Refusal::UnknownSession,
            ..
        }
    ));
    let _ = opened.grants;
}

#[tokio::test]
async fn a_stand_in_handed_out_for_another_configuration_or_line_too_long_is_turned_away() {
    let opened = opened();
    let stale = built_in_digest(&key(), opened.fake.as_ref());
    *opened.fake.revision.lock().unwrap() = "next".into();
    let (_, answer) = opened.connect(&stale, &opened.token).await;
    assert!(matches!(
        answer,
        Answer::Refused {
            reason: Refusal::ConfigurationChanged,
            ..
        }
    ));
    let (_, answer) = opened
        .connect(
            &built_in_digest(&key(), opened.fake.as_ref()),
            "no-such-token",
        )
        .await;
    assert!(matches!(
        answer,
        Answer::Refused {
            reason: Refusal::UnknownSession,
            ..
        }
    ));

    let mut client = opened.admitted().await;
    client
        .to
        .write_all(&vec![b' '; MAX_BUILT_IN_LINE_BYTES + 1])
        .await
        .unwrap();
    assert_eq!(client.next().await, None, "the connection ends");
}
