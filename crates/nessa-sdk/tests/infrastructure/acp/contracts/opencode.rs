//! The Opencode profile against a handler speaking Opencode's own shapes: what
//! it selects, what it refuses to proceed without, and what survives
//! translation.
//!
//! Not all of the handler's frames were recorded, and the difference is stated
//! in the handler's own docstring rather than glossed here. The `initialize`
//! result and the `session/new` shapes were read off Opencode 1.18.31 by
//! driving the binary with an empty home. The tool call, the permission request
//! and the mid-turn mode change were not: each only happens during a model
//! turn, and this environment's network policy does not allow OpenCode Zen's
//! host, so those are written from the ACP specification. What the tests below
//! prove about them is that this profile translates those shapes correctly —
//! not that Opencode sends them.
use super::support::*;
use crate::domain::agent_execution::tools::ToolContent;
use crate::domain::model_metadata::entities::ModelMetadata;
use crate::infrastructure::{model_metadata_json::load_catalog, process::ProcessScope};
use std::{fs::File, path::PathBuf, time::Duration};
use tokio::{io::AsyncReadExt, process::ChildStdout, time::timeout};

const HARNESS_DEADLINE: Duration = Duration::from_secs(60);
const MAXIMUM_HARNESS_OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Debug, PartialEq, Eq)]
enum HarnessReadFailure {
    Deadline,
    OutputLimit,
    Closed,
    Read(String),
}

struct HarnessSupervisor {
    scope: ProcessScope,
    root: PathBuf,
    stdout: ChildStdout,
}

impl HarnessSupervisor {
    async fn start(arguments: &[&str]) -> Self {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/infrastructure/acp/contracts/fixtures/opencode_live_boundary.py");
        let arguments = arguments
            .iter()
            .map(|argument| (*argument).to_owned())
            .collect::<Vec<_>>();
        let started = ProcessScope::spawn_with_private_directory(move |root| {
            let mut command = tokio::process::Command::new("python3");
            command
                .arg(&fixture)
                .arg("--supervised-root")
                .arg(root)
                .args(&arguments);
            command
        });
        let (mut scope, root) = match started {
            Ok(started) => started,
            Err(failure) => {
                let (cause, recovery) = failure.into_parts();
                if let Some(mut directory) = recovery {
                    directory
                        .release(Duration::from_secs(5))
                        .await
                        .expect("failed harness start fixture is released");
                }
                panic!("could not start Python Opencode harness: {cause}");
            }
        };
        let stdout = scope.stdout.take().expect("harness stdout is piped");
        Self {
            scope,
            root,
            stdout,
        }
    }

    async fn read_message(
        &mut self,
        budget: Duration,
    ) -> Result<serde_json::Value, HarnessReadFailure> {
        timeout(budget, async {
            let mut message = Vec::new();
            let mut chunk = [0_u8; 8192];
            loop {
                let count = self
                    .stdout
                    .read(&mut chunk)
                    .await
                    .map_err(|error| HarnessReadFailure::Read(error.to_string()))?;
                if count == 0 {
                    return Err(HarnessReadFailure::Closed);
                }
                let end = chunk[..count]
                    .iter()
                    .position(|byte| *byte == b'\n')
                    .unwrap_or(count);
                if message.len() + end > MAXIMUM_HARNESS_OUTPUT_BYTES {
                    return Err(HarnessReadFailure::OutputLimit);
                }
                message.extend_from_slice(&chunk[..end]);
                if end != count {
                    return serde_json::from_slice(&message)
                        .map_err(|error| HarnessReadFailure::Read(error.to_string()));
                }
            }
        })
        .await
        .map_err(|_| HarnessReadFailure::Deadline)?
    }

    async fn cleanup(&mut self) {
        self.scope
            .cleanup(Duration::ZERO, Duration::from_secs(5))
            .await
            .expect("harness process group cleanup is confirmed");
        assert!(
            !self.root.exists(),
            "fixture root remained after confirmed process cleanup"
        );
    }
}

