//! Stage-scoped immutable gateway lifecycle journal.
//!
//! ```text
//! application session -> retained directory -> acknowledged immutable record
//! ```
//! Arrows mean durable writes. One retained session validates the whole stage,
//! holds its lock, and orders every record for its attempt.
use crate::gateway::{
    application::{
        GatewayError, GatewayLifecycleRecovery, GatewayLifecycleRecoveryStep,
        GatewayReconciliationAttempt, GatewayReconciliationAudit, GatewayReconciliationEffect,
        GatewayReconciliationIntent, GatewayReconciliationJournalSession,
        GatewayReconciliationOutcome, GatewayReconciliationOutcomeError,
        GatewayReconciliationRequest, MonotonicClock,
    },
    domain::value_objects::{
        AuditDeliveryReceipt, BundledSurface, LifecycleCommandResult, LifecycleEffect,
        LifecycleEffectPredicate, LifecycleFailedPhase, LifecycleHistory, LifecycleObservation,
        LifecycleObservationSource, LifecyclePhysicalOutcome, LifecyclePlanStep, LifecycleRecord,
        LifecycleRecordKind, LifecycleRecordPayload, ReconciliationCause,
        ReconciliationCleanupDecision, ReconciliationCorrelation, ReconciliationEvidence,
        ReconciliationIncarnation, ReconciliationInitiator, ReconciliationTarget,
        SystemdJobAttempt, SystemdJobMode, SystemdJobOperation, SystemdManagerIdentity,
        SystemdRuntimeObservation, SystemdUnitName, SystemdUnitState,
    },
};
use nessa_local_storage::{OpenMode, PrivateDirectory, PrivateFileType};
use serde::{
    de::{self, MapAccess, SeqAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use serde_json::{json, Value};
use std::{
    ffi::OsStr,
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    os::fd::AsRawFd,
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

const DIRECTORY: &str = "gateway-reconciliation-journal";
const LOCK_FILE: &str = "journal.lock";
const MAX_RECORD_BYTES: u64 = 256 * 1024;

pub(in crate::gateway::infrastructure) struct FileReconciliationAudit {
    config_root: Option<PathBuf>,
    clock: Arc<dyn MonotonicClock>,
}

impl FileReconciliationAudit {
    pub(in crate::gateway::infrastructure) fn new(
        config_root: Option<PathBuf>,
        clock: Arc<dyn MonotonicClock>,
    ) -> Self {
        Self { config_root, clock }
    }

    fn open_directory(&self) -> Result<PrivateDirectory, GatewayError> {
        let root = self.config_root.clone().ok_or_else(|| {
            GatewayError::Registration(
                "Application config storage is required for gateway audit".into(),
            )
        })?;
        let directory = root.join(DIRECTORY);
        PrivateDirectory::create_path(&root, &directory)
            .map_err(|error| GatewayError::Registration(error.to_string()))
    }
}

struct FileJournalSession {
    directory: PrivateDirectory,
    _lock: File,
    attempt: GatewayReconciliationAttempt,
    state: Mutex<SessionState>,
    advanced: Condvar,
    deadline: Option<Instant>,
    clock: Arc<dyn MonotonicClock>,
    recovery: Option<GatewayLifecycleRecovery>,
}

struct SessionState {
    namespace: Option<String>,
    next_sequence: u64,
    terminal: bool,
    history: Option<LifecycleHistory>,
    latest_observation: Option<LifecycleObservation>,
}

enum JournalAppendError {
    Rejected(GatewayError),
    Delivery(GatewayError),
}

impl JournalAppendError {
    fn into_gateway_error(self) -> GatewayError {
        match self {
            Self::Rejected(error) | Self::Delivery(error) => error,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredRecord {
    service_namespace: String,
    attempt_correlation: String,
    sequence: u64,
    kind: String,
    payload: Value,
}

struct UniqueJson(Value);

impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct UniqueJsonVisitor;
        impl<'de> Visitor<'de> for UniqueJsonVisitor {
            type Value = UniqueJson;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("JSON without duplicate object keys")
            }

            fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Bool(value)))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Number(value.into())))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Number(value.into())))
            }

            fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                serde_json::Number::from_f64(value)
                    .map(Value::Number)
                    .map(UniqueJson)
                    .ok_or_else(|| E::custom("invalid JSON number"))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::String(value.to_owned())))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::String(value)))
            }

            fn visit_none<E>(self) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Null))
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Null))
            }

            fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: Deserializer<'de>,
            {
                UniqueJson::deserialize(deserializer)
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut values = Vec::new();
                while let Some(value) = sequence.next_element::<UniqueJson>()? {
                    values.push(value.0);
                }
                Ok(UniqueJson(Value::Array(values)))
            }

            fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut values = serde_json::Map::new();
                while let Some(key) = object.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(de::Error::custom(format!(
                            "duplicate gateway lifecycle JSON field: {key}"
                        )));
                    }
                    values.insert(key, object.next_value::<UniqueJson>()?.0);
                }
                Ok(UniqueJson(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(UniqueJsonVisitor)
    }
}

impl StoredRecord {
    fn file_name(&self) -> String {
        format!(
            "{}-{:020}-{}.json",
            self.attempt_correlation, self.sequence, self.kind
        )
    }

    fn validate_header(&self) -> Result<(), GatewayError> {
        ReconciliationCorrelation::parse(self.attempt_correlation.clone())
            .map_err(|error| GatewayError::Registration(error.to_string()))?;
        if self.service_namespace.trim().is_empty()
            || record_kind(&self.kind).is_none()
            || self.file_name().contains('/')
        {
            return Err(GatewayError::Registration(
                "Invalid gateway lifecycle journal record header".into(),
            ));
        }
        validate_payload_shape(&self.kind, &self.payload)?;
        Ok(())
    }
}

fn validate_payload_shape(kind: &str, payload: &Value) -> Result<(), GatewayError> {
    let expected: &[&str] = match record_kind(kind) {
        Some(LifecycleRecordKind::Intent) => &["origin", "target", "before"],
        Some(LifecycleRecordKind::JoinedRequest) => &["originRequestCorrelation", "joinedRequest"],
        Some(LifecycleRecordKind::EffectPlan) => {
            &["planId", "expectedBefore", "target", "primary", "cleanup"]
        }
        Some(LifecycleRecordKind::NativeAttempt) => &["planId", "stepId", "attempt"],
        Some(LifecycleRecordKind::EffectCompletion) => &["planId", "stepId", "result"],
        Some(LifecycleRecordKind::Observation) => &["source", "state"],
        Some(LifecycleRecordKind::Outcome) => &["physical", "lastConfirmed", "cleanup"],
        None => {
            return Err(GatewayError::Registration(
                "Unknown gateway lifecycle record kind".into(),
            ))
        }
    };
    let object = payload.as_object().ok_or_else(|| {
        GatewayError::Registration("Gateway lifecycle record payload must be an object".into())
    })?;
    if object.len() != expected.len() || expected.iter().any(|field| !object.contains_key(*field)) {
        return Err(GatewayError::Registration(
            "Gateway lifecycle record payload has missing or unknown fields".into(),
        ));
    }
    Ok(())
}

fn object<'value>(
    value: &'value Value,
    fields: &[&str],
) -> Result<&'value serde_json::Map<String, Value>, GatewayError> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid_record("expected an object"))?;
    if object.len() != fields.len() || fields.iter().any(|field| !object.contains_key(*field)) {
        return Err(invalid_record("object has missing or unknown fields"));
    }
    Ok(object)
}

fn string(object: &serde_json::Map<String, Value>, field: &str) -> Result<String, GatewayError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| invalid_record("expected a string field"))
}

fn invalid_record(detail: &str) -> GatewayError {
    GatewayError::Registration(format!(
        "Invalid gateway lifecycle journal record: {detail}"
    ))
}

fn parse_target(value: &Value) -> Result<ReconciliationTarget, GatewayError> {
    let value = object(
        value,
        &["service", "runtimeFingerprint", "serviceGeneration"],
    )?;
    ReconciliationTarget::new(
        string(value, "service")?,
        string(value, "runtimeFingerprint")?,
        string(value, "serviceGeneration")?,
    )
    .map_err(|error| invalid_record(&error.to_string()))
}

fn parse_identity(value: &Value) -> Result<ReconciliationIncarnation, GatewayError> {
    let value = object(
        value,
        &[
            "service",
            "runtimeFingerprint",
            "runtimeInstance",
            "serviceGeneration",
            "processId",
            "port",
        ],
    )?;
    let target = ReconciliationTarget::new(
        string(value, "service")?,
        string(value, "runtimeFingerprint")?,
        string(value, "serviceGeneration")?,
    )
    .map_err(|error| invalid_record(&error.to_string()))?;
    let process_id = value
        .get("processId")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| invalid_record("invalid process ID"))?;
    let port = value
        .get("port")
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .ok_or_else(|| invalid_record("invalid port"))?;
    ReconciliationIncarnation::new(target, string(value, "runtimeInstance")?, process_id, port)
        .map_err(|error| invalid_record(&error.to_string()))
}

fn optional_identity(value: &Value) -> Result<Option<ReconciliationIncarnation>, GatewayError> {
    if value.is_null() {
        Ok(None)
    } else {
        parse_identity(value).map(Some)
    }
}

fn parse_cause(value: &str) -> Result<ReconciliationCause, GatewayError> {
    match value {
        "startup" => Ok(ReconciliationCause::Startup),
        "credential_load" => Ok(ReconciliationCause::CredentialLoad),
        "explicit_retry" => Ok(ReconciliationCause::ExplicitRetry),
        "claude_configuration_changed" => Ok(ReconciliationCause::ClaudeConfigurationChanged),
        "desktop_quit_policy" => Ok(ReconciliationCause::DesktopQuitPolicy),
        _ => Err(invalid_record("unknown reconciliation cause")),
    }
}

