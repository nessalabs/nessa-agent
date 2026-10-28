use crate::{
    env::Stage,
    github_relay::{
        application::{catch_up, RelaySource, RelayStore},
        domain::{Binding, Event, Failure, Page},
        infrastructure::{relay_configuration, SqliteRelayStore},
    },
};
use std::sync::{
    atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
    Arc,
};
fn binding(user: &str) -> Binding {
    Binding::new(
        user.into(),
        "42".into(),
        "nessalabs/nessa-agent".into(),
        uuid::Uuid::new_v4(),
    )
    .unwrap()
}
fn page(user: &str, sequence: i64) -> Page {
    Page {
        user_id: user.into(),
        next: sequence,
        has_more: false,
        ticket: "ticket".into(),
        events: vec![Event {
            sequence,
            delivery_id: uuid::Uuid::new_v4().to_string(),
            event: "issues".into(),
            payload: "{}".into(),
            received_at: "2026-10-25T00:00:00Z".into(),
            repository_id: "42".into(),
        }],
    }
}
struct TestSelection(AtomicU64);
impl TestSelection {
    fn new() -> Self {
        Self(AtomicU64::new(1))
    }
}
impl crate::github_relay::application::LiveSelectionAdmission for TestSelection {
    fn commit_in_generation(
        &self,
        generation: u64,
        commit: &mut dyn FnMut() -> Result<(), Failure>,
    ) -> Result<(), Failure> {
        if self.0.load(Ordering::SeqCst) != generation {
            return Err(Failure::VaultLocked);
        }
        commit()
    }
}
struct TestKey;
impl crate::github_relay::application::InboxKeySource for TestKey {
    fn load(&self) -> Result<Arc<nessa_local_database::DatabaseKey>, Failure> {
        Ok(Arc::new(nessa_local_database::DatabaseKey::new([11; 32])))
    }
}
fn store(directory: &tempfile::TempDir) -> SqliteRelayStore {
    SqliteRelayStore::new(
        directory.path().join("private/inbox.sqlite"),
        Arc::new(|| 1000000),
        Arc::new(TestKey),
        Arc::new(TestSelection::new()),
    )
}

