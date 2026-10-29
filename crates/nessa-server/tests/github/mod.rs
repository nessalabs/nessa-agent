//! The sign-in owner against a scripted GitHub, a store and an audit that
//! can fail, and a timer the test moves. One test at least for each row of
//! the table in `docs/design/github-connect.md`.
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::Notify;

use crate::github::application::{
    ApiFailure, AuditUnavailable, DeviceAuthorization, DeviceCode, EndCause, GithubApi,
    GithubAudit, GithubAuditEvent, GithubAuditRecord, GithubCaller, GithubConnections, GithubError,
    GithubReadError, GithubReadFailure, GithubTokenStore, PollAnswer, StoreUnavailable,
};
use crate::github::domain::{
    AccessToken, ConnectEnd, GithubAccount, GithubReadKind, GithubRepository, GithubSignIn,
    GithubStatus,
};
use crate::timer::{manual::ManualTimer, Moment, Timer};

const INTERVAL: Duration = Duration::from_secs(5);
const LIFETIME: Duration = Duration::from_secs(900);

fn token(value: &str) -> AccessToken {
    AccessToken::new(value.into()).unwrap()
}

fn octo() -> GithubAccount {
    GithubAccount {
        id: 7,
        login: "octo".into(),
    }
}

fn caller(credential: &str) -> GithubCaller {
    GithubCaller {
        organization_id: "org".into(),
        principal_id: "owner".into(),
        credential_id: credential.into(),
    }
}

/// GitHub, answering from a script. A poll with nothing scripted is pending.
struct ScriptedGithub {
    device: Mutex<Result<DeviceAuthorization, ApiFailure>>,
    device_requests: Mutex<usize>,
    hold_device: Mutex<Option<Arc<Notify>>>,
    device_started: Notify,
    polls: Mutex<VecDeque<Result<PollAnswer, ApiFailure>>>,
    polled: Mutex<usize>,
    /// Held polls wait here until the test lets them answer.
    hold_polls: Mutex<Option<Arc<Notify>>>,
    accounts: Mutex<VecDeque<Result<GithubAccount, ApiFailure>>>,
    reads: Mutex<Vec<(AccessToken, GithubRepository, GithubReadKind)>>,
    read_answer: Mutex<Result<Value, GithubReadFailure>>,
    read_started: Notify,
    hold_reads: Mutex<Option<Arc<Notify>>>,
}

impl Default for ScriptedGithub {
    fn default() -> Self {
        Self {
            device: Mutex::new(Ok(DeviceAuthorization {
                device_code: DeviceCode("device".into()),
                user_code: "ABCD-1234".into(),
                verification_uri: "https://github.com/login/device".into(),
                expires_in: LIFETIME,
                interval: INTERVAL,
            })),
            device_requests: Mutex::new(0),
            hold_device: Mutex::default(),
            device_started: Notify::new(),
            polls: Mutex::default(),
            polled: Mutex::new(0),
            hold_polls: Mutex::default(),
            accounts: Mutex::default(),
            reads: Mutex::default(),
            read_answer: Mutex::new(Err(GithubReadFailure::NotFound)),
            read_started: Notify::new(),
            hold_reads: Mutex::new(None),
        }
    }
}

impl ScriptedGithub {
    fn answer(&self, answers: impl IntoIterator<Item = Result<PollAnswer, ApiFailure>>) {
        self.polls.lock().unwrap().extend(answers);
    }
}

#[async_trait]
impl GithubApi for ScriptedGithub {
    async fn request_device_code(&self) -> Result<DeviceAuthorization, ApiFailure> {
        *self.device_requests.lock().unwrap() += 1;
        self.device_started.notify_one();
        let hold = self.hold_device.lock().unwrap().clone();
        if let Some(hold) = hold {
            hold.notified().await;
        }
        self.device.lock().unwrap().clone()
    }
    async fn poll(&self, device_code: &DeviceCode) -> Result<PollAnswer, ApiFailure> {
        assert_eq!(device_code, &DeviceCode("device".into()));
        let hold = self.hold_polls.lock().unwrap().clone();
        if let Some(hold) = hold {
            hold.notified().await;
        }
        *self.polled.lock().unwrap() += 1;
        self.polls
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(PollAnswer::Pending))
    }
    async fn account(&self, _: &AccessToken) -> Result<GithubAccount, ApiFailure> {
        self.accounts
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(octo()))
    }
    async fn read_repository(
        &self,
        token: &AccessToken,
        repository: &GithubRepository,
        kind: GithubReadKind,
    ) -> Result<Value, GithubReadFailure> {
        self.reads
            .lock()
            .unwrap()
            .push((token.clone(), repository.clone(), kind));
        self.read_started.notify_one();
        let hold = self.hold_reads.lock().unwrap().clone();
        if let Some(hold) = hold {
            hold.notified().await;
        }
        self.read_answer.lock().unwrap().clone()
    }
}

#[derive(Default)]
struct MemoryStore {
    held: Mutex<Option<GithubSignIn>>,
    hold_load: Mutex<Option<Arc<Notify>>>,
    load_started: Notify,
    hold_save: Mutex<Option<Arc<Notify>>>,
    save_started: Notify,
    unreadable: Mutex<bool>,
    unwritable: Mutex<bool>,
    /// A save writes the file, then fails (its directory not synced).
    fails_after_writing: Mutex<bool>,
}

#[async_trait]
impl GithubTokenStore for MemoryStore {
    async fn load(&self, _generation: u64) -> Result<Option<GithubSignIn>, StoreUnavailable> {
        let receipt = if *self.unreadable.lock().unwrap() {
            Err(StoreUnavailable)
        } else {
            Ok(self.held.lock().unwrap().clone())
        };
        let hold = self.hold_load.lock().unwrap().clone();
        if let Some(hold) = hold {
            self.load_started.notify_one();
            hold.notified().await;
        }
        receipt
    }
    async fn save(&self, _generation: u64, sign_in: &GithubSignIn) -> Result<(), StoreUnavailable> {
        if *self.unwritable.lock().unwrap() {
            return Err(StoreUnavailable);
        }
        *self.held.lock().unwrap() = Some(sign_in.clone());
        if *self.fails_after_writing.lock().unwrap() {
            return Err(StoreUnavailable);
        }
        let hold = self.hold_save.lock().unwrap().clone();
        if let Some(hold) = hold {
            self.save_started.notify_one();
            hold.notified().await;
        }
        Ok(())
    }
    async fn delete(&self, _generation: u64) -> Result<(), StoreUnavailable> {
        if *self.unwritable.lock().unwrap() {
            return Err(StoreUnavailable);
        }
        *self.held.lock().unwrap() = None;
        Ok(())
    }
}

