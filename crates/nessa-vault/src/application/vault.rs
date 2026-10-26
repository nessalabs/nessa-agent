use super::{Audit, AuditRecord, Caller, Clock, Repository};
use crate::{
    contract,
    domain::{EntryName, Failure, Passphrase, Secret, Status, VaultKey},
};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex, Weak,
};
/// Observation of a connection's lifetime. Clones do not retain its right to unlock.
#[derive(Clone)]
pub struct Lease(Arc<LeaseState>);
struct LeaseState {
    id: String,
    caller: Caller,
    alive: AtomicBool,
}
/// Unique close guard owned by the connection loop, independently of request clones.
pub struct Session {
    vault: Arc<Vault>,
    lease: Lease,
    closed: bool,
}
impl Session {
    pub fn lease(&self) -> Lease {
        self.lease.clone()
    }
    pub fn close(&mut self) -> Result<(), Failure> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        self.lease.0.alive.store(false, Ordering::SeqCst);
        self.vault.close_lease(&self.lease, "connection_closed")
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        if self.close().is_err() { /* Keys are already cleared; explicit close reports the audit failure. */
        }
    }
}
struct Unlocked {
    key: VaultKey,
    lease: Weak<LeaseState>,
}
struct State {
    unlocked: Option<Unlocked>,
    failures: usize,
    retry_at: u64,
}
/// Session-scoped unlock and reusable secret access; no process-global store.
pub struct Vault {
    repository: Arc<dyn Repository>,
    audit: Arc<dyn Audit>,
    clock: Arc<dyn Clock>,
    state: Mutex<State>,
    derivation: Mutex<()>,
    sequence: AtomicU64,
}
impl Vault {
    pub fn new(
        repository: Arc<dyn Repository>,
        audit: Arc<dyn Audit>,
        clock: Arc<dyn Clock>,
    ) -> Arc<Self> {
        Arc::new(Self {
            repository,
            audit,
            clock,
            state: Mutex::new(State {
                unlocked: None,
                failures: 0,
                retry_at: 0,
            }),
            derivation: Mutex::new(()),
            sequence: AtomicU64::new(0),
        })
    }
    /// `id` is minted by transport composition, never read from request parameters.
    pub fn session(self: &Arc<Self>, id: String, caller: Caller) -> Session {
        Session {
            vault: self.clone(),
            lease: Lease(Arc::new(LeaseState {
                id,
                caller,
                alive: AtomicBool::new(true),
            })),
            closed: false,
        }
    }
    fn state(&self) -> Result<std::sync::MutexGuard<'_, State>, Failure> {
        self.state.lock().map_err(|_| Failure::Unavailable)
    }
    fn live(state: &mut State) -> Option<Arc<LeaseState>> {
        let lease = state
            .unlocked
            .as_ref()
            .and_then(|u| u.lease.upgrade())
            .filter(|lease| lease.alive.load(Ordering::SeqCst));
        lease
    }
    pub fn status(&self) -> Result<Status, Failure> {
        let mut state = self.state()?;
        if Self::live(&mut state).is_some() {
            return Ok(Status::Unlocked);
        }
        drop(state);
        Ok(if self.repository.configured()? {
            Status::Locked
        } else {
            Status::Unconfigured
        })
    }
    fn evidence(
        &self,
        lease: &Lease,
        action: &'static str,
        transition: (Status, Status),
        cause: &'static str,
        result: &'static str,
        target: Option<&EntryName>,
    ) -> Result<(), Failure> {
        self.audit
            .record(&AuditRecord {
                operation: self.sequence.fetch_add(1, Ordering::Relaxed),
                lease: lease.0.id.clone(),
                caller: lease.0.caller.clone(),
                action,
                before: transition.0,
                after: transition.1,
                target: target.cloned(),
                cause,
                result,
            })
            .map_err(|_| Failure::AuditUnavailable)
    }
    fn admit(&self, lease: &Lease) -> Result<(), Failure> {
        if !lease.0.alive.load(Ordering::SeqCst) {
            return Err(Failure::SessionClosed);
        }
        let state = self.state()?;
        if state
            .unlocked
            .as_ref()
            .and_then(|u| u.lease.upgrade())
            .is_some_and(|current| !Arc::ptr_eq(&current, &lease.0))
        {
            return Err(Failure::SessionOwned);
        }
        if self.clock.seconds() < state.retry_at {
            return Err(Failure::Cooldown);
        }
        Ok(())
    }
    pub fn create(
        &self,
        lease: &Lease,
        passphrase: Passphrase,
        initial: &[(EntryName, Secret)],
    ) -> Result<Status, Failure> {
        self.open(lease, passphrase, Some(initial))
    }
    pub fn unlock(&self, lease: &Lease, passphrase: Passphrase) -> Result<Status, Failure> {
        self.open(lease, passphrase, None)
    }
    fn open(
        &self,
        lease: &Lease,
        passphrase: Passphrase,
        initial: Option<&[(EntryName, Secret)]>,
    ) -> Result<Status, Failure> {
        let _derivation = self.derivation.try_lock().map_err(|_| Failure::Busy)?;
        self.admit(lease)?;
        let before = self.status()?;
        let action = if initial.is_some() { "setup" } else { "unlock" };
        self.evidence(
            lease,
            action,
            (before, before),
            "explicit_request",
            "requested",
            None,
        )?;
        let derived = match initial {
            Some(entries) => self.repository.create(&passphrase, entries),
            None => self.repository.unlock(&passphrase),
        };
        let key = match derived {
            Ok(key) => key,
            Err(problem) => {
                {
                    let mut state = self.state()?;
                    if problem == Failure::WrongPassphrase {
                        state.failures += 1;
                        if state.failures >= contract::FAILED_ATTEMPTS {
                            state.retry_at = self
                                .clock
                                .seconds()
                                .saturating_add(contract::COOLDOWN_SECONDS as u64);
                            state.failures = 0;
                        }
                    }
                }
                self.evidence(
                    lease,
                    action,
                    (before, self.status()?),
                    "verification_failed",
                    refusal(problem),
                    None,
                )?;
                return Err(problem);
            }
        };
        // Publication is conditional on the actual connection owner, not any request Arc.
        let mut state = self.state()?;
        if !lease.0.alive.load(Ordering::SeqCst) {
            drop(state);
            self.evidence(
                lease,
                action,
                (before, self.status()?),
                "connection_closed",
                "not_published",
                None,
            )?;
            return Err(Failure::SessionClosed);
        }
        if state
            .unlocked
            .as_ref()
            .and_then(|u| u.lease.upgrade())
            .is_some_and(|current| !Arc::ptr_eq(&current, &lease.0))
        {
            return Err(Failure::SessionOwned);
        }
        self.evidence(
            lease,
            action,
            (before, Status::Unlocked),
            "verified_receipt",
            "published",
            None,
        )?;
        if !lease.0.alive.load(Ordering::SeqCst) {
            return Err(Failure::SessionClosed);
        }
        state.unlocked = Some(Unlocked {
            key,
            lease: Arc::downgrade(&lease.0),
        });
        state.failures = 0;
        state.retry_at = 0;
        Ok(Status::Unlocked)
    }
    /// Locking erases the retained key before reporting an audit failure.
    pub fn lock(&self, lease: &Lease) -> Result<Status, Failure> {
        if !lease.0.alive.load(Ordering::SeqCst) {
            return Err(Failure::SessionClosed);
        }
        {
            let state = self.state()?;
            if state
                .unlocked
                .as_ref()
                .and_then(|u| u.lease.upgrade())
                .is_some_and(|owner| !Arc::ptr_eq(&owner, &lease.0))
            {
                return Err(Failure::SessionOwned);
            }
        }
        self.close_lease(lease, "explicit_request")?;
        self.status()
    }
    fn close_lease(&self, lease: &Lease, cause: &'static str) -> Result<(), Failure> {
        let mut state = self.state()?;
        let owned = state
            .unlocked
            .as_ref()
            .and_then(|value| value.lease.upgrade())
            .is_some_and(|current| Arc::ptr_eq(&current, &lease.0));
        if !owned {
            return Ok(());
        }
        state.unlocked = None;
        drop(state);
        self.evidence(
            lease,
            "lock",
            (Status::Unlocked, Status::Locked),
            cause,
            "key_cleared",
            None,
        )
    }
    fn access<T>(&self, work: impl FnOnce(&VaultKey) -> Result<T, Failure>) -> Result<T, Failure> {
        let mut state = self.state()?;
        Self::live(&mut state).ok_or(Failure::Locked)?;
        let unlocked = state.unlocked.as_ref().ok_or(Failure::Locked)?;
        work(&unlocked.key)
    }
    pub fn read(&self, name: &EntryName) -> Result<Option<Secret>, Failure> {
        self.access(|key| self.repository.read(key, name))
    }
    pub fn write(&self, name: &EntryName, value: &[u8]) -> Result<(), Failure> {
        self.change_entry(name, Some(value))
    }
    pub fn delete(&self, name: &EntryName) -> Result<(), Failure> {
        self.change_entry(name, None)
    }
    fn change_entry(&self, name: &EntryName, value: Option<&[u8]>) -> Result<(), Failure> {
        let mut state = self.state()?;
        let current = Self::live(&mut state).ok_or(Failure::Locked)?;
        let lease = Lease(current);
        let action = if value.is_some() {
            "entry_saved"
        } else {
            "entry_deleted"
        };
        self.evidence(
            &lease,
            action,
            (Status::Unlocked, Status::Unlocked),
            "internal_consumer",
            "requested",
            Some(name),
        )?;
        let key = &state.unlocked.as_ref().ok_or(Failure::Locked)?.key;
        match value {
            Some(value) => self.repository.write(key, name, value)?,
            None => self.repository.delete(key, name)?,
        }
        self.evidence(
            &lease,
            action,
            (Status::Unlocked, Status::Unlocked),
            "encrypted_receipt",
            "committed",
            Some(name),
        )
    }
}
fn refusal(problem: Failure) -> &'static str {
    match problem {
        Failure::WrongPassphrase => "wrong_passphrase_or_ciphertext",
        Failure::AlreadyConfigured => "already_configured",
        Failure::Capacity => "capacity",
        _ => "unavailable",
    }
}
