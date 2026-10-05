use super::*;
struct Refused;
impl ProviderLogin for Refused {
    fn available(&self) -> bool {
        false
    }
    fn open(&self, _: Provider) -> Result<(), LoginFailure> {
        Err(LoginFailure::Unavailable)
    }
}
#[test]
fn only_bundled_conversation_windows_may_launch_provider_login() {
    assert_eq!(admit(panel::MAIN_WINDOW), Ok(()));
    assert_eq!(admit(desktop_window::DESKTOP_WINDOW), Ok(()));
    assert_eq!(
        admit(panel::SETUP_WINDOW),
        Err(LoginFailure::UntrustedCaller)
    );
    assert_eq!(admit("external"), Err(LoginFailure::UntrustedCaller));
}
#[test]
fn a_host_without_login_reports_unavailable() {
    assert!(!Refused.available());
    assert_eq!(
        tauri::async_runtime::block_on(launch(Arc::new(Refused), Provider::Claude)),
        Err(LoginFailure::Unavailable)
    );
}
