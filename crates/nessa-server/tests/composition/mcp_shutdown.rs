//! The gateway's MCP stop as its cleanup runs it (`cleanup_product`, then
//! `composition::mcp_servers::stop`): admission closes first, admitted
//! writes drain to their outcome records before the servers stop, and a
//! drain that runs out of time is a shutdown that does not confirm.
use super::*;
use crate::core::{Outcome, ShutdownStage};
use crate::mcp_servers::application::{
    InspectCut, Inspection, LiveSetOutcome, McpServerAuditPhase, McpServerCause, McpServerOutcome,
    McpServerSettingsError,
};
use crate::mcp_servers::domain::{ServerEdit, ServerSave};
use crate::mcp_servers::infrastructure::settings_test_support::{
    config, entry, initiator, inspected_over, server, settings_for, settings_over, LeapingClock,
    ManualClock, MemoryFiles, RecordingAudit, ScriptedInspector,
};
use crate::product::HostWatchFixture;
use nessa_sdk::infrastructure::mcp::McpError;
use std::sync::atomic::Ordering;

fn save(name: &str) -> ServerEdit {
    ServerEdit::Save(ServerSave {
        previous_name: None,
        server: server(name),
        env: vec![],
        enabled: true,
    })
}

/// Wait, five real seconds at most, until `done`.
async fn within(what: &str, done: impl Fn() -> bool) {
    let started = std::time::Instant::now();
    while !done() {
        assert!(started.elapsed() < Duration::from_secs(5), "never: {what}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// `task`'s answer, within ten real seconds: a test fails rather than hangs.
async fn joined<T>(task: tokio::task::JoinHandle<T>) -> T {
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("answered in time")
        .unwrap()
}

fn stage(report: &ReportSlot) -> Option<ShutdownStage> {
    report.lock().unwrap().as_ref().map(ShutdownReport::stage)
}

/// LS3c and LS3d through the gateway's cleanup: a save held in its publish
/// when shutdown begins. Admission closes before anything is drained — a
/// later save is `stopping` — the MCP stop waits for the held save while
/// the servers keep running, and the save's outcome is recorded with its
/// live set replaced before the servers stop; then the client refuses a
/// replacement, and the report has the MCP stop `Ok`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_mcp_stop_drains_admitted_writes_before_the_servers_stop() {
    let fixture = HostWatchFixture::idle().await;
    let files = MemoryFiles::holding(config(vec![]));
    let audit = Arc::new(RecordingAudit::default());
    let (settings, servers) = settings_for(files.clone(), audit.clone());
    let settings = Arc::new(settings);
    let state = fixture.state().with_mcp_server_settings(settings.clone());
    let revision = settings.list().await.unwrap().revision;
    let (release, gate) = std::sync::mpsc::channel();
    *files.publish_gate.lock().unwrap() = Some(gate);
    let editing = tokio::spawn({
        let settings = settings.clone();
        let revision = revision.clone();
        async move { settings.edit(initiator(), revision, save("a")).await }
    });
    within("the write starts", || {
        files.publishing.load(Ordering::SeqCst)
    })
    .await;
    let report: Arc<ReportSlot> = Arc::new(Mutex::new(None));
    let cleanup = tokio::spawn({
        let report = report.clone();
        let state = state.clone();
        let servers = servers.clone();
        async move {
            cleanup_product(
                &report,
                &state,
                async { Ok(()) },
                async { Ok(()) },
                None::<std::future::Ready<Result<(), ConversationError>>>,
                super::super::mcp_servers::stop(state.mcp_server_settings.as_deref(), &servers),
                std::future::ready(Ok(())),
                Duration::from_secs(30),
            )
            .await;
        }
    });
    within("the MCP stop is reached", || {
        stage(&report) == Some(ShutdownStage::Servers)
    })
    .await;
    // Admission closed: refused, with nothing recorded.
    assert_eq!(
        settings.edit(initiator(), revision, save("b")).await,
        Err(McpServerSettingsError::Stopping)
    );
    assert_eq!(audit.records().len(), 1);
    // The held save is waited for; the servers run on meanwhile.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!cleanup.is_finished(), "the stop did not wait for the save");
    assert_eq!(stage(&report), Some(ShutdownStage::Servers));
    assert_eq!(servers.replace(servers.configured()), Ok(()));
    release.send(()).unwrap();
    joined(cleanup).await;
    // Recorded, and its live set replaced, before the servers stopped.
    let records = audit.records();
    assert_eq!(records.len(), 2);
    assert!(matches!(
        &records[1].phase,
        McpServerAuditPhase::Outcome(McpServerOutcome::Applied {
            live_set: LiveSetOutcome::Replaced,
            ..
        })
    ));
    let live: Vec<_> = servers
        .configured()
        .into_iter()
        .map(|launch| launch.server.name)
        .collect();
    assert_eq!(live, ["a", "nessa"]);
    // Then stopped.
    assert_eq!(
        servers.replace(servers.configured()),
        Err(McpError::Stopped)
    );
    let servers_outcome = report.lock().unwrap().as_ref().unwrap().servers();
    assert_eq!(servers_outcome, Outcome::Ok);
    assert!(joined(editing).await.is_ok());
}

/// A drain that runs out of time is a shutdown failure: a save waiting on a
/// lock that is never let go, on a clock that never moves, is still running
/// at the drain's bound (reached on Tokio's paused clock). The servers stop
/// all the same; the report carries `Unfinished` for the MCP stop and does
/// not confirm.
#[tokio::test]
async fn an_unfinished_drain_is_an_unconfirmed_shutdown() {
    let fixture = HostWatchFixture::idle().await;
    let files = MemoryFiles::holding(config(vec![]));
    let audit = Arc::new(RecordingAudit::default());
    // The lock's wait is on this clock, which nothing advances.
    let (settings, servers) = settings_over(
        files.clone(),
        audit.clone(),
        Arc::new(ManualClock::default()),
    );
    let settings = Arc::new(settings);
    let state = fixture.state().with_mcp_server_settings(settings.clone());
    let revision = settings.list().await.unwrap().revision;
    files.held.store(true, Ordering::SeqCst);
    let _editing = tokio::spawn({
        let settings = settings.clone();
        async move { settings.edit(initiator(), revision, save("a")).await }
    });
    within("the save is under way", || audit.records().len() == 1).await;
    tokio::time::pause();
    let report: ReportSlot = Mutex::new(None);
    let started = tokio::time::Instant::now();
    cleanup_product(
        &report,
        &state,
        async { Ok(()) },
        async { Ok(()) },
        None::<std::future::Ready<Result<(), ConversationError>>>,
        super::super::mcp_servers::stop(state.mcp_server_settings.as_deref(), &servers),
        std::future::ready(Ok(())),
        Duration::from_secs(30),
    )
    .await;
    // At the bound, give or take the timer's millisecond.
    let took = started.elapsed();
    assert!(
        took >= settings.drain_bound() && took < settings.drain_bound() + Duration::from_secs(1),
        "{took:?}"
    );
    {
        let report = report.lock().unwrap();
        let report = report.as_ref().unwrap();
        assert_eq!(
            report.servers(),
            Outcome::Failed(crate::mcp_servers::application::Unfinished { running: 1 })
        );
        assert!(report.native().is_ok(), "cleanup went on past the drain");
        assert!(!report.confirmed());
    }
    assert_eq!(
        servers.replace(servers.configured()),
        Err(McpError::Stopped)
    );
    assert!(shutdown_result(&report).is_err());
}

/// X-early, through the gateway's cleanup: shutdown begins while a
/// conversation is still draining. Admission closes at once — a later save
/// or inspection is `stopping`, unaudited — and the inspection already
/// running is cut and recorded now, while the conversations have not yet
/// drained and the MCP stop has not begun; only then do the servers stop.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn x_early_admission_closes_and_inspections_are_cut_while_conversations_drain() {
    let fixture = HostWatchFixture::idle().await;
    let files = MemoryFiles::holding(config(vec![entry("a")]));
    let audit = Arc::new(RecordingAudit::default());
    let inspector = Arc::new(ScriptedInspector::default());
    // Held until stopped: the inspection never answers on its own.
    inspector
        .gate
        .forget_permits(tokio::sync::Semaphore::MAX_PERMITS);
    let (settings, servers) = inspected_over(
        files.clone(),
        audit.clone(),
        Arc::new(LeapingClock::default()),
        inspector.clone(),
    );
    let settings = Arc::new(settings);
    let state = fixture.state().with_mcp_server_settings(settings.clone());
    let revision = settings.list().await.unwrap().revision;
    let inspecting = tokio::spawn({
        let settings = settings.clone();
        async move { settings.inspect(initiator(), "a").await }
    });
    within("the server is started", || {
        !inspector.asked.lock().unwrap().is_empty()
    })
    .await;
    let (drained, draining) = tokio::sync::oneshot::channel::<()>();
    let report: Arc<ReportSlot> = Arc::new(Mutex::new(None));
    let cleanup = tokio::spawn({
        let report = report.clone();
        let state = state.clone();
        let servers = servers.clone();
        async move {
            cleanup_product(
                &report,
                &state,
                async { Ok(()) },
                async { Ok(()) },
                Some(async move {
                    let _ = draining.await;
                    Ok::<(), ConversationError>(())
                }),
                super::super::mcp_servers::stop(state.mcp_server_settings.as_deref(), &servers),
                std::future::ready(Ok(())),
                Duration::from_secs(30),
            )
            .await;
        }
    });
    // The inspection is cut and recorded while the conversations drain.
    let answered = joined(inspecting).await;
    assert_eq!(
        answered,
        Ok(Inspection {
            tools: vec![],
            cut: Some(InspectCut::Stopping),
        })
    );
    let records = audit.records();
    assert_eq!(records.len(), 2);
    assert_eq!(
        records[1].phase,
        McpServerAuditPhase::Outcome(McpServerOutcome::Inspected {
            tools: 0,
            cut: Some(InspectCut::Stopping),
        })
    );
    assert_eq!(records[1].cause, McpServerCause::GatewayStopping);
    // Admission closed: refused, with nothing recorded.
    assert_eq!(
        settings.edit(initiator(), revision, save("b")).await,
        Err(McpServerSettingsError::Stopping)
    );
    assert_eq!(
        settings.inspect(initiator(), "a").await,
        Err(McpServerSettingsError::Stopping)
    );
    assert_eq!(audit.records().len(), 2);
    assert!(
        !cleanup.is_finished(),
        "cleanup did not wait for the conversations"
    );
    // The MCP stop has not begun: the servers still take a replacement.
    assert_eq!(servers.replace(servers.configured()), Ok(()));
    drained.send(()).unwrap();
    joined(cleanup).await;
    assert_eq!(
        servers.replace(servers.configured()),
        Err(McpError::Stopped)
    );
    assert_eq!(
        report.lock().unwrap().as_ref().unwrap().servers(),
        Outcome::Ok
    );
}
