//! The gateway's own evidence about its SSH environments: each connection,
//! each refusal by version or business, each connection lost, and each frame
//! dropped because it named no lease or harness the gateway holds (row L9).
//! One immutable JSON file per record, like the gateway's other audits.
use nessa_local_storage::{create_directory, sync_directory, PrivateTempFile};
use serde::Serialize;
use std::{io, io::Write, path::PathBuf, sync::Arc};
use uuid::Uuid;

/// One record of what happened between this gateway and a host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum EnvironmentEvent {
    /// A host answered with this build's hello.
    Connected { host: String, workspace: String },
    /// A host answered with another build's hello, or none: refused.
    VersionRefused { host: String, build: Option<String> },
    /// A host is serving another connection.
    Busy { host: String },
    /// A host could not serve leases at all: its configuration.
    NotConfigured { host: String },
    /// A connection ended, ending every lease it carried.
    ConnectionLost { host: String, leases: Vec<String> },
    /// A frame named a lease or harness this connection does not hold, or
    /// was not one this build reads, and was dropped.
    FrameDropped {
        host: String,
        lease: Option<String>,
        channel: Option<u32>,
        frame: String,
    },
    /// A harness wrote more than the gateway queues for its binding; its
    /// output was ended and the harness stopped.
    OutputOverflow {
        host: String,
        lease: String,
        channel: u32,
    },
}

/// Where those records go.
pub(crate) trait EnvironmentAudit: Send + Sync {
    /// Record `event` durably.
    ///
    /// # Errors
    /// The record is not durable.
    fn record(&self, event: &EnvironmentEvent) -> io::Result<()>;
}

/// The durable audit, a directory of one file per record.
pub(crate) struct DurableEnvironmentAudit {
    directory: PathBuf,
}

impl DurableEnvironmentAudit {
    /// The audit kept in `directory`, created if absent.
    ///
    /// # Errors
    /// The directory could not be created privately.
    pub(crate) fn new(directory: PathBuf) -> io::Result<Arc<Self>> {
        create_directory(&directory)?;
        Ok(Arc::new(Self { directory }))
    }
}

impl EnvironmentAudit for DurableEnvironmentAudit {
    fn record(&self, event: &EnvironmentEvent) -> io::Result<()> {
        let mut file = PrivateTempFile::new_in(&self.directory)?;
        serde_json::to_writer(file.as_file_mut(), event).map_err(io::Error::other)?;
        file.as_file_mut().write_all(b"\n")?;
        file.as_file().sync_all()?;
        file.persist(&self.directory.join(format!("{}.json", Uuid::new_v4())))?;
        sync_directory(&self.directory)
    }
}
