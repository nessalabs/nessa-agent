//! Disposable macOS qualification of the production load diagnostic.

use std::process::ExitCode;

#[cfg(target_os = "macos")]
#[path = "../../src/app_sandbox/origin.rs"]
mod app_sandbox;
#[cfg(target_os = "macos")]
#[allow(dead_code)] // Other startup-log entry points belong to the shipped host.
#[path = "../../src/host_log.rs"]
mod host_log;
#[cfg(target_os = "macos")]
#[path = "../../src/host_refusal/copy.rs"]
mod host_refusal;
#[cfg(target_os = "macos")]
#[path = "../../src/links/policy.rs"]
mod navigation;
#[cfg(target_os = "macos")]
#[path = "../../src/page_load.rs"]
mod page_load;

#[cfg(target_os = "macos")]
mod native;
#[cfg(target_os = "macos")]
mod probe;
#[cfg(target_os = "macos")]
mod server;

fn main() -> ExitCode {
    #[cfg(target_os = "macos")]
    if let Err(error) = native::run() {
        eprintln!("page_load_acceptance: FAIL: {error}");
        return ExitCode::FAILURE;
    }
    #[cfg(not(target_os = "macos"))]
    println!("page_load_acceptance: requires macOS; native lifecycle not run");
    ExitCode::SUCCESS
}
