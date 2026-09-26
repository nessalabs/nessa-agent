//! Validates the gateway's durable account of a non-retryable startup failure.

use crate::gateway::domain::value_objects::{
    ReconciliationTarget, StartupFailureRecoveryAuthority,
};
use nessa_local_storage::OpenMode;
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::Path,
    sync::LazyLock,
};

const RECORD_FILE: &str = "gateway-startup-failure.json";
const RECORD_MAX_BYTES: u64 = 65_536;
const EXIT_CODES_JSON: &str = include_str!("../../../../protocol/defaults/gateway-exit-codes.json");

#[derive(Debug, Deserialize)]
struct GatewayExitCodes {
    codes: BTreeMap<String, u8>,
    #[serde(rename = "recordedStartupFailureReasons")]
    recorded_startup_failure_reasons: BTreeSet<String>,
}

static CODES: LazyLock<GatewayExitCodes> = LazyLock::new(|| {
    serde_json::from_str(EXIT_CODES_JSON).expect("bundled gateway-exit-codes.json must parse")
});

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RecordedFailure {
    reason: String,
    exit_code: u8,
    message: String,
    service_generation: String,
    process_id: u32,
}

impl RecordedFailure {
    pub(super) fn belongs_to(&self, generation: &str) -> bool {
        self.service_generation == generation
    }
    pub(super) fn reason(&self) -> &str {
        &self.reason
    }
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(super) fn process_id(&self) -> u32 {
        self.process_id
    }
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(super) fn authority(
        &self,
        target: ReconciliationTarget,
    ) -> Option<StartupFailureRecoveryAuthority> {
        validated_authority(self.reason.clone(), self.exit_code, target, self.process_id)
    }
    pub(super) fn describe(&self) -> String {
        format!(
            "gateway recorded a startup failure it will not retry: {} (reason {}, code {}, pid {})",
            self.message, self.reason, self.exit_code, self.process_id
        )
    }
}

pub(super) fn recorded_failure(logs: &Path) -> Option<RecordedFailure> {
    let file =
        nessa_local_storage::open(&logs.join(RECORD_FILE), OpenMode::ReadNonblocking).ok()?;
    let mut bytes = Vec::new();
    file.take(RECORD_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    parse_record(&bytes)
}

pub(super) fn forget_recorded_failure(logs: &Path) {
    match std::fs::remove_file(logs.join(RECORD_FILE)) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => eprintln!("[nessa] could not clear the gateway's startup failure: {error}"),
    }
}

pub(super) fn parse_record(bytes: &[u8]) -> Option<RecordedFailure> {
    if bytes.len() as u64 > RECORD_MAX_BYTES {
        return None;
    }
    let record: RecordedFailure = serde_json::from_slice(bytes).ok()?;
    (record.process_id != 0
        && CODES
            .recorded_startup_failure_reasons
            .contains(&record.reason)
        && CODES.codes.get(&record.reason) == Some(&record.exit_code))
    .then_some(record)
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(super) fn validated_authority(
    reason: String,
    exit_code: u8,
    target: ReconciliationTarget,
    process_id: u32,
) -> Option<StartupFailureRecoveryAuthority> {
    (CODES.recorded_startup_failure_reasons.contains(&reason)
        && CODES.codes.get(&reason) == Some(&exit_code))
    .then(|| StartupFailureRecoveryAuthority::new(target, reason, exit_code, process_id).ok())
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GENERATION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn bytes(reason: &str, code: u8) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "reason": reason,
            "exitCode": code,
            "message": "failed",
            "serviceGeneration": GENERATION,
            "processId": 41,
        }))
        .unwrap()
    }

    #[test]
    fn only_the_shared_closed_reason_set_can_become_authority() {
        for (reason, code) in &CODES.codes {
            assert_eq!(
                parse_record(&bytes(reason, *code)).is_some(),
                CODES.recorded_startup_failure_reasons.contains(reason),
                "{reason}"
            );
        }
        assert_eq!(parse_record(&bytes("configuration", 28)), None);
        assert_eq!(parse_record(&bytes("unknown", 99)), None);
        assert_eq!(
            parse_record(
                serde_json::json!({
                    "reason":"configuration", "exitCode":20, "message":"failed",
                    "serviceGeneration":GENERATION, "processId":0,
                })
                .to_string()
                .as_bytes()
            ),
            None
        );
    }
}
