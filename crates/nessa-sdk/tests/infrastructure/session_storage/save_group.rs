//! Pure envelope/publication checks; real writer/source/restart checks are separate.
use super::*;
use event_stream::{IncarnationId, StreamId};
use nessa_sync::replication::domain::Id;
use serde_json::Value;

fn stream() -> StreamKey {
    StreamKey {
        id: StreamId::new("conversation").unwrap(),
        incarnation: IncarnationId([7; 16]),
    }
}
fn identity(base: u64, generation: u64) -> SaveIdentity {
    SaveIdentity::binding(&SessionSaveGeneration::new(
        SessionSaveBackend::Record {
            stream: Id::new("conversation").unwrap(),
            incarnation: [7; 16],
        },
        base,
        generation,
    ))
    .unwrap()
}
fn accept(
    progress: &mut GroupProgress,
    kind: FactKind,
    header: &Header,
    payload: &[u8],
    position: u64,
) -> Result<Header, StorageError> {
    let bytes = header.encode(payload);
    // Exercise a header crossing physical-piece boundaries rather than an inline-only decoder.
    for piece in bytes.chunks(13) {
        progress.piece(piece)?;
    }
    progress.complete(&FactKey::new(kind, None, header.ordinal).unwrap(), position)
}

#[test]
fn units_and_aborts_do_not_publish_and_only_exact_completion_advances() {
    let mut progress = GroupProgress::after(0);
    let unit = Header::unit(identity(0, 0), 0, EMPTY_CHAIN, b"first");
    assert!(unit.identity.matches_stream(&stream()));
    accept(&mut progress, FactKind::SaveUnit, &unit, b"first", 3).unwrap();
    assert!(progress.is_unfinished());
    assert_eq!(progress.published(), 0);
    assert!(progress.checkpoint().is_none());
    progress.piece(b"partial next frame").unwrap();
    progress.reset_frame();
    assert!(progress.is_unfinished());
    assert_eq!(progress.published(), 0);
    let complete = Header::unit(identity(0, 0), 1, unit.chain(5), b"");
    accept(&mut progress, FactKind::SaveComplete, &complete, b"", 7).unwrap();
    assert!(!progress.is_unfinished());
    assert_eq!(progress.published(), 7);
    assert!(accept(&mut progress, FactKind::SaveComplete, &complete, b"", 8).is_err());
}

#[test]
fn completed_prefix_stays_published_during_same_generation_extension_and_checkpoint_restage() {
    let mut progress = GroupProgress::after(0);
    let first = Header::unit(identity(0, 0), 0, EMPTY_CHAIN, b"P");
    accept(&mut progress, FactKind::SaveUnit, &first, b"P", 1).unwrap();
    let p_complete = Header::unit(identity(0, 0), 1, first.chain(1), b"");
    accept(&mut progress, FactKind::SaveComplete, &p_complete, b"", 2).unwrap();
    let p_checkpoint = progress.checkpoint();
    let second = Header::unit(identity(0, 0), 1, first.chain(1), b"Q");
    accept(&mut progress, FactKind::SaveUnit, &second, b"Q", 3).unwrap();
    assert_eq!(progress.published(), 2);
    let incarnation = Uuid::from_bytes([7; 16]).simple().to_string();
    let mut reopened =
        GroupProgress::restore(2, progress.checkpoint(), "conversation", &incarnation, 1).unwrap();
    // The checkpoint represents P, so Q's actual downloaded journal is restaged once.
    accept(&mut reopened, FactKind::SaveUnit, &second, b"Q", 3).unwrap();
    let q_complete = Header::unit(identity(0, 0), 2, second.chain(1), b"");
    accept(&mut reopened, FactKind::SaveComplete, &q_complete, b"", 4).unwrap();
    assert_eq!(reopened.published(), 4);
    assert!(GroupProgress::restore(2, p_checkpoint.clone(), "other", &incarnation, 1).is_err());
    assert!(
        GroupProgress::restore(2, p_checkpoint.clone(), "conversation", &incarnation, 0).is_err()
    );
    assert!(GroupProgress::restore(2, p_checkpoint, "conversation", &incarnation, 2).is_err());
    // Reopened Q retains two original-generation unit facts, including P's prefix.
    let mut q_reopened =
        GroupProgress::restore(4, reopened.checkpoint(), "conversation", &incarnation, 2).unwrap();
    let third = Header::unit(identity(4, 1), 0, EMPTY_CHAIN, b"R");
    accept(&mut q_reopened, FactKind::SaveUnit, &third, b"R", 5).unwrap();
    let r_complete = Header::unit(identity(4, 1), 1, third.chain(1), b"");
    accept(&mut q_reopened, FactKind::SaveComplete, &r_complete, b"", 6).unwrap();
    // Last-completion metadata owns R's one unit, not P/Q's cumulative total.
    let r_checkpoint = q_reopened.checkpoint();
    assert!(
        GroupProgress::restore(6, r_checkpoint.clone(), "conversation", &incarnation, 3).is_ok()
    );
    let mut wrong_base = r_checkpoint.clone().unwrap();
    wrong_base.identity.base = 0;
    assert!(GroupProgress::restore(6, Some(wrong_base), "conversation", &incarnation, 3).is_err());
    let mut wrong_generation = r_checkpoint.unwrap();
    wrong_generation.identity.generation = 0;
    assert!(
        GroupProgress::restore(6, Some(wrong_generation), "conversation", &incarnation, 3).is_err()
    );
}

