//! Test-only ports for the attachment service. Every double can fail the way
//! its real counterpart can, because those are the paths nothing else reaches.
use crate::{
    attachments::{
        application::{
            AttachmentAudit, AttachmentAuditRecord, AttachmentCaller, AttachmentDependencies,
            AttachmentLimits, AttachmentService, AttachmentStore, AuditUnavailable, BeginOutcome,
            BeginUpload, Confirmation, ConversationOwnership, Discard, HoldClaim, ImageNormalizer,
            Kept, NormalizeError, NormalizeFuture, NormalizedImage, OwnershipUnavailable,
            PortFuture, ReceivedBytes, ReleaseReport, ReleasedHold, SecretsUnavailable,
            StagedUpload, StoreUnavailable, TicketSecrets, UploadBody, UploadInterrupted,
        },
        domain::{Attachment, Hold, HoldState, MediaType},
    },
    conversation::domain::ConversationId,
};
use nessa_auth::{
    application::ports::Clock,
    domain::{OrganizationId, PrincipalId},
};
use nessa_sdk::domain::common::value_objects::Sha256Digest;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::{mpsc, oneshot, Notify};

pub(crate) const CONVERSATION: &str = "00000000-0000-4000-8000-0000000000a1";
pub(crate) const OTHER_CONVERSATION: &str = "00000000-0000-4000-8000-0000000000a2";

pub(crate) fn conversation(id: &str) -> ConversationId {
    ConversationId::new(id).unwrap()
}
pub(crate) fn organization(id: &str) -> OrganizationId {
    OrganizationId::new(id).unwrap()
}
pub(crate) fn principal(id: &str) -> PrincipalId {
    PrincipalId::new(id).unwrap()
}
pub(crate) fn digest_of(bytes: &[u8]) -> Sha256Digest {
    Sha256Digest::from_bytes(Sha256::digest(bytes).into())
}
pub(crate) fn attachment(bytes: &[u8], media_type: &str) -> Attachment {
    Attachment::new(
        digest_of(bytes),
        MediaType::parse(media_type).unwrap(),
        bytes.len() as u64,
    )
    .unwrap()
}
pub(crate) fn caller(organization_id: &str, principal_id: &str) -> AttachmentCaller {
    AttachmentCaller {
        organization_id: organization(organization_id),
        principal_id: principal(principal_id),
        surface_id: "panel".into(),
    }
}
pub(crate) fn begin_request(conversation_id: &str, bytes: &[u8], media_type: &str) -> BeginUpload {
    BeginUpload {
        conversation_id: conversation_id.into(),
        request_id: "begin-1".into(),
        digest: digest_of(bytes).to_string(),
        media_type: media_type.into(),
        size: bytes.len() as u64,
    }
}

