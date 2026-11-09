//! Time per-entry access admission on a paired reader's catalogue and on
//! the owner's list, against the same reads with admission bypassed. Not run
//! in CI.
//!
//! ```text
//! cargo run --release -p nessa-server --example catalogue_admission_bench -- \
//!     [--sizes 500,5000] [--repeats 20] [--evaluations 20000]
//! ```
//!
//! For each size N it seeds a throwaway `LocalConversationStore` with N of
//! one owner's conversations, each with a summary and shared with one
//! receiver (Read and Comment in turn, every tenth one then deleted), and
//! then:
//!
//! - reads the receiver's whole catalogue manifest through
//!   `NessaCatalogueSource`, the adapter behind
//!   `conversation.catalogueManifest`, page by page at the published page
//!   size, once with each entry admitted by the checked-in Cedar policy and
//!   once by an evaluator that allows everything (admission bypassed), and
//!   reports p50/p99 per page and the evaluations each page asked;
//! - lists the owner's conversations as `ConversationService::list` does at
//!   its bound (the store's listing, one past `MAX_LISTED_CONVERSATIONS`,
//!   then `OwnerAccess::admits_each`), with Cedar, with admission bypassed,
//!   and asking Cedar row by row as the list did before. The service itself
//!   is not built: its environment is crate-private, and the list's own work
//!   past admission (live state of open conversations) is none here.
//!
//! Finally it times single Cedar evaluations of `catalogue.entry` and
//! `conversation.act`. The report is plain text on stdout; diagnostics go
//! to stderr.
use nessa_auth::domain::{CredentialId, OrganizationId, PrincipalId};
use nessa_protocol::agents::AgentId;
use nessa_protocol::conversation::domain::{
    conversation_catalogue_schema, conversation_catalogue_stream, ConversationApprovalMode,
    ConversationId, ConversationModelId, ConversationSummary,
};
use nessa_server::conversation::application::{
    AccessAction, AccessAsk, AccessDecision, AccessResource, AccessSubject, CatalogueEntryAccess,
    ConversationAccessPolicyEvaluator, ConversationCaller, ConversationCatalogue,
    ConversationListing, ConversationRepository, ConversationSummaries, OwnerAccess, Reader,
    RequestShape, ShareGrantChange, ShareGrantTransition, ShareGrants, MAX_LISTED_CONVERSATIONS,
};
use nessa_server::conversation::domain::{
    Capabilities, Conversation, ConversationDeletion, ShareAccess,
};
use nessa_server::conversation::infrastructure::{
    CedarConversationAccessPolicy, LocalConversationStore, NessaCatalogueSource,
};
use nessa_sync::replication::catalogue::{
    CataloguePass, CatalogueSource, ManifestRequest, MAX_CATALOGUE_ENTRIES,
};
use nessa_sync::replication::domain::{Id, Scope};
use std::error::Error;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing_subscriber::EnvFilter;

const ORGANIZATION: &str = "org";
const OWNER: &str = "alice";
const RECEIVER: &str = "receiver";

fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()))
        .init();
    if let Err(error) = run() {
        eprintln!("catalogue admission benchmark failed: {error}");
        std::process::exit(1);
    }
}

struct Arguments {
    sizes: Vec<usize>,
    repeats: usize,
    evaluations: usize,
}

fn arguments() -> Result<Arguments, Box<dyn Error>> {
    let mut parsed = Arguments {
        sizes: vec![500, 5000],
        repeats: 20,
        evaluations: 20_000,
    };
    let mut given = std::env::args().skip(1);
    while let Some(flag) = given.next() {
        let value = given.next().ok_or(format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--sizes" => {
                parsed.sizes = value.split(',').map(str::parse).collect::<Result<_, _>>()?
            }
            "--repeats" => parsed.repeats = value.parse()?,
            "--evaluations" => parsed.evaluations = value.parse()?,
            _ => return Err(format!("unknown flag {flag}").into()),
        }
    }
    Ok(parsed)
}

/// Allows everything: admission bypassed.
struct AllowAll;
impl ConversationAccessPolicyEvaluator for AllowAll {
    fn evaluate(&self, _: &AccessAsk<'_>) -> AccessDecision {
        AccessDecision::Allow {
            policies: Vec::new(),
        }
    }
}

/// Counts what it passes on.
struct Counting(Arc<dyn ConversationAccessPolicyEvaluator>, AtomicUsize);
impl ConversationAccessPolicyEvaluator for Counting {
    fn evaluate(&self, ask: &AccessAsk<'_>) -> AccessDecision {
        self.1.fetch_add(1, Ordering::Relaxed);
        self.0.evaluate(ask)
    }
}

