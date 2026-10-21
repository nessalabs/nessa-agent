//! Quarantine, acceptance, and deferred-open decisions.
//!
//! No clock, no files, and no JSON. Infrastructure parses records and asks
//! these functions what to keep.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use unicode_normalization::UnicodeNormalization;

use super::encoding::{decode_identity, encode_identity};
use super::record::{checked_identity, Fingerprint, MAX_NAME_BYTES};
use super::{CredentialKind, DeletionFence, DeletionLog, FenceKind, StoreKind};

pub const JOURNAL_HEADER: &str = "nessa-quarantine/1";
pub const DEFERRED_HEADER: &str = "nessa-deferred/1";
/// Sealed marker. Acceptance does not remove it, so historical outbox bytes
/// are not a command to run.
pub const REPLAY_BODY: &str = "blocked\n";
/// The restored gateway does not keep the old private key.
pub const IDENTITY_BODY: &str = "re-pair\n";

/// Why a restore transition stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreFault {
    Verification,
    Rollback,
    LiveOccupied,
    QuarantineOccupied,
    ExecutorsUnfenced,
    Audit,
    Denied,
    SourceMissing,
    SourceChanged,
    Interrupted,
    Fingerprint,
    Rename,
    Unreadable,
    NotDeferred,
    Tamper,
    SessionDiffers,
    Paths,
    /// A session database still contained a fenced conversation and could not be filtered.
    FencedBody,
    /// The session file is still readable and could not be removed, moved, or emptied.
    Retained,
}

impl RestoreFault {
    /// The cause stored on the audit line for this stop.
    pub const fn cause(self) -> &'static str {
        match self {
            Self::Verification => "verification",
            Self::Rollback => "rollback",
            Self::LiveOccupied => "live-occupied",
            Self::QuarantineOccupied => "quarantine-occupied",
            Self::ExecutorsUnfenced => "executors-unfenced",
            Self::Audit => "audit",
            Self::Denied => "denied",
            Self::SourceMissing => "source-missing",
            Self::SourceChanged => "source-changed",
            Self::Interrupted => "interrupted",
            Self::Fingerprint => "fingerprint",
            Self::Rename => "rename",
            Self::Unreadable => "unreadable",
            Self::NotDeferred => "not-deferred",
            Self::Tamper => "tamper",
            Self::SessionDiffers => "session-differs",
            Self::Paths => "paths",
            Self::FencedBody => "fenced-body",
            Self::Retained => "retained",
        }
    }
}

/// What one lease line did, in file order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseMark {
    /// `granted` or `command_granted`.
    Opened,
    /// `ended`.
    Closed,
    /// Any other line. It does not open or close a lease.
    Noted,
}

/// Lease ids that are still open after `events`, in the order they opened.
pub fn leases_to_interrupt(events: &[(String, LeaseMark)]) -> Vec<String> {
    let mut open = Vec::new();
    for (id, mark) in events {
        match mark {
            LeaseMark::Opened => {
                if !open.iter().any(|existing: &String| existing == id) {
                    open.push(id.clone());
                }
            }
            LeaseMark::Closed => open.retain(|existing| existing != id),
            LeaseMark::Noted => {}
        }
    }
    open
}

/// Whether the later inventory may replace the cut's fence set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freshness {
    /// No later file, or a later file with the same fence keys.
    Cut,
    /// A later file that still has every cut fence and adds more.
    Superset,
}

impl Freshness {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cut => "cut",
            Self::Superset => "superset",
        }
    }

    fn parse(token: &str) -> Option<Self> {
        match token {
            "cut" => Some(Self::Cut),
            "superset" => Some(Self::Superset),
            _ => None,
        }
    }
}

/// The fence set a stage may apply. A later log that drops a cut fence is a rollback.
pub fn fresher_fences<'a>(
    cut: &'a DeletionLog,
    later: Option<&'a DeletionLog>,
) -> Result<(&'a DeletionLog, Freshness), RestoreFault> {
    let Some(later) = later else {
        return Ok((cut, Freshness::Cut));
    };
    let mut later_kinds = BTreeMap::new();
    for entry in later.entries() {
        if later_kinds
            .insert(entry.fence().key().to_owned(), entry.fence().kind().clone())
            .is_some()
        {
            return Err(RestoreFault::Rollback);
        }
    }
    for entry in cut.entries() {
        match later_kinds.get(entry.fence().key()) {
            Some(kind) if kind == entry.fence().kind() => {}
            _ => return Err(RestoreFault::Rollback),
        }
    }
    let freshness = if later.entries().len() == cut.entries().len() {
        Freshness::Cut
    } else {
        Freshness::Superset
    };
    Ok((later, freshness))
}

