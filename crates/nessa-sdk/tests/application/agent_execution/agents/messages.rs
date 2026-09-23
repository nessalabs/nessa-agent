//! Actual message bytes are validated independently of caller token estimates,
//! and a message's images are refused by type before anything is accepted.
use super::*;
use nessa_sdk::application::dto::ImageInputLimitsDto;
use nessa_sdk::domain::common::value_objects::{ImageMediaType, Sha256Digest};
use std::sync::OnceLock;

async fn submit(
    agent: &Agent,
    input: ExecutionRequest,
    operation: usize,
) -> Result<ExecutionOutcome, AgentError> {
    match operation {
        0 => agent.invoke(input, actor()).await,
        1 => agent.enqueue(input, actor()).await?.wait().await,
        2 => agent.enqueue_steering(input, actor()).await?.wait().await,
        _ => match agent.steer(input, actor()).await? {
            SteeringDelivery::Queued(receipt) => receipt.wait().await,
            SteeringDelivery::Injected { .. } => panic!("idle fixture cannot inject"),
        },
    }
}

#[tokio::test]
async fn message_byte_limit_precedes_every_admission_save() {
    for operation in 0..4 {
        for oversized in [false, true] {
            let storage = MemoryStorage::default();
            let provider = TestProvider::new();
            let agent = attached_agent(provider.clone(), storage.manager().await)
                .await
                .unwrap();
            let writes = storage.0.lock().unwrap().writes;
            let mut text = "é".repeat(ExecutionRequest::MAX_MESSAGE_BYTES / 2);
            if oversized {
                text.push('x');
            }
            let input = ExecutionRequest {
                user_message: UserMessage::text_only(PromptText::new(text).unwrap()),
                ..request("bytes")
            };
            let result = submit(&agent, input, operation).await;
            if oversized {
                assert!(matches!(result, Err(AgentError::InvalidInput(_))));
                assert_eq!(provider.calls.executions.load(Ordering::SeqCst), 0);
                assert_eq!(storage.0.lock().unwrap().writes, writes);
                assert!(storage.snapshot().invocations.is_empty());
            } else {
                assert_eq!(result, Ok(ExecutionOutcome::Completed));
                assert_eq!(provider.calls.executions.load(Ordering::SeqCst), 1);
                assert_eq!(
                    storage.snapshot().invocations[0]
                        .request
                        .user_message
                        .text_str()
                        .len(),
                    ExecutionRequest::MAX_MESSAGE_BYTES
                );
            }
            agent.close(actor()).await.unwrap();
        }
    }
}

#[tokio::test]
async fn custom_storage_cannot_restore_oversized_input_before_provider_open() {
    let storage = MemoryStorage::default();
    let agent = attached_agent(TestProvider::new(), storage.manager().await)
        .await
        .unwrap();
    agent.invoke(request("saved"), actor()).await.unwrap();
    agent.close(actor()).await.unwrap();
    drop(agent);
    storage
        .0
        .lock()
        .unwrap()
        .snapshot
        .as_mut()
        .unwrap()
        .invocations[0]
        .request
        .user_message = UserMessage::text_only(
        PromptText::new("x".repeat(ExecutionRequest::MAX_MESSAGE_BYTES + 1)).unwrap(),
    );
    let writes = storage.0.lock().unwrap().writes;
    let provider = TestProvider::new();
    assert!(matches!(
        attached_agent(provider.clone(), storage.manager().await).await,
        Err(AgentError::Storage(StorageError::Corrupt(_)))
    ));
    assert!(provider.calls.opens.lock().unwrap().is_empty());
    assert_eq!(storage.0.lock().unwrap().writes, writes);
    assert_eq!(
        storage.snapshot().invocations[0]
            .request
            .user_message
            .text_str()
            .len(),
        ExecutionRequest::MAX_MESSAGE_BYTES + 1
    );
}

