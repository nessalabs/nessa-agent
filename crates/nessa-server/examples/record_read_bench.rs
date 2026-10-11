//! Time cold and warm record reads through the gateway's record read adapter,
//! and the SDK saves that wrote them. Not run in CI.
//!
//! ```text
//! cargo run --release -p nessa-server --example record_read_bench -- \
//!     [--sizes 20,200,2000] [--turns-per-save 1] [--assistant-bytes N]
//! ```
//!
//! For each conversation size N (messages; half user, half assistant, with
//! realistic lengths, or every answer `--assistant-bytes` long) it writes one
//! session through the public SDK `RecordStorage` lease, one save per
//! `--turns-per-save` turns, timing each save. It then shuts that storage down
//! and opens a fresh one on the same files, as a gateway restart does, and reads
//! through `NessaRecordReadSource`, the adapter behind `conversation.recordsHead`
//! and `conversation.recordsPage`, without the socket or admission in front of
//! it. Reads repeat immediately while the answer is `source_preparing`; the
//! example never sleeps between them, so the cold numbers are gateway work, not
//! a client's poll interval. A first page at that head follows, then warm head
//! and page repeats.
//!
//! The report is one JSON document on stdout; diagnostics go to stderr.
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::{domain::ConversationId, read_scope::ReceiverReadScope};
use nessa_sdk::{
    application::agent_execution::{
        executions::{ExecutionEvent, ExecutionRequest, ExecutionUpdate},
        permissions::ActionContext,
        providers::ProviderIdentity,
        sessions::{
            InvocationRecord, SessionChange, SessionSaveUnit, SessionSnapshot, SessionStorage,
            SubmissionAcknowledgement,
        },
    },
    domain::agent_execution::{
        executions::{ExecutionId, ExecutionOutcome, MessageChunk, SubmissionMode},
        prompts::{PromptText, UserMessage},
        sessions::{ExecutionSessionId, ProviderContext, SessionId},
    },
    infrastructure::session_storage::{RecordStorage, MAX_PHYSICAL_RECORD_PAYLOAD_BYTES},
};
use nessa_server::conversation::{
    application::{
        RecordReadError, RecordReadLease, RecordReadOperation, RecordReadSource, RecordReadValue,
    },
    infrastructure::NessaRecordReadSource,
};
use nessa_sync::replication::domain::{Id, PageRequest, Scope};
use serde_json::{json, Value};
use std::{
    error::Error,
    io::{self, Write},
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::runtime::Handle;

const CONVERSATION: &str = "6f1c1d2e-3b4a-4c5d-8e9f-0a1b2c3d4e5f";

const WARM_REPEATS: usize = 20;

fn main() {
    tracing_subscriber::fmt()
        .with_writer(io::stderr)
        .with_ansi(false)
        .init();
    if let Err(error) = run() {
        tracing::error!(%error, "Record read benchmark failed");
        std::process::exit(1);
    }
}

struct Arguments {
    sizes: Vec<usize>,
    turns_per_save: usize,
    assistant_bytes: Option<usize>,
}

fn arguments() -> Result<Arguments, Box<dyn Error>> {
    let mut parsed = Arguments {
        sizes: vec![20, 200, 2000],
        turns_per_save: 1,
        assistant_bytes: None,
    };
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let value = args.next().ok_or("every flag takes a value")?;
        match flag.as_str() {
            "--sizes" => {
                parsed.sizes = value.split(',').map(str::parse).collect::<Result<_, _>>()?;
                // Each turn is one user and one assistant message.
                if parsed.sizes.iter().any(|size| *size == 0 || size % 2 != 0) {
                    return Err("--sizes takes positive even message counts".into());
                }
            }
            "--turns-per-save" => parsed.turns_per_save = value.parse::<usize>()?.max(1),
            "--assistant-bytes" => parsed.assistant_bytes = Some(value.parse()?),
            _ => return Err(format!("unknown flag {flag}").into()),
        }
    }
    Ok(parsed)
}

fn run() -> Result<(), Box<dyn Error>> {
    let arguments = arguments()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let mut sizes = Vec::new();
    for &size in &arguments.sizes {
        tracing::info!(size, "Measuring conversation");
        sizes.push(runtime.block_on(measure(size, &arguments))?);
    }
    let report = json!({
        "machine": machine(),
        "turnsPerSave": arguments.turns_per_save,
        "assistantBytes": arguments.assistant_bytes,
        "sizes": sizes,
    });
    let mut output = io::stdout().lock();
    serde_json::to_writer_pretty(&mut output, &report)?;
    writeln!(output)?;
    Ok(())
}

