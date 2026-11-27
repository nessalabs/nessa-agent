//! Where each send came from, through the real SDK Agent: the origin record
//! written before admission (and the send refused when it cannot be), and the
//! turn's span the same send starts.
use super::{
    ConversationCaller, ConversationDependencies, ConversationError, ConversationLimits,
    ConversationService, ProviderSessionErasers, SentFrom, SubmissionMode, SubmittedMessage,
    TurnOrigin,
};
use crate::application::turn_telemetry::turn_span;
use crate::application::{ConversationAgent, ConversationAgents};
use crate::conversation_test_support::{
    AcceptingAudit, AcceptingCreationAudit, AcceptingDeletionAudit, MemoryRepository,
    MemorySummaries, Provider, ProviderFactory, RecordingFileLinkAudit, RecordingTurnOriginAudit,
    TestClock, Unlisted, DELETION_BUDGETS,
};
use crate::infrastructure::ObservedExecutionAudit;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::{domain::ConversationId, view::AUTO_PICK_SURFACE};
use nessa_sdk::infrastructure::session_storage::InMemoryStorage;
use nessa_telemetry::{Event, Shape, Sink, Telemetry, Value};
use std::collections::HashMap;
use std::sync::{atomic::Ordering, Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Default)]
struct Recording(Arc<Mutex<Vec<Event>>>);
impl Sink for Recording {
    fn emit(&self, event: &Event) {
        self.0.lock().unwrap().push(event.clone());
    }
}

fn caller(surface: &str, action: &str) -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: surface.into(),
        action_id: action.into(),
    }
}

struct Fixture {
    service: ConversationService,
    origins: Arc<RecordingTurnOriginAudit>,
    telemetry: Telemetry,
    told: Recording,
    id: ConversationId,
}

async fn conversation() -> Fixture {
    let provider = Arc::new(ProviderFactory::default());
    provider.image_input.store(false, Ordering::SeqCst);
    let origins = Arc::new(RecordingTurnOriginAudit::default());
    let told = Recording::default();
    let telemetry = Telemetry::new(vec![Box::new(told.clone())], 64).unwrap();
    // The agent's execution audit is told as the turn's span, as the
    // gateway composes it, and the provider audits through it too.
    let audit: Arc<dyn nessa_sdk::application::agent_execution::executions::ExecutionAudit> =
        Arc::new(ObservedExecutionAudit::new(
            Arc::new(AcceptingAudit),
            telemetry.clone(),
            Arc::new(TestClock),
        ));
    *provider.finish_audit.lock().unwrap() = Some(audit.clone());
    let agents = ConversationAgents::new(
        HashMap::from([(
            AgentId::Claude,
            ConversationAgent {
                workspace: None,
                provider: Arc::new(Provider::new(provider.clone())),
                execution_audit: audit,
                reserved_output_tokens: 4096,
                readiness: None,
                sandbox:
                    nessa_sdk::domain::agent_execution::leases::SandboxProfiles::HARNESS_DEFAULT,
            },
        )]),
        AgentId::Claude,
    )
    .unwrap();
    let service = ConversationService::new(
        ConversationDependencies {
            input_files: None,
            access_policy: crate::conversation_test_support::access_policy(),
            agents,
            storage: Arc::new(InMemoryStorage::new()),
            metadata: Arc::new(MemoryRepository::default()),
            mode_audit: Arc::new(crate::conversation_test_support::AcceptingModeAudit),
            creation_audit: Arc::new(AcceptingCreationAudit),
            turn_origin_audit: origins.clone(),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            summaries: Arc::new(MemorySummaries::default()),
            listing: Arc::new(Unlisted),
            deletion_audit: Arc::new(AcceptingDeletionAudit),
            provider_sessions: ProviderSessionErasers::default(),
            deletion_budgets: DELETION_BUDGETS,
            message_commit_clock: Arc::new(
                nessa_sdk::infrastructure::session_storage::RuntimeMessageCommitClock::new(),
            ),
            clock: Arc::new(TestClock),
            environment: crate::infrastructure::in_process_environment().into(),
        },
        ConversationLimits::default(),
        Some("/workspace".into()),
    )
    .unwrap();
    service.bind_telemetry(telemetry.clone());
    let id = ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap();
    service
        .create(
            id.clone(),
            caller("panel", "create"),
            crate::application::RequestedConversation::default(),
        )
        .await
        .unwrap();
    Fixture {
        service,
        origins,
        telemetry,
        told,
        id,
    }
}

fn message(text: &str, sent_from: Option<SentFrom>) -> SubmittedMessage {
    SubmittedMessage {
        sent_from,
        uploads: Vec::new(),
        text: text.into(),
        images: Vec::new(),
        files: Vec::new(),
    }
}

impl Fixture {
    async fn send(
        &self,
        surface: &str,
        action: &str,
        execution: &str,
        sent_from: Option<SentFrom>,
    ) -> Result<(), ConversationError> {
        self.service
            .submit(
                self.id.clone(),
                caller(surface, action),
                execution.into(),
                message("a private message", sent_from),
                SubmissionMode::Queue,
            )
            .await
            .map(|_| ())
    }

    fn told(&self) -> Vec<Event> {
        assert!(self.telemetry.flush(Duration::from_secs(5)));
        self.told.0.lock().unwrap().clone()
    }
}

fn attribute<'a>(event: &'a Event, key: &str) -> Option<&'a Value> {
    event
        .attributes()
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, value)| value)
}