#[tokio::test]
async fn an_image_for_a_text_only_binding_is_refused_before_every_admission_save() {
    let image = image(ImageMediaType::Png, 64);
    for operation in 0..4 {
        for text in [Some("look at this"), None] {
            let storage = MemoryStorage::default();
            let provider = TestProvider::new();
            let agent = attached_agent(provider.clone(), storage.manager().await)
                .await
                .unwrap();
            let writes = storage.0.lock().unwrap().writes;
            let input = ExecutionRequest {
                user_message: UserMessage::new(
                    text.map(|text| PromptText::new(text).unwrap()),
                    vec![image],
                    Vec::new(),
                )
                .unwrap(),
                ..request("image")
            };
            // Refused, never sent as its text alone with the image dropped.
            // The attachment itself carries no image, which is its own typed
            // fact rather than a sentence about unmet capabilities.
            let result = submit(&agent, input, operation).await;
            assert_eq!(
                result,
                Err(AgentError::ImageInputRefused(ImageInputRefusal::NotOffered)),
                "operation {operation}"
            );
            assert_eq!(provider.calls.executions.load(Ordering::SeqCst), 0);
            if operation != 3 {
                assert_eq!(storage.0.lock().unwrap().writes, writes);
                assert!(storage.snapshot().invocations.is_empty());
            } else {
                let snapshot = storage.snapshot();
                assert_eq!(snapshot.invocations.len(), 1, "operation {operation}");
                assert_eq!(
                    snapshot.invocations[0].result,
                    Some(result),
                    "operation {operation}"
                );
            }
            agent.close(actor()).await.unwrap();
        }
    }
}

fn image(media_type: ImageMediaType, size: u64) -> ImageReference {
    ImageReference::new(Sha256Digest::from_bytes([3; 32]), media_type, size).unwrap()
}

