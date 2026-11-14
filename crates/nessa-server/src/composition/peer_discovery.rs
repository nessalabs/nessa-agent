//! Discovery key agreement on the pinned session, and the LAN announcement
//! that carries one rotating token per paired peer. Neither path enrolls.
use nessa_auth::{
    adapters::local::{GrantorPeer, LocalCredentialStore},
    application::ports::Clock,
    domain::pairing::DeviceKey,
};
use nessa_client_core::pairing::NativeClientError;
use nessa_peer_gateways::{
    application::{
        announcement_bytes, discovery_from_json, discovery_to_json, exchange, instance_label,
        maintain_announcement, AnnouncementStep, Discovery, DiscoveryPipe, DiscoveryRefusal,
        ExchangeError, GrantorDiscovery, Phase, Role, Session, TokenPeer, DISCOVERY_WINDOW,
    },
    infrastructure::{platform_lan_announcer, LanRegistration, PeerCommands, PeerEntry, PeerPhase},
};
use nessa_protocol::pairing::socket::DeadlineStream;
use nessa_protocol::pairing::EnrollmentChannel;
use std::{
    io::{self, ErrorKind},
    net::SocketAddr,
    sync::Arc,
    time::Duration,
};
use tokio::{sync::watch, task::JoinHandle};

/// The granting side of discovery, stored on the peer's pairing record.
pub(super) struct RegistryDiscovery {
    pub store: Arc<LocalCredentialStore>,
    pub clock: Arc<dyn Clock>,
}

impl GrantorDiscovery for RegistryDiscovery {
    fn serve(&self, peer: DeviceKey, pipe: &mut dyn DiscoveryPipe) -> Result<(), DiscoveryRefusal> {
        let now_seconds = self.clock.unix_milliseconds() / 1000;
        let found = self
            .store
            .peer_discovery(&peer, now_seconds)
            .map_err(|_| DiscoveryRefusal::Unavailable)?;
        let Some(found) = found else {
            return Err(DiscoveryRefusal::NotPaired);
        };
        let discovery = match found.discovery_json.as_deref() {
            None => Discovery::None,
            Some(json) => discovery_from_json(json).ok_or(DiscoveryRefusal::Unavailable)?,
        };
        let mut session = Session::new(Role::Grantor, Phase::Active, discovery);
        let store = &self.store;
        let exchanged = exchange(
            &mut session,
            pipe,
            &mut random_bytes,
            &mut |commit| {
                if !commit.durable {
                    return Ok(());
                }
                let json = discovery_to_json(&commit.discovery, commit.audit.as_ref());
                store
                    .write_peer_discovery(
                        &found.invitation,
                        &peer,
                        self.clock.unix_milliseconds() / 1000,
                        json.as_deref(),
                    )
                    .map_err(|_| io::Error::other("discovery record"))
            },
            false,
        );
        exchanged.map_err(|_| DiscoveryRefusal::Unavailable)
    }
}

fn random_bytes() -> Option<[u8; 32]> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).ok()?;
    Some(bytes)
}

