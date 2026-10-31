//! Which copy of `nessa` serves a cloud workspace's VM: the image's own
//! while it speaks this build's lease protocol, else this build, installed
//! in the VM over its own connection.
use super::*;
use crate::conversation::infrastructure::ssh_environment::{
    Build, BuildSource, EnvironmentEvent, InstallTimings,
};
use crate::env_serve::install::Platform;
use crate::workspaces::{application::WorkspaceStream, domain::Failure};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const THIS_BUILD: &[u8] = b"this build";

fn this_digest() -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(THIS_BUILD))
}

struct Built;
impl BuildSource for Built {
    fn open(&self) -> io::Result<Build> {
        let mut file = tempfile::tempfile()?;
        std::io::Write::write_all(&mut file, THIS_BUILD)?;
        std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(0))?;
        Ok(Build {
            file,
            digest: this_digest(),
        })
    }
}

fn installer() -> HostInstaller {
    HostInstaller::new(
        Arc::new(Unused),
        Arc::new(Built),
        Platform {
            os: "linux".into(),
            arch: "x86_64".into(),
            libc: "gnu".into(),
        },
        LEASE_PROTOCOL,
        InstallTimings {
            probe: Duration::from_secs(5),
            upload: Duration::from_secs(5),
        },
    )
}

/// The installer's own shell: every command of a VM's install runs over
/// that VM's connection instead.
struct Unused;
impl RemoteShell for Unused {
    fn run<'a>(
        &'a self,
        _: &'a SshDestination,
        _: String,
        _: Option<std::fs::File>,
    ) -> ShellFuture<'a> {
        panic!("a VM's install ran on the installer's own shell")
    }
}

#[derive(Default)]
struct Audit(Mutex<Vec<EnvironmentEvent>>);
impl EnvironmentAudit for Audit {
    fn record(&self, event: &EnvironmentEvent) -> io::Result<()> {
        self.0.lock().unwrap().push(event.clone());
        Ok(())
    }
}

/// A VM whose image's copy says `image`, answering the installer's probe
/// and upload as a Linux VM does.
struct Vm {
    image: Option<String>,
    installed: AtomicBool,
    commands: Mutex<Vec<String>>,
}
impl Vm {
    fn with_image(image: Option<&str>) -> Self {
        Self {
            image: image.map(str::to_owned),
            installed: AtomicBool::new(false),
            commands: Mutex::default(),
        }
    }
}
#[async_trait::async_trait]
impl WorkspaceRuntime for Vm {
    async fn serve(&self, _: ServedCopy) -> Result<WorkspaceStream, Failure> {
        Err(Failure::Provider)
    }
    async fn image_protocol(&self) -> Option<String> {
        self.image.clone()
    }
    async fn run(&self, command: String, input: Option<std::fs::File>) -> io::Result<String> {
        self.commands.lock().unwrap().push(command.clone());
        if command.contains("umask 077") {
            assert!(input.is_some(), "an upload sends the build");
            self.installed.store(true, Ordering::SeqCst);
            return Ok("installed".into());
        }
        Ok(if self.installed.load(Ordering::SeqCst) {
            "present".into()
        } else {
            "absent Linux x86_64 gnu".into()
        })
    }
}

fn conversation() -> ConversationId {
    ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap()
}

#[tokio::test]
async fn an_image_that_speaks_this_protocol_serves_as_it_is() {
    let vm = Vm::with_image(Some(LEASE_PROTOCOL));
    let copy = served_copy(&installer(), &Audit::default(), &conversation(), &vm, true).await;
    assert_eq!(copy, Ok(ServedCopy::Image));
    assert!(vm.commands.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_vm_from_an_older_image_gets_this_build_installed_and_served() {
    let vm = Vm::with_image(Some("0000000000000000"));
    let audit = Audit::default();
    let installer = installer();
    let copy = served_copy(&installer, &audit, &conversation(), &vm, true).await;
    let served = installer.serve_command().await.unwrap();
    assert!(served.contains(&this_digest()));
    assert_eq!(copy, Ok(ServedCopy::Installed(served.clone())));
    assert!(vm.installed.load(Ordering::SeqCst));
    // Recorded as any host's install, the VM named by its thread.
    let host = format!("boat-{}", conversation());
    assert!(matches!(
        audit.0.lock().unwrap().last(),
        Some(EnvironmentEvent::Installed { host: named, lease, .. })
            if *named == host && *lease == conversation().to_string()
    ));
    // Opened again, the copy is there: nothing is sent twice.
    let again = served_copy(&installer, &audit, &conversation(), &vm, true).await;
    assert_eq!(again, Ok(ServedCopy::Installed(served)));
    assert_eq!(
        vm.commands
            .lock()
            .unwrap()
            .iter()
            .filter(|command| command.contains("umask 077"))
            .count(),
        1
    );
}

#[tokio::test]
async fn an_account_of_a_held_lease_never_installs() {
    let vm = Vm::with_image(None);
    let installer = installer();
    let copy = served_copy(&installer, &Audit::default(), &conversation(), &vm, false).await;
    assert_eq!(
        copy,
        Ok(ServedCopy::Installed(
            installer.serve_command().await.unwrap()
        ))
    );
    assert!(vm.commands.lock().unwrap().is_empty());
}
