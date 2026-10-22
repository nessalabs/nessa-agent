//! Durable acceptance intent, completion, and on-disk generation agreement.

use super::{
    current_journal_is_trusted, current_staging_id, JournalFields, PromotionCause, SealFields,
};
use crate::backup::domain::seal_attestation;

fn row<'a>(
    after: &'a str,
    cause: &'a str,
    generation: &'a str,
    digest: &'a str,
) -> JournalFields<'a> {
    JournalFields {
        seal: SealFields {
            target: "/live",
            before: "verified",
            after,
            cause,
            decision: "effect",
            attestations: "lease-1 executor-stopped\n",
        },
        journal_digest: digest,
        root_generation: generation,
    }
}

fn trusted(lines: &[JournalFields<'_>], digest: &str, marker: Option<&str>) -> bool {
    current_journal_is_trusted(lines.iter().copied(), "/live", digest, marker)
}

#[test]
fn pending_acceptance_invalidates_an_older_live_generation() {
    let old_intent = row("promoting", "accepting", "stage-old", "digest-old");
    let old_live = row("live", "accepted", "stage-old", "digest-old");
    let new_intent = row("promoting", "accepting", "stage-new", "digest-new");
    assert!(trusted(
        &[old_intent, old_live],
        "digest-old",
        Some("stage-old")
    ));
    // The newer live completion was never stored. Restoring the old digest and
    // marker cannot revive its historical receipt.
    let lines = [old_intent, old_live, new_intent];
    assert!(!trusted(&lines, "digest-old", Some("stage-old")));
    assert!(!trusted(&lines, "digest-new", Some("stage-new")));
    let new_live = row("live", "accepted", "stage-new", "digest-new");
    let complete = [old_intent, old_live, new_intent, new_live];
    assert!(trusted(&complete, "digest-new", Some("stage-new")));
    assert!(!trusted(&complete, "digest-old", Some("stage-old")));
}

#[test]
fn live_authority_requires_intent_digest_and_marker_agreement() {
    let intent = row("promoting", "accepting", "stage-current", "digest-current");
    let live = row("live", "accepted", "stage-current", "digest-current");
    assert!(!trusted(&[live], "digest-current", Some("stage-current")));
    assert!(trusted(
        &[intent, live],
        "digest-current",
        Some("stage-current")
    ));
    assert!(!trusted(&[intent, live], "digest-current", None));
    assert!(!trusted(
        &[intent, live],
        "digest-current",
        Some("stage-old")
    ));
    assert!(!trusted(
        &[intent, live],
        "digest-old",
        Some("stage-current")
    ));
    for contradictory in [
        row("live", "accepted", "stage-other", "digest-current"),
        row("live", "accepted", "stage-current", "digest-other"),
    ] {
        assert!(!trusted(
            &[intent, contradictory],
            "digest-current",
            Some("stage-current")
        ));
        assert!(!trusted(
            &[intent, contradictory, live],
            "digest-current",
            Some("stage-current")
        ));
    }
    // Repeating an already completed receipt preserves the same authority.
    assert!(trusted(
        &[intent, live, live],
        "digest-current",
        Some("stage-current")
    ));
}

#[test]
fn malformed_or_same_digest_new_intent_does_not_restore_old_authority() {
    let intent = row("promoting", "accepting", "stage-old", "same-digest");
    let live = row("live", "accepted", "stage-old", "same-digest");
    for next in [
        row("promoting", "accepting", "stage-new", "same-digest"),
        row("promoting", "accepting", "", "same-digest"),
        row("promoting", "accepting", "stage-new", ""),
        row("promoting", "wrong-cause", "stage-new", "same-digest"),
        row("live", "wrong-cause", "stage-old", "same-digest"),
    ] {
        assert!(!trusted(
            &[intent, live, next],
            "same-digest",
            Some("stage-old")
        ));
    }
    // A different root's intent cannot invalidate this root.
    let mut other = row("promoting", "accepting", "stage-other", "same-digest");
    other.seal.target = "/other";
    assert!(trusted(
        &[intent, live, other],
        "same-digest",
        Some("stage-old")
    ));
}

#[test]
fn every_promoted_outcome_retains_its_independent_facts_in_the_receipt() {
    let intent = row("promoting", "accepting", "stage-current", "digest-current");
    let mut causes = std::collections::BTreeSet::new();
    for durable in [false, true] {
        for retained in [false, true] {
            for restricted in [false, true] {
                let cause = PromotionCause::from_effects(durable, retained, restricted);
                assert!(
                    causes.insert(cause.as_str()),
                    "two physical outcomes share a cause"
                );
                assert_eq!(PromotionCause::parse(cause.as_str()), Some(cause));
                assert_eq!(cause.as_str().contains("unsynced"), !durable);
                assert_eq!(cause.as_str().contains("retained"), retained);
                assert_eq!(cause.as_str().contains("restricted"), restricted);
                let live = row("live", cause.as_str(), "stage-current", "digest-current");
                assert!(trusted(
                    &[intent, live],
                    "digest-current",
                    Some("stage-current")
                ));
            }
        }
    }
    assert_eq!(causes.len(), 8);
    assert_eq!(PromotionCause::parse("unknown-outcome"), None);
}

#[test]
fn a_seal_and_marker_without_ready_completion_leave_the_stage_pending() {
    let attestation = seal_attestation("digest-current", "stage-current");
    let mut sealed = row("verified", "sealed", "", "");
    sealed.seal.before = "absent";
    sealed.seal.attestations = &attestation;
    let ready = row("ready", "ready", "stage-current", "digest-current");
    assert_eq!(
        current_staging_id([sealed], "/live", "digest-current"),
        None
    );
    assert!(!trusted(&[sealed], "digest-current", Some("stage-current")));
    assert_eq!(
        current_staging_id([sealed, ready], "/live", "digest-current").as_deref(),
        Some("stage-current")
    );
    assert!(trusted(
        &[sealed, ready],
        "digest-current",
        Some("stage-current")
    ));
    assert!(!trusted(&[sealed, ready], "digest-current", None));
    assert!(!trusted(
        &[sealed, ready],
        "digest-current",
        Some("stage-other")
    ));
    assert!(trusted(
        &[sealed, ready, ready],
        "digest-current",
        Some("stage-current")
    ));
}

#[test]
fn orphan_or_mismatched_ready_receipts_do_not_create_stage_authority() {
    let attestation = seal_attestation("digest-current", "stage-current");
    let mut sealed = row("verified", "sealed", "", "");
    sealed.seal.before = "absent";
    sealed.seal.attestations = &attestation;
    let ready = row("ready", "ready", "stage-current", "digest-current");
    assert!(!trusted(&[ready], "digest-current", Some("stage-current")));
    for bad in [
        row("ready", "ready", "stage-other", "digest-current"),
        row("ready", "ready", "stage-current", "digest-other"),
        row("ready", "wrong-cause", "stage-current", "digest-current"),
    ] {
        assert!(!trusted(
            &[sealed, bad],
            "digest-current",
            Some("stage-current")
        ));
        assert!(!trusted(
            &[sealed, bad, ready],
            "digest-current",
            Some("stage-current")
        ));
    }
}

#[test]
fn a_new_pending_stage_supersedes_a_completed_stage_with_the_same_bytes() {
    let old_attestation = seal_attestation("same-digest", "stage-old");
    let new_attestation = seal_attestation("same-digest", "stage-new");
    let mut old_seal = row("verified", "sealed", "", "");
    old_seal.seal.before = "absent";
    old_seal.seal.attestations = &old_attestation;
    let old_ready = row("ready", "ready", "stage-old", "same-digest");
    let mut new_seal = old_seal;
    new_seal.seal.attestations = &new_attestation;
    let new_ready = row("ready", "ready", "stage-new", "same-digest");
    assert!(trusted(
        &[old_seal, old_ready],
        "same-digest",
        Some("stage-old")
    ));
    let pending = [old_seal, old_ready, new_seal];
    assert!(!trusted(&pending, "same-digest", Some("stage-old")));
    assert!(!trusted(&pending, "same-digest", Some("stage-new")));
    assert_eq!(current_staging_id(pending, "/live", "same-digest"), None);
    assert!(!trusted(
        &[old_seal, old_ready, new_seal, old_ready],
        "same-digest",
        Some("stage-old")
    ));
    assert!(trusted(
        &[old_seal, old_ready, new_seal, new_ready],
        "same-digest",
        Some("stage-new")
    ));
}

#[test]
fn spending_a_pending_or_ready_stage_prevents_later_receipt_revival() {
    let attestation = seal_attestation("digest-current", "stage-current");
    let mut sealed = row("verified", "sealed", "", "");
    sealed.seal.before = "absent";
    sealed.seal.attestations = &attestation;
    let ready = row("ready", "ready", "stage-current", "digest-current");
    let mut spent = row("unready", "audit", "", "");
    spent.seal.attestations = "stage-current";
    assert!(!trusted(
        &[sealed, spent, ready],
        "digest-current",
        Some("stage-current")
    ));
    assert!(!trusted(
        &[sealed, ready, spent, ready],
        "digest-current",
        Some("stage-current")
    ));
}
