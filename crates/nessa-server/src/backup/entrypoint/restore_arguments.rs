//! Arguments for the restore command. Paths are absolute and explicit.

use std::ffi::OsString;
use std::path::PathBuf;

pub const RESTORE_HELP: &str = "\
Usage:
  nessa-backup-restore stage --bundle ABSOLUTE_PATH --mac-key-file ABSOLUTE_PATH --destination ABSOLUTE_PATH [--later-inventory ABSOLUTE_PATH] [--source ABSOLUTE_PATH]
  nessa-backup-restore accept --destination ABSOLUTE_PATH [--namespace ABSOLUTE_PATH] [--receipt LEASE=TEXT]
  nessa-backup-restore list --destination ABSOLUTE_PATH
  nessa-backup-restore open --destination ABSOLUTE_PATH --conversation ID

Stages an authenticated backup into destination/quarantine. The live path stays
absent, and this command starts no gateway, harness, or command. accept renames
that quarantine onto the empty destination/live path after Cedar allows it and,
when a lease was still open, every --receipt is present. The restored gateway
does not keep the old private key. open fetches one deferred conversation from
the source recorded at stage, and refuses when that source is gone. Other
conversations are not written.
";

#[derive(Debug, PartialEq, Eq)]
pub enum RestoreCommand {
    Help,
    Stage {
        bundle: PathBuf,
        mac_key_file: PathBuf,
        destination: PathBuf,
        later_inventory: Option<PathBuf>,
        source: Option<PathBuf>,
    },
    Accept {
        destination: PathBuf,
        namespace: Option<PathBuf>,
        receipts: Vec<(String, String)>,
    },
    List {
        destination: PathBuf,
    },
    Open {
        destination: PathBuf,
        conversation: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RestoreUsageError;

pub fn parse_restore(arguments: &[OsString]) -> Result<RestoreCommand, RestoreUsageError> {
    if arguments.is_empty()
        || arguments == [OsString::from("--help")]
        || arguments == [OsString::from("-h")]
    {
        return Ok(RestoreCommand::Help);
    }
    let (verb, rest) = arguments.split_first().ok_or(RestoreUsageError)?;
    match verb.to_str() {
        Some("stage") => parse_stage(rest),
        Some("accept") => parse_accept(rest),
        Some("list") => parse_list(rest),
        Some("open") => parse_open(rest),
        _ => Err(RestoreUsageError),
    }
}

fn parse_stage(arguments: &[OsString]) -> Result<RestoreCommand, RestoreUsageError> {
    let mut bundle = None;
    let mut mac_key_file = None;
    let mut destination = None;
    let mut later_inventory = None;
    let mut source = None;
    let mut index = 0;
    while index < arguments.len() {
        let flag = arguments[index].to_str().ok_or(RestoreUsageError)?;
        let value = arguments.get(index + 1).ok_or(RestoreUsageError)?;
        let path = absolute(value)?;
        match flag {
            "--bundle" if bundle.is_none() => bundle = Some(path),
            "--mac-key-file" if mac_key_file.is_none() => mac_key_file = Some(path),
            "--destination" if destination.is_none() => destination = Some(path),
            "--later-inventory" if later_inventory.is_none() => later_inventory = Some(path),
            "--source" if source.is_none() => source = Some(path),
            _ => return Err(RestoreUsageError),
        }
        index += 2;
    }
    Ok(RestoreCommand::Stage {
        bundle: bundle.ok_or(RestoreUsageError)?,
        mac_key_file: mac_key_file.ok_or(RestoreUsageError)?,
        destination: destination.ok_or(RestoreUsageError)?,
        later_inventory,
        source,
    })
}

fn parse_accept(arguments: &[OsString]) -> Result<RestoreCommand, RestoreUsageError> {
    let mut destination = None;
    let mut namespace = None;
    let mut receipts = Vec::new();
    let mut index = 0;
    while index < arguments.len() {
        let flag = arguments[index].to_str().ok_or(RestoreUsageError)?;
        let value = arguments.get(index + 1).ok_or(RestoreUsageError)?;
        match flag {
            "--destination" if destination.is_none() => destination = Some(absolute(value)?),
            "--namespace" if namespace.is_none() => namespace = Some(absolute(value)?),
            "--receipt" => receipts.push(receipt(value)?),
            _ => return Err(RestoreUsageError),
        }
        index += 2;
    }
    if receipts.is_empty() && arguments.iter().any(|arg| arg == "--receipt") {
        return Err(RestoreUsageError);
    }
    Ok(RestoreCommand::Accept {
        destination: destination.ok_or(RestoreUsageError)?,
        namespace,
        receipts,
    })
}

fn parse_list(arguments: &[OsString]) -> Result<RestoreCommand, RestoreUsageError> {
    if arguments.len() != 2 || arguments[0].to_str() != Some("--destination") {
        return Err(RestoreUsageError);
    }
    Ok(RestoreCommand::List {
        destination: absolute(&arguments[1])?,
    })
}

fn parse_open(arguments: &[OsString]) -> Result<RestoreCommand, RestoreUsageError> {
    let mut destination = None;
    let mut conversation = None;
    let mut index = 0;
    while index < arguments.len() {
        let flag = arguments[index].to_str().ok_or(RestoreUsageError)?;
        let value = arguments.get(index + 1).ok_or(RestoreUsageError)?;
        match flag {
            "--destination" if destination.is_none() => destination = Some(absolute(value)?),
            "--conversation" if conversation.is_none() => {
                let text = value.to_str().ok_or(RestoreUsageError)?;
                if text.is_empty() || text.contains(['\n', '\0', ' ']) {
                    return Err(RestoreUsageError);
                }
                conversation = Some(text.to_owned());
            }
            _ => return Err(RestoreUsageError),
        }
        index += 2;
    }
    Ok(RestoreCommand::Open {
        destination: destination.ok_or(RestoreUsageError)?,
        conversation: conversation.ok_or(RestoreUsageError)?,
    })
}

fn absolute(value: &OsString) -> Result<PathBuf, RestoreUsageError> {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(RestoreUsageError)
    }
}

fn receipt(value: &OsString) -> Result<(String, String), RestoreUsageError> {
    let text = value.to_str().ok_or(RestoreUsageError)?;
    let Some((lease, receipt)) = text.split_once('=') else {
        return Err(RestoreUsageError);
    };
    if lease.is_empty()
        || receipt.is_empty()
        || lease.contains([' ', '\n', '\0'])
        || receipt.contains(['\n', '\0'])
    {
        return Err(RestoreUsageError);
    }
    Ok((lease.to_owned(), receipt.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arg(text: &str) -> OsString {
        OsString::from(text)
    }

    #[test]
    fn stage_requires_absolute_paths() {
        let root = if cfg!(windows) {
            r"C:\bundle"
        } else {
            "/bundle"
        };
        let key = if cfg!(windows) { r"C:\key" } else { "/key" };
        let destination = if cfg!(windows) { r"C:\dest" } else { "/dest" };
        let parsed = parse_restore(&[
            arg("stage"),
            arg("--bundle"),
            arg(root),
            arg("--mac-key-file"),
            arg(key),
            arg("--destination"),
            arg(destination),
        ])
        .unwrap();
        assert!(matches!(parsed, RestoreCommand::Stage { .. }));
        assert_eq!(
            parse_restore(&[
                arg("stage"),
                arg("--bundle"),
                arg("relative"),
                arg("--mac-key-file"),
                arg(key),
                arg("--destination"),
                arg(destination),
            ]),
            Err(RestoreUsageError)
        );
    }

    #[test]
    fn a_receipt_is_one_lease_and_its_text() {
        let destination = if cfg!(windows) { r"C:\dest" } else { "/dest" };
        let parsed = parse_restore(&[
            arg("accept"),
            arg("--destination"),
            arg(destination),
            arg("--receipt"),
            arg("lease-drill=stopped host"),
        ])
        .unwrap();
        match parsed {
            RestoreCommand::Accept { receipts, .. } => {
                assert_eq!(
                    receipts,
                    vec![("lease-drill".to_owned(), "stopped host".to_owned())]
                );
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            parse_restore(&[
                arg("accept"),
                arg("--destination"),
                arg(destination),
                arg("--receipt"),
                arg("=missing"),
            ]),
            Err(RestoreUsageError)
        );
    }
}
