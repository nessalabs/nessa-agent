//! Own the private database lock and bounded physical outbox operations.
#[cfg(test)]
use super::{audit, publication, registry};
use super::{
    audit::{Cause, Record},
    codec, integer,
    journal::{commit, delivery_audit, finish},
    publication::{publish, SavedAudit},
    queue,
    registry::StoredRegistration,
    restoration, source_reset, stored,
    stored::StoredDelivery,
    unavailable, SCHEMA,
};

use crate::webhooks::application::{
    DeliveryCause, Outbox, OutboxError, PendingDelivery, QueuedRecord, MAX_BODY_BYTES,
    MAX_OUTBOX_WORKERS,
};
use crate::webhooks::domain::{Delivery, Phase};
use async_trait::async_trait;
use fs2::FileExt;
use nessa_auth::application::ports::Clock;
use nessa_local_database::{
    rusqlite::{params, Connection, OptionalExtension},
    Schema,
};
use nessa_local_storage::{OpenMode, PrivateTempFile};
#[cfg(test)]
use std::sync::Condvar;
use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
#[cfg(test)]
use tokio::sync::Notify;
use tokio::{sync::Semaphore, time::Instant};
use uuid::Uuid;

pub(super) struct Owner {
    connection: Mutex<Connection>,
    attempt: Mutex<Option<Uuid>>,
    lock: File,
    audit: PathBuf,
    capacity: Arc<Semaphore>,
    pub(super) clock: Arc<dyn Clock>,
    #[cfg(test)]
    admission_pause: Mutex<Option<Arc<AdmissionPause>>>,
    #[cfg(test)]
    pin_pause: Mutex<Option<Arc<AdmissionPause>>>,
}
// Only disposable native adapter fixtures can pause this actual physical work.
#[cfg(test)]
struct AdmissionPause {
    after_commit: bool,
    entered: Notify,
    released: Mutex<bool>,
    wake: Condvar,
}
#[cfg(test)]
impl AdmissionPause {
    fn wait(&self, after_commit: bool) {
        if self.after_commit != after_commit {
            return;
        }
        let mut released = self.released.lock().unwrap();
        self.entered.notify_one();
        while !*released {
            released = self.wake.wait(released).unwrap();
        }
    }
}
#[cfg(test)]
pub(crate) struct AdmissionControl(Arc<AdmissionPause>);
#[cfg(test)]
impl AdmissionControl {
    pub(crate) async fn entered(&self) {
        self.0.entered.notified().await;
    }
    pub(crate) fn release(&self) {
        *self.0.released.lock().unwrap() = true;
        self.0.wake.notify_all();
    }
}
#[cfg(test)]
impl Drop for AdmissionControl {
    fn drop(&mut self) {
        self.release();
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.lock);
    }
}

#[derive(Clone)]
pub struct SqlOutbox {
    pub(super) owner: Arc<Owner>,
}

/// Retained by the physical sender before admission. Dropping an observer or a
/// clone cannot release the native exclusion; only consumed settlement does.
#[derive(Clone)]
pub(crate) struct DeliveryAttempt {
    owner: Arc<Owner>,
    id: Uuid,
    registration: Uuid,
    before: Delivery,
    capture: Option<Uuid>,
}
enum TransitionCommit {
    Ordinary(Option<Instant>),
    Attempt(Option<Instant>),
}
enum AttemptWork {
    Admit,
    Response,
    Unposted,
}

