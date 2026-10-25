//! Private startup seeds: real preparation, no issuance or network effects.
use super::*;
use nessa_auth::adapters::cedar::CedarPolicyEvaluator;
use serde_json::{json, Value};
struct Time;
impl Clock for Time {
    fn unix_milliseconds(&self) -> u64 {
        2_000
    }
}
struct Fixture {
    root: tempfile::TempDir,
    credentials: Arc<LocalCredentialStore>,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let credentials = Arc::new(
            LocalCredentialStore::open(
                root.path(),
                "auth.json",
                Arc::new(CedarPolicyEvaluator::new().unwrap()),
            )
            .unwrap(),
        );
        Self { root, credentials }
    }
    async fn prepare(
        &self,
        settings: &RuntimeConfig,
    ) -> Result<Option<Arc<WebhookRuntime>>, RunError> {
        prepare(
            settings,
            self.root.path(),
            self.credentials.clone(),
            self.credentials.clone(),
            Arc::new(Time),
        )
        .await
    }
    fn files(&self) -> Vec<String> {
        let mut files = std::fs::read_dir(self.root.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        files.sort();
        files
    }
}
fn seed(principal: &str) -> Value {
    json!({"principalId":principal,"url":"https://recipient.test/synthetic-private-path","macKey":STANDARD.encode([7;32])})
}
fn config(seeds: Vec<Value>) -> RuntimeConfig {
    serde_json::from_value(json!({"webhooks":seeds})).unwrap()
}
#[tokio::test]
async fn absent_seeds_and_database_have_no_webhook_file_effects() {
    let f = Fixture::new();
    let before = f.files();
    assert!(f
        .prepare(&RuntimeConfig::default())
        .await
        .unwrap()
        .is_none());
    assert_eq!(f.files(), before);
}
#[tokio::test]
async fn invalid_private_seeds_refuse_before_opening_storage() {
    let f = Fixture::new();
    let before = f.files();
    let mut invalids = vec![
        vec![seed("hook"), seed("hook")],
        (0..=MAX_REGISTRATIONS)
            .map(|i| seed(&format!("hook-{i}")))
            .collect(),
    ];
    for (field, value) in [
        ("principalId", json!("")),
        ("url", json!("http://recipient.test/synthetic")),
        ("macKey", json!("not-base64")),
        ("macKey", json!(STANDARD.encode([7; 31]))),
        ("macKey", json!("!".repeat(44))),
    ] {
        let mut invalid = seed("hook");
        invalid[field] = value;
        invalids.push(vec![invalid]);
    }
    for seeds in invalids {
        assert!(f.prepare(&config(seeds)).await.is_err());
        assert_eq!(
            f.files(),
            before,
            "a refused seed must not open private webhook storage"
        );
    }
}
#[tokio::test]
async fn bounded_seed_configuration_opens_without_issuing_and_reopens_after_removal() {
    let f = Fixture::new();
    let seeds = (0..MAX_REGISTRATIONS)
        .map(|i| seed(&format!("hook-{i}")))
        .collect();
    let runtime = f.prepare(&config(seeds)).await.unwrap().unwrap();
    assert!(runtime.store().intents().await.unwrap().is_empty());
    assert!(
        !f.root.path().join("auth.json").exists(),
        "opening seeds never initializes or issues credentials"
    );
    runtime.shutdown().await.unwrap();
    drop(runtime);
    let restored = f
        .prepare(&RuntimeConfig::default())
        .await
        .unwrap()
        .expect("existing private outbox remains owned without current seeds");
    assert!(restored.store().intents().await.unwrap().is_empty());
    restored.shutdown().await.unwrap();
}
#[test]
fn configuration_debug_redacts_url_and_mac_and_unknown_fields_refuse() {
    let settings = config(vec![seed("hook")]);
    let debug = format!("{settings:?}");
    assert!(!debug.contains("synthetic-private-path"));
    assert!(!debug.contains(&STANDARD.encode([7; 32])));
    let mut invalid = seed("hook");
    invalid["allowPrivateAddresses"] = json!(true);
    assert!(serde_json::from_value::<RuntimeConfig>(json!({"webhooks":[invalid]})).is_err());
}
#[tokio::test]
async fn a_non_file_outbox_refuses_instead_of_becoming_absent() {
    let f = Fixture::new();
    std::fs::create_dir(f.root.path().join("webhooks.sqlite")).unwrap();
    let before = f.files();
    assert!(f.prepare(&RuntimeConfig::default()).await.is_err());
    assert_eq!(f.files(), before);
}
#[cfg(unix)]
#[tokio::test]
async fn an_outbox_symlink_refuses_without_following_it() {
    let f = Fixture::new();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("untouched");
    std::fs::write(&target, b"synthetic-marker").unwrap();
    std::os::unix::fs::symlink(&target, f.root.path().join("webhooks.sqlite")).unwrap();
    assert!(f.prepare(&RuntimeConfig::default()).await.is_err());
    assert_eq!(std::fs::read(target).unwrap(), b"synthetic-marker");
}
