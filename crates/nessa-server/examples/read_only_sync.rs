//! Retained read-only data demonstration; composition owns runtime effects.
fn main() -> std::process::ExitCode {
    nessa_server::composition::run_read_only_example(std::env::args().skip(1).collect())
}