/// A clock a test moves by hand.
pub(crate) struct ManualClock(AtomicU64);
impl ManualClock {
    pub(crate) fn at(now_ms: u64) -> Self {
        Self(AtomicU64::new(now_ms))
    }
    pub(crate) fn set(&self, now_ms: u64) {
        self.0.store(now_ms, Ordering::SeqCst);
    }
}
impl Clock for ManualClock {
    fn unix_milliseconds(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// One record held at the sink until the test lets it go.
pub(crate) struct AuditGate {
    /// Which attempt waits, counting from zero.
    pub(crate) attempt: usize,
    pub(crate) open: oneshot::Receiver<()>,
    /// Whether the record is refused once let go, or kept.
    pub(crate) then_refuse: bool,
}

/// Keeps every record it acknowledged. Can refuse, can never answer, and can
/// hold one chosen record at the door while the test does something else.
#[derive(Default)]
pub(crate) struct RecordingAudit {
    pub(crate) gate: Mutex<Option<AuditGate>>,
    /// Signalled when the gated record has arrived and is waiting.
    pub(crate) entered: Notify,
    pub(crate) records: Mutex<Vec<AttachmentAuditRecord>>,
    pub(crate) refusing: AtomicBool,
    pub(crate) stalled: AtomicBool,
    pub(crate) attempts: AtomicUsize,
    /// Signalled once per acknowledged record, so a test can wait for one
    /// without guessing how long it takes.
    pub(crate) recorded: Notify,
}
impl RecordingAudit {
    /// Hold the record of the given attempt (counting every attempt so far
    /// and from zero) until the returned sender fires or drops.
    pub(crate) fn hold_attempt(&self, attempt: usize, then_refuse: bool) -> oneshot::Sender<()> {
        let (release, open) = oneshot::channel();
        *self.gate.lock().unwrap() = Some(AuditGate {
            attempt,
            open,
            then_refuse,
        });
        release
    }
    /// Hold the record `skip` records after the next one.
    pub(crate) fn hold_after(&self, skip: usize, then_refuse: bool) -> oneshot::Sender<()> {
        self.hold_attempt(self.attempts.load(Ordering::SeqCst) + skip, then_refuse)
    }
    /// Every record so far, in order, emptying the list.
    pub(crate) fn taken_all(&self) -> Vec<AttachmentAuditRecord> {
        std::mem::take(&mut self.records.lock().unwrap())
    }
    /// The same without `TicketIssued`, which every begin leaves and which
    /// would otherwise lead every list a test about something else reads.
    /// Issuance has tests of its own, which read [`Self::taken_all`].
    pub(crate) fn taken(&self) -> Vec<AttachmentAuditRecord> {
        let mut records = self.taken_all();
        records.retain(|record| !matches!(record, AttachmentAuditRecord::TicketIssued { .. }));
        records
    }
}
impl AttachmentAudit for RecordingAudit {
    fn record(&self, record: AttachmentAuditRecord) -> PortFuture<'_, (), AuditUnavailable> {
        Box::pin(async move {
            let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
            let gate = {
                let mut gate = self.gate.lock().unwrap();
                gate.take_if(|gate| gate.attempt == attempt)
            };
            if let Some(gate) = gate {
                self.entered.notify_one();
                let _ = gate.open.await;
                if gate.then_refuse {
                    return Err(AuditUnavailable);
                }
            }
            if self.stalled.load(Ordering::SeqCst) {
                std::future::pending::<()>().await;
            }
            if self.refusing.load(Ordering::SeqCst) {
                return Err(AuditUnavailable);
            }
            self.records.lock().unwrap().push(record);
            self.recorded.notify_one();
            Ok(())
        })
    }
}

/// Who owns which conversation, or no answer at all.
#[derive(Default)]
pub(crate) struct FixedOwnership {
    pub(crate) owners: Mutex<HashMap<ConversationId, (OrganizationId, PrincipalId)>>,
    pub(crate) unavailable: AtomicBool,
    pub(crate) asked: AtomicUsize,
}
impl FixedOwnership {
    pub(crate) fn give(&self, conversation_id: &str, organization_id: &str, principal_id: &str) {
        self.owners.lock().unwrap().insert(
            conversation(conversation_id),
            (organization(organization_id), principal(principal_id)),
        );
    }
}
impl ConversationOwnership for FixedOwnership {
    fn owns<'a>(
        &'a self,
        organization_id: &'a OrganizationId,
        principal_id: &'a PrincipalId,
        conversation_id: &'a ConversationId,
    ) -> PortFuture<'a, bool, OwnershipUnavailable> {
        Box::pin(async move {
            self.asked.fetch_add(1, Ordering::SeqCst);
            if self.unavailable.load(Ordering::SeqCst) {
                return Err(OwnershipUnavailable);
            }
            Ok(self
                .owners
                .lock()
                .unwrap()
                .get(conversation_id)
                .is_some_and(|(organization, owner)| {
                    organization == organization_id && owner == principal_id
                }))
        })
    }
}

