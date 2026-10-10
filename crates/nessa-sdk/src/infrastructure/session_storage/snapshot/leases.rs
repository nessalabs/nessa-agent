//! Lease records as stored: a `kind` and a `body`, the body being the record's
//! JSON text for that kind.
//!
//! ```text
//! LeaseRecord ──encode──▶ { kind, body } ──decode──▶ LeaseRecord
//!                                          └─(unknown kind)─▶ LeaseRecord::Unreadable
//! ```
//!
//! The body is kept as text so a record of a kind this build does not know is
//! carried and written back exactly as it was. A known kind whose body does not
//! decode is corrupt: a later build that changes a body's shape writes a new
//! kind instead.
use super::{permissions::Actor, tools::corrupt};
use crate::{
    application::agent_execution::sessions::{CurrentLease, LeaseRecord, StorageError},
    domain::agent_execution::{
        executions::ExecutionId,
        leases::{
            AgentWork, EnvironmentRef, LeaseCleanup, LeaseDeadline, LeaseEndCause, LeaseGrants,
            LeaseId, LeaseRefusal, LeaseRevision, LeaseTerms, LeaseWork, SandboxProfile,
            SshDestination,
        },
    },
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

/// Longest stored kind; the same bound an unreadable record keeps.
const KIND_BYTES: usize = LeaseRecord::MAX_UNREADABLE_KIND_BYTES;
/// Longest stored body; the same bound an unreadable record keeps.
const BODY_BYTES: usize = LeaseRecord::MAX_UNREADABLE_BODY_BYTES;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WireLease {
    kind: String,
    body: String,
}

/// The current lease in a checkpoint: its records and the newest revision
/// known for it.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SavedLease {
    revision: Option<u64>,
    records: Vec<WireLease>,
}
impl From<&CurrentLease> for SavedLease {
    fn from(value: &CurrentLease) -> Self {
        Self {
            revision: value.revision().map(LeaseRevision::get),
            records: value.records().iter().map(WireLease::from).collect(),
        }
    }
}
impl SavedLease {
    pub(super) fn decode(self) -> Result<CurrentLease, StorageError> {
        let revision = self
            .revision
            .map(LeaseRevision::new)
            .transpose()
            .map_err(corrupt)?;
        let records = self
            .records
            .into_iter()
            .map(WireLease::decode)
            .collect::<Result<Vec<_>, _>>()?;
        CurrentLease::resume(revision, &records)
    }
}

