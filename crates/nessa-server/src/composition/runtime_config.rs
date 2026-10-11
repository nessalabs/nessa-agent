//! File loading belongs to composition; consumers receive typed settings.
use super::agent::AgentsConfig;
use crate::{
    conversation::domain::CommandPolicy,
    core::RunError,
    product::{ConfiguredLimits, OperationalLimits, SessionSettings},
};
use nessa_auth::adapters::local::LocalStoreConfig;
use nessa_sdk::domain::agent_execution::leases::SshDestination;
use serde::Deserialize;
use std::{io::Read, net::SocketAddr, path::Path, time::Duration};

/// The most bytes `config.json` may have: what startup reads and what a
/// change to the stored MCP servers may write (`mcp_servers::settings`).
pub(super) const MAX_CONFIG_BYTES: usize = 65_536;

#[derive(Default, Debug, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct RuntimeConfig {
    pub registry: LocalStoreConfig,
    pub session: SessionConfig,
    /// Tier-3 admission and the cold-read budget. Absent keeps the defaults.
    pub limits: LimitsConfig,
    pub agents: Option<AgentsConfig>,
    /// Native device pairing; absent or `null` keeps it off (design rows S1, S2).
    pub native: Option<NativeConfig>,
    /// The SSH hosts a conversation may be created on, each an OpenSSH
    /// destination (a `~/.ssh/config` alias or `user@host`). Absent or empty
    /// names none, and no conversation reaches a host (issue #699).
    pub ssh_hosts: Vec<String>,
    /// What `nessa env serve` on this host offers a gateway beyond agents.
    /// Absent offers nothing more. Read only where `env serve` runs: Unix.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub env_serve: EnvServeConfig,
    /// The agent's environment tools, `environments_list` and `run`
    /// (issue #700). Absent or `null` grants neither: the agent sees no such
    /// tools.
    pub environment_tools: Option<EnvironmentToolsConfig>,
}

/// Where the agent may run commands, and which.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct EnvironmentToolsConfig {
    /// The `sshHosts` entries commands are granted on. Every one must be
    /// named there too.
    pub command_hosts: Vec<String>,
    /// When given, only these programs, each named by its file name.
    #[serde(default)]
    pub allow_programs: Option<Vec<String>>,
    /// Never these programs, each named by its file name; a denial wins.
    #[serde(default)]
    pub deny_programs: Vec<String>,
}

/// Most programs `allowPrograms` or `denyPrograms` may name.
pub(super) const MAX_POLICY_PROGRAMS: usize = 64;

/// What this host serves a gateway beyond its agents' harnesses.
#[derive(Default, Debug, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct EnvServeConfig {
    /// Whether a gateway may run commands here under command leases
    /// (issue #700). Off unless set: a command runs as this account with
    /// nothing enclosing it.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub commands: bool,
}

/// Most SSH hosts the configuration may name: what `agents.list` carries.
pub(super) const MAX_SSH_HOSTS: usize = 16;

