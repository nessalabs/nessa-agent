use crate::{
    gateway_endpoint::application::GatewayEndpointAccess,
    surface_credential::SurfaceCredentials,
    vault::application::{
        contract::{VAULT_ERROR_CODES, VAULT_PASSPHRASE_CHANGE_STATES, VAULT_STATES},
        Command, Failure, VaultConnection,
    },
};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
    process::{Child, ChildStdin, ChildStdout, Command as ProcessCommand, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use zeroize::Zeroizing;
struct Pipes {
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    child: Arc<Mutex<Child>>,
}
impl Drop for Pipes {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
/// One retained child belongs to the native application, not a webview's render.
pub struct NativeVault {
    binary: PathBuf,
    endpoint: Arc<GatewayEndpointAccess>,
    credential: Arc<dyn SurfaceCredentials>,
    stage: String,
    port: u16,
    pipes: Mutex<Option<Pipes>>,
    active: Mutex<Option<(u64, Arc<Mutex<Child>>)>>,
    closed: AtomicBool,
    next_request: AtomicU64,
    cancelled_through: AtomicU64,
}
impl NativeVault {
    pub fn new(
        binary: PathBuf,
        endpoint: Arc<GatewayEndpointAccess>,
        credential: Arc<dyn SurfaceCredentials>,
        stage: String,
        port: u16,
    ) -> Self {
        Self {
            binary,
            endpoint,
            credential,
            stage,
            port,
            pipes: Mutex::new(None),
            active: Mutex::new(None),
            closed: AtomicBool::new(false),
            next_request: AtomicU64::new(1),
            cancelled_through: AtomicU64::new(0),
        }
    }
    fn start(&self, request_id: u64) -> Result<Pipes, Failure> {
        if self.closed.load(Ordering::SeqCst) || !self.binary.is_absolute() {
            return Err(Failure::VaultUnavailable);
        }
        let url = self
            .endpoint
            .resolve(&self.stage)
            .map_err(|_| Failure::VaultUnavailable)?
            .unwrap_or_else(|| format!("ws://127.0.0.1:{}", self.port));
        self.endpoint
            .permits_credential_for(&self.stage, &url)
            .map_err(|_| Failure::VaultUnavailable)?;
        let credential = Zeroizing::new(
            self.credential
                .read(&self.stage)
                .map_err(|_| Failure::VaultUnavailable)?,
        );
        let address = url
            .strip_prefix("ws://")
            .ok_or(Failure::VaultUnavailable)?
            .trim_end_matches("/session");
        let address: std::net::SocketAddr =
            address.parse().map_err(|_| Failure::VaultUnavailable)?;
        let mut child = ProcessCommand::new(&self.binary)
            .args(["vault", "session"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| Failure::VaultUnavailable)?;
        let input = child.stdin.take().ok_or(Failure::VaultUnavailable)?;
        let output = child.stdout.take().ok_or(Failure::VaultUnavailable)?;
        let child = Arc::new(Mutex::new(child));
        let mut pipes = Pipes {
            input,
            output: BufReader::new(output),
            child: child.clone(),
        };
        {
            let mut active = self.active.lock().map_err(|_| Failure::VaultUnavailable)?;
            *active = Some((request_id, child));
            if self.closed.load(Ordering::SeqCst)
                || self.cancelled_through.load(Ordering::SeqCst) >= request_id
            {
                return Err(Failure::VaultUnavailable);
            }
        }
        let init = Zeroizing::new(
            serde_json::to_vec(&Initialization {
                address: address.to_string(),
                credential: credential.as_str(),
            })
            .map_err(|_| Failure::VaultUnavailable)?,
        );
        send(&mut pipes.input, &init)?;
        let ready: Ready = serde_json::from_slice(&read_line(&mut pipes.output)?)
            .map_err(|_| Failure::VaultUnavailable)?;
        if !ready.ready {
            return Err(Failure::VaultUnavailable);
        }
        Ok(pipes)
    }
}
impl VaultConnection for NativeVault {
    fn next_request(&self) -> u64 {
        self.next_request.fetch_add(1, Ordering::SeqCst)
    }
    fn command(&self, request: Command, request_id: u64) -> Result<serde_json::Value, Failure> {
        if request
            .passphrase
            .as_ref()
            .is_some_and(|value| value.len() > 16384)
        {
            return Err(Failure::VaultUnavailable);
        }
        // Reject overlap instead of accumulating passphrases or blocking worker queues.
        let mut slot = self.pipes.try_lock().map_err(|_| Failure::VaultBusy)?;
        if self.closed.load(Ordering::SeqCst) {
            return Err(Failure::VaultUnavailable);
        }
        if slot.is_none() {
            *slot = Some(self.start(request_id)?);
        }
        if self.cancelled_through.load(Ordering::SeqCst) >= request_id {
            return Err(Failure::VaultUnavailable);
        }
        if let Ok(mut active) = self.active.lock() {
            if let Some((id, child)) = active.as_mut() {
                *id = request_id;
                if self.cancelled_through.load(Ordering::SeqCst) >= request_id {
                    if let Ok(mut child) = child.lock() {
                        let _ = child.kill();
                    }
                    return Err(Failure::VaultUnavailable);
                }
            }
        }
        let operation = request.operation();
        let unconfirmed = request.unconfirmed_failure();
        let passphrase = request.passphrase;
        let bytes = Zeroizing::new(
            serde_json::to_vec(&WireCommand {
                operation,
                passphrase: passphrase.as_deref().map(|value| value.as_str()),
            })
            .map_err(|_| Failure::VaultUnavailable)?,
        );
        if bytes.len() > 32768 {
            return Err(Failure::VaultUnavailable);
        }
        let pipes = slot.as_mut().ok_or(Failure::VaultUnavailable)?;
        // Once any command bytes may have crossed the pipe, an I/O failure
        // cannot establish that a passphrase change left storage untouched.
        let mut result = exchange(pipes, &bytes, operation).map_err(|_| unconfirmed);
        if connection_unusable(&result) {
            *slot = None;
            // Status is a projection. Secret-bearing commands never replay after
            // an uncertain delivery; their caller checks status explicitly.
            if operation == "status"
                && !self.closed.load(Ordering::SeqCst)
                && self.cancelled_through.load(Ordering::SeqCst) < request_id
            {
                *slot = Some(self.start(request_id)?);
                result = exchange(
                    slot.as_mut().ok_or(Failure::VaultUnavailable)?,
                    &bytes,
                    operation,
                );
                if connection_unusable(&result) {
                    *slot = None;
                }
            }
        }
        if self.cancelled_through.load(Ordering::SeqCst) >= request_id {
            *slot = None;
            return Err(unconfirmed);
        }
        result
    }
    fn interrupt(&self, request_id: u64) {
        self.cancelled_through
            .fetch_max(request_id, Ordering::SeqCst);
        if let Ok(active) = self.active.lock() {
            if let Some((id, child)) = active.as_ref() {
                if *id == request_id {
                    if let Ok(mut child) = child.lock() {
                        let _ = child.kill();
                    }
                }
            }
        }
    }
    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        if let Ok(mut active) = self.active.lock() {
            if let Some((_, child)) = active.take() {
                if let Ok(mut child) = child.lock() {
                    let _ = child.kill();
                }
            }
        }
    }
}
impl Drop for NativeVault {
    fn drop(&mut self) {
        self.close();
    }
}
fn exchange(
    pipes: &mut Pipes,
    bytes: &[u8],
    operation: &str,
) -> Result<serde_json::Value, Failure> {
    send(&mut pipes.input, bytes).and_then(|()| reply(&mut pipes.output, operation))
}
fn connection_unusable(result: &Result<serde_json::Value, Failure>) -> bool {
    match result {
        Err(_) => true,
        Ok(reply) => {
            reply["ok"] == false
                && matches!(
                    reply["code"].as_str(),
                    Some(
                        "vault_unavailable"
                            | "vault_change_not_confirmed"
                            | "vault_change_not_confirmed_audit_unavailable"
                    )
                )
        }
    }
}
fn send(pipe: &mut ChildStdin, bytes: &[u8]) -> Result<(), Failure> {
    pipe.write_all(bytes)
        .and_then(|()| pipe.write_all(b"\n"))
        .and_then(|()| pipe.flush())
        .map_err(|_| Failure::VaultUnavailable)
}
fn read_line(pipe: &mut BufReader<ChildStdout>) -> Result<Vec<u8>, Failure> {
    let mut bytes = Vec::new();
    let length = Read::take(pipe, 32769)
        .read_until(b'\n', &mut bytes)
        .map_err(|_| Failure::VaultUnavailable)?;
    if length == 0 || length > 32768 || bytes.last() != Some(&b'\n') {
        return Err(Failure::VaultUnavailable);
    }
    Ok(bytes)
}
fn reply(pipe: &mut BufReader<ChildStdout>, operation: &str) -> Result<serde_json::Value, Failure> {
    let reply: Reply =
        serde_json::from_slice(&read_line(pipe)?).map_err(|_| Failure::VaultUnavailable)?;
    let valid = match &reply {
        Reply::Success(success) => {
            let states = if operation == "changePassphrase" {
                VAULT_PASSPHRASE_CHANGE_STATES
            } else {
                VAULT_STATES
            };
            success.ok && states.contains(&success.status.state.as_str())
        }
        Reply::Refusal(refusal) => {
            !refusal.ok && VAULT_ERROR_CODES.contains(&refusal.code.as_str())
        }
    };
    if !valid {
        return Err(Failure::VaultUnavailable);
    }
    serde_json::to_value(reply).map_err(|_| Failure::VaultUnavailable)
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Ready {
    ready: bool,
}
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(untagged)]
enum Reply {
    Success(Success),
    Refusal(Refusal),
}
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Success {
    ok: bool,
    status: ReplyStatus,
}
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct ReplyStatus {
    state: String,
}
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Refusal {
    ok: bool,
    code: String,
}

#[derive(serde::Serialize)]
struct Initialization<'a> {
    address: String,
    credential: &'a str,
}
#[derive(serde::Serialize)]
struct WireCommand<'a> {
    operation: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    passphrase: Option<&'a str>,
}

#[cfg(all(test, unix))]
#[path = "../../../tests/vault/process.rs"]
mod tests;
