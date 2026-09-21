//! What the durable file-link record holds, and what it refuses to overwrite.
use super::*;
use crate::conversation::domain::ConversationId;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use serde_json::Value;

fn linked(paths: &[&str], observed_at_ms: u64) -> ConversationFileLinkAuditRecord {
    ConversationFileLinkAuditRecord {
        conversation_id: ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap(),
        organization_id: OrganizationId::new("org").unwrap(),
        execution_id: "execution-1".into(),
        paths: paths.iter().map(|path| (*path).to_string()).collect(),
        before: ConversationFileLinkState::NotNamed,
        after: ConversationFileLinkState::Named,
        cause: ConversationFileLinkCause::CallerSubmitted,
        initiator_principal_id: PrincipalId::new("owner").unwrap(),
        initiator_surface_id: "panel".into(),
        observed_at_ms,
    }
}

fn sole_record(directory: &std::path::Path) -> Value {
    let mut entries: Vec<_> = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(entries.len(), 1, "{entries:?}");
    serde_json::from_slice(&std::fs::read(entries.pop().unwrap()).unwrap()).unwrap()
}

#[tokio::test]
async fn a_record_names_every_path_its_submission_and_who_pointed_the_agent_there() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("file-links");
    let audit = DurableConversationFileLinkAudit::new(directory.clone()).unwrap();

    // Two paths, in attachment order, one of them with characters that have to
    // survive being written down exactly.
    audit
        .record(linked(
            &["/Users/ada/report (final).pdf", "/tmp/отчёт.pdf"],
            110,
        ))
        .await
        .unwrap();

    let stored = sole_record(&directory);
    assert_eq!(stored["kind"], "conversation_files_named");
    assert_eq!(
        stored["target"]["conversationId"],
        "00000000-0000-4000-8000-000000000001"
    );
    assert_eq!(stored["target"]["organizationId"], "org");
    assert_eq!(stored["target"]["executionId"], "execution-1");
    assert_eq!(
        stored["target"]["paths"],
        serde_json::json!(["/Users/ada/report (final).pdf", "/tmp/отчёт.pdf"])
    );
    assert_eq!(stored["transition"]["before"], "not_named");
    assert_eq!(stored["transition"]["after"], "named");
    assert_eq!(stored["cause"], "caller_submitted");
    assert_eq!(stored["initiator"]["principalId"], "owner");
    assert_eq!(stored["initiator"]["surfaceId"], "panel");
    assert_eq!(stored["observedAtMs"], 110);
}

#[tokio::test]
async fn the_same_submission_reconciles_and_a_different_one_fails_closed() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("file-links");
    let audit = DurableConversationFileLinkAudit::new(directory.clone()).unwrap();

    // A retry of the same submission is the same grant seen again, not another.
    audit.record(linked(&["/tmp/a.pdf"], 110)).await.unwrap();
    audit.record(linked(&["/tmp/a.pdf"], 120)).await.unwrap();
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
    // The first observation is the one kept.
    assert_eq!(sole_record(&directory)["observedAtMs"], 110);

    // The same submission claiming other paths, another caller, or another
    // surface is evidence that contradicts what is already stored.
    for changed in [
        {
            let mut record = linked(&["/tmp/b.pdf"], 130);
            record.execution_id = "execution-1".into();
            record
        },
        {
            let mut record = linked(&["/tmp/a.pdf"], 130);
            record.initiator_principal_id = PrincipalId::new("someone-else").unwrap();
            record
        },
        {
            let mut record = linked(&["/tmp/a.pdf"], 130);
            record.initiator_surface_id = "another-surface".into();
            record
        },
    ] {
        assert!(matches!(
            audit.record(changed).await,
            Err(ConversationError::Audit)
        ));
    }
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);

    // A different submission in the same conversation is its own record.
    let mut other = linked(&["/tmp/a.pdf"], 140);
    other.execution_id = "execution-2".into();
    audit.record(other).await.unwrap();
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 2);
}

