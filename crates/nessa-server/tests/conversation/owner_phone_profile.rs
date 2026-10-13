//! The composed enforcing profile changes the actual receiver journal epoch.
use super::*;
use crate::conversation::{
    application::{AdmitPassiveRead, PassiveRead, ReceiverAuthority},
    infrastructure::{CedarConversationAccessPolicy, LocalConversationStore},
};
use nessa_auth::application::authorization::AuthorizeAction;
use nessa_protocol::conversation::read_scope::ReadRefusal;

#[tokio::test]
async fn canonical_access_sources_change_receiver_profile_and_fence_saved_read_epoch() {
    let f = fixture().await;
    let auth = CedarPolicyEvaluator::profile_digest();
    let sources = CedarConversationAccessPolicy::profile_sources();
    let profile = CedarOwnerPhonePolicyEvaluator::receiver_profile_digest(&auth, &sources);
    let path = f
        ._directory
        .path()
        .canonicalize()
        .unwrap()
        .join("receiver-access/receiver-access.sqlite3");
    let current = LocalReceiverAuthority::open_existing(&path, &profile, Arc::new(Time)).unwrap();
    let before = current
        .resolve(f.session.context().credential_id())
        .await
        .unwrap()
        .unwrap();
    assert!(before.active);
    let repeated = LocalReceiverAuthority::open_existing(&path, &profile, Arc::new(Time)).unwrap();
    assert_eq!(
        repeated.resolve(&before.credential_id).await.unwrap(),
        Some(before.clone())
    );
    for changed in 0..sources.len() {
        let mut owned = sources
            .iter()
            .map(|source| (*source).to_owned())
            .collect::<Vec<_>>();
        owned[changed].push_str("\n// changed enforcing source identity\n");
        let altered = owned.iter().map(String::as_str).collect::<Vec<_>>();
        assert_ne!(
            profile,
            CedarOwnerPhonePolicyEvaluator::receiver_profile_digest(&auth, &altered)
        );
    }
    let mut owned = sources
        .iter()
        .map(|source| (*source).to_owned())
        .collect::<Vec<_>>();
    owned[3].push_str("\n// changed enforcing reader bundle identity\n");
    let altered = owned.iter().map(String::as_str).collect::<Vec<_>>();
    let next_profile = CedarOwnerPhonePolicyEvaluator::receiver_profile_digest(&auth, &altered);
    let next = LocalReceiverAuthority::open_existing(&path, &next_profile, Arc::new(Time)).unwrap();
    let after = next.resolve(&before.credential_id).await.unwrap().unwrap();
    assert_eq!(after.receiver_id, before.receiver_id);
    assert_eq!(after.credential_id, before.credential_id);
    assert_eq!(after.access_epoch, before.access_epoch + 1);
    assert!(after.active);
    let raw = nessa_local_database::rusqlite::Connection::open(&path).unwrap();
    let evidence = raw.query_row(
        "SELECT before_epoch,after_epoch,cause,initiator_kind,before_credential,after_credential FROM receiver_transitions WHERE receiver_id=?1 ORDER BY sequence DESC LIMIT 1",
        [&after.receiver_id], |row| Ok((row.get::<_,i64>(0)?, row.get::<_,i64>(1)?, row.get::<_,String>(2)?, row.get::<_,String>(3)?, row.get::<_,String>(4)?, row.get::<_,String>(5)?)),
    ).unwrap();
    assert_eq!(
        evidence,
        (
            i64::try_from(before.access_epoch).unwrap(),
            i64::try_from(after.access_epoch).unwrap(),
            "policy_changed".into(),
            "system".into(),
            before.credential_id.as_str().into(),
            after.credential_id.as_str().into()
        )
    );

    let metadata_root = f
        ._directory
        .path()
        .canonicalize()
        .unwrap()
        .join("profile-metadata");
    nessa_local_storage::create_directory(&metadata_root).unwrap();
    let metadata = LocalConversationStore::open(&metadata_root.join("metadata.sqlite3")).unwrap();
    let holding = ActualHolding(Arc::new(next));
    let policy = CedarPolicyEvaluator::new().unwrap();
    let admission = AdmitPassiveRead {
        authorization: AuthorizeAction {
            access: f.registry.as_ref(),
            clock: &Time,
            policy: &policy,
        },
        gateway: &f.gateway,
        receivers: holding.0.as_ref(),
        conversations: &metadata,
        share_grants: &metadata,
        grants: &crate::product::passive_read::PUBLISHED_PASSIVE_READ_GRANTS,
        access_policy: crate::conversation_test_support::access_policy(),
        owner_phone_purposes: f.registry.as_ref(),
        owner_phone_receivers: Some(&holding),
        owner_phone_policy: Arc::new(CedarOwnerPhonePolicyEvaluator::new().unwrap()),
    };
    assert_eq!(
        admission
            .catalogue(
                &f.session,
                &before.receiver_id,
                before.access_epoch,
                PassiveRead::CatalogueHead
            )
            .await,
        Err(ReadRefusal::StaleEpoch)
    );
    let fresh = admission
        .catalogue(
            &f.session,
            &after.receiver_id,
            after.access_epoch,
            PassiveRead::CatalogueHead,
        )
        .await
        .unwrap();
    assert_eq!(fresh.access_epoch, after.access_epoch);
}
