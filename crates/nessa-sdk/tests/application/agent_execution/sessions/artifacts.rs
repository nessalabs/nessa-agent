//! Artifact records: the values one holds, the one rule a record is committed
//! and read back under, and the fold that appends it and takes it back.

use super::super::test_support::{record_log, snapshot, text, turn};
use super::*;
use crate::application::agent_execution::sessions::{
    records::continuation::Continuation, retained, CurrentLease, LeaseRecord, SessionChange,
};
use crate::domain::agent_execution::leases::{
    AgentWork, EnvironmentRef, LeaseDeadline, LeaseEndCause, LeaseGrants, LeaseRefusal,
    LeaseRevision, LeaseTerms, LeaseWork, SandboxProfile,
};

const DIGEST: &str = "sha256:00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff00ff";

fn actor(request: &str) -> ActionContext {
    ActionContext::new("person", "desktop", request).unwrap()
}
fn terms() -> LeaseTerms {
    LeaseTerms {
        environment: EnvironmentRef::Here,
        work: LeaseWork::Agent(AgentWork::new("claude", "sonnet").unwrap()),
        sandbox: SandboxProfile::HarnessDefault,
        grants: LeaseGrants::Opening,
        deadline: LeaseDeadline::UntilEnded,
    }
}
fn issued(lease: &str) -> LeaseRecord {
    LeaseRecord::Issued {
        lease: LeaseId::new(lease).unwrap(),
        revision: LeaseRevision::FIRST,
        terms: terms(),
        actor: actor("send"),
    }
}
fn file(size: u64) -> Result<PublishedFile, ArtifactValueError> {
    PublishedFile::new(
        Sha256Digest::parse(DIGEST).unwrap(),
        MediaType::parse("text/plain").unwrap(),
        size,
    )
}
fn artifact(lease: &str, turn: Option<&str>) -> ArtifactRecord {
    ArtifactRecord {
        lease: LeaseId::new(lease).unwrap(),
        turn: turn.map(|turn| ExecutionId::new(turn).unwrap()),
        name: ArtifactName::new("report.pdf").unwrap(),
        file: file(12).unwrap(),
        actor: actor("send"),
    }
}
fn fold(records: &[LeaseRecord]) -> Option<CurrentLease> {
    let mut current = None;
    for record in records {
        current = Some(CurrentLease::apply(current.as_ref(), record).unwrap());
    }
    current
}
/// A conversation with one accepted turn, `turn-1`, under `lease`.
fn under(lease: Option<CurrentLease>) -> SessionSnapshot {
    let mut snapshot = snapshot(vec![turn("turn-1", text(), Vec::new())]);
    snapshot.lease = lease;
    snapshot
}
fn known(snapshot: &SessionSnapshot) -> impl Fn(&ExecutionId) -> bool + '_ {
    |turn| {
        snapshot
            .invocations
            .iter()
            .any(|invocation| &invocation.request.execution_id == turn)
    }
}

#[test]
fn an_artifact_name_is_a_one_line_file_name() {
    let longest = "é".repeat(ArtifactName::MAX_BYTES / 2);
    for accepted in ["report.pdf", "..", " spaced name ", longest.as_str()] {
        assert_eq!(ArtifactName::new(accepted).unwrap().as_str(), accepted);
    }
    let too_long = "x".repeat(ArtifactName::MAX_BYTES + 1);
    for refused in [
        "",
        "a/b",
        "/",
        "nul\0",
        "tab\t",
        "line\n",
        "\u{7f}",
        "\u{85}",
        too_long.as_str(),
    ] {
        assert_eq!(
            ArtifactName::new(refused),
            Err(ArtifactValueError::Name),
            "{refused:?}"
        );
    }
}

#[test]
fn a_published_file_is_one_byte_to_its_bound() {
    for size in [1, PublishedFile::MAX_BYTES] {
        let file = file(size).unwrap();
        assert_eq!(file.size(), size);
        assert_eq!(file.digest().to_string(), DIGEST);
        assert_eq!(file.media_type().as_str(), "text/plain");
    }
    for size in [0, PublishedFile::MAX_BYTES + 1] {
        assert_eq!(file(size), Err(ArtifactValueError::Size));
    }
    let messages = [ArtifactValueError::Name, ArtifactValueError::Size].map(|e| e.to_string());
    assert_ne!(messages[0], messages[1]);
    assert!(messages[0].contains("255") && messages[1].contains("67108864"));
}

#[test]
fn a_record_under_the_live_lease_its_issuer_asked_for_is_admitted() {
    let snapshot = under(fold(&[issued("lease-1")]));
    for record in [
        artifact("lease-1", Some("turn-1")),
        artifact("lease-1", None),
    ] {
        assert_eq!(record.admit(&snapshot, known(&snapshot)), Ok(()));
        assert_eq!(record.admit_saved(&snapshot, known(&snapshot)), Ok(()));
    }
}

