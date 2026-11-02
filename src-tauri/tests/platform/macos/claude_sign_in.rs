use super::*;

const TOKEN: &str = "sk-ant-oat01-Fixture_token-0123456789abcdef";

fn script(body: &str) -> Command {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", body]);
    command
}

#[test]
fn the_token_a_cli_shows_on_its_terminal_is_answered() {
    let shown = format!(
        "[ -t 0 ] && [ -t 1 ] || exit 3; [ \"$(stty size)\" = '50 400' ] || exit 4; \
         printf '\\033[2GYour\\033[7Gtoken:\\r\\n\\033[2G{TOKEN}\\033[120GStore it\\r\\n'"
    );
    assert_eq!(
        mint_with(&mut script(&shown), Duration::from_secs(10)).as_deref(),
        Ok(TOKEN)
    );
}

#[test]
fn a_cli_left_waiting_after_its_token_is_not_waited_for() {
    let started = Instant::now();
    let waits = format!("printf '{TOKEN}\\r\\nPress Enter\\r\\n'; sleep 30");
    assert_eq!(
        mint_with(&mut script(&waits), Duration::from_secs(20)).as_deref(),
        Ok(TOKEN)
    );
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn a_cli_that_ends_or_times_out_without_a_token_completes_nothing() {
    assert_eq!(
        mint_with(&mut script("echo cancelled; exit 1"), Duration::from_secs(10)),
        Err(LoginFailure::NotCompleted)
    );
    let started = Instant::now();
    assert_eq!(
        mint_with(&mut script("sleep 30"), Duration::from_millis(300)),
        Err(LoginFailure::NotCompleted)
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_missing_program_is_unavailable() {
    assert_eq!(
        mint_with(&mut Command::new("/nonexistent/claude"), Duration::from_secs(1)),
        Err(LoginFailure::Unavailable)
    );
}