impl SqlOutbox {
    /// Read-only native metadata. It never publishes or reclaims owed evidence.
    pub(super) async fn read<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Connection) -> Result<T, OutboxError> + Send + 'static,
    ) -> Result<T, OutboxError> {
        let owner = self.owner.clone();
        if owner.attempt.lock().map_err(unavailable)?.is_some() {
            return Err(OutboxError::Unavailable);
        }
        let permit = owner
            .capacity
            .clone()
            .try_acquire_owned()
            .map_err(|_| OutboxError::Capacity)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let connection = owner.connection.lock().map_err(unavailable)?;
            if owner.attempt.lock().map_err(unavailable)?.is_some() {
                return Err(OutboxError::Unavailable);
            }
            work(&connection)
        })
        .await
        .map_err(unavailable)?
    }

    pub(crate) fn attempt_pending(&self) -> bool {
        self.owner
            .attempt
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
    }

    pub fn open(path: &Path, audit: PathBuf, clock: Arc<dyn Clock>) -> Result<Self, OutboxError> {
        let lock = nessa_local_storage::open(&path.with_extension("lock"), OpenMode::OpenOrCreate)
            .map_err(unavailable)?;
        lock.try_lock_exclusive().map_err(unavailable)?;
        let schema = Schema::new(SCHEMA).map_err(unavailable)?;
        let connection =
            nessa_local_database::open(path, &schema).map_err(|error| match error {
                nessa_local_database::OpenError::Unreadable(_)
                | nessa_local_database::OpenError::Damaged(_)
                | nessa_local_database::OpenError::Version { .. } => OutboxError::Invalid,
                _ => OutboxError::Unavailable,
            })?;
        super::schema::validate(&connection)?;
        super::intents::validate(&connection)?;
        source_reset::validate_all(&connection)?;
        stored::validate(&connection)?;
        restoration::validate(&connection)?;
        nessa_local_storage::create_directory(&audit).map_err(unavailable)?;
        PrivateTempFile::clear_stale(&audit).map_err(unavailable)?;
        Ok(Self {
            owner: Arc::new(Owner {
                connection: Mutex::new(connection),
                attempt: Mutex::new(None),
                lock,
                audit,
                capacity: Arc::new(Semaphore::new(MAX_OUTBOX_WORKERS)),
                clock,
                #[cfg(test)]
                admission_pause: Mutex::new(None),
                #[cfg(test)]
                pin_pause: Mutex::new(None),
            }),
        })
    }

    pub(super) async fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut Connection, &Path) -> Result<T, OutboxError> + Send + 'static,
    ) -> Result<T, OutboxError> {
        self.run_checked(
            |_| Ok(()),
            move |connection, audit, ()| work(connection, audit),
        )
        .await
    }

    /// Correlate supplied original facts before owed publication. This check
    /// grants no credential or conversation permission.
    pub(super) async fn run_checked<T: Send + 'static, C: Send + 'static>(
        &self,
        check: impl FnOnce(&Connection) -> Result<C, OutboxError> + Send + 'static,
        work: impl FnOnce(&mut Connection, &Path, C) -> Result<T, OutboxError> + Send + 'static,
    ) -> Result<T, OutboxError> {
        let owner = self.owner.clone();
        if owner.attempt.lock().map_err(unavailable)?.is_some() {
            return Err(OutboxError::Unavailable);
        }
        let permit = owner
            .capacity
            .clone()
            .try_acquire_owned()
            .map_err(|_| OutboxError::Capacity)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut connection = owner.connection.lock().map_err(unavailable)?;
            // An operation may already have been queued when admission pinned.
            if owner.attempt.lock().map_err(unavailable)?.is_some() {
                return Err(OutboxError::Unavailable);
            }
            let checked = check(&connection)?;
            finish(
                &mut connection,
                &owner.audit,
                owner.clock.unix_milliseconds(),
            )?;
            work(&mut connection, &owner.audit, checked)
        })
        .await
        .map_err(unavailable)?
    }

    pub(crate) fn delivery_attempt(
        &self,
        registration: Uuid,
        before: Delivery,
        capture: Option<Uuid>,
    ) -> DeliveryAttempt {
        DeliveryAttempt {
            owner: self.owner.clone(),
            id: Uuid::new_v4(),
            registration,
            before,
            capture,
        }
    }

    /// No generic publication/cleanup precedes or follows matching attempt work.
    /// The connection mutex orders pin installation against every ordinary job.
    async fn run_attempt<T: Send + 'static>(
        &self,
        attempt: &DeliveryAttempt,
        operation: AttemptWork,
        work: impl FnOnce(&mut Connection, &Path) -> Result<T, OutboxError> + Send + 'static,
    ) -> Result<T, OutboxError> {
        if !Arc::ptr_eq(&self.owner, &attempt.owner) {
            return Err(OutboxError::Invalid);
        }
        let attempt = attempt.clone();
        let owner = self.owner.clone();
        let permit = owner
            .capacity
            .clone()
            .try_acquire_owned()
            .map_err(|_| OutboxError::Capacity)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut connection = owner.connection.lock().map_err(unavailable)?;
            #[cfg(test)]
            if matches!(operation, AttemptWork::Admit) {
                if let Some(pause) = owner.pin_pause.lock().unwrap().take() {
                    pause.wait(false);
                }
            }
            let pinned = *owner.attempt.lock().map_err(unavailable)?;
            match pinned {
                Some(id) if id != attempt.id => return Err(OutboxError::Unavailable),
                None if matches!(operation, AttemptWork::Response) => {
                    return Err(OutboxError::Conflict);
                }
                None if matches!(operation, AttemptWork::Admit) => {
                    let saved = stored::by_id(&connection, attempt.before.id())?
                        .ok_or(OutboxError::Conflict)?;
                    stored::check_current(&connection, &saved)?;
                    let registration = super::registry::by_id(&connection, attempt.registration)?;
                    let current = super::cursor::read(
                        &connection,
                        attempt.registration,
                        &saved.conversation,
                    )?
                    .ok_or(OutboxError::Invalid)?;
                    if saved.registration != attempt.registration
                        || saved.delivery != attempt.before
                        || !registration.lifecycle.retains_queued()
                        || current.reset_required
                        || current.reset != attempt.capture
                        || saved.source_reset.is_some()
                        || !codec::same_source(&current.scope, &saved.scope)
                    {
                        return Err(OutboxError::Conflict);
                    }
                    *owner.attempt.lock().map_err(unavailable)? = Some(attempt.id);
                }
                None if matches!(operation, AttemptWork::Unposted) => {
                    // Capacity can refuse admission before pin installation.
                    // Protect a new unposted observation before writing it too.
                    if let Some(saved) = stored::by_id(&connection, attempt.before.id())? {
                        if saved.registration != attempt.registration {
                            return Err(OutboxError::Invalid);
                        }
                        if saved.delivery == attempt.before {
                            *owner.attempt.lock().map_err(unavailable)? = Some(attempt.id);
                        }
                    }
                }
                _ => {}
            }
            work(&mut connection, &owner.audit)
        })
        .await
        .map_err(unavailable)?
    }

    /// Called synchronously after the runtime consumes the actual settlement
    /// acknowledgement, with all physical work joined. No Drop path clears it.
    pub(crate) fn acknowledge_attempt(&self, attempt: &DeliveryAttempt) -> Result<(), OutboxError> {
        if !Arc::ptr_eq(&self.owner, &attempt.owner) {
            return Err(OutboxError::Invalid);
        }
        let mut pinned = self.owner.attempt.lock().map_err(unavailable)?;
        match *pinned {
            Some(id) if id == attempt.id => {
                *pinned = None;
                Ok(())
            }
            None => Ok(()), // Admission lost before installation; no POST began.
            Some(_) => Err(OutboxError::Conflict),
        }
    }
}

