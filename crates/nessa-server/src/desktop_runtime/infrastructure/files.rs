use crate::desktop_runtime::{
    application::{RetirementAudit, RetirementRecord, RetirementResult},
    domain::{
        validate_retirement_evidence, RetirementCause, RetirementFence, RetirementRefusal,
        RetirementRequest, RunningRuntime,
    },
};
use nessa_auth::application::ports::Clock;
use nessa_local_storage::{create_directory, open, sync_directory, OpenMode, PrivateTempFile};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};
use std::{
    future::Future,
    io::{ErrorKind, Read},
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    request_id: String,
    target_fingerprint: String,
    running_instance: String,
    running_generation: String,
    target_generation: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecordedResult {
    request_id: String,
    target_fingerprint: String,
    running_fingerprint: String,
    requested_instance: String,
    requested_running_generation: String,
    running_instance: String,
    running_generation: String,
    target_generation: String,
    retired: bool,
    #[serde(deserialize_with = "required_nullable_error")]
    retirement_request_id: Option<String>,
    #[serde(deserialize_with = "required_nullable_cause")]
    retirement_cause: Option<RecordedCause>,
    #[serde(deserialize_with = "required_nullable_error")]
    cleanup_error: Option<String>,
    #[serde(deserialize_with = "required_nullable_error")]
    audit_error: Option<String>,
    /// Absent from retired results, and from results a gateway wrote before
    /// refusals had names.
    #[serde(default)]
    refusal: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecordedCause {
    principal_id: String,
    surface_id: String,
    request_id: String,
}
fn required_nullable_error<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)
}
fn required_nullable_cause<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<RecordedCause>, D::Error> {
    Option::<RecordedCause>::deserialize(deserializer)
}
#[derive(Clone)]
pub(crate) struct RetirementFiles {
    directory: PathBuf,
    clock: Arc<dyn Clock>,
}
impl RetirementFiles {
    pub(crate) fn new(root: &Path, clock: Arc<dyn Clock>) -> Result<Self, String> {
        let directory = root.join("gateway-upgrade");
        create_directory(&directory).map_err(|e| e.to_string())?;
        create_directory(&directory.join("audit")).map_err(|e| e.to_string())?;
        Ok(Self { directory, clock })
    }
    pub(crate) fn request(&self) -> Result<RetirementRequest, String> {
        let file = open(
            &self.directory.join("request.json"),
            OpenMode::ReadNonblocking,
        )
        .map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        file.take(4097)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 4096 {
            return Err("upgrade request exceeds limit".into());
        }
        let request: Request = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        RetirementRequest::new(
            request.request_id,
            request.target_fingerprint,
            request.running_instance,
            request.running_generation,
            request.target_generation,
        )
        .map_err(str::to_owned)
    }
    pub(crate) fn fence(&self) -> Result<Option<RetirementFence>, String> {
        self.evidence()
    }
    fn evidence(&self) -> Result<Option<RetirementFence>, String> {
        let file = match open(
            &self.directory.join("result.json"),
            OpenMode::ReadNonblocking,
        ) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        let mut bytes = Vec::new();
        file.take(65537)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes.len() > 65536 {
            return Err("retirement result exceeds limit".into());
        }
        let result: RecordedResult =
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        let request = RetirementRequest::new(
            result.request_id,
            result.target_fingerprint,
            result.requested_instance,
            result.requested_running_generation,
            result.target_generation,
        )
        .map_err(str::to_owned)?;
        let running = RunningRuntime::new(
            result.running_fingerprint,
            result.running_instance,
            1,
            result.running_generation,
        )
        .map_err(str::to_owned)?;
        let cause = result
            .retirement_cause
            .map(|cause| {
                RetirementCause::new(cause.principal_id, cause.surface_id, cause.request_id)
            })
            .transpose()
            .map_err(str::to_owned)?;
        if result.retirement_request_id.as_deref()
            != cause.as_ref().map(RetirementCause::request_id)
        {
            return Err("retirement correlation differs from lifecycle attribution".into());
        }
        // The reader holds the writer's rule: a retired result names no
        // refusal, and a refusal is one of the published names. A result with
        // no refusal at all may come from a gateway that predates the names.
        match result.refusal.as_deref() {
            Some(_) if result.retired => {
                return Err("a retired result names a refusal".into());
            }
            Some(name) if RetirementRefusal::named(name).is_none() => {
                return Err("retirement result names an unknown refusal".into());
            }
            _ => {}
        }
        validate_retirement_evidence(
            request,
            &running,
            cause,
            result.retired,
            result.cleanup_error.is_some() || result.audit_error.is_some(),
        )
        .map_err(str::to_owned)
    }

