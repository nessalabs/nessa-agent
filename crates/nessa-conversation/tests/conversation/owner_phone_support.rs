//! A paired owner phone over the real signed Auth registry, receiver
//! authority and pinned native transport, for the owner-phone policy tests
//! here and the product-route tests in nessa-server.
use crate::device_pairing::ConversationReceivers;
use crate::{domain::ReceiverBinding, infrastructure::LocalReceiverAuthority};
use nessa_auth::{
    adapters::{
        cedar::CedarPolicyEvaluator,
        local::{BootstrapRequest, LocalCredentialStore},
        pairing::{verify_owner_phone_request, GatewayTrust, NativeIdentity, NativeTransport},
    },
    application::{
        dto::*,
        pairing::*,
        ports::{Clock, CredentialEvidence},
        session::{AuthenticateSession, AuthenticatedSession},
    },
    domain::{pairing::*, *},
};
use nessa_device_pairing::application::{PairingReceivers, ReceiverDispatch};
use std::{
    net::{TcpListener, TcpStream},
    sync::Arc,
    thread,
    time::Duration,
};
use zeroize::Zeroizing;

pub struct Time;
impl Clock for Time {
    fn unix_milliseconds(&self) -> u64 {
        110_001
    }
}
pub struct Fixture {
    pub _directory: tempfile::TempDir,
    pub purpose: OwnerPhonePurpose,
    pub session: AuthenticatedSession,
    pub bootstrap: AuthenticatedSession,
    pub receiver: ReceiverBinding,
    pub gateway: Resource,
    pub registry: Arc<LocalCredentialStore>,
    pub receivers: Arc<LocalReceiverAuthority>,
}
pub async fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    let parent = directory.path().canonicalize().unwrap();
    #[cfg(not(unix))]
    let parent = directory.path().to_path_buf();
    let registry_root = parent.join("registry");
    nessa_local_storage::create_directory(&registry_root).unwrap();
    let policy = Arc::new(CedarPolicyEvaluator::new().unwrap());
    let registry = Arc::new(
        LocalCredentialStore::open(&registry_root, "credentials.v1.json", policy.clone()).unwrap(),
    );
    let grants = [
        "credential.manage",
        "conversation.read",
        "conversation.write",
        "server.read",
    ]
    .into_iter()
    .map(|action| CredentialGrantDto {
        action: action.into(),
        resource: ResourceDto {
            organization_id: "org".into(),
            id: "gateway".into(),
        },
    })
    .collect();
    let issued = registry
        .bootstrap(BootstrapRequest {
            gateway_id: "gateway".into(),
            organization: OrganizationInputDto { id: "org".into() },
            principal: PrincipalInputDto {
                id: "owner".into(),
                kind: PrincipalKindDto::Human,
            },
            membership: MembershipInputDto {
                id: "member".into(),
                principal_id: "owner".into(),
                organization_id: "org".into(),
                role: MembershipRoleDto::Admin,
                state: MembershipStateDto::Active,
            },
            credential_id: "bootstrap".into(),
            issued_at: 100,
            expires_at: Some(100_000),
            grants,
        })
        .unwrap();
    let audience = AudienceId::new("gateway").unwrap();
    let bootstrap = AuthenticateSession {
        policy: &nessa_auth::adapters::cedar::CedarPolicyEvaluator::new().unwrap(),
        verifier: registry.as_ref(),
        access: registry.as_ref(),
        clock: &Time,
    }
    .execute(&issued.evidence, &audience)
    .await
    .unwrap();
    let gateway = Resource::new(
        OrganizationId::new("org").unwrap(),
        ResourceId::new("gateway").unwrap(),
    );
    let context = bootstrap.context();
    let source = OwnerPhoneSource::new(
        context.principal_id().clone(),
        context.membership_id().clone(),
        context.audience_id().clone(),
        context.credential_id().clone(),
        gateway.clone(),
    )
    .unwrap();
    let identity =
        NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([42; 32]))).unwrap();
    let server_identity =
        NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([10; 32]))).unwrap();
    let pin = server_identity.public_spki();
    let original = OwnerPhoneRequest::new(
        OwnerPhoneRequestId::new([7; 16]).unwrap(),
        source,
        pin,
        "127.0.0.1:19744".parse().unwrap(),
        identity.device_key(),
    )
    .unwrap();
    let proof = verify_owner_phone_request(
        original.clone(),
        &identity.sign_owner_phone_request(&original).unwrap(),
    )
    .unwrap();
    let intent = ConsentIntent::new(
        ConsentIntentId::new([8; 16]),
        1,
        audience.clone(),
        context.principal_id().clone(),
        context.membership_id().clone(),
        gateway.clone(),
        ConsentClass::OwnerPhoneReadWrite,
    )
    .unwrap();
    let initial = PairingRecord::new_owner_phone(
        InvitationId::new([9; 16]),
        intent.clone(),
        Time.unix_milliseconds(),
        PairingPolicy::initial(),
    )
    .unwrap();
    let admit = || async {
        AuthorizePairing {
            access: registry.as_ref(),
            policy: policy.as_ref(),
            clock: &Time,
        }
        .execute(&bootstrap, &intent, &gateway)
        .await
        .unwrap()
    };
    let approved = registry
        .authorize_owner_phone(&initial, &proof, &admit().await, &Time)
        .await
        .unwrap();
    let credential = CredentialId::new("phone-device").unwrap();
    registry
        .stage_pairing(
            approved.id(),
            credential.clone(),
            AttemptId::new([77; 16]),
            &admit().await,
            &Time,
        )
        .unwrap();
    let receiver_root = parent.join("receiver-access");
    nessa_local_storage::create_directory(&receiver_root).unwrap();
    let receivers = Arc::new(
        LocalReceiverAuthority::open(
            &receiver_root.join("receiver-access.sqlite3"),
            "policy",
            Arc::new(Time),
        )
        .unwrap(),
    );
    let real = ConversationReceivers::new(receivers.clone());
    let stage = registry.clone().acquire_stage(approved.id()).unwrap();
    let dispatch = ReceiverDispatch::for_pair(&stage).unwrap();
    let paired = real.pair(&dispatch).unwrap();
    registry
        .remember_receiver(approved.id(), &paired, &Time)
        .unwrap();
    registry
        .publish_pairing(approved.id(), &admit().await, &Time)
        .unwrap();
    drop(dispatch);
    drop(stage);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        NativeTransport::accept(socket, &server_identity).unwrap()
    });
    let socket = TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    socket
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let _client = NativeTransport::connect(socket, &identity, GatewayTrust::Pinned(pin)).unwrap();
    let channel = worker.join().unwrap();
    let verifier = registry.device_verifier(channel.device_proof());
    let session = AuthenticateSession {
        policy: &nessa_auth::adapters::cedar::CedarPolicyEvaluator::new().unwrap(),
        verifier: &verifier,
        access: registry.as_ref(),
        clock: &Time,
    }
    .execute(
        &CredentialEvidence::new(credential.as_str().as_bytes().to_vec()).unwrap(),
        &audience,
    )
    .await
    .unwrap();
    let purpose = registry
        .owner_phone_purpose(&session, &Time)
        .await
        .unwrap()
        .unwrap();
    let receiver = receivers.binding(&credential).unwrap().unwrap();
    Fixture {
        _directory: directory,
        purpose,
        session,
        bootstrap,
        receiver,
        gateway,
        registry,
        receivers,
    }
}
pub struct ActualHolding(pub Arc<LocalReceiverAuthority>);
impl crate::application::OwnerPhoneReceiverAuthority for ActualHolding {
    fn holding<'a>(
        &'a self,
        purpose: &'a OwnerPhonePurpose,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        Option<ReceiverBinding>,
                        nessa_protocol::conversation::read_scope::ReadRefusal,
                    >,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            self.0
                .holding(&crate::domain::PairedReceiver {
                    receiver_id: purpose.receiver().as_str().into(),
                    credential_id: purpose.credential().clone(),
                    organization_id: purpose.organization().clone(),
                    owner_id: purpose.owner().clone(),
                    paired_epoch: purpose.paired_epoch(),
                })
                .map_err(|_| nessa_protocol::conversation::read_scope::ReadRefusal::Unverifiable)
        })
    }
}