/// Distinct, reproducible secrets: the n-th is 32 bytes of n.
#[derive(Default)]
pub(crate) struct CountingSecrets {
    issued: AtomicU64,
    pub(crate) unavailable: AtomicBool,
}
impl TicketSecrets for CountingSecrets {
    fn fresh(&self) -> Result<[u8; 32], SecretsUnavailable> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(SecretsUnavailable);
        }
        let next = self.issued.fetch_add(1, Ordering::SeqCst) + 1;
        Ok([next as u8; 32])
    }
}

/// Answers every image the same way, and remembers what it was shown.
pub(crate) struct StubNormalizer {
    pub(crate) answer: Mutex<Result<NormalizedImage, NormalizeError>>,
    pub(crate) seen: Mutex<Vec<(Vec<u8>, String)>>,
}
impl StubNormalizer {
    pub(crate) fn producing(bytes: &[u8], media_type: &str) -> Self {
        Self {
            answer: Mutex::new(Ok(NormalizedImage {
                bytes: bytes.to_vec(),
                media_type: media_type.into(),
            })),
            seen: Mutex::default(),
        }
    }
    pub(crate) fn failing(error: NormalizeError) -> Self {
        Self {
            answer: Mutex::new(Err(error)),
            seen: Mutex::default(),
        }
    }
}
impl ImageNormalizer for StubNormalizer {
    fn normalize<'a>(
        &'a self,
        original: Vec<u8>,
        declared_media_type: &'a str,
    ) -> NormalizeFuture<'a> {
        Box::pin(async move {
            self.seen
                .lock()
                .unwrap()
                .push((original, declared_media_type.to_owned()));
            self.answer.lock().unwrap().clone()
        })
    }
}

/// A body fed by the test: send chunks, send an interruption, drop the sender
/// to finish, or hold it to stall.
pub(crate) struct ChannelBody(mpsc::UnboundedReceiver<Result<Vec<u8>, UploadInterrupted>>);
pub(crate) type BodySender = mpsc::UnboundedSender<Result<Vec<u8>, UploadInterrupted>>;
impl ChannelBody {
    pub(crate) fn open() -> (BodySender, Box<dyn UploadBody>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (sender, Box::new(Self(receiver)))
    }
    /// A complete body, already delivered, in chunks of `chunk` bytes.
    pub(crate) fn of(bytes: &[u8], chunk: usize) -> Box<dyn UploadBody> {
        let (sender, body) = Self::open();
        for part in bytes.chunks(chunk.max(1)) {
            sender.send(Ok(part.to_vec())).unwrap();
        }
        body
    }
}
impl UploadBody for ChannelBody {
    fn next(&mut self) -> PortFuture<'_, Option<Vec<u8>>, UploadInterrupted> {
        Box::pin(async move { self.0.recv().await.transpose() })
    }
}