fn parse_initiator(value: &str) -> Result<ReconciliationInitiator, GatewayError> {
    match value {
        "desktop_host" => Ok(ReconciliationInitiator::DesktopHost),
        "main_window" => Ok(ReconciliationInitiator::BundledSurface(
            BundledSurface::Main,
        )),
        "setup_window" => Ok(ReconciliationInitiator::BundledSurface(
            BundledSurface::Setup,
        )),
        _ => Err(invalid_record("unknown reconciliation initiator")),
    }
}

fn parse_request(
    value: &Value,
) -> Result<
    (
        ReconciliationCorrelation,
        ReconciliationCause,
        ReconciliationInitiator,
    ),
    GatewayError,
> {
    let value = object(value, &["correlation", "cause", "initiator"])?;
    let correlation = ReconciliationCorrelation::parse(string(value, "correlation")?)
        .map_err(|error| invalid_record(&error.to_string()))?;
    Ok((
        correlation,
        parse_cause(&string(value, "cause")?)?,
        parse_initiator(&string(value, "initiator")?)?,
    ))
}

fn parse_effect(value: &Value) -> Result<LifecycleEffect, GatewayError> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid_record("effect must be an object"))?;
    let kind = string(object, "kind")?;
    match kind.as_str() {
        "stage_runtime" => {
            object_exact(object, &["kind", "fingerprint"])?;
            Ok(LifecycleEffect::StageRuntime {
                fingerprint: string(object, "fingerprint")?,
            })
        }
        "request_retirement" | "stop_agents" => {
            object_exact(object, &["kind", "incarnation"])?;
            let incarnation = parse_identity(&object["incarnation"])?;
            Ok(if kind == "request_retirement" {
                LifecycleEffect::RequestRetirement { incarnation }
            } else {
                LifecycleEffect::StopAgents { incarnation }
            })
        }
        "request_systemd_retirement" => {
            object_exact(object, &["kind", "incarnation", "requestId"])?;
            Ok(LifecycleEffect::RequestSystemdRetirement {
                incarnation: parse_identity(&object["incarnation"])?,
                request_id: string(object, "requestId")?,
            })
        }
        "unload_service" => {
            object_exact(object, &["kind", "service"])?;
            Ok(LifecycleEffect::UnloadService {
                service: string(object, "service")?,
            })
        }
        "publish_service_definition" | "bootstrap_service" | "adopt_ready_incarnation" => {
            object_exact(object, &["kind", "target"])?;
            let target = parse_target(&object["target"])?;
            Ok(match kind.as_str() {
                "publish_service_definition" => {
                    LifecycleEffect::PublishServiceDefinition { target }
                }
                "bootstrap_service" => LifecycleEffect::BootstrapService { target },
                _ => LifecycleEffect::AdoptReadyIncarnation { target },
            })
        }
        "publish_systemd_service_definition" => {
            object_exact(object, &["kind", "target", "definitionDigest"])?;
            Ok(LifecycleEffect::PublishSystemdServiceDefinition {
                target: parse_target(&object["target"])?,
                definition_digest: string(object, "definitionDigest")?,
            })
        }
        "create_gateway_data_directory"
        | "create_systemd_wants_directory"
        | "publish_systemd_wants_link" => {
            object_exact(object, &["kind", "target"])?;
            let target = parse_target(&object["target"])?;
            Ok(match kind.as_str() {
                "create_gateway_data_directory" => {
                    LifecycleEffect::CreateGatewayDataDirectory { target }
                }
                "create_systemd_wants_directory" => {
                    LifecycleEffect::CreateSystemdWantsDirectory { target }
                }
                _ => LifecycleEffect::PublishSystemdWantsLink { target },
            })
        }
        "reload_systemd_manager" | "start_systemd_unit" | "stop_systemd_unit" => {
            let has_mode = kind != "reload_systemd_manager";
            object_exact(
                object,
                if has_mode {
                    &["kind", "manager", "unit", "mode"]
                } else {
                    &["kind", "manager", "unit"]
                },
            )?;
            let manager = parse_systemd_manager(&object["manager"])?;
            let unit = SystemdUnitName::parse(string(object, "unit")?)
                .map_err(|error| invalid_record(&error.to_string()))?;
            Ok(match kind.as_str() {
                "reload_systemd_manager" => LifecycleEffect::ReloadSystemdManager { manager, unit },
                "start_systemd_unit" => LifecycleEffect::StartSystemdUnit {
                    manager,
                    unit,
                    mode: parse_systemd_job_mode(&string(object, "mode")?)?,
                },
                _ => LifecycleEffect::StopSystemdUnit {
                    manager,
                    unit,
                    mode: parse_systemd_job_mode(&string(object, "mode")?)?,
                },
            })
        }
        "prune_runtime" => {
            object_exact(object, &["kind", "fingerprint"])?;
            Ok(LifecycleEffect::PruneRuntime {
                fingerprint: string(object, "fingerprint")?,
            })
        }
        "remove_staging_runtime" => {
            object_exact(object, &["kind", "generation"])?;
            Ok(LifecycleEffect::RemoveStagingRuntime {
                generation: string(object, "generation")?,
            })
        }
        _ => Err(invalid_record("unknown lifecycle effect")),
    }
}

fn parse_systemd_manager(value: &Value) -> Result<SystemdManagerIdentity, GatewayError> {
    let value = object(value, &["uniqueName", "processId", "userId"])?;
    let process_id = value["processId"]
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| invalid_record("invalid systemd manager process ID"))?;
    let user_id = value["userId"]
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| invalid_record("invalid systemd manager user ID"))?;
    SystemdManagerIdentity::new(string(value, "uniqueName")?, process_id, user_id)
        .map_err(|error| invalid_record(&error.to_string()))
}

fn parse_systemd_job_mode(value: &str) -> Result<SystemdJobMode, GatewayError> {
    match value {
        "fail" => Ok(SystemdJobMode::Fail),
        _ => Err(invalid_record("unknown systemd job mode")),
    }
}

fn parse_systemd_job_attempt(value: &Value) -> Result<SystemdJobAttempt, GatewayError> {
    let value = object(
        value,
        &[
            "manager",
            "operation",
            "mode",
            "unit",
            "objectPath",
            "jobId",
        ],
    )?;
    let operation = match string(value, "operation")?.as_str() {
        "start" => SystemdJobOperation::Start,
        "stop" => SystemdJobOperation::Stop,
        _ => return Err(invalid_record("unknown systemd job operation")),
    };
    let unit = SystemdUnitName::parse(string(value, "unit")?)
        .map_err(|error| invalid_record(&error.to_string()))?;
    let job_id = value["jobId"]
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| invalid_record("invalid systemd job ID"))?;
    SystemdJobAttempt::new(
        parse_systemd_manager(&value["manager"])?,
        operation,
        parse_systemd_job_mode(&string(value, "mode")?)?,
        unit,
        string(value, "objectPath")?,
        job_id,
    )
    .map_err(|error| invalid_record(&error.to_string()))
}

fn object_exact(
    object: &serde_json::Map<String, Value>,
    fields: &[&str],
) -> Result<(), GatewayError> {
    if object.len() != fields.len() || fields.iter().any(|field| !object.contains_key(*field)) {
        Err(invalid_record("object has missing or unknown fields"))
    } else {
        Ok(())
    }
}

fn parse_predicate(value: &Value) -> Result<LifecycleEffectPredicate, GatewayError> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid_record("predicate must be an object"))?;
    let kind = string(object, "kind")?;
    match kind.as_str() {
        "always" | "primary_returned" | "primary_accepted" => {
            object_exact(object, &["kind"])?;
            Ok(match kind.as_str() {
                "always" => LifecycleEffectPredicate::Always,
                "primary_returned" => LifecycleEffectPredicate::PrimaryReturned,
                _ => LifecycleEffectPredicate::PrimaryAccepted,
            })
        }
        "observation_matches" => {
            object_exact(object, &["kind", "incarnation"])?;
            Ok(LifecycleEffectPredicate::ObservationMatches(
                parse_identity(&object["incarnation"])?,
            ))
        }
        _ => Err(invalid_record("unknown effect predicate")),
    }
}

fn parse_step(value: &Value) -> Result<LifecyclePlanStep, GatewayError> {
    let value = object(value, &["id", "effect", "predicate"])?;
    LifecyclePlanStep::new(
        string(value, "id")?,
        parse_effect(&value["effect"])?,
        parse_predicate(&value["predicate"])?,
    )
    .map_err(|error| invalid_record(&error.to_string()))
}

fn parse_command_result(value: &Value) -> Result<LifecycleCommandResult, GatewayError> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid_record("command result must be an object"))?;
    let kind = string(object, "kind")?;
    if kind == "accepted" {
        object_exact(object, &["kind"])?;
        return Ok(LifecycleCommandResult::Accepted);
    }
    object_exact(object, &["kind", "message"])?;
    let message = string(object, "message")?;
    match kind.as_str() {
        "rejected" => Ok(LifecycleCommandResult::Rejected(message)),
        "failed" => Ok(LifecycleCommandResult::Failed(message)),
        "indeterminate" => Ok(LifecycleCommandResult::Indeterminate(message)),
        _ => Err(invalid_record("unknown command result")),
    }
}