async fn run_python_harness(arguments: &[&str]) {
    let mut supervisor = HarnessSupervisor::start(arguments).await;
    let message = supervisor.read_message(HARNESS_DEADLINE).await;
    supervisor.cleanup().await;
    let message = message.expect("Python Opencode harness did not return a bounded result");
    assert!(
        message["ok"].as_bool() == Some(true),
        "Python Opencode harness failed: {}",
        message["error"].as_str().unwrap_or("missing error")
    );
}

#[cfg(unix)]
fn process_exists(identifier: u32) -> bool {
    let result = unsafe { libc::kill(identifier as i32, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(unix)]
#[tokio::test]
async fn outer_deadline_reaps_active_acp_descendants_before_releasing_fixture_root() {
    let mut supervisor = HarnessSupervisor::start(&["outer-timeout"]).await;
    let root = supervisor.root.clone();
    let active = supervisor
        .read_message(Duration::from_secs(5))
        .await
        .expect("synthetic ACP and descendant reached their blocking state");
    assert_eq!(active["state"], "active");
    let acp = active["acpPid"].as_u64().unwrap() as u32;
    let descendant = active["descendantPid"].as_u64().unwrap() as u32;
    assert!(process_exists(acp));
    assert!(process_exists(descendant));

    assert_eq!(
        supervisor.read_message(Duration::from_millis(100)).await,
        Err(HarnessReadFailure::Deadline)
    );
    assert!(root.exists(), "fixture root was released before cleanup");
    supervisor.cleanup().await;
    assert!(!process_exists(acp));
    assert!(!process_exists(descendant));
    assert!(!root.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn silent_provider_reaches_inner_rpc_deadline_and_is_reaped() {
    run_python_harness(&["silent-provider"]).await;
}

async fn run_live_opencode_harness(mode: &str, arguments: &[PathBuf]) {
    let binary = std::env::var_os("NESSA_PINNED_OPENCODE_BINARY")
        .expect("set NESSA_PINNED_OPENCODE_BINARY to an audited 1.18.31 executable");
    let binary = binary.to_string_lossy();
    let arguments = arguments
        .iter()
        .map(|argument| argument.to_string_lossy())
        .collect::<Vec<_>>();
    let mut harness_arguments = vec![mode, binary.as_ref()];
    harness_arguments.extend(arguments.iter().map(|argument| argument.as_ref()));
    run_python_harness(&harness_arguments).await;
}

/// This is opt-in because obtaining the audited binary belongs to the installer
/// boundary. The harness itself never downloads or modifies that executable.
#[tokio::test]
#[ignore = "requires NESSA_PINNED_OPENCODE_BINARY from the audited installer"]
async fn pinned_binary_cannot_load_caller_config_tools_mcp_or_native_hooks() {
    run_live_opencode_harness("boundary", &[]).await;
}

/// The production launch disables remote model refresh, so this compares the
/// shipped catalogue with the pinned binary's embedded, credentialed options.
#[tokio::test]
#[ignore = "requires NESSA_PINNED_OPENCODE_BINARY from the audited installer"]
async fn shipped_opencode_models_remain_in_the_pinned_catalogue() {
    let catalogue = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data/models.json");
    for (model, auth_tier) in [
        ("opencode/big-pickle", "public"),
        ("opencode/nemotron-3-ultra-free", "public"),
        ("opencode/mimo-v2.5-free", "opencode-api"),
    ] {
        run_live_opencode_harness(
            "catalogue",
            &[catalogue.clone(), model.into(), auth_tier.into()],
        )
        .await;
    }
}

/// Unlike the raw-process marker harness, this drives the compiled binding's
/// launch environment, process scope, startup ordering, and restoration path.
#[tokio::test]
#[ignore = "requires NESSA_PINNED_OPENCODE_BINARY from the audited installer"]
async fn compiled_binding_opens_and_restores_with_the_pinned_binary() {
    let _process_slot = process_test_slot().await;
    let binary = PathBuf::from(
        std::env::var_os("NESSA_PINNED_OPENCODE_BINARY")
            .expect("set NESSA_PINNED_OPENCODE_BINARY to an audited 1.18.31 executable"),
    );
    let (_root, mut config, _) = opencode_configuration("unused-by-live-binary", 16);
    config.executable = ExecutableUseSnapshot::unmanaged(binary);
    config.arguments = vec!["acp".into()];
    config.launch_timeout = Duration::from_secs(30);
    config.startup_timeout = Duration::from_secs(15);
    let catalog = load_catalog(
        File::open(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data/models.json")).unwrap(),
    )
    .unwrap();
    let model = ModelMetadata::try_from(catalog.select("opencode", "opencode/big-pickle").unwrap())
        .unwrap();
    let binding = OpencodeAcpProvider::new(
        config,
        &model,
        TokenLimits::new(128_000, 32_000).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap();

    let opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let session_id = opened.session.id().clone();
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    let restored = binding
        .open(ProviderOpenRequest::without_startup_control(Some(
            session_id.clone(),
        )))
        .await
        .unwrap();
    assert_eq!(restored.session.id(), &session_id);
    restored
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
}

#[tokio::test]
async fn a_session_is_opened_configured_and_prompted_through_the_shared_runtime() {
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_opencode_binding("echo", 16);
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    assert_eq!(
        opened.session.capabilities().model().model_id(),
        "exact-fixture-model"
    );
    assert_eq!(
        opened
            .session
            .execute(prompt("first"))
            .await
            .into_result()
            .unwrap(),
        ExecutionOutcome::Completed
    );
    assert_eq!(
        next(&mut opened).await,
        ExecutionUpdate::Message(MessageChunk::text("first"))
    );
    assert_eq!(
        next(&mut opened).await,
        ExecutionUpdate::Finished(ExecutionOutcome::Completed)
    );
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root, "pid");
}

/// Opencode opens every session in `build` — it has no way to start in the
/// other — so the session this binding hands back has to be one it moved into
/// `plan` first. The handler asserts the order the selections arrive in; this
/// asserts that they arrive at all before anything can be prompted.
#[tokio::test]
async fn a_session_is_in_the_configured_mode_before_it_can_be_prompted() {
    let _process_slot = process_test_slot().await;
    let (_root, binding) = test_opencode_binding("echo", 16);
    // Opening is what applies the configuration: a session that reached the
    // caller is a configured one, and the handler refuses the prompt otherwise.
    let opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    assert_eq!(
        opened
            .session
            .execute(prompt("first"))
            .await
            .into_result()
            .unwrap(),
        ExecutionOutcome::Completed
    );
}

/// A mode change after the fact is the session leaving the policy it was opened
/// under. It arrives as an update rather than as an answer, so nothing else
/// would have questioned it.
#[tokio::test]
async fn a_session_that_leaves_its_mode_afterwards_fails_the_execution() {
    let _process_slot = process_test_slot().await;
    let (_root, binding) = test_opencode_binding("left-the-mode", 16);
    let opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let outcome = opened.session.execute(prompt("first")).await.into_result();
    assert!(outcome.is_err(), "{outcome:?}");
}

/// The version is the one this profile was written against, and the name is the
/// agent's own. An Opencode of another version is one whose wire behaviour
/// nobody here has read, and it is refused rather than driven on the hope that
/// nothing moved. Nothing yet ties that version to one anybody installs — see
/// the note on `VERSION`.
#[tokio::test]
async fn an_opencode_this_profile_was_not_written_against_is_refused() {
    for mode in ["wrong-harness", "wrong-version"] {
        let _process_slot = process_test_slot().await;
        let (_root, binding) = test_opencode_binding(mode, 16);
        let opened = binding
            .open(ProviderOpenRequest::without_startup_control(None))
            .await;
        assert!(opened.is_err(), "{mode} was accepted");
    }
}

#[tokio::test]
async fn a_session_is_refused_rather_than_run_half_configured() {
    for mode in ["model-not-offered", "model-refused", "mode-refused"] {
        let _process_slot = process_test_slot().await;
        let (_root, binding) = test_opencode_binding(mode, 16);
        let opened = binding
            .open(ProviderOpenRequest::without_startup_control(None))
            .await;
        assert!(opened.is_err(), "{mode} opened a session anyway");
    }
}

/// A provider may report its prior configuration while this binding is still
/// applying a requested model, so that advisory update is not final eligibility
/// evidence and need not already name the requested model.
#[tokio::test]
async fn opencode_reporting_its_configuration_while_it_is_being_configured_is_not_a_failure() {
    let _process_slot = process_test_slot().await;
    let (_root, binding) = test_opencode_binding("startup-update-configuring", 16);
    assert!(binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .is_ok());
}

#[tokio::test]
async fn only_a_correlated_advisory_update_may_race_the_session_response() {
    for mode in [
        "wrong-session-startup-update",
        "execution-output-before-session-response",
    ] {
        let _process_slot = process_test_slot().await;
        let (_root, binding) = test_opencode_binding(mode, 16);
        assert!(
            binding
                .open(ProviderOpenRequest::without_startup_control(None))
                .await
                .is_err(),
            "{mode} was admitted"
        );
    }
}

/// Opencode opens every session in `build` and offers no way to start in the
/// mode this binding wants, so until the selection lands, `build` is the honest
/// answer to what mode the session is in. Announcing it is Opencode being
/// truthful, and the session must survive it — the pair of this test and
/// `a_session_that_leaves_its_mode_afterwards_fails_the_execution` is the whole
/// rule: tolerated before the selection, refused after it.
#[tokio::test]
async fn opencode_naming_the_mode_it_opened_in_is_not_leaving_the_one_it_is_given() {
    let _process_slot = process_test_slot().await;
    let (_root, binding) = test_opencode_binding("announces-start-mode", 16);
    assert!(binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .is_ok());
}

/// Tolerating the mode Opencode opens in is not tolerating any mode at all.
/// The window before the selection lands admits the two modes the session
/// offers and nothing else, so a session announcing a third is still refused —
/// otherwise "we have not configured it yet" would be a hole a session could
/// be put into any policy through.
#[tokio::test]
async fn a_mode_the_session_never_offered_is_refused_even_before_it_is_configured() {
    let _process_slot = process_test_slot().await;
    let (_root, binding) = test_opencode_binding("announces-unknown-mode", 16);
    assert!(binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .is_err());
}

/// Opencode reports tool calls in the protocol's own shape, so what this
/// profile adds is that they survive whole — the announcement and the update
/// that completes it are one call, not two.
#[tokio::test]
async fn a_tool_call_arrives_with_what_it_read() {
    let _process_slot = process_test_slot().await;
    let (_root, binding) = test_opencode_binding("read-tool-call", 16);
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let running = start(&opened, "first").await;
    let mut contents = Vec::new();
    loop {
        match next(&mut opened).await {
            ExecutionUpdate::Tool(update) => {
                if let Some(content) = update.content() {
                    contents.extend(content.iter().cloned());
                }
            }
            ExecutionUpdate::Finished(_) => break,
            _ => {}
        }
    }
    running.await.unwrap().unwrap();
    assert_eq!(contents, vec![ToolContent::text("fn main() {}".to_owned())]);
}

/// A permission request reaches the host naming what class of action it is and
/// carrying the arguments it would act on.
///
/// Named by its ACP kind, not by its title. The title here is "Edit
/// src/main.rs", which is the friendlier label and the wrong thing to record: a
/// title is display text Opencode composes, possibly out of what the model
/// supplied, and nothing makes it agree with `rawInput`. The kind is one of the
/// protocol's ten and the arguments say what is being acted on, which together
/// are a decision somebody can be held to.
#[tokio::test]
async fn an_edit_approval_reaches_the_host_with_what_it_would_change() {
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_opencode_binding("edit-permission", 16);
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let running = start(&opened, "first").await;
    let ExecutionUpdate::PermissionRequested {
        id, input, options, ..
    } = next(&mut opened).await
    else {
        panic!("expected permission");
    };
    // The kind, though the frame also carries a title. A decision recorded
    // against a name with nothing under it is not a reviewed decision, and one
    // recorded against a name the request chose the wording of is not either.
    assert_eq!(input.name, "edit");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&input.arguments_json).unwrap(),
        serde_json::json!({"filePath": "src/main.rs", "newText": "fn main() {}"})
    );
    let allow = options
        .choices()
        .iter()
        .find(|option| {
            option.decision().clone()
                == PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request())
        })
        .unwrap()
        .id()
        .clone();
    opened
        .session
        .answer_permission(PermissionAnswer {
            attribution: attribution(),
            execution_id: ExecutionId::new("first").unwrap(),
            id,
            option_id: allow.clone(),
        })
        .await
        .map_err(|failure| failure.into_error())
        .unwrap();
    assert_eq!(running.await.unwrap().unwrap(), ExecutionOutcome::Completed);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            &std::fs::read_to_string(root.path().join("permission-outcome")).unwrap()
        )
        .unwrap(),
        serde_json::json!({"outcome":"selected","optionId":allow.as_str()})
    );
}