#[tokio::test]
async fn a_submission_identity_is_hashed_rather_than_spelled_into_a_file_name() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("file-links");
    let audit = DurableConversationFileLinkAudit::new(directory.clone()).unwrap();

    // Whatever a caller sends as a submission identity, it never becomes part
    // of a path: this one would otherwise escape the directory entirely.
    let mut record = linked(&["/tmp/a.pdf"], 110);
    record.execution_id = "../../escaped/x\u{0}/y".into();
    audit.record(record).await.unwrap();

    let name = std::fs::read_dir(&directory)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .file_name();
    let name = name.to_str().unwrap();
    assert!(
        name.starts_with("conversation-files-00000000-0000-4000-8000-000000000001-"),
        "{name}"
    );
    assert!(
        name.strip_suffix(".json")
            .unwrap()
            .rsplit('-')
            .next()
            .unwrap()
            .chars()
            .all(|character| character.is_ascii_hexdigit()),
        "{name}"
    );
    // The identity itself is still recorded, in the record rather than the name.
    assert_eq!(
        sole_record(&directory)["target"]["executionId"],
        "../../escaped/x\u{0}/y"
    );
}

/// The reader both writers are held to, asked directly for each answer it can
/// give.
///
/// It is called at two moments — before publishing, and again by a writer that
/// lost the race for the name — and the second of those is the one a test
/// cannot stage through `record`: reaching it means arriving while another
/// writer holds the name, which is a matter of timing rather than of input.
/// So the reader is asked here for each of its outcomes on its own, and what
/// the two callers do with each outcome is then a line each.
///
/// The `Absent` answer is why the loser's arm is not a success. A name that is
/// taken and then gone is not evidence of anything: nothing in this crate
/// removes a record, so it means something outside did, and a writer that
/// treated it as agreement would report a durable grant with nothing durable
/// behind it.
#[test]
fn stored_evidence_answers_absent_agrees_or_refuses_and_never_guesses() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path();
    // Written the way the adapter writes: private and owned by this process.
    // A record this process could not have written is damage, and is its own
    // case below.
    let put = |path: &Path, bytes: &[u8]| {
        open(path, OpenMode::CreateNew)
            .unwrap()
            .write_all(bytes)
            .unwrap();
    };
    let value = serde_json::json!({"recordId": "r", "target": {"paths": ["/tmp/a.pdf"]}, "observedAtMs": 110});

    // Nothing there. The writer before a publish writes; the writer after a
    // lost publish refuses, because the record it lost to has gone.
    let missing = directory.join("missing.json");
    assert!(matches!(
        stored_evidence(&missing, &value, "r"),
        Ok(Stored::Absent)
    ));

    // The same evidence, with the one field two writers may honestly differ
    // about differing.
    let same = directory.join("same.json");
    let mut later = value.clone();
    later["observedAtMs"] = serde_json::json!(999);
    put(&same, &serde_json::to_vec(&later).unwrap());
    assert!(matches!(
        stored_evidence(&same, &value, "r"),
        Ok(Stored::Agrees)
    ));

    // Evidence that contradicts this submission about anything else.
    let other = directory.join("other.json");
    let mut different = value.clone();
    different["target"]["paths"] = serde_json::json!(["/tmp/b.pdf"]);
    put(&other, &serde_json::to_vec(&different).unwrap());
    assert!(matches!(
        stored_evidence(&other, &value, "r"),
        Err(ConversationError::Audit)
    ));

    // Bytes that are not the JSON this writes, and bytes larger than any
    // record this writes. Both are damage rather than disagreement, and both
    // fail closed rather than being repaired.
    let junk = directory.join("junk.json");
    put(&junk, b"not json at all");
    assert!(matches!(
        stored_evidence(&junk, &value, "r"),
        Err(ConversationError::Audit)
    ));
    let huge = directory.join("huge.json");
    put(&huge, &[b'x'; MAX_RECORD_BYTES + 1]);
    assert!(matches!(
        stored_evidence(&huge, &value, "r"),
        Err(ConversationError::Audit)
    ));

    // And a name that is taken by something that is not a private record at
    // all is read as damage, not as absence.
    let folder = directory.join("folder.json");
    std::fs::create_dir(&folder).unwrap();
    assert!(matches!(
        stored_evidence(&folder, &value, "r"),
        Err(ConversationError::Audit)
    ));
}

