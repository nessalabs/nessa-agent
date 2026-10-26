//! Owner pairing methods on the authenticated socket: parse, ask the gateway's
//! enrollment runtime, and project the record it returns.
//!
//! The socket has already asked Cedar for `credential.manage`. The runtime asks
//! Auth again for the exact consent intent (`AuthorizePairing`) and makes every
//! enrollment decision; nothing here decides one. Design rows O1–O4, O7 and W1
//! in `docs/design/auth/device-pairing.md` ("Owner routes and mounting").
use super::{
    socket::{failure, success},
    state::ProductRouteState,
};
use crate::device_pairing::{
    application::OwnerError,
    infrastructure::{PairingRuntimeError, RegistrationError},
};
use crate::tunnel::application::{TunnelMode, TunnelToken};
use nessa_auth::{
    application::{
        dto::{CredentialGrantDto, ResourceDto},
        pairing::{OwnerDecision, PairingStoreError},
        ports::AccessError,
        session::AuthenticatedSession,
    },
    domain::pairing::{
        ConsentClass, DeviceKey, InvitationId, PairingError, PairingInitiator, PairingPhase,
        PairingRecord, TerminalCause,
    },
};
use nessa_protocol::product::generated::PairingTunnelMode;
use nessa_protocol::product::generated::{
    PairingActivationStop, PairingApproveParams, PairingApproveResult, PairingCreateParams,
    PairingCreateResult, PairingEnrollee, PairingErrorCode, PairingInitiator as WireInitiator,
    PairingInitiatorKind, PairingInvitationParams, PairingOwnerPhase, PairingOwnerStatus,
    PairingPendingResult, PairingReceiver, PairingTerminal, PairingTerminalCause,
};
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame};
use nessa_protocol::tunnel::TunnelAddress;
use serde_json::json;

pub(super) async fn dispatch(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    let Some(pairing) = state.pairing.as_ref() else {
        return failure(&frame.id, PairingErrorCode::PairingNotConfigured.as_str());
    };
    let id = frame.id;
    let result = match frame.method.as_str() {
        "pairing.create" => {
            let Ok(params) = serde_json::from_value::<PairingCreateParams>(frame.params) else {
                return failure(&id, "invalid_request");
            };
            let class = match params.enrollee {
                None | Some(PairingEnrollee::Device) => ConsentClass::DeviceRead,
                Some(PairingEnrollee::OwnerPhone) => ConsentClass::OwnerPhoneReadWrite,
                Some(PairingEnrollee::Gateway) => ConsentClass::PeerRead,
                Some(PairingEnrollee::Environment) => ConsentClass::PeerEnvironment,
            };
            match pairing.create(session, class).await {
                Ok(created) => {
                    let display = created.code().display_bytes();
                    let Ok(code) = std::str::from_utf8(display.as_ref()) else {
                        return failure(&id, PairingErrorCode::PairingUnavailable.as_str());
                    };
                    return success(
                        &id,
                        &PairingCreateResult {
                            code: code.to_owned(),
                            status: owner_status(created.record()),
                        },
                    );
                }
                Err(error) => Err(error),
            }
        }
        "pairing.pending" => {
            if frame.params != json!({}) {
                return failure(&id, "invalid_request");
            }
            match pairing.pending(session).await {
                Ok(records) => {
                    return success(
                        &id,
                        &PairingPendingResult {
                            items: records.iter().map(owner_status).collect(),
                        },
                    )
                }
                Err(error) => Err(error),
            }
        }
        "pairing.approve" => {
            let Ok(params) = serde_json::from_value::<PairingApproveParams>(frame.params) else {
                return failure(&id, "invalid_request");
            };
            match pairing
                .approve(
                    session,
                    InvitationId::new(params.invitation_id),
                    DeviceKey::new(params.device_key),
                )
                .await
            {
                Ok(approval) => {
                    return success(
                        &id,
                        &PairingApproveResult {
                            status: owner_status(&approval.record),
                            activation_stopped: approval.stopped.map(|stopped| {
                                if stopped.retryable() {
                                    PairingActivationStop::Retryable
                                } else {
                                    PairingActivationStop::Permanent
                                }
                            }),
                        },
                    )
                }
                Err(error) => Err(error),
            }
        }
        method @ ("pairing.status" | "pairing.deny" | "pairing.cancel") => {
            let Ok(params) = serde_json::from_value::<PairingInvitationParams>(frame.params) else {
                return failure(&id, "invalid_request");
            };
            let invitation = InvitationId::new(params.invitation_id);
            match method {
                "pairing.status" => pairing.status(session, invitation).await,
                "pairing.deny" => {
                    pairing
                        .decide(session, invitation, OwnerDecision::Deny)
                        .await
                }
                _ => {
                    pairing
                        .decide(session, invitation, OwnerDecision::Cancel)
                        .await
                }
            }
        }
        _ => return failure(&id, "unknown_method"),
    };
    match result {
        Ok(record) => success(&id, &owner_status(&record)),
        Err(error) => failure(&id, refusal(error)),
    }
}