fn parse_observation(value: &Value) -> Result<LifecycleObservation, GatewayError> {
    let value = object(
        value,
        &[
            "version",
            "incarnation",
            "targetArtifactPresent",
            "systemd",
            "systemdState",
        ],
    )?;
    let version = value["version"]
        .as_u64()
        .ok_or_else(|| invalid_record("invalid observation version"))?;
    let artifact = value["targetArtifactPresent"]
        .as_bool()
        .ok_or_else(|| invalid_record("invalid target artifact fact"))?;
    let incarnation = optional_identity(&value["incarnation"])?;
    let systemd_state = if value["systemdState"].is_null() {
        None
    } else {
        Some(
            SystemdUnitState::parse(&string(value, "systemdState")?)
                .map_err(|error| invalid_record(&error.to_string()))?,
        )
    };
    if value["systemd"].is_null() {
        return match systemd_state {
            Some(state) => LifecycleObservation::with_systemd_state(version, artifact, state)
                .map_err(|error| invalid_record(&error.to_string())),
            None if incarnation.is_none() => Ok(LifecycleObservation::new(version, None, artifact)),
            None => Err(invalid_record(
                "portable observation has no native platform evidence",
            )),
        };
    }
    if systemd_state != Some(SystemdUnitState::Active) {
        return Err(invalid_record(
            "active systemd runtime disagrees with its physical state",
        ));
    }
    let incarnation = incarnation
        .ok_or_else(|| invalid_record("systemd observation has no portable incarnation"))?;
    LifecycleObservation::with_systemd(
        version,
        incarnation,
        artifact,
        parse_systemd_runtime(&value["systemd"])?,
    )
    .map_err(|error| invalid_record(&error.to_string()))
}

fn parse_systemd_runtime(value: &Value) -> Result<SystemdRuntimeObservation, GatewayError> {
    let value = object(
        value,
        &[
            "target",
            "manager",
            "unit",
            "invocation",
            "mainProcessId",
            "enabled",
        ],
    )?;
    let invocation = value["invocation"]
        .as_array()
        .ok_or_else(|| invalid_record("systemd invocation must be a byte array"))?
        .iter()
        .map(|value| {
            value
                .as_u64()
                .and_then(|value| u8::try_from(value).ok())
                .ok_or_else(|| invalid_record("systemd invocation byte is invalid"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let process_id = value["mainProcessId"]
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| invalid_record("systemd main process ID is invalid"))?;
    SystemdRuntimeObservation::new(
        parse_target(&value["target"])?,
        parse_systemd_manager(&value["manager"])?,
        SystemdUnitName::parse(string(value, "unit")?)
            .map_err(|error| invalid_record(&error.to_string()))?,
        crate::gateway::domain::value_objects::SystemdInvocationId::new(invocation)
            .map_err(|error| invalid_record(&error.to_string()))?,
        process_id,
        value["enabled"]
            .as_bool()
            .ok_or_else(|| invalid_record("systemd enabled state is invalid"))?,
    )
    .map_err(|error| invalid_record(&error.to_string()))
}

fn parse_observation_source(value: &Value) -> Result<LifecycleObservationSource, GatewayError> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid_record("observation source must be an object"))?;
    let kind = string(object, "kind")?;
    match kind.as_str() {
        "intent" => {
            object_exact(object, &["kind"])?;
            Ok(LifecycleObservationSource::Intent)
        }
        "effect" => {
            object_exact(object, &["kind", "planId", "stepId"])?;
            Ok(LifecycleObservationSource::Effect {
                plan_id: string(object, "planId")?,
                step_id: string(object, "stepId")?,
            })
        }
        _ => Err(invalid_record("unknown observation source")),
    }
}

fn parse_cleanup(value: &Value) -> Result<ReconciliationCleanupDecision, GatewayError> {
    match value.as_str() {
        Some("retain_prior") => Ok(ReconciliationCleanupDecision::RetainPrior),
        Some("clear_prior") => Ok(ReconciliationCleanupDecision::ClearPrior),
        Some("adopt_claimed") => Ok(ReconciliationCleanupDecision::AdoptClaimed),
        _ => Err(invalid_record("unknown cleanup decision")),
    }
}

fn parse_failed_phase(value: &str) -> Result<LifecycleFailedPhase, GatewayError> {
    match value {
        "intent_delivery" => Ok(LifecycleFailedPhase::IntentDelivery),
        "planning" => Ok(LifecycleFailedPhase::Planning),
        "native_dispatch" => Ok(LifecycleFailedPhase::NativeDispatch),
        "native_completion_delivery" => Ok(LifecycleFailedPhase::NativeCompletionDelivery),
        "observation" => Ok(LifecycleFailedPhase::Observation),
        "cleanup" => Ok(LifecycleFailedPhase::Cleanup),
        "outcome_delivery" => Ok(LifecycleFailedPhase::OutcomeDelivery),
        _ => Err(invalid_record("unknown failed phase")),
    }
}

fn parse_physical(value: &Value) -> Result<LifecyclePhysicalOutcome, GatewayError> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid_record("physical outcome must be an object"))?;
    let kind = string(object, "kind")?;
    match kind.as_str() {
        "confirmed" => {
            object_exact(object, &["kind", "incarnation"])?;
            Ok(LifecyclePhysicalOutcome::Confirmed(parse_identity(
                &object["incarnation"],
            )?))
        }
        "failed" => {
            object_exact(object, &["kind", "phase", "message"])?;
            Ok(LifecyclePhysicalOutcome::Failed {
                phase: parse_failed_phase(&string(object, "phase")?)?,
                message: string(object, "message")?,
            })
        }
        "stop_agents_settled" => {
            object_exact(object, &["kind", "intended", "command", "observed"])?;
            Ok(LifecyclePhysicalOutcome::StopAgentsSettled {
                intended: parse_identity(&object["intended"])?,
                command: parse_command_result(&object["command"])?,
                observed: parse_observation(&object["observed"])?,
            })
        }
        _ => Err(invalid_record("unknown physical outcome")),
    }
}

fn domain_record(stored: &StoredRecord) -> Result<LifecycleRecord, GatewayError> {
    let correlation = ReconciliationCorrelation::parse(stored.attempt_correlation.clone())
        .map_err(|error| invalid_record(&error.to_string()))?;
    let payload = object(
        &stored.payload,
        match record_kind(&stored.kind) {
            Some(LifecycleRecordKind::Intent) => &["origin", "target", "before"],
            Some(LifecycleRecordKind::JoinedRequest) => {
                &["originRequestCorrelation", "joinedRequest"]
            }
            Some(LifecycleRecordKind::EffectPlan) => {
                &["planId", "expectedBefore", "target", "primary", "cleanup"]
            }
            Some(LifecycleRecordKind::NativeAttempt) => &["planId", "stepId", "attempt"],
            Some(LifecycleRecordKind::EffectCompletion) => &["planId", "stepId", "result"],
            Some(LifecycleRecordKind::Observation) => &["source", "state"],
            Some(LifecycleRecordKind::Outcome) => &["physical", "lastConfirmed", "cleanup"],
            None => return Err(invalid_record("unknown record kind")),
        },
    )?;
    let payload = match record_kind(&stored.kind).expect("kind checked") {
        LifecycleRecordKind::Intent => {
            let (request_correlation, cause, initiator) = parse_request(&payload["origin"])?;
            LifecycleRecordPayload::Intent {
                request_correlation,
                cause,
                initiator,
                target: parse_target(&payload["target"])?,
                before: optional_identity(&payload["before"])?,
            }
        }
        LifecycleRecordKind::JoinedRequest => {
            let (request_correlation, cause, initiator) = parse_request(&payload["joinedRequest"])?;
            LifecycleRecordPayload::JoinedRequest {
                origin_request_correlation: ReconciliationCorrelation::parse(string(
                    payload,
                    "originRequestCorrelation",
                )?)
                .map_err(|error| invalid_record(&error.to_string()))?,
                request_correlation,
                cause,
                initiator,
            }
        }
        LifecycleRecordKind::EffectPlan => LifecycleRecordPayload::EffectPlan {
            plan_id: string(payload, "planId")?,
            expected_before: optional_identity(&payload["expectedBefore"])?,
            target: parse_target(&payload["target"])?,
            primary: parse_step(&payload["primary"])?,
            cleanup: payload["cleanup"]
                .as_array()
                .ok_or_else(|| invalid_record("cleanup steps must be an array"))?
                .iter()
                .map(parse_step)
                .collect::<Result<Vec<_>, _>>()?,
        },
        LifecycleRecordKind::NativeAttempt => LifecycleRecordPayload::NativeAttempt {
            plan_id: string(payload, "planId")?,
            step_id: string(payload, "stepId")?,
            attempt: parse_systemd_job_attempt(&payload["attempt"])?,
        },
        LifecycleRecordKind::EffectCompletion => LifecycleRecordPayload::EffectCompletion {
            plan_id: string(payload, "planId")?,
            step_id: string(payload, "stepId")?,
            result: parse_command_result(&payload["result"])?,
        },
        LifecycleRecordKind::Observation => LifecycleRecordPayload::Observation {
            source: parse_observation_source(&payload["source"])?,
            state: parse_observation(&payload["state"])?,
        },
        LifecycleRecordKind::Outcome => LifecycleRecordPayload::Outcome {
            physical: parse_physical(&payload["physical"])?,
            last_confirmed: if payload["lastConfirmed"].is_null() {
                None
            } else {
                Some(parse_observation(&payload["lastConfirmed"])?)
            },
            cleanup: parse_cleanup(&payload["cleanup"])?,
        },
    };
    LifecycleRecord::new(
        stored.service_namespace.clone(),
        correlation,
        stored.sequence,
        payload,
    )
    .map_err(|error| invalid_record(&error.to_string()))
}

fn record_kind(kind: &str) -> Option<LifecycleRecordKind> {
    [
        LifecycleRecordKind::Intent,
        LifecycleRecordKind::JoinedRequest,
        LifecycleRecordKind::EffectPlan,
        LifecycleRecordKind::NativeAttempt,
        LifecycleRecordKind::EffectCompletion,
        LifecycleRecordKind::Observation,
        LifecycleRecordKind::Outcome,
    ]
    .into_iter()
    .find(|candidate| candidate.file_name() == kind)
}