#[tokio::test]
async fn failed_page_rolls_back_events_and_cursor_and_reopen_replays() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory);
    let binding = binding("11");
    store.bind(&binding, false).await.unwrap();
    let mut bad = page("11", 1);
    let mut duplicate = bad.events[0].clone();
    duplicate.sequence = 2;
    bad.events.push(duplicate);
    bad.next = 2;
    assert_eq!(store.commit(&binding, 0, &bad).await, Err(Failure::Storage));
    assert_eq!(store.cursor(&binding).await.unwrap(), 0);
    assert!(store.events(&binding).await.unwrap().is_empty());
    let good = page("11", 1);
    store.commit(&binding, 0, &good).await.unwrap();
    let reopened = SqliteRelayStore::new(
        directory.path().join("private/inbox.sqlite"),
        Arc::new(|| 1000000),
        Arc::new(TestKey),
        Arc::new(TestSelection::new()),
    );
    assert_eq!(reopened.cursor(&binding).await.unwrap(), 1);
    assert_eq!(reopened.events(&binding).await.unwrap(), good.events);
}
#[tokio::test]
async fn concurrent_consumers_commit_only_the_page_matching_the_current_cursor() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory);
    let binding = binding("11");
    store.bind(&binding, false).await.unwrap();
    let page = page("11", 1);
    let (a, b) = tokio::join!(
        store.commit(&binding, 0, &page),
        store.commit(&binding, 0, &page)
    );
    assert!(
        (a.is_ok() && b == Err(Failure::CursorChanged))
            || (b.is_ok() && a == Err(Failure::CursorChanged))
    );
    assert_eq!(store.events(&binding).await.unwrap().len(), 1);
}
#[tokio::test]
async fn account_and_repository_correlation_precede_local_effects() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory);
    let first = binding("11");
    let second = binding("22");
    store.bind(&first, false).await.unwrap();
    store.bind(&second, false).await.unwrap();
    assert_eq!(
        store.commit(&first, 0, &page("22", 1)).await,
        Err(Failure::IdentityChanged)
    );
    let mut wrong_repo = page("11", 1);
    wrong_repo.events[0].repository_id = "43".into();
    assert_eq!(
        store.commit(&first, 0, &wrong_repo).await,
        Err(Failure::InvalidResponse)
    );
    store.commit(&first, 0, &page("11", 1)).await.unwrap();
    assert!(store.events(&second).await.unwrap().is_empty());
    assert_eq!(store.cursor(&second).await.unwrap(), 0);
}
#[tokio::test]
async fn gap_is_durable_and_requires_explicit_owner_acknowledgement() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory);
    let binding = binding("11");
    store.bind(&binding, false).await.unwrap();
    store.gap(&binding, 10).await.unwrap();
    store.gap(&binding, 10).await.unwrap();
    assert_eq!(store.cursor(&binding).await, Err(Failure::Gap(10)));
    store.bind(&binding, false).await.unwrap();
    assert_eq!(store.cursor(&binding).await, Err(Failure::Gap(10)));
    store.bind(&binding, true).await.unwrap();
    assert_eq!(store.cursor(&binding).await.unwrap(), 10);
    let schema = nessa_local_database::Schema::new(include_str!(
        "../../src/github_relay/infrastructure/schema.sql"
    ))
    .unwrap();
    let db = nessa_local_database::open_encrypted(
        &directory.path().join("private/inbox.sqlite"),
        &schema,
        &nessa_local_database::DatabaseKey::new([11; 32]),
    )
    .unwrap();
    let evidence:(String,String,i64,i64)=db.query_row("SELECT cause,initiator,before_cursor,after_cursor FROM audit WHERE cause='gap_accepted'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
    assert_eq!(
        evidence,
        ("gap_accepted".into(), "local-owner:github:11".into(), 0, 10)
    );
}
#[tokio::test]
async fn retention_records_delivery_identity_and_system_cause_with_cursor_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let now = Arc::new(AtomicI64::new(1000000));
    let clock = now.clone();
    let store = SqliteRelayStore::new(
        directory.path().join("private/inbox.sqlite"),
        Arc::new(move || clock.load(Ordering::SeqCst)),
        Arc::new(TestKey),
        Arc::new(TestSelection::new()),
    );
    let binding = binding("11");
    store.bind(&binding, false).await.unwrap();
    let first = page("11", 1);
    store.commit(&binding, 0, &first).await.unwrap();
    now.store(2000000, Ordering::SeqCst);
    store.commit(&binding, 1, &page("11", 2)).await.unwrap();
    assert_eq!(store.events(&binding).await.unwrap().len(), 1);
    assert_eq!(store.cursor(&binding).await.unwrap(), 2);
    let schema = nessa_local_database::Schema::new(include_str!(
        "../../src/github_relay/infrastructure/schema.sql"
    ))
    .unwrap();
    let db = nessa_local_database::open_encrypted(
        &directory.path().join("private/inbox.sqlite"),
        &schema,
        &nessa_local_database::DatabaseKey::new([11; 32]),
    )
    .unwrap();
    let identity:String=db.query_row("SELECT delivery_id FROM audit WHERE cause='retention_expired' AND initiator='system:local-relay'",[],|r|r.get(0)).unwrap();
    assert_eq!(identity, first.events[0].delivery_id);
}
struct OfflineThenPage(AtomicBool);
#[async_trait::async_trait]
impl RelaySource for OfflineThenPage {
    async fn page(&self, _: &str, binding: &Binding, after: i64) -> Result<Page, Failure> {
        if !self.0.swap(true, Ordering::SeqCst) {
            return Err(Failure::Offline);
        }
        Ok(page(binding.user_id(), after + 1))
    }
}
#[tokio::test]
async fn offline_transport_preserves_cursor_then_recovers_using_injected_source() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory);
    let binding = binding("11");
    store.bind(&binding, false).await.unwrap();
    let source = OfflineThenPage(AtomicBool::new(false));
    assert!(matches!(
        catch_up(&source, &store, &binding, &Credentials).await,
        Err(Failure::Offline)
    ));
    assert_eq!(store.cursor(&binding).await.unwrap(), 0);
    catch_up(&source, &store, &binding, &Credentials)
        .await
        .unwrap();
    assert_eq!(store.cursor(&binding).await.unwrap(), 1);
}
#[test]
fn stage_table_selects_dev_and_refuses_unconfigured_release_stages() {
    assert_eq!(
        relay_configuration(Stage::Dev).unwrap().origin,
        "https://nessa-github-webhooks.saurav-panthee.workers.dev"
    );
    for stage in [Stage::Alpha, Stage::Ci, Stage::Prod] {
        assert!(matches!(
            relay_configuration(stage),
            Err(Failure::UnconfiguredStage)
        ));
    }
}

