//! Substitutes for what the resource ticket store reads from outside — the
//! clock and the random source — and a recorder of the ends it reports.
use super::{ResourceTicketStore, TicketEvent, TicketEvents, TokenSource};
use crate::conversation::application::{HeldResource, McpAppRef};
use crate::conversation::domain::ConversationId;
use nessa_auth::application::ports::Clock;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};

pub(crate) const CONVERSATION: &str = "6a1c2d3e-0000-4000-8000-000000000001";
pub(crate) const OTHER_CONVERSATION: &str = "6a1c2d3e-0000-4000-8000-000000000002";

pub(crate) fn conversation(id: &str) -> ConversationId {
    ConversationId::new(id).unwrap()
}

/// One mount of the app of tool call `execution`.
pub(crate) fn app(execution: &str, instance: &str) -> McpAppRef {
    McpAppRef {
        execution_id: execution.into(),
        tool_id: "show_chart".into(),
        instance_id: instance.into(),
    }
}

pub(crate) fn held(conversation_id: &str, app: McpAppRef, bytes: &[u8]) -> HeldResource {
    HeldResource {
        conversation_id: conversation(conversation_id),
        app,
        bytes: Arc::from(bytes),
    }
}

/// Time that moves only when a test says so.
#[derive(Default)]
pub(crate) struct ManualClock(AtomicU64);
impl ManualClock {
    pub(crate) fn at(milliseconds: u64) -> Arc<Self> {
        Arc::new(Self(AtomicU64::new(milliseconds)))
    }
    pub(crate) fn advance(&self, milliseconds: u64) {
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
pub(crate) struct ScriptedRandom {
    next: AtomicU64,
    repeat: bool,
    failing: AtomicBool,
}
impl ScriptedRandom {
    pub(crate) fn counting() -> Arc<Self> {
        Arc::new(Self {
            next: AtomicU64::new(1),
            repeat: false,
            failing: AtomicBool::new(false),
        })
    }
    pub(crate) fn repeating() -> Arc<Self> {
        Arc::new(Self {
            next: AtomicU64::new(7),
            repeat: true,
            failing: AtomicBool::new(false),
        })
    }
    pub(crate) fn fail(&self, failing: bool) {
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
pub(crate) struct RecordedEnds(Mutex<Vec<TicketEvent>>);
impl RecordedEnds {
    pub(crate) fn take(&self) -> Vec<TicketEvent> {
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
pub(crate) struct Fixture {
    pub(crate) clock: Arc<ManualClock>,
    pub(crate) random: Arc<ScriptedRandom>,
    pub(crate) ends: Arc<RecordedEnds>,
    pub(crate) store: Arc<ResourceTicketStore>,
}
impl Fixture {
    pub(crate) fn new() -> Self {
        Self::with_random(ScriptedRandom::counting())
    }
    pub(crate) fn with_random(random: Arc<ScriptedRandom>) -> Self {
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
        }
    }
}