fn acquire_lock(
    directory: &PrivateDirectory,
    deadline: Option<Instant>,
    clock: &dyn MonotonicClock,
) -> Result<File, GatewayError> {
    let file = directory
        .open_file(OsStr::new(LOCK_FILE), OpenMode::OpenOrCreate)
        .map_err(|error| GatewayError::Registration(error.to_string()))?;
    let deadline = deadline.unwrap_or_else(|| clock.now() + Duration::from_secs(120));
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(file);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::WouldBlock || clock.now() >= deadline {
            return Err(GatewayError::Registration(format!(
                "Cannot acquire gateway lifecycle journal lock: {error}"
            )));
        }
        clock.wait(Duration::from_millis(50));
    }
}

fn load_records(directory: &PrivateDirectory) -> Result<Vec<StoredRecord>, GatewayError> {
    let mut records = Vec::new();
    for entry in directory
        .entries()
        .map_err(|error| GatewayError::Registration(error.to_string()))?
    {
        let entry = entry.map_err(|error| GatewayError::Registration(error.to_string()))?;
        if entry.name() == OsStr::new(LOCK_FILE) {
            continue;
        }
        if nessa_local_storage::is_private_temporary_name(entry.name()) {
            if entry.file_type() != PrivateFileType::RegularFile {
                return Err(GatewayError::Registration(
                    "Gateway lifecycle journal contains an unsafe temporary entry".into(),
                ));
            }
            let temporary = directory
                .open_file(entry.name(), OpenMode::ReadNonblocking)
                .map_err(|error| GatewayError::Registration(error.to_string()))?;
            directory
                .remove_file(entry.name(), &temporary)
                .map_err(|error| GatewayError::Registration(error.to_string()))?;
            directory
                .sync()
                .map_err(|error| GatewayError::Registration(error.to_string()))?;
            continue;
        }
        if entry.file_type() != PrivateFileType::RegularFile {
            return Err(GatewayError::Registration(
                "Gateway lifecycle journal contains an unsafe entry".into(),
            ));
        }
        let record = acknowledge_final_record(directory, entry.name(), None, None)?.0;
        let expected_name = record.file_name();
        if entry.name().to_str() != Some(expected_name.as_str()) {
            return Err(GatewayError::Registration(
                "Gateway lifecycle journal filename disagrees with its record".into(),
            ));
        }
        records.push(record);
    }
    records.sort_by(|left, right| {
        left.attempt_correlation
            .cmp(&right.attempt_correlation)
            .then(left.sequence.cmp(&right.sequence))
    });
    validate_stage_records(&records)?;
    Ok(records)
}

fn validate_stage_records(records: &[StoredRecord]) -> Result<(), GatewayError> {
    let mut unresolved = 0usize;
    let mut index = 0usize;
    while index < records.len() {
        let correlation = &records[index].attempt_correlation;
        let start = index;
        while index < records.len() && &records[index].attempt_correlation == correlation {
            records[index].validate_header()?;
            index += 1;
        }
        let chain = records[start..index]
            .iter()
            .map(domain_record)
            .collect::<Result<Vec<_>, _>>()?;
        let history = LifecycleHistory::restore(&chain)
            .map_err(|error| GatewayError::Registration(error.to_string()))?;
        if !history.is_terminal() {
            unresolved += 1;
        }
    }
    if unresolved > 1 {
        return Err(GatewayError::Registration(
            "Gateway lifecycle journal contains multiple unresolved attempts".into(),
        ));
    }
    Ok(())
}

fn unresolved_recovery(
    records: &[StoredRecord],
) -> Result<Option<(GatewayLifecycleRecovery, LifecycleHistory)>, GatewayError> {
    let mut index = 0usize;
    while index < records.len() {
        let correlation = &records[index].attempt_correlation;
        let start = index;
        while index < records.len() && &records[index].attempt_correlation == correlation {
            index += 1;
        }
        let chain = records[start..index]
            .iter()
            .map(domain_record)
            .collect::<Result<Vec<_>, _>>()?;
        let history = LifecycleHistory::restore(&chain)
            .map_err(|error| GatewayError::Registration(error.to_string()))?;
        if history.is_terminal() {
            continue;
        }
        let LifecycleRecordPayload::Intent {
            request_correlation,
            cause,
            initiator,
            target,
            before,
        } = chain[0].payload()
        else {
            return Err(GatewayError::Registration(
                "Gateway lifecycle recovery is missing its intent".into(),
            ));
        };
        let evidence = ReconciliationEvidence::new(*cause, *initiator)
            .map_err(|error| GatewayError::Registration(error.to_string()))?;
        let request = GatewayReconciliationRequest::new(request_correlation.clone(), evidence);
        let attempt =
            GatewayReconciliationAttempt::new(chain[0].attempt_correlation().clone(), request)?;
        let has_effect_plan = chain
            .iter()
            .any(|record| record.kind() == LifecycleRecordKind::EffectPlan);
        let pending_step = recovery_step(&chain, history.pending_observation_source().as_ref())?;
        return Ok(Some((
            GatewayLifecycleRecovery::new(
                attempt,
                target.clone(),
                before.clone(),
                has_effect_plan,
                history.latest_observation().cloned(),
                pending_step,
                history.pending_observation_source(),
            ),
            history,
        )));
    }
    Ok(None)
}

fn recovery_step(
    records: &[LifecycleRecord],
    pending_observation: Option<&LifecycleObservationSource>,
) -> Result<Option<GatewayLifecycleRecoveryStep>, GatewayError> {
    let mut primary_steps = Vec::new();
    let mut all_steps = Vec::new();
    let mut native_attempts = Vec::new();
    let mut completions = Vec::new();
    for record in records {
        match record.payload() {
            LifecycleRecordPayload::EffectPlan {
                plan_id,
                primary,
                cleanup,
                ..
            } => {
                primary_steps.push((plan_id.clone(), primary.clone()));
                all_steps.push((plan_id.clone(), primary.clone()));
                all_steps.extend(cleanup.iter().cloned().map(|step| (plan_id.clone(), step)));
            }
            LifecycleRecordPayload::EffectCompletion {
                plan_id,
                step_id,
                result,
            } => completions.push((plan_id.clone(), step_id.clone(), result.clone())),
            LifecycleRecordPayload::NativeAttempt {
                plan_id,
                step_id,
                attempt,
            } => native_attempts.push((plan_id.clone(), step_id.clone(), attempt.clone())),
            _ => {}
        }
    }
    if let Some(LifecycleObservationSource::Effect { plan_id, step_id }) = pending_observation {
        let step = all_steps
            .iter()
            .find(|(candidate_plan, step)| candidate_plan == plan_id && step.id() == step_id)
            .map(|(_, step)| step.clone())
            .ok_or_else(|| invalid_record("pending observation has no primary plan step"))?;
        let completion = completions
            .iter()
            .find(|(candidate_plan, candidate_step, _)| {
                candidate_plan == plan_id && candidate_step == step_id
            })
            .map(|(_, _, result)| result.clone())
            .ok_or_else(|| invalid_record("pending observation has no completion"))?;
        return Ok(Some(GatewayLifecycleRecoveryStep::new(
            plan_id.clone(),
            step,
            Some(completion),
            native_attempts
                .iter()
                .find(|(candidate_plan, candidate_step, _)| {
                    candidate_plan == plan_id && candidate_step == step_id
                })
                .map(|(_, _, attempt)| attempt.clone()),
        )));
    }
    let incomplete = primary_steps
        .into_iter()
        .filter(|(plan_id, step)| {
            !completions
                .iter()
                .any(|(candidate_plan, candidate_step, _)| {
                    candidate_plan == plan_id && candidate_step == step.id()
                })
        })
        .collect::<Vec<_>>();
    match incomplete.as_slice() {
        [] => Ok(None),
        [(plan_id, step)] => Ok(Some(GatewayLifecycleRecoveryStep::new(
            plan_id.clone(),
            step.clone(),
            None,
            native_attempts
                .iter()
                .find(|(candidate_plan, candidate_step, _)| {
                    candidate_plan == plan_id && candidate_step == step.id()
                })
                .map(|(_, _, attempt)| attempt.clone()),
        ))),
        _ => Err(GatewayError::Registration(
            "Gateway lifecycle recovery has multiple uncompleted primary effects".into(),
        )),
    }
}

fn acknowledge_final_record(
    directory: &PrivateDirectory,
    name: &OsStr,
    expected: Option<&StoredRecord>,
    retained_file: Option<&File>,
) -> Result<(StoredRecord, AuditDeliveryReceipt), GatewayError> {
    let mut file = match retained_file {
        Some(file) => file
            .try_clone()
            .map_err(|error| GatewayError::Registration(error.to_string()))?,
        None => directory
            .open_file(name, OpenMode::ReadNonblocking)
            .map_err(|error| GatewayError::Registration(error.to_string()))?,
    };
    let identity_file = file
        .try_clone()
        .map_err(|error| GatewayError::Registration(error.to_string()))?;
    let record = validate_record_acknowledgement(
        expected,
        || {
            directory
                .sync()
                .map_err(|error| GatewayError::Registration(error.to_string()))
        },
        || {
            directory
                .verify_binding()
                .map_err(|error| GatewayError::Registration(error.to_string()))
        },
        || {
            directory
                .named_file_is(name, &identity_file)
                .map_err(|error| GatewayError::Registration(error.to_string()))
        },
        || {
            file.seek(SeekFrom::Start(0))
                .map_err(|error| GatewayError::Registration(error.to_string()))?;
            let mut bytes = Vec::new();
            (&mut file)
                .take(MAX_RECORD_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| GatewayError::Registration(error.to_string()))?;
            Ok(bytes)
        },
    )?;
    let kind = record_kind(&record.kind).ok_or_else(|| {
        GatewayError::Registration("Unknown gateway lifecycle record kind".into())
    })?;
    let correlation = ReconciliationCorrelation::parse(record.attempt_correlation.clone())
        .map_err(|error| GatewayError::Registration(error.to_string()))?;
    let receipt = AuditDeliveryReceipt::new(correlation, record.sequence, kind);
    Ok((record, receipt))
}