pub fn share_grant_is_fenced(
    conversation_id: &str,
    receiver_id: &str,
    credential_id: &str,
    policy_revision: Option<u64>,
    fences: &[DeletionFence],
) -> bool {
    fences.iter().any(|fence| {
        matches!(
            fence.kind(),
            FenceKind::ShareGrant {
                conversation_id: conversation,
                receiver_id: receiver,
                credential_id: credential,
                sequence,
                ..
            } if conversation == conversation_id
                && receiver == receiver_id
                && credential == credential_id
                && policy_revision.is_none_or(|revision| revision <= *sequence)
        )
    })
}

pub fn credential_is_fenced(
    kind: CredentialKind,
    credential_id: &str,
    fences: &[DeletionFence],
) -> bool {
    fences.iter().any(|fence| {
        matches!(
            fence.kind(),
            FenceKind::Credential {
                kind: fenced,
                credential_id: id,
                ..
            } if id == credential_id && same_stored_credential(*fenced, kind)
        )
    })
}

/// A hosted worker and an environment peer share one registry class. Export
/// stores that credential as an environment peer and labels the fence worker.
fn same_stored_credential(fenced: CredentialKind, stored: CredentialKind) -> bool {
    fenced == stored
        || matches!(
            (fenced, stored),
            (CredentialKind::Worker, CredentialKind::EnvironmentPeer)
                | (CredentialKind::EnvironmentPeer, CredentialKind::Worker)
        )
}

pub fn conversation_is_fenced(conversation_id: &str, fences: &[DeletionFence]) -> bool {
    fences.iter().any(
        |fence| matches!(fence.kind(), FenceKind::Conversation { id } if id == conversation_id),
    )
}

/// Which metadata row a record is, from its identity prefix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetadataRow {
    Conversation,
    Summary,
    Tombstone,
    Other,
}

pub struct MetadataFacts<'a> {
    pub row: MetadataRow,
    pub conversation_id: Option<&'a str>,
}

/// A tombstone stays. A live conversation or summary named by a fence does not.
pub fn metadata_omitted(
    facts: MetadataFacts<'_>,
    fences: &[DeletionFence],
) -> Result<bool, RestoreFault> {
    match facts.row {
        MetadataRow::Tombstone | MetadataRow::Other => Ok(false),
        MetadataRow::Conversation | MetadataRow::Summary => {
            let Some(id) = facts.conversation_id else {
                return Err(RestoreFault::Unreadable);
            };
            Ok(conversation_is_fenced(id, fences))
        }
    }
}

/// The lock file for `root`. It is a sibling of `root`, so it has to stay
/// distinct from the other restore root under the same path identity.
pub fn restore_lock_path(root: &Path) -> Result<PathBuf, RestoreFault> {
    let parent = root.parent().ok_or(RestoreFault::Paths)?;
    let mut name = root.file_name().ok_or(RestoreFault::Paths)?.to_os_string();
    name.push(".restore-lock");
    Ok(parent.join(name))
}

/// The two roots of a restore. Neither may be the other, or sit inside it.
/// `.` and `..` are collapsed before the comparison. Windows and macOS fold
/// Unicode case. macOS also treats a composed name and its decomposition as
/// one path. The comparison happens before either lock is taken.
pub fn paths_are_distinct(quarantine: &Path, live: &Path) -> Result<(), RestoreFault> {
    paths_conflict(quarantine, live, host_volume())
}

/// The volume that will hold a path's final component.
///
/// `device` is the volume identity. Two paths on different devices are different
/// objects. `case` is how that volume compares names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VolumeProbe {
    pub device: u64,
    pub case: VolumeCase,
}

/// Whether `left` and `right` are one object on the volumes that will hold them.
///
/// The same device uses the rule that folds more names. Different devices still
/// conflict when one path sits inside the other.
pub fn paths_conflict_on(
    left: &Path,
    right: &Path,
    left_vol: VolumeProbe,
    right_vol: VolumeProbe,
) -> Result<(), RestoreFault> {
    let left_root = normalized_root(left)?;
    let right_root = normalized_root(right)?;
    if left_root == right_root {
        return Err(RestoreFault::Paths);
    }
    if left_vol.device == right_vol.device {
        return paths_conflict(left, right, stricter_case(left_vol.case, right_vol.case));
    }
    let left_parts = identity_parts(left, left_vol.case)?;
    let right_on_left = identity_parts(right, left_vol.case)?;
    if starts_with_longer(&left_parts, &right_on_left) {
        return Err(RestoreFault::Paths);
    }
    let right_parts = identity_parts(right, right_vol.case)?;
    let left_on_right = identity_parts(left, right_vol.case)?;
    if starts_with_longer(&right_parts, &left_on_right) {
        return Err(RestoreFault::Paths);
    }
    Ok(())
}

