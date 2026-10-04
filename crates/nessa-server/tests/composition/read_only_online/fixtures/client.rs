//! The actual command composition and bounded wire privilege/budget probes.
use super::super::super::super::profile::Profile;
use super::CLIENT;
use crate::composition::read_only_example;
use crate::product::generated::{
    ProductClientMetadata, SessionAuthenticateParams, MAX_AUTH_CREDENTIAL_CHARACTERS,
    PRODUCT_HANDSHAKE_METHOD, PRODUCT_VERSION,
};
use crate::product::passive_read::wire::encode_request;
use serde::Serialize;
use serde_json::Value;
use std::collections::VecDeque;
use std::io::{self, ErrorKind, Result as IoResult, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use tungstenite::protocol::CloseFrame;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

struct RefusedOutput;
impl Write for RefusedOutput {
    fn write(&mut self, _: &[u8]) -> IoResult<usize> {
        Err(ErrorKind::BrokenPipe.into())
    }
    fn flush(&mut self) -> IoResult<()> {
        Ok(())
    }
}
#[test]
fn client_child() {
    let Ok(args) = std::env::var("NESSA_ONLINE_COMMAND") else {
        return;
    };
    let args: Vec<String> = serde_json::from_str(&args).unwrap();
    let result = if std::env::var_os("NESSA_ONLINE_LOSE_OUTPUT").is_some() {
        read_only_example::execute(&args, &mut RefusedOutput)
    } else {
        read_only_example::execute(&args, &mut io::stdout().lock())
    };
    if let Err(error) = &result {
        eprintln!("command failure: {error:?}");
    }
    std::process::exit(if result.is_ok() { 0 } else { 1 });
}
pub(crate) fn command(args: Vec<String>, lose: bool) -> (bool, Option<Value>) {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args(["--exact", CLIENT, "--nocapture"]).env(
        "NESSA_ONLINE_COMMAND",
        serde_json::to_string(&args).unwrap(),
    );
    if lose {
        command.env("NESSA_ONLINE_LOSE_OUTPUT", "1");
    }
    let output = command.output().unwrap();
    let stdout = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .find(|line| line.starts_with('{'))
        .map(|line| serde_json::from_str(line).unwrap());
    if stdout.is_none() {
        eprintln!("client stderr: {}", String::from_utf8_lossy(&output.stderr));
    }
    (output.status.success(), stdout)
}
// Boundary probes use the same real paired credential and generated request encoder.
pub(crate) struct WireClient {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    next: u64,
    // Frames that arrived while a call waited for its own response, in order.
    held: VecDeque<Frame>,
}
/// One frame as it arrived: its decoded text with the exact encoded length, or
/// the peer's close.
pub(crate) enum Frame {
    Text { value: Value, bytes: usize },
    Closed(Option<CloseFrame>),
}
impl WireClient {
    pub(crate) fn connect(root: &Path) -> Self {
        let profile = Profile::load(&root.join("profile.json")).unwrap();
        let endpoint = profile.endpoint().unwrap();
        let credential = profile.credential(MAX_AUTH_CREDENTIAL_CHARACTERS).unwrap();
        let (socket, _) =
            tungstenite::connect(format!("{}/session", endpoint.web_socket_url())).unwrap();
        if let MaybeTlsStream::Plain(stream) = socket.get_ref() {
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
        }
        let mut client = Self {
            socket,
            next: 0,
            held: VecDeque::new(),
        };
        let challenge = client.value();
        let params = SessionAuthenticateParams {
            min_version: PRODUCT_VERSION,
            max_version: PRODUCT_VERSION,
            nonce: challenge["payload"]["nonce"].as_str().unwrap().into(),
            credential,
            client: ProductClientMetadata {
                id: "boundary-probe".into(),
            },
        };
        let ready = client.call(PRODUCT_HANDSHAKE_METHOD, &params);
        assert_eq!(ready["ok"], true);
        client
    }
    fn value(&mut self) -> Value {
        match self.frame() {
            Frame::Text { value, .. } => value,
            Frame::Closed(close) => panic!("peer closed: {close:?}"),
        }
    }
    /// The next frame in arrival order, held ones first.
    pub(crate) fn frame(&mut self) -> Frame {
        if let Some(frame) = self.held.pop_front() {
            return frame;
        }
        for _ in 0..16 {
            match self.socket.read() {
                Ok(Message::Text(text)) => {
                    return Frame::Text {
                        value: serde_json::from_str(&text).unwrap(),
                        bytes: text.len(),
                    }
                }
                Ok(Message::Ping(value)) => self.socket.send(Message::Pong(value)).unwrap(),
                Ok(Message::Close(close)) => return Frame::Closed(close),
                Ok(_) => {}
                Err(error) => panic!("frame read failed: {error}"),
            }
        }
        panic!("bounded fixture frame capacity");
    }
    pub(crate) fn call(&mut self, method: &str, params: &impl Serialize) -> Value {
        self.next += 1;
        let id = self.next.to_string();
        let text = encode_request(&id, method, params).unwrap();
        self.socket.send(Message::Text(text.into())).unwrap();
        for _ in 0..16 {
            let frame = match self.socket.read().unwrap() {
                Message::Text(text) => Frame::Text {
                    value: serde_json::from_str(&text).unwrap(),
                    bytes: text.len(),
                },
                Message::Ping(value) => {
                    self.socket.send(Message::Pong(value)).unwrap();
                    continue;
                }
                Message::Close(close) => panic!("peer closed during call: {close:?}"),
                _ => continue,
            };
            if let Frame::Text { value, .. } = &frame {
                if value["id"] == id {
                    return value.clone();
                }
            }
            self.held.push_back(frame);
        }
        panic!("bounded fixture response capacity");
    }
}
