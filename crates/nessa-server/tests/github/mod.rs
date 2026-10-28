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
    GithubReadFailure, GithubTokenStore, PollAnswer, StoreUnavailable,
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
    unreadable: Mutex<bool>,
    unwritable: Mutex<bool>,
    /// A save writes the file, then fails (its directory not synced).
    fails_after_writing: Mutex<bool>,
}

#[async_trait]
impl GithubTokenStore for MemoryStore {
    async fn load(&self) -> Result<Option<GithubSignIn>, StoreUnavailable> {
        if *self.unreadable.lock().unwrap() {
            return Err(StoreUnavailable);
        }
        Ok(self.held.lock().unwrap().clone())
    }
    async fn save(&self, sign_in: &GithubSignIn) -> Result<(), StoreUnavailable> {
        if *self.unwritable.lock().unwrap() {
            return Err(StoreUnavailable);
        }
        *self.held.lock().unwrap() = Some(sign_in.clone());
        if *self.fails_after_writing.lock().unwrap() {
            return Err(StoreUnavailable);
        }
        Ok(())
    }
    async fn delete(&self) -> Result<(), StoreUnavailable> {
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

async fn signed_in(fixture: &Fixture) -> GithubConnections {
    *fixture.store.held.lock().unwrap() = Some(GithubSignIn {
        account: octo(),
        token: token("ghu_kept"),
    });
    fixture.connections().await
}

#[tokio::test]
async fn repository_reads_use_only_the_current_connected_sign_in() {
    use crate::github::application::GithubReadError;

    let fixture = Fixture::new();
    let repository = GithubRepository::new("nessalabs", "nessa-agent").unwrap();
    let connections = fixture.connections().await;
    assert_eq!(
        connections
            .read_repository(&repository, GithubReadKind::Repository)
            .await,
        Err(GithubReadError::NotConnected)
    );
    assert!(fixture.github.reads.lock().unwrap().is_empty());

    let connections = signed_in(&fixture).await;
    *fixture.github.read_answer.lock().unwrap() =
        Ok(serde_json::json!({"full_name":"nessalabs/nessa-agent"}));
    assert_eq!(
        connections
            .read_repository(&repository, GithubReadKind::Repository)
            .await,
        Ok(serde_json::json!({"full_name":"nessalabs/nessa-agent"}))
    );
    assert_eq!(
        *fixture.github.reads.lock().unwrap(),
        [(
            token("ghu_kept"),
            repository.clone(),
            GithubReadKind::Repository
        )]
    );

    connections.disconnect(caller("panel")).await.unwrap();
    assert_eq!(
        connections
            .read_repository(&repository, GithubReadKind::Repository)
            .await,
        Err(GithubReadError::NotConnected)
    );
    assert_eq!(fixture.github.reads.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn disconnect_waits_for_a_read_and_later_reads_see_signed_out() {
    use crate::github::application::GithubReadError;

    let fixture = Fixture::new();
    let connections = Arc::new(signed_in(&fixture).await);
    let repository = GithubRepository::new("nessalabs", "nessa-agent").unwrap();
    let release = Arc::new(Notify::new());
    *fixture.github.hold_reads.lock().unwrap() = Some(release.clone());
    *fixture.github.read_answer.lock().unwrap() = Ok(serde_json::json!({"id":1}));

    let reader = {
        let connections = connections.clone();
        let repository = repository.clone();
        tokio::spawn(async move {
            connections
                .read_repository(&repository, GithubReadKind::Repository)
                .await
        })
    };
    fixture.github.read_started.notified().await;
    let disconnect = {
        let connections = connections.clone();
        tokio::spawn(async move { connections.disconnect(caller("panel")).await })
    };
    tokio::task::yield_now().await;
    assert!(!disconnect.is_finished());
    assert!(fixture.store.held.lock().unwrap().is_some());

    release.notify_one();
    assert_eq!(reader.await.unwrap(), Ok(serde_json::json!({"id":1})));
    assert_eq!(disconnect.await.unwrap(), Ok(signed_out(None)));
    assert_eq!(
        connections
            .read_repository(&repository, GithubReadKind::Repository)
            .await,
        Err(GithubReadError::NotConnected)
    );
    assert_eq!(fixture.github.reads.lock().unwrap().len(), 1);
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