fn stricter_case(left: VolumeCase, right: VolumeCase) -> VolumeCase {
    match (left, right) {
        (VolumeCase::UnicodeDecomposed, _) | (_, VolumeCase::UnicodeDecomposed) => {
            VolumeCase::UnicodeDecomposed
        }
        (VolumeCase::Unicode, _) | (_, VolumeCase::Unicode) => VolumeCase::Unicode,
        (VolumeCase::Bytes, VolumeCase::Bytes) => VolumeCase::Bytes,
    }
}

/// How a volume compares two path components that do not exist yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VolumeCase {
    /// Linux. Bytes are the identity.
    Bytes,
    /// Windows. Unicode case folds. Composition does not.
    Unicode,
    /// macOS. Unicode case folds, and names are compared decomposed.
    UnicodeDecomposed,
}

pub fn host_volume() -> VolumeCase {
    if cfg!(target_os = "macos") {
        VolumeCase::UnicodeDecomposed
    } else if cfg!(windows) {
        VolumeCase::Unicode
    } else {
        VolumeCase::Bytes
    }
}

pub fn paths_conflict(left: &Path, right: &Path, volume: VolumeCase) -> Result<(), RestoreFault> {
    let left = identity_parts(left, volume)?;
    let right = identity_parts(right, volume)?;
    if left == right || starts_with_longer(&left, &right) || starts_with_longer(&right, &left) {
        return Err(RestoreFault::Paths);
    }
    Ok(())
}

fn starts_with_longer(parent: &[String], child: &[String]) -> bool {
    child.starts_with(parent) && child.len() > parent.len()
}

fn identity_parts(path: &Path, volume: VolumeCase) -> Result<Vec<String>, RestoreFault> {
    let path = normalized_root(path)?;
    Ok(path
        .components()
        .map(|component| component_key(component, volume))
        .collect())
}

fn component_key(component: Component<'_>, volume: VolumeCase) -> String {
    match component {
        Component::Prefix(prefix) => fold_name(prefix.as_os_str(), volume),
        Component::RootDir => String::new(),
        Component::CurDir | Component::ParentDir => String::new(),
        Component::Normal(name) => fold_name(name, volume),
    }
}

fn fold_name(name: &OsStr, volume: VolumeCase) -> String {
    let Some(text) = name.to_str() else {
        return raw_name(name);
    };
    match volume {
        VolumeCase::Bytes => text.to_owned(),
        VolumeCase::Unicode => unicode_fold(text),
        VolumeCase::UnicodeDecomposed => unicode_fold(&text.nfd().collect::<String>()),
    }
}

fn unicode_fold(text: &str) -> String {
    text.chars().flat_map(char::to_lowercase).collect()
}

/// A non-Unicode component stays byte-distinct. A lossy display is not an identity.
fn raw_name(name: &OsStr) -> String {
    let mut out = String::from("raw:");
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        for byte in name.as_bytes() {
            out.push_str(&format!("{byte:02x}"));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        for unit in name.encode_wide() {
            out.push_str(&format!("{unit:04x}"));
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = name;
    }
    out
}

/// An absolute path with `.` and `..` removed. A `..` above the root is refused.
pub fn normalized_root(path: &Path) -> Result<PathBuf, RestoreFault> {
    if !path.is_absolute() {
        return Err(RestoreFault::Paths);
    }
    let mut normal = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normal.pop() {
                    return Err(RestoreFault::Paths);
                }
            }
            other => normal.push(other.as_os_str()),
        }
    }
    if !normal.is_absolute() {
        return Err(RestoreFault::Paths);
    }
    Ok(normal)
}

/// One file the quarantine seal covers. `path` is relative and uses `/`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SealedFile {
    pub path: String,
    pub fingerprint: Fingerprint,
}

