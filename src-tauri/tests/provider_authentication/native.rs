use super::*;
#[test]
fn each_provider_opens_its_own_login_without_external_command_text() {
    assert!(NativeProviderLogin.available());
    assert!(script(Provider::Claude).contains("claude auth login"));
    assert!(script(Provider::Codex).contains("codex login"));
}