fn validate_record_acknowledgement(
    expected: Option<&StoredRecord>,
    sync_directory: impl FnOnce() -> Result<(), GatewayError>,
    mut verify_binding: impl FnMut() -> Result<(), GatewayError>,
    mut name_matches: impl FnMut() -> Result<bool, GatewayError>,
    read: impl FnOnce() -> Result<Vec<u8>, GatewayError>,
) -> Result<StoredRecord, GatewayError> {
    sync_directory()?;
    verify_binding()?;
    if !name_matches()? {
        return Err(GatewayError::Registration(
            "Gateway lifecycle record identity changed during acknowledgement".into(),
        ));
    }
    let bytes = read()?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(GatewayError::Registration(
            "Gateway lifecycle record exceeds limit".into(),
        ));
    }
    let mut deserializer = serde_json::Deserializer::from_slice(&bytes);
    let unique = UniqueJson::deserialize(&mut deserializer)
        .map_err(|error| GatewayError::Registration(error.to_string()))?;
    deserializer
        .end()
        .map_err(|error| GatewayError::Registration(error.to_string()))?;
    let record: StoredRecord = serde_json::from_value(unique.0)
        .map_err(|error| GatewayError::Registration(error.to_string()))?;
    record.validate_header()?;
    verify_binding()?;
    if !name_matches()? {
        return Err(GatewayError::Registration(
            "Gateway lifecycle record identity changed during read-back".into(),
        ));
    }
    if expected.is_some_and(|expected| expected != &record) {
        return Err(GatewayError::Registration(
            "Gateway lifecycle retry disagrees with the published record".into(),
        ));
    }
    Ok(record)
}

impl FileJournalSession {
    fn check_deadline(&self) -> Result<(), GatewayError> {
        if self
            .deadline
            .is_some_and(|deadline| self.clock.now() >= deadline)
        {
            Err(GatewayError::Registration(
                "Gateway lifecycle journal deadline passed".into(),
            ))
        } else {
            Ok(())
        }
    }

    fn append(
        &self,
        namespace: String,
        domain_payload: LifecycleRecordPayload,
        payload: Value,
    ) -> Result<AuditDeliveryReceipt, JournalAppendError> {
        self.check_deadline()
            .map_err(JournalAppendError::Delivery)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| {
                GatewayError::Registration("Gateway lifecycle journal state is unavailable".into())
            })
            .map_err(JournalAppendError::Delivery)?;
        if state.terminal {
            return Err(JournalAppendError::Rejected(GatewayError::Registration(
                "Gateway lifecycle attempt is already settled".into(),
            )));
        }
        if state
            .namespace
            .as_ref()
            .is_some_and(|established| established != &namespace)
        {
            return Err(JournalAppendError::Rejected(GatewayError::Registration(
                "Gateway lifecycle attempt changed service namespace".into(),
            )));
        }
        let domain_record = LifecycleRecord::new(
            namespace.clone(),
            self.attempt.correlation().clone(),
            state.next_sequence,
            domain_payload,
        )
        .map_err(|error| {
            JournalAppendError::Rejected(GatewayError::Registration(error.to_string()))
        })?;
        let next_history = match &state.history {
            Some(history) => {
                let mut history = history.clone();
                history.append(&domain_record).map_err(|error| {
                    JournalAppendError::Rejected(GatewayError::Registration(error.to_string()))
                })?;
                history
            }
            None => LifecycleHistory::restore(std::slice::from_ref(&domain_record)).map_err(
                |error| JournalAppendError::Rejected(GatewayError::Registration(error.to_string())),
            )?,
        };
        let kind = domain_record.kind();
        let record = StoredRecord {
            service_namespace: namespace.clone(),
            attempt_correlation: self.attempt.correlation().as_str().to_owned(),
            sequence: state.next_sequence,
            kind: kind.file_name().into(),
            payload,
        };
        let name = record.file_name();
        let bytes = serde_json::to_vec(&record)
            .map_err(|error| GatewayError::Registration(error.to_string()))
            .map_err(JournalAppendError::Delivery)?;
        let mut temporary = self
            .directory
            .reserve_temp()
            .map_err(|error| GatewayError::Registration(error.to_string()))
            .map_err(JournalAppendError::Delivery)?;
        temporary
            .as_file_mut()
            .write_all(&bytes)
            .map_err(|error| GatewayError::Registration(error.to_string()))
            .map_err(JournalAppendError::Delivery)?;
        self.check_deadline()
            .map_err(JournalAppendError::Delivery)?;
        let published = temporary.publish_new(OsStr::new(&name));
        let receipt = match published {
            Ok(published) => {
                acknowledge_final_record(
                    &self.directory,
                    OsStr::new(&name),
                    Some(&record),
                    Some(published.as_file()),
                )
                .map_err(JournalAppendError::Delivery)?
                .1
            }
            Err(error) if error.source_error().kind() == io::ErrorKind::AlreadyExists => {
                acknowledge_final_record(&self.directory, OsStr::new(&name), Some(&record), None)
                    .map_err(JournalAppendError::Delivery)?
                    .1
            }
            Err(error) => {
                return Err(JournalAppendError::Delivery(GatewayError::Registration(
                    format!("Could not record gateway lifecycle: {error}"),
                )))
            }
        };
        self.check_deadline()
            .map_err(JournalAppendError::Delivery)?;
        state.namespace = Some(namespace);
        state.next_sequence += 1;
        state.terminal = kind == LifecycleRecordKind::Outcome;
        state.latest_observation = next_history.latest_observation().cloned();
        state.history = Some(next_history);
        self.advanced.notify_all();
        Ok(receipt)
    }

    fn namespace(&self) -> Result<String, GatewayError> {
        let wait = match self.deadline {
            Some(deadline) => deadline
                .checked_duration_since(self.clock.now())
                .ok_or_else(|| {
                    GatewayError::Registration("Gateway lifecycle journal deadline passed".into())
                })?,
            None => Duration::from_secs(120),
        };
        let (state, timeout) = self
            .advanced
            .wait_timeout_while(
                self.state.lock().map_err(|_| {
                    GatewayError::Registration(
                        "Gateway lifecycle journal state is unavailable".into(),
                    )
                })?,
                wait,
                |state| state.namespace.is_none(),
            )
            .map_err(|_| {
                GatewayError::Registration("Gateway lifecycle journal state is unavailable".into())
            })?;
        if timeout.timed_out() {
            return Err(GatewayError::Registration(
                "Gateway intent was not acknowledged before the journal deadline".into(),
            ));
        }
        state
            .namespace
            .clone()
            .ok_or_else(|| GatewayError::Registration("Gateway intent is not acknowledged".into()))
    }
}

impl GatewayReconciliationAudit for FileReconciliationAudit {
    fn open(
        self: Arc<Self>,
        attempt: &GatewayReconciliationAttempt,
        deadline: Option<Instant>,
    ) -> Result<Arc<dyn GatewayReconciliationJournalSession>, GatewayError> {
        let directory = self.open_directory()?;
        let lock = acquire_lock(&directory, deadline, self.clock.as_ref())?;
        let records = load_records(&directory)?;
        if deadline.is_some_and(|deadline| self.clock.now() >= deadline) {
            return Err(GatewayError::Registration(
                "Gateway lifecycle journal deadline passed during recovery".into(),
            ));
        }
        let recovery = unresolved_recovery(&records)?;
        let (session_attempt, namespace, next_sequence, history, latest_observation, recovery) =
            match recovery {
                Some((recovery, history)) => (
                    recovery.attempt().clone(),
                    Some(recovery.target().service().to_owned()),
                    history.next_sequence(),
                    Some(history.clone()),
                    history.latest_observation().cloned(),
                    Some(recovery),
                ),
                None => (attempt.clone(), None, 0, None, None, None),
            };
        Ok(Arc::new(FileJournalSession {
            directory,
            _lock: lock,
            attempt: session_attempt,
            state: Mutex::new(SessionState {
                namespace,
                next_sequence,
                terminal: false,
                history,
                latest_observation,
            }),
            advanced: Condvar::new(),
            deadline,
            clock: self.clock.clone(),
            recovery,
        }))
    }
}

impl GatewayReconciliationJournalSession for FileJournalSession {
    fn recovery(&self) -> Option<GatewayLifecycleRecovery> {
        self.recovery.clone()
    }

    fn intent(&self, intent: &GatewayReconciliationIntent) -> Result<(), GatewayError> {
        if intent.attempt() != &self.attempt {
            return Err(GatewayError::Registration(
                "Gateway intent belongs to a different journal session".into(),
            ));
        }
        self.append(
            intent.target().service().to_owned(),
            LifecycleRecordPayload::Intent {
                request_correlation: intent.attempt().origin().correlation().clone(),
                cause: intent.attempt().origin().evidence().cause(),
                initiator: intent.attempt().origin().evidence().initiator(),
                target: intent.target().clone(),
                before: intent.before().cloned(),
            },
            json!({
                "origin": request(intent.attempt().origin()),
                "target": target(intent.target()),
                "before": intent.before().map(identity),
            }),
        )
        .map_err(JournalAppendError::into_gateway_error)?;
        Ok(())
    }

