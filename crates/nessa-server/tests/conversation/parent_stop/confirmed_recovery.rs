//! A cancellation receipt can commit before the provider turn settles.
use super::*;
use crate::conversation::application::{FailureBackoff, UnfinishedTurns};
use nessa_sdk::domain::agent_execution::executions::SchedulingCause;

#[test]
#[ignore = "invoked only by the confirmed Stop crash parent"]
fn confirmed_stop_crash_child() {
    let root = PathBuf::from(std::env::var_os("NESSA_MUTATION_ROOT").unwrap());
    let target = ConversationId::new(&std::env::var("NESSA_MUTATION_TARGET").unwrap()).unwrap();
    let cascade = std::env::var("NESSA_STOP_CASCADE").unwrap() == "true";
    let stage = std::env::var("NESSA_STOP_STAGE").unwrap_or_else(|_| "settled".into());
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let provider = Arc::new(ProviderFactory::default());
            provider.turn_cancel.store(true, Ordering::SeqCst);
            let (service, storage, _metadata) = fixture(&root, provider.clone());
            bind(&service, &storage, &root, cascade).await;
            opened(&service, &provider, &target).await;
            // Native cancel acknowledges, but this separate execution gate keeps
            // settlement unfinished. Killing now leaves real Running SDK history.
            let _parent = hold_turn(&service, &provider, &target, "parent").await;
            let child_id = child(&service, &target, "parent", "create-child").await;
            let _child = hold_turn(&service, &provider, &child_id, "child").await;
            let _held_cancel;
            match stage.as_str() {
                "accepted" => {
                    _held_cancel = None;
                    let faulty = Arc::new(FaultyCommands {
                        storage: storage.clone(),
                        target: target.clone(),
                        commit: true,
                        stage: MutationStage::Accepted,
                        once: Arc::new(AtomicBool::new(true)),
                    });
                    assert!(service
                        .stop_command(
                            faulty,
                            target.clone(),
                            caller("confirmed-stop"),
                            "parent".into()
                        )
                        .await
                        .is_err());
                    assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 0);
                }
                "attempted" => {
                    let (release, gate) = oneshot::channel();
                    _held_cancel = Some(release);
                    *provider.cancel_gate.lock().unwrap() = Some(gate);
                    let entered = provider.cancel_started.notified();
                    tokio::spawn({
                        let service = service.clone();
                        let storage = storage.clone();
                        let target = target.clone();
                        async move {
                            service
                                .stop_command(
                                    storage,
                                    target,
                                    caller("confirmed-stop"),
                                    "parent".into(),
                                )
                                .await
                        }
                    });
                    tokio::time::timeout(TURN_HOLD_HANG, entered).await.unwrap();
                }
                "settled" => {
                    _held_cancel = None;
                    let receipt = service
                        .stop_command(
                            storage.clone(),
                            target.clone(),
                            caller("confirmed-stop"),
                            "parent".into(),
                        )
                        .await
                        .unwrap();
                    assert_eq!(receipt.stage(), MutationStage::Settled);
                    assert_eq!(receipt.outcome(), Some(MutationOutcome::Cancelled));
                    assert_eq!(
                        provider.cancel_calls.load(Ordering::SeqCst),
                        if cascade { 2 } else { 1 }
                    );
                }
                other => panic!("unknown Stop stage {other}"),
            }
            let live = service.resolve(&target, &caller("inspect")).await.unwrap();
            let history = live.agent.session_manager().snapshot().await.unwrap();
            let parent = history
                .invocations
                .iter()
                .find(|r| r.request.execution_id.as_str() == "parent")
                .unwrap();
            assert!(parent.result.is_none());
            assert_eq!(
                parent.scheduling.last().unwrap().stage,
                InvocationStage::Running
            );
            std::fs::write(root.join("confirmed-child"), child_id.to_string()).unwrap();
            std::fs::write(root.join("confirmed-stop"), b"1").unwrap();
            std::future::pending::<()>().await;
        });
}

async fn crash_after_stop(
    root: &Path,
    target: &ConversationId,
    cascade: bool,
    stage: &str,
) -> ConversationId {
    let log = root.with_extension("crash.log");
    let output = std::fs::File::create(&log).unwrap();
    let mut process = ChildOwner(Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "conversation::application::service::mutation::tests::parent_stop::confirmed_recovery::confirmed_stop_crash_child", "--ignored", "--nocapture"])
        .env("NESSA_MUTATION_ROOT", root).env("NESSA_MUTATION_TARGET", target.to_string())
        .env("NESSA_STOP_CASCADE", cascade.to_string()).env("NESSA_STOP_STAGE",stage)
        .stdout(Stdio::from(output.try_clone().unwrap())).stderr(Stdio::from(output)).spawn().unwrap());
    wait_marker(root, "confirmed-stop", &mut process, &log).await;
    process.0.kill().unwrap();
    assert!(!process.0.wait().unwrap().success());
    drop(process);
    ConversationId::new(&std::fs::read_to_string(root.join("confirmed-child")).unwrap()).unwrap()
}