/// Where the native enrollment listener binds. The address is numeric: serde's
/// standard `SocketAddr` parser, so no hostname is ever looked up.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct NativeConfig {
    pub listen_address: SocketAddr,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct SessionConfig {
    handshake_timeout_ms: u64,
    write_timeout_ms: u64,
    current_state_interval_ms: u64,
}
/// Counts and the cold-read budget. Omitted fields stay the value object's defaults.
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct LimitsConfig {
    requests: u64,
    controls: u64,
    record_reads: u64,
    upload_begins: u64,
    deletions: u64,
    ordinary_slots: u64,
    control_slots: u64,
    app_calls_per_socket: u64,
    read_work_budget_ms: u64,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        let limits = OperationalLimits::default();
        Self {
            requests: limits.requests() as u64,
            controls: limits.controls() as u64,
            record_reads: limits.record_reads() as u64,
            upload_begins: limits.upload_begins() as u64,
            deletions: limits.deletions() as u64,
            ordinary_slots: limits.ordinary_slots() as u64,
            control_slots: limits.control_slots() as u64,
            app_calls_per_socket: limits.app_calls_per_socket() as u64,
            read_work_budget_ms: u64::try_from(limits.read_work_budget().as_millis())
                .unwrap_or(u64::MAX),
        }
    }
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            handshake_timeout_ms: 10_000,
            write_timeout_ms: 5_000,
            current_state_interval_ms: 1_000,
        }
    }
}
impl RuntimeConfig {
    /// Optional config.json beside auth/, scoped to the same stage and instance.
    /// Invalid existing files fail startup; only a missing file selects defaults.
    pub fn load(auth_directory: &Path) -> Result<Self, RunError> {
        let path = config_path(
            auth_directory
                .parent()
                .ok_or_else(|| invalid("invalid data directory"))?,
        );
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => return Err(refused("config.json must be a regular file")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default())
            }
            Err(error) => return Err(invalid(error)),
        }
        let file = nessa_local_storage::open(&path, nessa_local_storage::OpenMode::ReadNonblocking)
            .map_err(invalid)?;
        let mut bytes = Vec::new();
        file.take(MAX_CONFIG_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(invalid)?;
        if bytes.len() > MAX_CONFIG_BYTES {
            return Err(refused("config.json exceeds 64 KiB"));
        }
        Self::parse(&bytes)
    }

    pub(super) fn parse(bytes: &[u8]) -> Result<Self, RunError> {
        let config: Self = serde_json::from_slice(bytes).map_err(refused)?;
        config.registry.validate().map_err(refused)?;
        config.session()?;
        config.limits()?;
        config.ssh_hosts()?;
        config.command_policy()?;
        Ok(config)
    }

    /// The agent's command policy; `None` when its environment tools are not
    /// configured. A host not in `sshHosts`, or a program named by more than
    /// its file name, refuses the configuration.
    pub fn command_policy(&self) -> Result<Option<CommandPolicy>, RunError> {
        let Some(tools) = &self.environment_tools else {
            return Ok(None);
        };
        let hosts = self.ssh_hosts()?;
        for host in &tools.command_hosts {
            if !hosts.iter().any(|named| named.as_str() == host) {
                return Err(refused(format!(
                    "environmentTools.commandHosts: {host:?} is not in sshHosts"
                )));
            }
        }
        let programs = |key: &str, names: &[String]| {
            if names.len() > MAX_POLICY_PROGRAMS {
                return Err(refused(format!(
                    "environmentTools.{key} names more than {MAX_POLICY_PROGRAMS} programs"
                )));
            }
            match names
                .iter()
                .find(|name| name.is_empty() || name.contains('/') || name.trim() != name.as_str())
            {
                Some(name) => Err(refused(format!(
                    "environmentTools.{key}: {name:?} is not a program's file name"
                ))),
                None => Ok(()),
            }
        };
        if let Some(allow) = &tools.allow_programs {
            programs("allowPrograms", allow)?;
        }
        programs("denyPrograms", &tools.deny_programs)?;
        Ok(Some(CommandPolicy::new(
            tools.command_hosts.iter().cloned(),
            tools.allow_programs.clone(),
            tools.deny_programs.clone(),
        )))
    }

    /// The configured SSH hosts, each read into its value object, which owns
    /// what a destination may be: never an option to `ssh`, never a shell
    /// word. A repeated or unreadable one refuses the configuration.
    pub fn ssh_hosts(&self) -> Result<Vec<SshDestination>, RunError> {
        if self.ssh_hosts.len() > MAX_SSH_HOSTS {
            return Err(refused(format!(
                "sshHosts names more than {MAX_SSH_HOSTS} hosts"
            )));
        }
        let mut hosts: Vec<SshDestination> = Vec::with_capacity(self.ssh_hosts.len());
        for host in &self.ssh_hosts {
            let destination = SshDestination::new(host.as_str())
                .map_err(|_| refused(format!("sshHosts: {host:?} is not an SSH destination")))?;
            if hosts.contains(&destination) {
                return Err(refused(format!("sshHosts names {host:?} twice")));
            }
            hosts.push(destination);
        }
        Ok(hosts)
    }

    pub fn session(&self) -> Result<SessionSettings, RunError> {
        // The settings type owns what a usable deadline is, so a configuration
        // file and an embedding caller are rejected by the same rule.
        SessionSettings::new(
            Duration::from_millis(self.session.handshake_timeout_ms),
            Duration::from_millis(self.session.write_timeout_ms),
            Duration::from_millis(self.session.current_state_interval_ms),
        )
        .map_err(refused)
    }

    pub fn limits(&self) -> Result<OperationalLimits, RunError> {
        // The value object owns what a usable count and budget are, so a
        // configuration file and an embedding caller are rejected by the same rule.
        OperationalLimits::configured(ConfiguredLimits {
            requests: self.limits.requests,
            controls: self.limits.controls,
            record_reads: self.limits.record_reads,
            upload_begins: self.limits.upload_begins,
            deletions: self.limits.deletions,
            ordinary_slots: self.limits.ordinary_slots,
            control_slots: self.limits.control_slots,
            app_calls_per_socket: self.limits.app_calls_per_socket,
            read_work_budget: Duration::from_millis(self.limits.read_work_budget_ms),
        })
        .map_err(refused)
    }
}
/// Where the namespace at `namespace` keeps `config.json`.
pub(super) fn config_path(namespace: &Path) -> std::path::PathBuf {
    namespace.join("config.json")
}