impl SqlOutbox {
    pub(crate) async fn due_after(
        &self,
        now_ms: u64,
        after: Option<Uuid>,
    ) -> Result<Vec<PendingDelivery>, OutboxError> {
        self.run(move |connection,_| {
            let mut statement=connection.prepare(
                "SELECT d.id,d.registration,d.conversation,d.incarnation,d.position,d.scope,d.state,d.phase,d.due_at,(SELECT reset FROM source_reset_members WHERE delivery=d.id),d.body,
                    r.id,r.credential,r.principal,r.organization,r.owner,r.destination,r.mac_key,r.active,r.stop,r.issued_at
                 FROM deliveries d JOIN registrations r ON d.registration=r.id
                 WHERE d.phase='queued' OR (d.phase='retry' AND d.due_at<=?1)
                 ORDER BY CASE WHEN ?2 IS NULL OR d.registration>?2 THEN 0 ELSE 1 END,d.registration,d.conversation,d.position LIMIT 4"
            ).map_err(unavailable)?;
            let mut rows=statement.query(params![integer(now_ms)?, after.map(|id| id.to_string())]).map_err(unavailable)?;
            let mut result=Vec::new();
            while let Some(row)=rows.next().map_err(unavailable)? {
                let saved=StoredDelivery::row(row).map_err(unavailable)?.checked(connection)?;
                stored::check_current(connection,&saved)?;
                let registration=StoredRegistration::row(row,11).map_err(unavailable)?.checked()?.registration;
                let body:Vec<u8>=row.get(10).map_err(unavailable)?;
                if registration.id!=saved.registration || body.len()>MAX_BODY_BYTES {return Err(OutboxError::Invalid);}
                let capture=super::cursor::read(connection,saved.registration,&saved.conversation)?.ok_or(OutboxError::Invalid)?.reset;
                result.push(PendingDelivery {registration,conversation:saved.conversation,scope:saved.scope,capture,position:saved.position,delivery:saved.delivery,body});
            }
            Ok(result)
        }).await
    }
}

