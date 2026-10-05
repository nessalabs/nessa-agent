use super::*;

#[test]
fn unavailable_launcher_never_offers_login() {
    assert!(!LinuxProviderLogin.available());
    for provider in [Provider::Claude, Provider::Codex] {
        assert_eq!(
            LinuxProviderLogin.open(provider),
            Err(LoginFailure::Unavailable)
        );
    }
}