fn machine() -> Value {
    let load = std::process::Command::new("sysctl")
        .args(["-n", "vm.loadavg"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty());
    json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "cpus": std::thread::available_parallelism().map(usize::from).unwrap_or(0),
        "loadAverage": load,
    })
}

async fn measure(size: usize, arguments: &Arguments) -> Result<Value, Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("records");
    let conversation = ConversationId::new(CONVERSATION)?;
    let id = SessionId::new(CONVERSATION)?;
    let saves = seed(&root, &id, size, arguments).await?;
    let disk = directory_bytes(&root)?;

    // A gateway restart: nothing of the writer's storage survives in memory.
    let storage = Arc::new(RecordStorage::new(&root)?);
    let source =
        NessaRecordReadSource::new(storage.clone(), sid("gateway-origin"), Handle::current());
    let admitted = ReceiverReadScope {
        receiver_id: "receiver".into(),
        organization_id: OrganizationId::new("organization")?,
        owner_id: PrincipalId::new("owner")?,
        conversation_id: conversation,
        access_epoch: 3,
    };
    let started = Instant::now();
    let cold = read_until_ready(&source, &admitted, || RecordReadOperation::Head).await?;
    let RecordReadValue::Head(head) = cold.value else {
        return Err("a head read answers a head".into());
    };
    let first_page = || RecordReadOperation::Page(page(head.scope.clone(), head.head));
    let cold_page = read_until_ready(&source, &admitted, first_page).await?;
    let first_page_ms = millis(started.elapsed());

    let mut warm_head = Vec::new();
    let mut warm_page = Vec::new();
    for _ in 0..WARM_REPEATS {
        warm_head.push(
            read_until_ready(&source, &admitted, || RecordReadOperation::Head)
                .await?
                .elapsed,
        );
        warm_page.push(
            read_until_ready(&source, &admitted, first_page)
                .await?
                .elapsed,
        );
    }
    source
        .shutdown()
        .await
        .map_err(|error| format!("{error:?}"))?;
    drop(source);
    let storage = Arc::into_inner(storage).ok_or("bench retained a storage clone")?;
    storage.shutdown().await?;
    Ok(json!({
        "messages": size,
        "physicalRecords": head.head,
        "save": {
            "count": saves.len(),
            "p50Ms": percentile(&saves, 50),
            "p95Ms": percentile(&saves, 95),
            "maxMs": percentile(&saves, 100),
            "totalMs": millis(saves.iter().sum()),
        },
        "disk": {
            "bytes": disk,
            "bytesPerMessage": disk / size.max(1) as u64,
        },
        "cold": {
            "headReads": cold.attempts,
            "sourcePreparing": cold.attempts - 1,
            "untilHeadMs": millis(cold.elapsed),
            "pageReads": cold_page.attempts,
            "untilFirstPageMs": first_page_ms,
        },
        "warm": {
            "headP50Ms": percentile(&warm_head, 50),
            "pageP50Ms": percentile(&warm_page, 50),
        },
    }))
}

/// Write `size` messages, one save per `turns_per_save` turns, as the gateway
/// would: an open save, then each turn's input, observations and settlement.
async fn seed(
    root: &Path,
    id: &SessionId,
    size: usize,
    arguments: &Arguments,
) -> Result<Vec<Duration>, Box<dyn Error>> {
    let storage = RecordStorage::new(root)?;
    storage.initialize().await?;
    let lease = storage.open(id.clone()).await?;
    let provider = ProviderIdentity::new("claude", "claude-sonnet-5", "workspace")?;
    let context = ProviderContext::Recorded(ExecutionSessionId::new("bench-provider-session")?);
    let mut snapshot = SessionSnapshot {
        id: id.clone(),
        provider: provider.clone(),
        provider_context: context.clone(),
        invocations: Vec::new(),
        queue_history: Vec::new(),
        lease: None,
        artifacts: Vec::new(),
    };
    let mut binding = lease.load().await?.binding().clone();
    let opened = SessionSaveUnit::new(vec![SessionChange::Opened {
        id: id.clone(),
        provider,
        context,
    }])?;
    binding = lease
        .save_changes(binding, snapshot.clone(), vec![opened])
        .await?
        .next()
        .clone();
    let mut saves = Vec::new();
    let mut units = Vec::new();
    for turn in 0..size / 2 {
        let record = turn_record(turn, arguments.assistant_bytes)?;
        units.push(turn_unit(&record)?);
        snapshot.invocations.push(record);
        if units.len() == arguments.turns_per_save || turn + 1 == size / 2 {
            let next = snapshot.clone();
            let changes = std::mem::take(&mut units);
            let started = Instant::now();
            let receipt = lease.save_changes(binding, next, changes).await?;
            saves.push(started.elapsed());
            binding = receipt.next().clone();
        }
    }
    drop(lease);
    storage.shutdown().await?;
    Ok(saves)
}