    pub(crate) fn result(&self, result: &RetirementResult) -> Result<(), String> {
        if result.retired == result.refusal.is_some() {
            return Err(
                "a refusal must accompany exactly the retirements that did not happen".into(),
            );
        }
        let cause = result
            .retirement_cause
            .as_ref()
            .map(|cause| {
                RetirementCause::new(
                    cause.principal_id().to_owned(),
                    cause.surface_id().to_owned(),
                    cause.request_id().to_owned(),
                )
            })
            .transpose()
            .map_err(str::to_owned)?;
        let incoming = validate_retirement_evidence(
            result.request.clone(),
            &result.running,
            cause,
            result.retired,
            result.cleanup_error.is_some() || result.audit_error.is_some(),
        )
        .map_err(str::to_owned)?;
        let prior = self
            .evidence()?
            .filter(|fence| fence.applies_to(&result.running));
        if prior.as_ref().is_some_and(|prior| {
            incoming.as_ref().is_none()
                || incoming
                    .as_ref()
                    .is_some_and(|incoming| incoming.cause() != prior.cause())
                || (prior.confirmed() && !result.retired)
        }) {
            return Err(
                "admitted retirement evidence preserved; rejection cannot erase its admission fence"
                    .into(),
            );
        }
        let retirement_cause = prior
            .as_ref()
            .map(RetirementFence::cause)
            .or_else(|| incoming.as_ref().map(RetirementFence::cause));
        let mut recorded = json!({
                "requestId": result.request.id(), "targetFingerprint": result.request.target().as_str(),
                "runningFingerprint": result.running.fingerprint().as_str(), "runningInstance": result.running.instance().as_str(), "requestedInstance": result.request.running_instance().as_str(), "runningGeneration": result.running.generation().as_str(), "requestedRunningGeneration": result.request.running_generation().as_str(), "targetGeneration": result.request.target_generation().as_str(), "retirementCause": retirement_cause.map(|cause| json!({"principalId":cause.principal_id(),"surfaceId":cause.surface_id(),"requestId":cause.request_id()})), "retirementRequestId": retirement_cause.map(RetirementCause::request_id), "retired": result.retired,
                "cleanupError": result.cleanup_error, "auditError": result.audit_error
        });
        // Written only when there is one, so a retired result is exactly what
        // every earlier gateway and host wrote and reads (ADR 221).
        if let Some(refusal) = result.refusal {
            recorded["refusal"] = json!(refusal.as_str());
        }
        write(&self.directory, "result.json", &recorded)
    }
}
fn write(directory: &Path, name: &str, value: &Value) -> Result<(), String> {
    let mut file = PrivateTempFile::new_in(directory).map_err(|e| e.to_string())?;
    serde_json::to_writer(file.as_file_mut(), value).map_err(|e| e.to_string())?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    file.persist(&directory.join(name))
        .map_err(|e| e.to_string())?;
    sync_directory(directory).map_err(|e| e.to_string())
}
impl RetirementAudit for RetirementFiles {
    fn record(
        &self,
        record: RetirementRecord,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        let files = self.clone();
        let observed_at_ms = self.clock.unix_milliseconds();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let prior = files
                    .evidence()?
                    .filter(|fence| fence.applies_to(&record.running));
                let current_cause = record
                    .retirement_cause
                    .as_ref()
                    .map(|cause| {
                        RetirementCause::new(
                            cause.principal_id().to_owned(),
                            cause.surface_id().to_owned(),
                            cause.request_id().to_owned(),
                        )
                    })
                    .transpose()
                    .map_err(str::to_owned)?;
                let retirement_cause = prior
                    .as_ref()
                    .map(|fence| fence.cause().clone())
                    .or(current_cause);
                let retirement_request_id = retirement_cause.as_ref().map(RetirementCause::request_id);
                let initiator = retirement_cause.as_ref().map_or("desktop_runtime", RetirementCause::principal_id);
                let cause = retirement_cause.as_ref().map_or("gateway_upgrade", RetirementCause::surface_id);
                write(&files.directory.join("audit"), &format!("{}.json", Uuid::new_v4()), &json!({
                    "observedAtMs": observed_at_ms, "retirementRequestId": retirement_request_id, "requestId": record.request.id(), "targetFingerprint": record.request.target().as_str(),
                    "runningFingerprint": record.running.fingerprint().as_str(), "runningInstance": record.running.instance().as_str(), "requestedInstance": record.request.running_instance().as_str(), "runningGeneration": record.running.generation().as_str(), "requestedRunningGeneration": record.request.running_generation().as_str(), "targetGeneration": record.request.target_generation().as_str(), "initiator": initiator, "cause": cause,
                    "before": "retirement_requested", "after": if record.cleanup_error.is_none() { "retired" } else { "retirement_unconfirmed" },
                    "cleanupError": record.cleanup_error, "retirementCause": record.retirement_cause.as_ref().map(|actor| json!({"principalId":actor.principal_id(),"surfaceId":actor.surface_id(),"requestId":actor.request_id()}))
                }))
            })
            .await
            .map_err(|e| e.to_string())?
        })
    }
}

#[cfg(test)]
#[path = "../../../tests/desktop_runtime/files.rs"]
mod tests;
