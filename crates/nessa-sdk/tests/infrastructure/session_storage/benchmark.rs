//! Manual deterministic workload for persistence save and restoration measurements.
use super::*;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};

struct Measurement {
    invocations: usize,
    checkpoints: usize,
    journal_bytes: u64,
    save_elapsed: Duration,
    restore_elapsed: Duration,
}

async fn measure_streamed_events(events: usize, checkpoint_every: usize) -> Measurement {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("private");
    let storage = LocalFileStorage::new(&directory).unwrap();
    let name = format!("persistence-stream-{events}");
    let lease = storage.open(id(&name)).await.unwrap();
    let mut value = snapshot(&name);
    value.invocations[0].events.clear();
    let execution_id = value.invocations[0].request.execution_id.clone();
    let mut save_elapsed = Duration::ZERO;
    let mut checkpoints = 0;

    for index in 0..events {
        value.invocations[0].events.push(ExecutionEvent::new(
            execution_id.clone(),
            ExecutionUpdate::Message(MessageChunk::text(format!("chunk-{index}"))),
        ));
        if (index + 1) % checkpoint_every == 0 || index + 1 == events {
            let started = Instant::now();
            lease.save(value.clone()).await.unwrap();
            save_elapsed += started.elapsed();
            checkpoints += 1;
        }
    }

    let started = Instant::now();
    let restored = lease.load().await.unwrap().unwrap();
    let restore_elapsed = started.elapsed();
    assert_same(&restored, &value);
    Measurement {
        invocations: value.invocations.len(),
        checkpoints,
        journal_bytes: std::fs::metadata(journal_path(&directory, &name))
            .unwrap()
            .len(),
        save_elapsed,
        restore_elapsed,
    }
}

fn report(label: &str, result: &Measurement) {
    eprintln!(
        "workload={label} invocations={} checkpoints={} journal_bytes={} save_ms={:.3} restore_ms={:.3}",
        result.invocations,
        result.checkpoints,
        result.journal_bytes,
        result.save_elapsed.as_secs_f64() * 1_000.0,
        result.restore_elapsed.as_secs_f64() * 1_000.0,
    );
}

async fn measure(invocations: usize, checkpoint_every: usize) -> Measurement {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("private");
    let storage = LocalFileStorage::new(&directory).unwrap();
    let name = format!("persistence-scale-{invocations}");
    let lease = storage.open(id(&name)).await.unwrap();
    let mut value = snapshot(&name);
    let template = value.invocations.pop().unwrap();
    let mut save_elapsed = Duration::ZERO;
    let mut checkpoints = 0;

    for index in 0..invocations {
        let execution_id = ExecutionId::new(format!("execution-{index}")).unwrap();
        let mut invocation = template.clone();
        invocation.request.execution_id = execution_id.clone();
        invocation.events = vec![ExecutionEvent::new(
            execution_id,
            ExecutionUpdate::Message(MessageChunk::text(format!("output-{index}"))),
        )];
        value.invocations.push(invocation);
        if (index + 1) % checkpoint_every == 0 || index + 1 == invocations {
            let started = Instant::now();
            lease.save(value.clone()).await.unwrap();
            save_elapsed += started.elapsed();
            checkpoints += 1;
        }
    }

    let started = Instant::now();
    let restored = lease.load().await.unwrap().unwrap();
    let restore_elapsed = started.elapsed();
    assert_same(&restored, &value);
    Measurement {
        invocations,
        checkpoints,
        journal_bytes: std::fs::metadata(journal_path(&directory, &name))
            .unwrap()
            .len(),
        save_elapsed,
        restore_elapsed,
    }
}

#[tokio::test]
#[ignore = "manual persistence performance measurement"]
async fn persistence_scale_with_repeated_checkpoints() {
    for (invocations, checkpoint_every) in [(100, 10), (1_000, 100), (10_000, 1_000)] {
        let result = measure(invocations, checkpoint_every).await;
        report(&format!("history-{invocations}"), &result);
    }

    let frequent = measure(1_000, 10).await;
    report("history-1000-frequent-checkpoints", &frequent);

    let streamed = measure_streamed_events(10_000, 100).await;
    report("one-turn-10000-streamed-chunks", &streamed);
}

#[tokio::test]
#[ignore = "manual concurrent persistence and scheduling measurement"]
async fn concurrent_session_persistence_and_scheduler_delay() {
    const SESSIONS: usize = 8;
    let (ready_send, mut ready_receive) = mpsc::channel(SESSIONS);
    let (release, _) = watch::channel(None::<Instant>);
    let mut tasks = Vec::new();
    for _ in 0..SESSIONS {
        let ready_send = ready_send.clone();
        let mut release = release.subscribe();
        tasks.push(tokio::spawn(async move {
            ready_send.send(()).await.unwrap();
            release.wait_for(Option::is_some).await.unwrap();
            let scheduling_delay = release.borrow().expect("release instant").elapsed();
            (measure(100, 10).await, scheduling_delay)
        }));
    }
    drop(ready_send);
    for _ in 0..SESSIONS {
        ready_receive.recv().await.unwrap();
    }
    let wall_started = Instant::now();
    release.send_replace(Some(Instant::now()));

    let mut delays = Vec::new();
    let mut journal_bytes = 0;
    for task in tasks {
        let (result, delay) = task.await.unwrap();
        assert_eq!(result.invocations, 100);
        assert_eq!(result.checkpoints, 10);
        journal_bytes += result.journal_bytes;
        delays.push(delay);
    }
    delays.sort_unstable();
    eprintln!(
        "workload=concurrent-sessions sessions={SESSIONS} invocations_per_session=100 checkpoints_per_session=10 journal_bytes={} wall_ms={:.3} scheduling_delay_median_us={} scheduling_delay_max_us={}",
        journal_bytes,
        wall_started.elapsed().as_secs_f64() * 1_000.0,
        delays[SESSIONS / 2].as_micros(),
        delays.last().unwrap().as_micros(),
    );
}
