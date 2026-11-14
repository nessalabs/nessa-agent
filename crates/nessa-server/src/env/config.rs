//! Env var names, default values, and build-time version for [`super::environment`].

/// Process environment variable names (`NESSA_*`).
pub mod key {
    pub const DATA_DIR: &str = "NESSA_DATA_DIR";
    pub const INSTANCE: &str = "NESSA_INSTANCE";
    pub const HOME: &str = "HOME";
    pub const UPTIME_BACKEND: &str = "NESSA_UPTIME_BACKEND";
    pub const UPTIME_FIXED_MS: &str = "NESSA_UPTIME_FIXED_MS";
    pub const STAGE: &str = "NESSA_STAGE";
    pub const HOST: &str = "NESSA_HOST";
    pub const PORT: &str = "NESSA_PORT";
    /// The launchd service generation the desktop host registered a managed
    /// gateway under. Set only in that plist, so its presence is what tells a
    /// process it is the desktop's background service rather than a hand-run one.
    pub const SERVICE_GENERATION: &str = "NESSA_SERVICE_GENERATION";
}

/// Fallback values when a var is unset.
///
/// The port is not here: it depends on the stage, and its one table lives in
/// `protocol/defaults/gateway-ports.json` behind [`super::stage_port`].
pub mod default {
    pub const HOST: &str = "127.0.0.1";
}

/// Crate version from `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The standalone `nessa` a release publishes for each target, built from
/// these same sources just before this build (#734), as
/// `<target>=<sha256>:<bytes>`, comma-separated. Set by the release
/// workflow when it compiles a release; `None` in every other build, which
/// so installs only its own executable on a host.
pub const HOST_BUILDS: Option<&str> = option_env!("NESSA_HOST_BUILDS");

/// Where [`HOST_BUILDS`] are published: the release's download address,
/// under which each is `nessa-<target>`. Set beside them. A local desktop
/// build sets [`BUNDLED_HOST_BUILDS`] instead: its builds ship inside the
/// app, beside this executable.
pub const HOST_BUILDS_FROM: Option<&str> = option_env!("NESSA_HOST_BUILDS_FROM");

/// The [`HOST_BUILDS_FROM`] that names the `host-builds` folder beside this
/// executable rather than a download address
/// (`scripts/desktop/prepare-runtime.mjs` puts them there).
pub const BUNDLED_HOST_BUILDS: &str = "bundled";

/// Name the lease protocol by the sources that define it, listed once: the
/// names for checking the list, the fingerprint for the hello.
macro_rules! lease_protocol {
    ($($source:literal),+ $(,)?) => {
        /// The sources [`LEASE_PROTOCOL`] is taken over, relative to this file.
        #[cfg(all(test, unix))]
        pub(crate) const LEASE_PROTOCOL_SOURCES: &[&str] = &[$($source),+];
        // One step per byte of the sources: as long as they are, and
        // never more. The lint is for loops that might not end.
        #[allow(long_running_const_eval)]
        const LEASE_PROTOCOL_DIGITS: [u8; 16] =
            nessa_protocol::lease::fingerprint(&[$(include_bytes!($source) as &[u8]),+]);
    };
}

// The frames, their framing, both ends that write, read and act on them, and
// what the host's answers mean: how it launches a harness (with the variables
// each agent's binding may set), runs a command and which commands it takes,
// records leases, and cleans a harness up.
// Every source naming a lease frame, every source of `env serve`, and every
// binding declaring launch variables is here
// (`every_source_of_the_lease_contract_names_the_protocol`).
lease_protocol!(
    "../../../nessa-protocol/src/lease.rs",
    "../../../nessa-protocol/src/pairing/frames.rs",
    "../../../nessa-protocol/src/build_update.rs",
    "../../../nessa-env-serve/src/lib.rs",
    "../../../nessa-env-serve/src/application/mod.rs",
    "../../../nessa-env-serve/src/application/serve.rs",
    "../../../nessa-env-serve/src/application/artifacts.rs",
    "../../../nessa-env-serve/src/application/fetches.rs",
    "../../../nessa-env-serve/src/application/wire.rs",
    "../../../nessa-env-serve/src/application/redial.rs",
    "../../../nessa-env-serve/src/application/approval.rs",
    "../../../nessa-env-serve/src/application/enrollment.rs",
    "../../../nessa-env-serve/src/application/grants.rs",
    "../../../nessa-env-serve/src/application/lease_policy.rs",
    "../timer/mod.rs",
    "../../../nessa-env-serve/src/domain/mod.rs",
    "../../../nessa-env-serve/src/domain/grants.rs",
    "../../../nessa-env-serve/src/infrastructure/mod.rs",
    "../../../nessa-env-serve/src/infrastructure/commands.rs",
    "../../../nessa-env-serve/src/infrastructure/launcher.rs",
    "../../../nessa-env-serve/src/infrastructure/ledger.rs",
    "../../../nessa-env-serve/src/infrastructure/lock.rs",
    "../../../nessa-env-serve/src/infrastructure/outbox.rs",
    "../../../nessa-env-serve/src/infrastructure/inbox.rs",
    "../../../nessa-env-serve/src/infrastructure/dial.rs",
    "../../../nessa-env-serve/src/infrastructure/pairing.rs",
    "../../../nessa-env-serve/src/infrastructure/self_update.rs",
    "../../../nessa-env-serve/src/infrastructure/grant_audit.rs",
    "../../../nessa-env-serve/src/infrastructure/grants.rs",
    "../../../nessa-env-serve/src/infrastructure/cedar/mod.rs",
    "../../../nessa-env-serve/src/infrastructure/cedar/evaluator.rs",
    "../../../nessa-env-serve/src/infrastructure/cedar/schema.json",
    "../../../nessa-env-serve/src/infrastructure/cedar/policies.cedar",
    "../../../nessa-env-serve/src/machine.rs",
    "../devices/infrastructure/machine.rs",
    "../../../nessa-env-serve/src/install.rs",
    "../conversation/infrastructure/ssh_environment/link.rs",
    "../conversation/infrastructure/ssh_environment/environment.rs",
    "../conversation/infrastructure/ssh_environment/transfer.rs",
    "../conversation/infrastructure/ssh_environment/sftp.rs",
    "../../../nessa-sdk/src/infrastructure/claude_acp/sessions/binding.rs",
    "../../../nessa-sdk/src/infrastructure/codex_acp/sessions/binding.rs",
    "../../../nessa-sdk/src/infrastructure/opencode_acp/sessions/binding.rs",
    "../../../nessa-sdk/src/infrastructure/harness_process.rs",
    "../../../nessa-sdk/src/infrastructure/process.rs",
    "../../../nessa-sdk/src/domain/agent_execution/leases/value_objects.rs",
    "../../../nessa-types/src/lib.rs",
);

/// The lease protocol this build speaks: a fingerprint of its sources, taken
/// when they are compiled. Any change to the frames or to how either end
/// reads them changes it, with nothing to bump; a gateway speaks only to an
/// environment saying the same in its hello. Not [`VERSION`], which every
/// revision shares. A `static`, so the fingerprint is taken once, here: a
/// `const` would be evaluated again in each crate that reads it.
#[allow(long_running_const_eval)]
pub static LEASE_PROTOCOL: &str = match std::str::from_utf8(&LEASE_PROTOCOL_DIGITS) {
    Ok(protocol) => protocol,
    Err(_) => panic!("a fingerprint is hex digits"),
};

#[cfg(all(test, unix))]
#[path = "../../tests/env/lease_sources.rs"]
mod tests;