/// The owner's view of one record. Every field is read from the record; the
/// cleanup flag is the record's own `cleanup_pending`, never stored here.
pub(super) fn owner_status(record: &PairingRecord) -> PairingOwnerStatus {
    let intent = record.intent();
    let grant = intent.grant();
    PairingOwnerStatus {
        invitation_id: *record.id().bytes(),
        consent_id: *intent.id().bytes(),
        generation: intent.generation(),
        class: intent.class().as_str().to_owned(),
        grant: CredentialGrantDto {
            action: grant.action().as_str().to_owned(),
            resource: ResourceDto {
                organization_id: grant.resource().organization_id().as_str().to_owned(),
                id: grant.resource().id().as_str().to_owned(),
            },
        },
        created_at_ms: record.created_at_ms(),
        expires_at_ms: record.expires_at_ms(),
        phase: match record.phase() {
            PairingPhase::Available => PairingOwnerPhase::Available,
            PairingPhase::Claimed => PairingOwnerPhase::Claimed,
            PairingPhase::Approved => PairingOwnerPhase::Approved,
            PairingPhase::Staging => PairingOwnerPhase::Staging,
            PairingPhase::Active => PairingOwnerPhase::Active,
            PairingPhase::Terminal => PairingOwnerPhase::Terminal,
        },
        claimed_device_key: record.claim_binding().map(|(_, key)| *key.bytes()),
        credential_id: record.credential().map(|id| id.as_str().to_owned()),
        receiver: record
            .receiver_binding()
            .map(|(receiver, access_epoch)| PairingReceiver {
                receiver_id: receiver.as_str().to_owned(),
                access_epoch,
            }),
        terminal: record.terminal().map(|(cause, initiator)| PairingTerminal {
            cause: match cause {
                TerminalCause::CredentialRevoked => PairingTerminalCause::CredentialRevoked,
                TerminalCause::Denied => PairingTerminalCause::Denied,
                TerminalCause::Cancelled => PairingTerminalCause::Cancelled,
                TerminalCause::Expired => PairingTerminalCause::Expired,
                TerminalCause::Restarted => PairingTerminalCause::Restarted,
            },
            initiator: match initiator {
                PairingInitiator::LocalOperator => {
                    initiator_kind(PairingInitiatorKind::LocalOperator)
                }
                PairingInitiator::System => initiator_kind(PairingInitiatorKind::System),
                PairingInitiator::Principal(principal) => WireInitiator {
                    principal_id: Some(principal.as_str().to_owned()),
                    ..initiator_kind(PairingInitiatorKind::Principal)
                },
                PairingInitiator::Device(key) => WireInitiator {
                    device_key: Some(*key.bytes()),
                    ..initiator_kind(PairingInitiatorKind::Device)
                },
            },
        }),
        cleanup_pending: record.cleanup_pending(),
    }
}

/// `pairing.setLinking`: answered whether or not native pairing runs now,
/// because turning it on is how it comes to run. The socket has already asked
/// Cedar for `credential.manage`; what to save and whether to start again is
/// the `NativeLinking` port's.
pub(super) async fn set_linking(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    use crate::device_pairing::application::LinkingApply;
    use nessa_protocol::product::generated::{
        PairingLinkingApply, PairingSetLinkingParams, PairingSetLinkingResult,
    };
    let Ok(params) = serde_json::from_value::<PairingSetLinkingParams>(frame.params) else {
        return failure(&frame.id, "invalid_request");
    };
    let Some(linking) = state.native_linking.as_ref() else {
        return failure(&frame.id, PairingErrorCode::PairingUnavailable.as_str());
    };
    match linking.set(linking_caller(session), params.enabled).await {
        Ok(change) => success(
            &frame.id,
            &PairingSetLinkingResult {
                enabled: change.enabled,
                applies: match change.applies {
                    LinkingApply::Unchanged => PairingLinkingApply::Unchanged,
                    LinkingApply::Restarting => PairingLinkingApply::Restarting,
                },
            },
        ),
        Err(error) => failure(&frame.id, linking_refusal(error).as_str()),
    }
}