#[tokio::test]
async fn disconnected_generation_cannot_commit_or_delete_a_replacement_connection() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory);
    let first = binding("11");
    store.bind(&first, false).await.unwrap();
    let pending = page("11", 1);
    store.unbind(&first).await.unwrap();
    let replacement = binding("11");
    store.bind(&replacement, false).await.unwrap();
    assert_eq!(
        store.commit(&first, 0, &pending).await,
        Err(Failure::CursorChanged)
    );
    assert_eq!(store.unbind(&first).await, Err(Failure::CursorChanged));
    assert_eq!(store.gap(&first, 9).await, Err(Failure::CursorChanged));
    assert_eq!(store.cursor(&replacement).await.unwrap(), 0);
    assert!(store.events(&replacement).await.unwrap().is_empty());
    store.commit(&replacement, 0, &pending).await.unwrap();
    assert_eq!(store.cursor(&replacement).await.unwrap(), 1);
}
#[tokio::test]
async fn a_real_sqlite_write_failure_rolls_back_the_whole_page() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory);
    let binding = binding("11");
    store.bind(&binding, false).await.unwrap();
    let schema = nessa_local_database::Schema::new(include_str!(
        "../../src/github_relay/infrastructure/schema.sql"
    ))
    .unwrap();
    let db = nessa_local_database::open_encrypted(
        &directory.path().join("private/inbox.sqlite"),
        &schema,
        &nessa_local_database::DatabaseKey::new([11; 32]),
    )
    .unwrap();
    db.execute_batch("CREATE TRIGGER disk_failure BEFORE INSERT ON events WHEN NEW.sequence=2 BEGIN SELECT RAISE(ABORT,'injected write failure'); END;").unwrap();
    let mut both = page("11", 1);
    both.events.push(page("11", 2).events.remove(0));
    both.next = 2;
    assert_eq!(
        store.commit(&binding, 0, &both).await,
        Err(Failure::Storage)
    );
    assert_eq!(store.cursor(&binding).await.unwrap(), 0);
    assert!(store.events(&binding).await.unwrap().is_empty());
    db.execute_batch("DROP TRIGGER disk_failure;").unwrap();
    drop(db);
    store.commit(&binding, 0, &both).await.unwrap();
    assert_eq!(store.events(&binding).await.unwrap().len(), 2);
}
#[test]
fn malformed_provider_identities_cannot_become_bindings() {
    for id in ["0", "-1", "01", "../11", "9007199254740992"] {
        assert!(Binding::new(
            id.into(),
            "42".into(),
            "nessalabs/nessa-agent".into(),
            uuid::Uuid::new_v4()
        )
        .is_err());
    }
}

#[tokio::test]
async fn redelivery_after_cloud_expiry_preserves_local_identity_and_advances_cursor() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory);
    let binding = binding("11");
    store.bind(&binding, false).await.unwrap();
    let first = page("11", 1);
    store.commit(&binding, 0, &first).await.unwrap();
    let mut redelivery = page("11", 2);
    redelivery.events[0].delivery_id = first.events[0].delivery_id.clone();
    store.commit(&binding, 1, &redelivery).await.unwrap();
    assert_eq!(store.cursor(&binding).await.unwrap(), 2);
    assert_eq!(store.events(&binding).await.unwrap(), first.events);
    let mut changed = page("11", 3);
    changed.events[0].delivery_id = first.events[0].delivery_id.clone();
    changed.events[0].payload = "{\"changed\":true}".into();
    assert_eq!(
        store.commit(&binding, 2, &changed).await,
        Err(Failure::InvalidResponse)
    );
    assert_eq!(store.cursor(&binding).await.unwrap(), 2);
}