/// Image input is offered exactly when composition supplied a byte source, and
/// never otherwise.
///
/// This binding declared text-only for a while, because the shared worker built
/// every `session/prompt` as a single text block and an image declared here had
/// nowhere to go. The worker now carries image blocks, so the honest answer
/// flipped: withholding the modality is what would leave a caller's picture
/// behind. Both directions are asserted, because one alone passes on a binding
/// that ignores the source and hardcodes the answer either way.
///
/// The model is one that takes images. The fixture's own model is text-only,
/// and `EffectiveCapabilities` intersects the two, so through that model the
/// bit would come out text whatever the binding said.
#[tokio::test]
async fn a_session_offers_image_input_exactly_when_it_has_somewhere_to_read_bytes() {
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_opencode_binding_on_a_model_that_takes_images_from(
        "echo",
        16,
        Some(Arc::new(UnreadImages)),
    );
    let opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let features = opened.session.capabilities().features();
    assert!(features.input().text());
    assert!(
        features.input().image(),
        "a binding with a byte source withheld the model's image input"
    );
    // Nothing Opencode serves sends an image back.
    assert!(!features.output().image());
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root, "pid");

    let (root, binding) = test_opencode_binding_on_a_model_that_takes_images("echo", 16);
    let opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let features = opened.session.capabilities().features();
    assert!(features.input().text());
    assert!(
        !features.input().image(),
        "a binding with no byte source offered an image it could not read"
    );
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root, "pid");
}

