//! Native storage for the panel's surface credential, which the panel, setup and
//! the desktop window read. Renderer input never selects a file.
use crate::composition::HostDependencies;
use crate::gateway::{
    application::Gateway,
    infrastructure::{desktop_startup_wait, GatewayReader},
};
use crate::host_refusal::SurfaceCommandError;
use std::{io::Read, path::PathBuf, sync::Arc, time::Duration};
use tauri::State;

/// Why there is no token to hand over.
///
/// A variant rather than a sentence, because the repairs differ and something
/// has to be able to tell them apart. The sentence is still here — it is what
/// [`Display`] writes, and the command hands that to the webview — but the fact
/// is the variant, so a test asserts which refusal happened instead of matching
/// a fragment of prose that any rewording breaks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialRefusal {
    /// Never provisioned. Starting the local server creates one.
    NotProvisioned,
    /// There, and the operating system would not open it.
    Refused,
    /// Asked for a stage this credential was not built for.
    ///
    /// `bundle` is the stage this window was started on. `requested` is the
    /// stage the page asked the local server for.
    WrongStage { bundle: String, requested: String },
    /// The environment named a namespace that cannot hold a credential.
    UnusableNamespace,
    /// Opened, and what came out is not a token.
    NotAToken(&'static str),
    /// Anything else the operating system said, in its own words.
    Unopenable(String),
}

impl std::fmt::Display for CredentialRefusal {
    /// The sentence somebody reads, which names the repair where there is one.
    ///
    /// A file that is not there has never been provisioned, and starting the
    /// local server provisions it. A file that is there and was refused is a
    /// permissions problem the server will not touch, because provisioning
    /// never replaces an existing credential. The old wording covered both with
    /// "missing or unsafe" and named no command, so neither case told anybody
    /// what to do.
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotProvisioned => {
                out.write_str(&crate::host_refusal::sentence("not-provisioned"))
            }
            Self::Refused => out.write_str(
                "The chat credential was refused: it, or a directory above it, must be \
                 yours alone (mode 0600/0700 on Unix, a private DACL on Windows). Repair \
                 those permissions, or remove the credential file and start the local \
                 server to provision a new one.",
            ),
            Self::WrongStage { bundle, requested } => out.write_str(&crate::host_refusal::fill(
                "wrong-stage",
                &[("bundle", bundle), ("requested", requested)],
            )),
            Self::UnusableNamespace => out.write_str("Invalid native credential namespace"),
            Self::NotAToken(why) => write!(out, "Chat credential is {why}"),
            Self::Unopenable(error) => {
                write!(out, "The chat credential could not be opened: {error}.")
            }
        }
    }
}

/// Where the bundled surface's token comes from.
///
/// The host's own port: a stage in, a token or a reason out, and no path or
/// file handle visible to a caller. The real implementation reads the
/// stage-scoped credential file; a test substitutes one that answers at once,
/// including with the refusals a real keyring failure is hardest to arrange.
pub trait SurfaceCredentials: Send + Sync {
    /// The token for `stage`, or why there is not one to hand over.
    fn read(&self, stage: &str) -> Result<String, CredentialRefusal>;
}

pub struct SurfaceCredential {
    root: Option<PathBuf>,
    relative: PathBuf,
    stage: String,
}

impl SurfaceCredential {
    /// Build the credential reader from composition's one resolved service namespace.
    pub fn for_namespace(namespace: Option<ServiceNamespace>, stage: String) -> Self {
        Self {
            root: namespace.as_ref().map(|value| value.root.clone()),
            relative: namespace
                .map(|value| value.relative.join("auth/surfaces/nessa-panel.token"))
                .unwrap_or_default(),
            stage,
        }
    }
}

/// A validated relative service namespace anchored at one trusted data root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ServiceNamespace {
    pub(crate) root: PathBuf,
    pub(crate) relative: PathBuf,
}

impl ServiceNamespace {
    /// `logs` under this namespace. The gateway advertisement and `gateway.log`
    /// live here, and the desktop startup log is the sibling file `host.log`.
    pub(crate) fn logs_relative(&self) -> PathBuf {
        self.relative.join("logs")
    }

    /// Absolute form of [`Self::logs_relative`].
    pub(crate) fn log_directory(&self) -> PathBuf {
        self.root.join(self.logs_relative())
    }

    /// The gateway's `config.json`, at the top of this namespace: where its
    /// stored MCP servers are (the gateway's `runtime_config::config_path`).
    pub(crate) fn config_file(&self) -> PathBuf {
        self.root.join(&self.relative).join("config.json")
    }
}