/// The wire's code for a linking or tunnel refusal.
fn linking_refusal(error: crate::device_pairing::application::LinkingError) -> PairingErrorCode {
    use crate::device_pairing::application::LinkingError;
    match error {
        LinkingError::Busy => PairingErrorCode::PairingBusy,
        LinkingError::AddressInUse => PairingErrorCode::PairingAddressInUse,
        LinkingError::Unavailable => PairingErrorCode::PairingUnavailable,
        LinkingError::AuditUnavailable => PairingErrorCode::PairingAuditUnavailable,
        LinkingError::LinkingOff => PairingErrorCode::PairingLinkingOff,
        LinkingError::TunnelIncomplete => PairingErrorCode::PairingTunnelIncomplete,
    }
}

fn wire_mode(mode: TunnelMode) -> PairingTunnelMode {
    match mode {
        TunnelMode::Quick => PairingTunnelMode::Quick,
        TunnelMode::Named => PairingTunnelMode::Named,
    }
}

/// `pairing.setTunnel`: like `pairing.setLinking`, answered whether or not
/// a tunnel runs now. The socket has already asked Cedar for
/// `credential.manage`. A hostname or token that is not one is refused
/// here, before the port sees it.
pub(super) async fn set_tunnel(
    state: &ProductRouteState,
    session: &AuthenticatedSession,
    frame: RequestFrame,
) -> OutgoingMessage {
    use crate::device_pairing::application::{LinkingApply, TunnelRequest};
    use nessa_protocol::product::generated::{
        PairingLinkingApply, PairingSetTunnelParams, PairingSetTunnelResult,
    };
    let Ok(params) = serde_json::from_value::<PairingSetTunnelParams>(frame.params) else {
        return failure(&frame.id, "invalid_request");
    };
    let hostname = match params.hostname.map(TunnelAddress::try_from).transpose() {
        Ok(hostname) => hostname,
        Err(_) => return failure(&frame.id, "invalid_request"),
    };
    let token = match params
        .token
        .map(|token| TunnelToken::new(token).ok_or(()))
        .transpose()
    {
        Ok(token) => token,
        Err(()) => return failure(&frame.id, "invalid_request"),
    };
    let Some(linking) = state.native_linking.as_ref() else {
        return failure(&frame.id, PairingErrorCode::PairingUnavailable.as_str());
    };
    let request = TunnelRequest {
        enabled: params.enabled,
        mode: params.mode.map(|mode| match mode {
            PairingTunnelMode::Quick => TunnelMode::Quick,
            PairingTunnelMode::Named => TunnelMode::Named,
        }),
        hostname,
        token,
    };
    match linking.set_tunnel(linking_caller(session), request).await {
        Ok(change) => success(
            &frame.id,
            &PairingSetTunnelResult {
                enabled: change.enabled,
                mode: wire_mode(change.mode),
                applies: match change.applies {
                    LinkingApply::Unchanged => PairingLinkingApply::Unchanged,
                    LinkingApply::Restarting => PairingLinkingApply::Restarting,
                },
            },
        ),
        Err(error) => failure(&frame.id, linking_refusal(error).as_str()),
    }
}

/// `pairing.tunnel`: the saved tunnel (never its token) and this run's.
pub(super) async fn tunnel(state: &ProductRouteState, frame: RequestFrame) -> OutgoingMessage {
    use crate::services::application::ServiceStatus;
    use nessa_protocol::product::generated::{PairingTunnelResult, PairingTunnelStatus};
    if frame.params != json!({}) {
        return failure(&frame.id, "invalid_request");
    }
    let Some(linking) = state.native_linking.as_ref() else {
        return failure(&frame.id, PairingErrorCode::PairingUnavailable.as_str());
    };
    let saved = match linking.tunnel().await {
        Ok(saved) => saved,
        Err(error) => return failure(&frame.id, linking_refusal(error).as_str()),
    };
    let live = state
        .tunnel
        .as_ref()
        .map(|tunnel| tunnel.status.borrow().clone());
    let (status, address, failure_reason) = match live {
        None => (PairingTunnelStatus::Off, None, None),
        Some(ServiceStatus::Starting) => (PairingTunnelStatus::Starting, None, None),
        Some(ServiceStatus::Ready(address)) => (
            PairingTunnelStatus::Ready,
            Some(address.as_str().to_owned()),
            None,
        ),
        Some(ServiceStatus::Failed(failure)) => (
            PairingTunnelStatus::Failed,
            None,
            Some(failure.as_str().to_owned()),
        ),
    };
    success(
        &frame.id,
        &PairingTunnelResult {
            enabled: saved.enabled,
            mode: wire_mode(saved.mode),
            hostname: saved.hostname.map(String::from),
            has_token: saved.has_token,
            ingress: saved.ingress.to_string(),
            status,
            address,
            failure: failure_reason,
        },
    )
}

