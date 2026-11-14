//! Publish the provider wire/storage bounds from their checked-in contract.
use std::{env, fs, path::PathBuf};
fn main() {
    let source = "../../protocol/defaults/github-relay-contract.json";
    println!("cargo:rerun-if-changed={source}");
    let values: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(source).expect("GitHub relay contract readable"))
            .expect("GitHub relay contract JSON");
    let fields = [
        ("pageSize", "PAGE_SIZE", "usize"),
        ("payloadBytes", "PAYLOAD_BYTES", "usize"),
        ("storedEvents", "STORED_EVENTS", "i64"),
        ("storedBytes", "STORED_BYTES", "i64"),
        ("retentionSeconds", "RETENTION_SECONDS", "i64"),
        ("ticketSeconds", "TICKET_SECONDS", "u64"),
        ("renewAfterSeconds", "RENEW_AFTER_SECONDS", "u64"),
        ("bindings", "BINDINGS", "i64"),
        ("auditRows", "AUDIT_ROWS", "i64"),
        ("sweepRows", "SWEEP_ROWS", "i64"),
    ];
    assert_eq!(
        values.as_object().expect("contract is an object").len(),
        fields.len(),
        "unrecognized contract field"
    );
    assert!(
        values["auditRows"].as_u64() >= values["storedEvents"].as_u64(),
        "one bulk expiry must fit its audit window"
    );
    assert!(
        values["renewAfterSeconds"].as_u64() < values["ticketSeconds"].as_u64(),
        "watch renews before ticket expiry"
    );
    let mut code = String::new();
    for (field, name, kind) in fields {
        let value = values[field]
            .as_u64()
            .filter(|v| *v > 0 && *v <= i32::MAX as u64)
            .expect("positive bounded contract field");
        code.push_str(&format!("pub const {name}: {kind} = {value};\n"));
    }
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").expect("Cargo OUT_DIR"))
            .join("github_relay_contract.rs"),
        code,
    )
    .expect("write GitHub relay contract");
}
