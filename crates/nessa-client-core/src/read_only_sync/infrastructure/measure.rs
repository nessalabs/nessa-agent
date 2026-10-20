//! Weak-link measurement against a real SDK record source and the private cache.
//!
//! Payload bytes are the record payloads the cache kept. The pace wait is the
//! schedule's, on a virtual clock. Relay preamble bytes are the encoded
//! reader `Connect` and `Joined` frames and are not payload. Nothing here is
//! a phone radio, TLS, or a sampled production percentile.
use crate::read_only_sync::application::driver::run_records;
use crate::read_only_sync::application::link::{conservative_quote, run_quantum};
use crate::read_only_sync::application::CachePolicy;
use crate::read_only_sync::domain::{
    Admit, Billing, Candidate, Deferral, Interest, LinkSchedule, LinkSpec, Presence, Quote, Route,
    Settle, Subscription,
};
use crate::read_only_sync::infrastructure::cache::ReadOnlyCache;
use nessa_auth::application::ports::Clock;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::relay::RelayFrame;
use nessa_sdk::application::agent_execution::{
    providers::ProviderIdentity,
    sessions::{SessionChange, SessionSaveUnit, SessionStorage},
};
use nessa_sdk::domain::agent_execution::sessions::{
    ExecutionSessionId, ProviderContext, SessionId,
};
use nessa_sdk::domain::common::value_objects::Sha256Digest;
use nessa_sdk::infrastructure::session_storage::{
    RecordStorage, MAX_PHYSICAL_RECORD_PAYLOAD_BYTES,
};
use nessa_sync::replication::application::{Access, RecordSource, ScopeAuthorizer, SourceError};
use nessa_sync::replication::domain::{Id, Limits, Page, PageRequest, Record, Scope};
use nessa_sync::replication::infrastructure::MAX_PAGE_PAYLOAD;
use serde_json::{json, Value};
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

const OPEN: &str = "00000000-0000-4000-8000-000000000001";
const PACE: u64 = 8_000;

struct FixedClock;
impl Clock for FixedClock {
    fn unix_milliseconds(&self) -> u64 {
        123_000
    }
}

struct Allowed;
impl ScopeAuthorizer for Allowed {
    fn authorize(&mut self, scope: &Scope) -> Access {
        Access::Allowed(scope.clone())
    }
}

struct CountingSource {
    records: Vec<Record>,
    /// Payload bytes returned to the driver.
    payload: u64,
}
impl RecordSource for CountingSource {
    fn head(&mut self, _: &Scope) -> Result<u64, SourceError> {
        Ok(self.records.last().map_or(0, |record| record.position))
    }
    fn page(&mut self, request: &PageRequest) -> Result<Page, SourceError> {
        let records: Vec<Record> = self
            .records
            .iter()
            .filter(|record| record.position > request.after && record.position <= request.target)
            .take(request.max_records)
            .cloned()
            .collect();
        self.payload += records
            .iter()
            .map(|record| record.payload.len() as u64)
            .sum::<u64>();
        Ok(Page {
            request: request.clone(),
            records,
        })
    }
}

/// The harness report. One JSON object: profiles, the size table, and the
/// rows that copy nothing.
pub(crate) fn report() -> Value {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("measurement runtime");
    runtime.block_on(report_async())
}