/// The authenticated caller, as the linking audit names it.
fn linking_caller(
    session: &AuthenticatedSession,
) -> crate::device_pairing::application::LinkingCaller {
    let context = session.context();
    crate::device_pairing::application::LinkingCaller {
        organization_id: context.organization_id().as_str().to_owned(),
        principal_id: context.principal_id().as_str().to_owned(),
        credential_id: context.credential_id().as_str().to_owned(),
    }
}

fn initiator_kind(kind: PairingInitiatorKind) -> WireInitiator {
    WireInitiator {
        kind,
        principal_id: None,
        device_key: None,
    }
}

/// The wire code for a refused owner command. Total over every runtime
/// failure, so a new failure does not compile until it is given a code.
pub(super) fn refusal(error: PairingRuntimeError) -> &'static str {
    let code = match error {
        PairingRuntimeError::Owner(OwnerError::Authorization(error)) => {
            return match error {
                AccessError::Denied => "forbidden",
                // Another owner's invitation answers as an unknown one, so its
                // existence is not disclosed (design row O3).
                AccessError::IdentityMismatch => PairingErrorCode::PairingNotFound.as_str(),
                AccessError::Unavailable | AccessError::StaleRevision => {
                    PairingErrorCode::PairingUnavailable.as_str()
                }
                AccessError::InvalidCredential
                | AccessError::CredentialRevoked
                | AccessError::CredentialExpired
                | AccessError::InactiveMembership
                | AccessError::Unsupported => "unauthorized",
            };
        }
        PairingRuntimeError::Owner(OwnerError::Enrollment(error))
        | PairingRuntimeError::Enrollment(error) => store_refusal(error),
        PairingRuntimeError::Owner(OwnerError::Domain(error)) => domain_refusal(error),
        PairingRuntimeError::Owner(OwnerError::Entropy) => PairingErrorCode::PairingUnavailable,
        PairingRuntimeError::Busy | PairingRuntimeError::Registration(RegistrationError::Busy) => {
            PairingErrorCode::PairingBusy
        }
        PairingRuntimeError::NoInvitation => PairingErrorCode::PairingNotFound,
        PairingRuntimeError::Registration(RegistrationError::Crypto(_))
        | PairingRuntimeError::Crypto(_)
        | PairingRuntimeError::Handshake { .. }
        | PairingRuntimeError::WorkerFault(_)
        | PairingRuntimeError::Receiver(_)
        | PairingRuntimeError::Cleanup(_)
        | PairingRuntimeError::Entropy
        // Shutting down: retrying against the next gateway can succeed.
        | PairingRuntimeError::ShuttingDown => PairingErrorCode::PairingUnavailable,
    };
    code.as_str()
}

fn store_refusal(error: PairingStoreError) -> PairingErrorCode {
    match error {
        PairingStoreError::NotFound => PairingErrorCode::PairingNotFound,
        PairingStoreError::Domain(error) => domain_refusal(error),
        PairingStoreError::StageOccupied
        | PairingStoreError::GatewayKeyHistoryExists
        | PairingStoreError::PrivateState(_)
        | PairingStoreError::WorkerFault(_)
        | PairingStoreError::StaleRevision
        | PairingStoreError::Unavailable => PairingErrorCode::PairingUnavailable,
    }
}

fn domain_refusal(error: PairingError) -> PairingErrorCode {
    match error {
        PairingError::AvailableSlotOccupied => PairingErrorCode::PairingSlotOccupied,
        PairingError::Capacity => PairingErrorCode::PairingCapacity,
        PairingError::Conflict => PairingErrorCode::PairingConflict,
        PairingError::Expired
        | PairingError::AttemptsExhausted
        | PairingError::Ineligible
        | PairingError::WrongActor
        | PairingError::StaleGeneration
        | PairingError::Invalid => PairingErrorCode::PairingIneligible,
    }
}