#[tokio::test]
async fn a_fresh_send_records_where_it_came_from_before_its_span_starts() {
    let fixture = conversation().await;
    let browser = SentFrom {
        client: "nessa-browser".into(),
        spoken: true,
    };
    fixture
        .send("panel", "request-1", "e1", Some(browser.clone()))
        .await
        .unwrap();

    let records = fixture.origins.records.lock().unwrap().clone();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.conversation_id, fixture.id);
    assert_eq!(record.organization_id.as_str(), "org");
    assert_eq!(record.execution_id, "e1");
    assert_eq!(record.request_id, "request-1");
    assert!(
        matches!(record.origin, Some(TurnOrigin::Person { .. })),
        "{:?}",
        record.origin
    );
    assert_eq!(record.sent_from, Some(browser));
    assert_eq!(record.initiator_principal_id.as_str(), "person");
    assert_eq!(record.initiator_surface_id, "panel");

    // The turn's span started, said so, and was not ended as refused.
    let told = fixture.told();
    let started = told
        .iter()
        .find(|event| matches!(event.shape(), Shape::SpanStart { .. }))
        .expect("the turn's span started");
    assert_eq!(
        attribute(started, "nessa.origin.kind"),
        Some(&"person".into())
    );
    assert_eq!(
        attribute(started, "nessa.client"),
        Some(&"nessa-browser".into())
    );
    assert_eq!(attribute(started, "nessa.spoken"), Some(&true.into()));
    assert_eq!(
        attribute(started, "nessa.turn.execution_id"),
        Some(&"e1".into())
    );
    assert!(told
        .iter()
        .all(|event| attribute(event, "nessa.turn.outcome") != Some(&"refused".into())));
    assert!(!format!("{told:?}").contains("a private message"));

    // The same attempt sent again once the agent has it records nothing new.
    fixture
        .send("panel", "request-1", "e1", None)
        .await
        .unwrap();
    assert_eq!(fixture.origins.records.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_turn_that_runs_is_one_span_from_its_send_to_the_agent_finishing_it() {
    let fixture = conversation().await;
    fixture
        .send("panel", "request-1", "e1", None)
        .await
        .unwrap();
    // The SDK's own `Finished` record ends it. It names the provider's
    // session, not the conversation, which the audit found from the turn's
    // admission.
    let span = turn_span(&fixture.id.to_string(), "e1");
    let mut told = Vec::new();
    for _ in 0..100 {
        told = fixture.told();
        if told
            .iter()
            .any(|event| event.shape() == &Shape::SpanEnd { span: span.clone() })
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let within: Vec<_> = told
        .iter()
        .filter(|event| match event.shape() {
            Shape::SpanStart { span: started, .. } | Shape::SpanEnd { span: started } => {
                started == &span
            }
            Shape::Point { within } => within.as_ref() == Some(&span),
            _ => false,
        })
        .map(|event| event.name().to_owned())
        .collect();
    assert_eq!(
        within,
        ["nessa.turn", "nessa.turn.admitted", "nessa.turn"],
        "{told:?}"
    );
    let ended = told
        .iter()
        .find(|event| event.shape() == &Shape::SpanEnd { span: span.clone() })
        .unwrap();
    assert_eq!(
        attribute(ended, "nessa.turn.outcome"),
        Some(&"completed".into())
    );
    assert_eq!(
        attribute(ended, "nessa.turn.cause"),
        Some(&"finished".into())
    );
}

#[tokio::test]
async fn auto_pick_and_an_event_are_recorded_as_themselves_not_as_the_person() {
    let fixture = conversation().await;
    fixture
        .send(AUTO_PICK_SURFACE, "pick", "e1", None)
        .await
        .unwrap();
    fixture
        .send("event:github", "hook", "e2", None)
        .await
        .unwrap();
    let records = fixture.origins.records.lock().unwrap().clone();
    let origin = |execution: &str| {
        records
            .iter()
            .find(|record| record.execution_id == execution)
            .and_then(|record| record.origin.clone())
    };
    assert_eq!(origin("e1"), Some(TurnOrigin::AutoPick));
    assert_eq!(
        origin("e2"),
        Some(TurnOrigin::Event {
            source: "github".into()
        })
    );
}

#[tokio::test]
async fn an_origin_that_cannot_be_recorded_refuses_the_send_before_anything_is_told() {
    let fixture = conversation().await;
    fixture.origins.refuses.store(true, Ordering::SeqCst);
    assert!(matches!(
        fixture.send("panel", "request-1", "e1", None).await,
        Err(ConversationError::Audit)
    ));
    let view = fixture
        .service
        .read(fixture.id.clone(), caller("panel", "read"))
        .await
        .unwrap();
    assert!(view
        .messages
        .iter()
        .all(|message| message.execution_id != "e1"));
    assert!(view
        .pending
        .iter()
        .all(|pending| pending.execution_id != "e1"));
    assert!(fixture
        .told()
        .iter()
        .all(|event| attribute(event, "nessa.turn.execution_id") != Some(&"e1".into())));
}

/// A span stays open for whatever may still run the turn, waits for the next
/// offer when the agent is refusing for now, and ends as refused only when
/// nothing was taken.
#[test]
fn a_span_ends_as_refused_only_when_nothing_was_taken() {
    use super::service::{span_after, SpanAfter};
    use nessa_sdk::application::agent_execution::agents::AgentError;
    assert_eq!(span_after(Ok(()), false), SpanAfter::Open);
    let closed = ConversationError::Agent(AgentError::Closed);
    assert_eq!(span_after(Err(&closed), true), SpanAfter::OfferedAgain);
    assert_eq!(span_after(Err(&closed), false), SpanAfter::Refused);
    for saved in [
        ConversationError::RevokedAfterSave,
        ConversationError::UnavailableAfterSave,
        ConversationError::Agent(AgentError::SubmissionUnresolved),
    ] {
        assert_eq!(span_after(Err(&saved), false), SpanAfter::Open, "{saved:?}");
    }
}