    fn outcome(
        &self,
        outcome: &GatewayReconciliationOutcome,
    ) -> Result<(), GatewayReconciliationOutcomeError> {
        let physical = match outcome.effect() {
            GatewayReconciliationEffect::Confirmed { after, .. } => {
                LifecyclePhysicalOutcome::Confirmed(after.clone())
            }
            GatewayReconciliationEffect::Failed { error, .. } => LifecyclePhysicalOutcome::Failed {
                phase: outcome.failed_phase(),
                message: error.to_string(),
            },
            GatewayReconciliationEffect::RejectedReport { error, .. } => {
                LifecyclePhysicalOutcome::Failed {
                    phase: LifecycleFailedPhase::Observation,
                    message: error.to_string(),
                }
            }
        };
        let last_confirmed = self
            .state
            .lock()
            .map_err(|_| {
                GatewayError::Registration("Gateway lifecycle journal state is unavailable".into())
            })
            .map_err(GatewayReconciliationOutcomeError::Delivery)?
            .latest_observation
            .clone();
        let namespace = self
            .namespace()
            .map_err(GatewayReconciliationOutcomeError::Delivery)?;
        self.append(
            namespace,
            LifecycleRecordPayload::Outcome {
                physical: physical.clone(),
                last_confirmed: last_confirmed.clone(),
                cleanup: outcome.cleanup(),
            },
            json!({
                "physical":lifecycle_physical(&physical),
                "lastConfirmed":last_confirmed.as_ref().map(observation),
                "cleanup":cleanup(outcome.cleanup()),
            }),
        )
        .map_err(|error| match error {
            JournalAppendError::Rejected(error) => {
                GatewayReconciliationOutcomeError::Rejected(error)
            }
            JournalAppendError::Delivery(error) => {
                GatewayReconciliationOutcomeError::Delivery(error)
            }
        })?;
        Ok(())
    }

    fn joined(&self, joined: &GatewayReconciliationRequest) -> Result<(), GatewayError> {
        self.append(
            self.namespace()?,
            LifecycleRecordPayload::JoinedRequest {
                origin_request_correlation: self.attempt.origin().correlation().clone(),
                request_correlation: joined.correlation().clone(),
                cause: joined.evidence().cause(),
                initiator: joined.evidence().initiator(),
            },
            json!({
                "originRequestCorrelation": self.attempt.origin().correlation().as_str(),
                "joinedRequest": request(joined),
            }),
        )
        .map_err(JournalAppendError::into_gateway_error)?;
        Ok(())
    }

    fn effect_plan(
        &self,
        plan_id: &str,
        expected_before: Option<&ReconciliationIncarnation>,
        target_value: &ReconciliationTarget,
        primary: &LifecyclePlanStep,
        cleanup_steps: &[LifecyclePlanStep],
    ) -> Result<AuditDeliveryReceipt, GatewayError> {
        self.append(
            self.namespace()?,
            LifecycleRecordPayload::EffectPlan {
                plan_id: plan_id.into(),
                expected_before: expected_before.cloned(),
                target: target_value.clone(),
                primary: primary.clone(),
                cleanup: cleanup_steps.to_vec(),
            },
            json!({
                "planId": plan_id,
                "expectedBefore": expected_before.map(identity),
                "target": target(target_value),
                "primary": plan_step(primary),
                "cleanup": cleanup_steps.iter().map(plan_step).collect::<Vec<_>>(),
            }),
        )
        .map_err(JournalAppendError::into_gateway_error)
    }

    fn effect_completion(
        &self,
        plan_id: &str,
        step_id: &str,
        result: &LifecycleCommandResult,
    ) -> Result<AuditDeliveryReceipt, GatewayError> {
        self.append(
            self.namespace()?,
            LifecycleRecordPayload::EffectCompletion {
                plan_id: plan_id.into(),
                step_id: step_id.into(),
                result: result.clone(),
            },
            json!({"planId":plan_id, "stepId":step_id, "result":command_result(result)}),
        )
        .map_err(JournalAppendError::into_gateway_error)
    }

    fn native_attempt(
        &self,
        plan_id: &str,
        step_id: &str,
        attempt: &SystemdJobAttempt,
    ) -> Result<AuditDeliveryReceipt, GatewayError> {
        self.append(
            self.namespace()?,
            LifecycleRecordPayload::NativeAttempt {
                plan_id: plan_id.into(),
                step_id: step_id.into(),
                attempt: attempt.clone(),
            },
            json!({
                "planId":plan_id,
                "stepId":step_id,
                "attempt":systemd_job_attempt(attempt),
            }),
        )
        .map_err(JournalAppendError::into_gateway_error)
    }

    fn observation(
        &self,
        source: &LifecycleObservationSource,
        state: &LifecycleObservation,
    ) -> Result<AuditDeliveryReceipt, GatewayError> {
        self.append(
            self.namespace()?,
            LifecycleRecordPayload::Observation {
                source: source.clone(),
                state: state.clone(),
            },
            json!({"source":observation_source(source), "state":observation(state)}),
        )
        .map_err(JournalAppendError::into_gateway_error)
    }

    fn physical_outcome(
        &self,
        physical: &LifecyclePhysicalOutcome,
        last_confirmed: Option<&LifecycleObservation>,
        cleanup_value: ReconciliationCleanupDecision,
    ) -> Result<AuditDeliveryReceipt, GatewayError> {
        self.append(
            self.namespace()?,
            LifecycleRecordPayload::Outcome {
                physical: physical.clone(),
                last_confirmed: last_confirmed.cloned(),
                cleanup: cleanup_value,
            },
            json!({
                "physical": lifecycle_physical(physical),
                "lastConfirmed": last_confirmed.map(observation),
                "cleanup": cleanup(cleanup_value),
            }),
        )
        .map_err(JournalAppendError::into_gateway_error)
    }
}

fn plan_step(step: &LifecyclePlanStep) -> Value {
    json!({
        "id":step.id(),
        "effect":lifecycle_effect(step.effect()),
        "predicate":effect_predicate(step.predicate()),
    })
}

fn lifecycle_effect(effect: &LifecycleEffect) -> Value {
    match effect {
        LifecycleEffect::StageRuntime { fingerprint } => {
            json!({"kind":"stage_runtime", "fingerprint":fingerprint})
        }
        LifecycleEffect::RequestRetirement { incarnation } => {
            json!({"kind":"request_retirement", "incarnation":identity(incarnation)})
        }
        LifecycleEffect::RequestSystemdRetirement {
            incarnation,
            request_id,
        } => json!({
            "kind":"request_systemd_retirement",
            "incarnation":identity(incarnation),
            "requestId":request_id,
        }),
        LifecycleEffect::UnloadService { service } => {
            json!({"kind":"unload_service", "service":service})
        }
        LifecycleEffect::PublishServiceDefinition { target } => {
            json!({"kind":"publish_service_definition", "target":self::target(target)})
        }
        LifecycleEffect::PublishSystemdServiceDefinition {
            target,
            definition_digest,
        } => json!({
            "kind":"publish_systemd_service_definition",
            "target":self::target(target),
            "definitionDigest":definition_digest,
        }),
        LifecycleEffect::CreateGatewayDataDirectory { target } => {
            json!({"kind":"create_gateway_data_directory", "target":self::target(target)})
        }
        LifecycleEffect::ReloadSystemdManager { manager, unit } => json!({
            "kind":"reload_systemd_manager",
            "manager":systemd_manager(manager),
            "unit":unit.as_str(),
        }),
        LifecycleEffect::CreateSystemdWantsDirectory { target } => {
            json!({"kind":"create_systemd_wants_directory", "target":self::target(target)})
        }
        LifecycleEffect::PublishSystemdWantsLink { target } => {
            json!({"kind":"publish_systemd_wants_link", "target":self::target(target)})
        }
        LifecycleEffect::StartSystemdUnit {
            manager,
            unit,
            mode,
        } => json!({
            "kind":"start_systemd_unit",
            "manager":systemd_manager(manager),
            "unit":unit.as_str(),
            "mode":mode.as_str(),
        }),
        LifecycleEffect::StopSystemdUnit {
            manager,
            unit,
            mode,
        } => json!({
            "kind":"stop_systemd_unit",
            "manager":systemd_manager(manager),
            "unit":unit.as_str(),
            "mode":mode.as_str(),
        }),
        LifecycleEffect::BootstrapService { target } => {
            json!({"kind":"bootstrap_service", "target":self::target(target)})
        }
        LifecycleEffect::AdoptReadyIncarnation { target } => {
            json!({"kind":"adopt_ready_incarnation", "target":self::target(target)})
        }
        LifecycleEffect::PruneRuntime { fingerprint } => {
            json!({"kind":"prune_runtime", "fingerprint":fingerprint})
        }
        LifecycleEffect::RemoveStagingRuntime { generation } => {
            json!({"kind":"remove_staging_runtime", "generation":generation})
        }
        LifecycleEffect::StopAgents { incarnation } => {
            json!({"kind":"stop_agents", "incarnation":identity(incarnation)})
        }
    }
}

