//! Print the weak-link scheduling report as one JSON document on stdout.
//!
//! ```text
//! cargo run -p nessa-client-core --example weak_link_measure
//! ```
//!
//! Diagnostics, if any, go to stderr. The report is the measurement in
//! `docs/design/weak-link-scheduling.md`.
fn main() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("measurement runtime");
    let report = runtime.block_on(nessa_client_core::retained::weak_link_report());
    println!("{}", serde_json::to_string_pretty(&report).expect("report"));
}