/// What the record says happened, not just that the session carried on.
///
/// The three shared contract files audit closure, finishing and cancellation
/// against the Claude binding, because the runtime that writes those records is
/// shared. The permission record is the one this adapter has a hand in: the
/// name and the arguments in it come out of `opencode_acp::tools::wire`, and
/// nothing else re-derives them. So this asserts the record, not the outcome —
/// a translation that answered Opencode correctly while filing the decision
/// under another session, another tool, or no attribution would pass every
/// other test in this file.
#[tokio::test]
async fn an_answered_permission_is_recorded_against_the_session_and_the_tool_that_asked() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, binding) = test_opencode_binding_with_audit("edit-permission", 16, audit.clone());
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let session_id = opened.session.id().clone();
    let running = start(&opened, "first").await;
    let ExecutionUpdate::PermissionRequested { id, options, .. } = next(&mut opened).await else {
        panic!("expected permission");
    };
    let allow = options
        .choices()
        .iter()
        .find(|option| {
            option.decision().clone()
                == PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request())
        })
        .unwrap()
        .id()
        .clone();
    opened
        .session
        .answer_permission(PermissionAnswer {
            attribution: attribution(),
            execution_id: ExecutionId::new("first").unwrap(),
            id,
            option_id: allow,
        })
        .await
        .map_err(|failure| failure.into_error())
        .unwrap();
    assert_eq!(running.await.unwrap().unwrap(), ExecutionOutcome::Completed);
    let answers = audit.answers.lock().unwrap().clone();
    // Two records, in this order: the decision as it was taken, then the write
    // that carried it. The order is the claim — a binding that told Opencode
    // first and filed the decision afterwards would record `Written` before
    // anything said what was chosen.
    let [selected, written] = &answers[..] else {
        panic!(
            "expected a selected and a written record, got {}",
            answers.len()
        );
    };
    assert_eq!(selected.delivery(), &PermissionAnswerDelivery::Selected);
    assert_eq!(written.delivery(), &PermissionAnswerDelivery::Written);
    for answer in [selected, written] {
        assert_eq!(answer.session_id(), &session_id);
        let resolution = answer.resolution();
        assert_eq!(resolution.session_id(), &session_id);
        // The kind, and the arguments the request carried — the same pair the
        // host was shown. A record naming `edit` over somebody else's
        // arguments, or the request's own title over these, is not the
        // decision that was taken.
        assert_eq!(resolution.input().name, "edit");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&resolution.input().arguments_json).unwrap(),
            serde_json::json!({"filePath": "src/main.rs", "newText": "fn main() {}"})
        );
        assert_eq!(
            resolution.attribution().actor(),
            attribution().actor(),
            "the record credits somebody other than the actor that answered"
        );
    }
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root, "pid");
}