/// Which events an audit refuses to keep.
type Refusal = fn(&GithubAuditEvent) -> bool;

/// Records every event; refuses the ones `refuse` names.
#[derive(Default)]
struct RecordingAudit {
    hold_connected: Mutex<Option<Arc<Notify>>>,
    connected_started: Notify,
    records: Mutex<Vec<GithubAuditRecord>>,
    refuse: Mutex<Option<Refusal>>,
}

impl RecordingAudit {
    fn events(&self) -> Vec<GithubAuditEvent> {
        self.records
            .lock()
            .unwrap()
            .iter()
            .map(|record| record.event.clone())
            .collect()
    }
}

#[async_trait]
impl GithubAudit for RecordingAudit {
    async fn record(&self, record: &GithubAuditRecord) -> Result<(), AuditUnavailable> {
        let hold = self.hold_connected.lock().unwrap().clone();
        if let Some(hold) =
            hold.filter(|_| matches!(record.event, GithubAuditEvent::Connected { .. }))
        {
            self.connected_started.notify_one();
            hold.notified().await;
        }
        self.record_cleanup(record)
    }
    fn record_cleanup(&self, record: &GithubAuditRecord) -> Result<(), AuditUnavailable> {
        if self
            .refuse
            .lock()
            .unwrap()
            .is_some_and(|refuse| refuse(&record.event))
        {
            return Err(AuditUnavailable);
        }
        self.records.lock().unwrap().push(record.clone());
        Ok(())
    }
}