#[async_trait]
impl Outbox for SqlOutbox {
    async fn queue(&self, record: QueuedRecord) -> Result<Uuid, OutboxError> {
        self.run(move |connection, audit| queue::queue(connection, audit, record))
            .await
    }

    async fn due(&self, now_ms: u64) -> Result<Vec<PendingDelivery>, OutboxError> {
        self.due_after(now_ms, None).await
    }

    async fn transition(
        &self,
        before: &Delivery,
        after: &Delivery,
        cause: DeliveryCause,
        at_ms: u64,
    ) -> Result<(), OutboxError> {
        let (before, after) = (before.clone(), after.clone());
        let clock = self.owner.clock.clone();
        self.run(move |connection, audit| {
            write_transition(connection, &before, &after, cause, at_ms)?;
            finish(connection, audit, clock.unix_milliseconds())?;
            Ok(())
        })
        .await
    }

    async fn recover(&self, at_ms: u64) -> Result<(), OutboxError> {
        self.recover_sending(at_ms, DeliveryCause::Restart).await
    }
}

#[cfg(test)]
#[path = "../../../../tests/webhooks/infrastructure/outbox.rs"]
mod tests;

/// Persist one checked replacement; publication and reclamation stay with the caller.
fn write_transition(
    connection: &mut Connection,
    before: &Delivery,
    after: &Delivery,
    cause: DeliveryCause,
    at_ms: u64,
) -> Result<(), OutboxError> {
    if !cause.allows(before, after, at_ms) {
        return Err(OutboxError::Invalid);
    }
    write_transition_before(connection, before, after, cause, at_ms, None)
}

fn write_transition_before(
    connection: &mut Connection,
    before: &Delivery,
    after: &Delivery,
    cause: DeliveryCause,
    at_ms: u64,
    deadline: Option<Instant>,
) -> Result<(), OutboxError> {
    write_transition_commit(
        connection,
        before,
        after,
        cause,
        at_ms,
        TransitionCommit::Ordinary(deadline),
    )
}

fn write_transition_commit(
    connection: &mut Connection,
    before: &Delivery,
    after: &Delivery,
    cause: DeliveryCause,
    at_ms: u64,
    mode: TransitionCommit,
) -> Result<(), OutboxError> {
    let deadline = match mode {
        TransitionCommit::Ordinary(deadline) | TransitionCommit::Attempt(deadline) => deadline,
    };
    if deadline.is_some_and(|at| Instant::now() >= at) {
        return Err(OutboxError::Conflict);
    }
    let transaction = connection.transaction().map_err(unavailable)?;
    let saved = stored::by_id(&transaction, before.id())?.ok_or(OutboxError::Conflict)?;
    stored::check_current(&transaction, &saved)?;
    let changed = transaction
        .execute(
            "UPDATE deliveries SET state=?1 WHERE id=?2 AND state=?3",
            params![
                codec::encode(after)?,
                after.id().to_string(),
                codec::encode(before)?
            ],
        )
        .map_err(unavailable)?;
    if changed != 1 {
        return Err(OutboxError::Conflict);
    }
    delivery_audit(&transaction, after, Some(before), cause, at_ms)?;
    if matches!(mode, TransitionCommit::Attempt(_)) {
        super::journal::commit_observation(transaction, saved.registration, deadline)?;
    } else {
        super::journal::commit_before(transaction, Some(saved.registration), at_ms, deadline)?;
    }
    Ok(())
}