/// SHA-256 of the sorted `path SP fingerprint` lines.
pub fn seal_files(files: &[SealedFile]) -> Result<Fingerprint, RestoreFault> {
    let mut ordered = files.to_vec();
    ordered.sort_by(|left, right| left.path.cmp(&right.path));
    let mut text = String::new();
    for file in &ordered {
        if !relative_path_is_safe(&file.path) {
            return Err(RestoreFault::Tamper);
        }
        text.push_str(&file.path);
        text.push(' ');
        text.push_str(&file.fingerprint.hex());
        text.push('\n');
    }
    Ok(Fingerprint::of(text.as_bytes()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeferredLine {
    pub store: StoreKind,
    pub conversation_id: String,
    pub size: u64,
    pub digest: Fingerprint,
}

pub fn encode_deferred(lines: &[DeferredLine]) -> Result<String, RestoreFault> {
    let mut text = String::from(DEFERRED_HEADER);
    text.push('\n');
    for line in lines {
        checked_identity(line.conversation_id.clone()).map_err(|_| RestoreFault::Unreadable)?;
        text.push_str(line.store.as_str());
        text.push(' ');
        text.push_str(&encode_identity(&line.conversation_id));
        text.push(' ');
        text.push_str(&line.size.to_string());
        text.push(' ');
        text.push_str(&line.digest.hex());
        text.push('\n');
    }
    Ok(text)
}

pub fn decode_deferred(text: &str) -> Result<Vec<DeferredLine>, RestoreFault> {
    let mut lines = text.lines();
    if lines.next() != Some(DEFERRED_HEADER) {
        return Err(RestoreFault::Tamper);
    }
    let mut decoded = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split(' ');
        let store = StoreKind::parse(parts.next().ok_or(RestoreFault::Tamper)?)
            .ok_or(RestoreFault::Tamper)?;
        let conversation_id =
            decode_identity(parts.next().ok_or(RestoreFault::Tamper)?, MAX_NAME_BYTES)
                .map_err(|_| RestoreFault::Tamper)?;
        let size: u64 = parts
            .next()
            .ok_or(RestoreFault::Tamper)?
            .parse()
            .map_err(|_| RestoreFault::Tamper)?;
        let digest = Fingerprint::from_hex(parts.next().ok_or(RestoreFault::Tamper)?)
            .map_err(|_| RestoreFault::Tamper)?;
        if parts.next().is_some() {
            return Err(RestoreFault::Tamper);
        }
        decoded.push(DeferredLine {
            store,
            conversation_id,
            size,
            digest,
        });
    }
    Ok(decoded)
}

/// The deferred line an open must match. Canonical records win; a notes-only
/// line is the same stream. Two lines that disagree are not a version.
pub fn deferred_for_open(
    lines: &[DeferredLine],
    conversation_id: &str,
) -> Result<DeferredLine, RestoreFault> {
    let matched: Vec<_> = lines
        .iter()
        .filter(|line| line.conversation_id == conversation_id)
        .collect();
    if matched.is_empty() {
        return Err(RestoreFault::NotDeferred);
    }
    let expected = &matched[0];
    if matched
        .iter()
        .any(|line| line.size != expected.size || line.digest != expected.digest)
    {
        return Err(RestoreFault::Fingerprint);
    }
    matched
        .iter()
        .find(|line| line.store == StoreKind::CanonicalRecords)
        .or_else(|| matched.first())
        .map(|line| (*line).clone())
        .ok_or(RestoreFault::NotDeferred)
}

/// A verified quarantine journal. Parsing checks the seal against its own file lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Journal {
    pub freshness: Freshness,
    pub seal: Fingerprint,
    pub leases: Vec<String>,
    pub files: Vec<SealedFile>,
}

pub fn encode_journal(
    freshness: Freshness,
    leases: &[String],
    files: &[SealedFile],
) -> Result<String, RestoreFault> {
    let seal = seal_files(files)?;
    let mut ordered = files.to_vec();
    ordered.sort_by(|left, right| left.path.cmp(&right.path));
    let mut text = String::from(JOURNAL_HEADER);
    text.push('\n');
    text.push_str("phase verified\n");
    text.push_str("freshness ");
    text.push_str(freshness.as_str());
    text.push('\n');
    text.push_str("seal ");
    text.push_str(&seal.hex());
    text.push('\n');
    for lease in leases {
        checked_identity(lease.clone()).map_err(|_| RestoreFault::Unreadable)?;
        text.push_str("lease ");
        text.push_str(&encode_identity(lease));
        text.push('\n');
    }
    for file in &ordered {
        text.push_str("file ");
        text.push_str(&encode_identity(&file.path));
        text.push(' ');
        text.push_str(&file.fingerprint.hex());
        text.push('\n');
    }
    Ok(text)
}

pub fn read_journal(text: &str) -> Result<Journal, RestoreFault> {
    let mut lines = text.lines();
    if lines.next() != Some(JOURNAL_HEADER) {
        return Err(RestoreFault::Tamper);
    }
    if lines.next() != Some("phase verified") {
        return Err(RestoreFault::Tamper);
    }
    let freshness = Freshness::parse(
        lines
            .next()
            .ok_or(RestoreFault::Tamper)?
            .strip_prefix("freshness ")
            .ok_or(RestoreFault::Tamper)?,
    )
    .ok_or(RestoreFault::Tamper)?;
    let seal = Fingerprint::from_hex(
        lines
            .next()
            .ok_or(RestoreFault::Tamper)?
            .strip_prefix("seal ")
            .ok_or(RestoreFault::Tamper)?,
    )
    .map_err(|_| RestoreFault::Tamper)?;
    let mut leases = Vec::new();
    let mut files = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some(encoded) = line.strip_prefix("lease ") {
            leases
                .push(decode_identity(encoded, MAX_NAME_BYTES).map_err(|_| RestoreFault::Tamper)?);
            continue;
        }
        let rest = line.strip_prefix("file ").ok_or(RestoreFault::Tamper)?;
        let (encoded, hex) = rest.rsplit_once(' ').ok_or(RestoreFault::Tamper)?;
        let path = decode_identity(encoded, MAX_NAME_BYTES).map_err(|_| RestoreFault::Tamper)?;
        if !relative_path_is_safe(&path) {
            return Err(RestoreFault::Tamper);
        }
        files.push(SealedFile {
            path,
            fingerprint: Fingerprint::from_hex(hex).map_err(|_| RestoreFault::Tamper)?,
        });
    }
    let actual = seal_files(&files)?;
    if actual != seal {
        return Err(RestoreFault::Tamper);
    }
    Ok(Journal {
        freshness,
        seal,
        leases,
        files,
    })
}