/// `config.json` could not be read: the file system can change before the next
/// start, so this stays a retried setup failure.
fn invalid(error: impl std::fmt::Display) -> RunError {
    RunError::Authentication(format!("invalid runtime config: {error}"))
}
/// `config.json` was read and its contents refused. Reading the same file
/// again refuses it the same way (design row S2).
fn refused(error: impl std::fmt::Display) -> RunError {
    RunError::RuntimeConfig(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::product::InvalidSessionSettings;
    #[test]
    fn config_loading_accepts_only_private_regular_files() {
        use std::io::Write;
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("namespace");
        nessa_local_storage::create_directory(&directory).unwrap();
        let auth = directory.join("auth");
        assert!(RuntimeConfig::load(&auth).is_ok());
        let path = directory.join("config.json");
        nessa_local_storage::open(&path, nessa_local_storage::OpenMode::CreateNew)
            .unwrap()
            .write_all(br#"{"session":{"handshakeTimeoutMs":1000}}"#)
            .unwrap();
        assert_eq!(
            RuntimeConfig::load(&auth)
                .unwrap()
                .session()
                .unwrap()
                .handshake_timeout(),
            Duration::from_secs(1)
        );
        std::fs::hard_link(&path, directory.join("alias.json")).unwrap();
        assert!(RuntimeConfig::load(&auth).is_err());
        std::fs::remove_file(directory.join("alias.json")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::{symlink, PermissionsExt};
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(RuntimeConfig::load(&auth).is_err());
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            let target = directory.join("target.json");
            std::fs::rename(&path, &target).unwrap();
            symlink(&target, &path).unwrap();
            assert!(RuntimeConfig::load(&auth).is_err());
            std::fs::remove_file(target).unwrap();
            assert!(RuntimeConfig::load(&auth).is_err());
        }
    }

    #[test]
    fn partial_settings_override_only_their_scope() {
        let a = RuntimeConfig::parse(br#"{"registry":{"maxCredentials":2000,"maxRegistryBytes":8388608},"session":{"writeTimeoutMs":75}}"#).unwrap();
        let b = RuntimeConfig::parse(b"{}").unwrap();
        assert_eq!(a.registry.max_credentials, 2000);
        assert_eq!(a.registry.max_registry_bytes, 8388608);
        assert_eq!(a.registry.max_receipts, 2000);
        assert_eq!(
            a.session().unwrap().write_timeout(),
            Duration::from_millis(75)
        );
        assert_eq!(b.registry.max_credentials, 1000);
        assert_eq!(b.session().unwrap().write_timeout(), Duration::from_secs(5));
        let narrowed = RuntimeConfig::parse(br#"{"limits":{"requests":2}}"#).unwrap();
        assert_eq!(narrowed.limits().unwrap().requests(), 2);
        assert_eq!(
            narrowed.limits().unwrap().controls(),
            OperationalLimits::default().controls()
        );
    }
    /// Design row S2: refused contents are a configuration failure that a
    /// restart cannot fix, whichever section they are in.
    #[test]
    fn refused_runtime_configuration_is_not_worth_restarting_for() {
        for bytes in [
            br#"{"native":{"listenAddress":"localhost:47650"}}"#.as_slice(),
            br#"{"native":{"listenAddress":"127.0.0.1"}}"#,
            br#"{"native":{"listenAddress":"127.0.0.1:1","tls":true}}"#,
            br#"{"session":{"writeTimeoutMs":0}}"#,
            br#"{"limits":{"requests":0}}"#,
            br#"{"limits":{"appCallsPerSocket":1}}"#,
            br#"{"limits":{"readWorkBudgetMs":0}}"#,
            br#"{"limits":{"unknown":1}}"#,
            b"not json",
        ] {
            assert!(matches!(
                RuntimeConfig::parse(bytes),
                Err(RunError::RuntimeConfig(_))
            ));
        }
    }

    #[test]
    fn invalid_settings_are_never_silently_defaulted() {
        for bytes in [
            br#"{"registry":{"maxCredentials":0}}"#.as_slice(),
            br#"{"registry":{"maxRegistryBytes":18446744073709551615}}"#,
            br#"{"session":{"writeTimeoutMs":0}}"#,
            br#"{"session":{"writeTimeoutMs":-1}}"#,
            br#"{"session":{"writeTimoutMs":3}}"#,
            br#"{"unknown":true}"#,
            br#"{"limits":{"requests":0}}"#,
            br#"{"limits":{"appCallsPerSocket":1}}"#,
            b"not json",
        ] {
            assert!(RuntimeConfig::parse(bytes).is_err());
        }
    }
    /// Design rows S1 and S2: native pairing is off unless named, and a
    /// malformed native section refuses startup before anything is opened.
    #[test]
    fn native_config_refuses_before_effect() {
        assert!(RuntimeConfig::parse(b"{}").unwrap().native.is_none());
        assert!(RuntimeConfig::parse(br#"{"native":null}"#)
            .unwrap()
            .native
            .is_none());
        for (bytes, address) in [
            (
                br#"{"native":{"listenAddress":"127.0.0.1:47650"}}"#.as_slice(),
                "127.0.0.1:47650",
            ),
            (br#"{"native":{"listenAddress":"[::1]:0"}}"#, "[::1]:0"),
            (
                br#"{"native":{"listenAddress":"0.0.0.0:47650"}}"#,
                "0.0.0.0:47650",
            ),
        ] {
            assert_eq!(
                RuntimeConfig::parse(bytes)
                    .unwrap()
                    .native
                    .unwrap()
                    .listen_address,
                address.parse::<SocketAddr>().unwrap()
            );
        }
        for bytes in [
            br#"{"native":{"listenAddress":"localhost:47650"}}"#.as_slice(),
            br#"{"native":{"listenAddress":"127.0.0.1"}}"#,
            br#"{"native":{"listenAddress":""}}"#,
            br#"{"native":{"listenAddress":47650}}"#,
            br#"{"native":{}}"#,
            br#"{"native":{"listenAddress":"127.0.0.1:1","tls":true}}"#,
            br#"{"native":"127.0.0.1:47650"}"#,
        ] {
            assert!(
                RuntimeConfig::parse(bytes).is_err(),
                "{}",
                String::from_utf8_lossy(bytes)
            );
        }
    }

    #[test]
    fn a_zero_polling_interval_is_rejected_by_both_entry_paths() {
        // The file path and a caller constructing settings directly now fail on
        // the same rule, rather than one of them reaching `tokio::time::interval`.
        assert!(RuntimeConfig::parse(br#"{"session":{"currentStateIntervalMs":0}}"#).is_err());
        assert_eq!(
            SessionSettings::new(
                Duration::from_secs(10),
                Duration::from_secs(5),
                Duration::ZERO,
            )
            .unwrap_err(),
            InvalidSessionSettings::CurrentStateInterval
        );
        assert_eq!(
            RuntimeConfig::parse(br#"{"session":{"currentStateIntervalMs":250}}"#)
                .unwrap()
                .session()
                .unwrap()
                .current_state_interval(),
            Duration::from_millis(250)
        );
    }
}

#[cfg(test)]
#[path = "../../tests/composition/runtime_config.rs"]
mod ssh_hosts_tests;
