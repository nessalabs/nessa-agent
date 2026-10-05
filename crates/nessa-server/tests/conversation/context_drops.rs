//! The one recorder of held MCP App contexts dropped unsent (#390,
//! `docs/design/mcp-app-calls.md`, rows C15 and C15b): every drop reported
//! before the gateway stops is written before the recorder returns, in the
//! order reported; a drop whose record fails is logged by its conversation
//! and call, and the next is still written.
use super::*;
use crate::conversation::application::{
    ContextDrop, ConversationError, ConversationFuture, McpAppAsk, McpAppAuditPhase,
    McpAppInitiator, McpAppRef,
};
use nessa_auth::domain::OrganizationId;
use nessa_protocol::conversation::domain::ConversationId;
use std::sync::{Mutex, PoisonError};

/// An audit that keeps what it commits, and fails the record of every call
/// whose id begins `lost`.
#[derive(Default)]
struct Kept {
    records: Mutex<Vec<McpAppAuditRecord>>,
}
impl McpAppAudit for Kept {
    fn record(&self, record: McpAppAuditRecord) -> ConversationFuture<'_, ()> {
        let failing = record.call_id.starts_with("lost");
        Box::pin(async move {
            if failing {
                return Err(ConversationError::Audit);
            }
            self.records.lock().unwrap().push(record);
            Ok(())
        })
    }
}
impl Kept {
    fn calls(&self) -> Vec<String> {
        self.records
            .lock()
            .unwrap()
            .iter()
            .map(|record| record.call_id.clone())
            .collect()
    }
}

/// The drop of the update `call` held.
fn dropped(call: &str) -> McpAppAuditRecord {
    McpAppAuditRecord {
        conversation_id: ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap(),
        organization_id: OrganizationId::new("org").unwrap(),
        call_id: call.into(),
        request_id: format!("request-{call}"),
        app: McpAppRef {
            execution_id: "e1".into(),
            tool_id: "t1".into(),
            instance_id: "i1".into(),
        },
        ask: McpAppAsk::UpdateModelContext {
            server: "charts".into(),
        },
        initiator: McpAppInitiator::System,
        phase: McpAppAuditPhase::ContextDropped {
            cause: ContextDrop::ConversationEnded,
        },
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_stopping_recorder_writes_every_drop_already_reported_in_order() {
    let captured = Capture(Arc::new(Mutex::new(Vec::new())));
    let subscriber = tracing_subscriber::fmt()
        .with_writer(captured.clone())
        .with_ansi(false)
        .finish();
    let _logging = tracing::subscriber::set_default(subscriber);
    let (sender, drops) = tokio::sync::mpsc::unbounded_channel();
    let audit = Arc::new(Kept::default());
    let (stop, stopping) = tokio::sync::oneshot::channel();
    // The conversations end, reporting their drops, then the gateway stops —
    // before the recorder has had a turn. The sender lives on.
    for call in ["u1", "u2", "u3"] {
        sender.context_dropped(dropped(call));
    }
    let _ = stop.send(());
    audit_context_drops(drops, audit.clone(), stopping).await;
    assert_eq!(audit.calls(), ["u1", "u2", "u3"]);
    // A drop reported once it stopped has no one to write it, and says so,
    // by its conversation and its call.
    sender.context_dropped(dropped("u4"));
    assert_eq!(audit.calls().len(), 3);
    let logged = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(
        logged.contains("an MCP App context's drop could not be audited: nothing receives it"),
        "{logged}"
    );
    assert!(
        logged.contains("call_id=\"u4\"") || logged.contains("call_id=u4"),
        "{logged}"
    );
    assert!(
        logged.contains("00000000-0000-4000-8000-000000000001"),
        "{logged}"
    );
}

#[tokio::test]
async fn a_recorder_ends_once_every_sender_is_gone_having_written_what_they_sent() {
    let (sender, drops) = tokio::sync::mpsc::unbounded_channel();
    let audit = Arc::new(Kept::default());
    let (_running, stopping) = tokio::sync::oneshot::channel();
    let recording = tokio::spawn(audit_context_drops(drops, audit.clone(), stopping));
    sender.context_dropped(dropped("u1"));
    drop(sender);
    recording.await.unwrap();
    assert_eq!(audit.calls(), ["u1"]);
}

#[derive(Clone)]
struct Capture(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Capture {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buffer);
        Ok(buffer.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_drop_whose_record_fails_is_logged_by_its_call_and_the_next_is_written() {
    let captured = Capture(Arc::new(Mutex::new(Vec::new())));
    let subscriber = tracing_subscriber::fmt()
        .with_writer(captured.clone())
        .with_ansi(false)
        .finish();
    let _logging = tracing::subscriber::set_default(subscriber);
    let (sender, drops) = tokio::sync::mpsc::unbounded_channel();
    let audit = Arc::new(Kept::default());
    let (stop, stopping) = tokio::sync::oneshot::channel();
    sender.context_dropped(dropped("lost"));
    sender.context_dropped(dropped("kept"));
    let _ = stop.send(());
    // Run here, on this thread, so its log reaches the capture; a failure
    // that panicked would fail the test here.
    audit_context_drops(drops, audit.clone(), stopping).await;
    // The next is written all the same.
    assert_eq!(audit.calls(), ["kept"]);
    let logged = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(
        logged.contains("an MCP App context's drop could not be audited"),
        "{logged}"
    );
    assert!(
        logged.contains("call_id=\"lost\"") || logged.contains("call_id=lost"),
        "{logged}"
    );
    assert!(
        logged.contains("00000000-0000-4000-8000-000000000001"),
        "{logged}"
    );
    assert!(!logged.contains("kept"), "{logged}");
}