impl SqlOutbox {
    /// The sole serial sender invokes this only after joining physical work
    /// and settling its retained actual outcome. No remaining Sending row has
    /// a live POST owner; reconciliation records its uncertainty without POST.
    pub(crate) async fn recover_live(&self, at_ms: u64) -> Result<(), OutboxError> {
        self.recover_sending(at_ms, DeliveryCause::Reconcile).await
    }
    async fn recover_sending(&self, at_ms: u64, cause: DeliveryCause) -> Result<(), OutboxError> {
        let clock = self.owner.clock.clone();
        self.run(move |connection, audit| {
            loop {
                let next = connection
                    .query_row(
                        &format!(
                            "{} FROM deliveries INDEXED BY ready_deliveries WHERE phase='sending' LIMIT 1",
                            stored::SELECT
                        ),
                        [],
                        StoredDelivery::row,
                    )
                    .optional()
                    .map_err(unavailable)?;
                let Some(saved) = next else {
                    break;
                };
                let saved = saved.checked(connection)?;
                let before = saved.delivery;
                if before.phase() != Phase::Sending {
                    return Err(OutboxError::Invalid);
                }
                let after = before.recovered();
                let transaction = connection.transaction().map_err(unavailable)?;
                let changed = transaction
                    .execute(
                        "UPDATE deliveries SET state=?1 WHERE id=?2 AND state=?3",
                        params![
                            codec::encode(&after)?,
                            after.id().to_string(),
                            saved.saved_state
                        ],
                    )
                    .map_err(unavailable)?;
                if changed != 1 {
                    return Err(OutboxError::Conflict);
                }
                delivery_audit(
                    &transaction,
                    &after,
                    Some(&before),
                    cause,
                    at_ms,
                )?;
                commit(transaction, Some(saved.registration), at_ms)?;
                finish(connection, audit, clock.unix_milliseconds())?;
            }
            finish(connection, audit, clock.unix_milliseconds())
        })
        .await
    }
}

impl SqlOutbox {
    #[cfg(test)]
    pub(crate) fn pause_pin(&self) -> AdmissionControl {
        let pause = Arc::new(AdmissionPause {
            after_commit: false,
            entered: Notify::new(),
            released: Mutex::new(false),
            wake: Condvar::new(),
        });
        assert!(self
            .owner
            .pin_pause
            .lock()
            .unwrap()
            .replace(pause.clone())
            .is_none());
        AdmissionControl(pause)
    }

    #[cfg(test)]
    pub(crate) fn pause_admission(&self, after_commit: bool) -> AdmissionControl {
        let pause = Arc::new(AdmissionPause {
            after_commit,
            entered: Notify::new(),
            released: Mutex::new(false),
            wake: Condvar::new(),
        });
        assert!(self
            .owner
            .admission_pause
            .lock()
            .unwrap()
            .replace(pause.clone())
            .is_none());
        AdmissionControl(pause)
    }

    /// Admission SQL remains owned by the calling sender even after its permit
    /// deadline. Late success is a conditional commit, never permission to POST.
    pub(crate) async fn admit_before(
        &self,
        attempt: &DeliveryAttempt,
        after: &Delivery,
        at_ms: u64,
        deadline: Instant,
    ) -> Result<(), OutboxError> {
        let (before, after) = (attempt.before.clone(), after.clone());
        #[cfg(test)]
        let pause = self.owner.admission_pause.lock().unwrap().take();
        self.run_attempt(attempt, AttemptWork::Admit, move |connection, audit| {
            #[cfg(test)]
            if let Some(pause) = &pause {
                pause.wait(false);
            }
            if !DeliveryCause::Admission.allows(&before, &after, at_ms) {
                return Err(OutboxError::Invalid);
            }
            write_transition_commit(
                connection,
                &before,
                &after,
                DeliveryCause::Admission,
                at_ms,
                TransitionCommit::Attempt(Some(deadline)),
            )?;
            #[cfg(test)]
            if let Some(pause) = &pause {
                pause.wait(true);
            }
            publish(connection, audit)
        })
        .await
    }