/// Announces while `stop` is false, and withdraws on the way out.
pub(super) struct LanTask {
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl LanTask {
    pub(super) fn start(
        peers: Arc<PeerCommands>,
        store: Arc<LocalCredentialStore>,
        clock: Arc<dyn Clock>,
        address: Option<SocketAddr>,
    ) -> Self {
        let (stop, stopped) = watch::channel(false);
        let task = tokio::spawn(async move {
            let announcer = platform_lan_announcer();
            let mut instance = [0u8; 8];
            let label = if getrandom::fill(&mut instance).is_ok() {
                Some(instance_label(&instance))
            } else {
                None
            };
            let mut registration: Option<Box<dyn LanRegistration>> = None;
            let mut published: Option<Vec<u8>> = None;
            let retries = discovery_cycles(&peers, stopped.clone());
            maintain_announcement(stopped, retries, |step| {
                if matches!(step, AnnouncementStep::Withdraw) {
                    if let Some(registration) = registration.take() { registration.withdraw(); }
                    return;
                }
                let payload = address
                    .and_then(|address| announce_payload(&peers, &store, clock.as_ref(), address));
                if payload.as_ref() != published.as_ref() {
                    if let Some(previous) = registration.take() {
                        previous.withdraw();
                    }
                    published = None;
                    if let (Some(payload), Some(address), Some(label)) =
                        (payload, address, label.as_deref())
                    {
                        match announcer.announce(address, &payload, label) {
                            Ok(live) => {
                                published = Some(payload);
                                registration = Some(live);
                            }
                            Err(error) => {
                                tracing::warn!(%error, "paired gateway not announced on the local network");
                            }
                        }
                    }
                }
            }).await;
        });
        Self { stop, task }
    }

    pub(super) fn signal_stop(&self) {
        self.stop.send_replace(true);
    }

    pub(super) async fn join(self) {
        self.signal_stop();
        if let Err(error) = self.task.await {
            tracing::error!(?error, "paired gateway announcement task fault");
        }
    }
}

async fn discovery_cycles(peers: &PeerCommands, mut stopped: watch::Receiver<bool>) {
    loop {
        if *stopped.borrow() {
            return;
        }
        drive_dialers(peers, &stopped).await;
        if *stopped.borrow() {
            return;
        }
        tokio::select! {
            _ = stopped.changed() => return,
            _ = crate::timer::clock::sleep(refresh_interval()) => {}
        }
    }
}

// Each network attempt is CONNECT (5s) plus TLS_DEADLINE (10s) and the
// single ENROLLMENT_DEADLINE (30s), not a new allowance per frame. Stop is
// observed before the next peer; local storage/worker drain is still joined.
async fn drive_dialers(peers: &PeerCommands, stopped: &watch::Receiver<bool>) {
    let Ok(entries) = peers.records().list() else {
        return;
    };
    for entry in entries {
        if *stopped.borrow() {
            return;
        }
        let PeerEntry::Readable(record) = entry else {
            continue;
        };
        if !record.needs_discovery_exchange() {
            continue;
        }
        let key = *record.key();
        let records = peers.records().clone();
        let _ = peers
            .open_peer_discovery(key, move |channel| dial_discovery(&records, key, channel))
            .await;
    }
}

fn dial_discovery(
    records: &Arc<nessa_peer_gateways::infrastructure::PeerRecords>,
    key: DeviceKey,
    channel: &mut EnrollmentChannel<DeadlineStream>,
) -> Result<(), NativeClientError> {
    let found = records
        .get(&key)
        .map_err(|_| NativeClientError::Io(ErrorKind::Other))?
        .ok_or(NativeClientError::NoPending)?;
    let PeerEntry::Readable(record) = found else {
        return Err(NativeClientError::NoPending);
    };
    if !record.needs_discovery_exchange() {
        return Err(NativeClientError::NoPending);
    }
    let mut session = Session::new(Role::Dialer, Phase::Active, record.discovery().clone());
    if matches!(session.discovery, Discovery::None) {
        let generation = random_bytes().ok_or(NativeClientError::Entropy)?;
        let private_key = random_bytes().ok_or(NativeClientError::Entropy)?;
        let commit = session
            .mint(generation, private_key)
            .ok_or(NativeClientError::Phase)?;
        records
            .write_discovery(&key, commit.discovery.clone(), commit.audit.as_ref())
            .map_err(|_| NativeClientError::Io(ErrorKind::Other))?;
        session.discovery = commit.discovery;
    }
    let mut pipe = FramePipe { channel };
    let exchanged = exchange(
        &mut session,
        &mut pipe,
        &mut random_bytes,
        &mut |commit| {
            if commit.durable {
                records
                    .write_discovery(&key, commit.discovery.clone(), commit.audit.as_ref())
                    .map_err(|_| io::Error::other("discovery"))?;
            }
            Ok(())
        },
        true,
    );
    match exchanged {
        Ok(()) => Ok(()),
        Err(ExchangeError::Refused) => {
            let _ = records.set_credential_current(&key, false);
            Err(NativeClientError::Refused)
        }
        Err(ExchangeError::Io(error)) => Err(NativeClientError::Io(error.kind())),
        Err(ExchangeError::BadMessage) => Err(NativeClientError::Phase),
        Err(ExchangeError::Entropy) => Err(NativeClientError::Entropy),
    }
}

struct FramePipe<'a> {
    channel: &'a mut EnrollmentChannel<DeadlineStream>,
}

impl DiscoveryPipe for FramePipe<'_> {
    fn send_frame(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.channel
            .send_envelope(bytes)
            .map_err(|_| io::Error::new(ErrorKind::InvalidData, "discovery frame"))
    }
    fn recv_frame(&mut self) -> io::Result<Vec<u8>> {
        self.channel
            .receive_envelope()
            .map_err(|_| io::Error::new(ErrorKind::InvalidData, "discovery frame"))
    }
}

fn refresh_interval() -> Duration {
    DISCOVERY_WINDOW / 4
}

fn announce_payload(
    peers: &PeerCommands,
    store: &LocalCredentialStore,
    clock: &dyn Clock,
    address: SocketAddr,
) -> Option<Vec<u8>> {
    let mut macs = Vec::new();
    if let Ok(entries) = peers.records().list() {
        for entry in entries {
            let PeerEntry::Readable(record) = entry else {
                continue;
            };
            if !matches!(record.phase(), PeerPhase::Active { .. }) || !record.credential_current() {
                continue;
            }
            if let Some(mac) = record.discovery().installed_mac() {
                macs.push(*mac);
            }
        }
    }
    let now_seconds = clock.unix_milliseconds() / 1000;
    if let Ok(grantors) = store.peer_discoveries(now_seconds) {
        for peer in grantors {
            push_grantor(&mut macs, &peer);
        }
    }
    let tokens: Vec<TokenPeer<'_>> = macs
        .iter()
        .map(|mac| TokenPeer {
            mac,
            phase: Phase::Active,
            credential_current: true,
        })
        .collect();
    announcement_bytes(&tokens, address, clock.unix_milliseconds())
}

fn push_grantor(macs: &mut Vec<[u8; 32]>, peer: &GrantorPeer) {
    if !peer.phase_active || !peer.credential_current {
        return;
    }
    let Some(json) = peer.discovery_json.as_deref() else {
        return;
    };
    let Some(mac) =
        discovery_from_json(json).and_then(|discovery| discovery.installed_mac().copied())
    else {
        return;
    };
    if !macs.contains(&mac) {
        macs.push(mac);
    }
}
