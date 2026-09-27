use crate::shell::application::{Audit, Evidence, RunRequest, Task};
use crate::shell::domain::{ToolInitiator, ToolRequestId};
use nessa_local_storage::{create_directory, sync_directory, PrivateTempFile};
use serde_json::json;
use std::{
    io::Write,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

pub struct PrivateAudit {
    directory: PathBuf,
}
impl PrivateAudit {
    pub fn new(directory: PathBuf) -> std::io::Result<Self> {
        create_directory(&directory)?;
        Ok(Self { directory })
    }
}
impl Audit for PrivateAudit {
    fn record<'a>(
        &'a self,
        request: &'a RunRequest,
        evidence: Evidence,
    ) -> Task<'a, Result<(), ()>> {
        let directory = self.directory.clone();
        let request = request.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || -> std::io::Result<()> {
                let initiator = match request.invocation.initiator() {
                    ToolInitiator::ConfiguredProviderToolCall => "configured_provider_tool_call",
                };
                let (event, state) = match evidence {
                    Evidence::Admitted => ("admitted", json!({"initiator":initiator})),
                    Evidence::Started { scope_id, process_id, os_pid } => ("started", json!({"scopeId":scope_id,"processId":process_id,"osPid":os_pid})),
                    Evidence::Finished(r) => ("finished", json!({"cause":format!("{:?}", r.cause),"scopeId":r.scope_id,"processId":r.process_id,"osPid":r.os_pid,"exitCode":r.exit_code,"exitSignal":r.exit_signal,"forced":r.forced,"stdout":r.stdout,"stderr":r.stderr,"droppedBytes":r.dropped_bytes,"cleanupVerified":r.cleanup_verified,"cleanupError":r.cleanup_error,"outputErrors":r.output_errors,"auditError":r.audit_error})),
                };
                let request_id = match request.invocation.request_id() {
                    ToolRequestId::Signed(value) => json!(value),
                    ToolRequestId::Unsigned(value) => json!(value),
                    ToolRequestId::Text(value) => json!(value),
                };
                let correlation = json!({"connectionId":request.invocation.connection_id(),"requestId":request_id,"initiator":initiator});
                let value = json!({"commandId":request.id,"correlation":correlation,"command":request.command.text(),"cwd":request.cwd,"timeoutSeconds":request.command.timeout().as_secs(),"event":event,"observedAtMs":SystemTime::now().duration_since(UNIX_EPOCH).map_err(std::io::Error::other)?.as_millis(),"state":state});
                let mut file = PrivateTempFile::new_in(&directory)?;
                serde_json::to_writer(file.as_file_mut(), &value)?;
                file.as_file_mut().write_all(b"\n")?;
                file.as_file().sync_all()?;
                file.persist(&directory.join(format!("{}-{}-{}.json", request.id, event, Uuid::new_v4())))?;
                sync_directory(&directory)
            }).await.map_err(|_| ())?.map_err(|_| ())
        })
    }
}