fn systemd_manager(manager: &SystemdManagerIdentity) -> Value {
    json!({
        "uniqueName":manager.unique_name(),
        "processId":manager.process_id(),
        "userId":manager.user_id(),
    })
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn systemd_job_attempt(attempt: &SystemdJobAttempt) -> Value {
    json!({
        "manager":systemd_manager(attempt.manager()),
        "operation":match attempt.operation() {
            SystemdJobOperation::Start => "start",
            SystemdJobOperation::Stop => "stop",
        },
        "mode":attempt.mode().as_str(),
        "unit":attempt.unit().as_str(),
        "objectPath":attempt.object_path(),
        "jobId":attempt.job_id(),
    })
}

fn effect_predicate(predicate: &LifecycleEffectPredicate) -> Value {
    match predicate {
        LifecycleEffectPredicate::Always => json!({"kind":"always"}),
        LifecycleEffectPredicate::PrimaryReturned => json!({"kind":"primary_returned"}),
        LifecycleEffectPredicate::PrimaryAccepted => json!({"kind":"primary_accepted"}),
        LifecycleEffectPredicate::ObservationMatches(incarnation) => {
            json!({"kind":"observation_matches", "incarnation":identity(incarnation)})
        }
    }
}

fn command_result(result: &LifecycleCommandResult) -> Value {
    match result {
        LifecycleCommandResult::Accepted => json!({"kind":"accepted"}),
        LifecycleCommandResult::Rejected(message) => {
            json!({"kind":"rejected", "message":message})
        }
        LifecycleCommandResult::Failed(message) => json!({"kind":"failed", "message":message}),
        LifecycleCommandResult::Indeterminate(message) => {
            json!({"kind":"indeterminate", "message":message})
        }
    }
}

fn observation_source(source: &LifecycleObservationSource) -> Value {
    match source {
        LifecycleObservationSource::Intent => json!({"kind":"intent"}),
        LifecycleObservationSource::Effect { plan_id, step_id } => {
            json!({"kind":"effect", "planId":plan_id, "stepId":step_id})
        }
    }
}

fn observation(state: &LifecycleObservation) -> Value {
    json!({
        "version":state.version(),
        "incarnation":state.incarnation().map(identity),
        "targetArtifactPresent":state.target_artifact_present(),
        "systemd":state.systemd().map(systemd_runtime),
        "systemdState":state.systemd_state().map(SystemdUnitState::as_str),
    })
}

fn systemd_runtime(state: &SystemdRuntimeObservation) -> Value {
    json!({
        "target":target(state.target()),
        "manager":systemd_manager(state.manager()),
        "unit":state.unit().as_str(),
        "invocation":state.invocation().bytes(),
        "mainProcessId":state.main_process_id(),
        "enabled":state.enabled(),
    })
}

fn lifecycle_physical(physical: &LifecyclePhysicalOutcome) -> Value {
    match physical {
        LifecyclePhysicalOutcome::Confirmed(incarnation) => {
            json!({"kind":"confirmed", "incarnation":identity(incarnation)})
        }
        LifecyclePhysicalOutcome::StopAgentsSettled {
            intended,
            command,
            observed,
        } => json!({
            "kind":"stop_agents_settled",
            "intended":identity(intended),
            "command":command_result(command),
            "observed":observation(observed),
        }),
        LifecyclePhysicalOutcome::Failed { phase, message } => {
            json!({"kind":"failed", "phase":failed_phase(*phase), "message":message})
        }
    }
}

fn failed_phase(phase: LifecycleFailedPhase) -> &'static str {
    match phase {
        LifecycleFailedPhase::IntentDelivery => "intent_delivery",
        LifecycleFailedPhase::Planning => "planning",
        LifecycleFailedPhase::NativeDispatch => "native_dispatch",
        LifecycleFailedPhase::NativeCompletionDelivery => "native_completion_delivery",
        LifecycleFailedPhase::Observation => "observation",
        LifecycleFailedPhase::Cleanup => "cleanup",
        LifecycleFailedPhase::OutcomeDelivery => "outcome_delivery",
    }
}

fn request(request: &GatewayReconciliationRequest) -> Value {
    json!({
        "correlation": request.correlation().as_str(),
        "cause": cause(request.evidence().cause()),
        "initiator": initiator(request.evidence().initiator()),
    })
}

fn cause(cause: ReconciliationCause) -> &'static str {
    match cause {
        ReconciliationCause::Startup => "startup",
        ReconciliationCause::CredentialLoad => "credential_load",
        ReconciliationCause::ExplicitRetry => "explicit_retry",
        ReconciliationCause::ClaudeConfigurationChanged => "claude_configuration_changed",
        ReconciliationCause::DesktopQuitPolicy => "desktop_quit_policy",
    }
}

fn initiator(initiator: ReconciliationInitiator) -> &'static str {
    match initiator {
        ReconciliationInitiator::DesktopHost => "desktop_host",
        ReconciliationInitiator::BundledSurface(BundledSurface::Main) => "main_window",
        ReconciliationInitiator::BundledSurface(BundledSurface::Setup) => "setup_window",
    }
}

fn cleanup(cleanup: ReconciliationCleanupDecision) -> &'static str {
    match cleanup {
        ReconciliationCleanupDecision::RetainPrior => "retain_prior",
        ReconciliationCleanupDecision::ClearPrior => "clear_prior",
        ReconciliationCleanupDecision::AdoptClaimed => "adopt_claimed",
    }
}

fn target(target: &ReconciliationTarget) -> Value {
    json!({
        "service": target.service(),
        "runtimeFingerprint": target.runtime_fingerprint(),
        "serviceGeneration": target.service_generation(),
    })
}