async fn report_async() -> Value {
    let root = std::env::temp_dir().join(format!("nessa-weak-link-{}", uuid::Uuid::new_v4()));
    let _remove = Remove(&root);
    nessa_local_storage::create_directory(&root).expect("measurement directory");
    let (scope, records) = source_records(&root).await;
    let unpaced = copy(&root, &scope, &records, None);
    let paced = copy(&root, &scope, &records, NonZeroU64::new(PACE));
    let profiles_match = unpaced["payloadBytes"] == paced["payloadBytes"]
        && unpaced["downloaded"] == paced["downloaded"]
        && unpaced["applied"] == paced["applied"]
        && unpaced["reopenDownloaded"] == unpaced["downloaded"]
        && paced["reopenApplied"] == paced["applied"];
    let table = size_table();
    let waits = table
        .iter()
        .map(|row| row["waitMs"].as_u64().expect("wait"))
        .collect::<Vec<_>>();
    json!({
        "unit": "payload bytes",
        "clock": "pace waits are the schedule's virtual clock; wallMs is process time and is not a percentile",
        "profiles": [unpaced, paced],
        "profilesMatch": profiles_match,
        "sizeTable": table,
        "orderStatistics": {
            "method": "nearest rank on the size table, ceil(percent * n / 100) - 1",
            "of": "the 18 pace waits, not a sampled link",
            "p50Ms": nearest_rank(&waits, 50),
            "p95Ms": nearest_rank(&waits, 95),
        },
        "experiments": experiments(page_bytes()),
        "relayPreambleBytes": relay_preamble_bytes(),
        "notMeasured": [
            "phone radio",
            "operating-system sleep",
            "TLS",
            "IP billing",
            "sampled production lag",
            "fragment reassembly",
        ],
    })
}

struct Remove<'a>(&'a Path);
impl Drop for Remove<'_> {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.0);
    }
}

fn copy(root: &Path, scope: &Scope, records: &[Record], pace: Option<NonZeroU64>) -> Value {
    let name = match pace {
        None => "unpaced.sqlite3",
        Some(_) => "paced.sqlite3",
    };
    let path = cache_path(root, name);
    let mut cache = ReadOnlyCache::open(&path, policy(), Arc::new(FixedClock)).expect("cache");
    let mut source = CountingSource {
        records: records.to_vec(),
        payload: 0,
    };
    let open = ConversationId::new(OPEN).expect("conversation");
    let interest = Interest::OlderHistory(open.clone());
    let mut schedule = LinkSchedule::new(LinkSpec {
        route: Route::Direct,
        presence: Presence::Foreground,
        billing: Billing::Unmetered,
        pace_bytes_per_second: pace,
        open: Some(open),
        subscription: Subscription::Down,
    });
    let started = Instant::now();
    let mut virtual_ms = 0_u64;
    for _ in 0..64 {
        let saved = cache.cached_progress(scope).expect("progress");
        let head = source.head(scope).expect("head");
        if saved
            .as_ref()
            .is_some_and(|saved| saved.downloaded == head && saved.applied == head)
        {
            break;
        }
        let candidate = Candidate {
            interest: interest.clone(),
            quote: conservative_quote(&interest, page_bytes()),
        };
        let (next, admit, settled) = run_quantum(schedule, &[candidate], &mut |_| {
            let before = source.payload;
            run_records(
                scope,
                &mut Allowed,
                &mut source,
                &mut cache,
                policy().suffix_page(),
                1,
                &FixedClock,
            )
            .expect("page");
            source.payload - before
        });
        assert!(matches!(admit, Admit::Go { .. }), "{admit:?}");
        let Some(Settle::Settled { wait_ms, .. }) = settled else {
            panic!("a copied page settles");
        };
        virtual_ms += wait_ms;
        schedule = if wait_ms > 0 {
            next.pace_elapsed()
        } else {
            next
        };
    }
    let wall_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let saved = cache
        .cached_progress(scope)
        .expect("progress")
        .expect("copied");
    let payload = source.payload;
    drop(cache);
    let mut reopened = ReadOnlyCache::open(&path, policy(), Arc::new(FixedClock)).expect("reopen");
    let again = reopened
        .cached_progress(scope)
        .expect("reopen progress")
        .expect("reopened rows");
    json!({
        "name": if pace.is_some() { "paced-8000" } else { "unpaced" },
        "bytesPerSecond": pace.map(NonZeroU64::get),
        "payloadBytes": payload,
        "downloaded": saved.downloaded,
        "applied": saved.applied,
        "reopenDownloaded": again.downloaded,
        "reopenApplied": again.applied,
        "virtualLagMs": virtual_ms,
        "wallMs": wall_ms,
    })
}

fn page_bytes() -> u64 {
    u64::try_from(policy().suffix_page().max_payload_bytes()).expect("page bytes")
}

