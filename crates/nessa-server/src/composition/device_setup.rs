//! What Devices sets SSH hosts up with: where the person's SSH
//! configuration is, and [`HostInstall`] over the same installer, OpenSSH
//! client and environment audit a conversation's first lease on a host
//! uses (`local_auth::environments`), so connecting a host from Devices
//! and a lease's first use of it install it the same way and leave the same
//! records.
use crate::conversation::infrastructure::ssh_environment::{
    DurableEnvironmentAudit, HostInstaller, OpenSshConnector,
};
use crate::core::RunError;
use crate::devices::application::{HostInstall, SetupFuture};
use nessa_sdk::domain::agent_execution::leases::{LeaseRefusal, SshDestination};
use std::{path::Path, path::PathBuf, sync::Arc};

/// The home directory the gateway runs under, whose `.ssh/config` names the
/// person's machines: `HOME`, or the root when it is not set.
pub(super) fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

/// This build installed on a host over OpenSSH, recorded in the
/// conversations' environment audit.
pub(super) struct SshInstall {
    installer: HostInstaller,
    audit: Arc<DurableEnvironmentAudit>,
}

impl SshInstall {
    /// Kept beside the conversations under `root`: release builds in
    /// `host-builds`, records in `audit/environments`.
    pub(super) fn new(root: &Path) -> Result<Self, RunError> {
        let audit = DurableEnvironmentAudit::new(
            root.join("audit").join("environments"),
            Arc::new(super::local_auth::SystemClock),
        )
        .map_err(|error| RunError::Agent(error.to_string()))?;
        let installer =
            HostInstaller::this_build(Arc::new(OpenSshConnector::new()), root.join("host-builds"));
        #[cfg(all(test, unix))]
        let installer = super::vm_background_gateway::installer(installer);
        Ok(Self { installer, audit })
    }
}

impl HostInstall for SshInstall {
    fn install<'a>(
        &'a self,
        host: &'a SshDestination,
        operation: &'a str,
    ) -> SetupFuture<'a, Result<(), LeaseRefusal>> {
        Box::pin(async move {
            self.installer
                .ensure(host, operation, self.audit.as_ref())
                .await
                .map(|_| ())
        })
    }
}