fn identity(gateway: &ReconciliationIncarnation) -> Value {
    json!({
        "service": gateway.target().service(),
        "runtimeFingerprint": gateway.target().runtime_fingerprint(),
        "runtimeInstance": gateway.runtime_instance(),
        "serviceGeneration": gateway.target().service_generation(),
        "processId": gateway.process_id(),
        "port": gateway.port(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::{
        application::{GatewayReconciliationEffectTiming, GatewayReconciliationIntentDelivery},
        domain::value_objects::{ReconciliationHistoryFact, SystemdInvocationId},
    };
    use std::{
        cell::Cell,
        fs,
        sync::atomic::{AtomicUsize, Ordering},
    };

    struct AdvancingClock {
        now: Mutex<Instant>,
        waits: AtomicUsize,
    }

    impl MonotonicClock for AdvancingClock {
        fn now(&self) -> Instant {
            *self.now.lock().unwrap()
        }

        fn wait(&self, duration: Duration) {
            self.waits.fetch_add(1, Ordering::SeqCst);
            let mut now = self.now.lock().unwrap();
            *now += duration;
        }
    }

    fn stored(correlation: &str, sequence: u64, kind: LifecycleRecordKind) -> StoredRecord {
        let observation = json!({
            "version":1,
            "incarnation":null,
            "targetArtifactPresent":false,
            "systemd":null,
            "systemdState":null,
        });
        let payload = match kind {
            LifecycleRecordKind::Intent => json!({
                "origin":{
                    "correlation":"00000000-0000-4000-8000-000000000099",
                    "cause":"startup",
                    "initiator":"desktop_host",
                },
                "target":{
                    "service":"gui/501/so.nessa.gateway.prod",
                    "runtimeFingerprint":"a".repeat(64),
                    "serviceGeneration":"b".repeat(64),
                },
                "before":null,
            }),
            LifecycleRecordKind::EffectPlan => json!({
                "planId":"stage-runtime",
                "expectedBefore":null,
                "target":{
                    "service":"gui/501/so.nessa.gateway.prod",
                    "runtimeFingerprint":"a".repeat(64),
                    "serviceGeneration":"b".repeat(64),
                },
                "primary":{
                    "id":"primary",
                    "effect":{"kind":"stage_runtime", "fingerprint":"a".repeat(64)},
                    "predicate":{"kind":"always"},
                },
                "cleanup":[],
            }),
            LifecycleRecordKind::EffectCompletion => json!({
                "planId":"stage-runtime",
                "stepId":"primary",
                "result":{"kind":"accepted"},
            }),
            LifecycleRecordKind::Observation => json!({
                "source":{"kind":"intent"},
                "state":observation,
            }),
            LifecycleRecordKind::Outcome => json!({
                "physical":{"kind":"failed", "phase":"planning", "message":"closed"},
                "lastConfirmed":observation,
                "cleanup":"retain_prior",
            }),
            _ => panic!("unsupported stored-record fixture kind"),
        };
        StoredRecord {
            service_namespace: "gui/501/so.nessa.gateway.prod".into(),
            attempt_correlation: correlation.into(),
            sequence,
            kind: kind.file_name().into(),
            payload,
        }
    }

    #[test]
    fn systemd_job_and_runtime_evidence_round_trip_without_dropping_identity_fields() {
        let unit = SystemdUnitName::parse("nessa-gateway-prod.service".into()).unwrap();
        let manager = SystemdManagerIdentity::new(":1.42".into(), 42, 501).unwrap();
        let attempt = SystemdJobAttempt::new(
            manager.clone(),
            SystemdJobOperation::Start,
            SystemdJobMode::Fail,
            unit.clone(),
            "/org/freedesktop/systemd1/job/19".into(),
            19,
        )
        .unwrap();
        assert_eq!(
            parse_systemd_job_attempt(&systemd_job_attempt(&attempt)).unwrap(),
            attempt
        );

        let target =
            ReconciliationTarget::new(unit.as_str().into(), "a".repeat(64), "b".repeat(64))
                .unwrap();
        let runtime = SystemdRuntimeObservation::new(
            target,
            manager,
            unit,
            SystemdInvocationId::new(vec![7; 16]).unwrap(),
            99,
            true,
        )
        .unwrap();
        assert_eq!(
            parse_systemd_runtime(&systemd_runtime(&runtime)).unwrap(),
            runtime
        );

        for state in [
            SystemdUnitState::Absent,
            SystemdUnitState::Inactive,
            SystemdUnitState::Activating,
            SystemdUnitState::Deactivating,
            SystemdUnitState::Failed,
            SystemdUnitState::Unknown,
        ] {
            let observed = LifecycleObservation::with_systemd_state(1, false, state).unwrap();
            assert_eq!(
                parse_observation(&observation(&observed)).unwrap(),
                observed
            );
        }
    }

    #[test]
    fn systemd_retirement_effect_round_trips_its_request_correlation() {
        let value = json!({
            "kind":"request_systemd_retirement",
            "incarnation":{
                "service":"nessa-gateway-prod.service",
                "runtimeFingerprint":"a".repeat(64),
                "runtimeInstance":"550e8400-e29b-41d4-a716-446655440001",
                "serviceGeneration":"b".repeat(64),
                "processId":41,
                "port":7420,
            },
            "requestId":"550e8400-e29b-41d4-a716-446655440000",
        });
        let effect = parse_effect(&value).unwrap();
        assert_eq!(lifecycle_effect(&effect), value);
    }

    #[test]
    fn stage_validation_refuses_multiple_unresolved_attempts() {
        let first = "00000000-0000-4000-8000-000000000001";
        let second = "00000000-0000-4000-8000-000000000002";
        let records = vec![
            stored(first, 0, LifecycleRecordKind::Intent),
            stored(second, 0, LifecycleRecordKind::Intent),
        ];
        assert!(validate_stage_records(&records).is_err());
    }

    #[test]
    fn stage_validation_allows_settled_history_before_one_unresolved_attempt() {
        let settled = "00000000-0000-4000-8000-000000000001";
        let unresolved = "00000000-0000-4000-8000-000000000002";
        let records = vec![
            stored(settled, 0, LifecycleRecordKind::Intent),
            stored(settled, 1, LifecycleRecordKind::Observation),
            stored(settled, 2, LifecycleRecordKind::Outcome),
            stored(unresolved, 0, LifecycleRecordKind::Intent),
        ];
        validate_stage_records(&records).expect("one unresolved attempt is valid");
        let (recovery, history) = unresolved_recovery(&records)
            .unwrap()
            .expect("unresolved attempt is restored");
        assert_eq!(recovery.attempt().correlation().as_str(), unresolved);
        assert_eq!(recovery.target().service(), "gui/501/so.nessa.gateway.prod");
        assert!(!recovery.has_effect_plan());
        assert_eq!(history.next_sequence(), 1);
    }

    #[test]
    fn recovery_retains_the_exact_step_before_and_after_completion() {
        let attempt = "00000000-0000-4000-8000-000000000002";
        let planned = vec![
            stored(attempt, 0, LifecycleRecordKind::Intent),
            stored(attempt, 1, LifecycleRecordKind::EffectPlan),
        ];
        let completed = vec![
            planned[0].clone(),
            planned[1].clone(),
            stored(attempt, 2, LifecycleRecordKind::EffectCompletion),
        ];

        let (planned_recovery, _) = unresolved_recovery(&planned).unwrap().unwrap();
        let planned_step = planned_recovery.pending_step().unwrap();
        assert_eq!(planned_step.plan_id(), "stage-runtime");
        assert_eq!(planned_step.step().id(), "primary");
        assert_eq!(planned_step.completion(), None);

        let (completed_recovery, _) = unresolved_recovery(&completed).unwrap().unwrap();
        assert_eq!(
            completed_recovery.pending_step().unwrap().completion(),
            Some(&LifecycleCommandResult::Accepted)
        );
    }

    #[test]
    fn stage_validation_refuses_cross_record_namespace_mismatch() {
        let correlation = "00000000-0000-4000-8000-000000000001";
        let first = stored(correlation, 0, LifecycleRecordKind::Intent);
        let mut second = stored(correlation, 1, LifecycleRecordKind::Outcome);
        second.service_namespace = "gui/501/so.nessa.gateway.other".into();
        assert!(validate_stage_records(&[first, second]).is_err());
    }

    #[test]
    fn stage_scan_removes_only_regular_private_temporaries() {
        let temporary = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let root = temporary.path().join("Nessa");
        let journal = root.join(DIRECTORY);
        nessa_local_storage::create_private_directory_path(&root).unwrap();
        nessa_local_storage::create_private_directory_path(&journal).unwrap();
        let directory = PrivateDirectory::open_path(&root, &journal).unwrap();
        let name = OsStr::new(".nessa-0123456789abcdef0123456789abcdef.tmp");
        directory.open_file(name, OpenMode::CreateNew).unwrap();

        assert!(load_records(&directory).unwrap().is_empty());
        assert!(!journal.join(name).exists());

        fs::create_dir(journal.join(name)).unwrap();
        assert!(load_records(&directory).is_err());
        assert!(journal.join(name).is_dir());
    }

    #[test]
    fn stage_lock_retry_uses_the_injected_clock_wait() {
        let temporary = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let root = temporary.path().join("Nessa");
        let journal = root.join(DIRECTORY);
        nessa_local_storage::create_private_directory_path(&root).unwrap();
        nessa_local_storage::create_private_directory_path(&journal).unwrap();
        let directory = PrivateDirectory::open_path(&root, &journal).unwrap();
        let start = Instant::now();
        let clock = AdvancingClock {
            now: Mutex::new(start),
            waits: AtomicUsize::new(0),
        };
        let _held = acquire_lock(&directory, None, &clock).unwrap();

        assert!(
            acquire_lock(&directory, Some(start + Duration::from_millis(100)), &clock,).is_err()
        );
        assert_eq!(clock.waits.load(Ordering::SeqCst), 2);
        assert_eq!(clock.now(), start + Duration::from_millis(100));
    }

    #[test]
    fn acknowledgement_requires_each_directory_sync_and_exact_retry_can_recover() {
        let record = stored(
            "00000000-0000-4000-8000-000000000001",
            0,
            LifecycleRecordKind::Intent,
        );
        let bytes = serde_json::to_vec(&record).unwrap();
        let syncs = Cell::new(0usize);
        let acknowledge = || {
            validate_record_acknowledgement(
                Some(&record),
                || {
                    syncs.set(syncs.get() + 1);
                    if syncs.get() == 1 {
                        Err(GatewayError::Registration("injected sync failure".into()))
                    } else {
                        Ok(())
                    }
                },
                || Ok(()),
                || Ok(true),
                || Ok(bytes.clone()),
            )
        };

        assert!(acknowledge().is_err());
        assert_eq!(acknowledge().unwrap(), record);
        assert_eq!(syncs.get(), 2);

        for _ in 0..2 {
            assert!(validate_record_acknowledgement(
                Some(&record),
                || Err(GatewayError::Registration("persistent sync failure".into())),
                || Ok(()),
                || Ok(true),
                || Ok(bytes.clone()),
            )
            .is_err());
        }
    }

    #[test]
    fn acknowledgement_refuses_name_replacement_before_and_after_readback() {
        let record = stored(
            "00000000-0000-4000-8000-000000000001",
            0,
            LifecycleRecordKind::Intent,
        );
        let bytes = serde_json::to_vec(&record).unwrap();
        for replaced_at in [1usize, 2] {
            let checks = Cell::new(0usize);
            let result = validate_record_acknowledgement(
                Some(&record),
                || Ok(()),
                || Ok(()),
                || {
                    checks.set(checks.get() + 1);
                    Ok(checks.get() != replaced_at)
                },
                || Ok(bytes.clone()),
            );
            assert!(result.is_err());
            assert_eq!(checks.get(), replaced_at);
        }
    }

    #[test]
    fn acknowledgement_refuses_visible_but_nonidentical_retry_content() {
        let expected = stored(
            "00000000-0000-4000-8000-000000000001",
            0,
            LifecycleRecordKind::Intent,
        );
        let mut changed = expected.clone();
        changed.service_namespace = "gui/501/so.nessa.gateway.changed".into();

        assert!(validate_record_acknowledgement(
            Some(&expected),
            || Ok(()),
            || Ok(()),
            || Ok(true),
            || Ok(serde_json::to_vec(&changed).unwrap()),
        )
        .is_err());
    }

    #[test]
    fn contradictory_outcome_is_classified_as_rejected_by_the_file_adapter() {
        let temporary = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let root = temporary.path().join("Nessa");
        let clock = Arc::new(AdvancingClock {
            now: Mutex::new(Instant::now()),
            waits: AtomicUsize::new(0),
        });
        let request = GatewayReconciliationRequest::new(
            ReconciliationCorrelation::parse("00000000-0000-4000-8000-000000000101".into())
                .unwrap(),
            ReconciliationEvidence::new(
                ReconciliationCause::Startup,
                ReconciliationInitiator::DesktopHost,
            )
            .unwrap(),
        );
        let attempt = GatewayReconciliationAttempt::new(
            ReconciliationCorrelation::parse("00000000-0000-4000-8000-000000000102".into())
                .unwrap(),
            request,
        )
        .unwrap();
        let prior_target = ReconciliationTarget::new(
            "gui/501/so.nessa.gateway.prod".into(),
            "c".repeat(64),
            "d".repeat(64),
        )
        .unwrap();
        let prior = ReconciliationIncarnation::new(
            prior_target,
            "00000000-0000-4000-8000-000000000103".into(),
            103,
            7420,
        )
        .unwrap();
        let target = ReconciliationTarget::new(
            "gui/501/so.nessa.gateway.prod".into(),
            "a".repeat(64),
            "b".repeat(64),
        )
        .unwrap();
        let intent =
            GatewayReconciliationIntent::new(attempt.clone(), target, Some(prior.clone())).unwrap();
        let session = Arc::new(FileReconciliationAudit::new(Some(root), clock))
            .open(&attempt, None)
            .unwrap();
        session.intent(&intent).unwrap();
        session
            .observation(
                &LifecycleObservationSource::Intent,
                &LifecycleObservation::new(1, Some(prior), false),
            )
            .unwrap();
        let outcome = GatewayReconciliationOutcome::assess(
            intent,
            GatewayReconciliationIntentDelivery::Acknowledged,
            GatewayReconciliationEffectTiming::AfterIntentAcknowledgement,
            vec![
                ReconciliationHistoryFact::RetirementAcknowledged,
                ReconciliationHistoryFact::OldServiceUnloaded,
            ],
            LifecycleFailedPhase::Planning,
            Err(GatewayError::Registration("planning failed".into())),
        );
        assert_eq!(outcome.cleanup(), ReconciliationCleanupDecision::ClearPrior);

        assert!(matches!(
            session.outcome(&outcome),
            Err(GatewayReconciliationOutcomeError::Rejected(_))
        ));
    }

    #[test]
    fn acknowledgement_rejects_duplicate_nested_fields_before_value_decoding() {
        let record = stored(
            "00000000-0000-4000-8000-000000000001",
            0,
            LifecycleRecordKind::Intent,
        );
        let encoded = serde_json::to_string(&record).unwrap();
        let duplicate = encoded.replacen(
            "\"service\":\"gui/501/so.nessa.gateway.prod\"",
            "\"service\":\"gui/501/so.nessa.gateway.changed\",\"service\":\"gui/501/so.nessa.gateway.prod\"",
            1,
        );
        assert_ne!(duplicate, encoded);

        let error = validate_record_acknowledgement(
            None,
            || Ok(()),
            || Ok(()),
            || Ok(true),
            || Ok(duplicate.as_bytes().to_vec()),
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("duplicate gateway lifecycle JSON field"));
    }
}
