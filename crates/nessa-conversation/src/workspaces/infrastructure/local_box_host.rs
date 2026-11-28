//! A stand-in cloud for Cloud Boxes on this machine, for the conversation
//! stress check's `cloudbox` step (#1427). Composition uses it in place of
//! Boat's machines only in a debug build started with `NESSA_CLOUD_BOX_HOST`.
//!
//! Each "VM" is a directory, and the commands the gateway runs in one run
//! here, with that directory as their home. Everything after the VM is made
//! is the real path: `WorkspaceCloudBoxProvider` runs the real bootstrap
//! helper, which starts a real second gateway and a tunnel stand-in.
//!
//! ```text
//! <root>/bin/          what /opt/nessa/bin/ holds in an image (the helper,
//!                      nessa, cloudflared): commands naming that directory
//!                      run from here instead
//! <root>/env           KEY=VALUE lines every command is given
//! <root>/home/          what an image leaves in its user's home: copied
//!                      into each new VM's home, modes and all
//! <root>/vms/<key>/    one VM, its home; box.env holds its own ports
//! <root>/lose-next-create
//!                      present: the next create makes its VM, then answers
//!                      as an answer lost on the way back (and removes it)
//! <root>/events.log    one line per create, connect, stop and remove
//!
//! Its Boat connection is its own (`LocalBoxSettings`): an account no Boat
//! knows, always connected, so the step needs no sign-in first.
//!
//! create  ──▶ vms/<key>/ (again for the same key: the same VM)
//! connect ──▶ a runtime running `sh -c` in vms/<key>/
//! stop    ──▶ every process the helper started there ends (its disk stays)
//! remove  ──▶ stop, then the directory goes
//! ```
use crate::workspaces::{
    application::{Provider, RestCause, Settings, WorkspaceReceipt, WorkspaceRuntime},
    domain::{
        Admitted, BoatConnection, CloudAccount, CloudConnection, EnvironmentIdentity, Failure,
        RefusalDisposition, VmSpec,
    },
};
use async_trait::async_trait;
use std::{
    collections::BTreeMap,
    io::{self, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    sync::Arc,
};

/// Where an image keeps the programs a Cloud Box runs.
const IMAGE_BIN: &str = "/opt/nessa/bin/";
/// The helper's own directory under a VM's home (`NESSA_BOX_DIR`'s default).
const BOX_DIR: &str = ".nessa-box";
/// The environment every stand-in VM is admitted under.
const ENVIRONMENT_ID: &str = "6f1d3c1e-4c2b-4f43-9a51-1d6f0b6a2c01";
const ENVIRONMENT_VERSION_ID: &str = "8b2e6a4d-3f7c-4d1e-a2b9-5c0d7e8f9a12";

pub struct LocalBoxHost {
    root: PathBuf,
}

impl LocalBoxHost {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn vm_dir(&self, vm: &str) -> Result<PathBuf, Failure> {
        if vm.is_empty()
            || !vm
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        {
            return Err(Failure::InvalidInput);
        }
        Ok(self.root.join("vms").join(vm))
    }

    fn log(&self, line: &str) {
        let written = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join("events.log"))
            .and_then(|mut file| writeln!(file, "{line}"));
        if let Err(error) = written {
            tracing::warn!(%error, "the local box host could not log an event");
        }
    }

    fn admission() -> Result<Admitted, Failure> {
        EnvironmentIdentity::new(
            ENVIRONMENT_ID.into(),
            "nessa".into(),
            ENVIRONMENT_VERSION_ID.into(),
            1,
        )
        .map(Admitted::Boat)
    }

    /// End every process the helper started in `dir`, as an archived VM
    /// keeps its disk and runs nothing, and wait until each has ended: a
    /// stopped VM runs nothing, and a process still exiting would keep
    /// writing into a directory being removed.
    async fn end_processes(dir: &Path) {
        let mut groups = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir.join(BOX_DIR)) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("pid") {
                continue;
            }
            let Some(pid) = std::fs::read_to_string(&path)
                .ok()
                .and_then(|text| text.trim().parse::<i32>().ok())
                .filter(|pid| *pid > 1)
            else {
                continue;
            };
            // Each was started in a session of its own: its group goes too.
            let ended = std::process::Command::new("kill")
                .args(["-TERM", "--", &format!("-{pid}")])
                .stderr(std::process::Stdio::null())
                .status();
            match ended {
                Ok(_) => groups.push(pid),
                Err(error) => {
                    tracing::warn!(%error, pid, "the local box host could not end a process");
                }
            }
            let _ = std::fs::remove_file(&path);
        }
        for _ in 0..END_WAIT_STEPS {
            groups.retain(|pid| group_runs(*pid));
            if groups.is_empty() {
                return;
            }
            crate::timer::clock::sleep(END_WAIT_STEP).await;
        }
        for pid in groups {
            tracing::warn!(pid, "a local box process outlived its stop; killing it");
            let _ = std::process::Command::new("kill")
                .args(["-KILL", "--", &format!("-{pid}")])
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
}

/// How long a stop waits for the box's processes to end, in steps.
const END_WAIT_STEP: std::time::Duration = std::time::Duration::from_millis(100);
const END_WAIT_STEPS: usize = 150;

