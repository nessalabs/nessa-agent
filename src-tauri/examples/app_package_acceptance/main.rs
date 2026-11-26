//! Standalone Linux acceptance fixture for rendering an installed static package.

use std::process::ExitCode;

#[cfg(target_os = "linux")]
mod native;
#[cfg(target_os = "linux")]
mod packages;

fn main() -> ExitCode {
    #[cfg(target_os = "linux")]
    {
        let Some(directory) = std::env::args_os().nth(1) else {
            eprintln!("app_package_acceptance requires a disposable directory");
            return ExitCode::FAILURE;
        };
        if let Err(error) = native::run(std::path::Path::new(&directory)) {
            eprintln!("app_package_acceptance: {error}");
            return ExitCode::FAILURE;
        }
        ExitCode::SUCCESS
    }
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("app_package_acceptance requires Linux WebKitGTK");
        ExitCode::FAILURE
    }
}
