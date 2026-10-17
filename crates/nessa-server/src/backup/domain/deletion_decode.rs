//! The deletion inventory decoder shared by live storage and verification.

use super::encoding::decode;
use super::{CredentialKind, DeletionEntry, DeletionFence, DeletionLog, Fingerprint, GrantRole};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeletionReadError {
    UnsupportedVersion,
    Malformed,
}

impl DeletionLog {
    /// Decode the live inventory grammar. This keeps its accepted blank lines
    /// and noncanonical spellings; a verifier also compares `canonical()`.
    pub fn decode(text: &str) -> Result<Self, DeletionReadError> {
        match text.lines().next() {
            Some("nessa-deletion-inventory/1") => {}
            Some(header) if header.starts_with("nessa-deletion-inventory/") => {
                return Err(DeletionReadError::UnsupportedVersion);
            }
            _ => return Err(DeletionReadError::Malformed),
        }
        parse_entries(text).map_err(|()| DeletionReadError::Malformed)
    }
}

fn parse_entries(canonical: &str) -> Result<DeletionLog, ()> {
    let mut lines = canonical.lines();
    // `decode` owns header/version validation before entering this helper.
    lines.next();
    let mut entries = Vec::new();
    for (index, line) in lines.enumerate() {
        if line.is_empty() {
            continue;
        }
        let sequence = (index as u64) + 1;
        let fence = parse_entry(line, sequence)?;
        entries.push(DeletionEntry::from_persisted(sequence, fence).map_err(|_| ())?);
    }
    DeletionLog::new(entries).map_err(|_| ())
}

fn parse_entry(line: &str, sequence: u64) -> Result<DeletionFence, ()> {
    let mut parts = line.split(' ');
    let got = parts.next().ok_or(())?.parse::<u64>().map_err(|_| ())?;
    if got != sequence {
        return Err(());
    }
    match parts.next().ok_or(())? {
        "conversation" => {
            let id = decode(parts.next().ok_or(())?)?;
            if parts.next().is_some() {
                return Err(());
            }
            DeletionFence::conversation(id).map_err(|_| ())
        }
        "grant" => {
            let conversation_id = decode(parts.next().ok_or(())?)?;
            let receiver_id = decode(parts.next().ok_or(())?)?;
            let credential_id = decode(parts.next().ok_or(())?)?;
            let role = GrantRole::parse(parts.next().ok_or(())?).ok_or(())?;
            let grant_sequence = parts.next().ok_or(())?.parse::<u64>().map_err(|_| ())?;
            if parts.next().is_some() {
                return Err(());
            }
            DeletionFence::share_grant(
                conversation_id,
                receiver_id,
                credential_id,
                role,
                grant_sequence,
            )
            .map_err(|_| ())
        }
        "credential" => {
            let kind = CredentialKind::parse(parts.next().ok_or(())?).ok_or(())?;
            let credential_id = decode(parts.next().ok_or(())?)?;
            let hex = parts.next().ok_or(())?;
            if parts.next().is_some() {
                return Err(());
            }
            DeletionFence::credential(
                kind,
                credential_id,
                Fingerprint::from_hex(hex).map_err(|_| ())?,
            )
            .map_err(|_| ())
        }
        _ => Err(()),
    }
}