fn policy() -> CachePolicy {
    CachePolicy::new(
        32 * 1024 * 1024,
        8 * 1024 * 1024,
        Limits::new(16, MAX_PAGE_PAYLOAD, MAX_PHYSICAL_RECORD_PAYLOAD_BYTES).expect("page limits"),
    )
    .expect("cache policy")
}

fn cache_path(root: &Path, name: &str) -> PathBuf {
    let private = root.join("cache");
    nessa_local_storage::create_directory(&private).expect("cache directory");
    private.join(name)
}

fn size_table() -> Vec<Value> {
    let mut rows = Vec::new();
    for bytes in [1, 4, 8, 16, 32, 64].map(|kib| kib * 1024) {
        for rate in [2_000_u64, 8_000, 32_000] {
            let schedule = LinkSchedule::new(LinkSpec {
                route: Route::Direct,
                presence: Presence::Foreground,
                billing: Billing::Unmetered,
                pace_bytes_per_second: NonZeroU64::new(rate),
                open: Some(ConversationId::new(OPEN).expect("conversation")),
                subscription: Subscription::Up,
            });
            let candidate = Candidate {
                interest: Interest::OpenTranscript(
                    ConversationId::new(OPEN).expect("conversation"),
                ),
                quote: Quote::Known(bytes),
            };
            let (schedule, _) = schedule.admit(std::slice::from_ref(&candidate));
            let (_, settled) = schedule.finish(bytes);
            let Settle::Settled { wait_ms, .. } = settled else {
                panic!("size row settles");
            };
            rows.push(json!({
                "payloadBytes": bytes,
                "bytesPerSecond": rate,
                "waitMs": wait_ms,
            }));
        }
    }
    rows
}

fn nearest_rank(waits: &[u64], percent: u64) -> u64 {
    let mut sorted = waits.to_vec();
    sorted.sort_unstable();
    let index = (percent * sorted.len() as u64).div_ceil(100) - 1;
    sorted[usize::try_from(index).expect("index")]
}