#[tokio::test]
async fn a_confirmed_stop_survives_process_loss_before_turn_settlement() {
    for cascade in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("data");
        let target = id();
        let child_id = crash_after_stop(&root, &target, cascade, "settled").await;
        let provider = Arc::new(ProviderFactory::default());
        provider.turn_cancel.store(true, Ordering::SeqCst);
        let (service, storage, metadata) = fixture(&root, provider.clone());
        // Settings changes do not reinterpret a confirmed selection.
        bind(&service, &storage, &root, !cascade).await;
        service.bind_unfinished_turns(metadata.clone(), FailureBackoff::default());
        service.mark_unfinished(&target).await.unwrap();
        let live = service
            .resolve(&target, &caller("inspect-reopened"))
            .await
            .unwrap();
        assert_eq!(live.agent.owed_turns().await.len(), 1);
        assert!(service.resume_owed(&target, live.clone()).await);
        assert!(
            live.agent.owed_turns().await.is_empty(),
            "a confirmed Stop must not remain owed after a crash"
        );
        let history = live.agent.session_manager().snapshot().await.unwrap();
        let parent = history
            .invocations
            .iter()
            .find(|r| r.request.execution_id.as_str() == "parent")
            .unwrap();
        assert_eq!(
            parent.scheduling.last().unwrap().cause,
            SchedulingCause::NotResumed
        );
        assert_eq!(
            parent.scheduling.last().unwrap().actor.as_ref(),
            Some(&caller("confirmed-stop").actor().unwrap())
        );
        assert!(resumed_turn_id(&history.invocations, &parent.request.execution_id).is_none());
        assert!(
            !service.resume_owed(&target, live.clone()).await,
            "reconciled idle work clears its mark"
        );
        assert!(UnfinishedTurns::marked(metadata.as_ref())
            .await
            .unwrap()
            .is_empty());
        let child_live = service
            .resolve(&child_id, &caller("inspect-child"))
            .await
            .unwrap();
        assert_eq!(child_live.agent.owed_turns().await.len(), 1);
        service.mark_unfinished(&child_id).await.unwrap();
        // A non-cascading Stop never paid for the child's continuation.
        if cascade {
            assert!(service.resume_owed(&child_id, child_live.clone()).await);
            assert!(child_live.agent.owed_turns().await.is_empty());
        } else {
            assert!(service
                .captured_stop_actor(&child_id, "child", &caller("inspect-child"))
                .await
                .unwrap()
                .is_none());
        }
        assert_eq!(
            provider.cancel_calls.load(Ordering::SeqCst),
            0,
            "historical reconciliation sends no native cancel"
        );
        assert!(
            provider.executions.lock().unwrap().is_empty(),
            "confirmed stopped work never gets a successor"
        );
        finish_turn(&service, &provider, &target, "person-next").await;
        assert!(provider
            .executions
            .lock()
            .unwrap()
            .iter()
            .any(|id| id == "person-next"));
        drop((live, child_live));
        retire(service, storage, metadata).await;
    }
}

#[tokio::test]
async fn an_unconfirmed_stop_remains_uncertain_after_process_loss() {
    for stage in ["accepted", "attempted"] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("data");
        let target = id();
        crash_after_stop(&root, &target, false, stage).await;
        let provider = Arc::new(ProviderFactory::default());
        provider.turn_cancel.store(true, Ordering::SeqCst);
        let (service, storage, metadata) = fixture(&root, provider.clone());
        bind(&service, &storage, &root, true).await;
        service.bind_unfinished_turns(metadata.clone(), FailureBackoff::default());
        service.mark_unfinished(&target).await.unwrap();
        let live = service
            .resolve(&target, &caller("read-pending"))
            .await
            .unwrap();
        assert!(!service.resume_owed(&target, live.clone()).await);
        assert_eq!(live.agent.owed_turns().await.len(), 1);
        assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 0);
        assert!(provider.executions.lock().unwrap().is_empty());
        assert_eq!(
            UnfinishedTurns::marked(metadata.as_ref())
                .await
                .unwrap()
                .len(),
            1
        );
        drop(live);
        retire(service, storage, metadata).await;
    }
}
