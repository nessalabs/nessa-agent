use super::*;
#[test]
fn each_provider_opens_its_own_login_without_external_command_text() {
    assert!(MacosProviderLogin.available());
    assert!(script(Provider::Claude).contains("claude auth login"));
    assert!(script(Provider::Codex).contains("codex login"));
}

#[test]
fn login_allows_a_delayed_consent_response() {
    let mut child = std::process::Command::new("/usr/bin/python3")
        .args(["-c", "import time; time.sleep(11)"])
        .spawn()
        .unwrap();
    assert_eq!(wait_for_login(&mut child, LOGIN_TIMEOUT), Ok(()));
}

#[test]
fn login_deadline_kills_and_reaps_a_stalled_launcher() {
    let mut child = std::process::Command::new("/usr/bin/python3")
        .args(["-c", "import time; time.sleep(60)"])
        .spawn()
        .unwrap();
    assert_eq!(
        wait_for_login(&mut child, Duration::from_millis(20)),
        Err(LoginFailure::LaunchFailed)
    );
    assert!(child.try_wait().unwrap().is_some());
}