/// A refusal is filed the same way an approval is.
///
/// The allow path is the one that lets a tool run, so it was written first.
/// This is the other half of the same claim, and it is not covered by the
/// shared lifecycle records: a denial goes through the same `record_answer`
/// with the same `ToolReviewInput` derived from `opencode_acp::tools::wire`,
/// so the thing under test here is this profile's naming of the request, not
/// the runtime's handling of it. "Nobody allowed this" and "somebody refused
/// this" are different facts, and only one of them is written down if a
/// binding files approvals alone.
#[tokio::test]
async fn a_refused_permission_is_recorded_as_the_refusal_it_was() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, binding) = test_opencode_binding_with_audit("edit-permission", 16, audit.clone());
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let session_id = opened.session.id().clone();
    let running = start(&opened, "first").await;
    let ExecutionUpdate::PermissionRequested { id, options, .. } = next(&mut opened).await else {
        panic!("expected permission");
    };
    let deny = options
        .choices()
        .iter()
        .find(|option| option.decision().effect() == PermissionEffect::Deny)
        .expect("the request offered a refusal")
        .id()
        .clone();
    opened
        .session
        .answer_permission(PermissionAnswer {
            attribution: attribution(),
            execution_id: ExecutionId::new("first").unwrap(),
            id,
            option_id: deny.clone(),
        })
        .await
        .map_err(|failure| failure.into_error())
        .unwrap();
    assert_eq!(running.await.unwrap().unwrap(), ExecutionOutcome::Completed);

    let answers = audit.answers.lock().unwrap().clone();
    // The same two records in the same order as an approval: what was decided,
    // then that it was delivered. A binding that filed only the answers which
    // let something happen would have one record here, or none.
    let [selected, written] = &answers[..] else {
        panic!(
            "expected a selected and a written record, got {}",
            answers.len()
        );
    };
    assert_eq!(selected.delivery(), &PermissionAnswerDelivery::Selected);
    assert_eq!(written.delivery(), &PermissionAnswerDelivery::Written);
    for answer in [selected, written] {
        assert_eq!(answer.session_id(), &session_id);
        let resolution = answer.resolution();
        // Named by what it was about, exactly as the approval is: a refusal
        // recorded against the wrong tool or the wrong arguments says nothing
        // about what was refused.
        assert_eq!(resolution.input().name, "edit");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&resolution.input().arguments_json).unwrap(),
            serde_json::json!({"filePath": "src/main.rs", "newText": "fn main() {}"})
        );
    }
    // And Opencode was told the refusal, rather than the request being left to
    // time out into one.
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            &std::fs::read_to_string(root.path().join("permission-outcome")).unwrap()
        )
        .unwrap(),
        serde_json::json!({"outcome":"selected","optionId":deny.as_str()})
    );
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root, "pid");
}

