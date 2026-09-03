/// Per-connection state for one WebSocket client.
///
/// Lives for the lifetime of a single socket in `ws::handle_socket`. Separate from
/// [`AppState`](crate::app::state::AppState), which is shared across all connections.
#[derive(Debug, Default)]
pub struct WsSession {
    /// Monotonic counter for outbound **event** frames (`seq` on the wire).
    /// Each [`next_seq`] assigns the next number so clients can order server pushes.
    seq: u64,
    /// Whether this client completed a successful `connect` RPC.
    connected: bool,
    /// Nonce from the initial `connect.challenge` for this socket.
    challenge_nonce: Option<String>,
}

impl WsSession {
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate the next event sequence number and return it.
    ///
    /// Used when building `EventFrame` payloads (e.g. the initial `connect.challenge`).
    /// Response frames (`type: "res"`) do not use this — they correlate by request `id`.
    pub fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    /// Record that `connect` succeeded for this socket.
    pub fn mark_connected(&mut self) {
        self.connected = true;
    }

    /// Returns true after a successful `connect`; gates RPCs like `server.health`.
    pub fn is_connected(&self) -> bool {
        self.connected
    }

    /// Store the challenge nonce sent to this client.
    pub fn set_challenge_nonce(&mut self, nonce: String) {
        self.challenge_nonce = Some(nonce);
    }

    /// Nonce the client must echo in `connect` params.
    pub fn challenge_nonce(&self) -> Option<&str> {
        self.challenge_nonce.as_deref()
    }
}
