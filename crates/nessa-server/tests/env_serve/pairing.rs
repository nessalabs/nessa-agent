use super::answer_to_failure;
use crate::env_serve::application::Answer;
use nessa_auth::application::pairing::PrivateStateError;
use nessa_client_core::pairing::NativeClientError;
use nessa_protocol::clock::Clock;
use std::sync::Arc;
use std::{io::ErrorKind, path::Path};

struct Fixed;
impl Clock for Fixed {
    fn elapsed_ms(&self) -> u64 {
        0
    }
}

fn unusable(answer: &Answer) -> bool {
    matches!(answer, Answer::Unusable(reason) if reason.contains("remove /data/pairing"))
}

/// Only a pairing record this machine cannot use, or a gateway answering
/// out of turn, stops waiting; everything else is asked again, a failed
/// save among them (its pending record is kept: storage failures other
/// than corrupt or conflicting land in the same arm).
#[test]
fn a_failed_status_is_unusable_only_when_no_later_ask_can_change_it() {
    let directory = Path::new("/data/pairing");
    assert_eq!(
        answer_to_failure(NativeClientError::NoPending, directory),
        Answer::NotPaired
    );
    for error in [
        NativeClientError::Storage(PrivateStateError::Corrupt),
        NativeClientError::Storage(PrivateStateError::Conflict),
        NativeClientError::Phase,
        NativeClientError::OtherEnrollee,
    ] {
        assert!(unusable(&answer_to_failure(error, directory)), "{error:?}");
    }
    for error in [
        NativeClientError::Io(ErrorKind::ConnectionReset),
        NativeClientError::Busy,
        NativeClientError::Refused,
        NativeClientError::Entropy,
        NativeClientError::Storage(PrivateStateError::Unavailable),
        NativeClientError::Storage(PrivateStateError::Uncertain),
    ] {
        assert!(
            matches!(answer_to_failure(error, directory), Answer::Unanswered(_)),
            "{error:?}"
        );
    }
}

/// The credential replaced the pending record, and the direct address does
/// not answer. Recovery still asks the relay for that credential's gateway.
#[tokio::test]
async fn a_saved_credential_is_reached_through_the_relay_when_direct_is_down() {
    use super::FilePairingEnvironment;
    use nessa_auth::adapters::pairing::{FilePairingState, NativeIdentity};
    use nessa_auth::application::pairing::{ClientPendingStore, PrivateKeyMaterial};
    use nessa_auth::domain::pairing::{
        AttemptId, ConsentClass, ConsentIntentId, InvitationId, PublicIntent,
    };
    use nessa_auth::domain::{CredentialId, ResourceId};
    use nessa_protocol::relay::{read_frame, write_frame, RelayAddress, RelayFrame};
    use std::io::Read;
    use std::time::Duration;
    use zeroize::Zeroizing;

    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap().join("root");
    nessa_local_storage::create_directory(&path).unwrap();
    nessa_local_storage::create_directory_beneath(&path, Path::new("pairing")).unwrap();
    let intent = PublicIntent::new(
        InvitationId::new([1; InvitationId::LENGTH]),
        AttemptId::new([2; AttemptId::LENGTH]),
        ConsentIntentId::new([3; ConsentIntentId::LENGTH]),
        1,
        9,
        ConsentClass::PeerEnvironment,
    )
    .unwrap();
    let key = PrivateKeyMaterial::new(Zeroizing::new([4; 32]));
    let pin = NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([5; 32])))
        .unwrap()
        .public_spki();
    let gateway = nessa_auth::adapters::pairing::relay_key(&pin).unwrap();
    {
        let store = FilePairingState::open(&path, Path::new("pairing")).unwrap();
        store.save_pending(&key, &pin, intent, None).unwrap();
        store
            .save_credential(
                &CredentialId::new("env-1").unwrap(),
                &ResourceId::new("receiver-1").unwrap(),
                intent,
            )
            .unwrap();
        assert!(store.load_pending().unwrap().is_none());
    }
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let direct = closed.local_addr().unwrap();
    drop(closed);
    let relay_socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let relay_port = relay_socket.local_addr().unwrap().port();
    let relay = RelayAddress::try_from(format!("127.0.0.1:{relay_port}")).unwrap();
    let (sent, heard) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = relay_socket.accept().unwrap();
        let frame = read_frame(&mut stream).unwrap();
        sent.send(frame).unwrap();
        write_frame(&mut stream, &RelayFrame::Joined).unwrap();
        let mut rest = [0; 8];
        let _ = stream.read(&mut rest);
    });
    let pairing = FilePairingEnvironment::open(&path, &direct.to_string(), Arc::new(Fixed))
        .unwrap()
        .through(Some(relay));
    let stream = tokio::time::timeout(Duration::from_secs(3), pairing.connect_or_relay())
        .await
        .expect("the relay is asked once the direct address refuses")
        .expect("the saved credential's pin is the relay route");
    drop(stream);
    pairing.shutdown().await;
    assert_eq!(
        heard.recv_timeout(Duration::from_secs(1)).unwrap(),
        RelayFrame::Connect { key: gateway },
        "the relay is asked for the credential's gateway, not the pending record"
    );
}
