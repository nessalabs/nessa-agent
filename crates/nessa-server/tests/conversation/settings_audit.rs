//! A settings change is kept as written: who asked, for what, from what,
//! and what came of it; a step recorded again is acknowledged, not doubled.
use super::*;
use crate::conversation::application::ConversationCaller;
use crate::conversation::domain::{ChannelId, WorkspacePlacement};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::domain::ConversationId;

struct At(u64);
impl Clock for At {
    fn unix_milliseconds(&self) -> u64 {
        self.0
    }
}

fn caller() -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "desktop".into(),
        action_id: "request-1".into(),
    }
}

fn read(directory: &std::path::Path, id: &str) -> Value {
    serde_json::from_slice(&std::fs::read(directory.join(format!("{id}.json"))).unwrap()).unwrap()
}

#[tokio::test]
async fn a_channel_move_keeps_both_places_its_main_chats_and_who_asked() {
    let directory = tempfile::tempdir().unwrap();
    let audit =
        DurableSettingsAudit::new(directory.path().join("settings"), Arc::new(At(7))).unwrap();
    let old = ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap();
    let fresh = ConversationId::new("00000000-0000-4000-8000-000000000002").unwrap();
    let change = SettingsChange::ChannelPlacement {
        channel: ChannelId::from_random([1; 10]),
        before: ChannelExecution::default(),
        after: ChannelExecution {
            coordinator: WorkspacePlacement::Boat,
            threads: WorkspacePlacement::Local,
        },
        coordinator: old.clone(),
    };
    for outcome in [
        SettingsOutcome::Requested,
        SettingsOutcome::Applied {
            changed: true,
            coordinator: Some(fresh.clone()),
        },
    ] {
        let record = SettingsAuditRecord {
            operation_id: "op-1".into(),
            caller: caller(),
            change: change.clone(),
            outcome,
        };
        audit.record(record.clone()).await.unwrap();
        // The same step again is acknowledged, not written twice.
        audit.record(record).await.unwrap();
    }
    let asked = read(
        &directory.path().join("settings"),
        "settings-op-1-requested",
    );
    assert_eq!(asked["kind"], "channel_placement");
    assert_eq!(asked["transition"]["before"]["coordinator"], "local");
    assert_eq!(asked["transition"]["requested"]["coordinator"], "boat");
    assert_eq!(
        asked["target"]["coordinatorConversationId"],
        old.to_string()
    );
    assert_eq!(asked["initiator"]["principalId"], "person");
    assert_eq!(asked["correlationId"], "request-1");
    let done = read(&directory.path().join("settings"), "settings-op-1-outcome");
    assert_eq!(done["outcome"]["outcome"], "applied");
    assert_eq!(
        done["outcome"]["coordinatorConversationId"],
        fresh.to_string()
    );
    assert_eq!(done["observedAtMs"], 7);
}

#[tokio::test]
async fn an_app_revoke_names_its_server_and_a_contradicting_rewrite_is_refused() {
    let directory = tempfile::tempdir().unwrap();
    let directory = directory.path().join("settings");
    let audit = DurableSettingsAudit::new(directory.clone(), Arc::new(At(1))).unwrap();
    let record = |server: &str| SettingsAuditRecord {
        operation_id: "op-2".into(),
        caller: caller(),
        change: SettingsChange::AppUntrust {
            server: server.into(),
        },
        outcome: SettingsOutcome::Failed,
    };
    audit.record(record("charts")).await.unwrap();
    let kept = read(&directory, "settings-op-2-outcome");
    assert_eq!(kept["kind"], "app_untrust");
    assert_eq!(kept["target"]["server"], "charts");
    assert_eq!(kept["outcome"]["outcome"], "failed");
    assert!(audit.record(record("huddle")).await.is_err());
}