#[test]
fn changed_payload_boundary_scope_base_generation_and_count_refuse() {
    let unit = Header::unit(identity(0, 0), 0, EMPTY_CHAIN, b"original");
    let mut changed = GroupProgress::after(0);
    assert!(accept(&mut changed, FactKind::SaveUnit, &unit, b"changed", 1).is_err());
    for header in [
        Header::unit(identity(1, 0), 0, EMPTY_CHAIN, b"original"),
        Header::unit(identity(0, 1), 0, EMPTY_CHAIN, b"original"),
        Header::unit(identity(0, 0), 1, EMPTY_CHAIN, b"original"),
    ] {
        assert!(accept(
            &mut GroupProgress::after(0),
            FactKind::SaveUnit,
            &header,
            b"original",
            1
        )
        .is_err());
    }
    let mut progress = GroupProgress::after(0);
    accept(&mut progress, FactKind::SaveUnit, &unit, b"original", 1).unwrap();
    let bad = Header::unit(identity(0, 0), 2, unit.chain(8), b"");
    assert!(accept(&mut progress, FactKind::SaveComplete, &bad, b"", 2).is_err());
    let foreign = StreamKey {
        id: stream().id,
        incarnation: IncarnationId([8; 16]),
    };
    assert!(!unit.identity.matches_stream(&foreign));
}