/// An in-memory store with the real one's contract, and switches for its failures.
#[derive(Default)]
pub(crate) struct MemoryStore {
    state: Arc<Mutex<MemoryState>>,
    pub(crate) unavailable: AtomicBool,
    pub(crate) keep_fails: Arc<AtomicBool>,
    pub(crate) confirm_fails: AtomicBool,
    pub(crate) discard_fails: AtomicBool,
    /// Transfers staged and not yet kept or dropped.
    pub(crate) staged: Arc<AtomicUsize>,
    /// Bytes written to staged transfers, ever.
    pub(crate) written: Arc<AtomicU64>,
}
struct MemoryRecord {
    hold: Hold,
    kept: bool,
    generation: String,
}
#[derive(Default)]
struct MemoryState {
    blobs: HashMap<Sha256Digest, Vec<u8>>,
    records: Vec<MemoryRecord>,
    generations: u64,
    /// Stored digests whose hold cannot be released.
    stuck: Vec<Sha256Digest>,
}
impl MemoryStore {
    pub(crate) fn blob(&self, digest: Sha256Digest) -> Option<Vec<u8>> {
        self.state.lock().unwrap().blobs.get(&digest).cloned()
    }
    pub(crate) fn blob_count(&self) -> usize {
        self.state.lock().unwrap().blobs.len()
    }
    /// Usable holds only, as everything that asks the store sees them.
    pub(crate) fn held(&self) -> Vec<Hold> {
        let state = self.state.lock().unwrap();
        let kept = state.records.iter().filter(|record| record.kept);
        kept.map(|record| record.hold.clone()).collect()
    }
    /// Holds written and not yet usable.
    pub(crate) fn pending(&self) -> usize {
        let state = self.state.lock().unwrap();
        state.records.iter().filter(|record| !record.kept).count()
    }
    pub(crate) fn stick(&self, stored: Sha256Digest) {
        self.state.lock().unwrap().stuck.push(stored);
    }
    fn check(&self) -> Result<(), StoreUnavailable> {
        if self.unavailable.load(Ordering::SeqCst) {
            Err(StoreUnavailable)
        } else {
            Ok(())
        }
    }
}
impl MemoryState {
    /// A hold is one conversation keeping one stored file: digest and type.
    fn position(&self, hold: &Hold) -> Option<usize> {
        self.records.iter().position(|known| {
            known.hold.keeps(
                hold.organization_id(),
                hold.conversation_id(),
                hold.stored(),
            )
        })
    }
    fn remove_unheld(&mut self, digest: Sha256Digest) -> bool {
        let held = self
            .records
            .iter()
            .any(|record| record.hold.stored().digest() == digest);
        !held && self.blobs.remove(&digest).is_some()
    }
}
impl AttachmentStore for MemoryStore {
    fn holds<'a>(
        &'a self,
        organization_id: &'a OrganizationId,
        conversation_id: &'a ConversationId,
        stored: &'a Attachment,
    ) -> PortFuture<'a, bool, StoreUnavailable> {
        Box::pin(async move {
            self.check()?;
            let state = self.state.lock().unwrap();
            Ok(state.records.iter().any(|record| {
                record.kept
                    && record.hold.keeps(organization_id, conversation_id, stored)
                    && state.blobs.contains_key(&stored.digest())
            }))
        })
    }
    fn find_upload<'a>(
        &'a self,
        organization_id: &'a OrganizationId,
        conversation_id: &'a ConversationId,
        uploaded: &'a Attachment,
    ) -> PortFuture<'a, Option<Hold>, StoreUnavailable> {
        Box::pin(async move {
            self.check()?;
            let state = self.state.lock().unwrap();
            Ok(state
                .records
                .iter()
                .find(|record| {
                    record.kept
                        && record
                            .hold
                            .came_from(organization_id, conversation_id, uploaded)
                        && state.blobs.contains_key(&record.hold.stored().digest())
                })
                .map(|record| record.hold.clone()))
        })
    }
    fn stage(&self) -> PortFuture<'_, Box<dyn StagedUpload>, StoreUnavailable> {
        Box::pin(async move {
            self.check()?;
            self.staged.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(MemoryStaged {
                state: self.state.clone(),
                keep_fails: self.keep_fails.clone(),
                staged: self.staged.clone(),
                written: self.written.clone(),
                bytes: Vec::new(),
                finished: false,
            }) as Box<dyn StagedUpload>)
        })
    }
    fn confirm<'a>(
        &'a self,
        hold: &'a Hold,
        claim: &'a HoldClaim,
    ) -> PortFuture<'a, Confirmation, StoreUnavailable> {
        Box::pin(async move {
            if self.confirm_fails.load(Ordering::SeqCst) {
                return Err(StoreUnavailable);
            }
            let mut state = self.state.lock().unwrap();
            let Some(index) = state.position(hold) else {
                return Ok(Confirmation::Gone);
            };
            let record = &mut state.records[index];
            if record.kept {
                return Ok(if record.generation == claim.as_str() {
                    Confirmation::Confirmed
                } else {
                    Confirmation::AlreadyKept
                });
            }
            *record = MemoryRecord {
                hold: hold.clone(),
                kept: true,
                generation: claim.as_str().to_owned(),
            };
            Ok(Confirmation::Confirmed)
        })
    }
    fn discard<'a>(
        &'a self,
        hold: &'a Hold,
        claim: &'a HoldClaim,
    ) -> PortFuture<'a, Discard, StoreUnavailable> {
        Box::pin(async move {
            if self.discard_fails.load(Ordering::SeqCst) {
                return Err(StoreUnavailable);
            }
            let mut state = self.state.lock().unwrap();
            match state.position(hold) {
                Some(index) if state.records[index].generation == claim.as_str() => {
                    state.records.remove(index);
                    state.remove_unheld(hold.stored().digest());
                    Ok(Discard::Discarded)
                }
                _ => Ok(Discard::NotMine),
            }
        })
    }
    fn release<'a>(
        &'a self,
        organization_id: &'a OrganizationId,
        conversation_id: &'a ConversationId,
    ) -> PortFuture<'a, ReleaseReport, StoreUnavailable> {
        Box::pin(async move {
            self.check()?;
            let mut state = self.state.lock().unwrap();
            let mut report = ReleaseReport::default();
            let (mine, others): (Vec<_>, Vec<_>) = std::mem::take(&mut state.records)
                .into_iter()
                .partition(|record| {
                    record.hold.organization_id() == organization_id
                        && record.hold.conversation_id() == conversation_id
                });
            state.records = others;
            for record in mine {
                if state.stuck.contains(&record.hold.stored().digest()) {
                    report.failures += 1;
                    state.records.push(record);
                } else {
                    report.released.push(ReleasedHold {
                        hold: record.hold,
                        was: if record.kept {
                            HoldState::Held
                        } else {
                            HoldState::Pending
                        },
                    });
                }
            }
            let mut considered = Vec::new();
            for released in &report.released {
                let digest = released.hold.stored().digest();
                if considered.contains(&digest) {
                    continue;
                }
                considered.push(digest);
                if state.remove_unheld(digest) {
                    report.removed.push(released.hold.clone());
                }
            }
            Ok(report)
        })
    }
    fn read(
        &self,
        digest: Sha256Digest,
        limit: u64,
    ) -> PortFuture<'_, Option<Vec<u8>>, StoreUnavailable> {
        Box::pin(async move {
            self.check()?;
            Ok(self.blob(digest).map(|mut bytes| {
                bytes.truncate(limit as usize);
                bytes
            }))
        })
    }
}