#[test]
fn a_record_that_is_not_the_live_lease_s_is_refused() {
    let ending = LeaseRecord::Ending {
        lease: LeaseId::new("lease-1").unwrap(),
        cause: LeaseEndCause::Closed,
        actor: None,
    };
    let refused = LeaseRecord::Refused {
        lease: LeaseId::new("lease-1").unwrap(),
        revision: LeaseRevision::FIRST,
        terms: terms(),
        refusal: LeaseRefusal::SandboxUnavailable,
        actor: actor("send"),
    };
    let mut other_actor = artifact("lease-1", None);
    other_actor.actor = actor("other");
    for (lease, record) in [
        (None, artifact("lease-1", None)),
        (fold(&[refused]), artifact("lease-1", None)),
        (fold(&[issued("lease-2")]), artifact("lease-1", None)),
        (
            fold(&[issued("lease-1"), ending]),
            artifact("lease-1", None),
        ),
        (fold(&[issued("lease-1")]), other_actor),
    ] {
        let snapshot = under(lease);
        assert_eq!(
            record.admit(&snapshot, known(&snapshot)),
            Err(ArtifactRefusal::NotThisLease)
        );
        assert!(matches!(
            record.admit_saved(&snapshot, known(&snapshot)),
            Err(StorageError::Corrupt(message)) if message.contains("not the latest live one")
        ));
    }
}

#[test]
fn a_record_under_an_unreadable_lease_is_refused_but_read_back_as_written() {
    let snapshot = under(fold(&[
        issued("lease-1"),
        LeaseRecord::Unreadable {
            kind: "paused".into(),
            body: "{}".into(),
        },
    ]));
    let record = artifact("lease-1", None);
    assert_eq!(
        record.admit(&snapshot, known(&snapshot)),
        Err(ArtifactRefusal::LeaseUnreadable)
    );
    assert_eq!(record.admit_saved(&snapshot, known(&snapshot)), Ok(()));
}

#[test]
fn a_record_naming_a_turn_never_accepted_is_refused() {
    let snapshot = under(fold(&[issued("lease-1")]));
    let record = artifact("lease-1", Some("never"));
    assert_eq!(
        record.admit(&snapshot, known(&snapshot)),
        Err(ArtifactRefusal::UnknownTurn)
    );
    assert!(record.admit_saved(&snapshot, known(&snapshot)).is_err());
}

#[test]
fn a_full_conversation_refuses_another_rather_than_dropping_one() {
    let mut snapshot = under(fold(&[issued("lease-1")]));
    snapshot.artifacts = vec![artifact("lease-1", None); ArtifactRecord::MAX_PER_CONVERSATION];
    assert_eq!(
        artifact("lease-1", None).admit(&snapshot, known(&snapshot)),
        Err(ArtifactRefusal::Full)
    );
    assert_eq!(validate_saved(&snapshot), Ok(()));
    snapshot.artifacts.push(artifact("lease-1", None));
    assert!(matches!(
        validate_saved(&snapshot),
        Err(StorageError::Corrupt(_))
    ));
}

#[test]
fn a_saved_artifact_must_name_a_turn_the_conversation_accepted() {
    let mut snapshot = under(None);
    snapshot.artifacts = vec![artifact("lease-1", Some("turn-1")), artifact("old", None)];
    assert_eq!(validate_saved(&snapshot), Ok(()));
    snapshot.artifacts.push(artifact("lease-1", Some("never")));
    assert!(matches!(
        validate_saved(&snapshot),
        Err(StorageError::Corrupt(message)) if message.contains("did not accept")
    ));
}

#[test]
fn every_refusal_explains_itself() {
    let messages: std::collections::HashSet<String> = [
        ArtifactRefusal::Full,
        ArtifactRefusal::NotThisLease,
        ArtifactRefusal::LeaseUnreadable,
        ArtifactRefusal::UnknownTurn,
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    assert_eq!(messages.len(), 4);
}

/// The fold appends an admitted record and accounts its bytes; a unit that
/// fails after it takes the record and its bytes back with it.
#[test]
fn the_fold_appends_an_artifact_and_a_failed_unit_takes_it_back() {
    let saved = under(None);
    let mut continuation = Continuation::empty();
    continuation.apply_unit(&record_log(&saved)).unwrap();
    continuation
        .apply_unit(&[SessionChange::Lease(issued("lease-1"))])
        .unwrap();
    let before = continuation.snapshot_bytes;

    let first = artifact("lease-1", Some("turn-1"));
    continuation
        .apply_unit(&[SessionChange::Artifact(first.clone())])
        .unwrap();
    let folded = continuation.snapshot.as_ref().unwrap();
    assert_eq!(folded.artifacts, vec![first.clone()]);
    assert_eq!(continuation.snapshot_bytes, retained::snapshot(folded));
    assert!(continuation.snapshot_bytes > before);
    let accounted = continuation.snapshot_bytes;

    // The second record is admitted and then taken back when the record
    // after it, under another lease, is refused.
    let refused = continuation.apply_unit(&[
        SessionChange::Artifact(artifact("lease-1", None)),
        SessionChange::Artifact(artifact("lease-2", None)),
    ]);
    assert!(matches!(refused, Err(StorageError::Corrupt(_))));
    let folded = continuation.snapshot.as_ref().unwrap();
    assert_eq!(folded.artifacts, vec![first]);
    assert_eq!(continuation.snapshot_bytes, accounted);
}

#[test]
fn an_artifact_before_the_conversation_opened_is_corrupt() {
    let mut continuation = Continuation::empty();
    assert!(matches!(
        continuation.apply_unit(&[SessionChange::Artifact(artifact("lease-1", None))]),
        Err(StorageError::Corrupt(message)) if message.contains("precedes session open")
    ));
}