struct Ready {
    attempts: usize,
    elapsed: Duration,
    value: RecordReadValue,
}

/// Repeat one adapter read until it is not `source_preparing`, as a client
/// retries, but without any interval between reads.
async fn read_until_ready(
    source: &NessaRecordReadSource,
    admitted: &ReceiverReadScope,
    operation: impl Fn() -> RecordReadOperation,
) -> Result<Ready, Box<dyn Error>> {
    let started = Instant::now();
    let mut attempts = 0;
    loop {
        attempts += 1;
        match source
            .read(admitted.clone(), operation(), RecordReadLease::new(()))
            .await
        {
            Ok(response) => {
                return Ok(Ready {
                    attempts,
                    elapsed: started.elapsed(),
                    value: response.value,
                })
            }
            Err(RecordReadError::SourcePreparing) => {}
            Err(error) => return Err(format!("{error:?}").into()),
        }
    }
}

fn page(scope: Scope, target: u64) -> PageRequest {
    PageRequest {
        scope,
        after: 0,
        target,
        max_records: 64,
        max_payload_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
        max_record_bytes: MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
    }
}

fn sid(value: &str) -> Id {
    Id::new(value).expect("fixed bench identifiers are valid")
}

fn turn_record(
    turn: usize,
    assistant_bytes: Option<usize>,
) -> Result<InvocationRecord, Box<dyn Error>> {
    let id = ExecutionId::new(format!("turn-{turn:06}"))?;
    let user = filler(turn, 80 + (turn * 37) % 320, "Question");
    let length = assistant_bytes.unwrap_or(400 + (turn * 211) % 2000);
    let assistant = filler(turn, length, "Answer");
    Ok(InvocationRecord {
        target_event_offset: None,
        submission: SubmissionMode::Immediate,
        request: ExecutionRequest {
            execution_id: id.clone(),
            user_message: UserMessage::text_only(PromptText::new(user)?),
            estimated_input_tokens: 1,
            reserved_output_tokens: 1,
        },
        actor: ActionContext::new("bench", "bench", format!("turn-{turn}"))?,
        acknowledgement: SubmissionAcknowledgement::Pending,
        events: vec![
            ExecutionEvent::new(
                id.clone(),
                ExecutionUpdate::Message(MessageChunk::text(assistant)),
            ),
            ExecutionEvent::new(id, ExecutionUpdate::Finished(ExecutionOutcome::Completed)),
        ],
        scheduling: Vec::new(),
        provider_report: None,
        local_cancellation: None,
        local_outcome: Some(ExecutionOutcome::Completed),
        cancellation: None,
        result: Some(Ok(ExecutionOutcome::Completed)),
    })
}

/// The decisions that saved `record`, in the order the SDK writes them.
fn turn_unit(record: &InvocationRecord) -> Result<SessionSaveUnit, Box<dyn Error>> {
    let mut input = record.clone();
    input.events.clear();
    input.result = None;
    input.local_outcome = None;
    let mut changes = vec![SessionChange::InputAccepted(Box::new(input))];
    changes.extend(
        record
            .events
            .iter()
            .cloned()
            .map(SessionChange::ProviderObservation),
    );
    changes.push(SessionChange::LocalSettlement {
        execution_id: record.request.execution_id.clone(),
        before: None,
        after: Ok(ExecutionOutcome::Completed),
        local_outcome: Some(ExecutionOutcome::Completed),
    });
    Ok(SessionSaveUnit::new(changes)?)
}

fn filler(turn: usize, length: usize, label: &str) -> String {
    const WORDS: [&str; 12] = [
        "gateway",
        "transcript",
        "receiver",
        "epoch",
        "catalogue",
        "device",
        "sync",
        "record",
        "page",
        "fold",
        "cache",
        "turn",
    ];
    let mut text = format!("{label} {turn}:");
    let mut index = turn;
    while text.len() < length {
        text.push(' ');
        text.push_str(WORDS[index % WORDS.len()]);
        index = index.wrapping_mul(31).wrapping_add(7);
    }
    text.truncate(length);
    text
}

fn directory_bytes(root: &Path) -> io::Result<u64> {
    let mut total = 0;
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        total += if metadata.is_dir() {
            directory_bytes(&entry.path())?
        } else {
            metadata.len()
        };
    }
    Ok(total)
}

fn millis(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 1_000_000.0).round() / 1000.0
}

fn percentile(samples: &[Duration], percent: usize) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort();
    match sorted.len() {
        0 => 0.0,
        len => millis(sorted[(len - 1) * percent / 100]),
    }
}