fn experiments(page: u64) -> Vec<Value> {
    let open = ConversationId::new(OPEN).expect("conversation");
    let transcript = Candidate {
        interest: Interest::OpenTranscript(open.clone()),
        quote: conservative_quote(&Interest::OpenTranscript(open.clone()), page),
    };
    let artefact = Candidate {
        interest: Interest::Artifact {
            conversation: open.clone(),
            digest: Sha256Digest::from_bytes([4; 32]),
        },
        quote: conservative_quote(
            &Interest::Artifact {
                conversation: open.clone(),
                digest: Sha256Digest::from_bytes([4; 32]),
            },
            page,
        ),
    };
    let history_of = open.clone();
    let base = LinkSpec {
        route: Route::Direct,
        presence: Presence::Foreground,
        billing: Billing::Metered { remaining: page },
        pace_bytes_per_second: None,
        open: Some(open),
        subscription: Subscription::Up,
    };
    let decision = |schedule: LinkSchedule, candidates: &[Candidate]| {
        let calls = std::cell::Cell::new(0_u64);
        let (schedule, admit, _) = run_quantum(schedule, candidates, &mut |_| {
            calls.set(calls.get() + 1);
            0
        });
        let (name, winner) = match &admit {
            Admit::Defer(Deferral::Unreachable) => ("unreachable", "none"),
            Admit::Defer(Deferral::Reconnect) => ("reconnect", "none"),
            Admit::Defer(Deferral::Sleep) => ("sleep", "none"),
            Admit::Defer(Deferral::Backpressure) => ("backpressure", "none"),
            Admit::Defer(Deferral::UnknownQuote) => ("unknownQuote", "none"),
            Admit::Defer(Deferral::Allowance) => ("allowance", "none"),
            Admit::Go { interest, .. } => ("admit", interest_name(interest)),
            Admit::None => ("none", "none"),
            Admit::InFlight => ("inFlight", "none"),
        };
        json!({
            "decision": name,
            "winner": winner,
            "quantumRan": calls.get() == 1,
            "keptBytes": 0,
            "remaining": schedule.remaining(),
        })
    };
    let foreground = decision(
        LinkSchedule::new(base.clone()),
        &[artefact.clone(), transcript.clone()],
    );
    let background = decision(
        LinkSchedule::new(LinkSpec {
            presence: Presence::Background,
            ..base.clone()
        }),
        &[artefact.clone(), transcript.clone()],
    );
    vec![
        json!({
            "name": "foreground",
            "winner": "openTranscript",
            "artifactQuoteBytes": artefact.quote_bytes(),
            "transcriptQuoteBytes": transcript.quote_bytes(),
            "run": foreground,
        }),
        json!({
            "name": "background",
            "winner": "openTranscript",
            "run": background,
        }),
        named(
            "sleep",
            decision(
                LinkSchedule::new(base.clone()).sleep(),
                std::slice::from_ref(&transcript),
            ),
        ),
        named(
            "wake",
            decision(
                LinkSchedule::new(base.clone())
                    .sleep()
                    .wake(Presence::Foreground),
                std::slice::from_ref(&transcript),
            ),
        ),
        named(
            "unreachable",
            decision(
                LinkSchedule::new(base.clone()).unreachable(),
                std::slice::from_ref(&transcript),
            ),
        ),
        named(
            "reachable",
            decision(
                LinkSchedule::new(base.clone()).unreachable().reachable(),
                std::slice::from_ref(&transcript),
            ),
        ),
        named("catalogue", {
            let quote = conservative_quote(&Interest::Catalogue, page);
            let Quote::Known(bytes) = quote else {
                panic!("catalogue quote is known");
            };
            decision(
                LinkSchedule::new(LinkSpec {
                    billing: Billing::Metered { remaining: bytes },
                    ..base.clone()
                }),
                &[Candidate {
                    interest: Interest::Catalogue,
                    quote,
                }],
            )
        }),
        named(
            "subscriptionDown",
            decision(
                LinkSchedule::new(base.clone()).subscription(Subscription::Down),
                &[
                    transcript.clone(),
                    Candidate {
                        interest: Interest::OlderHistory(history_of.clone()),
                        quote: conservative_quote(&Interest::OlderHistory(history_of), page),
                    },
                ],
            ),
        ),
        {
            let schedule = LinkSchedule::new(base.clone()).relay();
            let route = match schedule.route() {
                Route::Relay => "relay",
                Route::Direct => "direct",
            };
            json!({
                "name": "relay",
                "route": route,
                "run": decision(schedule, std::slice::from_ref(&transcript)),
            })
        },
        named(
            "allowanceRaised",
            allowance_raised(LinkSchedule::new(base.clone()), transcript.clone(), page),
        ),
        named(
            "meteredUnknownQuote",
            decision(
                LinkSchedule::new(base.clone()),
                &[Candidate {
                    quote: Quote::Unknown,
                    ..transcript.clone()
                }],
            ),
        ),
        named(
            "reconnectAfterLoss",
            reconnect_after_loss(LinkSchedule::new(base), transcript),
        ),
    ]
}

fn named(name: &str, run: Value) -> Value {
    json!({"name": name, "run": run})
}

fn reconnect_after_loss(schedule: LinkSchedule, transcript: Candidate) -> Value {
    let (schedule, _) = schedule.admit(std::slice::from_ref(&transcript));
    let schedule = schedule.connection_lost();
    let in_flight = schedule.in_flight();
    let (schedule, settled) = schedule.cancel(0);
    let (schedule, admit, _) = run_quantum(schedule, std::slice::from_ref(&transcript), &mut |_| 1);
    let (after, after_admit, _) = run_quantum(
        schedule.backoff_elapsed(),
        std::slice::from_ref(&transcript),
        &mut |_| 0,
    );
    json!({
        "cancelKept": match settled {
            Settle::Settled { kept, reconnecting, .. } => json!({"kept": kept, "reconnecting": reconnecting}),
            Settle::Idle => json!(null),
        },
        "next": match admit {
            Admit::Defer(Deferral::Reconnect) => "reconnect",
            _ => "other",
        },
        "inFlightBeforeCancel": in_flight,
        "keptBytes": 0,
        "afterBackoff": match after_admit {
            Admit::Go { .. } => "admit",
            _ => "other",
        },
        "remaining": after.remaining(),
    })
}