fn organization() -> OrganizationId {
    OrganizationId::new(ORGANIZATION).unwrap()
}
fn owner() -> PrincipalId {
    PrincipalId::new(OWNER).unwrap()
}
fn caller() -> ConversationCaller {
    ConversationCaller {
        organization_id: organization(),
        principal_id: owner(),
        surface_id: "bench".into(),
        action_id: "bench".into(),
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let arguments = arguments()?;
    let cedar = Arc::new(CedarConversationAccessPolicy::new()?);
    println!(
        "catalogue admission bench: page size {MAX_CATALOGUE_ENTRIES}, list bound {MAX_LISTED_CONVERSATIONS}, {} repeats",
        arguments.repeats
    );
    for &size in &arguments.sizes {
        bench_size(size, arguments.repeats, &cedar)?;
    }
    bench_evaluations(arguments.evaluations, cedar.as_ref());
    Ok(())
}

fn bench_size(
    size: usize,
    repeats: usize,
    cedar: &Arc<CedarConversationAccessPolicy>,
) -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let private = directory.path().join("conversations");
    std::fs::create_dir(&private)?;
    #[cfg(unix)]
    {
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700))?;
    }
    let store = Arc::new(
        LocalConversationStore::open(&private.join("metadata.sqlite3"))?
            .with_access_policy(cedar.clone()),
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let seeded = Instant::now();
    let incarnation = runtime.block_on(seed(&store, size))?;
    eprintln!(
        "seeded {size} shared conversations in {:?}",
        seeded.elapsed()
    );
    println!("\n== N = {size} ==");

    // Manifest: the receiver's whole catalogue, page by page.
    let counted = Arc::new(Counting(cedar.clone(), AtomicUsize::new(0)));
    let admitted = manifest_pass(&store, counted.clone(), &incarnation, repeats)?;
    let evaluations = counted.1.load(Ordering::Relaxed);
    let bypassed = manifest_pass(&store, Arc::new(AllowAll), &incarnation, repeats)?;
    let pages_per_read = admitted.pages.len() / repeats;
    println!(
        "manifest: {} entries in {pages_per_read} pages per read; {:.1} Cedar evaluations per page",
        admitted.entries / repeats,
        evaluations as f64 / admitted.pages.len() as f64,
    );
    report("  page, Cedar per entry", &admitted.pages);
    report("  page, admission bypassed", &bypassed.pages);
    report("  whole read, Cedar per entry", &admitted.reads);
    report("  whole read, admission bypassed", &bypassed.reads);

    // List: as `ConversationService::list` at its bound.
    let caller = caller();
    let mut sql = Vec::new();
    let mut cached = Vec::new();
    let mut row_by_row = Vec::new();
    let mut allowed = Vec::new();
    let mut shown = 0;
    let mut asked = 0;
    for _ in 0..repeats {
        let started = Instant::now();
        let listed = runtime.block_on(store.list(
            &caller.organization_id,
            &caller.principal_id,
            false,
            MAX_LISTED_CONVERSATIONS + 1,
            &Reader::Owner,
        ))?;
        sql.push(started.elapsed());
        let rows: Vec<&Conversation> = listed
            .conversations
            .iter()
            .map(|row| &row.conversation)
            .collect();
        let counting = Counting(cedar.clone(), AtomicUsize::new(0));
        let started = Instant::now();
        let admitted = OwnerAccess::new(&counting)
            .admits_each(rows.iter().copied(), &caller)
            .map_err(|refusal| format!("{refusal:?}"))?;
        cached.push(started.elapsed());
        asked = counting.1.load(Ordering::Relaxed);
        shown = admitted.iter().filter(|admitted| **admitted).count();
        let access = OwnerAccess::new(cedar.as_ref());
        let started = Instant::now();
        for row in &rows {
            access
                .admits(row, &caller)
                .map_err(|refusal| format!("{refusal:?}"))?;
        }
        row_by_row.push(started.elapsed());
        let started = Instant::now();
        OwnerAccess::new(&AllowAll)
            .admits_each(rows.iter().copied(), &caller)
            .map_err(|refusal| format!("{refusal:?}"))?;
        allowed.push(started.elapsed());
    }
    println!("list: {shown} rows shown; {asked} Cedar evaluations per list");
    report("  listing SQL", &sql);
    report("  admission, Cedar once per kind of row", &cached);
    report("  admission, Cedar row by row (before)", &row_by_row);
    report("  admission bypassed", &allowed);
    drop(runtime);
    drop(store);
    Ok(())
}

/// N conversations of the owner's, each summarized and shared with the
/// receiver, Read and Comment in turn, every tenth one then deleted. Returns
/// the catalogue's incarnation.
async fn seed(store: &Arc<LocalConversationStore>, size: usize) -> Result<String, Box<dyn Error>> {
    for index in 0..size {
        let id = ConversationId::new(&uuid::Uuid::new_v4().to_string())?;
        let at = 1_700_000_000_000 + index as u64;
        let conversation = Conversation::new(
            id.clone(),
            organization(),
            owner(),
            "bench".into(),
            format!("create-{index}"),
            at,
            AgentId::Claude,
            ConversationModelId::new("bench-model")?,
            ConversationApprovalMode::Ask,
        )
        .map_err(|error| format!("{error:?}"))?;
        store.create(conversation).await?;
        store
            .record(
                &id,
                ConversationSummary::after_message(None, "said something", None, at),
            )
            .await?;
        let role = if index % 2 == 0 {
            Capabilities::read()
        } else {
            Capabilities::comment()
        };
        let receipt = store
            .change(ShareGrantChange {
                transition: ShareGrantTransition::Grant(
                    ShareAccess::new(role, None).map_err(|error| format!("{error:?}"))?,
                ),
                conversation_id: id.clone(),
                principal_id: Some(PrincipalId::new(format!("grantee-{RECEIVER}")).unwrap()),
                receiver_id: Some(RECEIVER.into()),
                credential_id: CredentialId::new(format!("{RECEIVER}-credential"))?,
                expected_policy_revision: None,
                initiator: caller(),
                at_ms: at,
            })
            .await?;
        if !receipt.applied {
            return Err("a grant the bench makes changed nothing".into());
        }
        if index % 10 == 9 {
            store
                .record_deletion(
                    &id,
                    ConversationDeletion::new(
                        organization(),
                        owner(),
                        "bench".into(),
                        format!("delete-{index}"),
                        at + 1,
                    )
                    .map_err(|error| format!("{error:?}"))?,
                )
                .await?;
        }
    }
    let head = store
        .head(&organization(), &owner(), &Reader::Owner)
        .await?;
    Ok(head.incarnation)
}