/// Whether a name may be one path segment of a namespace.
///
/// Conservative on purpose: these come from the environment and are joined into
/// a path, so anything that could climb out of the namespace or name something
/// else entirely is refused rather than escaped.
fn segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// Where a credential lives, given what the environment said.
///
/// Pure, and separate from reading the environment, because this is the part
/// with a rule in it: `prod` is the namespace root itself while every other
/// stage is a directory under it, an instance nests under `instances/`, and a
/// base that is relative or a stage that is not a single safe segment yields no
/// root at all — which is what makes the credential unreadable rather than read
/// from somewhere unintended. The standard asks for exactly this split: the
/// environment read is exempt from the seam rule, the interpretation of what it
/// said is not.
///
/// Mirrors `crates/nessa-server/src/env/paths.rs`, which is what actually
/// writes the file.
pub(crate) fn service_namespace(
    base: Option<PathBuf>,
    stage: &str,
    instance: Option<&str>,
) -> Option<ServiceNamespace> {
    let root =
        base.filter(|base| base.is_absolute() && segment(stage) && instance.is_none_or(segment))?;
    let mut relative = PathBuf::new();
    if stage != "prod" {
        relative.push(stage);
    }
    if let Some(instance) = instance {
        relative.push("instances");
        relative.push(instance);
    }
    Some(ServiceNamespace { root, relative })
}

/// Say which of the two things went wrong, because the repairs differ.
///
/// A file that is not there has never been provisioned, and starting the local
/// server provisions it. A file that is there and was refused is a permissions
/// problem the server will not touch, because provisioning never replaces an
/// existing credential. The old wording covered both with "missing or unsafe"
/// and named no command, so neither case told anybody what to do. Anything else
/// keeps the operating system's own words rather than a guessed cause.
/// Which refusal an operating-system error is.
fn unreadable(error: &std::io::Error) -> CredentialRefusal {
    if nessa_local_storage::is_unsafe_file(error) {
        return CredentialRefusal::Refused;
    }
    match error.kind() {
        std::io::ErrorKind::NotFound => CredentialRefusal::NotProvisioned,
        std::io::ErrorKind::PermissionDenied => CredentialRefusal::Refused,
        _ => CredentialRefusal::Unopenable(error.to_string()),
    }
}

impl SurfaceCredentials for SurfaceCredential {
    fn read(&self, stage: &str) -> Result<String, CredentialRefusal> {
        if stage != self.stage {
            return Err(CredentialRefusal::WrongStage {
                bundle: self.stage.clone(),
                requested: stage.to_owned(),
            });
        }
        let root = self
            .root
            .as_ref()
            .ok_or(CredentialRefusal::UnusableNamespace)?;
        let mut file = nessa_local_storage::open_beneath(
            root,
            &self.relative,
            nessa_local_storage::OpenMode::ReadNonblocking,
        )
        .map_err(|error| unreadable(&error))?;
        if file
            .metadata()
            .map_err(|_| CredentialRefusal::NotAToken("not inspectable"))?
            .len()
            > 16385
        {
            return Err(CredentialRefusal::NotAToken("too large"));
        }
        let mut token = String::new();
        (&mut file)
            .take(16385)
            .read_to_string(&mut token)
            .map_err(|_| CredentialRefusal::NotAToken("not readable"))?;
        let token = token.trim().to_owned();
        if token.is_empty() || token.len() > 16384 {
            return Err(CredentialRefusal::NotAToken("invalid"));
        }
        Ok(token)
    }
}

/// The order a credential load goes in, with its two outside things supplied.
///
/// Split from [`load_surface_credential`] so the rules survive without a window
/// server: only the panel, setup and the desktop window may ask
/// ([`GatewayReader`]); in a packaged build the panel and setup wait for the
/// gateway to reconcile before anything is handed over, and the desktop window
/// is served only once it has, without reconciling (`GatewayReader::ready`,
/// held by the desktop-window tests below); a build without a gateway
/// does not wait at all; and the refusal for the wrong window happens before
/// any of those, so a stray webview cannot make the app register a service.
fn credential_error(refusal: CredentialRefusal) -> SurfaceCommandError {
    match refusal {
        CredentialRefusal::NotProvisioned => SurfaceCommandError::not_provisioned(),
        CredentialRefusal::WrongStage { bundle, requested } => {
            SurfaceCommandError::wrong_stage(bundle, requested)
        }
        other => SurfaceCommandError::message(other.to_string()),
    }
}

async fn load_for(
    label: &str,
    gateway: Option<&Gateway>,
    endpoint: Arc<crate::gateway_endpoint::application::GatewayEndpointAccess>,
    credential: &dyn SurfaceCredentials,
    stage: &str,
    url: &str,
    startup_wait: Duration,
) -> Result<String, SurfaceCommandError> {
    let reader = GatewayReader::of_window(label).map_err(SurfaceCommandError::message)?;
    if let Some(gateway) = gateway {
        reader
            .ready(gateway, startup_wait)
            .await
            .map_err(SurfaceCommandError::from)?;
    }
    let stage_for_endpoint = stage.to_owned();
    let requested_url = url.to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        endpoint.permits_credential_for(&stage_for_endpoint, &requested_url)
    })
    .await
    .map_err(|error| SurfaceCommandError::message(error.to_string()))?
    .map_err(SurfaceCommandError::from)?;
    credential.read(stage).map_err(credential_error)
}