struct LiveSource {
    latest: AtomicI64,
    arrival_on_watch: AtomicBool,
}
struct SilentNotifications;
#[async_trait::async_trait]
impl crate::github_relay::application::RelayNotifications for SilentNotifications {
    async fn changed(&mut self) -> Result<(), Failure> {
        std::future::pending().await
    }
}
#[async_trait::async_trait]
impl RelaySource for LiveSource {
    async fn page(&self, _: &str, binding: &Binding, after: i64) -> Result<Page, Failure> {
        let latest = self.latest.load(Ordering::SeqCst);
        if after >= latest {
            return Ok(Page {
                user_id: binding.user_id().into(),
                events: vec![],
                next: after,
                ticket: "ticket".into(),
                has_more: false,
            });
        }
        let mut page = page(binding.user_id(), after + 1);
        page.has_more = after + 1 < latest;
        Ok(page)
    }
}
#[async_trait::async_trait]
impl crate::github_relay::application::RelayLiveSource for LiveSource {
    async fn watch(
        &self,
        _: &str,
    ) -> Result<Box<dyn crate::github_relay::application::RelayNotifications>, Failure> {
        if self.arrival_on_watch.swap(false, Ordering::SeqCst) {
            self.latest.store(2, Ordering::SeqCst);
        }
        Ok(Box::new(SilentNotifications))
    }
}
struct ManualWait {
    waiting: AtomicBool,
    renew: tokio::sync::Notify,
}
#[async_trait::async_trait]
impl crate::github_relay::application::RelayWait for ManualWait {
    async fn wait(&self, duration: std::time::Duration) {
        if duration.as_secs() == crate::github_relay::domain::contract::RENEW_AFTER_SECONDS {
            self.waiting.store(true, Ordering::SeqCst);
            self.renew.notified().await;
        } else {
            std::future::pending::<()>().await;
        }
    }
}
struct Credentials;
#[async_trait::async_trait]
impl crate::github_relay::application::GitHubCredentials for Credentials {
    async fn token(&self) -> Result<crate::github::domain::AccessToken, Failure> {
        Ok(crate::github::domain::AccessToken::new("test-token".into()).unwrap())
    }
    async fn identify(&self, _: &str) -> Result<Binding, Failure> {
        Err(Failure::InvalidResponse)
    }
}
struct Reporter;
impl crate::github_relay::application::RelayReporter for Reporter {
    fn disconnected(&self, _: &Binding, failure: &Failure) {
        panic!("unexpected disconnection: {failure}")
    }
    fn storage_failed(&self, failure: &Failure) {
        panic!("unexpected storage failure: {failure}")
    }
    fn task_failed(&self) {
        panic!("unexpected task failure")
    }
}
async fn reaches_cursor(store: &SqliteRelayStore, binding: &Binding, expected: i64) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if store.cursor(binding).await.unwrap() == expected {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("consumer reached durable cursor");
}
#[tokio::test]
async fn subscribing_before_the_second_read_closes_the_live_arrival_race() {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(store(&directory));
    let binding = binding("11");
    store.bind(&binding, false).await.unwrap();
    let source = Arc::new(LiveSource {
        latest: AtomicI64::new(1),
        arrival_on_watch: AtomicBool::new(true),
    });
    let wait = Arc::new(ManualWait {
        waiting: AtomicBool::new(false),
        renew: tokio::sync::Notify::new(),
    });
    let task = {
        let (source, store, wait, binding) =
            (source.clone(), store.clone(), wait.clone(), binding.clone());
        tokio::spawn(async move {
            crate::github_relay::application::follow(
                source.as_ref(),
                store.as_ref(),
                &Credentials,
                &binding,
                wait.as_ref(),
                &Reporter,
            )
            .await
        })
    };
    reaches_cursor(store.as_ref(), &binding, 2).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
}
#[tokio::test]
async fn a_lost_notification_is_recovered_by_the_injected_renewal_clock() {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(store(&directory));
    let binding = binding("11");
    store.bind(&binding, false).await.unwrap();
    let source = Arc::new(LiveSource {
        latest: AtomicI64::new(1),
        arrival_on_watch: AtomicBool::new(false),
    });
    let wait = Arc::new(ManualWait {
        waiting: AtomicBool::new(false),
        renew: tokio::sync::Notify::new(),
    });
    let task = {
        let (source, store, wait, binding) =
            (source.clone(), store.clone(), wait.clone(), binding.clone());
        tokio::spawn(async move {
            crate::github_relay::application::follow(
                source.as_ref(),
                store.as_ref(),
                &Credentials,
                &binding,
                wait.as_ref(),
                &Reporter,
            )
            .await
        })
    };
    reaches_cursor(store.as_ref(), &binding, 1).await;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !wait.waiting.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    source.latest.store(2, Ordering::SeqCst);
    wait.renew.notify_one();
    reaches_cursor(store.as_ref(), &binding, 2).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(store.events(&binding).await.unwrap().len(), 2);
}

