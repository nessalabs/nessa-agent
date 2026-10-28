//! Bounded anonymous-pipe surface for the native app's retained vault connection.
//! Input EOF interrupts a pending RPC independently, so a crashed host cannot
//! leave a late unlock alive. No input field is printed in diagnostics.
use crate::core::RunError;
use nessa_client_core::local_product::{LocalProduct, LocalProductError};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    io::{self, BufRead, Write},
    net::SocketAddr,
    sync::mpsc,
};
use zeroize::Zeroizing;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Init {
    address: SocketAddr,
    credential: String,
}
fn line(reader: &mut impl BufRead) -> Result<Option<Zeroizing<String>>, RunError> {
    let mut bytes = Zeroizing::new(Vec::new());
    let mut chunk = Vec::new();
    loop {
        let available = reader.fill_buf().map_err(|_| problem())?;
        if available.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Err(problem())
            };
        }
        let consumed = available
            .iter()
            .position(|b| *b == b'\n')
            .map_or(available.len(), |i| i + 1);
        if bytes.len() + consumed > 32768 {
            return Err(problem());
        }
        chunk.extend_from_slice(&available[..consumed]);
        reader.consume(consumed);
        let done = chunk.last() == Some(&b'\n');
        bytes.extend_from_slice(&chunk);
        zeroize::Zeroize::zeroize(&mut chunk);
        if done {
            let text = String::from_utf8(std::mem::take(&mut *bytes)).map_err(|_| problem())?;
            return Ok(Some(Zeroizing::new(text)));
        }
    }
}
fn output(value: &Value) -> Result<(), RunError> {
    let mut out = io::stdout().lock();
    serde_json::to_writer(&mut out, value).map_err(|_| problem())?;
    writeln!(&mut out).map_err(|_| problem())?;
    out.flush().map_err(|_| problem())
}
pub(super) fn serve() -> Result<(), RunError> {
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    let init = line(&mut reader)?.ok_or_else(problem)?;
    let Init {
        address,
        credential,
    } = serde_json::from_str(&init).map_err(|_| problem())?;
    let credential = Zeroizing::new(credential);
    let mut client =
        LocalProduct::connect_with_credential(address, &credential).map_err(|_| problem())?;
    drop(credential);
    drop(init);
    drop(reader);
    let cancellation = client.cancellation().map_err(|_| problem())?;
    let (send, receive) = mpsc::sync_channel(1);
    // This reader owns only input and cancellation, never the current RPC's lifetime.
    std::thread::spawn(move || {
        let mut reader = io::stdin().lock();
        while let Ok(Some(command)) = line(&mut reader) {
            if send.send(command).is_err() {
                break;
            }
        }
        cancellation.close();
    });
    output(&json!({"ready":true}))?;
    for command in receive {
        let mut value: Value = serde_json::from_str(&command).map_err(|_| problem())?;
        let Some(object) = value.as_object_mut() else {
            return Err(problem());
        };
        let method = match object
            .remove("operation")
            .and_then(|v| v.as_str().map(str::to_owned))
            .as_deref()
        {
            Some("status") => "vault.status",
            Some("setup") => "vault.setup",
            Some("unlock") => "vault.unlock",
            Some("lock") => "vault.lock",
            _ => return Err(problem()),
        };
        let params = match object.remove("passphrase") {
            Some(Value::String(passphrase)) => json!({"passphrase":passphrase}),
            Some(_) => return Err(problem()),
            None => json!({}),
        };
        if !object.is_empty() {
            return Err(problem());
        }
        let result: Result<Value, _> = client.request(method, params);
        match result {
            Ok(status) => output(&json!({"ok":true,"status":status}))?,
            Err(LocalProductError::Vault(code)) => {
                output(&json!({"ok":false,"code":code.as_str()}))?
            }
            Err(LocalProductError::Unauthorized | LocalProductError::Denied) => {
                output(&json!({"ok":false,"code":"forbidden"}))?
            }
            Err(_) => {
                output(&json!({"ok":false,"code":"vault_unavailable"}))?;
                return Err(problem());
            }
        }
    }
    Ok(())
}
fn problem() -> RunError {
    RunError::Runtime("vault session transport unavailable".into())
}