    /// A retained attempt that never invoked POST observes actual local state.
    /// This does not acknowledge its earlier failed Sending receipt. Published
    /// retirement can remove the guarded row; that is no current work, not ACK.
    pub(crate) async fn settle_unposted(
        &self,
        attempt: &DeliveryAttempt,
        sending: &Delivery,
        expired: bool,
        at_ms: u64,
    ) -> Result<(), OutboxError> {
        let (before, sending) = (attempt.before.clone(), sending.clone());
        let registration = attempt.registration;
        self.run_attempt(attempt, AttemptWork::Unposted, move |connection, audit| {
            restoration::validate(connection)?;
            let Some(saved) = stored::by_id(connection, before.id())? else {
                return if super::retirement::read(connection, registration)?.is_some() {
                    Ok(())
                } else {
                    Err(OutboxError::Conflict)
                };
            };
            stored::check_current(connection, &saved)?;
            if saved.registration != registration {
                return Err(OutboxError::Invalid);
            }
            let (after, cause) = if saved.delivery == before {
                (
                    before.refused().map_err(super::invalid)?,
                    DeliveryCause::AuditUnavailable,
                )
            } else if saved.delivery == sending {
                if expired {
                    (
                        sending.not_dispatched().map_err(super::invalid)?,
                        DeliveryCause::AdmissionExpired,
                    )
                } else {
                    (sending.recovered(), DeliveryCause::Reconcile)
                }
            } else if matches!(saved.delivery.phase(), Phase::Terminal(_)) {
                // A real conditional stop/settlement won. Do not replace its fact.
                return publish(connection, audit);
            } else {
                return Err(OutboxError::Conflict);
            };
            if !cause.allows(&saved.delivery, &after, at_ms) {
                return Err(OutboxError::Invalid);
            }
            write_transition_commit(
                connection,
                &saved.delivery,
                &after,
                cause,
                at_ms,
                TransitionCommit::Attempt(None),
            )?;
            publish(connection, audit)
        })
        .await
    }

    /// Settle the supervisor's actual observation. An identical retained audit
    /// can prove a prior commit, but absent state/evidence never proves an ACK.
    pub(crate) async fn settle_response(
        &self,
        attempt: &DeliveryAttempt,
        before: &Delivery,
        after: &Delivery,
        at_ms: u64,
    ) -> Result<(), OutboxError> {
        let (prior, observed) = (before.clone(), after.clone());
        let (before, after) = (before.clone(), after.clone());
        if before.id() != attempt.before.id() {
            return Err(OutboxError::Invalid);
        }
        self.run_attempt(attempt, AttemptWork::Response, move |connection, audit| {
            let committed = response_committed(connection, &prior, &observed, at_ms)?;
            if committed {
                return publish(connection, audit);
            }
            write_transition_commit(
                connection,
                &before,
                &after,
                DeliveryCause::Response,
                at_ms,
                TransitionCommit::Attempt(None),
            )?;
            // Do not reclaim this observation before its actual ACK returns.
            // Later normal work still performs the existing bounded cleanup.
            publish(connection, audit)
        })
        .await
    }
}
fn response_committed(
    connection: &Connection,
    before: &Delivery,
    after: &Delivery,
    at_ms: u64,
) -> Result<bool, OutboxError> {
    if !DeliveryCause::Response.allows(before, after, at_ms) {
        return Err(OutboxError::Invalid);
    }
    let current = stored::by_id(connection, before.id())?.ok_or(OutboxError::Conflict)?;
    stored::check_current(connection, &current)?;
    if current.delivery == *before {
        return Ok(false);
    }
    if current.delivery != *after {
        return Err(OutboxError::Conflict);
    }
    // Reuse the whole retained-history owner before any publication or trimming.
    restoration::validate(connection)?;
    let saved = connection.query_row(
        "SELECT id,registration,delivery,record,published,source_reset FROM audit INDEXED BY delivery_audit WHERE delivery=?1 AND json_extract(record,'$.cause')='response' ORDER BY rowid DESC LIMIT 1",
        [before.id().to_string()], SavedAudit::row,
    ).optional().map_err(unavailable)?.ok_or(OutboxError::Conflict)?;
    let (_, text) = saved.checked()?;
    match Record::decode(&text)? {
        Record::Delivery {
            before_state: Some(prior),
            after_state: observed,
            cause: Cause::Response,
            at_ms: actual,
            ..
        } if prior.checked()? == *before && observed.checked()? == *after && actual == at_ms => {
            Ok(true)
        }
        _ => Err(OutboxError::Conflict),
    }
}
