//! The actual command composition and bounded wire privilege/budget probes.
use super::super::super::super::profile::Profile;
use super::CLIENT;
use crate::composition::read_only_example;
use crate::device_pairing::infrastructure::{
    encode_frame,
    wire::{encode_request as encode_envelope, NativePairingRequest},
    EnrollmentChannel, FrameReader, MAX_PROTECTED_REQUEST_BYTES, MAX_PROTECTED_RESPONSE_BYTES,
};
use nessa_auth::adapters::pairing::{GatewayTrust, NativeIdentity, NativeTransport};
use nessa_auth::application::pairing::ClientPendingStore;
use nessa_protocol::product::generated::{
    ProductClientMetadata, SessionAuthenticateParams, PRODUCT_HANDSHAKE_METHOD, PRODUCT_VERSION,
};
use nessa_protocol::product::passive_read::encode_request;
use serde::Serialize;
use serde_json::Value;
use std::collections::VecDeque;
use std::io::{self, ErrorKind, Read, Result as IoResult, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

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
    let input = &mut io::stdin().lock();
    let result = if std::env::var_os("NESSA_ONLINE_LOSE_OUTPUT").is_some() {
        read_only_example::execute(&args, input, &mut RefusedOutput)
    } else {
        read_only_example::execute(&args, input, &mut io::stdout().lock())
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
/// A running `watch` command: its stdout lines as they arrive, each stamped
/// with the moment this process received it.
pub(crate) struct WatchChild {
    child: Child,
    lines: Receiver<(Value, Instant)>,
}
impl WatchChild {
    pub(crate) fn spawn(args: Vec<String>) -> Self {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CLIENT, "--nocapture"])
            .env(
                "NESSA_ONLINE_COMMAND",
                serde_json::to_string(&args).unwrap(),
            )
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = io::BufReader::new(child.stdout.take().unwrap());
        let (sender, lines) = channel();
        std::thread::spawn(move || {
            for line in io::BufRead::lines(stdout) {
                let Ok(line) = line else { return };
                if line.starts_with('{') {
                    let stamped = (serde_json::from_str(&line).unwrap(), Instant::now());
                    if sender.send(stamped).is_err() {
                        return;
                    }
                }
            }
        });
        Self { child, lines }
    }
    /// The next line and when it arrived, waiting at most 30 s.
    pub(crate) fn next_line(&mut self) -> (Value, Instant) {
        self.lines
            .recv_timeout(Duration::from_secs(30))
            .expect("watch line within 30 s")
    }
    /// Every line it writes until it exits, and whether it succeeded.
    pub(crate) fn finish(mut self) -> (Vec<Value>, bool) {
        let succeeded = self.child.wait().unwrap().success();
        let lines = self.lines.iter().map(|(line, _)| line).collect();
        (lines, succeeded)
    }
    /// Whether the command exited successfully, once it has ended.
    pub(crate) fn succeeded(mut self) -> bool {
        self.child.wait().unwrap().success()
    }
}
impl Drop for WatchChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
// Boundary probes use the same real paired device and generated request encoder,
// on a protected native connection of their own.
pub(crate) struct WireClient {
    transport: NativeTransport<TcpStream>,
    frames: FrameReader,
    next: u64,
    // Frames that arrived while a call waited for its own response, in order.
    held: VecDeque<Frame>,
}
/// One frame as it arrived: its decoded text with the exact encoded length, or
/// the end of the connection. A native close carries no reason.
pub(crate) enum Frame {
    Text { value: Value, bytes: usize },
    Closed,
}
impl WireClient {
    pub(crate) fn connect(root: &Path) -> Self {
        let profile = Profile::load(&root.join("profile.json")).unwrap();
        // The private state is held only while the credential is read, so the
        // example's own commands can open it afterwards.
        let saved = profile
            .private_state()
            .unwrap()
            .load_credential()
            .unwrap()
            .unwrap();
        let credential = saved.credential().as_str().to_owned();
        let (key, pin, _) = saved.into_enrollment().into_parts();
        let identity = NativeIdentity::restore(key).unwrap();
        let socket = TcpStream::connect(profile.gateway).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let transport =
            NativeTransport::connect(socket, &identity, GatewayTrust::Pinned(pin)).unwrap();
        let mut selector = EnrollmentChannel::new(transport);
        selector
            .send_envelope(&encode_envelope(&NativePairingRequest::OpenProduct).unwrap())
            .unwrap();
        let mut client = Self {
            transport: selector.into_transport(),
            frames: FrameReader::new(MAX_PROTECTED_RESPONSE_BYTES),
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
        assert_eq!(ready["ok"], true, "{ready}");
        client
    }
    fn value(&mut self) -> Value {
        match self.frame() {
            Frame::Text { value, .. } => value,
            Frame::Closed => panic!("the gateway closed the probe connection"),
        }
    }
    /// The next frame in arrival order, held ones first.
    pub(crate) fn frame(&mut self) -> Frame {
        if let Some(frame) = self.held.pop_front() {
            return frame;
        }
        self.read_frame()
    }
    fn read_frame(&mut self) -> Frame {
        loop {
            let buffer = self.frames.unfilled().unwrap();
            let count = match self.transport.read(buffer) {
                Ok(0) => return Frame::Closed,
                Ok(count) => count,
                Err(error)
                    if matches!(
                        error.kind(),
                        ErrorKind::UnexpectedEof
                            | ErrorKind::ConnectionReset
                            | ErrorKind::ConnectionAborted
                    ) =>
                {
                    return Frame::Closed
                }
                Err(error) => panic!("frame read failed: {error}"),
            };
            if let Some(body) = self.frames.filled(count).unwrap() {
                return Frame::Text {
                    value: serde_json::from_slice(&body).unwrap(),
                    bytes: body.len(),
                };
            }
        }
    }
    pub(crate) fn call(&mut self, method: &str, params: &impl Serialize) -> Value {
        self.next += 1;
        let id = self.next.to_string();
        let text = encode_request(&id, method, params).unwrap();
        let frame = encode_frame(MAX_PROTECTED_REQUEST_BYTES, text.as_bytes()).unwrap();
        self.transport.write_all(&frame).unwrap();
        self.transport.flush().unwrap();
        for _ in 0..16 {
            let frame = self.read_frame();
            match &frame {
                Frame::Text { value, .. } if value["id"] == id => return value.clone(),
                Frame::Text { .. } => self.held.push_back(frame),
                Frame::Closed => panic!("the gateway closed the probe connection during a call"),
            }
        }
        panic!("bounded fixture response capacity");
    }
}