fn allowance_raised(schedule: LinkSchedule, transcript: Candidate, page: u64) -> Value {
    let over = Candidate {
        quote: Quote::Known(page.saturating_add(1)),
        ..transcript
    };
    let (schedule, before, _) = run_quantum(schedule, std::slice::from_ref(&over), &mut |_| 1);
    let schedule = schedule
        .with_allowance(page.saturating_add(1))
        .expect("idle");
    let (_, after, _) = run_quantum(schedule, std::slice::from_ref(&over), &mut |_| 0);
    json!({
        "before": match before {
            Admit::Defer(Deferral::Allowance) => "allowance",
            _ => "other",
        },
        "after": match after {
            Admit::Go { .. } => "admit",
            _ => "other",
        },
        "keptBytes": 0,
    })
}

fn interest_name(interest: &Interest) -> &'static str {
    match interest {
        Interest::OpenTranscript(_) => "openTranscript",
        Interest::Catalogue => "catalogue",
        Interest::OlderHistory(_) => "olderHistory",
        Interest::Artifact { .. } => "artifact",
    }
}

trait QuoteBytes {
    fn quote_bytes(&self) -> u64;
}
impl QuoteBytes for Candidate {
    fn quote_bytes(&self) -> u64 {
        match self.quote {
            Quote::Known(bytes) => bytes,
            Quote::Unknown => 0,
        }
    }
}

fn relay_preamble_bytes() -> usize {
    RelayFrame::Connect { key: [0; 32] }.encode().len() + RelayFrame::Joined.encode().len()
}

async fn source_records(root: &Path) -> (Scope, Vec<Record>) {
    let storage = RecordStorage::new(root.join("source")).expect("source");
    storage.initialize().await.expect("initialize");
    let session = SessionId::new(OPEN).expect("session");
    let lease = storage.open(session.clone()).await.expect("open");
    let provider = ProviderIdentity::new("provider", "model", "workspace").expect("provider");
    let mut context = ProviderContext::Absent;
    let mut changes = vec![SessionChange::Opened {
        id: session.clone(),
        provider: provider.clone(),
        context: context.clone(),
    }];
    for index in 0..2 {
        let next = ProviderContext::Recorded(
            ExecutionSessionId::new(format!("{index:04}{}", "x".repeat(240))).expect("context"),
        );
        changes.push(SessionChange::ProviderContext {
            before: context,
            after: next.clone(),
        });
        context = next;
    }
    lease
        .save_changes(
            lease.load().await.expect("binding").binding().clone(),
            vec![SessionSaveUnit::new(changes).expect("unit")],
        )
        .await
        .expect("save");
    drop(lease);
    let mut source = storage
        .record_source(&session, id("origin"))
        .await
        .expect("source")
        .expect("records");
    let scope = source.scope(id("receiver"), id("epoch"));
    tokio::task::spawn_blocking(move || {
        use nessa_sdk::infrastructure::session_storage::RecordReadStatus;
        let target = loop {
            if let RecordReadStatus::Ready(head) = source.bounded_head(&scope).expect("head") {
                break head;
            }
        };
        let mut records = Vec::new();
        let mut after = 0;
        while after < target {
            let limits = policy().suffix_page();
            let request = PageRequest {
                scope: scope.clone(),
                after,
                target,
                max_records: limits.max_records(),
                max_payload_bytes: limits.max_payload_bytes(),
                max_record_bytes: limits.max_record_bytes(),
            };
            let RecordReadStatus::Ready(page) = source.bounded_page(&request).expect("page") else {
                panic!("the head was ready");
            };
            after = page.records.last().expect("record").position;
            records.extend(page.records);
        }
        (scope, records)
    })
    .await
    .expect("join")
}

fn id(text: &str) -> Id {
    Id::new(text).expect("id")
}

#[cfg(test)]
#[path = "../../../tests/read_only_sync/infrastructure/measure.rs"]
mod tests;
