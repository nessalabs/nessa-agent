//! Offline inspection constructs only the verifier's existing read capabilities.

use std::ffi::OsString;
use std::io::Write;

use serde::Serialize;

use super::backup_output::print;
use crate::backup::application::{
    plan_offline_restore, OfflineRestorePlan, VerificationFailure, VerificationLimits,
};
use crate::backup::entrypoint::{parse_verification, VerificationCommand, VerificationUsageError};
use crate::backup::infrastructure::{read_verification_key, DirectoryVerificationSource};

const HELP: &str = "Usage: nessa-backup-plan --bundle ABSOLUTE_PATH --mac-key-file ABSOLUTE_PATH\n\nInspects a verified cut and reports unresolved restore prerequisites.\nNo import, quarantine, activation or policy override is implemented.\nExit 0 means an inspection report, never restore readiness or authority.\n";

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum PlanningOutput {
    Inspected { report: OfflineRestorePlan },
    Failed { reason: VerificationFailure },
    Usage,
}

/// Print an offline report without profile bootstrap or any activation adapter.
/// Arguments reuse the verifier's strict explicit-input parser; no plan input,
/// destination, activation option or implicit key/profile lookup is accepted.
pub fn run_restore_planner(arguments: &[OsString], output: &mut dyn Write) -> u8 {
    let command = match parse_verification(arguments) {
        Ok(command) => command,
        Err(VerificationUsageError) => return print(output, &PlanningOutput::Usage, 2),
    };
    let VerificationCommand::Verify {
        bundle,
        mac_key_file,
    } = command
    else {
        return if output.write_all(HELP.as_bytes()).is_ok() {
            0
        } else {
            1
        };
    };
    let result = read_verification_key(&mac_key_file).and_then(|key| {
        let source = DirectoryVerificationSource::open(bundle)?;
        plan_offline_restore(&source, &key, VerificationLimits::default())
    });
    match result {
        Ok(report) => print(output, &PlanningOutput::Inspected { report }, 0),
        Err(reason) => print(output, &PlanningOutput::Failed { reason }, 1),
    }
}