/// Whether any process of the group led by `pid` still runs.
fn group_runs(pid: i32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", "--", &format!("-{pid}")])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Copy `from` into `to`, keeping each entry's mode; a missing `from` is an
/// image that leaves nothing in the home.
fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    let entries = match std::fs::read_dir(from) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let target = to.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            std::fs::create_dir_all(&target)?;
            std::fs::set_permissions(&target, entry.metadata()?.permissions())?;
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Two free loopback ports, for one VM's gateway and its tunnel's metrics.
fn free_ports() -> io::Result<(u16, u16)> {
    let first = TcpListener::bind("127.0.0.1:0")?;
    let second = TcpListener::bind("127.0.0.1:0")?;
    Ok((first.local_addr()?.port(), second.local_addr()?.port()))
}

/// `KEY=VALUE` lines; anything else is skipped.
fn read_env(path: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
        .filter(|(key, _)| !key.is_empty() && !key.starts_with('#'))
        .collect()
}

struct LocalRuntime {
    root: PathBuf,
    home: PathBuf,
}

#[async_trait]
impl WorkspaceRuntime for LocalRuntime {
    async fn run(&self, command: String, input: Option<std::fs::File>) -> io::Result<String> {
        let bin = format!("{}/", self.root.join("bin").display());
        let command = command.replace(IMAGE_BIN, &bin);
        let mut process = tokio::process::Command::new("sh");
        process
            .arg("-c")
            .arg(command)
            .current_dir(&self.home)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &self.home)
            .envs(read_env(&self.root.join("env")))
            .envs(read_env(&self.home.join("box.env")))
            .stdin(match input {
                Some(file) => std::process::Stdio::from(file),
                None => std::process::Stdio::null(),
            })
            .kill_on_drop(true);
        let output = process.output().await?;
        crate::workspaces::infrastructure::ssh::log_stderr(
            &self.home.display().to_string(),
            &String::from_utf8_lossy(&output.stderr),
        );
        if !output.status.success() {
            return Err(io::Error::other(format!(
                "command ended with {}",
                output.status
            )));
        }
        String::from_utf8(output.stdout).map_err(|_| io::Error::other("output is not UTF-8"))
    }
}

#[async_trait]
impl Provider for LocalBoxHost {
    async fn validate(&self, _account: &CloudAccount, _spec: &VmSpec) -> Result<(), Failure> {
        Ok(())
    }

    async fn create(
        &self,
        _account: &CloudAccount,
        _spec: &VmSpec,
        key: &str,
    ) -> Result<WorkspaceReceipt, Failure> {
        let dir = self.vm_dir(key)?;
        if !dir.join("box.env").exists() {
            std::fs::create_dir_all(&dir).map_err(|_| Failure::Provider)?;
            copy_tree(&self.root.join("home"), &dir).map_err(|_| Failure::Provider)?;
            let (port, metrics) = free_ports().map_err(|_| Failure::Provider)?;
            std::fs::write(
                dir.join("box.env"),
                format!("NESSA_PORT={port}\nNESSA_APP_TUNNEL_METRICS=127.0.0.1:{metrics}\n"),
            )
            .map_err(|_| Failure::Provider)?;
        }
        if std::fs::remove_file(self.root.join("lose-next-create")).is_ok() {
            self.log(&format!("create {key} lost"));
            return Err(Failure::Provider);
        }
        self.log(&format!("create {key}"));
        Ok(WorkspaceReceipt {
            vm: key.to_owned(),
            admission: Self::admission()?,
            refusal: None,
            disposition: RefusalDisposition::RollbackNewVm,
        })
    }

    async fn connect(
        &self,
        _account: &CloudAccount,
        _spec: &VmSpec,
        _key: &str,
        vm: &str,
        _admission: &Admitted,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure> {
        let home = self.vm_dir(vm)?;
        if !home.is_dir() {
            return Err(Failure::Provider);
        }
        self.log(&format!("connect {vm}"));
        Ok(Arc::new(LocalRuntime {
            root: self.root.clone(),
            home,
        }))
    }

    async fn remove(
        &self,
        _account: &CloudAccount,
        vm: &str,
        _admission: &Admitted,
    ) -> Result<(), Failure> {
        let dir = self.vm_dir(vm)?;
        Self::end_processes(&dir).await;
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return Err(Failure::Provider),
        }
        self.log(&format!("remove {vm}"));
        Ok(())
    }

    async fn stop(
        &self,
        _account: &CloudAccount,
        vm: &str,
        _cause: RestCause,
    ) -> Result<(), Failure> {
        Self::end_processes(&self.vm_dir(vm)?).await;
        self.log(&format!("stop {vm}"));
        Ok(())
    }
}

/// The stand-in cloud's connection: always there, never saved or cleared,
/// and its token reaches nothing but [`LocalBoxHost`].
pub struct LocalBoxSettings;

#[async_trait]
impl Settings for LocalBoxSettings {
    async fn read(&self) -> Result<CloudConnection, Failure> {
        BoatConnection::new(
            "local-box-host-token".into(),
            "nessa".into(),
            "local-box".into(),
        )
        .map(CloudConnection::Boat)
    }
    fn generation(&self) -> Result<u64, Failure> {
        Ok(0)
    }
    async fn save(&self, _generation: u64, _connection: CloudConnection) -> Result<(), Failure> {
        Err(Failure::InvalidInput)
    }
    async fn clear(&self) -> Result<(), Failure> {
        Err(Failure::InvalidInput)
    }
}
