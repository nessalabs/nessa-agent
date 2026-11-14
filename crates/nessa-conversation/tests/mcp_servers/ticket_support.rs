//! Substitutes for what the resource ticket store reads from outside — the
//! clock and the random source — a recorder of the ends it reports, and an
//! audit that can be made to fail.
use super::{ResourceTicketStore, TicketEvent, TicketEvents, TokenSource};
use crate::application::{
    ConversationError, ConversationFuture, HeldResource, McpAppAsk, McpAppAudit, McpAppAuditPhase,
    McpAppAuditRecord, McpAppInitiator, McpAppRef, ResourceTickets,
};
use nessa_auth::application::ports::Clock;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::domain::ConversationId;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};

pub const CONVERSATION: &str = "6a1c2d3e-0000-4000-8000-000000000001";
pub const OTHER_CONVERSATION: &str = "6a1c2d3e-0000-4000-8000-000000000002";

pub fn conversation(id: &str) -> ConversationId {
    ConversationId::new(id).unwrap()
}

/// One mount of the app of tool call `execution`.
pub fn app(execution: &str, instance: &str) -> McpAppRef {
    McpAppRef {
        execution_id: execution.into(),
        tool_id: "show_chart".into(),
        instance_id: instance.into(),
    }
}

/// The app, on behalf of the person whose credential it runs under.
pub fn app_initiator() -> McpAppInitiator {
    McpAppInitiator::App {
        principal_id: PrincipalId::new("owner").unwrap(),
        surface_id: "surface-1".into(),
    }
}

/// `bytes`, held for the `mcp.readResource` call `call-<execution>` of the
/// mount `app` in `conversation_id`, as the service last recorded it.
pub fn held(conversation_id: &str, app: McpAppRef, bytes: &[u8]) -> HeldResource {
    HeldResource {
        record: McpAppAuditRecord {
            conversation_id: conversation(conversation_id),
            organization_id: OrganizationId::new("organization").unwrap(),
            call_id: format!("call-{}", app.execution_id),
            request_id: "read-1".into(),
            app,
            ask: McpAppAsk::ReadResource {
                server: "charts".into(),
                uri: "ui://charts/chart.html".into(),
            },
            initiator: app_initiator(),
            phase: McpAppAuditPhase::Admitted,
        },
        bytes: Arc::from(bytes),
    }
}

/// Every record committed, in order; or, while failing, none.
#[derive(Default)]
pub struct RecordingAudit {
    records: Mutex<Vec<McpAppAuditRecord>>,
    failing: AtomicBool,
}
impl RecordingAudit {
    pub fn fail(&self, failing: bool) {
        self.failing.store(failing, Ordering::SeqCst);
    }
    pub fn take(&self) -> Vec<McpAppAuditRecord> {
        std::mem::take(&mut self.records.lock().unwrap())
    }
}
impl McpAppAudit for RecordingAudit {
    fn record(&self, record: McpAppAuditRecord) -> ConversationFuture<'_, ()> {
        Box::pin(async move {
            if self.failing.load(Ordering::SeqCst) {
                return Err(ConversationError::Audit);
            }
            self.records.lock().unwrap().push(record);
            Ok(())
        })
    }
}

/// Time that moves only when a test says so.
#[derive(Default)]
pub struct ManualClock(AtomicU64);
impl ManualClock {
    pub fn at(milliseconds: u64) -> Arc<Self> {
        Arc::new(Self(AtomicU64::new(milliseconds)))
    }
    pub fn advance(&self, milliseconds: u64) {
        self.0.fetch_add(milliseconds, Ordering::SeqCst);
    }
}
impl Clock for ManualClock {
    fn unix_milliseconds(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// Distinct bytes each time, until told to fail; or the same bytes every
/// time, as a broken source would give.
pub struct ScriptedRandom {
    next: AtomicU64,
    repeat: bool,
    failing: AtomicBool,
}
impl ScriptedRandom {
    pub fn counting() -> Arc<Self> {
        Arc::new(Self {
            next: AtomicU64::new(1),
            repeat: false,
            failing: AtomicBool::new(false),
        })
    }
    pub fn repeating() -> Arc<Self> {
        Arc::new(Self {
            next: AtomicU64::new(7),
            repeat: true,
            failing: AtomicBool::new(false),
        })
    }
    pub fn fail(&self, failing: bool) {
        self.failing.store(failing, Ordering::SeqCst);
    }
}
impl TokenSource for ScriptedRandom {
    fn fill(&self, bytes: &mut [u8; 32]) -> Result<(), String> {
        if self.failing.load(Ordering::SeqCst) {
            return Err("no entropy".into());
        }
        let value = if self.repeat {
            self.next.load(Ordering::SeqCst)
        } else {
            self.next.fetch_add(1, Ordering::SeqCst)
        };
        *bytes = [0; 32];
        bytes[..8].copy_from_slice(&value.to_be_bytes());
        Ok(())
    }
}

/// Every end the store reports, in order.
#[derive(Default)]
pub struct RecordedEnds(Mutex<Vec<TicketEvent>>);
impl RecordedEnds {
    pub fn take(&self) -> Vec<TicketEvent> {
        std::mem::take(&mut self.0.lock().unwrap())
    }
}
impl TicketEvents for RecordedEnds {
    fn ticket_ended(&self, event: TicketEvent) {
        self.0.lock().unwrap().push(event);
    }
}

/// A store over a manual clock at `start`, counting random bytes, and a
/// recorder.
/// Issue `resource` on `store` and make its ticket redeemable at once, as
/// the conversation service does once the issue is on record.
pub fn issued(
    store: &ResourceTicketStore,
    resource: HeldResource,
) -> Result<String, crate::application::TicketRefusal> {
    let ticket = store.issue(resource)?;
    store.activate(&ticket).expect("issued just now");
    Ok(ticket)
}

pub struct Fixture {
    pub clock: Arc<ManualClock>,
    pub random: Arc<ScriptedRandom>,
    pub ends: Arc<RecordedEnds>,
    pub store: Arc<ResourceTicketStore>,
    /// What the route records redemptions in.
    pub audit: Arc<RecordingAudit>,
}
impl Default for Fixture {
    fn default() -> Self {
        Self::new()
    }
}
impl Fixture {
    pub fn new() -> Self {
        Self::with_random(ScriptedRandom::counting())
    }
    pub fn with_random(random: Arc<ScriptedRandom>) -> Self {
        let clock = ManualClock::at(1_700_000_000_000);
        let ends = Arc::new(RecordedEnds::default());
        let store = Arc::new(ResourceTicketStore::new(
            clock.clone(),
            random.clone(),
            ends.clone(),
        ));
        Self {
            clock,
            random,
            ends,
            store,
            audit: Arc::new(RecordingAudit::default()),
        }
    }
}