fn sync_id(value: &str) -> Result<Id, Box<dyn Error>> {
    Id::new(value).map_err(|error| format!("{error:?}").into())
}

struct Pass {
    pages: Vec<Duration>,
    reads: Vec<Duration>,
    entries: usize,
}

/// Read the receiver's whole manifest `repeats` times through a source whose
/// entries `policy` admits, timing each page and each whole read.
fn manifest_pass(
    store: &Arc<LocalConversationStore>,
    policy: Arc<dyn ConversationAccessPolicyEvaluator>,
    incarnation: &str,
    repeats: usize,
) -> Result<Pass, Box<dyn Error>> {
    let scope = Scope::new(
        sync_id(RECEIVER)?,
        sync_id("origin")?,
        conversation_catalogue_stream(&organization(), &owner()),
        sync_id(incarnation)?,
        conversation_catalogue_schema(),
        sync_id("epoch")?,
    );
    let mut source = NessaCatalogueSource::new(
        store.clone(),
        caller(),
        CatalogueEntryAccess::new(policy, owner()),
        scope.clone(),
    )
    .map_err(|error| format!("{error:?}"))?;
    let head = source.head(&scope).map_err(|error| format!("{error:?}"))?;
    let mut pass = Pass {
        pages: Vec::new(),
        reads: Vec::new(),
        entries: 0,
    };
    for repeat in 0..repeats {
        let started = Instant::now();
        let mut cursor = None;
        loop {
            let request = ManifestRequest {
                pass: CataloguePass {
                    scope: scope.clone(),
                    completed: 0,
                    boundary: head,
                    cursor: cursor.clone(),
                    generation: repeat as u64 + 1,
                },
                max_entries: MAX_CATALOGUE_ENTRIES,
            };
            let page_started = Instant::now();
            let page = source
                .manifest(&request)
                .map_err(|error| format!("{error:?}"))?;
            pass.pages.push(page_started.elapsed());
            pass.entries += page.entries.len();
            cursor = page.entries.last().map(|entry| entry.key.clone());
            if !page.has_more {
                break;
            }
        }
        pass.reads.push(started.elapsed());
    }
    source.finish().map_err(|error| format!("{error:?}"))?;
    Ok(pass)
}

/// Single Cedar evaluations: a paired reader's catalogue entry and an
/// owner's act.
fn bench_evaluations(count: usize, cedar: &CedarConversationAccessPolicy) {
    let (organization, owner) = (organization(), owner());
    let entry = AccessAsk {
        subject: AccessSubject::PairedReader {
            organization: &organization,
            signed_in_as: &owner,
            binding_owner: &owner,
            receiver: RECEIVER,
        },
        action: AccessAction::CatalogueEntry,
        resource: AccessResource::Conversation {
            organization: &organization,
            owner: &owner,
            deleted: false,
        },
        held: Some(Capabilities::read()),
        request: RequestShape::default(),
        target: None,
    };
    let act = AccessAsk {
        subject: AccessSubject::Person {
            organization: &organization,
            principal: &owner,
        },
        action: AccessAction::Act,
        held: None,
        ..entry
    };
    println!("\n== single Cedar evaluations ({count} each) ==");
    for (name, ask) in [("catalogue.entry", &entry), ("conversation.act", &act)] {
        let mut times = Vec::with_capacity(count);
        for _ in 0..count {
            let started = Instant::now();
            let decision = cedar.evaluate(ask);
            times.push(started.elapsed());
            assert!(matches!(decision, AccessDecision::Allow { .. }), "{name}");
        }
        report(&format!("  {name}"), &times);
    }
}

fn report(name: &str, times: &[Duration]) {
    let mut sorted = times.to_vec();
    sorted.sort();
    let at = |quantile: f64| {
        let index = ((sorted.len() as f64 - 1.0) * quantile).round() as usize;
        sorted[index]
    };
    println!(
        "{name:<44} n={:<6} p50={:>10.1?} p99={:>10.1?} max={:>10.1?}",
        sorted.len(),
        at(0.5),
        at(0.99),
        sorted[sorted.len() - 1],
    );
}
