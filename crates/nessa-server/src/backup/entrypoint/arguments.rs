//! Explicit path arguments for the standalone read-only tool.

use std::ffi::OsString;
use std::path::PathBuf;

pub const VERIFY_HELP: &str = "Usage: nessa-backup-verify --bundle ABSOLUTE_PATH --mac-key-file ABSOLUTE_PATH\n\nVerifies export-format-v1 integrity with an explicitly supplied 32-byte raw HMAC key.\nDoes not start a gateway, import data, establish freshness or activate restored authority.\n";

pub enum VerificationCommand {
    Help,
    Verify {
        bundle: PathBuf,
        mac_key_file: PathBuf,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerificationUsageError;

/// Both inputs are explicit absolute paths. No profile or environment fallback.
pub fn parse_verification(
    arguments: &[OsString],
) -> Result<VerificationCommand, VerificationUsageError> {
    if arguments == [OsString::from("--help")] || arguments == [OsString::from("-h")] {
        return Ok(VerificationCommand::Help);
    }
    if arguments.len() != 4 {
        return Err(VerificationUsageError);
    }
    let mut bundle = None;
    let mut mac_key_file = None;
    for pair in arguments.as_chunks::<2>().0 {
        let path = PathBuf::from(&pair[1]);
        if !path.is_absolute() {
            return Err(VerificationUsageError);
        }
        match pair[0].to_str() {
            Some("--bundle") if bundle.is_none() => bundle = Some(path),
            Some("--mac-key-file") if mac_key_file.is_none() => mac_key_file = Some(path),
            _ => return Err(VerificationUsageError),
        }
    }
    Ok(VerificationCommand::Verify {
        bundle: bundle.ok_or(VerificationUsageError)?,
        mac_key_file: mac_key_file.ok_or(VerificationUsageError)?,
    })
}