/// Where a verified tree sits. The rename is what makes it live.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Posture {
    Quarantined,
    Live,
}

pub fn posture(at_live_path: bool) -> Posture {
    if at_live_path {
        Posture::Live
    } else {
        Posture::Quarantined
    }
}

pub fn receipt_is_usable(text: &str) -> bool {
    !text.is_empty() && text.len() <= MAX_NAME_BYTES && text.bytes().all(|byte| byte >= 0x20)
}

/// The audit spelling of a path.
///
/// `Path::display` replaces bytes that are not Unicode, so two roots can share
/// one line. A path that is not Unicode is not a root.
pub fn audit_path(path: &Path) -> Result<String, RestoreFault> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or(RestoreFault::Paths)
}

/// One lease and the receipt that covered it.
///
/// Each value is percent-encoded. A space in either value is not the separator,
/// so `a b` with `c` is a different line from `a` with `b c`.
pub fn attested_pair(lease: &str, receipt: &str) -> String {
    format!("{} {}\n", encode_identity(lease), encode_identity(receipt))
}

/// The sealed effect names this journal digest and this one staging.
pub fn seal_attestation(digest: &str, staging_id: &str) -> String {
    format!(
        "{} {}",
        encode_identity(digest),
        encode_identity(staging_id)
    )
}

/// The staging id stored beside `digest`, when the attestation is that pair.
pub fn seal_staging_id(attestations: &str, digest: &str) -> Option<String> {
    let mut tokens = attestations.split_whitespace();
    let left = tokens.next()?;
    let right = tokens.next()?;
    if tokens.next().is_some() {
        return None;
    }
    let left = decode_identity(left, MAX_NAME_BYTES).ok()?;
    let right = decode_identity(right, MAX_NAME_BYTES).ok()?;
    if left == digest {
        Some(right)
    } else {
        None
    }
}

/// Whether an effect's attestations name `id` as one of their tokens.
pub fn attestation_mentions(attestations: &str, id: &str) -> bool {
    attestations.split_whitespace().any(|token| {
        decode_identity(token, MAX_NAME_BYTES)
            .ok()
            .is_some_and(|decoded| decoded == id)
    })
}

/// A staging id that can be stored in the readiness marker.
pub fn staging_id_is_usable(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_NAME_BYTES
        && id.bytes().all(|byte| {
            matches!(
                byte,
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~'
            )
        })
}

