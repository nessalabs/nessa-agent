//! The durable [`EnvironmentAudit`]: one immutable JSON file per record in
//! the gateway's audit directory, with when it was observed. Composed only by
//! the Unix gateway (`composition/local_auth.rs`).
use super::audit::{EnvironmentAudit, EnvironmentEvent};
use nessa_auth::application::ports::Clock;
use nessa_local_storage::{create_directory, sync_directory, PrivateTempFile};
use serde::Serialize;
use std::{io, io::Write, path::PathBuf, sync::Arc};
use uuid::Uuid;

/// The durable audit, a directory of one file per record.
pub(crate) struct DurableEnvironmentAudit {
    directory: PathBuf,
    clock: Arc<dyn Clock>,
}

/// One record as written: the event and when it was observed.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Recorded<'a> {
    observed_at_ms: u64,
    #[serde(flatten)]
    event: &'a EnvironmentEvent,
}

impl DurableEnvironmentAudit {
    /// The audit kept in `directory`, created if absent.
    ///
    /// # Errors
    /// The directory could not be created privately.
    pub(crate) fn new(directory: PathBuf, clock: Arc<dyn Clock>) -> io::Result<Arc<Self>> {
        create_directory(&directory)?;
        Ok(Arc::new(Self { directory, clock }))
    }
}

impl EnvironmentAudit for DurableEnvironmentAudit {
    fn record(&self, event: &EnvironmentEvent) -> io::Result<()> {
        let mut file = PrivateTempFile::new_in(&self.directory)?;
        let recorded = Recorded {
            observed_at_ms: self.clock.unix_milliseconds(),
            event,
        };
        serde_json::to_writer(file.as_file_mut(), &recorded).map_err(io::Error::other)?;
        file.as_file_mut().write_all(b"\n")?;
        file.as_file().sync_all()?;
        file.persist(&self.directory.join(format!("{}.json", Uuid::new_v4())))?;
        sync_directory(&self.directory)
    }
}
