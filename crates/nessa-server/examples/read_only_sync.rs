//! Retained read-only data demonstration; composition owns runtime effects.
use std::process::ExitCode;
fn main() -> ExitCode {
    nessa_server::composition::run_read_only_example(std::env::args().skip(1).collect())
}