pub fn deferred_session_name(conversation_id: &str) -> Result<String, RestoreFault> {
    checked_identity(conversation_id.to_owned()).map_err(|_| RestoreFault::Unreadable)?;
    Ok(format!(
        "sessions/{}.sqlite3",
        encode_identity(conversation_id)
    ))
}

/// A helper beside one session file: `sessions/<id>.sqlite3` plus
/// `.rejected` or `.publication`.
///
/// A `.partial` file is not a helper. Bytes that were not published are an
/// extra, and accept refuses them.
pub fn session_sidecar(relative: &str) -> Option<String> {
    let session = relative
        .strip_suffix(".rejected")
        .or_else(|| relative.strip_suffix(".publication"))?;
    if !session.starts_with("sessions/")
        || !session.ends_with(".sqlite3")
        || !relative_path_is_safe(session)
    {
        return None;
    }
    Some(session.to_owned())
}

/// A file accept did not seal. Only a deferred conversation's session file qualifies.
pub fn extra_session_allowed(relative: &str, deferred: &[DeferredLine]) -> bool {
    deferred
        .iter()
        .any(|line| deferred_session_name(&line.conversation_id).is_ok_and(|name| name == relative))
}

pub fn relative_path_is_safe(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::backup::domain::{CredentialKind, DeletionFence, DeletionLog, GrantRole};

    fn fence_log(fences: Vec<DeletionFence>) -> DeletionLog {
        let mut log = DeletionLog::default();
        for fence in fences {
            log = log.append(fence).0;
        }
        log
    }

    #[test]
    fn an_open_lease_is_interrupted_and_a_closed_one_is_not() {
        let events = [
            ("live".into(), LeaseMark::Opened),
            ("done".into(), LeaseMark::Opened),
            ("done".into(), LeaseMark::Closed),
            ("live".into(), LeaseMark::Noted),
            ("live".into(), LeaseMark::Opened),
        ];
        assert_eq!(leases_to_interrupt(&events), vec!["live".to_owned()]);
    }

    #[test]
    fn an_older_inventory_is_a_rollback() {
        let cut = fence_log(vec![DeletionFence::conversation("conv-1").unwrap()]);
        let empty = DeletionLog::default();
        assert_eq!(
            fresher_fences(&cut, Some(&empty)).unwrap_err(),
            RestoreFault::Rollback
        );
        let extra = fence_log(vec![
            DeletionFence::conversation("conv-1").unwrap(),
            DeletionFence::conversation("conv-2").unwrap(),
        ]);
        let (chosen, freshness) = fresher_fences(&cut, Some(&extra)).unwrap();
        assert_eq!(freshness, Freshness::Superset);
        assert_eq!(chosen.entries().len(), 2);
        assert_eq!(fresher_fences(&cut, None).unwrap().1, Freshness::Cut);
    }

    #[test]
    fn fences_omit_a_revoked_grant_and_keep_a_tombstone() {
        let fences = vec![
            DeletionFence::share_grant("conv-1", "recv-1", "cred-1", GrantRole::Read, 4).unwrap(),
            DeletionFence::credential(CredentialKind::Device, "cred-9", Fingerprint::of(b"public"))
                .unwrap(),
            DeletionFence::conversation("conv-1").unwrap(),
        ];
        assert!(share_grant_is_fenced(
            "conv-1",
            "recv-1",
            "cred-1",
            Some(4),
            &fences
        ));
        assert!(!share_grant_is_fenced(
            "conv-1",
            "recv-1",
            "cred-1",
            Some(5),
            &fences
        ));
        assert!(share_grant_is_fenced(
            "conv-1", "recv-1", "cred-1", None, &fences
        ));
        assert!(!share_grant_is_fenced(
            "conv-1",
            "recv-2",
            "cred-1",
            Some(4),
            &fences
        ));
        assert!(credential_is_fenced(
            CredentialKind::Device,
            "cred-9",
            &fences
        ));
        assert!(!credential_is_fenced(
            CredentialKind::Peer,
            "cred-9",
            &fences
        ));
        let worker = [DeletionFence::credential(
            CredentialKind::Worker,
            "cred-w",
            Fingerprint::of(b"public"),
        )
        .unwrap()];
        assert!(credential_is_fenced(
            CredentialKind::EnvironmentPeer,
            "cred-w",
            &worker
        ));
        assert!(credential_is_fenced(
            CredentialKind::Worker,
            "cred-w",
            &worker
        ));
        assert!(!credential_is_fenced(
            CredentialKind::Device,
            "cred-w",
            &worker
        ));
        assert!(metadata_omitted(
            MetadataFacts {
                row: MetadataRow::Conversation,
                conversation_id: Some("conv-1"),
            },
            &fences,
        )
        .unwrap());
        assert!(!metadata_omitted(
            MetadataFacts {
                row: MetadataRow::Tombstone,
                conversation_id: Some("conv-1"),
            },
            &fences,
        )
        .unwrap());
        assert_eq!(
            metadata_omitted(
                MetadataFacts {
                    row: MetadataRow::Summary,
                    conversation_id: None,
                },
                &fences,
            )
            .unwrap_err(),
            RestoreFault::Unreadable
        );
    }

    #[test]
    fn a_journal_seal_rejects_a_changed_file_line() {
        let files = vec![SealedFile {
            path: "identity.txt".into(),
            fingerprint: Fingerprint::of(b"re-pair\n"),
        }];
        let text = encode_journal(Freshness::Cut, &[], &files).unwrap();
        let journal = read_journal(&text).unwrap();
        assert_eq!(journal.files, files);
        let tampered = text.replace(&files[0].fingerprint.hex(), &Fingerprint::of(b"x").hex());
        assert_eq!(read_journal(&tampered).unwrap_err(), RestoreFault::Tamper);
    }

    #[test]
    fn paths_and_receipts_are_constrained() {
        let root = if cfg!(windows) {
            PathBuf::from(r"C:\")
        } else {
            PathBuf::from("/")
        };
        let quarantine = root.join("q");
        let live = root.join("live");
        assert!(paths_are_distinct(&quarantine, &live).is_ok());
        assert_eq!(
            paths_are_distinct(&live, &live).unwrap_err(),
            RestoreFault::Paths
        );
        assert_eq!(
            paths_are_distinct(&live.join("quarantine"), &live).unwrap_err(),
            RestoreFault::Paths
        );
        assert_eq!(
            paths_are_distinct(Path::new("q"), Path::new("live")).unwrap_err(),
            RestoreFault::Paths
        );
        assert_eq!(
            paths_are_distinct(&root.join("parent").join("..").join("live"), &live).unwrap_err(),
            RestoreFault::Paths
        );
        let upper = root.join("Ä");
        let lower = root.join("ä");
        assert!(paths_conflict(&upper, &lower, VolumeCase::Bytes).is_ok());
        assert_eq!(
            paths_conflict(&upper, &lower, VolumeCase::Unicode).unwrap_err(),
            RestoreFault::Paths
        );
        assert_eq!(
            paths_conflict(&upper, &lower, VolumeCase::UnicodeDecomposed).unwrap_err(),
            RestoreFault::Paths
        );
        let composed = root.join("\u{00e9}");
        let decomposed = root.join("e\u{0301}");
        assert!(paths_conflict(&composed, &decomposed, VolumeCase::Unicode).is_ok());
        assert_eq!(
            paths_conflict(&composed, &decomposed, VolumeCase::UnicodeDecomposed).unwrap_err(),
            RestoreFault::Paths
        );
        assert_eq!(
            paths_conflict(
                &root.join("Ä").join("child"),
                &root.join("ä"),
                VolumeCase::Unicode
            )
            .unwrap_err(),
            RestoreFault::Paths
        );
        let digest = "abc";
        let staged = seal_attestation(digest, "stage-1");
        assert_eq!(seal_staging_id(&staged, digest).as_deref(), Some("stage-1"));
        assert!(seal_staging_id(&staged, "other").is_none());
        assert!(attestation_mentions("stage-1", "stage-1"));
        assert!(attestation_mentions(&staged, "stage-1"));
        assert!(staging_id_is_usable("stage-1"));
        assert!(!staging_id_is_usable("stage 1"));
        let folding = VolumeProbe {
            device: 1,
            case: VolumeCase::Unicode,
        };
        let sensitive = VolumeProbe {
            device: 2,
            case: VolumeCase::Bytes,
        };
        assert_eq!(
            paths_conflict_on(&root.join("q"), &root.join("Q"), folding, folding).unwrap_err(),
            RestoreFault::Paths
        );
        assert!(paths_conflict_on(&root.join("q"), &root.join("Q"), sensitive, sensitive).is_ok());
        assert!(paths_conflict_on(&root.join("q"), &root.join("Q"), folding, sensitive).is_ok());
        assert_eq!(
            paths_conflict_on(
                &root.join("mnt"),
                &root.join("mnt").join("q"),
                sensitive,
                folding
            )
            .unwrap_err(),
            RestoreFault::Paths
        );
        if cfg!(windows) {
            assert_eq!(
                paths_conflict(
                    &PathBuf::from(r"C:\Live"),
                    &PathBuf::from(r"c:\live"),
                    VolumeCase::Unicode,
                )
                .unwrap_err(),
                RestoreFault::Paths
            );
        }
        if cfg!(any(windows, target_os = "macos")) {
            assert_eq!(
                paths_are_distinct(&root.join("Restore"), &root.join("restore")).unwrap_err(),
                RestoreFault::Paths
            );
            assert_eq!(
                paths_are_distinct(&root.join("Restore").join("child"), &root.join("restore"))
                    .unwrap_err(),
                RestoreFault::Paths
            );
        } else {
            assert!(paths_are_distinct(&root.join("Restore"), &root.join("restore")).is_ok());
        }
        let quarantine_lock = restore_lock_path(&quarantine).unwrap();
        assert_eq!(
            paths_are_distinct(&quarantine_lock, &quarantine_lock).unwrap_err(),
            RestoreFault::Paths
        );
        let partial = root.join("q.partial-1-1");
        if cfg!(any(windows, target_os = "macos")) {
            assert_eq!(
                paths_are_distinct(&partial, &root.join("Q.partial-1-1")).unwrap_err(),
                RestoreFault::Paths
            );
        } else {
            assert!(paths_are_distinct(&partial, &root.join("Q.partial-1-1")).is_ok());
        }
        assert_eq!(
            session_sidecar("sessions/conv-bulk.sqlite3.publication").as_deref(),
            Some("sessions/conv-bulk.sqlite3")
        );
        assert!(session_sidecar("sessions/conv-bulk.sqlite3.partial").is_none());
        assert!(session_sidecar("sessions/conv-bulk.sqlite3.partial.rejected").is_none());
        assert!(session_sidecar("records/share-grants/revoked.publication").is_none());
        assert_eq!(
            attested_pair("lease-9", "host-stopped"),
            "lease-9 host-stopped\n"
        );
        assert_eq!(attested_pair("a b", "c"), "a%20b c\n");
        assert_eq!(attested_pair("a", "b c"), "a b%20c\n");
        assert_ne!(attested_pair("a b", "c"), attested_pair("a", "b c"));
        assert_eq!(audit_path(Path::new("/live")).unwrap(), "/live");
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            assert_eq!(
                audit_path(Path::new(std::ffi::OsStr::from_bytes(&[0xff]))).unwrap_err(),
                RestoreFault::Paths
            );
            assert_eq!(
                audit_path(Path::new(std::ffi::OsStr::from_bytes(&[0xfe]))).unwrap_err(),
                RestoreFault::Paths
            );
        }
        assert!(receipt_is_usable("stopped-host-1"));
        assert!(!receipt_is_usable(""));
        assert!(!receipt_is_usable("has\nline"));
        assert_eq!(posture(false), Posture::Quarantined);
        assert_eq!(posture(true), Posture::Live);
    }

    #[test]
    fn deferred_lines_agree_before_an_open() {
        let digest = Fingerprint::of(b"stream");
        let lines = vec![
            DeferredLine {
                store: StoreKind::Notes,
                conversation_id: "conv-bulk".into(),
                size: 3,
                digest,
            },
            DeferredLine {
                store: StoreKind::CanonicalRecords,
                conversation_id: "conv-bulk".into(),
                size: 3,
                digest,
            },
        ];
        let text = encode_deferred(&lines).unwrap();
        let decoded = decode_deferred(&text).unwrap();
        let open = deferred_for_open(&decoded, "conv-bulk").unwrap();
        assert_eq!(open.store, StoreKind::CanonicalRecords);
        assert!(extra_session_allowed(
            &deferred_session_name("conv-bulk").unwrap(),
            &decoded
        ));
        assert!(!extra_session_allowed("sessions/other.sqlite3", &decoded));
        let mut disagree = decoded.clone();
        disagree[0].size = 4;
        assert_eq!(
            deferred_for_open(&disagree, "conv-bulk").unwrap_err(),
            RestoreFault::Fingerprint
        );
        assert_eq!(
            deferred_for_open(&decoded, "missing").unwrap_err(),
            RestoreFault::NotDeferred
        );
    }
}