/// The audit is a precondition of the approval, not a report of it.
///
/// The same rule the Codex binding is held to, on the adapter that admits a
/// second vendor's permission frames: if the decision cannot be written down,
/// Opencode is never told to proceed on it. A binding that answered first and
/// recorded afterwards would leave a tool run with no record of who allowed it.
#[tokio::test]
async fn an_approval_the_audit_cannot_record_is_never_given_to_opencode() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit {
        reject: true,
        ..Default::default()
    });
    let (root, binding) = test_opencode_binding_with_audit("edit-permission", 16, audit);
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let running = start(&opened, "first").await;
    let ExecutionUpdate::PermissionRequested { id, options, .. } = next(&mut opened).await else {
        panic!("expected permission");
    };
    let allow = options
        .choices()
        .iter()
        .find(|option| {
            option.decision().clone()
                == PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request())
        })
        .unwrap()
        .id()
        .clone();
    let failure = opened
        .session
        .answer_permission(PermissionAnswer {
            attribution: attribution(),
            execution_id: ExecutionId::new("first").unwrap(),
            id,
            option_id: allow,
        })
        .await
        .unwrap_err();
    assert_eq!(failure.error(), &AgentError::AuditFailure);
    assert_eq!(running.await.unwrap(), Err(rejected_audits(3)));
    // Opencode is never told to proceed. If it is told anything, it is that the
    // request was cancelled, which is the session being torn down around it.
    if let Ok(told) = std::fs::read_to_string(root.path().join("permission-outcome")) {
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&told).unwrap(),
            serde_json::json!({"outcome": "cancelled"}),
        );
    }
    assert_eq!(
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result(),
        Err(rejected_audits(3))
    );
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn opencode_close_cancels_the_exact_pending_review_and_audits_its_caller() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, binding) = test_opencode_binding_with_audit("edit-permission", 16, audit.clone());
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let running = start(&opened, "cancelled-review").await;
    let ExecutionUpdate::PermissionRequested { id, input, .. } = next(&mut opened).await else {
        panic!("expected permission")
    };

    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    // Closing cancels the outstanding review. Opencode answers that cancellation
    // by completing the enclosing prompt, so its confirmed provider result stays
    // distinct from the local review cancellation and session closure.
    assert_eq!(running.await.unwrap().unwrap(), ExecutionOutcome::Completed);
    let cancellations = audit.records.lock().unwrap();
    assert_eq!(cancellations.len(), 1);
    assert_eq!(cancellations[0].session_id(), opened.session.id());
    assert_eq!(cancellations[0].request().id(), &id);
    assert_eq!(cancellations[0].input(), &input);
    assert_eq!(
        cancellations[0].request().state(),
        PermissionStateView::Cancelled {
            reason: &PermissionCancellationReason::session_closed()
        }
    );
    assert_eq!(
        cancellations[0].origin(),
        &CancellationOrigin::Client(close_action())
    );
    drop(cancellations);
    assert_eq!(audit.closures.lock().unwrap().len(), 1);
    let finishes = audit.finishes.lock().unwrap();
    assert_eq!(finishes.len(), 1);
    assert_eq!(finishes[0].execution_id().as_str(), "cancelled-review");
    assert_eq!(finishes[0].result(), &Ok(ExecutionOutcome::Completed));
    drop(finishes);
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn opencode_permission_cancellation_audit_failure_is_visible_after_cleanup() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit {
        reject: true,
        ..Default::default()
    });
    let (root, binding) = test_opencode_binding_with_audit("edit-permission", 16, audit);
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let running = start(&opened, "cancelled-review").await;
    assert!(matches!(
        next(&mut opened).await,
        ExecutionUpdate::PermissionRequested { .. }
    ));

    assert_eq!(
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result(),
        Err(rejected_audits(3))
    );
    assert_eq!(running.await.unwrap(), Err(rejected_audits(3)));
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn opencode_execution_deadline_retains_runtime_cause_and_correlation() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, binding) = test_opencode_binding_with_audit("stall", 16, audit.clone());
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let running = start(&opened, "deadline").await;
    assert_eq!(
        next(&mut opened).await,
        ExecutionUpdate::Message(MessageChunk::text("running"))
    );
    assert_eq!(running.await.unwrap(), Err(AgentError::Deadline));
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();

    let closures = audit.closures.lock().unwrap();
    assert_eq!(closures.len(), 1);
    assert_eq!(
        closures[0].closure().execution_id().unwrap().as_str(),
        "deadline"
    );
    assert_eq!(
        closures[0].closure().reason(),
        &PermissionCancellationReason::deadline_exceeded()
    );
    assert_eq!(closures[0].origin(), &CancellationOrigin::Runtime);
    drop(closures);
    let finishes = audit.finishes.lock().unwrap();
    assert_eq!(finishes.len(), 1);
    assert_eq!(finishes[0].execution_id().as_str(), "deadline");
    assert_eq!(
        finishes[0].result(),
        &Err(PermissionCancellationReason::deadline_exceeded())
    );
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn opencode_permission_answer_survives_the_execution_callers_loss() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, binding) = test_opencode_binding_with_audit("edit-permission", 16, audit.clone());
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let running = start(&opened, "lost-caller").await;
    let ExecutionUpdate::PermissionRequested { id, options, .. } = next(&mut opened).await else {
        panic!("expected permission")
    };
    let allow = options
        .choices()
        .iter()
        .find(|option| option.decision().effect() == PermissionEffect::Allow)
        .unwrap()
        .id()
        .clone();
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());

    opened
        .session
        .answer_permission(PermissionAnswer {
            attribution: attribution(),
            execution_id: ExecutionId::new("lost-caller").unwrap(),
            id,
            option_id: allow,
        })
        .await
        .map_err(|failure| failure.into_error())
        .unwrap();
    wait_for_file(&root, "permission-outcome").await;
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();

    let answers = audit.answers.lock().unwrap();
    assert_eq!(answers.len(), 2);
    assert_eq!(answers[0].delivery(), &PermissionAnswerDelivery::Selected);
    assert_eq!(answers[1].delivery(), &PermissionAnswerDelivery::Written);
    drop(answers);
    let finishes = audit.finishes.lock().unwrap();
    assert_eq!(finishes.len(), 1);
    assert_eq!(finishes[0].execution_id().as_str(), "lost-caller");
    assert_eq!(finishes[0].result(), &Ok(ExecutionOutcome::Completed));
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn opencode_process_exit_mid_turn_retains_failed_execution_audit() {
    let _process_slot = process_test_slot().await;
    let audit = Arc::new(RecordingAudit::default());
    let (root, binding) = test_opencode_binding_with_audit("process-exit", 16, audit.clone());
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let running = start(&opened, "process-exit").await;
    assert_eq!(
        next(&mut opened).await,
        ExecutionUpdate::Message(MessageChunk::text("running"))
    );
    assert!(matches!(
        running.await.unwrap(),
        Err(AgentError::Transport(_))
    ));
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();

    let closures = audit.closures.lock().unwrap();
    assert_eq!(closures.len(), 1);
    assert_eq!(
        closures[0].closure().execution_id().unwrap().as_str(),
        "process-exit"
    );
    assert_eq!(
        closures[0].closure().reason(),
        &PermissionCancellationReason::execution_failed()
    );
    assert_eq!(closures[0].origin(), &CancellationOrigin::Runtime);
    drop(closures);
    let finishes = audit.finishes.lock().unwrap();
    assert_eq!(finishes.len(), 1);
    assert_eq!(finishes[0].execution_id().as_str(), "process-exit");
    assert_eq!(
        finishes[0].result(),
        &Err(PermissionCancellationReason::execution_failed())
    );
    assert_gone(&root, "pid");
}

/// Opencode volunteers its command list the instant `session/new` is answered,
/// before Nessa has configured anything, and the session has to survive it.
///
/// It arrives in the startup window, where the runtime is still reading frames
/// against a session it may not have admitted yet and refusing execution output
/// that has no prompt behind it. An advisory update mistaken for either of
/// those fails `open()` outright, so the agent never starts at all — which is
/// why this is worth a test of its own even though the handler now sends it in
/// every mode.
#[tokio::test]
async fn the_command_list_opencode_volunteers_at_startup_does_not_stop_the_session() {
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_opencode_binding("startup-update-before-session-response", 16);
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    // Configuration still completed underneath it: the session is prompted and
    // answers, rather than merely having opened.
    let running = start(&opened, "first").await;
    let ExecutionUpdate::Message { .. } = next(&mut opened).await else {
        panic!("expected the echoed message");
    };
    assert_eq!(running.await.unwrap().unwrap(), ExecutionOutcome::Completed);
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root, "pid");
}