/// Two writers of the same submission that both find nothing stored. The read
/// is a courtesy, not the exclusion — what settles it is a create-only publish,
/// so exactly one record exists afterwards and the loser is held to it rather
/// than overwriting it.
///
/// Repeated, because the losers are the point and they are the half a quiet
/// machine skips. Every loser reads the winner's record back, and for that read
/// to answer, the winner's publication must have been complete the instant the
/// name existed. It once was not: the publish linked the name and released its
/// own afterwards, and a loser arriving between those two calls found a file
/// with two links, which local storage refuses as unsafe. The loser then
/// refused a submission that was perfectly well recorded — on a busy machine,
/// and nowhere else.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_writers_of_one_submission_leave_exactly_one_record() {
    for attempt in 0..32 {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("file-links");
        let audit =
            std::sync::Arc::new(DurableConversationFileLinkAudit::new(directory.clone()).unwrap());

        // Started together and released together, so all of them are past
        // their read before any has published.
        let gate = std::sync::Arc::new(tokio::sync::Barrier::new(8));
        let mut writing = Vec::new();
        for observed in 0..8 {
            let audit = audit.clone();
            let gate = gate.clone();
            writing.push(tokio::spawn(async move {
                gate.wait().await;
                audit.record(linked(&["/tmp/a.pdf"], 100 + observed)).await
            }));
        }
        for finished in futures_util::future::join_all(writing).await {
            // Every one of them succeeds: the same evidence, agreed, however
            // the race went. A lost publish is not a failed audit.
            finished.unwrap().unwrap_or_else(|error| {
                panic!("attempt {attempt}: agreeing writers all succeed: {error:?}")
            });
        }
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);

        // And a disagreeing writer arriving afterwards still fails closed.
        let mut other = linked(&["/tmp/b.pdf"], 200);
        other.execution_id = "execution-1".into();
        assert!(matches!(
            audit.record(other).await,
            Err(ConversationError::Audit)
        ));
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
    }
}

/// The case that actually needs a create-only publish: two writers of the same
/// submission naming *different* paths, both past their read before either has
/// written. Overwriting here would lose one caller's evidence and report
/// success to both, leaving a trail that names one file and nothing to say the
/// other was ever claimed. Exactly one of them must be refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_of_two_disagreeing_writers_is_refused_rather_than_overwriting() {
    for attempt in 0..32 {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("file-links");
        let audit =
            std::sync::Arc::new(DurableConversationFileLinkAudit::new(directory.clone()).unwrap());
        let gate = std::sync::Arc::new(tokio::sync::Barrier::new(2));

        let mut racing = Vec::new();
        for path in ["/tmp/a.pdf", "/tmp/b.pdf"] {
            let audit = audit.clone();
            let gate = gate.clone();
            racing.push(tokio::spawn(async move {
                gate.wait().await;
                audit.record(linked(&[path], 110)).await
            }));
        }
        let results: Vec<_> = futures_util::future::join_all(racing)
            .await
            .into_iter()
            .map(|joined| joined.unwrap())
            .collect();

        let refused = results.iter().filter(|result| result.is_err()).count();
        assert_eq!(
            refused, 1,
            "attempt {attempt}: {refused} of two disagreeing writers were refused"
        );
        // One record, and it is one of the two — not a mixture, and not the
        // second one written over the first while both were told they had won.
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        let stored = sole_record(&directory);
        let paths = stored["target"]["paths"].as_array().unwrap();
        assert_eq!(paths.len(), 1);
        assert!(
            ["/tmp/a.pdf", "/tmp/b.pdf"].contains(&paths[0].as_str().unwrap()),
            "{stored}"
        );
    }
}

#[tokio::test]
async fn a_directory_that_cannot_be_written_is_a_visible_audit_failure() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("file-links");
    let audit = DurableConversationFileLinkAudit::new(directory.clone()).unwrap();
    std::fs::remove_dir_all(&directory).unwrap();

    assert!(matches!(
        audit.record(linked(&["/tmp/a.pdf"], 110)).await,
        Err(ConversationError::Audit)
    ));
}
