use super::*;

#[test]
fn unavailable_launcher_never_offers_login() {
    assert!(!OtherProviderLogin.available());
    for provider in [Provider::Claude, Provider::Codex] {
        assert_eq!(
            OtherProviderLogin.open(provider),
            Err(LoginFailure::Unavailable)
        );
    }
}
