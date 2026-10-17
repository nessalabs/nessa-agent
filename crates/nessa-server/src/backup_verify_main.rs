use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    ExitCode::from(nessa_server::composition::run_backup_verifier(
        &arguments,
        &mut std::io::stdout(),
    ))
}