struct MemoryStaged {
    state: Arc<Mutex<MemoryState>>,
    keep_fails: Arc<AtomicBool>,
    staged: Arc<AtomicUsize>,
    written: Arc<AtomicU64>,
    bytes: Vec<u8>,
    finished: bool,
}
impl Drop for MemoryStaged {
    fn drop(&mut self) {
        self.staged.fetch_sub(1, Ordering::SeqCst);
    }
}
impl StagedUpload for MemoryStaged {
    fn write(&mut self, chunk: Vec<u8>) -> PortFuture<'_, (), StoreUnavailable> {
        Box::pin(async move {
            self.written.fetch_add(chunk.len() as u64, Ordering::SeqCst);
            self.bytes.extend_from_slice(&chunk);
            Ok(())
        })
    }
    fn finish(&mut self) -> PortFuture<'_, ReceivedBytes, StoreUnavailable> {
        Box::pin(async move {
            self.finished = true;
            Ok(ReceivedBytes {
                size: self.bytes.len() as u64,
                digest: digest_of(&self.bytes),
            })
        })
    }
    fn read(&mut self) -> PortFuture<'_, Vec<u8>, StoreUnavailable> {
        Box::pin(async move { Ok(self.bytes.clone()) })
    }
    fn keep(self: Box<Self>, hold: Hold) -> PortFuture<'static, Kept, StoreUnavailable> {
        Box::pin(async move {
            if self.keep_fails.load(Ordering::SeqCst)
                || !self.finished
                || digest_of(&self.bytes) != hold.stored().digest()
                || self.bytes.len() as u64 != hold.stored().size()
            {
                return Err(StoreUnavailable);
            }
            let mut state = self.state.lock().unwrap();
            state
                .blobs
                .entry(hold.stored().digest())
                .or_insert_with(|| self.bytes.clone());
            let existing = state.position(&hold);
            if let Some(index) = existing.filter(|index| state.records[*index].kept) {
                return Ok(Kept::Existing(state.records[index].hold.clone()));
            }
            if let Some(index) = existing {
                state.records.remove(index);
            }
            state.generations += 1;
            let generation = format!("generation-{}", state.generations);
            state.records.push(MemoryRecord {
                hold,
                kept: false,
                generation: generation.clone(),
            });
            Ok(Kept::Pending(HoldClaim::new(&generation)))
        })
    }
}

