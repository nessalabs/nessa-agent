//! Native storage for the bundled chat surface. Renderer input never selects a file.
use crate::composition::HostDependencies;
use crate::gateway::application::Gateway;
use crate::panel;
use std::{io::Read, path::PathBuf, sync::Arc};
use tauri::State;

/// Why there is no token to hand over.
///
/// A variant rather than a sentence, because the repairs differ and something
/// has to be able to tell them apart. The sentence is still here — it is what
/// [`Display`] writes, and the command hands that to the webview — but the fact
/// is the variant, so a test asserts which refusal happened instead of matching
/// a fragment of prose that any rewording breaks.
#[derive(Debug, PartialEq, Eq)]
pub enum CredentialRefusal {
    /// Never provisioned. Starting the local server creates one.
    NotProvisioned,
    /// There, and the operating system would not open it.
    Refused,
    /// Asked for a stage this credential was not built for.
    WrongStage,
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
            Self::NotProvisioned => out.write_str(
                "No chat credential has been provisioned yet. Start the local server \
                 (`just start`, or `just server`), which creates one on first run.",
            ),
            Self::Refused => out.write_str(
                "The chat credential was refused: it, or a directory above it, must be \
                 yours alone (mode 0600/0700 on Unix, a private DACL on Windows). Repair \
                 those permissions, or remove the credential file and start the local \
                 server to provision a new one.",
            ),
            Self::WrongStage => out.write_str("Desktop and gateway stages must match"),
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
    pub fn for_namespace(root: Option<PathBuf>, stage: String) -> Self {
        Self {
            root,
            relative: PathBuf::from("auth/surfaces/nessa-panel.token"),
            stage,
        }
    }
}

