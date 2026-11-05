//! One product call, shared by `nessa call` and `nessa mcp`.
//!
//! The catalog accepts the method and its params. The gateway port sends them.
//! Cedar, on that gateway, is the permission check.
use super::surface::{prepare, PrepareError};
use super::{CliError, Gateway, GatewayCallError};
use serde_json::{json, Value};

/// A call that did not return a payload. `code` is what stdout reports.
/// `detail` is a diagnostic and is not the payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallFailure {
    pub code: String,
    pub detail: String,
}

impl CallFailure {
    fn code(code: &str, detail: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for CallFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}

/// Prepare `method` and send it. The payload is the handler's JSON.
pub fn invoke(
    gateway: &mut dyn Gateway,
    method: &str,
    params: Value,
) -> Result<Value, CallFailure> {
    let prepared = prepare(method, &params).map_err(prepare_failure)?;
    match gateway.call(&prepared.method, prepared.params) {
        Ok(payload) => Ok(payload),
        Err(GatewayCallError::Refused { code }) => {
            Err(CallFailure::code(&code, "the gateway refused the call"))
        }
        Err(GatewayCallError::Unconfirmed { code }) => Err(CallFailure::code(
            &code,
            CliError::VaultChangeNotConfirmed.to_string(),
        )),
        Err(GatewayCallError::Transport(error)) => {
            Err(CallFailure::code("transport", error.to_string()))
        }
    }
}

fn prepare_failure(error: PrepareError) -> CallFailure {
    match error {
        PrepareError::Unknown => {
            CallFailure::code("unknown_method", "the manifest has no such method")
        }
        PrepareError::Excluded { reason } => CallFailure::code("invalid_request", reason),
        PrepareError::Invalid => {
            CallFailure::code("invalid_request", "params do not match the method schema")
        }
    }
}

/// The JSON a person or an agent reads for a refusal: the code, not the payload.
pub fn refusal_body(failure: &CallFailure) -> Value {
    json!({"ok": false, "code": failure.code})
}
