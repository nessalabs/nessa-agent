fn main() -> std::process::ExitCode {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    std::process::ExitCode::from(nessa_server::composition::run_backup_verifier(
        &arguments,
        &mut std::io::stdout(),
    ))
}
