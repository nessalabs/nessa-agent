//! Manual scaling benchmark over the production library (no private test probes).
use nessa_local_database::rusqlite::Connection;
use nessa_sdk::application::agent_execution::subagents::OwnershipStore;
use nessa_sdk::domain::agent_execution::{
    sessions::SessionId,
    subagents::{
        AgentLifetimeId, CloseOperationId, EvidenceFact, Initiator, LifetimeCause, LifetimeState,
        OwnershipGraph, PhysicalFact,
    },
};
use nessa_sdk::infrastructure::session_storage::SqliteOwnershipStore;
use std::{io::Write, sync::Arc, time::Instant};

fn private_directory() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    nessa_local_storage::create_directory(&directory.path().join("private")).unwrap();
    directory
}

#[test]
#[ignore = "manual local SQLite scaling measurements; writes raw CSV"]
fn benchmark_single_store_root_rows() {
    let output = std::env::var("NESSA_627_BENCH_OUT")
        .unwrap_or_else(|_| "/tmp/627-sqlite-worker.csv".into());
    let samples: usize = std::env::var("NESSA_627_BENCH_SAMPLES")
        .ok()
        .map(|value| value.parse().unwrap())
        .unwrap_or(1000);
    let batch_output = std::env::var("NESSA_627_BENCH_BATCH_OUT")
        .unwrap_or_else(|_| format!("{output}.batches.csv"));
    let mut raw = std::fs::File::create(output).unwrap();
    let mut batch_raw = std::fs::File::create(batch_output).unwrap();
    writeln!(batch_raw, "roots,closed,lifetimes,spawns,settlements,reports,live,body_bytes,instances,scheduled_callers,batch,wall_elapsed_ns").unwrap();
    writeln!(raw, "roots,closed,lifetimes,spawns,settlements,reports,live,body_bytes,instances,concurrency,operation,sample,elapsed_ns").unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(8)
        .enable_time()
        .build()
        .unwrap();
    for roots in [0, 32, 256, 2048] {
        for closed in [false, true] {
            if roots == 0 && closed {
                continue;
            }
            let mut graph = OwnershipGraph::new();
            for index in 0..roots {
                let life = AgentLifetimeId::new(format!("root-{index}")).unwrap();
                let _evidence = graph
                    .open_root(
                        SessionId::new(format!("session-{index}")).unwrap(),
                        life.clone(),
                        Initiator::Runtime,
                    )
                    .unwrap();
                if closed {
                    let operation = CloseOperationId::new(format!("close-{index}")).unwrap();
                    let _admission = graph
                        .begin_close(
                            &life,
                            operation.clone(),
                            LifetimeCause::OwnerDisposed,
                            Initiator::Runtime,
                        )
                        .unwrap();
                    let _evidence = graph
                        .apply_report(
                            &life,
                            &operation,
                            &life,
                            PhysicalFact::Released,
                            EvidenceFact::Acknowledged,
                        )
                        .unwrap();
                    for record in graph.pending_close_evidence(&life) {
                        graph
                            .acknowledge_observation(&record, EvidenceFact::Acknowledged)
                            .unwrap();
                    }
                    let completion = graph.prepare_completion(&life, &operation).unwrap();
                    graph
                        .acknowledge_completion(&completion, EvidenceFact::Acknowledged)
                        .unwrap();
                }
            }
            let snapshot = graph.snapshot();
            assert!(OwnershipGraph::restore(snapshot.clone())
                .refusal()
                .is_none());

            let expected_state = if closed {
                LifetimeState::Closed
            } else {
                LifetimeState::Open
            };
            assert!(snapshot
                .lifetimes
                .iter()
                .all(|row| row.state == expected_state));
            let live = snapshot
                .lifetimes
                .iter()
                .filter(|row| row.state == LifetimeState::Open)
                .count();
            let directory = private_directory();
            let store = Arc::new(
                SqliteOwnershipStore::open(&directory.path().join("private/ownership.sqlite3"))
                    .unwrap(),
            );
            let snapshot = Arc::new(snapshot);
            runtime.block_on(store.write(&snapshot)).unwrap();
            let connection =
                Connection::open(directory.path().join("private/ownership.sqlite3")).unwrap();
            let body: String = connection
                .query_row(
                    "SELECT body FROM ownership_snapshot WHERE id = 1",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            let body_bytes = body.len();
            drop(connection);
            runtime.block_on(async {
                for _ in 0..50 { store.write(&snapshot).await.unwrap(); store.read().await.unwrap(); }
                for sample in 0..samples {
                    let start = Instant::now(); store.write(&snapshot).await.unwrap();
                    writeln!(raw, "{roots},{closed},{},{},{},{},{live},{body_bytes},1,1,write,{sample},{}", snapshot.lifetimes.len(), snapshot.spawns.len(), snapshot.settlements.len(), snapshot.reports.len(), start.elapsed().as_nanos()).unwrap();
                    let start = Instant::now(); let loaded = store.read().await.unwrap();
                    let elapsed = start.elapsed().as_nanos();
                    assert_eq!(loaded.lifetimes.len(), roots);
                    writeln!(raw, "{roots},{closed},{},{},{},{},{live},{body_bytes},1,1,read,{sample},{elapsed}", snapshot.lifetimes.len(), snapshot.spawns.len(), snapshot.settlements.len(), snapshot.reports.len()).unwrap();
                }
                // Four scheduled callers through one instance.
                for batch in 0..(samples / 4) {
                    // Wall scope includes scheduling: before spawning until all complete.
                    let batch_start = Instant::now();
                    let mut calls = Vec::new();
                    for caller in 0..4 {
                        let store = store.clone(); let snapshot = snapshot.clone();
                        calls.push(tokio::spawn(async move { let start = Instant::now(); store.write(&snapshot).await.unwrap(); (batch * 4 + caller, start.elapsed().as_nanos()) }));
                    }
                    let mut completed = Vec::new();
                    for call in calls { completed.push(call.await.unwrap()); }
                    let batch_elapsed = batch_start.elapsed().as_nanos();
                    for (sample, elapsed) in completed {
                        writeln!(raw, "{roots},{closed},{},{},{},{},{live},{body_bytes},1,4,write,{sample},{elapsed}", snapshot.lifetimes.len(), snapshot.spawns.len(), snapshot.settlements.len(), snapshot.reports.len()).unwrap();
                    }
                    writeln!(batch_raw, "{roots},{closed},{},{},{},{},{live},{body_bytes},1,4,{batch},{batch_elapsed}", snapshot.lifetimes.len(), snapshot.spawns.len(), snapshot.settlements.len(), snapshot.reports.len()).unwrap();
                }
            });
        }
    }
}
