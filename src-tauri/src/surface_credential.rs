//! Native storage for the bundled chat surface. Renderer input never selects a file.
use crate::composition::HostDependencies;
use crate::gateway::application::Gateway;
use crate::panel;
use std::{io::Read, path::PathBuf};
use tauri::State;

/// Where the bundled surface's token comes from.
///
/// The host's own port: a stage in, a token or a reason out, and no path or
/// file handle visible to a caller. The real implementation reads the
/// stage-scoped credential file; a test substitutes one that answers at once,
/// including with the refusals a real keyring failure is hardest to arrange.
pub trait SurfaceCredentials: Send + Sync {
    /// The token for `stage`, or why there is not one to hand over.
    fn read(&self, stage: &str) -> Result<String, String>;
}

pub struct SurfaceCredential {
    root: Option<PathBuf>,
    relative: PathBuf,
    stage: String,
}

impl SurfaceCredential {
    /// The credential file this process's environment points at.
    ///
    /// `stage` is passed in rather than read here: composition resolves the
    /// stage once and the gateway is registered under the same one, and two
    /// independent reads of `NESSA_STAGE` are two things that could disagree.
    pub fn from_environment(stage: String) -> Self {
        let segment = |value: &str| {
            !value.is_empty()
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        };
        let instance = std::env::var("NESSA_INSTANCE").ok();
        let base = std::env::var("NESSA_DATA_DIR")
            .map(PathBuf::from)
            .ok()
            .or_else(|| {
                std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                    .ok()
                    .map(|home| PathBuf::from(home).join(".nessa"))
            });
        let root = base.filter(|base| {
            base.is_absolute() && segment(&stage) && instance.as_deref().is_none_or(segment)
        });
        let mut relative = PathBuf::new();
        if stage != "prod" {
            relative.push(&stage);
        }
        if let Some(instance) = instance {
            relative.push("instances");
            relative.push(instance);
        }
        relative.push("auth/surfaces/nessa-panel.token");
        Self {
            root,
            relative,
            stage,
        }
    }
}

/// Say which of the two things went wrong, because the repairs differ.
///
/// A file that is not there has never been provisioned, and starting the local
/// server provisions it. A file that is there and was refused is a permissions
/// problem the server will not touch, because provisioning never replaces an
/// existing credential. The old wording covered both with "missing or unsafe"
/// and named no command, so neither case told anybody what to do. Anything else
/// keeps the operating system's own words rather than a guessed cause.
fn unreadable(error: &std::io::Error) -> String {
    match error.kind() {
        std::io::ErrorKind::NotFound => {
            "No chat credential has been provisioned yet. Start the local server \
             (`just start`, or `just server`), which creates one on first run."
                .into()
        }
        std::io::ErrorKind::PermissionDenied => {
            "The chat credential was refused: it, or a directory above it, must be \
             yours alone (mode 0600/0700 on Unix, a private DACL on Windows). Repair \
             those permissions, or remove the credential file and start the local \
             server to provision a new one."
                .into()
        }
        _ => format!("The chat credential could not be opened: {error}."),
    }
}

impl SurfaceCredentials for SurfaceCredential {
    fn read(&self, stage: &str) -> Result<String, String> {
        if stage != self.stage {
            return Err("Desktop and gateway stages must match".into());
        }
        let root = self
            .root
            .as_ref()
            .ok_or("Invalid native credential namespace")?;
        let mut file = nessa_local_storage::open_beneath(
            root,
            &self.relative,
            nessa_local_storage::OpenMode::ReadNonblocking,
        )
        .map_err(|error| unreadable(&error))?;
        if file
            .metadata()
            .map_err(|_| "Cannot inspect chat credential")?
            .len()
            > 16385
        {
            return Err("Chat credential is too large".into());
        }
        let mut token = String::new();
        (&mut file)
            .take(16385)
            .read_to_string(&mut token)
            .map_err(|_| "Cannot read chat credential")?;
        let token = token.trim().to_owned();
        if token.is_empty() || token.len() > 16384 {
            return Err("Chat credential is invalid".into());
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
    credential: &dyn SurfaceCredentials,
    stage: &str,
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
    credential.read(stage)
}

#[tauri::command]
pub async fn load_surface_credential(
    window: tauri::WebviewWindow,
    deps: State<'_, HostDependencies>,
    stage: String,
) -> Result<String, String> {
    let gateway = deps.gateway.clone();
    let credential = deps.credential.clone();
    load_for(window.label(), gateway.as_deref(), &*credential, &stage).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::application::{GatewayError, GatewayHost, ReconciledGateway};
    use std::{fs, io::Write, path::Path, sync::Arc, sync::Mutex};

    /// A credential source that has already made up its mind, and writes down
    /// whether it was asked at all.
    struct FakeCredentials(Result<String, String>, Mutex<u32>);

    impl FakeCredentials {
        fn holding(token: &str) -> Self {
            Self(Ok(token.to_string()), Mutex::new(0))
        }

        fn refusing(reason: &str) -> Self {
            Self(Err(reason.to_string()), Mutex::new(0))
        }

        fn reads(&self) -> u32 {
            *self.1.lock().unwrap()
        }
    }

    impl SurfaceCredentials for FakeCredentials {
        fn read(&self, _stage: &str) -> Result<String, String> {
            *self.1.lock().unwrap() += 1;
            self.0.clone()
        }
    }

    /// A background service host that registers, or refuses to, without launchd.
    struct FakeHost {
        registration: Result<ReconciledGateway, GatewayError>,
        registrations: Mutex<u32>,
    }

    impl GatewayHost for FakeHost {
        fn register(&self, _: &Path, _: &str) -> Result<ReconciledGateway, GatewayError> {
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
            Gateway::bootstrap(host.clone(), "/runtime".into(), "ci".into()),
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
        )
    }

    fn load(
        label: &str,
        gateway: Option<&Gateway>,
        credential: &dyn SurfaceCredentials,
    ) -> Result<String, String> {
        tauri::async_runtime::block_on(load_for(label, gateway, credential, "ci"))
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
        let reason = unreadable(&std::io::Error::from(std::io::ErrorKind::NotFound));
        let credential = FakeCredentials::refusing(&reason);

        assert_eq!(
            load(panel::MAIN_WINDOW, None, &credential).err(),
            Some(reason)
        );
    }

    /// Absent and refused are different sentences with different instructions,
    /// and an unexpected failure is neither: it keeps the OS's own words.
    #[test]
    fn absence_and_refusal_are_told_apart() {
        let absent = unreadable(&std::io::Error::from(std::io::ErrorKind::NotFound));
        let refused = unreadable(&std::io::Error::from(std::io::ErrorKind::PermissionDenied));

        assert!(absent.contains("provisioned yet"), "{absent}");
        assert!(absent.contains("just start"), "{absent}");
        assert!(refused.contains("refused"), "{refused}");
        assert!(!refused.contains("provisioned yet"), "{refused}");
        assert!(
            unreadable(&std::io::Error::from(std::io::ErrorKind::TimedOut))
                .contains("could not be opened")
        );
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
        assert!(
            absent.read("ci").unwrap_err().contains("provisioned yet"),
            "an absent credential names provisioning as the next step"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(
                storage.read("ci").unwrap_err().contains("refused"),
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
