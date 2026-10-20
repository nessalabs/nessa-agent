//! Print the weak-link scheduling report as one JSON document on stdout.
//!
//! ```text
//! cargo run -p nessa-client-core --example weak_link_measure
//! ```
//!
//! Diagnostics, if any, go to stderr. The report is the measurement in
//! `docs/design/weak-link-scheduling.md`.
fn main() {
    let report = nessa_client_core::retained::weak_link_report();
    println!("{}", serde_json::to_string_pretty(&report).expect("report"));
}
