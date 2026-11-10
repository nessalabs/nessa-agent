//! A peer's retained JSON must preserve the immutable public choice.
use super::*;
use nessa_auth::domain::CredentialId;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn peer_records_keep_both_choices_and_refuse_missing_choice_or_relabelled_updates() {
    for choice in [false, true] {
        let fixture = Fixture::new().await;
        let dialing = Dialing::new(&fixture).await;
        let peer = NativeIdentity::generate(&mut OsEntropy).unwrap();
        let pin = peer.public_spki();
        let key = DeviceKey::new(pin[12..].try_into().unwrap());
        let intent = PublicIntent::new(
            InvitationId::new([1; 16]),
            AttemptId::new([2; 16]),
            ConsentIntentId::new([3; 16]),
            1,
            1000,
            ConsentClass::PeerRead,
        )
        .unwrap()
        .with_same_person(choice)
        .unwrap();
        let address: SocketAddr = "127.0.0.1:7443".parse().unwrap();
        dialing
            .records
            .enrolling(address, None)
            .save_pending(dialing.identity.key_material(), &pin, intent, None)
            .unwrap();
        let slot = dialing.records.slot(key);
        assert_eq!(slot.load_pending().unwrap().unwrap().intent(), intent);
        let (name, bytes) = dialing.files().into_iter().next().unwrap();
        let path = dialing.directory.join(name);
        let mut record: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(record["samePerson"], choice);
        record.as_object_mut().unwrap().remove("samePerson");
        std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(matches!(
            slot.load_pending(),
            Err(PrivateStateError::Corrupt)
        ));
        std::fs::write(&path, &bytes).unwrap();
        let credential = CredentialId::new("peer-credential").unwrap();
        let receiver = ResourceId::new("peer-receiver").unwrap();
        assert_eq!(
            slot.save_credential(
                &credential,
                &receiver,
                intent.with_same_person(!choice).unwrap()
            ),
            Err(PrivateStateError::Conflict)
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        slot.save_credential(&credential, &receiver, intent)
            .unwrap();
        assert_eq!(slot.load_credential().unwrap().unwrap().intent(), intent);
        fixture.gateway.shutdown().await;
    }
}
