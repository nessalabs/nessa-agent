//! The standalone verifier only constructs read capabilities for its two inputs.

use std::ffi::OsString;
use std::io::Write;

use serde::Serialize;

use crate::backup::application::{
    verify_bundle, VerificationFailure, VerificationLimits, VerificationReport,
};
use crate::backup::entrypoint::{
    parse_verification, VerificationCommand, VerificationUsageError, VERIFY_HELP,
};
use crate::backup::infrastructure::{
    read_verification_key, DirectoryVerificationSource, ExportFormat,
};

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum VerificationOutput {
    Verified { report: VerificationReport },
    Failed { reason: VerificationFailure },
    Usage,
}

/// Run without the gateway bootstrap, configuration, logging or live-profile
/// adapters. The process edge supplies arguments and its stdout writer.
pub fn run_backup_verifier(arguments: &[OsString], output: &mut dyn Write) -> u8 {
    let command = match parse_verification(arguments) {
        Ok(command) => command,
        Err(VerificationUsageError) => return print(output, &VerificationOutput::Usage, 2),
    };
    let VerificationCommand::Verify {
        bundle,
        mac_key_file,
    } = command
    else {
        return if output.write_all(VERIFY_HELP.as_bytes()).is_ok() {
            0
        } else {
            1
        };
    };
    let result = read_verification_key(&mac_key_file).and_then(|key| {
        let source = DirectoryVerificationSource::open(bundle)?;
        verify_bundle(&ExportFormat, &source, &key, VerificationLimits::default())
    });
    match result {
        Ok(report) => print(output, &VerificationOutput::Verified { report }, 0),
        Err(reason) => print(output, &VerificationOutput::Failed { reason }, 1),
    }
}

fn print(output: &mut dyn Write, result: &VerificationOutput, exit: u8) -> u8 {
    let Ok(mut bytes) = serde_json::to_vec(result) else {
        return 1;
    };
    bytes.push(b'\n');
    if output.write_all(&bytes).is_ok() {
        exit
    } else {
        1
    }
}