fn push_actual_key(
    progress: &mut GroupProgress,
    header: &Header,
    payload: &[u8],
    kind: FactKind,
    ordinal: u64,
    position: u64,
) -> Result<Header, StorageError> {
    progress.piece(&header.encode(payload))?;
    progress.complete(&FactKey::new(kind, None, ordinal).unwrap(), position)
}
fn completed_progress() -> GroupProgress {
    let mut progress = GroupProgress::after(0);
    let unit = Header::unit(identity(0, 0), 0, EMPTY_CHAIN, b"unit");
    push_actual_key(&mut progress, &unit, b"unit", FactKind::SaveUnit, 0, 1).unwrap();
    let complete = Header::unit(identity(0, 0), 1, unit.chain(4), &[]);
    push_actual_key(&mut progress, &complete, &[], FactKind::SaveComplete, 1, 2).unwrap();
    assert_eq!(progress.published(), 2);
    progress
}
#[test]
fn save_envelope_refuses_independent_digest_ordinal_chain_and_identity_contradictions() {
    let valid = Header::unit(identity(0, 0), 0, EMPTY_CHAIN, b"unit");
    let cases = [
        (valid.clone(), 1),
        (
            Header {
                previous: [2; 32],
                ..valid.clone()
            },
            0,
        ),
    ];
    for (header, ordinal) in cases {
        let mut progress = GroupProgress::after(0);
        assert!(push_actual_key(
            &mut progress,
            &header,
            b"unit",
            FactKind::SaveUnit,
            ordinal,
            1
        )
        .is_err());
        assert_eq!(progress.published(), 0);
    }
    for foreign in [
        identity(1, 1),
        identity(2, 2),
        SaveIdentity {
            stream: [2; 32],
            ..identity(2, 1)
        },
        SaveIdentity {
            incarnation: [2; 16],
            ..identity(2, 1)
        },
    ] {
        let mut progress = completed_progress();
        let header = Header::unit(foreign, 0, EMPTY_CHAIN, b"unit");
        assert!(
            push_actual_key(&mut progress, &header, b"unit", FactKind::SaveUnit, 0, 3).is_err()
        );
        assert_eq!(progress.published(), 2);
    }
    let mut good = completed_progress();
    let header = Header::unit(identity(2, 1), 0, EMPTY_CHAIN, b"unit");
    push_actual_key(&mut good, &header, b"unit", FactKind::SaveUnit, 0, 3).unwrap();
    assert_eq!(good.published(), 2);
    for bad in [
        Header::unit(identity(2, 1), 2, header.chain(4), b"next"),
        Header::unit(identity(2, 1), 1, [2; 32], b"next"),
    ] {
        let mut progress = good.clone();
        assert!(push_actual_key(
            &mut progress,
            &bad,
            b"next",
            FactKind::SaveUnit,
            bad.ordinal,
            4
        )
        .is_err());
        assert_eq!(progress.published(), 2);
    }
    let complete = Header::unit(identity(2, 1), 1, header.chain(4), &[]);
    push_actual_key(&mut good, &complete, &[], FactKind::SaveComplete, 1, 4).unwrap();
    assert_eq!(good.published(), 4);
}

#[test]
fn metadata_declaration_keeps_wire_and_preflight_field_authority_in_agreement() {
    let identity = identity(0, 0);
    let value = serde_json::to_value(&identity).unwrap();
    assert_eq!(
        serde_json::from_value::<SaveIdentity>(value.clone()).unwrap(),
        identity
    );
    let mut identity_keys = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    identity_keys.sort_unstable();
    assert_eq!(
        identity_keys,
        vec!["base", "generation", "incarnation", "stream"]
    );
    for (field, value) in value.as_object().unwrap() {
        match (SaveIdentity::field_kind(field).unwrap(), value) {
            (MetadataValueKind::FixedBytes(width), Value::Array(bytes)) => {
                assert_eq!(width, bytes.len())
            }
            (MetadataValueKind::Number, Value::Number(_)) => (),
            _ => panic!("metadata field kind disagrees with typed identity"),
        }
    }
    let mut progress = GroupProgress::after(0);
    let unit = Header::unit(identity.clone(), 0, EMPTY_CHAIN, b"unit");
    accept(&mut progress, FactKind::SaveUnit, &unit, b"unit", 1).unwrap();
    let complete = Header::unit(identity, 1, unit.chain(4), b"");
    accept(&mut progress, FactKind::SaveComplete, &complete, b"", 2).unwrap();
    let checkpoint = serde_json::to_value(progress.checkpoint().unwrap()).unwrap();
    let mut checkpoint_keys = checkpoint
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    checkpoint_keys.sort_unstable();
    assert_eq!(checkpoint_keys, vec!["chain", "count", "identity"]);
    let roundtrip: GroupCheckpoint = serde_json::from_value(checkpoint.clone()).unwrap();
    assert_eq!(serde_json::to_value(roundtrip).unwrap(), checkpoint);
    for (field, value) in checkpoint.as_object().unwrap() {
        match (GroupCheckpoint::field_kind(field).unwrap(), value) {
            (MetadataValueKind::Identity, Value::Object(_)) => (),
            (MetadataValueKind::FixedBytes(width), Value::Array(bytes)) => {
                assert_eq!(width, bytes.len())
            }
            (MetadataValueKind::Number, Value::Number(_)) => (),
            _ => panic!("metadata field kind disagrees with typed checkpoint"),
        }
    }
    assert!(SaveIdentity::field_kind("unexpected").is_none());
    assert!(GroupCheckpoint::field_kind("unexpected").is_none());
    let mut foreign_identity = value;
    foreign_identity
        .as_object_mut()
        .unwrap()
        .insert("unexpected".into(), serde_json::json!(0));
    assert!(serde_json::from_value::<SaveIdentity>(foreign_identity).is_err());
    let mut foreign_checkpoint = checkpoint;
    foreign_checkpoint
        .as_object_mut()
        .unwrap()
        .insert("unexpected".into(), serde_json::json!(0));
    assert!(serde_json::from_value::<GroupCheckpoint>(foreign_checkpoint).is_err());
}