/// A service over doubles, with every double reachable.
pub(crate) struct Fixture {
    pub(crate) service: AttachmentService,
    pub(crate) store: Arc<MemoryStore>,
    pub(crate) audit: Arc<RecordingAudit>,
    pub(crate) ownership: Arc<FixedOwnership>,
    pub(crate) secrets: Arc<CountingSecrets>,
    pub(crate) normalizer: Arc<StubNormalizer>,
    pub(crate) clock: Arc<ManualClock>,
}
pub(crate) const NOW_MS: u64 = 1_700_000_000_000;

impl Fixture {
    pub(crate) fn new(limits: AttachmentLimits) -> Self {
        Self::with_normalizer(limits, StubNormalizer::failing(NormalizeError::Failed))
    }
    pub(crate) fn with_normalizer(limits: AttachmentLimits, normalizer: StubNormalizer) -> Self {
        let store = Arc::new(MemoryStore::default());
        Self::over(store.clone(), store, limits, normalizer)
    }
    /// The same doubles over any store, so the real one can be driven too.
    pub(crate) fn over(
        store: Arc<dyn AttachmentStore>,
        memory: Arc<MemoryStore>,
        limits: AttachmentLimits,
        normalizer: StubNormalizer,
    ) -> Self {
        let audit = Arc::new(RecordingAudit::default());
        let ownership = Arc::new(FixedOwnership::default());
        ownership.give(CONVERSATION, "org", "owner");
        ownership.give(OTHER_CONVERSATION, "org", "owner");
        let secrets = Arc::new(CountingSecrets::default());
        let normalizer = Arc::new(normalizer);
        let clock = Arc::new(ManualClock::at(NOW_MS));
        let service = AttachmentService::new(
            AttachmentDependencies {
                store,
                audit: audit.clone(),
                ownership: ownership.clone(),
                secrets: secrets.clone(),
                normalizer: normalizer.clone(),
                clock: clock.clone(),
            },
            limits,
        );
        Self {
            service,
            store: memory,
            audit,
            ownership,
            secrets,
            normalizer,
            clock,
        }
    }
    /// Begin an upload as the owner and return the ticket's text.
    pub(crate) async fn ticket(
        &self,
        conversation_id: &str,
        bytes: &[u8],
        media_type: &str,
    ) -> String {
        self.ticket_as("begin-1", conversation_id, bytes, media_type)
            .await
    }
    /// The same, under a chosen action identifier: a different request.
    pub(crate) async fn ticket_as(
        &self,
        request_id: &str,
        conversation_id: &str,
        bytes: &[u8],
        media_type: &str,
    ) -> String {
        let mut request = begin_request(conversation_id, bytes, media_type);
        request.request_id = request_id.into();
        match self
            .service
            .begin(caller("org", "owner"), request)
            .await
            .unwrap()
        {
            BeginOutcome::UploadRequired { ticket, .. } => ticket.expose(),
            BeginOutcome::Stored(_) => panic!("a ticket was expected"),
        }
    }
    /// Begin and complete an upload as the owner.
    pub(crate) async fn upload(
        &self,
        conversation_id: &str,
        bytes: &[u8],
        media_type: &str,
    ) -> Attachment {
        let ticket = self.ticket(conversation_id, bytes, media_type).await;
        self.service
            .receive(&ticket, Some(bytes.len() as u64), ChannelBody::of(bytes, 7))
            .await
            .unwrap()
    }
}
