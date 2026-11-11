//! Observable offline-planning contract shared by application and CLI tests.

use serde_json::{json, Value};

pub fn assert_unavailable(value: &Value) {
    assert_eq!(value["status"], "inspected");
    let plan = &value["report"];
    assert_eq!(plan["activation"], "unavailable");
    assert_eq!(plan["sequence_status"], "proposal_unimplemented");
    assert_eq!(plan["requires_reverification"], true);
    assert_eq!(
        plan["verification"]["payload_compatibility"],
        "not_assessed"
    );
    assert_eq!(plan["verification"]["inventory_freshness"], "not_assessed");
    assert_eq!(plan["verification"]["restore_readiness"], "not_assessed");
    let prerequisites = plan["unresolved_prerequisites"].as_array().unwrap();
    for name in [
        "trusted_key_recovery",
        "independent_inventory_freshness_and_rollback_proof",
        "replacement_identity_policy",
        "old_source_and_remote_executor_fencing",
        "omission_acceptance_policy",
        "store_owner_payload_compatibility",
        "quarantine_implementation",
        "store_owner_import_implementation",
        "audited_authority_acceptance_implementation",
    ] {
        assert!(prerequisites.contains(&json!(name)), "missing {name}");
    }
    let sequence = plan["proposed_sequence"].as_array().unwrap();
    let position = |name| sequence.iter().position(|step| step == name).unwrap();
    assert_eq!(position("select_new_private_destination"), 0);
    assert!(
        position("persist_quarantine_before_any_consumers")
            < position("validate_and_import_through_store_owners")
    );
    assert!(
        position("reconcile_independently_trusted_inventory")
            < position("establish_chosen_identity_and_prove_fencing")
    );
    assert!(
        position("establish_chosen_identity_and_prove_fencing")
            < position("implement_explicit_audited_authority_acceptance")
    );
}