struct Fixture {
    github: Arc<ScriptedGithub>,
    store: Arc<MemoryStore>,
    audit: Arc<RecordingAudit>,
    timer: Arc<ManualTimer>,
    /// Waits already run out by [`Self::next_poll`].
    seen: Mutex<usize>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            github: Arc::default(),
            store: Arc::default(),
            audit: Arc::default(),
            timer: Arc::default(),
            seen: Mutex::new(0),
        }
    }

    async fn connections(&self) -> GithubConnections {
        GithubConnections::load(
            self.github.clone(),
            self.store.clone(),
            self.store.clone(),
            self.audit.clone(),
            self.timer.clone(),
        )
        .await
    }

    /// The code's deadline, for an attempt started at the timer's origin.
    fn expires_at(&self) -> Moment {
        Moment::from_origin(LIFETIME)
    }

    /// Run out the next wait between polls: the next one started that is
    /// not the code's own deadline.
    async fn next_poll(&self) -> Duration {
        let seen = *self.seen.lock().unwrap();
        let expires_at = self.expires_at();
        let wait = self
            .timer
            .started(|index, wait| index >= seen && wait.deadline != expires_at)
            .await;
        let index = self
            .timer
            .waits()
            .iter()
            .position(|each| *each == wait)
            .unwrap();
        *self.seen.lock().unwrap() = index + 1;
        self.timer.advance_to(wait.deadline);
        wait.limit()
    }

    /// Wait, in real time, until `connections` says what `wanted` accepts.
    async fn settled(
        &self,
        connections: &GithubConnections,
        wanted: impl Fn(&GithubStatus) -> bool,
    ) -> GithubStatus {
        let found = async {
            loop {
                let status = connections.status();
                if wanted(&status) {
                    return status;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        };
        tokio::time::timeout(Duration::from_secs(5), found)
            .await
            .unwrap_or_else(|_| panic!("never settled; now {:?}", connections.status()))
    }
}

fn signed_out(last: Option<ConnectEnd>) -> GithubStatus {
    GithubStatus::SignedOut {
        last,
        unrecorded: false,
    }
}

/// Signed out by `end`, which the audit refused to keep.
fn unrecorded(end: ConnectEnd) -> GithubStatus {
    GithubStatus::SignedOut {
        last: Some(end),
        unrecorded: true,
    }
}

fn connected() -> GithubStatus {
    GithubStatus::Connected {
        account: octo(),
        unrecorded: false,
    }
}

#[tokio::test]
async fn a_kept_sign_in_is_held_again_and_an_unreadable_store_says_so() {
    let fixture = Fixture::new();
    assert_eq!(fixture.connections().await.status(), signed_out(None));
    *fixture.store.held.lock().unwrap() = Some(GithubSignIn {
        account: octo(),
        token: token("ghu_kept"),
    });
    let connections = fixture.connections().await;
    assert_eq!(connections.status(), connected());
    assert_eq!(connections.token(), Some(token("ghu_kept")));
    *fixture.store.unreadable.lock().unwrap() = true;
    let connections = fixture.connections().await;
    assert_eq!(
        connections.status(),
        signed_out(Some(ConnectEnd::StoreUnavailable))
    );
    assert_eq!(connections.token(), None);
}

#[tokio::test]
async fn an_approved_code_signs_in_records_first_and_gives_agents_the_token() {
    let fixture = Fixture::new();
    let connections = fixture.connections().await;
    let waiting = connections.connect(caller("panel")).await.unwrap();
    assert_eq!(
        waiting,
        GithubStatus::Connecting {
            user_code: "ABCD-1234".into(),
            verification_uri: "https://github.com/login/device".into(),
            expires_in_seconds: 900,
        }
    );
    // Asking again answers the same code and asks GitHub for no other.
    assert_eq!(connections.connect(caller("other")).await.unwrap(), waiting);
    assert_eq!(*fixture.github.device_requests.lock().unwrap(), 1);
    assert_eq!(connections.token(), None);
    fixture.github.answer([
        Ok(PollAnswer::Pending),
        Ok(PollAnswer::Granted(token("ghu_new"))),
    ]);
    assert_eq!(fixture.next_poll().await, INTERVAL);
    assert_eq!(fixture.next_poll().await, INTERVAL);
    fixture
        .settled(&connections, |status| *status == connected())
        .await;
    assert_eq!(connections.token(), Some(token("ghu_new")));
    assert_eq!(
        fixture.store.held.lock().unwrap().clone(),
        Some(GithubSignIn {
            account: octo(),
            token: token("ghu_new")
        })
    );
    assert_eq!(
        fixture.audit.events(),
        [
            GithubAuditEvent::ConnectRequested,
            GithubAuditEvent::Connected { account: octo() }
        ]
    );
    let records = fixture.audit.records.lock().unwrap().clone();
    assert_eq!(records[0].operation_id, records[1].operation_id);
    assert_eq!(records[1].caller, caller("panel"), "the starter's sign-in");
    // Connecting again while signed in changes nothing.
    assert_eq!(
        connections.connect(caller("panel")).await.unwrap(),
        connected()
    );
    assert_eq!(*fixture.github.device_requests.lock().unwrap(), 1);
}

#[tokio::test]
async fn slow_down_waits_longer_and_a_lost_answer_is_asked_again() {
    let fixture = Fixture::new();
    let connections = fixture.connections().await;
    connections.connect(caller("panel")).await.unwrap();
    fixture.github.answer([
        Ok(PollAnswer::SlowDown {
            interval: Duration::from_secs(10),
        }),
        Err(ApiFailure::Unreachable),
        Err(ApiFailure::Malformed),
        Ok(PollAnswer::Granted(token("ghu_late"))),
    ]);
    assert_eq!(fixture.next_poll().await, INTERVAL);
    assert_eq!(fixture.next_poll().await, Duration::from_secs(10));
    assert_eq!(fixture.next_poll().await, Duration::from_secs(10));
    assert_eq!(fixture.next_poll().await, Duration::from_secs(10));
    fixture
        .settled(&connections, |status| *status == connected())
        .await;
}

#[tokio::test]
async fn an_approval_whose_account_cannot_be_read_yet_is_read_again() {
    let fixture = Fixture::new();
    let connections = fixture.connections().await;
    connections.connect(caller("panel")).await.unwrap();
    fixture
        .github
        .answer([Ok(PollAnswer::Granted(token("ghu_x")))]);
    fixture
        .github
        .accounts
        .lock()
        .unwrap()
        .push_back(Err(ApiFailure::Unreachable));
    fixture.next_poll().await;
    fixture.next_poll().await;
    fixture
        .settled(&connections, |status| *status == connected())
        .await;
    assert_eq!(*fixture.github.polled.lock().unwrap(), 1, "polled once");
}

#[tokio::test]
async fn a_refusal_an_expiry_or_a_refused_app_ends_the_attempt_signed_out() {
    for (answer, end) in [
        (Ok(PollAnswer::Denied), ConnectEnd::Denied),
        (Ok(PollAnswer::Expired), ConnectEnd::Expired),
        (Err(ApiFailure::Rejected), ConnectEnd::Rejected),
    ] {
        let fixture = Fixture::new();
        let connections = fixture.connections().await;
        connections.connect(caller("panel")).await.unwrap();
        fixture.github.answer([answer]);
        fixture.next_poll().await;
        fixture
            .settled(&connections, |status| *status == signed_out(Some(end)))
            .await;
        assert_eq!(
            fixture.audit.events(),
            [
                GithubAuditEvent::ConnectRequested,
                GithubAuditEvent::ConnectEnded {
                    end,
                    cause: EndCause::GithubAnswered
                }
            ]
        );
        assert_eq!(connections.token(), None);
    }
}

#[tokio::test]
async fn an_end_the_audit_refuses_still_ends_the_attempt_and_says_so() {
    for (answer, end) in [
        (Ok(PollAnswer::Denied), ConnectEnd::Denied),
        (Err(ApiFailure::Rejected), ConnectEnd::Rejected),
    ] {
        let fixture = Fixture::new();
        *fixture.audit.refuse.lock().unwrap() =
            Some(|event| matches!(event, GithubAuditEvent::ConnectEnded { .. }));
        let connections = fixture.connections().await;
        connections.connect(caller("panel")).await.unwrap();
        fixture.github.answer([answer]);
        fixture.next_poll().await;
        // The cause stands, and the status says it is not on record.
        fixture
            .settled(&connections, |status| *status == unrecorded(end))
            .await;
        assert_eq!(connections.token(), None);
    }
    // A cancel the audit refuses still stops the code, refuses, and says so.
    let fixture = Fixture::new();
    *fixture.audit.refuse.lock().unwrap() =
        Some(|event| matches!(event, GithubAuditEvent::ConnectCancelled { .. }));
    let connections = fixture.connections().await;
    connections.connect(caller("panel")).await.unwrap();
    assert_eq!(
        connections.cancel(caller("panel")).await,
        Err(GithubError::AuditUnavailable)
    );
    assert_eq!(connections.status(), unrecorded(ConnectEnd::Cancelled));
}

#[tokio::test]
async fn a_token_written_before_its_save_failed_is_deleted_and_never_held_again() {
    let fixture = Fixture::new();
    *fixture.store.fails_after_writing.lock().unwrap() = true;
    let connections = fixture.connections().await;
    connections.connect(caller("panel")).await.unwrap();
    fixture
        .github
        .answer([Ok(PollAnswer::Granted(token("ghu_x")))]);
    fixture.next_poll().await;
    fixture
        .settled(&connections, |status| {
            *status == signed_out(Some(ConnectEnd::StoreUnavailable))
        })
        .await;
    assert_eq!(fixture.store.held.lock().unwrap().clone(), None, "deleted");
    assert_eq!(
        fixture.audit.events().last(),
        Some(&GithubAuditEvent::ConnectEnded {
            end: ConnectEnd::StoreUnavailable,
            cause: EndCause::StoreFailed
        })
    );
    // The next start does not find it.
    assert_eq!(fixture.connections().await.token(), None);
}

#[tokio::test]
async fn a_disconnect_while_signed_out_deletes_a_stray_token() {
    let fixture = Fixture::new();
    let connections = fixture.connections().await;
    // A token the owner does not hold, left on disk by an earlier failure.
    *fixture.store.held.lock().unwrap() = Some(GithubSignIn {
        account: octo(),
        token: token("ghu_stray"),
    });
    assert_eq!(
        connections.disconnect(caller("panel")).await,
        Ok(signed_out(None))
    );
    assert_eq!(fixture.store.held.lock().unwrap().clone(), None);
    // One that cannot be deleted is refused, as a held one's would be.
    *fixture.store.held.lock().unwrap() = Some(GithubSignIn {
        account: octo(),
        token: token("ghu_stray"),
    });
    *fixture.store.unwritable.lock().unwrap() = true;
    assert_eq!(
        connections.disconnect(caller("panel")).await,
        Err(GithubError::StoreUnavailable)
    );
}

#[tokio::test]
async fn a_refused_token_on_the_account_read_is_a_refused_app() {
    let fixture = Fixture::new();
    let connections = fixture.connections().await;
    connections.connect(caller("panel")).await.unwrap();
    fixture
        .github
        .answer([Ok(PollAnswer::Granted(token("ghu_x")))]);
    fixture
        .github
        .accounts
        .lock()
        .unwrap()
        .push_back(Err(ApiFailure::Unauthorized));
    fixture.next_poll().await;
    fixture
        .settled(&connections, |status| {
            *status == signed_out(Some(ConnectEnd::Rejected))
        })
        .await;
    assert_eq!(fixture.store.held.lock().unwrap().clone(), None);
}

#[tokio::test]
async fn the_code_running_out_ends_the_attempt_expired() {
    let fixture = Fixture::new();
    let connections = fixture.connections().await;
    connections.connect(caller("panel")).await.unwrap();
    fixture.timer.advance_to(fixture.expires_at());
    fixture
        .settled(&connections, |status| {
            *status == signed_out(Some(ConnectEnd::Expired))
        })
        .await;
    // The deadline here decided it, not an answer from GitHub.
    assert_eq!(
        fixture.audit.events().last(),
        Some(&GithubAuditEvent::ConnectEnded {
            end: ConnectEnd::Expired,
            cause: EndCause::DeadlinePassed
        })
    );
}

/// G12: the person approved, but whose it is never came back before the
/// code's time ran out. That is GitHub out of reach, not the code expiring.
#[tokio::test]
async fn an_approval_whose_account_never_comes_back_ends_unreachable() {
    let fixture = Fixture::new();
    let connections = fixture.connections().await;
    connections.connect(caller("panel")).await.unwrap();
    fixture
        .github
        .answer([Ok(PollAnswer::Granted(token("ghu_x")))]);
    fixture
        .github
        .accounts
        .lock()
        .unwrap()
        .extend(std::iter::repeat_n(Err(ApiFailure::Unreachable), 10_000));
    fixture.next_poll().await;
    fixture.timer.advance_to(fixture.expires_at());
    fixture
        .settled(&connections, |status| {
            *status == signed_out(Some(ConnectEnd::Unreachable))
        })
        .await;
    assert_eq!(connections.token(), None);
    assert_eq!(
        fixture.audit.events().last(),
        Some(&GithubAuditEvent::ConnectEnded {
            end: ConnectEnd::Unreachable,
            cause: EndCause::DeadlinePassed
        })
    );
}

#[tokio::test]
async fn a_cancelled_attempts_late_approval_is_dropped() {
    let fixture = Fixture::new();
    let connections = fixture.connections().await;
    connections.connect(caller("panel")).await.unwrap();
    let hold = Arc::new(Notify::new());
    *fixture.github.hold_polls.lock().unwrap() = Some(hold.clone());
    fixture
        .github
        .answer([Ok(PollAnswer::Granted(token("ghu_late")))]);
    fixture.next_poll().await;
    // The poll is with GitHub when another window cancels.
    assert_eq!(
        connections.cancel(caller("other")).await.unwrap(),
        signed_out(Some(ConnectEnd::Cancelled))
    );
    hold.notify_waiters();
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(
        connections.status(),
        signed_out(Some(ConnectEnd::Cancelled))
    );
    assert_eq!(connections.token(), None);
    assert_eq!(fixture.store.held.lock().unwrap().clone(), None);
    let records = fixture.audit.records.lock().unwrap().clone();
    assert_eq!(
        records.last().unwrap().event,
        GithubAuditEvent::ConnectCancelled {
            by: caller("other")
        }
    );
    assert_eq!(records.last().unwrap().caller, caller("panel"));
    // Nothing waits, so a second cancel changes nothing.
    assert_eq!(
        connections.cancel(caller("other")).await.unwrap(),
        signed_out(Some(ConnectEnd::Cancelled))
    );
    assert_eq!(fixture.audit.events().len(), 2);
}

#[tokio::test]
async fn github_out_of_reach_or_refusing_the_app_starts_nothing() {
    for (failure, error, end, cause) in [
        (
            ApiFailure::Unreachable,
            GithubError::Unreachable,
            ConnectEnd::Unreachable,
            EndCause::GithubUnreachable,
        ),
        (
            ApiFailure::Rejected,
            GithubError::Rejected,
            ConnectEnd::Rejected,
            EndCause::GithubAnswered,
        ),
    ] {
        let fixture = Fixture::new();
        *fixture.github.device.lock().unwrap() = Err(failure);
        let connections = fixture.connections().await;
        assert_eq!(connections.connect(caller("panel")).await, Err(error));
        assert_eq!(connections.status(), signed_out(Some(end)));
        assert_eq!(
            fixture.audit.events(),
            [
                GithubAuditEvent::ConnectRequested,
                GithubAuditEvent::ConnectEnded { end, cause }
            ]
        );
        assert!(fixture.timer.waits().is_empty(), "nothing polls");
        // Its end not on record, the answer still says why nothing started.
        let fixture = Fixture::new();
        *fixture.github.device.lock().unwrap() = Err(failure);
        *fixture.audit.refuse.lock().unwrap() =
            Some(|event| matches!(event, GithubAuditEvent::ConnectEnded { .. }));
        let connections = fixture.connections().await;
        assert_eq!(connections.connect(caller("panel")).await, Err(error));
        assert_eq!(connections.status(), unrecorded(end));
    }
}

#[tokio::test]
async fn a_connect_that_cannot_be_recorded_asks_github_nothing() {
    let fixture = Fixture::new();
    *fixture.audit.refuse.lock().unwrap() =
        Some(|event| matches!(event, GithubAuditEvent::ConnectRequested));
    let connections = fixture.connections().await;
    assert_eq!(
        connections.connect(caller("panel")).await,
        Err(GithubError::AuditUnavailable)
    );
    assert_eq!(*fixture.github.device_requests.lock().unwrap(), 0);
    assert_eq!(connections.status(), signed_out(None));
}

#[tokio::test]
async fn an_approval_that_cannot_be_recorded_or_stored_is_dropped() {
    for (audit_fails, end) in [
        (true, ConnectEnd::AuditUnavailable),
        (false, ConnectEnd::StoreUnavailable),
    ] {
        let fixture = Fixture::new();
        if audit_fails {
            *fixture.audit.refuse.lock().unwrap() =
                Some(|event| matches!(event, GithubAuditEvent::Connected { .. }));
        } else {
            *fixture.store.unwritable.lock().unwrap() = true;
        }
        let connections = fixture.connections().await;
        connections.connect(caller("panel")).await.unwrap();
        fixture
            .github
            .answer([Ok(PollAnswer::Granted(token("ghu_x")))]);
        fixture.next_poll().await;
        fixture
            .settled(&connections, |status| *status == signed_out(Some(end)))
            .await;
        assert_eq!(connections.token(), None);
        assert_eq!(fixture.store.held.lock().unwrap().clone(), None);
    }
}

fn never_stopped() -> tokio::sync::watch::Receiver<bool> {
    let (send, receive) = tokio::sync::watch::channel(false);
    // A retained false sender represents a live call in these positive tests.
    tokio::spawn(async move { send.closed().await });
    receive
}

async fn signed_in(fixture: &Fixture) -> GithubConnections {
    *fixture.store.held.lock().unwrap() = Some(GithubSignIn {
        account: octo(),
        token: token("ghu_kept"),
    });
    fixture.connections().await
}

#[tokio::test]
async fn a_disconnect_records_then_deletes_and_stops_giving_the_token() {
    let fixture = Fixture::new();
    let connections = signed_in(&fixture).await;
    assert_eq!(
        connections.disconnect(caller("panel")).await.unwrap(),
        signed_out(None)
    );
    assert_eq!(connections.token(), None);
    assert_eq!(fixture.store.held.lock().unwrap().clone(), None);
    assert_eq!(
        fixture.audit.events(),
        [
            GithubAuditEvent::DisconnectRequested { account: octo() },
            GithubAuditEvent::Disconnected
        ]
    );
    // Signed out, it changes nothing.
    assert_eq!(
        connections.disconnect(caller("panel")).await.unwrap(),
        signed_out(None)
    );
    assert_eq!(fixture.audit.events().len(), 2);
}

#[tokio::test]
async fn a_disconnect_that_cannot_be_recorded_or_deleted_keeps_the_sign_in() {
    let fixture = Fixture::new();
    *fixture.audit.refuse.lock().unwrap() =
        Some(|event| matches!(event, GithubAuditEvent::DisconnectRequested { .. }));
    let connections = signed_in(&fixture).await;
    assert_eq!(
        connections.disconnect(caller("panel")).await,
        Err(GithubError::AuditUnavailable)
    );
    assert_eq!(connections.status(), connected());
    assert!(fixture.store.held.lock().unwrap().is_some());

    let fixture = Fixture::new();
    let connections = signed_in(&fixture).await;
    *fixture.store.unwritable.lock().unwrap() = true;
    assert_eq!(
        connections.disconnect(caller("panel")).await,
        Err(GithubError::StoreUnavailable)
    );
    assert_eq!(connections.status(), connected());
    assert_eq!(connections.token(), Some(token("ghu_kept")));
    assert_eq!(
        fixture.audit.events(),
        [
            GithubAuditEvent::DisconnectRequested { account: octo() },
            GithubAuditEvent::DisconnectFailed
        ]
    );
}

#[tokio::test]
async fn a_failed_disconnect_the_audit_refuses_keeps_the_sign_in_and_says_so() {
    let fixture = Fixture::new();
    let connections = signed_in(&fixture).await;
    *fixture.audit.refuse.lock().unwrap() =
        Some(|event| matches!(event, GithubAuditEvent::DisconnectFailed));
    *fixture.store.unwritable.lock().unwrap() = true;
    assert_eq!(
        connections.disconnect(caller("panel")).await,
        Err(GithubError::StoreUnavailable)
    );
    assert_eq!(
        connections.status(),
        GithubStatus::Connected {
            account: octo(),
            unrecorded: true,
        }
    );
    assert_eq!(connections.token(), Some(token("ghu_kept")));
}

#[tokio::test]
async fn a_disconnect_the_audit_refuses_still_signs_out_and_says_so() {
    let fixture = Fixture::new();
    let connections = signed_in(&fixture).await;
    *fixture.audit.refuse.lock().unwrap() =
        Some(|event| matches!(event, GithubAuditEvent::Disconnected));
    assert_eq!(
        connections.disconnect(caller("panel")).await,
        Err(GithubError::AuditUnavailable)
    );
    assert_eq!(
        connections.status(),
        GithubStatus::SignedOut {
            last: None,
            unrecorded: true,
        }
    );
    assert_eq!(connections.token(), None);
    assert_eq!(fixture.store.held.lock().unwrap().clone(), None);
}

#[tokio::test]
async fn a_disconnect_while_waiting_cancels_the_attempt() {
    let fixture = Fixture::new();
    let connections = fixture.connections().await;
    connections.connect(caller("panel")).await.unwrap();
    assert_eq!(
        connections.disconnect(caller("panel")).await.unwrap(),
        signed_out(Some(ConnectEnd::Cancelled))
    );
    // A new attempt starts afresh.
    assert!(matches!(
        connections.connect(caller("panel")).await.unwrap(),
        GithubStatus::Connecting { .. }
    ));
    assert_eq!(*fixture.github.device_requests.lock().unwrap(), 2);
}

#[tokio::test]
async fn the_time_left_is_counted_down_on_the_timer() {
    let fixture = Fixture::new();
    let connections = fixture.connections().await;
    connections.connect(caller("panel")).await.unwrap();
    fixture
        .timer
        .advance_to(Moment::from_origin(Duration::from_secs(4)));
    assert!(matches!(
        connections.status(),
        GithubStatus::Connecting {
            expires_in_seconds: 896,
            ..
        }
    ));
    // Moving the timer did not reach the first poll's wait.
    assert_eq!(
        fixture.timer.now(),
        Moment::from_origin(Duration::from_secs(4))
    );
}

impl crate::github::application::GithubAccess for MemoryStore {
    fn generation(&self) -> Option<u64> {
        Some(1)
    }
}

struct ChangingVault(std::sync::atomic::AtomicU64);
impl crate::github::application::GithubAccess for ChangingVault {
    fn generation(&self) -> Option<u64> {
        match self.0.load(std::sync::atomic::Ordering::SeqCst) {
            0 => None,
            value => Some(value),
        }
    }
}
#[tokio::test]
async fn a_device_code_from_before_lock_cannot_publish_in_a_new_unlock() {
    for reopened in [false, true] {
        let f = Fixture::new();
        let hold = Arc::new(Notify::new());
        *f.github.hold_device.lock().unwrap() = Some(hold.clone());
        let access = Arc::new(ChangingVault(std::sync::atomic::AtomicU64::new(1)));
        let connections = Arc::new(
            GithubConnections::load(
                f.github.clone(),
                access.clone(),
                f.store.clone(),
                f.audit.clone(),
                f.timer.clone(),
            )
            .await,
        );
        let started = {
            let connections = connections.clone();
            tokio::spawn(async move { connections.connect(caller("desktop")).await })
        };
        tokio::time::timeout(Duration::from_secs(5), f.github.device_started.notified())
            .await
            .unwrap();
        access.0.store(0, std::sync::atomic::Ordering::SeqCst);
        connections.clear_locked().unwrap();
        if reopened {
            access.0.store(2, std::sync::atomic::Ordering::SeqCst);
        }
        hold.notify_one();
        assert!(matches!(
            started.await.unwrap(),
            Err(GithubError::StoreUnavailable)
        ));
        assert!(matches!(
            connections.status(),
            GithubStatus::SignedOut { .. }
        ));
        assert_eq!(*f.github.polled.lock().unwrap(), 0);
        assert!(f.audit.records.lock().unwrap().iter().any(|r| matches!(
            r.event,
            GithubAuditEvent::ConnectEnded {
                cause: EndCause::VaultLocked,
                ..
            }
        )));
    }
}

#[tokio::test]
async fn stale_connected_cache_is_not_reused_and_old_cleanup_preserves_new_restore() {
    let f = Fixture::new();
    *f.store.held.lock().unwrap() = Some(GithubSignIn {
        account: octo(),
        token: token("private-test-token"),
    });
    let access = Arc::new(ChangingVault(std::sync::atomic::AtomicU64::new(1)));
    let connections = GithubConnections::load(
        f.github.clone(),
        access.clone(),
        f.store.clone(),
        f.audit.clone(),
        f.timer.clone(),
    )
    .await;
    assert!(connections.token().is_some());
    access.0.store(2, std::sync::atomic::Ordering::SeqCst);
    assert!(connections.token().is_none());
    assert!(matches!(
        connections.status(),
        GithubStatus::SignedOut { .. }
    ));
    connections.restore().await.unwrap();
    assert!(connections.token().is_some());
    connections.clear_locked().unwrap();
    assert!(connections.token().is_some());
}

#[tokio::test]
async fn stale_attempt_retirement_is_once_attributed_and_cleanup_failure_still_drops_codes() {
    for fail in [false, true] {
        let f = Fixture::new();
        let access = Arc::new(ChangingVault(std::sync::atomic::AtomicU64::new(1)));
        let connections = GithubConnections::load(
            f.github.clone(),
            access.clone(),
            f.store.clone(),
            f.audit.clone(),
            f.timer.clone(),
        )
        .await;
        connections
            .connect(caller("original-desktop"))
            .await
            .unwrap();
        if fail {
            *f.audit.refuse.lock().unwrap() =
                Some(|e| matches!(e, GithubAuditEvent::ConnectEnded { .. }));
        }
        access.0.store(2, std::sync::atomic::Ordering::SeqCst);
        let closed = connections.clear_locked();
        assert_eq!(closed.is_err(), fail);
        connections.clear_locked().unwrap();
        assert!(matches!(
            connections.status(),
            GithubStatus::SignedOut { .. }
        ));
        assert!(connections.token().is_none());
        let rows = f.audit.records.lock().unwrap();
        let ends: Vec<_> = rows
            .iter()
            .filter(|r| {
                matches!(
                    r.event,
                    GithubAuditEvent::ConnectEnded {
                        cause: EndCause::VaultLocked,
                        ..
                    }
                )
            })
            .collect();
        assert_eq!(ends.len(), usize::from(!fail));
        if let Some(end) = ends.first() {
            assert_eq!(end.caller, caller("original-desktop"));
            assert_eq!(end.operation_id, rows[0].operation_id);
        }
        assert_eq!(*f.github.polled.lock().unwrap(), 0);
    }
}

#[tokio::test]
async fn a_restore_receipt_from_an_old_generation_cannot_revive_its_token() {
    let f = Fixture::new();
    *f.store.held.lock().unwrap() = Some(GithubSignIn {
        account: octo(),
        token: token("old-receipt"),
    });
    let access = Arc::new(ChangingVault(std::sync::atomic::AtomicU64::new(1)));
    let connections = Arc::new(
        GithubConnections::load(
            f.github.clone(),
            access.clone(),
            f.store.clone(),
            f.audit.clone(),
            f.timer.clone(),
        )
        .await,
    );
    access.0.store(2, std::sync::atomic::Ordering::SeqCst);
    let hold = Arc::new(Notify::new());
    *f.store.hold_load.lock().unwrap() = Some(hold.clone());
    let pending = {
        let connections = connections.clone();
        tokio::spawn(async move { connections.restore().await })
    };
    tokio::time::timeout(Duration::from_secs(5), f.store.load_started.notified())
        .await
        .unwrap();
    access.0.store(3, std::sync::atomic::Ordering::SeqCst);
    connections.clear_locked().unwrap();
    *f.store.held.lock().unwrap() = Some(GithubSignIn {
        account: octo(),
        token: token("current-receipt"),
    });
    hold.notify_one();
    assert!(matches!(
        pending.await.unwrap(),
        Err(GithubError::StoreUnavailable)
    ));
    assert!(connections.token().is_none());
    *f.store.hold_load.lock().unwrap() = None;
    connections.restore().await.unwrap();
    assert_eq!(connections.token().unwrap().expose(), "current-receipt");
}

#[tokio::test]
async fn new_restore_does_not_wait_for_or_get_erased_by_an_old_provider_request() {
    for rejected in [false, true] {
        let f = Fixture::new();
        let hold = Arc::new(Notify::new());
        *f.github.hold_device.lock().unwrap() = Some(hold.clone());
        if rejected {
            *f.github.device.lock().unwrap() = Err(ApiFailure::Unreachable);
        }
        let access = Arc::new(ChangingVault(std::sync::atomic::AtomicU64::new(1)));
        let connections = Arc::new(
            GithubConnections::load(
                f.github.clone(),
                access.clone(),
                f.store.clone(),
                f.audit.clone(),
                f.timer.clone(),
            )
            .await,
        );
        let pending = {
            let connections = connections.clone();
            tokio::spawn(async move { connections.connect(caller("departed-desktop")).await })
        };
        tokio::time::timeout(Duration::from_secs(5), f.github.device_started.notified())
            .await
            .unwrap();
        access.0.store(2, std::sync::atomic::Ordering::SeqCst);
        *f.store.held.lock().unwrap() = Some(GithubSignIn {
            account: octo(),
            token: token("new-session-token"),
        });
        tokio::time::timeout(Duration::from_secs(5), connections.restore())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(connections.token().unwrap().expose(), "new-session-token");
        hold.notify_one();
        assert!(pending.await.unwrap().is_err());
        assert_eq!(connections.token().unwrap().expose(), "new-session-token");
    }
}

#[tokio::test]
async fn same_generation_disconnect_follows_restore_and_cannot_be_undone_by_its_receipt() {
    let f = Fixture::new();
    let access = Arc::new(ChangingVault(std::sync::atomic::AtomicU64::new(1)));
    let connections = Arc::new(
        GithubConnections::load(
            f.github.clone(),
            access.clone(),
            f.store.clone(),
            f.audit.clone(),
            f.timer.clone(),
        )
        .await,
    );
    access.0.store(2, std::sync::atomic::Ordering::SeqCst);
    *f.store.held.lock().unwrap() = Some(GithubSignIn {
        account: octo(),
        token: token("captured-before-disconnect"),
    });
    let hold = Arc::new(Notify::new());
    *f.store.hold_load.lock().unwrap() = Some(hold.clone());
    let restore = {
        let connections = connections.clone();
        tokio::spawn(async move { connections.restore().await })
    };
    tokio::time::timeout(Duration::from_secs(5), f.store.load_started.notified())
        .await
        .unwrap();
    // Poll the concurrent operation once so its admission is before the receipt.
    let disconnect = connections.disconnect(caller("disconnecting-desktop"));
    tokio::pin!(disconnect);
    tokio::select! {
        biased;
        result = &mut disconnect => panic!("disconnect passed an in-flight restore: {result:?}"),
        _ = std::future::ready(()) => {}
    }
    hold.notify_one();
    restore.await.unwrap().unwrap();
    disconnect.await.unwrap();
    assert!(connections.token().is_none());
    assert!(f.store.held.lock().unwrap().is_none());
    assert!(matches!(
        connections.status(),
        GithubStatus::SignedOut { .. }
    ));
}

#[tokio::test]
async fn a_failed_restore_cannot_erase_a_same_generation_connect_owner() {
    let f = Fixture::new();
    let access = Arc::new(ChangingVault(std::sync::atomic::AtomicU64::new(1)));
    let connections = Arc::new(
        GithubConnections::load(
            f.github.clone(),
            access.clone(),
            f.store.clone(),
            f.audit.clone(),
            f.timer.clone(),
        )
        .await,
    );
    access.0.store(2, std::sync::atomic::Ordering::SeqCst);
    *f.store.unreadable.lock().unwrap() = true;
    let hold = Arc::new(Notify::new());
    *f.store.hold_load.lock().unwrap() = Some(hold.clone());
    let restore = {
        let connections = connections.clone();
        tokio::spawn(async move { connections.restore().await })
    };
    tokio::time::timeout(Duration::from_secs(5), f.store.load_started.notified())
        .await
        .unwrap();
    let connect = connections.connect(caller("current-desktop"));
    tokio::pin!(connect);
    tokio::select! {
        biased;
        result = &mut connect => panic!("connect passed an in-flight restore: {result:?}"),
        _ = std::future::ready(()) => {}
    }
    hold.notify_one();
    assert!(matches!(
        restore.await.unwrap(),
        Err(GithubError::StoreUnavailable)
    ));
    connect.await.unwrap();
    assert!(matches!(
        connections.status(),
        GithubStatus::Connecting { .. }
    ));
    connections.cancel(caller("current-desktop")).await.unwrap();
    let rows = f.audit.records.lock().unwrap();
    let requested = rows
        .iter()
        .find(|row| matches!(row.event, GithubAuditEvent::ConnectRequested))
        .unwrap();
    let cancelled = rows
        .iter()
        .find(|row| matches!(row.event, GithubAuditEvent::ConnectCancelled { .. }))
        .unwrap();
    assert_eq!(requested.operation_id, cancelled.operation_id);
    assert_eq!(cancelled.caller, caller("current-desktop"));
}

#[tokio::test]
async fn approved_sign_in_stays_connecting_until_audit_and_store_receipts() {
    for audit_wait in [true, false] {
        let f = Fixture::new();
        let hold = Arc::new(Notify::new());
        if audit_wait {
            *f.audit.hold_connected.lock().unwrap() = Some(hold.clone());
        } else {
            *f.store.hold_save.lock().unwrap() = Some(hold.clone());
        }
        let connections = f.connections().await;
        connections.connect(caller("desktop")).await.unwrap();
        f.github
            .answer([Ok(PollAnswer::Granted(token("approved-token")))]);
        f.next_poll().await;
        let reached = if audit_wait {
            &f.audit.connected_started
        } else {
            &f.store.save_started
        };
        tokio::time::timeout(Duration::from_secs(5), reached.notified())
            .await
            .unwrap();
        let waiting = connections.status();
        let exposed = connections.token();
        hold.notify_one();
        assert!(
            matches!(waiting, GithubStatus::Connecting { .. }),
            "premature terminal status during audit_wait={audit_wait}: {waiting:?}"
        );
        assert!(exposed.is_none(), "no credential before both receipts");
        f.settled(&connections, |status| {
            matches!(status, GithubStatus::Connected { .. })
        })
        .await;
        assert_eq!(connections.token().unwrap().expose(), "approved-token");
    }
}

#[tokio::test]
async fn disconnect_during_approved_receipts_waits_and_removes_the_saved_sign_in() {
    for audit_wait in [true, false] {
        let f = Fixture::new();
        let hold = Arc::new(Notify::new());
        if audit_wait {
            *f.audit.hold_connected.lock().unwrap() = Some(hold.clone());
        } else {
            *f.store.hold_save.lock().unwrap() = Some(hold.clone());
        }
        let connections = f.connections().await;
        connections.connect(caller("desktop")).await.unwrap();
        f.github
            .answer([Ok(PollAnswer::Granted(token("approved-token")))]);
        f.next_poll().await;
        let reached = if audit_wait {
            &f.audit.connected_started
        } else {
            &f.store.save_started
        };
        tokio::time::timeout(Duration::from_secs(5), reached.notified())
            .await
            .unwrap();
        let disconnect = connections.disconnect(caller("disconnecting-desktop"));
        tokio::pin!(disconnect);
        tokio::select! {
            biased;
            result = &mut disconnect => panic!("disconnect overtook pending receipts: {result:?}"),
            _ = std::future::ready(()) => {}
        }
        hold.notify_one();
        tokio::time::timeout(Duration::from_secs(5), disconnect)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(connections.status(), signed_out(None));
        assert!(connections.token().is_none());
        assert!(f.store.held.lock().unwrap().is_none());
        assert!(matches!(
            f.audit.events().last(),
            Some(GithubAuditEvent::Disconnected)
        ));
    }
}

#[tokio::test]
async fn a_closed_vault_during_approved_receipts_preserves_a_new_restoration_and_terminal_owner() {
    for audit_wait in [true, false] {
        let f = Fixture::new();
        let access = Arc::new(ChangingVault(std::sync::atomic::AtomicU64::new(1)));
        let connections = GithubConnections::load(
            f.github.clone(),
            access.clone(),
            f.store.clone(),
            f.audit.clone(),
            f.timer.clone(),
        )
        .await;
        let hold = Arc::new(Notify::new());
        if audit_wait {
            *f.audit.hold_connected.lock().unwrap() = Some(hold.clone());
        } else {
            *f.store.hold_save.lock().unwrap() = Some(hold.clone());
        }
        connections
            .connect(caller("original-desktop"))
            .await
            .unwrap();
        f.github
            .answer([Ok(PollAnswer::Granted(token("old-approved-token")))]);
        f.next_poll().await;
        let reached = if audit_wait {
            &f.audit.connected_started
        } else {
            &f.store.save_started
        };
        tokio::time::timeout(Duration::from_secs(5), reached.notified())
            .await
            .unwrap();
        // Queue a real old-generation command: its eventual rejection proves
        // the old completion released its generation gate after publication.
        let cancel = connections.cancel(caller("departed-desktop"));
        tokio::pin!(cancel);
        tokio::select! {
            biased;
            result = &mut cancel => panic!("cancel overtook pending receipts: {result:?}"),
            _ = std::future::ready(()) => {}
        }
        access.0.store(0, std::sync::atomic::Ordering::SeqCst);
        connections.clear_locked().unwrap();
        connections.clear_locked().unwrap();
        assert_eq!(connections.status(), signed_out(None));
        assert!(connections.token().is_none());
        let before = f.audit.events();
        assert!(!before
            .iter()
            .any(|event| matches!(event, GithubAuditEvent::ConnectEnded { .. })));
        access.0.store(2, std::sync::atomic::Ordering::SeqCst);
        *f.store.held.lock().unwrap() = Some(GithubSignIn {
            account: octo(),
            token: token("new-session-token"),
        });
        // When the old save has not started, model the scoped store's refusal.
        // Real read/save/delete generation fences are exercised by token_vault tests.
        *f.store.unwritable.lock().unwrap() = audit_wait;
        tokio::time::timeout(Duration::from_secs(5), connections.restore())
            .await
            .unwrap()
            .unwrap();
        hold.notify_one();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(5), cancel)
                .await
                .unwrap(),
            Err(GithubError::StoreUnavailable)
        ));
        assert_eq!(connections.token().unwrap().expose(), "new-session-token");
        assert_eq!(
            f.store
                .held
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .token
                .expose(),
            "new-session-token"
        );
        let rows = f.audit.records.lock().unwrap();
        let ends: Vec<_> = rows
            .iter()
            .filter(|row| matches!(row.event, GithubAuditEvent::ConnectEnded { .. }))
            .collect();
        assert_eq!(ends.len(), usize::from(audit_wait));
        if let Some(ended) = ends.first() {
            assert_eq!(ended.operation_id, rows[0].operation_id);
            assert_eq!(ended.caller, caller("original-desktop"));
            assert!(matches!(
                ended.event,
                GithubAuditEvent::ConnectEnded {
                    cause: EndCause::VaultLocked,
                    ..
                }
            ));
        }
        assert_eq!(
            rows.iter()
                .filter(|row| matches!(row.event, GithubAuditEvent::Connected { .. }))
                .count(),
            1
        );
    }
}

mod read_http;
mod reads;