#[tauri::command]
pub async fn load_surface_credential(
    window: tauri::WebviewWindow,
    deps: State<'_, HostDependencies>,
    stage: String,
    url: String,
) -> Result<String, SurfaceCommandError> {
    let gateway = deps.gateway.clone();
    let endpoint = deps.endpoint.clone();
    let credential = deps.credential.clone();
    load_for(
        window.label(),
        gateway.as_deref(),
        endpoint,
        &*credential,
        &stage,
        &url,
        desktop_startup_wait(),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop_window::DESKTOP_WINDOW;
    use crate::gateway::application::{
        testing::{
            reconciled_registration, recording_gateway, recording_gateway_held,
            recording_gateway_held_on, recording_gateway_on, RecordingGatewayHost,
        },
        GatewayError, MonotonicClock, ReconciledGateway,
    };
    use crate::gateway::domain::value_objects::{BundledSurface, ReconciliationInitiator};
    use crate::panel;
    use nessa_gateway_endpoint::{
        application::EndpointDiscovery,
        domain::{EndpointIdentity, GatewayEndpoint},
    };
    use std::{
        fs,
        io::Write,
        sync::{mpsc::channel, Arc, Mutex, OnceLock},
        thread,
        time::Duration,
    };

    struct FixedEndpoint(Option<GatewayEndpoint>);

    impl EndpointDiscovery for FixedEndpoint {
        fn discover(&self) -> std::io::Result<Option<GatewayEndpoint>> {
            Ok(self.0.clone())
        }
    }

    fn endpoint_access(
        endpoint: Option<GatewayEndpoint>,
    ) -> crate::gateway_endpoint::application::GatewayEndpointAccess {
        crate::gateway_endpoint::application::GatewayEndpointAccess::new(
            "ci".into(),
            Arc::new(FixedEndpoint(endpoint)),
        )
    }

    /// A credential source that has already made up its mind, and writes down
    /// whether it was asked at all.
    struct FakeCredentials {
        outcome: Result<String, CredentialRefusal>,
        reads: Mutex<u32>,
    }

    impl FakeCredentials {
        fn holding(token: &str) -> Self {
            Self {
                outcome: Ok(token.to_string()),
                reads: Mutex::new(0),
            }
        }

        fn refusing(refusal: CredentialRefusal) -> Self {
            Self {
                outcome: Err(refusal),
                reads: Mutex::new(0),
            }
        }

        fn reads(&self) -> u32 {
            *self.reads.lock().unwrap()
        }
    }

    impl SurfaceCredentials for FakeCredentials {
        fn read(&self, _stage: &str) -> Result<String, CredentialRefusal> {
            *self.reads.lock().unwrap() += 1;
            self.outcome.clone()
        }
    }

    /// A read that does not wait out a startup still running.
    fn load(
        label: &str,
        gateway: Option<&Gateway>,
        credential: &dyn SurfaceCredentials,
    ) -> Result<String, SurfaceCommandError> {
        load_waiting(label, gateway, credential, Duration::ZERO)
    }

    fn load_waiting(
        label: &str,
        gateway: Option<&Gateway>,
        credential: &dyn SurfaceCredentials,
        startup_wait: Duration,
    ) -> Result<String, SurfaceCommandError> {
        tauri::async_runtime::block_on(load_for(
            label,
            gateway,
            Arc::new(endpoint_access(None)),
            credential,
            "ci",
            "ws://127.0.0.1:7420",
            startup_wait,
        ))
    }

    /// The whole of the ordering: the window is checked first, then the gateway
    /// is waited for, and only then is anything read.
    #[test]
    fn the_bundled_panel_waits_for_the_gateway_and_gets_its_token() {
        let credential = FakeCredentials::holding("fixture-only");
        let (gateway, host) = recording_gateway(Ok(reconciled_registration()));

        assert_eq!(
            load(panel::MAIN_WINDOW, Some(&gateway), &credential).unwrap(),
            "fixture-only"
        );
        assert_eq!(*host.registrations.lock().unwrap(), 1);
        assert_eq!(credential.reads(), 1);
    }

    #[test]
    fn the_bundled_setup_waits_for_the_gateway_and_gets_its_token() {
        let credential = FakeCredentials::holding("fixture-only");
        let (gateway, host) = recording_gateway(Ok(reconciled_registration()));
        assert_eq!(
            load(panel::SETUP_WINDOW, Some(&gateway), &credential).unwrap(),
            "fixture-only"
        );
        assert_eq!(*host.registrations.lock().unwrap(), 1);
        assert_eq!(credential.reads(), 1);
        assert_eq!(
            *host.initiators.lock().unwrap(),
            vec![ReconciliationInitiator::BundledSurface(
                BundledSurface::Setup
            )]
        );
    }

    /// The desktop window reads a gateway that startup brought up, under the
    /// panel's credential: once it is ready, the token is read and nothing is
    /// registered or reconciled on the window's behalf (H1′, #419).
    #[test]
    fn the_desktop_window_reads_a_ready_gateway_without_reconciling() {
        let credential = FakeCredentials::holding("fixture-only");
        let (gateway, host) = recording_gateway(Ok(reconciled_registration()));
        load(panel::MAIN_WINDOW, Some(&gateway), &credential).unwrap();
        let registered = *host.registrations.lock().unwrap();
        let initiators = host.initiators.lock().unwrap().len();

        assert_eq!(
            load(DESKTOP_WINDOW, Some(&gateway), &credential).unwrap(),
            "fixture-only"
        );
        assert_eq!(*host.registrations.lock().unwrap(), registered);
        assert_eq!(host.initiators.lock().unwrap().len(), initiators);
        assert_eq!(credential.reads(), 2);
    }

    /// Before the gateway is ready — never started, or failed — a desktop
    /// window read that does not wait is refused, and its asking starts
    /// nothing: no registration, no reconciliation, no token read (H2′).
    #[test]
    fn a_desktop_window_read_that_does_not_wait_is_refused_until_the_gateway_is_ready_and_starts_nothing(
    ) {
        let credential = FakeCredentials::holding("fixture-only");
        let (gateway, host) = recording_gateway(Ok(reconciled_registration()));
        for _ in 0..3 {
            assert_eq!(
                load(DESKTOP_WINDOW, Some(&gateway), &credential).err(),
                Some(SurfaceCommandError::not_ready())
            );
        }
        assert_eq!(*host.registrations.lock().unwrap(), 0);
        assert!(host.initiators.lock().unwrap().is_empty());
        assert_eq!(credential.reads(), 0);

        let (failed, host) =
            recording_gateway(Err(GatewayError::Registration("not installed".into())));
        let _ = load(panel::MAIN_WINDOW, Some(&failed), &credential);
        let registered = *host.registrations.lock().unwrap();
        assert_eq!(
            load(DESKTOP_WINDOW, Some(&failed), &credential).err(),
            Some(SurfaceCommandError::not_ready())
        );
        assert_eq!(*host.registrations.lock().unwrap(), registered);
        assert_eq!(credential.reads(), 0);
    }

    /// The host's own launch startup is in flight: a desktop window that does
    /// not wait is refused, starts no second reconciliation and reads nothing; once that
    /// startup is ready the window is served, the panel never having asked
    /// (H2″, H9).
    #[test]
    fn the_desktop_window_waits_out_the_hosts_own_startup_without_joining_it() {
        let credential = FakeCredentials::holding("fixture-only");
        let (gateway, host, entered, release) =
            recording_gateway_held(Ok(reconciled_registration()));
        // What the window was told while startup ran. The window's asks run on
        // a thread of their own with a deadline, and the registration is
        // released whatever they did, so a window that joined the
        // reconciliation fails here rather than waiting on it forever.
        let (while_starting, registered, read) = thread::scope(|scope| {
            let starting = scope.spawn(|| tauri::async_runtime::block_on(gateway.start()));
            entered.recv().unwrap();
            let (told_tx, told_rx) = channel();
            let (window_gateway, window_credential) = (&gateway, &credential);
            let asking = scope.spawn(move || {
                let told: Vec<_> = (0..3)
                    .map(|_| load(DESKTOP_WINDOW, Some(window_gateway), window_credential))
                    .collect();
                told_tx.send(told).unwrap();
            });
            let told = told_rx.recv_timeout(Duration::from_secs(5));
            let registered = *host.registrations.lock().unwrap();
            let read = credential.reads();
            release.send(()).unwrap();
            asking.join().unwrap();
            starting.join().unwrap().unwrap();
            (told, registered, read)
        });
        let while_starting =
            while_starting.expect("the window's asks waited on the host's startup");
        for told in while_starting {
            assert_eq!(told.err(), Some(SurfaceCommandError::not_ready()));
        }
        assert_eq!(registered, 1);
        assert_eq!(read, 0);
        assert_eq!(
            load(DESKTOP_WINDOW, Some(&gateway), &credential).unwrap(),
            "fixture-only"
        );
        assert_eq!(*host.registrations.lock().unwrap(), 1);
        assert_eq!(
            *host.initiators.lock().unwrap(),
            vec![ReconciliationInitiator::DesktopHost]
        );
    }

    type Told = Result<String, SurfaceCommandError>;

    /// What a desktop window waiting up to 10 s was told about a host startup
    /// held until the window had asked: `None` while it was held, then its
    /// answer once the startup ended as `registration` says.
    fn told_across_a_held_startup(
        registration: Result<ReconciledGateway, GatewayError>,
    ) -> (Option<Told>, Told, Arc<RecordingGatewayHost>) {
        let credential = FakeCredentials::holding("fixture-only");
        let (clock, paused) = PauseHeard::new();
        let (gateway, host, entered, release) =
            recording_gateway_held_on(registration, Arc::new(clock));
        let (before_ended, told) = thread::scope(|scope| {
            let starting = scope.spawn(|| tauri::async_runtime::block_on(gateway.start()));
            entered.recv().unwrap();
            let (told_tx, told_rx) = channel();
            let (window_gateway, window_credential) = (&gateway, &credential);
            let asking = scope.spawn(move || {
                told_tx
                    .send(load_waiting(
                        DESKTOP_WINDOW,
                        Some(window_gateway),
                        window_credential,
                        Duration::from_secs(10),
                    ))
                    .unwrap();
            });
            // The read pausing on the gateway's clock is the read holding:
            // it saw startup still running and did not answer. Startup is
            // released either way, so a read that never held fails below
            // instead of hanging the test.
            let held = paused.recv_timeout(Duration::from_secs(5)).is_ok();
            let before_ended = told_rx.try_recv().ok();
            release.send(()).unwrap();
            let _ = starting.join().unwrap();
            assert!(held, "the window's read never held for the startup");
            let told = told_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("the waiting window was not answered once startup ended");
            asking.join().unwrap();
            (before_ended, told)
        });
        (before_ended, told, host)
    }

    /// The system clock, telling the test each time a reader pauses on it.
    struct PauseHeard(Mutex<std::sync::mpsc::Sender<()>>);

    impl PauseHeard {
        fn new() -> (Self, std::sync::mpsc::Receiver<()>) {
            let (heard, paused) = channel();
            (Self(Mutex::new(heard)), paused)
        }
    }

    impl MonotonicClock for PauseHeard {
        fn now(&self) -> std::time::Instant {
            std::time::Instant::now()
        }

        fn pause(
            &self,
            duration: Duration,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
            let _ = self.0.lock().unwrap().send(());
            Box::pin(tokio::time::sleep(duration))
        }
    }

    /// A window opened at launch waits out the host's own startup, joining
    /// nothing, and is served the moment it is ready: it never sees the
    /// startup refusal a launch used to show for a few seconds.
    #[test]
    fn a_waiting_desktop_window_is_served_once_the_hosts_own_startup_is_ready() {
        let (before_ready, told, host) = told_across_a_held_startup(Ok(reconciled_registration()));
        assert_eq!(
            before_ready, None,
            "the window was answered before startup ended"
        );
        assert_eq!(told, Ok("fixture-only".into()));
        assert_eq!(*host.registrations.lock().unwrap(), 1);
        assert_eq!(
            *host.initiators.lock().unwrap(),
            vec![ReconciliationInitiator::DesktopHost]
        );
    }

    /// A startup that fails while the window waits is refused when it fails,
    /// not when the wait ends.
    #[test]
    fn a_waiting_desktop_window_is_refused_once_the_startup_it_waits_on_fails() {
        let (before_failed, told, host) =
            told_across_a_held_startup(Err(GatewayError::Registration("not installed".into())));
        assert_eq!(
            before_failed, None,
            "the window was answered before startup ended"
        );
        assert_eq!(told, Err(SurfaceCommandError::not_ready()));
        assert_eq!(*host.registrations.lock().unwrap(), 1);
    }

    /// A startup that failed is refused at once, however long the window
    /// would wait: only a bundled surface starts it again. One that never
    /// ends is refused when the wait does.
    #[test]
    fn a_waiting_desktop_window_is_refused_a_failed_startup_at_once_and_a_running_one_at_its_deadline(
    ) {
        let credential = FakeCredentials::holding("fixture-only");
        let (failed, _) =
            recording_gateway(Err(GatewayError::Registration("not installed".into())));
        let _ = load(panel::MAIN_WINDOW, Some(&failed), &credential);
        let asked = std::time::Instant::now();
        assert_eq!(
            load_waiting(
                DESKTOP_WINDOW,
                Some(&failed),
                &credential,
                Duration::from_secs(30)
            )
            .err(),
            Some(SurfaceCommandError::not_ready())
        );
        assert!(asked.elapsed() < Duration::from_secs(5));

        // A host startup that failed before any reconciliation ran is a
        // failed startup too, not one still starting.
        let (unclassified, host) = recording_gateway(Ok(reconciled_registration()));
        host.classify_panics
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(tauri::async_runtime::block_on(unclassified.start()).is_err());
        let asked = std::time::Instant::now();
        assert_eq!(
            load_waiting(
                DESKTOP_WINDOW,
                Some(&unclassified),
                &credential,
                Duration::from_secs(30)
            )
            .err(),
            Some(SurfaceCommandError::not_ready())
        );
        assert!(asked.elapsed() < Duration::from_secs(5));

        // Never started: still starting when the whole wait has passed, on
        // the gateway's clock, which here moves only as the read pauses.
        let clock = Arc::new(VirtualClock::default());
        let (starting, host) = recording_gateway_on(Ok(reconciled_registration()), clock.clone());
        assert_eq!(
            load_waiting(
                DESKTOP_WINDOW,
                Some(&starting),
                &credential,
                Duration::from_secs(15)
            )
            .err(),
            Some(SurfaceCommandError::not_ready())
        );
        assert_eq!(clock.paused(), Duration::from_secs(15));
        assert_eq!(*host.registrations.lock().unwrap(), 0);
        assert_eq!(credential.reads(), 0);
    }

    /// A clock that stands still until a reader pauses on it, then moves on
    /// by the pause at once.
    #[derive(Default)]
    struct VirtualClock {
        start: OnceLock<std::time::Instant>,
        paused: Mutex<Duration>,
    }

    impl VirtualClock {
        fn paused(&self) -> Duration {
            *self.paused.lock().unwrap()
        }
    }

    impl MonotonicClock for VirtualClock {
        fn now(&self) -> std::time::Instant {
            *self.start.get_or_init(std::time::Instant::now) + self.paused()
        }

        fn pause(
            &self,
            duration: Duration,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
            *self.paused.lock().unwrap() += duration;
            Box::pin(std::future::ready(()))
        }
    }

    /// The host's own start failing after a surface's reconciliation already
    /// brought the gateway up leaves it up: that reconciliation owns the phase.
    #[test]
    fn a_host_start_that_fails_after_a_surface_brought_the_gateway_up_leaves_it_ready() {
        let credential = FakeCredentials::holding("fixture-only");
        let (gateway, host) = recording_gateway(Ok(reconciled_registration()));
        load(panel::MAIN_WINDOW, Some(&gateway), &credential).unwrap();
        host.classify_panics
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(tauri::async_runtime::block_on(gateway.start()).is_err());
        assert_eq!(
            load(DESKTOP_WINDOW, Some(&gateway), &credential).unwrap(),
            "fixture-only"
        );
    }

    /// A build without a managed gateway does not wait for one, for the
    /// desktop window either (H6).
    #[test]
    fn the_desktop_window_in_a_build_without_a_gateway_does_not_wait() {
        let credential = FakeCredentials::holding("fixture-only");
        assert_eq!(
            load(DESKTOP_WINDOW, None, &credential).unwrap(),
            "fixture-only"
        );
    }

    #[test]
    fn setup_keeps_the_stage_and_verified_destination_guards() {
        let credential = FakeCredentials::holding("fixture-only");
        let endpoint = GatewayEndpoint::new(
            "ws://127.0.0.1:9137".into(),
            EndpointIdentity::new("5485b918-1eeb-4a4a-ad1d-9fdc70dfa231".into(), 4711).unwrap(),
        )
        .unwrap();
        for (stage, url) in [
            ("dev", "ws://127.0.0.1:9137"),
            ("ci", "ws://127.0.0.1:7420"),
        ] {
            let result = tauri::async_runtime::block_on(load_for(
                panel::SETUP_WINDOW,
                None,
                Arc::new(endpoint_access(Some(endpoint.clone()))),
                &credential,
                stage,
                url,
                Duration::ZERO,
            ));
            let expected = if stage == "dev" {
                SurfaceCommandError::wrong_stage("ci", "dev")
            } else {
                SurfaceCommandError::message(
                    "The credential request does not match the verified gateway endpoint",
                )
            };
            assert_eq!(result.err(), Some(expected));
            assert_eq!(credential.reads(), 0);
        }
    }

    /// The refusal that matters most: any other window is turned away before
    /// the gateway is touched, so a stray webview cannot make a packaged build
    /// register a background service, let alone read a token.
    #[test]
    fn another_window_is_refused_before_the_gateway_is_asked_for_anything() {
        let credential = FakeCredentials::holding("fixture-only");
        let (gateway, host) = recording_gateway(Ok(reconciled_registration()));

        assert_eq!(
            load("untrusted", Some(&gateway), &credential).err(),
            Some(SurfaceCommandError::message(
                "Only Nessa's own windows can read the gateway",
            ))
        );
        assert_eq!(*host.registrations.lock().unwrap(), 0);
        assert_eq!(credential.reads(), 0);
    }

    /// A gateway that will not reconcile is reported as itself, and nothing is
    /// read: handing a token to a surface with no gateway behind it would only
    /// move the failure somewhere less legible.
    #[test]
    fn a_gateway_that_will_not_reconcile_stops_the_load() {
        let credential = FakeCredentials::holding("fixture-only");
        let (gateway, _host) =
            recording_gateway(Err(GatewayError::Registration("not installed".into())));

        assert_eq!(
            load(panel::MAIN_WINDOW, Some(&gateway), &credential).err(),
            Some(SurfaceCommandError::message("not installed"))
        );
        assert_eq!(credential.reads(), 0);
    }

    #[test]
    fn a_mismatched_verified_destination_is_refused_before_the_token_is_read() {
        let credential = FakeCredentials::holding("fixture-only");
        let endpoint = GatewayEndpoint::new(
            "ws://127.0.0.1:9137".into(),
            EndpointIdentity::new("5485b918-1eeb-4a4a-ad1d-9fdc70dfa231".into(), 4711).unwrap(),
        )
        .unwrap();
        let result = tauri::async_runtime::block_on(load_for(
            panel::MAIN_WINDOW,
            None,
            Arc::new(endpoint_access(Some(endpoint))),
            &credential,
            "ci",
            "ws://127.0.0.1:7420",
            Duration::ZERO,
        ));
        assert!(result.is_err());
        assert_eq!(credential.reads(), 0);
    }

    /// A dev build has no gateway to wait for, and the credential is still the
    /// panel's to load.
    #[test]
    fn a_build_without_a_gateway_does_not_wait_for_one() {
        let credential = FakeCredentials::holding("fixture-only");

        assert_eq!(
            load(panel::MAIN_WINDOW, None, &credential).unwrap(),
            "fixture-only"
        );
    }

    /// The credential's own refusal reaches the surface unchanged: it is the
    /// only thing that knows whether the file is absent or was refused, and so
    /// the only thing that can name the next step.
    #[test]
    fn a_missing_credential_is_reported_as_the_source_put_it() {
        let credential = FakeCredentials::refusing(CredentialRefusal::NotProvisioned);

        assert_eq!(
            load(panel::MAIN_WINDOW, None, &credential).err(),
            Some(SurfaceCommandError::not_provisioned())
        );
    }

    /// Absent and refused are different facts with different repairs, and an
    /// unexpected failure is neither. Asserted as variants: the sentences are
    /// what somebody reads and are free to be reworded, and a test that matched
    /// fragments of them would fail on a rewording and pass on the two being
    /// confused, which is the wrong way round.
    #[test]
    fn absence_and_refusal_are_told_apart() {
        use std::io::ErrorKind;

        assert_eq!(
            unreadable(&std::io::Error::from(ErrorKind::NotFound)),
            CredentialRefusal::NotProvisioned
        );
        assert_eq!(
            unreadable(&std::io::Error::from(ErrorKind::PermissionDenied)),
            CredentialRefusal::Refused
        );
        assert!(matches!(
            unreadable(&std::io::Error::from(ErrorKind::TimedOut)),
            CredentialRefusal::Unopenable(_)
        ));
    }

    /// And each still says what to do about itself, because a variant nobody
    /// can read is no better than a sentence nobody can branch on.
    #[test]
    fn each_refusal_names_its_own_repair() {
        let absent = CredentialRefusal::NotProvisioned.to_string();
        let refused = CredentialRefusal::Refused.to_string();

        assert!(absent.contains("without the local server"), "{absent}");
        assert!(absent.contains("just start"), "{absent}");
        let mismatch = CredentialRefusal::WrongStage {
            bundle: "dev".into(),
            requested: "prod".into(),
        }
        .to_string();
        assert!(
            mismatch.contains("dev") && mismatch.contains("prod"),
            "{mismatch}"
        );
        assert!(refused.contains("refused"), "{refused}");
        assert!(!refused.contains("provisioned yet"), "{refused}");
        assert!(CredentialRefusal::Unopenable("disk fell off".into())
            .to_string()
            .contains("could not be opened"));
    }

    /// The path rule, which the environment read hands its answers to.
    ///
    /// `prod` is the namespace root itself and every other stage is a directory
    /// under it — that is what puts a dev credential beside a packaged one
    /// rather than on top of it.
    /// An absolute path on the platform running the test.
    ///
    /// `/data` is absolute on Unix and is not on Windows, where a path needs a
    /// drive — and `service_namespace` is right to refuse a base that is not
    /// absolute, so a test that hardcoded `/data` asserted the refusal there
    /// rather than the nesting it meant to.
    fn absolute(path: &str) -> PathBuf {
        match cfg!(windows) {
            true => PathBuf::from(format!("C:\\{path}")),
            false => PathBuf::from(format!("/{path}")),
        }
    }

    #[test]
    fn a_stage_that_is_not_prod_nests_and_prod_does_not() {
        let base = || Some(absolute("data"));

        let root = service_namespace(base(), "prod", None);
        assert_eq!(
            root,
            Some(ServiceNamespace {
                root: absolute("data"),
                relative: PathBuf::new()
            })
        );

        let root = service_namespace(base(), "dev", None);
        assert_eq!(
            root,
            Some(ServiceNamespace {
                root: absolute("data"),
                relative: PathBuf::from("dev")
            })
        );

        let root = service_namespace(base(), "dev", Some("wt"));
        assert_eq!(
            root,
            Some(ServiceNamespace {
                root: absolute("data"),
                relative: PathBuf::from("dev/instances/wt")
            })
        );

        let prod = service_namespace(base(), "prod", None).expect("prod namespace");
        assert_eq!(prod.log_directory(), absolute("data").join("logs"));
        let staged = service_namespace(base(), "dev", Some("wt")).expect("dev namespace");
        assert_eq!(
            staged.log_directory(),
            absolute("data")
                .join("dev")
                .join("instances")
                .join("wt")
                .join("logs")
        );
    }

    /// A namespace that cannot be trusted yields no root, and a credential with
    /// no root is unreadable rather than read from somewhere unintended. These
    /// values come from the environment and are joined into a path.
    #[test]
    fn a_namespace_that_could_climb_out_is_refused_outright() {
        for (base, stage, instance) in [
            (Some(PathBuf::from("relative/path")), "dev", None),
            (Some(absolute("data")), "..", None),
            (Some(absolute("data")), "", None),
            (Some(absolute("data")), "one/two", None),
            (Some(absolute("data")), "dev", Some("..")),
            (Some(absolute("data")), "dev", Some("one two")),
            (None, "dev", None),
        ] {
            let root = service_namespace(base, stage, instance);
            assert_eq!(root, None, "stage {stage:?} instance {instance:?}");
        }
    }

    fn temporary_directory(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "nessa-native-{name}-{}-{unique}",
            std::process::id()
        ));
        nessa_local_storage::create_directory(&root).unwrap();
        root
    }

    #[test]
    fn native_storage_checks_stage_and_private_file_permissions() {
        let root = temporary_directory("regular");
        let path = root.join("nessa-panel.token");
        let mut file =
            nessa_local_storage::open(&path, nessa_local_storage::OpenMode::CreateNew).unwrap();
        file.write_all(b"fixture-only\n").unwrap();
        let storage = SurfaceCredential {
            root: Some(root.clone()),
            relative: "nessa-panel.token".into(),
            stage: "ci".into(),
        };
        assert_eq!(storage.read("ci").unwrap(), "fixture-only");
        assert_eq!(
            storage.read("prod").unwrap_err(),
            CredentialRefusal::WrongStage {
                bundle: "ci".into(),
                requested: "prod".into(),
            }
        );
        let absent = SurfaceCredential {
            root: Some(root.clone()),
            relative: "never-provisioned.token".into(),
            stage: "ci".into(),
        };
        assert_eq!(
            absent.read("ci").unwrap_err(),
            CredentialRefusal::NotProvisioned,
            "an absent credential is absent, not refused"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(
                storage.read("ci").unwrap_err(),
                CredentialRefusal::Refused,
                "a readable-by-others credential is refused, not reported as absent"
            );
        }
        drop(file);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn native_storage_rejects_a_fifo_without_waiting_for_a_writer() {
        let root = temporary_directory("fifo");
        let path = root.join("nessa-panel.token");
        assert!(std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap()
            .success());
        let storage = SurfaceCredential {
            root: Some(root.clone()),
            relative: "nessa-panel.token".into(),
            stage: "ci".into(),
        };
        assert!(storage.read("ci").is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn native_storage_rejects_an_intermediate_symlink() {
        use std::os::unix::fs::symlink;

        let root = temporary_directory("intermediate-link");
        let outside = temporary_directory("outside");
        let mut token = nessa_local_storage::open(
            &outside.join("nessa-panel.token"),
            nessa_local_storage::OpenMode::CreateNew,
        )
        .unwrap();
        token.write_all(b"redirected-token").unwrap();
        drop(token);
        symlink(&outside, root.join("auth")).unwrap();
        let storage = SurfaceCredential {
            root: Some(root.clone()),
            relative: "auth/nessa-panel.token".into(),
            stage: "ci".into(),
        };
        assert!(storage.read("ci").is_err());
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn resolved_namespace_cannot_redirect_the_credential_outside_its_data_root() {
        use std::os::unix::fs::symlink;

        let root = temporary_directory("namespace-link");
        let outside = temporary_directory("namespace-outside");
        nessa_local_storage::create_directory(&outside.join("auth/surfaces")).unwrap();
        let mut token = nessa_local_storage::open(
            &outside.join("auth/surfaces/nessa-panel.token"),
            nessa_local_storage::OpenMode::CreateNew,
        )
        .unwrap();
        token.write_all(b"redirected-token").unwrap();
        drop(token);
        symlink(&outside, root.join("ci")).unwrap();

        let storage = SurfaceCredential::for_namespace(
            Some(ServiceNamespace {
                root: root.clone(),
                relative: PathBuf::from("ci"),
            }),
            "ci".into(),
        );
        assert_eq!(storage.read("ci").unwrap_err(), CredentialRefusal::Refused);
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }
}
