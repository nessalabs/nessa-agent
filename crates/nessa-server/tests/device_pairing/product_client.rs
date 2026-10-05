//! A minimal authenticated `/session` client for the owner pairing methods:
//! the real WebSocket handshake, one request at a time.
use serde_json::{json, Value};
use std::net::{SocketAddr, TcpStream};
use tungstenite::{stream::MaybeTlsStream, Message, WebSocket};

pub struct ProductClient {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    next: u64,
}
impl ProductClient {
    /// Connect and authenticate with `credential`, as the CLI does.
    pub fn connect(address: SocketAddr, credential: &str) -> Self {
        let (mut socket, _) = tungstenite::connect(format!("ws://{address}/session")).unwrap();
        let challenge = read(&mut socket);
        assert_eq!(challenge["event"], "session.challenge", "{challenge}");
        let mut client = Self { socket, next: 0 };
        let ready = client.call(
            "session.authenticate",
            json!({
                "minVersion": 1,
                "maxVersion": 1,
                "nonce": challenge["payload"]["nonce"],
                "credential": credential,
                "client": {"id": "nessa-cli"},
            }),
        );
        assert_eq!(ready["ok"], true, "{ready}");
        client
    }
    /// Send one request and return its response frame.
    pub fn call(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next.to_string();
        self.socket
            .send(Message::Text(
                json!({"type": "req", "id": id, "method": method, "params": params})
                    .to_string()
                    .into(),
            ))
            .unwrap();
        loop {
            let frame = read(&mut self.socket);
            if frame["type"] == "res" && frame["id"] == id {
                return frame;
            }
        }
    }
    /// The successful payload of `method`, or a panic naming its refusal.
    pub fn ok(&mut self, method: &str, params: Value) -> Value {
        let frame = self.call(method, params);
        assert_eq!(frame["ok"], true, "{method}: {frame}");
        frame["payload"].clone()
    }
    /// The refusal code of `method`, or a panic if it succeeded.
    pub fn refused(&mut self, method: &str, params: Value) -> String {
        let frame = self.call(method, params);
        assert_eq!(frame["ok"], false, "{method} succeeded: {frame}");
        frame["error"]["code"].as_str().unwrap().to_owned()
    }
}

fn read(socket: &mut WebSocket<MaybeTlsStream<TcpStream>>) -> Value {
    loop {
        if let Message::Text(text) = socket.read().unwrap() {
            return serde_json::from_str(&text).unwrap();
        }
    }
}