#[tokio::test]
async fn a_full_inbox_expiry_keeps_each_deletion_identity_in_the_bounded_audit() {
    let directory = tempfile::tempdir().unwrap();
    let store = store(&directory);
    let binding = binding("11");
    store.bind(&binding, false).await.unwrap();
    let schema = nessa_local_database::Schema::new(include_str!(
        "../../src/github_relay/infrastructure/schema.sql"
    ))
    .unwrap();
    let db = nessa_local_database::open_encrypted(
        &directory.path().join("private/inbox.sqlite"),
        &schema,
        &nessa_local_database::DatabaseKey::new([11; 32]),
    )
    .unwrap();
    db.execute("WITH RECURSIVE numbers(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM numbers WHERE n < ?) INSERT INTO events(user_id,repository_id,sequence,delivery_id,event,payload,received_at,stored_at) SELECT '11','42',n,printf('%08x-0000-4000-8000-000000000000',n),'issues','{}','2026-01-01T00:00:00Z',0 FROM numbers",[crate::github_relay::domain::contract::STORED_EVENTS]).unwrap();
    db.execute(
        "UPDATE bindings SET cursor=?",
        [crate::github_relay::domain::contract::STORED_EVENTS],
    )
    .unwrap();
    drop(db);
    let count = crate::github_relay::domain::contract::STORED_EVENTS;
    store
        .commit(&binding, count, &page("11", count + 1))
        .await
        .unwrap();
    let db = nessa_local_database::open_encrypted(
        &directory.path().join("private/inbox.sqlite"),
        &schema,
        &nessa_local_database::DatabaseKey::new([11; 32]),
    )
    .unwrap();
    let evidence: i64 = db
        .query_row(
            "SELECT COUNT(DISTINCT delivery_id) FROM audit WHERE cause='retention_expired'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(evidence, count);
    let receipt: i64 = db
        .query_row(
            "SELECT after_cursor FROM binding_audit WHERE cause='page_committed'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(receipt, count + 1);
}

struct ConnectedAccount {
    user: AtomicI64,
    unavailable: AtomicBool,
    count: usize,
}
#[async_trait::async_trait]
impl crate::github_relay::application::GitHubCredentials for ConnectedAccount {
    async fn token(&self) -> Result<crate::github::domain::AccessToken, Failure> {
        if self.user.load(Ordering::SeqCst) == 0 {
            Err(Failure::CredentialsUnavailable)
        } else {
            Ok(crate::github::domain::AccessToken::new("held-by-sign-in-owner".into()).unwrap())
        }
    }
    async fn identify(&self, _: &str) -> Result<Binding, Failure> {
        Ok(binding(&self.user.load(Ordering::SeqCst).to_string()))
    }
}
#[async_trait::async_trait]
impl crate::github_relay::application::ConnectedGithub for ConnectedAccount {
    fn user_id(&self) -> Option<String> {
        let user = self.user.load(Ordering::SeqCst);
        (user > 0).then(|| user.to_string())
    }
    async fn repositories(&self) -> Result<Vec<Binding>, Failure> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(Failure::Offline);
        }
        (0..self.count)
            .map(|i| {
                Binding::new(
                    self.user.load(Ordering::SeqCst).to_string(),
                    (42 + i).to_string(),
                    format!("owner/repo{i}"),
                    uuid::Uuid::new_v4(),
                )
            })
            .collect()
    }
}
#[tokio::test]
async fn opt_in_survives_restart_without_resetting_the_cursor_and_disable_fences_old_pages() {
    use crate::github_relay::application::LiveEvents;
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(store(&directory));
    let account = Arc::new(ConnectedAccount {
        user: AtomicI64::new(11),
        unavailable: AtomicBool::new(false),
        count: 1,
    });
    let live = LiveEvents::new(account.clone(), store.clone());
    assert_eq!(live.status().await.unwrap(), (false, 0, 0));
    live.set(1, true, "principal:owner/credential:one", false)
        .await
        .unwrap();
    let first = store.bindings().await.unwrap().remove(0);
    let first_page = page("11", 1);
    store.commit(&first, 0, &first_page).await.unwrap();
    live.set(1, true, "principal:owner/credential:one", false)
        .await
        .unwrap();
    assert_eq!(store.cursor(&first).await.unwrap(), 1);
    let restored = LiveEvents::new(account.clone(), Arc::new(self::store(&directory)));
    assert_eq!(restored.status().await.unwrap(), (true, 1, 0));
    store.gap(&first, 99).await.unwrap();
    assert_eq!(live.status().await.unwrap(), (true, 1, 1));
    live.set(1, true, "principal:owner/credential:one", true)
        .await
        .unwrap();
    let resumed = store.bindings().await.unwrap().remove(0);
    assert_ne!(resumed.generation(), first.generation());
    assert_eq!(store.cursor(&resumed).await.unwrap(), 99);
    assert_eq!(live.status().await.unwrap(), (true, 1, 0));
    live.set(1, false, "principal:owner/credential:one", false)
        .await
        .unwrap();
    assert_eq!(
        store.commit(&first, 1, &page("11", 2)).await,
        Err(Failure::CursorChanged)
    );
    assert!(store.events(&first).await.unwrap().is_empty());
    assert_eq!(restored.status().await.unwrap(), (false, 0, 0));
    live.set(1, true, "principal:owner/credential:one", false)
        .await
        .unwrap();
    let second = store.bindings().await.unwrap().remove(0);
    assert_ne!(first.generation(), second.generation());
    assert_eq!(
        store.commit(&first, 0, &first_page).await,
        Err(Failure::CursorChanged)
    );
    let db = nessa_local_database::open_encrypted(
        &directory.path().join("private/inbox.sqlite"),
        &nessa_local_database::Schema::new(include_str!(
            "../../src/github_relay/infrastructure/schema.sql"
        ))
        .unwrap(),
        &nessa_local_database::DatabaseKey::new([11; 32]),
    )
    .unwrap();
    let (cause, initiator): (String, String) = db
        .query_row(
            "SELECT cause,initiator FROM audit WHERE delivery_id=?",
            [&first_page.events[0].delivery_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        (cause.as_str(), initiator.as_str()),
        ("live_events_disabled", "principal:owner/credential:one")
    );
}
#[tokio::test]
async fn signed_out_offline_and_excess_repositories_cannot_enable_or_replace_the_selection() {
    use crate::github_relay::application::LiveEvents;
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(store(&directory));
    let account = Arc::new(ConnectedAccount {
        user: AtomicI64::new(0),
        unavailable: AtomicBool::new(false),
        count: 1,
    });
    let live = LiveEvents::new(account.clone(), store.clone());
    assert_eq!(
        live.set(1, true, "owner", false).await,
        Err(Failure::CredentialsUnavailable)
    );
    account.user.store(11, Ordering::SeqCst);
    live.set(1, true, "owner", false).await.unwrap();
    let first = store.bindings().await.unwrap();
    account.unavailable.store(true, Ordering::SeqCst);
    assert_eq!(
        live.set(1, true, "owner", false).await,
        Err(Failure::Offline)
    );
    assert_eq!(store.bindings().await.unwrap(), first);
    let excessive = LiveEvents::new(
        Arc::new(ConnectedAccount {
            user: AtomicI64::new(11),
            unavailable: AtomicBool::new(false),
            count: 17,
        }),
        store.clone(),
    );
    assert_eq!(
        excessive.set(1, true, "owner", false).await,
        Err(Failure::Capacity)
    );
    assert_eq!(store.bindings().await.unwrap(), first);
    account.user.store(22, Ordering::SeqCst);
    assert_eq!(live.status().await.unwrap(), (false, 0, 0));
    let guarded =
        crate::github_relay::infrastructure::ConnectedRelayStore::new(store.clone(), account);
    assert!(guarded.bindings().await.unwrap().is_empty());
    assert_eq!(
        guarded.commit(&first[0], 0, &page("11", 1)).await,
        Err(Failure::IdentityChanged)
    );
}
#[tokio::test]
async fn cleanup_audit_failure_refuses_disable_and_preserves_selection() {
    use crate::github_relay::application::LiveEvents;
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(store(&directory));
    let live = LiveEvents::new(
        Arc::new(ConnectedAccount {
            user: AtomicI64::new(11),
            unavailable: AtomicBool::new(false),
            count: 1,
        }),
        store.clone(),
    );
    live.set(1, true, "owner", false).await.unwrap();
    let binding = store.bindings().await.unwrap().remove(0);
    store.commit(&binding, 0, &page("11", 1)).await.unwrap();
    let db = nessa_local_database::open_encrypted(
        &directory.path().join("private/inbox.sqlite"),
        &nessa_local_database::Schema::new(include_str!(
            "../../src/github_relay/infrastructure/schema.sql"
        ))
        .unwrap(),
        &nessa_local_database::DatabaseKey::new([11; 32]),
    )
    .unwrap();
    db.execute_batch("CREATE TRIGGER refuse_cleanup BEFORE INSERT ON binding_audit WHEN NEW.cause='live_events_disabled' BEGIN SELECT RAISE(ABORT,'audit unavailable'); END;").unwrap();
    assert_eq!(
        live.set(1, false, "owner", false).await,
        Err(Failure::Storage)
    );
    assert_eq!(live.status().await.unwrap(), (true, 1, 0));
    assert_eq!(store.events(&binding).await.unwrap().len(), 1);
}

/// Count requests through the actual follower and SQLite store over a virtual
/// quiet day. The budget excludes event-triggered downloads and DO invocations.
#[tokio::test]
async fn a_quiet_day_uses_360_http_requests_per_repository() {
    use crate::github_relay::application::{
        follow, RelayLiveSource, RelayNotifications, RelayWait,
    };
    use std::sync::atomic::AtomicUsize;
    struct Quiet {
        pages: AtomicUsize,
        watches: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl RelaySource for Quiet {
        async fn page(&self, _: &str, b: &Binding, after: i64) -> Result<Page, Failure> {
            self.pages.fetch_add(1, Ordering::SeqCst);
            Ok(Page {
                user_id: b.user_id().into(),
                next: after,
                has_more: false,
                ticket: "ticket".into(),
                events: vec![],
            })
        }
    }
    #[async_trait::async_trait]
    impl RelayLiveSource for Quiet {
        async fn watch(&self, _: &str) -> Result<Box<dyn RelayNotifications>, Failure> {
            self.watches.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(SilentNotifications))
        }
    }
    struct Day {
        elapsed: AtomicI64,
        finished: tokio::sync::Notify,
    }
    #[async_trait::async_trait]
    impl RelayWait for Day {
        async fn wait(&self, duration: std::time::Duration) {
            let after = self
                .elapsed
                .fetch_add(duration.as_secs() as i64, Ordering::SeqCst)
                + duration.as_secs() as i64;
            if after >= 86400 {
                self.finished.notify_one();
                std::future::pending::<()>().await;
            }
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(store(&directory));
    let binding = binding("11");
    store.bind(&binding, false).await.unwrap();
    let source = Arc::new(Quiet {
        pages: AtomicUsize::new(0),
        watches: AtomicUsize::new(0),
    });
    let day = Arc::new(Day {
        elapsed: AtomicI64::new(0),
        finished: tokio::sync::Notify::new(),
    });
    let task = {
        let (source, store, day) = (source.clone(), store.clone(), day.clone());
        tokio::spawn(async move {
            follow(
                source.as_ref(),
                store.as_ref(),
                &Credentials,
                &binding,
                day.as_ref(),
                &Reporter,
            )
            .await
        })
    };
    tokio::time::timeout(std::time::Duration::from_secs(15), day.finished.notified())
        .await
        .unwrap();
    task.abort();
    let _ = task.await;
    assert_eq!(source.pages.load(Ordering::SeqCst), 240);
    assert_eq!(source.watches.load(Ordering::SeqCst), 120);
}

#[tokio::test]
async fn follower_refreshes_credentials_per_request_and_acquires_none_while_idle() {
    use crate::github_relay::application::{
        follow, GitHubCredentials, RelayLiveSource, RelayNotifications,
    };
    use std::sync::{atomic::AtomicUsize, Mutex};
    struct Rotating(AtomicUsize);
    #[async_trait::async_trait]
    impl GitHubCredentials for Rotating {
        async fn token(&self) -> Result<crate::github::domain::AccessToken, Failure> {
            Ok(crate::github::domain::AccessToken::new(format!(
                "fixture-token-{}",
                self.0.fetch_add(1, Ordering::SeqCst)
            ))
            .unwrap())
        }
        async fn identify(&self, _: &str) -> Result<Binding, Failure> {
            Err(Failure::InvalidResponse)
        }
    }
    struct Wake(Arc<tokio::sync::Notify>);
    #[async_trait::async_trait]
    impl RelayNotifications for Wake {
        async fn changed(&mut self) -> Result<(), Failure> {
            self.0.notified().await;
            Ok(())
        }
    }
    struct Source {
        tokens: Mutex<Vec<String>>,
        wake: Arc<tokio::sync::Notify>,
    }
    #[async_trait::async_trait]
    impl RelaySource for Source {
        async fn page(&self, token: &str, b: &Binding, after: i64) -> Result<Page, Failure> {
            let mut tokens = self.tokens.lock().unwrap();
            tokens.push(token.into());
            let mut page = page(b.user_id(), after + 1);
            page.has_more = tokens.len() == 1;
            Ok(page)
        }
    }
    #[async_trait::async_trait]
    impl RelayLiveSource for Source {
        async fn watch(&self, _: &str) -> Result<Box<dyn RelayNotifications>, Failure> {
            Ok(Box::new(Wake(self.wake.clone())))
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(store(&directory));
    let binding = binding("11");
    store.bind(&binding, false).await.unwrap();
    let credentials = Arc::new(Rotating(AtomicUsize::new(0)));
    let source = Arc::new(Source {
        tokens: Mutex::new(vec![]),
        wake: Arc::new(tokio::sync::Notify::new()),
    });
    let wait = Arc::new(ManualWait {
        waiting: AtomicBool::new(false),
        renew: tokio::sync::Notify::new(),
    });
    let task = {
        let (source, store, credentials, binding, wait) = (
            source.clone(),
            store.clone(),
            credentials.clone(),
            binding.clone(),
            wait.clone(),
        );
        tokio::spawn(async move {
            follow(
                source.as_ref(),
                store.as_ref(),
                credentials.as_ref(),
                &binding,
                wait.as_ref(),
                &Reporter,
            )
            .await
        })
    };
    reaches_cursor(store.as_ref(), &binding, 3).await;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !wait.waiting.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(credentials.0.load(Ordering::SeqCst), 3);
    source.wake.notify_one();
    reaches_cursor(store.as_ref(), &binding, 4).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(
        *source.tokens.lock().unwrap(),
        [
            "fixture-token-0",
            "fixture-token-1",
            "fixture-token-2",
            "fixture-token-3"
        ]
    );
    assert_eq!(credentials.0.load(Ordering::SeqCst), 4);
}

struct PausedSelectionKey {
    pause: AtomicBool,
    entered: tokio::sync::Notify,
    release: (std::sync::Mutex<bool>, std::sync::Condvar),
}
impl crate::github_relay::application::InboxKeySource for PausedSelectionKey {
    fn load(&self) -> Result<Arc<nessa_local_database::DatabaseKey>, Failure> {
        if self.pause.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            let (released, wake) = &self.release;
            let mut ready = released.lock().unwrap();
            while !*ready {
                ready = wake.wait(ready).unwrap();
            }
        }
        Ok(Arc::new(nessa_local_database::DatabaseKey::new([11; 32])))
    }
}

#[tokio::test]
async fn an_in_flight_old_unlock_cannot_remove_a_new_live_selection() {
    use crate::github_relay::application::LiveEvents;
    let directory = tempfile::tempdir().unwrap();
    let key = Arc::new(PausedSelectionKey {
        pause: AtomicBool::new(false),
        entered: tokio::sync::Notify::new(),
        release: (std::sync::Mutex::new(false), std::sync::Condvar::new()),
    });
    let selection = Arc::new(TestSelection::new());
    let store = Arc::new(SqliteRelayStore::new(
        directory.path().join("private/inbox.sqlite"),
        Arc::new(|| 1000000),
        key.clone(),
        selection.clone(),
    ));
    let account = Arc::new(ConnectedAccount {
        user: AtomicI64::new(11),
        unavailable: AtomicBool::new(false),
        count: 1,
    });
    let live = Arc::new(LiveEvents::new(account, store.clone()));
    live.set(1, true, "owner", false).await.unwrap();
    let old = store.bindings().await.unwrap().remove(0);
    let entered = key.entered.notified();
    key.pause.store(true, Ordering::SeqCst);
    let old_disable = {
        let live = live.clone();
        tokio::spawn(async move { live.set(1, false, "owner", false).await })
    };
    tokio::time::timeout(std::time::Duration::from_secs(15), entered)
        .await
        .unwrap();
    let new = Binding::new(
        old.user_id().into(),
        old.repository_id().into(),
        old.repository_name().into(),
        uuid::Uuid::new_v4(),
    )
    .unwrap();
    selection.0.store(2, Ordering::SeqCst);
    store.bind(&new, false).await.unwrap();
    let (released, wake) = &key.release;
    *released.lock().unwrap() = true;
    wake.notify_one();
    assert_eq!(old_disable.await.unwrap(), Err(Failure::VaultLocked));
    assert_eq!(store.bindings().await.unwrap(), vec![new]);
    let db = nessa_local_database::open_encrypted(
        &directory.path().join("private/inbox.sqlite"),
        &nessa_local_database::Schema::new(include_str!(
            "../../src/github_relay/infrastructure/schema.sql"
        ))
        .unwrap(),
        &nessa_local_database::DatabaseKey::new([11; 32]),
    )
    .unwrap();
    let stale_deletions: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM binding_audit WHERE cause='live_events_disabled'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stale_deletions, 0);
}