/// Resolve the service namespace once for composition to inject into endpoint
/// and credential readers. Environment reads stay at this composition edge.
pub(crate) fn service_namespace_from_environment(stage: &str) -> Option<PathBuf> {
    let instance = std::env::var("NESSA_INSTANCE").ok();
    let base = std::env::var("NESSA_DATA_DIR")
        .map(PathBuf::from)
        .ok()
        .or_else(|| {
            std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .ok()
                .map(|home| PathBuf::from(home).join(".nessa"))
        });
    service_namespace(base, stage, instance.as_deref())
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
fn service_namespace(
    base: Option<PathBuf>,
    stage: &str,
    instance: Option<&str>,
) -> Option<PathBuf> {
    let mut root =
        base.filter(|base| base.is_absolute() && segment(stage) && instance.is_none_or(segment))?;
    if stage != "prod" {
        root.push(stage);
    }
    if let Some(instance) = instance {
        root.push("instances");
        root.push(instance);
    }
    Some(root)
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
    match error.kind() {
        std::io::ErrorKind::NotFound => CredentialRefusal::NotProvisioned,
        std::io::ErrorKind::PermissionDenied => CredentialRefusal::Refused,
        _ => CredentialRefusal::Unopenable(error.to_string()),
    }
}

impl SurfaceCredentials for SurfaceCredential {
    fn read(&self, stage: &str) -> Result<String, CredentialRefusal> {
        if stage != self.stage {
            return Err(CredentialRefusal::WrongStage);
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
/// server: only the bundled panel may ask; a packaged build waits for the
/// gateway to reconcile before handing anything over, and a build without one
/// does not wait at all; and the refusal for the wrong window happens before
/// either of those, so a stray webview cannot make the app register a service.
async fn load_for(
    label: &str,
    gateway: Option<&Gateway>,
    endpoint: Arc<crate::gateway_endpoint::application::GatewayEndpointAccess>,
    credential: &dyn SurfaceCredentials,
    stage: &str,
    url: &str,
) -> Result<String, String> {
    if label != panel::MAIN_WINDOW {
        return Err("Only the bundled chat surface can load this credential".into());
    }
    if let Some(gateway) = gateway {
        gateway
            .wait_ready()
            .await
            .map_err(|error| error.to_string())?;
    }
    let stage_for_endpoint = stage.to_owned();
    let requested_url = url.to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        endpoint.permits_credential_for(&stage_for_endpoint, &requested_url)
    })
    .await
    .map_err(|error| error.to_string())??;
    // The variant becomes a sentence here, at the edge: the webview takes a
    // string, and everything above this point can still tell the refusals apart.
    credential
        .read(stage)
        .map_err(|refusal| refusal.to_string())
}

#[tauri::command]
pub async fn load_surface_credential(
    window: tauri::WebviewWindow,
    deps: State<'_, HostDependencies>,
    stage: String,
    url: String,
) -> Result<String, String> {
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
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::application::{
        testing::system_login_shell, GatewayError, GatewayHost, ReconciledGateway,
    };
    use crate::gateway::domain::value_objects::SearchPath;
    use nessa_gateway_endpoint::{
        application::EndpointDiscovery,
        domain::{EndpointIdentity, GatewayEndpoint},
    };
    use std::{fs, io::Write, path::Path, sync::Arc, sync::Mutex};

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
            match &self.outcome {
                Ok(token) => Ok(token.clone()),
                Err(CredentialRefusal::NotProvisioned) => Err(CredentialRefusal::NotProvisioned),
                Err(CredentialRefusal::Refused) => Err(CredentialRefusal::Refused),
                Err(CredentialRefusal::WrongStage) => Err(CredentialRefusal::WrongStage),
                Err(CredentialRefusal::UnusableNamespace) => {
                    Err(CredentialRefusal::UnusableNamespace)
                }
                Err(CredentialRefusal::NotAToken(why)) => Err(CredentialRefusal::NotAToken(why)),
                Err(CredentialRefusal::Unopenable(error)) => {
                    Err(CredentialRefusal::Unopenable(error.clone()))
                }
            }
        }
    }

    /// A background service host that registers, or refuses to, without launchd.
    struct FakeHost {
        registration: Result<ReconciledGateway, GatewayError>,
        registrations: Mutex<u32>,
    }

    impl GatewayHost for FakeHost {
        fn register(
            &self,
            _: &Path,
            _: &str,
            _: Option<&SearchPath>,
        ) -> Result<ReconciledGateway, GatewayError> {
            *self.registrations.lock().unwrap() += 1;
            self.registration.clone()
        }

        fn stop_agents(&self, _: &ReconciledGateway) -> Result<(), GatewayError> {
            Ok(())
        }
    }

    fn gateway(registration: Result<ReconciledGateway, GatewayError>) -> (Gateway, Arc<FakeHost>) {
        let host = Arc::new(FakeHost {
            registration,
            registrations: Mutex::new(0),
        });
        (
            Gateway::bootstrap(
                host.clone(),
                system_login_shell(),
                "/runtime".into(),
                "ci".into(),
            ),
            host,
        )
    }

    fn reconciled() -> ReconciledGateway {
        ReconciledGateway::new(
            "com.nessa.gateway".into(),
            "fingerprint".into(),
            "instance".into(),
            "generation".into(),
            42,
            7420,
        )
    }

    fn load(
        label: &str,
        gateway: Option<&Gateway>,
        credential: &dyn SurfaceCredentials,
    ) -> Result<String, String> {
        tauri::async_runtime::block_on(load_for(
            label,
            gateway,
            Arc::new(endpoint_access(None)),
            credential,
            "ci",
            "ws://127.0.0.1:7420/session",
        ))
    }

    /// The whole of the ordering: the window is checked first, then the gateway
    /// is waited for, and only then is anything read.
    #[test]
    fn the_bundled_panel_waits_for_the_gateway_and_gets_its_token() {
        let credential = FakeCredentials::holding("fixture-only");
        let (gateway, host) = gateway(Ok(reconciled()));

        assert_eq!(
            load(panel::MAIN_WINDOW, Some(&gateway), &credential).unwrap(),
            "fixture-only"
        );
        assert_eq!(*host.registrations.lock().unwrap(), 1);
        assert_eq!(credential.reads(), 1);
    }

    /// The refusal that matters most: any other window is turned away before
    /// the gateway is touched, so a stray webview cannot make a packaged build
    /// register a background service, let alone read a token.
    #[test]
    fn another_window_is_refused_before_the_gateway_is_asked_for_anything() {
        let credential = FakeCredentials::holding("fixture-only");
        let (gateway, host) = gateway(Ok(reconciled()));

        assert_eq!(
            load(panel::SETUP_WINDOW, Some(&gateway), &credential).err(),
            Some("Only the bundled chat surface can load this credential".to_string())
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
        let (gateway, _host) = gateway(Err(GatewayError::Registration("not installed".into())));

        assert_eq!(
            load(panel::MAIN_WINDOW, Some(&gateway), &credential).err(),
            Some("not installed".to_string())
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
            "ws://127.0.0.1:7420/session",
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
            Some(CredentialRefusal::NotProvisioned.to_string())
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

        assert!(absent.contains("provisioned yet"), "{absent}");
        assert!(absent.contains("just start"), "{absent}");
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
        assert_eq!(root, Some(absolute("data")));

        let root = service_namespace(base(), "dev", None);
        assert_eq!(root, Some(absolute("data/dev")));

        let root = service_namespace(base(), "dev", Some("wt"));
        assert_eq!(root, Some(absolute("data/dev/instances/wt")));
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
        assert!(storage.read("prod").is_err());
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
}