const ISSUED: &str = "issued";
const REFUSED: &str = "refused";
/// An issuance whose terms or refusal slice A's shape cannot hold: an
/// environment other than this process, or a refusal other than the sandbox.
/// A new kind rather than a wider body, so a build that knows only the first
/// shape reads it as unreadable (row L21) instead of corrupt.
const ISSUED_V2: &str = "issued_v2";
const REFUSED_V2: &str = "refused_v2";
const ENDING: &str = "ending";
const ENDED: &str = "ended";
const INTERRUPTED: &str = "interrupted";
const CLEANUP_REPORTED: &str = "cleanup_reported";
const EVENT_DROPPED: &str = "event_dropped";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Issuance {
    lease: String,
    revision: u64,
    terms: Terms,
    actor: Actor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refusal: Option<Refusal>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ending {
    lease: String,
    cause: Cause,
    actor: Option<Actor>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cleaned {
    lease: String,
    cleanup: Cleanup,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Interrupted {
    lease: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Dropped {
    lease: String,
    turn: String,
    cursor: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Terms {
    environment: Environment,
    work: Work,
    sandbox: Sandbox,
    grants: Grants,
    deadline: Deadline,
}
#[derive(Serialize, Deserialize)]
enum Environment {
    Here,
    Ssh(String),
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum Work {
    Agent { agent: String, model: String },
}
#[derive(Serialize, Deserialize)]
enum Sandbox {
    HarnessDefault,
}
#[derive(Serialize, Deserialize)]
enum Grants {
    Opening,
}
#[derive(Serialize, Deserialize)]
enum Deadline {
    UntilEnded,
    At(u64),
}
#[derive(Serialize, Deserialize)]
enum Cause {
    Stopped,
    Closed,
    Revoked,
    Expired,
    Lost,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum Cleanup {
    Confirmed { forced: bool },
    NotHeld,
}
#[derive(Serialize, Deserialize)]
enum Refusal {
    SandboxUnavailable,
    EnvironmentUnreachable,
    EnvironmentVersionMismatch,
    EnvironmentBusy,
    AgentUnavailable,
}

impl From<&LeaseTerms> for Terms {
    fn from(value: &LeaseTerms) -> Self {
        Self {
            environment: match &value.environment {
                EnvironmentRef::Here => Environment::Here,
                EnvironmentRef::Ssh(host) => Environment::Ssh(host.as_str().into()),
            },
            work: match &value.work {
                LeaseWork::Agent(work) => Work::Agent {
                    agent: work.agent().into(),
                    model: work.model().into(),
                },
            },
            sandbox: match value.sandbox {
                SandboxProfile::HarnessDefault => Sandbox::HarnessDefault,
            },
            grants: match value.grants {
                LeaseGrants::Opening => Grants::Opening,
            },
            deadline: match value.deadline {
                LeaseDeadline::UntilEnded => Deadline::UntilEnded,
                LeaseDeadline::At(at) => Deadline::At(at),
            },
        }
    }
}
impl Terms {
    fn decode(self) -> Result<LeaseTerms, StorageError> {
        Ok(LeaseTerms {
            environment: match self.environment {
                Environment::Here => EnvironmentRef::Here,
                Environment::Ssh(host) => {
                    EnvironmentRef::Ssh(SshDestination::new(host).map_err(corrupt)?)
                }
            },
            work: match self.work {
                Work::Agent { agent, model } => {
                    LeaseWork::Agent(AgentWork::new(agent, model).map_err(corrupt)?)
                }
            },
            sandbox: match self.sandbox {
                Sandbox::HarnessDefault => SandboxProfile::HarnessDefault,
            },
            grants: match self.grants {
                Grants::Opening => LeaseGrants::Opening,
            },
            deadline: match self.deadline {
                Deadline::UntilEnded => LeaseDeadline::UntilEnded,
                Deadline::At(at) => LeaseDeadline::At(at),
            },
        })
    }
}
impl From<LeaseEndCause> for Cause {
    fn from(value: LeaseEndCause) -> Self {
        match value {
            LeaseEndCause::Stopped => Self::Stopped,
            LeaseEndCause::Closed => Self::Closed,
            LeaseEndCause::Revoked => Self::Revoked,
            LeaseEndCause::Expired => Self::Expired,
            LeaseEndCause::Lost => Self::Lost,
        }
    }
}
impl From<Cause> for LeaseEndCause {
    fn from(value: Cause) -> Self {
        match value {
            Cause::Stopped => Self::Stopped,
            Cause::Closed => Self::Closed,
            Cause::Revoked => Self::Revoked,
            Cause::Expired => Self::Expired,
            Cause::Lost => Self::Lost,
        }
    }
}
impl From<LeaseCleanup> for Cleanup {
    fn from(value: LeaseCleanup) -> Self {
        match value {
            LeaseCleanup::Confirmed { forced } => Self::Confirmed { forced },
            LeaseCleanup::NotHeld => Self::NotHeld,
        }
    }
}
impl From<Cleanup> for LeaseCleanup {
    fn from(value: Cleanup) -> Self {
        match value {
            Cleanup::Confirmed { forced } => Self::Confirmed { forced },
            Cleanup::NotHeld => Self::NotHeld,
        }
    }
}
impl From<LeaseRefusal> for Refusal {
    fn from(value: LeaseRefusal) -> Self {
        match value {
            LeaseRefusal::SandboxUnavailable => Self::SandboxUnavailable,
            LeaseRefusal::EnvironmentUnreachable => Self::EnvironmentUnreachable,
            LeaseRefusal::EnvironmentVersionMismatch => Self::EnvironmentVersionMismatch,
            LeaseRefusal::EnvironmentBusy => Self::EnvironmentBusy,
            LeaseRefusal::AgentUnavailable => Self::AgentUnavailable,
        }
    }
}
impl From<Refusal> for LeaseRefusal {
    fn from(value: Refusal) -> Self {
        match value {
            Refusal::SandboxUnavailable => Self::SandboxUnavailable,
            Refusal::EnvironmentUnreachable => Self::EnvironmentUnreachable,
            Refusal::EnvironmentVersionMismatch => Self::EnvironmentVersionMismatch,
            Refusal::EnvironmentBusy => Self::EnvironmentBusy,
            Refusal::AgentUnavailable => Self::AgentUnavailable,
        }
    }
}

/// The kind an issuance or refusal is stored under: slice A's while its shape
/// holds the record, the second one otherwise.
fn issuance_kind(terms: &LeaseTerms, refusal: Option<LeaseRefusal>) -> &'static str {
    let first_shape = terms.environment == EnvironmentRef::Here
        && matches!(refusal, None | Some(LeaseRefusal::SandboxUnavailable));
    match (first_shape, refusal) {
        (true, None) => ISSUED,
        (true, Some(_)) => REFUSED,
        (false, None) => ISSUED_V2,
        (false, Some(_)) => REFUSED_V2,
    }
}

fn text<T: Serialize>(kind: &str, body: &T) -> WireLease {
    WireLease {
        kind: kind.into(),
        // These bodies hold only strings, numbers and unit variants, which
        // always serialize.
        body: serde_json::to_string(body).expect("a lease record body serializes"),
    }
}

fn body<T: DeserializeOwned>(body: &str) -> Result<T, StorageError> {
    serde_json::from_str(body).map_err(corrupt)
}

fn lease(value: String) -> Result<LeaseId, StorageError> {
    LeaseId::new(value).map_err(corrupt)
}

fn revision(value: u64) -> Result<LeaseRevision, StorageError> {
    LeaseRevision::new(value).map_err(corrupt)
}

impl From<&LeaseRecord> for WireLease {
    fn from(value: &LeaseRecord) -> Self {
        match value {
            LeaseRecord::Issued {
                lease,
                revision,
                terms,
                actor,
            } => text(
                issuance_kind(terms, None),
                &Issuance {
                    lease: lease.as_str().into(),
                    revision: revision.get(),
                    terms: terms.into(),
                    actor: actor.into(),
                    refusal: None,
                },
            ),
            LeaseRecord::Refused {
                lease,
                revision,
                terms,
                refusal,
                actor,
            } => text(
                issuance_kind(terms, Some(*refusal)),
                &Issuance {
                    lease: lease.as_str().into(),
                    revision: revision.get(),
                    terms: terms.into(),
                    actor: actor.into(),
                    refusal: Some((*refusal).into()),
                },
            ),
            LeaseRecord::Ending {
                lease,
                cause,
                actor,
            } => text(
                ENDING,
                &Ending {
                    lease: lease.as_str().into(),
                    cause: (*cause).into(),
                    actor: actor.as_ref().map(Actor::from),
                },
            ),
            LeaseRecord::Ended { lease, cleanup } => text(
                ENDED,
                &Cleaned {
                    lease: lease.as_str().into(),
                    cleanup: (*cleanup).into(),
                },
            ),
            LeaseRecord::Interrupted { lease } => text(
                INTERRUPTED,
                &Interrupted {
                    lease: lease.as_str().into(),
                },
            ),
            LeaseRecord::CleanupReported { lease, cleanup } => text(
                CLEANUP_REPORTED,
                &Cleaned {
                    lease: lease.as_str().into(),
                    cleanup: (*cleanup).into(),
                },
            ),
            LeaseRecord::EventDropped {
                lease,
                turn,
                cursor,
            } => text(
                EVENT_DROPPED,
                &Dropped {
                    lease: lease.as_str().into(),
                    turn: turn.as_str().into(),
                    cursor: *cursor,
                },
            ),
            LeaseRecord::Unreadable { kind, body } => Self {
                kind: kind.clone(),
                body: body.clone(),
            },
        }
    }
}

impl WireLease {
    pub(super) fn decode(self) -> Result<LeaseRecord, StorageError> {
        if self.kind.len() > KIND_BYTES || self.body.len() > BODY_BYTES {
            return Err(corrupt("a lease record exceeds its stored bound"));
        }
        Ok(match self.kind.as_str() {
            ISSUED | REFUSED | ISSUED_V2 | REFUSED_V2 => {
                let saved: Issuance = body(&self.body)?;
                let lease = lease(saved.lease)?;
                let revision = revision(saved.revision)?;
                let terms = saved.terms.decode()?;
                let actor = saved.actor.decode()?;
                let refusal = saved.refusal.map(LeaseRefusal::from);
                // Each shape holds only what it was written with: the first
                // kinds never name another environment or a newer refusal.
                if issuance_kind(&terms, refusal) != self.kind {
                    return Err(corrupt("a lease issuance is stored under the wrong kind"));
                }
                match (refusal, self.kind.as_str()) {
                    (None, ISSUED | ISSUED_V2) => LeaseRecord::Issued {
                        lease,
                        revision,
                        terms,
                        actor,
                    },
                    (Some(refusal), REFUSED | REFUSED_V2) => LeaseRecord::Refused {
                        lease,
                        revision,
                        terms,
                        refusal,
                        actor,
                    },
                    _ => return Err(corrupt("a lease issuance and its refusal disagree")),
                }
            }
            ENDING => {
                let saved: Ending = body(&self.body)?;
                LeaseRecord::Ending {
                    lease: lease(saved.lease)?,
                    cause: saved.cause.into(),
                    actor: saved.actor.map(Actor::decode).transpose()?,
                }
            }
            ENDED | CLEANUP_REPORTED => {
                let saved: Cleaned = body(&self.body)?;
                let lease = lease(saved.lease)?;
                let cleanup = saved.cleanup.into();
                if self.kind == ENDED {
                    LeaseRecord::Ended { lease, cleanup }
                } else {
                    LeaseRecord::CleanupReported { lease, cleanup }
                }
            }
            INTERRUPTED => {
                let saved: Interrupted = body(&self.body)?;
                LeaseRecord::Interrupted {
                    lease: lease(saved.lease)?,
                }
            }
            EVENT_DROPPED => {
                let saved: Dropped = body(&self.body)?;
                LeaseRecord::EventDropped {
                    lease: lease(saved.lease)?,
                    turn: ExecutionId::new(saved.turn).map_err(corrupt)?,
                    cursor: saved.cursor,
                }
            }
            _ => LeaseRecord::Unreadable {
                kind: self.kind,
                body: self.body,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::agent_execution::{
        permissions::ActionContext,
        sessions::{ProviderContext, SessionChange, SessionSnapshot},
    };
    use crate::infrastructure::session_storage::snapshot::{
        checkpoint::{history_fixture, Snapshot, SnapshotRef},
        decode::preflight_checkpoint,
        semantic::{decode_batch, encode_batch},
    };

    fn id(value: &str) -> LeaseId {
        LeaseId::new(value).unwrap()
    }
    fn actor() -> ActionContext {
        ActionContext::new("person", "desktop", "send").unwrap()
    }
    fn terms(deadline: LeaseDeadline) -> LeaseTerms {
        LeaseTerms {
            environment: EnvironmentRef::Here,
            work: LeaseWork::Agent(AgentWork::new("claude", "sonnet").unwrap()),
            sandbox: SandboxProfile::HarnessDefault,
            grants: LeaseGrants::Opening,
            deadline,
        }
    }
    fn every_kind() -> Vec<LeaseRecord> {
        let lease = id("lease-1");
        let mut records = vec![
            LeaseRecord::Issued {
                lease: lease.clone(),
                revision: LeaseRevision::FIRST,
                terms: terms(LeaseDeadline::UntilEnded),
                actor: actor(),
            },
            LeaseRecord::Refused {
                lease: lease.clone(),
                revision: LeaseRevision::new(9).unwrap(),
                terms: terms(LeaseDeadline::At(17)),
                refusal: LeaseRefusal::SandboxUnavailable,
                actor: actor(),
            },
            LeaseRecord::Ending {
                lease: lease.clone(),
                cause: LeaseEndCause::Stopped,
                actor: Some(actor()),
            },
            LeaseRecord::Ended {
                lease: lease.clone(),
                cleanup: LeaseCleanup::Confirmed { forced: true },
            },
            LeaseRecord::Interrupted {
                lease: lease.clone(),
            },
            LeaseRecord::CleanupReported {
                lease: lease.clone(),
                cleanup: LeaseCleanup::NotHeld,
            },
            LeaseRecord::EventDropped {
                lease: lease.clone(),
                turn: ExecutionId::new("turn-1").unwrap(),
                cursor: 42,
            },
            LeaseRecord::Unreadable {
                kind: "sleeping".into(),
                body: r#"{"since":3}"#.into(),
            },
        ];
        for cause in [
            LeaseEndCause::Closed,
            LeaseEndCause::Revoked,
            LeaseEndCause::Expired,
            LeaseEndCause::Lost,
        ] {
            records.push(LeaseRecord::Ending {
                lease: lease.clone(),
                cause,
                actor: None,
            });
        }
        records
    }
    fn batch(kind: &str, body: &str) -> String {
        format!(r#"{{"changes":[{{"Lease":{{"kind":"{kind}","body":"{body}"}}}}]}}"#)
    }
    fn corrupt_batch(bytes: &str) -> bool {
        matches!(
            decode_batch(bytes.as_bytes(), &ProviderContext::Absent),
            Err(StorageError::Corrupt(_))
        )
    }

    #[test]
    fn every_lease_record_round_trips_through_the_semantic_stream() {
        let changes: Vec<_> = every_kind().into_iter().map(SessionChange::Lease).collect();
        let bytes = encode_batch(&changes).unwrap();
        let decoded = decode_batch(&bytes, &ProviderContext::Absent).unwrap();
        assert_eq!(decoded, changes);
    }

    #[test]
    fn an_unknown_kind_is_kept_as_written_and_written_back_byte_for_byte() {
        let bytes = batch("paused", r#"{\"disk\":1}"#);
        let decoded = decode_batch(bytes.as_bytes(), &ProviderContext::Absent).unwrap();
        assert_eq!(
            decoded,
            vec![SessionChange::Lease(LeaseRecord::Unreadable {
                kind: "paused".into(),
                body: r#"{"disk":1}"#.into(),
            })]
        );
        assert_eq!(encode_batch(&decoded).unwrap(), bytes.as_bytes());
    }

    #[test]
    fn a_known_kind_whose_body_does_not_decode_is_corrupt() {
        let issued = r#"{\"lease\":\"a\",\"revision\":0,\"terms\":{\"environment\":\"Here\",\"work\":{\"Agent\":{\"agent\":\"a\",\"model\":\"m\"}},\"sandbox\":\"HarnessDefault\",\"grants\":\"Opening\",\"deadline\":\"UntilEnded\"},\"actor\":{\"principal_id\":\"p\",\"surface_id\":\"s\",\"request_id\":\"r\"}}"#;
        let cases = [
            ("issued", "not json"),
            ("issued", issued),
            (
                "issued",
                &issued
                    .replace("\\\"revision\\\":0", "\\\"revision\\\":1")
                    .replace("\\\"agent\\\":\\\"a\\\"", "\\\"agent\\\":\\\" \\\""),
            ),
            (
                "ending",
                r#"{\"lease\":\"bad id\",\"cause\":\"Stopped\",\"actor\":null}"#,
            ),
            ("ended", r#"{\"lease\":\"bad id\",\"cleanup\":\"NotHeld\"}"#),
            ("interrupted", r#"{\"lease\":\"\"}"#),
            (
                "event_dropped",
                r#"{\"lease\":\"a\",\"turn\":\"\",\"cursor\":0}"#,
            ),
            (
                "event_dropped",
                r#"{\"lease\":\"\",\"turn\":\"t\",\"cursor\":0}"#,
            ),
        ];
        for (kind, body) in cases {
            assert!(corrupt_batch(&batch(kind, body)), "{kind} {body}");
        }
    }

    #[test]
    fn an_issuance_slice_a_cannot_hold_is_kept_under_a_kind_a_slice_a_build_does_not_read() {
        let on_host = LeaseTerms {
            environment: EnvironmentRef::Ssh(SshDestination::new("me@devbox").unwrap()),
            ..terms(LeaseDeadline::UntilEnded)
        };
        let cases = [
            (
                LeaseRecord::Issued {
                    lease: id("lease-1"),
                    revision: LeaseRevision::FIRST,
                    terms: on_host.clone(),
                    actor: actor(),
                },
                ISSUED_V2,
            ),
            (
                LeaseRecord::Refused {
                    lease: id("lease-1"),
                    revision: LeaseRevision::FIRST,
                    terms: on_host,
                    refusal: LeaseRefusal::EnvironmentVersionMismatch,
                    actor: actor(),
                },
                REFUSED_V2,
            ),
            (
                // Here, with a refusal only a host gives.
                LeaseRecord::Refused {
                    lease: id("lease-1"),
                    revision: LeaseRevision::FIRST,
                    terms: terms(LeaseDeadline::UntilEnded),
                    refusal: LeaseRefusal::AgentUnavailable,
                    actor: actor(),
                },
                REFUSED_V2,
            ),
        ];
        for (record, kind) in cases {
            let wire = WireLease::from(&record);
            assert_eq!(wire.kind, kind);
            // Read back exactly.
            assert_eq!(
                WireLease {
                    kind: wire.kind.clone(),
                    body: wire.body.clone(),
                }
                .decode()
                .unwrap(),
                record
            );
            // Under the first kinds it is corrupt, never a lease here.
            let first = if kind == ISSUED_V2 { ISSUED } else { REFUSED };
            assert!(matches!(
                WireLease {
                    kind: first.into(),
                    body: wire.body,
                }
                .decode(),
                Err(StorageError::Corrupt(_))
            ));
        }
        // What slice A wrote is still written and read as it was.
        assert_eq!(WireLease::from(&every_kind()[0]).kind, ISSUED);
        assert_eq!(WireLease::from(&every_kind()[1]).kind, REFUSED);
    }

    #[test]
    fn a_stored_host_that_is_not_a_destination_is_corrupt() {
        let record = LeaseRecord::Issued {
            lease: id("lease-1"),
            revision: LeaseRevision::FIRST,
            terms: LeaseTerms {
                environment: EnvironmentRef::Ssh(SshDestination::new("devbox").unwrap()),
                ..terms(LeaseDeadline::UntilEnded)
            },
            actor: actor(),
        };
        let wire = WireLease::from(&record);
        assert!(matches!(
            WireLease {
                kind: wire.kind,
                body: wire.body.replace("devbox", "-oProxyCommand=x"),
            }
            .decode(),
            Err(StorageError::Corrupt(_))
        ));
    }

    #[test]
    fn an_issuance_and_its_refusal_must_agree() {
        let records = every_kind();
        let issued = WireLease::from(&records[0]);
        let refused = WireLease::from(&records[1]);
        for (kind, body) in [("refused", issued.body), ("issued", refused.body)] {
            assert!(matches!(
                WireLease {
                    kind: kind.into(),
                    body
                }
                .decode(),
                Err(StorageError::Corrupt(_))
            ));
        }
    }

    #[test]
    fn lease_text_past_its_bound_is_refused_before_it_is_read() {
        let long_kind = "k".repeat(KIND_BYTES + 1);
        let long_body = "b".repeat(BODY_BYTES + 1);
        assert!(corrupt_batch(&batch(&long_kind, "{}")));
        assert!(corrupt_batch(&batch("issued", &long_body)));
        for wire in [
            WireLease {
                kind: long_kind,
                body: String::new(),
            },
            WireLease {
                kind: "paused".into(),
                body: long_body,
            },
        ] {
            assert!(matches!(wire.decode(), Err(StorageError::Corrupt(_))));
        }
        let extra = r#"{"changes":[{"Lease":{"kind":"issued","body":"{}","more":1}}]}"#;
        assert!(corrupt_batch(extra));
    }

    fn checkpoint(snapshot: &SessionSnapshot) -> String {
        let text = serde_json::to_string(&SnapshotRef(snapshot)).unwrap();
        preflight_checkpoint(format!(r#"{{"snapshot":{text}}}"#).as_bytes()).unwrap();
        text
    }
    fn decode(text: &str) -> Result<SessionSnapshot, StorageError> {
        serde_json::from_str::<Snapshot>(text).unwrap().decode()
    }

    #[test]
    fn a_checkpoint_without_a_lease_is_written_as_before_leases_existed() {
        let text = checkpoint(&history_fixture(1));
        assert!(!text.contains("lease"));
        assert_eq!(decode(&text).unwrap().lease, None);
    }

    #[test]
    fn a_checkpoint_keeps_the_current_lease_and_folds_it_again() {
        let records = every_kind();
        let mut lease = None;
        for record in [
            &records[0],
            &records[2],
            &records[4],
            &records[6],
            &records[5],
        ] {
            lease = Some(CurrentLease::apply(lease.as_ref(), record).unwrap());
        }
        let mut snapshot = history_fixture(1);
        snapshot.lease = lease;
        let text = checkpoint(&snapshot);
        assert_eq!(decode(&text).unwrap(), snapshot);

        let saved = r#""lease":{"revision":1,"#;
        assert!(text.contains(saved));
        for revision in ["2", "0"] {
            let changed = text.replace(saved, &format!(r#""lease":{{"revision":{revision},"#));
            assert!(matches!(decode(&changed), Err(StorageError::Corrupt(_))));
        }
        let bad_record = text.replace(r#""kind":"interrupted""#, r#""kind":"ended""#);
        assert!(matches!(decode(&bad_record), Err(StorageError::Corrupt(_))));
    }

    /// The conversation's second lease, issued or refused after the first
    /// ended. A checkpoint keeps only its records, which begin at revision 2.
    fn second_lease(second: LeaseRecord) -> CurrentLease {
        let first = id("lease-1");
        let mut lease = None;
        for record in [
            LeaseRecord::Issued {
                lease: first.clone(),
                revision: LeaseRevision::FIRST,
                terms: terms(LeaseDeadline::UntilEnded),
                actor: actor(),
            },
            LeaseRecord::Ending {
                lease: first.clone(),
                cause: LeaseEndCause::Closed,
                actor: None,
            },
            LeaseRecord::Ended {
                lease: first,
                cleanup: LeaseCleanup::NotHeld,
            },
            second,
        ] {
            lease = Some(CurrentLease::apply(lease.as_ref(), &record).unwrap());
        }
        lease.unwrap()
    }

    #[test]
    fn a_checkpoint_of_a_second_lease_reads_back_issued_or_refused() {
        let revision = LeaseRevision::new(2).unwrap();
        for second in [
            LeaseRecord::Issued {
                lease: id("lease-2"),
                revision,
                terms: terms(LeaseDeadline::UntilEnded),
                actor: actor(),
            },
            LeaseRecord::Refused {
                lease: id("lease-2"),
                revision,
                terms: terms(LeaseDeadline::UntilEnded),
                refusal: LeaseRefusal::SandboxUnavailable,
                actor: actor(),
            },
        ] {
            let mut snapshot = history_fixture(1);
            snapshot.lease = Some(second_lease(second));
            let text = checkpoint(&snapshot);
            assert_eq!(decode(&text).unwrap(), snapshot);
            // The saved revision is still the one its records must name.
            let saved = r#""lease":{"revision":2,"#;
            assert!(text.contains(saved));
            for revision in ["1", "3"] {
                let changed = text.replace(saved, &format!(r#""lease":{{"revision":{revision},"#));
                assert!(matches!(decode(&changed), Err(StorageError::Corrupt(_))));
            }
        }
    }
}