#[test]
fn short_header_refuses_without_partial_decode_and_exact_header_accepts() {
    let header = Header::unit(identity(0, 0), 0, EMPTY_CHAIN, b"unit");
    let encoded = header.encode(&[]);
    for length in [0, 31, 47, 55, 63, 71, 103, HEADER_BYTES - 1] {
        assert!(Header::decode(&encoded[..length]).is_err());
    }
    assert_eq!(Header::decode(&encoded).unwrap(), header);
}

#[test]
fn empty_unit_and_completion_without_original_unit_refuse() {
    for kind in [FactKind::SaveUnit, FactKind::SaveComplete] {
        let mut progress = GroupProgress::after(0);
        let empty = Header::unit(identity(0, 0), 0, EMPTY_CHAIN, &[]);
        assert!(accept(&mut progress, kind, &empty, &[], 1).is_err());
        assert_eq!(progress.published(), 0);
        assert!(progress.checkpoint().is_none());
    }
    assert_eq!(completed_progress().published(), 2);
}

#[test]
fn completion_payload_refuses_and_exact_empty_completion_accepts() {
    let mut progress = GroupProgress::after(0);
    let unit = Header::unit(identity(0, 0), 0, EMPTY_CHAIN, b"unit");
    accept(&mut progress, FactKind::SaveUnit, &unit, b"unit", 1).unwrap();
    let carrying = Header::unit(identity(0, 0), 1, unit.chain(4), b"extra");
    assert!(accept(
        &mut progress,
        FactKind::SaveComplete,
        &carrying,
        b"extra",
        2
    )
    .is_err());
    assert_eq!(progress.published(), 0);
    progress.reset_frame();
    let exact = Header::unit(identity(0, 0), 1, unit.chain(4), &[]);
    accept(&mut progress, FactKind::SaveComplete, &exact, &[], 2).unwrap();
    assert_eq!(progress.published(), 2);
}

#[test]
fn unfinished_identity_switch_refuses_then_original_completion_accepts() {
    let mut progress = GroupProgress::after(0);
    let unit = Header::unit(identity(0, 0), 0, EMPTY_CHAIN, b"unit");
    accept(&mut progress, FactKind::SaveUnit, &unit, b"unit", 1).unwrap();
    // Same stream/incarnation, current published base and exact next generation;
    // only the prior group's unfinished ownership prevents this switch.
    let next = Header::unit(identity(0, 1), 0, EMPTY_CHAIN, b"next");
    assert!(accept(&mut progress, FactKind::SaveUnit, &next, b"next", 2).is_err());
    assert_eq!(progress.published(), 0);
    progress.reset_frame();
    let exact = Header::unit(identity(0, 0), 1, unit.chain(4), &[]);
    accept(&mut progress, FactKind::SaveComplete, &exact, &[], 2).unwrap();
    assert_eq!(progress.published(), 2);
}

#[test]
fn repeated_exact_completion_refuses_without_advancing_publication() {
    let mut progress = completed_progress();
    let unit = Header::unit(identity(0, 0), 0, EMPTY_CHAIN, b"unit");
    let complete = Header::unit(identity(0, 0), 1, unit.chain(4), &[]);
    assert!(accept(&mut progress, FactKind::SaveComplete, &complete, &[], 3).is_err());
    assert_eq!(progress.published(), 2);
    assert!(!progress.is_unfinished());
    progress.reset_frame();
    let next = Header::unit(identity(2, 1), 0, EMPTY_CHAIN, b"next");
    accept(&mut progress, FactKind::SaveUnit, &next, b"next", 3).unwrap();
    let exact = Header::unit(identity(2, 1), 1, next.chain(4), &[]);
    accept(&mut progress, FactKind::SaveComplete, &exact, &[], 4).unwrap();
    assert_eq!(progress.published(), 4);
}