/// A provider whose model takes PNG images of at most six bytes (eight as
/// base64), whose agent answers `agent` about images, and whose backend
/// refuses every input with `refuses` when that is set.
struct ImageProvider {
    executions: Arc<AtomicUsize>,
    agent: ProviderOperationCapabilities,
    refuses: Option<AgentError>,
}
struct ImageBackend {
    executions: Arc<AtomicUsize>,
    agent: ProviderOperationCapabilities,
    refuses: Option<AgentError>,
    sender: mpsc::UnboundedSender<ExecutionEvent>,
}
impl ImageProvider {
    fn new(agent: ProviderOperationCapabilities, refuses: Option<AgentError>) -> Arc<Self> {
        Arc::new(Self {
            executions: Arc::default(),
            agent,
            refuses,
        })
    }
}
fn image_capabilities() -> EffectiveCapabilities {
    let text = ModalitiesDto {
        text: true,
        image: false,
        audio: false,
    };
    let model = ModelMetadata::try_from(ModelMetadataDto {
        provider: "anthropic".into(),
        model_id: "fixture".into(),
        display_name: "Fixture".into(),
        input: ModalitiesDto {
            image: true,
            ..text
        },
        image_input: Some(ImageInputLimitsDto {
            media_types: vec!["image/png".into()],
            max_encoded_bytes: 8,
            max_edge_px: 8000,
            many_images_max_edge_px: 2000,
            native_long_edge_px: 1568,
        }),
        output: text,
        tool_use: true,
        reasoning: false,
        max_context_window_tokens: 1000,
        max_output_tokens: 100,
        knowledge_cutoff: "2026-01".into(),
        documentation_url: "https://example.com".into(),
    })
    .unwrap();
    let input = Modalities::new(true, true, false).unwrap();
    let output = Modalities::new(true, false, false).unwrap();
    EffectiveCapabilities::new(
        &model,
        BindingRestrictions::new(
            ModelFeatures::new(input, output, true, false),
            model.limits(),
        ),
        model.limits(),
    )
    .unwrap()
}
fn image_capabilities_ref() -> &'static EffectiveCapabilities {
    static CAPABILITIES: OnceLock<EffectiveCapabilities> = OnceLock::new();
    CAPABILITIES.get_or_init(image_capabilities)
}
impl AgentProvider for ImageProvider {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("anthropic", "fixture", "workspace").unwrap()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        image_capabilities_ref()
    }
    fn open(&self, request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        let (restore, _control) = request.into_parts();
        Box::pin(async move {
            let id =
                restore.unwrap_or_else(|| ExecutionSessionId::new("provider-context").unwrap());
            let (sender, receiver) = mpsc::unbounded_channel();
            Ok(OpenedProviderSession {
                session: ProviderSession::new(
                    id,
                    Arc::new(ImageBackend {
                        executions: self.executions.clone(),
                        agent: self.agent,
                        refuses: self.refuses.clone(),
                        sender,
                    }),
                    image_capabilities_ref().clone(),
                ),
                events: Box::new(TestEvents(receiver)),
            })
        })
    }
}
impl ProviderSessionBackend for ImageBackend {
    fn operation_capabilities(&self) -> ProviderOperationCapabilities {
        self.agent
    }
    fn validate_input(&self, _: &ExecutionRequest) -> Result<(), AgentError> {
        self.refuses.clone().map_or(Ok(()), Err)
    }
    fn prepare_invocation(&self) -> ProviderOperationFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
    fn execute(&self, input: ExecutionRequest) -> ProviderExecutionFuture<'_> {
        Box::pin(async move {
            self.executions.fetch_add(1, Ordering::SeqCst);
            let finished = ExecutionEvent::new(
                input.execution_id,
                ExecutionUpdate::Finished(ExecutionOutcome::Completed),
            );
            let result = self
                .sender
                .send(finished)
                .map(|()| ExecutionOutcome::Completed)
                .map_err(|_| AgentError::Backpressure);
            ProviderExecutionReply::Finished(ExecutionReport::new(
                Some(result),
                None,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn answer_permission(
        &self,
        _: PermissionAnswer,
    ) -> ProviderOperationFuture<'_, PermissionResolution> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::StalePermission,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn cancel_permission(
        &self,
        _: PermissionCancellationRequest,
    ) -> ProviderOperationFuture<'_, PermissionCancellation> {
        Box::pin(async {
            Err(ProviderOperationFailure::new(
                AgentError::StalePermission,
                ProviderSessionState::Usable,
            ))
        })
    }
    fn close(&self, _: SessionCloseRequest) -> CleanupFuture<'_> {
        Box::pin(async { CleanupReport::confirmed(CloseOutcome { forced: false }) })
    }
}

const AGENT_TAKES_IMAGES: ProviderOperationCapabilities = ProviderOperationCapabilities {
    negotiated: true,
    native_steering: false,
    session_resume: false,
    image_input: true,
    permission_denial: PermissionDenialCapability::Unknown,
    native_hook_suppression: NativeHookSuppressionCapability::Unknown,
    compaction_reporting: ProviderCompactionReportingCapability::Unknown,
    model_switch_reporting: ProviderModelSwitchReportingCapability::Unknown,
    permission_deferral: ProviderPermissionDeferralCapability::Unknown,
    elicitation_forwarding: ElicitationForwardingCapability::Unknown,
};
const AGENT_TAKES_NO_IMAGES: ProviderOperationCapabilities = ProviderOperationCapabilities {
    image_input: false,
    ..AGENT_TAKES_IMAGES
};
const AGENT_NOT_YET_KNOWN: ProviderOperationCapabilities = ProviderOperationCapabilities {
    negotiated: false,
    ..AGENT_TAKES_NO_IMAGES
};

#[tokio::test]
async fn provider_advertisements_cannot_enable_missing_application_integrations() {
    let provider = ImageProvider::new(
        ProviderOperationCapabilities {
            negotiated: true,
            permission_denial: PermissionDenialCapability::SupportedForOfferedPermissionReviews,
            native_hook_suppression:
                NativeHookSuppressionCapability::SupportedForUserConfiguredHooks,
            compaction_reporting:
                ProviderCompactionReportingCapability::SupportedWithInvocationCorrelation,
            model_switch_reporting:
                ProviderModelSwitchReportingCapability::SupportedAfterValidatedSwitch,
            permission_deferral:
                ProviderPermissionDeferralCapability::SupportedWithNonterminalOutcome,
            elicitation_forwarding:
                ElicitationForwardingCapability::SupportedWithCorrelatedRoundTrip,
            ..ProviderOperationCapabilities::default()
        },
        None,
    );
    let storage = MemoryStorage::default();
    let agent = attached_agent(provider, storage.manager().await)
        .await
        .unwrap();
    let capabilities = agent.operation_capabilities();
    assert_eq!(
        capabilities.compaction_reporting(),
        CompactionReportingCapability::UnsupportedNotImplemented
    );
    assert_eq!(
        capabilities.model_switch_reporting(),
        ModelSwitchReportingCapability::UnsupportedNotImplemented
    );
    assert_eq!(
        capabilities.permission_deferral(),
        PermissionDeferralCapability::UnsupportedNotImplemented
    );
    assert_eq!(
        capabilities.pre_tool_policy(),
        PreToolPolicyCapability::UnsupportedNotImplemented
    );
    assert_eq!(
        capabilities.policy_end_turn(),
        PolicyEndTurnCapability::UnsupportedNotImplemented
    );
    assert_eq!(
        capabilities.policy_close_session(),
        PolicyCloseSessionCapability::UnsupportedNotImplemented
    );
    assert_eq!(
        capabilities.incoming_elicitation(),
        IncomingElicitationCapability::UnsupportedNotImplemented
    );
}

/// Submit `input` every way an Agent accepts input and require the same
/// outcome each time, distinguishing application refusal from a durably admitted
/// input that only the opened provider can reject.
async fn every_entry(
    agent_answer: ProviderOperationCapabilities,
    refuses: Option<AgentError>,
    input: &ExecutionRequest,
    expected: Result<ExecutionOutcome, AgentError>,
    refusal_is_durably_admitted: bool,
) {
    for operation in 0..4 {
        let storage = MemoryStorage::default();
        let provider = ImageProvider::new(agent_answer, refuses.clone());
        let agent = attached_agent(provider.clone(), storage.manager().await)
            .await
            .unwrap();
        let writes = storage.0.lock().unwrap().writes;
        let result = submit(&agent, input.clone(), operation).await;
        assert_eq!(result, expected, "operation {operation}");
        let executions = provider.executions.load(Ordering::SeqCst);
        if expected.is_err() {
            assert_eq!(executions, 0, "operation {operation}");
            if refusal_is_durably_admitted || operation == 3 {
                let snapshot = storage.snapshot();
                assert_eq!(snapshot.invocations.len(), 1, "operation {operation}");
                assert_eq!(
                    snapshot.invocations[0].result,
                    Some(result),
                    "operation {operation}"
                );
            } else {
                assert_eq!(storage.0.lock().unwrap().writes, writes);
                assert!(storage.snapshot().invocations.is_empty());
            }
        } else {
            assert_eq!(executions, 1, "operation {operation}");
        }
        agent.close(actor()).await.unwrap();
    }
}
fn with_images(id: &str, images: Vec<ImageReference>) -> ExecutionRequest {
    ExecutionRequest {
        user_message: UserMessage::new(Some(PromptText::new("look").unwrap()), images, Vec::new())
            .unwrap(),
        ..request(id)
    }
}

#[tokio::test]
async fn an_agent_image_refusal_is_retained_without_provider_execution() {
    let input = with_images("image", vec![image(ImageMediaType::Png, 6)]);
    every_entry(
        AGENT_TAKES_NO_IMAGES,
        None,
        &input,
        Err(AgentError::ImageInputRefused(
            ImageInputRefusal::AgentDoesNotAccept,
        )),
        true,
    )
    .await;
    // The same agent still takes text: only the images were the problem.
    every_entry(
        AGENT_TAKES_NO_IMAGES,
        None,
        &request("text"),
        Ok(ExecutionOutcome::Completed),
        false,
    )
    .await;
}

#[tokio::test]
async fn an_agent_whose_answer_is_not_known_is_not_refused_at_admission() {
    // Not yet negotiated is not "no": the message is admitted and the adapter
    // answers for itself at dispatch. An agent that said yes is admitted too.
    let input = with_images("image", vec![image(ImageMediaType::Png, 6)]);
    for answer in [AGENT_NOT_YET_KNOWN, AGENT_TAKES_IMAGES] {
        every_entry(answer, None, &input, Ok(ExecutionOutcome::Completed), false).await;
    }
}

#[tokio::test]
async fn an_image_outside_the_models_limits_is_a_typed_refusal_before_every_admission_save() {
    for (images, refusal) in [
        (
            vec![image(ImageMediaType::Webp, 6)],
            ImageInputRefusal::MediaType(ImageMediaType::Webp),
        ),
        (
            // A good image first does not excuse the one after it.
            vec![image(ImageMediaType::Png, 6), image(ImageMediaType::Png, 7)],
            ImageInputRefusal::ImageTooLarge {
                size: 7,
                max_bytes: 6,
            },
        ),
    ] {
        every_entry(
            AGENT_TAKES_IMAGES,
            None,
            &with_images("image", images),
            Err(AgentError::ImageInputRefused(refusal)),
            false,
        )
        .await;
    }
}

#[tokio::test]
async fn backend_refusal_is_retained_without_provider_execution() {
    let refusal = AgentError::MessageTooLarge {
        encoded_bytes: 9000,
        max_bytes: 8192,
    };
    for input in [
        request("text"),
        with_images("image", vec![image(ImageMediaType::Png, 6)]),
    ] {
        every_entry(
            AGENT_TAKES_IMAGES,
            Some(refusal.clone()),
            &input,
            Err(refusal.clone()),
            true,
        )
        .await;
    }
}
