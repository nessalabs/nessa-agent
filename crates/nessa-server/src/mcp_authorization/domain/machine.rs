//! The authorization chart for one server. Values are replaced, not mutated.
use uuid::Uuid;

/// How long a browser attempt stays acceptable, from the moment it is bound.
pub const CONSENT_DEADLINE_MS: u64 = 10 * 60 * 1000;

/// Whether the current generation may be sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenAvailability {
    /// A stored token may be sent until it expires or is rejected.
    Usable,
    /// No token of this generation may be sent.
    Unavailable,
}

/// Whether a refresh of the current generation is in flight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefreshActivity {
    Idle,
    Refreshing,
}

/// What an incomplete authorization still owes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Obligation {
    /// The token request was sent and its answer was not retained.
    ExchangeUnknown,
    /// A candidate may have been written; the store did not say.
    StoreUnknown,
    /// The secret is stored and the outcome record is not acknowledged.
    EvidencePending,
    /// A refresh was sent and may have rotated the secret.
    RefreshUnknown,
}

/// Why admission was sealed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevokeCause {
    Revoke,
    Removed,
    ResourceChanged,
    InvalidGrant,
}

/// What the authorization server did with a revocation request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteObservation {
    /// A 2xx from an advertised revocation endpoint.
    Acknowledged,
    /// The metadata advertised no revocation endpoint.
    Unsupported,
    /// The endpoint was missing its answer, or the call failed.
    Unconfirmed,
}

/// Whether a private write was confirmed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Publication {
    Acknowledged,
    /// The store refused before a write.
    Refused,
    /// The store did not say whether the write landed.
    Unknown,
}

/// Whether private material is gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Deletion {
    Deleted,
    Failed,
    Unknown,
}

/// What revoke still has to show before consent can start again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settlement {
    pub local_drained: bool,
    pub secret_deleted: bool,
    pub remote: Option<RemoteObservation>,
    pub evidence_acked: bool,
}

impl Settlement {
    fn empty() -> Self {
        Self {
            local_drained: false,
            secret_deleted: false,
            remote: None,
            evidence_acked: false,
        }
    }

    fn settled(self) -> bool {
        self.local_drained && self.secret_deleted && self.remote.is_some() && self.evidence_acked
    }
}

/// The browser attempt bound to one authorize.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attempt {
    pub id: u64,
    pub state: String,
    pub deadline_ms: u64,
    pub consumed: bool,
}

/// Where this server's authorization is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    /// The probe found no authentication requirement.
    Unauthenticated,
    ConsentNeeded,
    Discovering {
        attempt: u64,
    },
    PendingConsent {
        attempt: u64,
    },
    Exchanging {
        attempt: u64,
    },
    Ready {
        availability: TokenAvailability,
        refresh: RefreshActivity,
        scope_required: bool,
    },
    AuthorizationIncomplete {
        obligation: Obligation,
    },
    Revoking {
        cause: RevokeCause,
        settlement: Settlement,
    },
    RevocationIncomplete {
        cause: RevokeCause,
        settlement: Settlement,
    },
}

/// One server's non-secret authorization facts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerAuth {
    pub server: Uuid,
    pub name: String,
    pub resource: String,
    pub phase: Phase,
    pub generation: u64,
    pub issuer: Option<String>,
    pub client_id: Option<String>,
    pub scopes: Vec<String>,
    pub expires_at_ms: Option<u64>,
    pub secret_present: bool,
    pub candidate_retained: bool,
    pub refresh_dispatched: bool,
    pub dispatch_fenced: bool,
    pub attempt: Option<Attempt>,
    pub next_attempt: u64,
    pub pkce_s256: bool,
    pub registration_offered: bool,
    pub revocation_offered: bool,
    pub token_endpoint: Option<String>,
    pub revocation_endpoint: Option<String>,
    pub authorization_endpoint: Option<String>,
    pub acknowledged_domains: Option<String>,
}

impl ServerAuth {
    /// A server with no authorization yet. Its resource is the stored URL.
    pub fn consent_needed(
        server: Uuid,
        name: impl Into<String>,
        resource: impl Into<String>,
    ) -> Self {
        Self {
            server,
            name: name.into(),
            resource: resource.into(),
            phase: Phase::ConsentNeeded,
            generation: 0,
            issuer: None,
            client_id: None,
            scopes: Vec::new(),
            expires_at_ms: None,
            secret_present: false,
            candidate_retained: false,
            refresh_dispatched: false,
            dispatch_fenced: false,
            attempt: None,
            next_attempt: 1,
            pkce_s256: false,
            registration_offered: false,
            revocation_offered: false,
            token_endpoint: None,
            revocation_endpoint: None,
            authorization_endpoint: None,
            acknowledged_domains: None,
        }
    }

    /// What a bearer call may do with this record against `resource` at `now_ms`.
    pub fn admission(&self, now_ms: u64, resource: &str) -> Admission {
        if matches!(self.phase, Phase::Unauthenticated) {
            return Admission::NoneRequired;
        }
        if self.dispatch_fenced || resource != self.resource {
            return Admission::Unauthorized;
        }
        match self.phase {
            Phase::Ready {
                availability: TokenAvailability::Usable,
                refresh: RefreshActivity::Idle,
                scope_required: false,
            } if self.secret_present && !expired(self.expires_at_ms, now_ms) => Admission::Bearer {
                generation: self.generation,
            },
            Phase::Ready {
                scope_required: true,
                ..
            } => Admission::InsufficientScope,
            Phase::Ready { .. } => Admission::NeedsRefresh,
            _ => Admission::Unauthorized,
        }
    }

    /// The next value after `command`, and the effects that follow it.
    pub fn step(&self, command: Command) -> Decision {
        match command {
            Command::Authorize { store_available } => self.authorize(store_available),
            Command::ProbedNoAuth => self.no_auth(),
            Command::DiscoveryAccepted(accepted) => self.discovery_accepted(accepted),
            Command::DiscoveryFailed {
                unsupported_registration,
            } => self.discovery_failed(unsupported_registration),
            Command::Registered { client_id } => self.registered(client_id),
            Command::ConsentReady { state, deadline_ms } => self.consent_ready(state, deadline_ms),
            Command::Callback {
                state,
                now_ms,
                denied,
                resource,
            } => self.callback(&state, now_ms, denied, &resource),
            Command::ExchangeRefused => self.exchange_refused(),
            Command::ExchangeUncertain => self.exchange_uncertain(),
            Command::SecretPublication {
                publication,
                generation,
                expires_at_ms,
                attempt,
                resource,
            } => {
                self.secret_publication(publication, generation, expires_at_ms, attempt, &resource)
            }
            Command::Evidence { acked } => self.evidence(acked),
            Command::RefreshRequested { now_ms, rejected } => {
                self.refresh_requested(now_ms, rejected)
            }
            Command::RefreshNotDispatched => self.refresh_not_dispatched(),
            Command::RefreshLost => self.refresh_lost(),
            Command::RefreshPublication {
                from_generation,
                publication,
                expires_at_ms,
                resource,
            } => self.refresh_publication(from_generation, publication, expires_at_ms, &resource),
            Command::InvalidGrant => self.begin_revoke(RevokeCause::InvalidGrant),
            Command::InsufficientScope => self.insufficient_scope(),
            Command::Revoke => self.begin_revoke(RevokeCause::Revoke),
            Command::ResourceChanged { url: _ } => self.begin_revoke(RevokeCause::ResourceChanged),
            Command::Removed => self.begin_revoke(RevokeCause::Removed),
            Command::RemoteObserved(observation) => self.remote_observed(observation),
            Command::SecretDeletion { deletion } => self.secret_deletion(deletion),
            Command::LocalDrained => self.local_drained(),
            Command::AcknowledgeDomains { digest } => self.acknowledge_domains(digest),
            Command::Restart {
                now_ms,
                secret_present,
                resource_now,
            } => self.restart(now_ms, secret_present, resource_now),
        }
    }

    fn authorize(&self, store_available: bool) -> Decision {
        if !store_available {
            return self.same(Refusal::StoreUnavailable);
        }
        if matches!(self.phase, Phase::AuthorizationIncomplete { .. }) {
            return self.same(Refusal::ReconciliationRequired);
        }
        if matches!(
            self.phase,
            Phase::Revoking { .. } | Phase::RevocationIncomplete { .. } | Phase::Exchanging { .. }
        ) {
            return self.same(Refusal::Busy);
        }
        if let Phase::Ready {
            availability: TokenAvailability::Usable,
            scope_required: false,
            refresh: RefreshActivity::Idle,
        } = self.phase
        {
            if self.secret_present && !self.dispatch_fenced {
                return self.same(Refusal::AlreadyReady);
            }
        }
        let mut next = self.clone();
        let attempt = next.next_attempt;
        next.next_attempt += 1;
        next.attempt = None;
        next.phase = Phase::Discovering { attempt };
        next.dispatch_fenced = false;
        Decision {
            auth: next,
            effects: vec![
                Effect::AbandonAttempt,
                Effect::RecordIntent {
                    action: "authorize",
                },
                Effect::Discover,
            ],
            refusal: None,
        }
    }

    fn no_auth(&self) -> Decision {
        if !matches!(self.phase, Phase::Discovering { .. }) {
            return self.same(Refusal::Stale);
        }
        let mut next = self.clone();
        next.phase = Phase::Unauthenticated;
        next.attempt = None;
        Decision {
            auth: next,
            effects: vec![Effect::RecordOutcome {
                action: "authorize",
            }],
            refusal: None,
        }
    }

    fn discovery_accepted(&self, accepted: AcceptedDiscovery) -> Decision {
        let Phase::Discovering { .. } = self.phase else {
            return self.same(Refusal::Stale);
        };
        if !accepted.pkce_s256 || !accepted.resource_matches || !accepted.issuer_matches {
            return self.back_to_consent(Refusal::BindingRejected);
        }
        if !accepted.registration_offered {
            return self.back_to_consent(Refusal::RegistrationUnsupported);
        }
        let mut next = self.clone();
        next.issuer = Some(accepted.issuer);
        next.scopes = accepted.scopes;
        next.pkce_s256 = true;
        next.registration_offered = true;
        next.revocation_offered = accepted.revocation_offered;
        Decision {
            auth: next,
            effects: vec![Effect::Register],
            refusal: None,
        }
    }

    fn discovery_failed(&self, unsupported_registration: bool) -> Decision {
        if !matches!(self.phase, Phase::Discovering { .. }) {
            return self.same(Refusal::Stale);
        }
        self.back_to_consent(if unsupported_registration {
            Refusal::RegistrationUnsupported
        } else {
            Refusal::DiscoveryFailed
        })
    }

    fn registered(&self, client_id: String) -> Decision {
        if !matches!(self.phase, Phase::Discovering { .. }) {
            return self.same(Refusal::Stale);
        }
        let mut next = self.clone();
        next.client_id = Some(client_id);
        Decision {
            auth: next,
            effects: vec![Effect::PrepareConsent],
            refusal: None,
        }
    }

    fn consent_ready(&self, state: String, deadline_ms: u64) -> Decision {
        let Phase::Discovering { attempt } = self.phase else {
            return self.same(Refusal::Stale);
        };
        let mut next = self.clone();
        next.attempt = Some(Attempt {
            id: attempt,
            state,
            deadline_ms,
            consumed: false,
        });
        next.phase = Phase::PendingConsent { attempt };
        Decision {
            auth: next,
            effects: vec![Effect::RecordOutcome {
                action: "authorize",
            }],
            refusal: None,
        }
    }

    fn callback(&self, state: &str, now_ms: u64, denied: bool, resource: &str) -> Decision {
        let Phase::PendingConsent { attempt } = self.phase else {
            return self.same(Refusal::StaleCallback);
        };
        let Some(current) = &self.attempt else {
            return self.same(Refusal::StaleCallback);
        };
        if current.consumed
            || current.id != attempt
            || current.state != state
            || self.resource != resource
        {
            return self.same(Refusal::StaleCallback);
        }
        if now_ms > current.deadline_ms {
            return self.back_to_consent(Refusal::Expired);
        }
        if denied {
            return self.back_to_consent(Refusal::Denied);
        }
        let mut next = self.clone();
        if let Some(attempt) = next.attempt.as_mut() {
            attempt.consumed = true;
        }
        next.phase = Phase::Exchanging { attempt };
        Decision {
            auth: next,
            effects: vec![
                Effect::RecordIntent { action: "exchange" },
                Effect::Exchange,
            ],
            refusal: None,
        }
    }

    fn exchange_refused(&self) -> Decision {
        if !matches!(self.phase, Phase::Exchanging { .. }) {
            return self.same(Refusal::Stale);
        }
        let mut next = self.back_to_consent(Refusal::ExchangeRefused);
        next.auth.candidate_retained = false;
        next
    }

    fn exchange_uncertain(&self) -> Decision {
        if !matches!(self.phase, Phase::Exchanging { .. }) {
            return self.same(Refusal::Stale);
        }
        self.incomplete(Obligation::ExchangeUnknown)
    }

    /// The exchange that produced a token reply is still the one in this record.
    pub fn exchange_reply_current(&self, attempt: u64, resource: &str) -> bool {
        matches!(self.phase, Phase::Exchanging { attempt: current } if current == attempt)
            && self.resource == resource
    }

    /// The refresh that produced a token reply is still the one in this record.
    pub fn refresh_reply_current(&self, generation: u64, resource: &str) -> bool {
        matches!(
            self.phase,
            Phase::Ready {
                refresh: RefreshActivity::Refreshing,
                ..
            }
        ) && self.generation == generation
            && self.resource == resource
    }

    fn secret_publication(
        &self,
        publication: Publication,
        generation: u64,
        expires_at_ms: Option<u64>,
        attempt: u64,
        resource: &str,
    ) -> Decision {
        if !self.exchange_reply_current(attempt, resource) {
            return self.stale_publication();
        }
        self.note_publication(publication, generation, expires_at_ms)
    }

    fn note_publication(
        &self,
        publication: Publication,
        generation: u64,
        expires_at_ms: Option<u64>,
    ) -> Decision {
        let exchanging = matches!(self.phase, Phase::Exchanging { .. });
        match publication {
            Publication::Refused if exchanging && !self.candidate_retained => {
                self.back_to_consent(Refusal::StoreRefused)
            }
            Publication::Refused => self.incomplete(Obligation::StoreUnknown),
            Publication::Unknown => self.incomplete(Obligation::StoreUnknown),
            Publication::Acknowledged => {
                let mut next = self.clone();
                next.secret_present = true;
                next.candidate_retained = true;
                next.generation = generation;
                next.expires_at_ms = expires_at_ms;
                next.refresh_dispatched = false;
                if exchanging {
                    next.phase = Phase::Exchanging {
                        attempt: match self.phase {
                            Phase::Exchanging { attempt } => attempt,
                            _ => 0,
                        },
                    };
                }
                Decision {
                    auth: next,
                    effects: vec![Effect::RecordOutcome { action: "store" }],
                    refusal: None,
                }
            }
        }
    }

    fn evidence(&self, acked: bool) -> Decision {
        if let Phase::Revoking {
            cause,
            mut settlement,
        } = self.phase
        {
            if !acked {
                return self.incomplete_revoke(cause, settlement);
            }
            settlement.evidence_acked = true;
            return self.settle(cause, settlement, true);
        }
        if !acked {
            if self.candidate_retained || self.secret_present {
                return self.incomplete(Obligation::EvidencePending);
            }
            return self.same(Refusal::AuditUnavailable);
        }
        if self.secret_present
            && matches!(
                self.phase,
                Phase::Exchanging { .. }
                    | Phase::Ready {
                        refresh: RefreshActivity::Refreshing,
                        ..
                    }
            )
        {
            let mut next = self.clone();
            next.phase = Phase::Ready {
                availability: TokenAvailability::Usable,
                refresh: RefreshActivity::Idle,
                scope_required: false,
            };
            next.dispatch_fenced = false;
            next.candidate_retained = false;
            next.attempt = None;
            return Decision {
                auth: next,
                effects: Vec::new(),
                refusal: None,
            };
        }
        self.same(Refusal::Stale)
    }

    fn refresh_requested(&self, now_ms: u64, rejected: bool) -> Decision {
        match self.phase {
            Phase::Ready {
                refresh: RefreshActivity::Refreshing,
                ..
            } => self.same(Refusal::JoinRefresh),
            Phase::Ready {
                refresh: RefreshActivity::Idle,
                scope_required: false,
                ..
            } if self.secret_present && !self.dispatch_fenced => {
                let mut next = self.clone();
                next.phase = Phase::Ready {
                    availability: if rejected || expired(self.expires_at_ms, now_ms) {
                        TokenAvailability::Unavailable
                    } else {
                        TokenAvailability::Usable
                    },
                    refresh: RefreshActivity::Refreshing,
                    scope_required: false,
                };
                next.refresh_dispatched = false;
                Decision {
                    auth: next,
                    effects: vec![
                        Effect::RecordIntent { action: "refresh" },
                        Effect::Refresh {
                            generation: self.generation,
                        },
                    ],
                    refusal: None,
                }
            }
            _ => self.same(Refusal::Unauthorized),
        }
    }

    fn refresh_not_dispatched(&self) -> Decision {
        let Phase::Ready {
            availability,
            refresh: RefreshActivity::Refreshing,
            scope_required,
        } = self.phase
        else {
            return self.same(Refusal::Stale);
        };
        if self.refresh_dispatched {
            return self.refresh_lost();
        }
        let mut next = self.clone();
        next.phase = Phase::Ready {
            availability,
            refresh: RefreshActivity::Idle,
            scope_required,
        };
        Decision {
            auth: next,
            effects: Vec::new(),
            refusal: None,
        }
    }

    fn refresh_lost(&self) -> Decision {
        if !matches!(
            self.phase,
            Phase::Ready {
                refresh: RefreshActivity::Refreshing,
                ..
            }
        ) {
            return self.same(Refusal::Stale);
        }
        self.incomplete(Obligation::RefreshUnknown)
    }

    fn refresh_publication(
        &self,
        from_generation: u64,
        publication: Publication,
        expires_at_ms: Option<u64>,
        resource: &str,
    ) -> Decision {
        if !self.refresh_reply_current(from_generation, resource) {
            return self.stale_publication();
        }
        self.note_publication(publication, self.generation + 1, expires_at_ms)
    }

    /// A reply whose attempt or resource is no longer current. A live exchange
    /// or refresh owns the secret; deleting it would drop that replacement.
    fn stale_publication(&self) -> Decision {
        let replacement_owns_secret = matches!(
            self.phase,
            Phase::Exchanging { .. }
                | Phase::Ready {
                    refresh: RefreshActivity::Refreshing,
                    ..
                }
        );
        Decision {
            auth: self.clone(),
            effects: if replacement_owns_secret {
                Vec::new()
            } else {
                vec![Effect::DeleteCandidate]
            },
            refusal: Some(Refusal::Stale),
        }
    }

    fn insufficient_scope(&self) -> Decision {
        let Phase::Ready { refresh, .. } = self.phase else {
            return self.same(Refusal::Stale);
        };
        let mut next = self.clone();
        next.phase = Phase::Ready {
            availability: TokenAvailability::Unavailable,
            refresh,
            scope_required: true,
        };
        Decision {
            auth: next,
            effects: vec![Effect::RecordOutcome { action: "scope" }],
            refusal: None,
        }
    }

    fn begin_revoke(&self, cause: RevokeCause) -> Decision {
        match self.phase {
            Phase::Revoking { .. } => self.same(Refusal::JoinRevoke),
            Phase::RevocationIncomplete { cause, settlement } => {
                self.settle(cause, settlement, false)
            }
            _ => {
                let mut next = self.clone();
                next.dispatch_fenced = true;
                next.attempt = None;
                next.refresh_dispatched = false;
                next.phase = Phase::Revoking {
                    cause,
                    settlement: Settlement::empty(),
                };
                Decision {
                    auth: next,
                    effects: vec![
                        Effect::AbandonAttempt,
                        Effect::RecordIntent { action: "revoke" },
                        Effect::DrainSessions,
                        Effect::RevokeRemote,
                        Effect::DeleteSecret,
                    ],
                    refusal: None,
                }
            }
        }
    }

    fn remote_observed(&self, observation: RemoteObservation) -> Decision {
        match self.phase {
            Phase::Revoking {
                cause,
                mut settlement,
            } => {
                settlement.remote = Some(observation);
                self.settle(cause, settlement, true)
            }
            _ => self.same(Refusal::Stale),
        }
    }

    fn secret_deletion(&self, deletion: Deletion) -> Decision {
        let Phase::Revoking {
            cause,
            mut settlement,
        } = self.phase
        else {
            return self.same(Refusal::Stale);
        };
        match deletion {
            Deletion::Deleted => {
                settlement.secret_deleted = true;
                let mut decided = self.settle(cause, settlement, true);
                decided.auth.secret_present = false;
                decided.auth.candidate_retained = false;
                decided
            }
            Deletion::Failed | Deletion::Unknown => {
                settlement.secret_deleted = false;
                self.incomplete_revoke(cause, settlement)
            }
        }
    }

    fn local_drained(&self) -> Decision {
        let Phase::Revoking {
            cause,
            mut settlement,
        } = self.phase
        else {
            return self.same(Refusal::Stale);
        };
        settlement.local_drained = true;
        self.settle(cause, settlement, true)
    }

    fn acknowledge_domains(&self, digest: String) -> Decision {
        let mut next = self.clone();
        next.acknowledged_domains = Some(digest);
        Decision {
            auth: next,
            effects: vec![Effect::RecordOutcome { action: "domains" }],
            refusal: None,
        }
    }

    fn restart(&self, now_ms: u64, secret_present: bool, resource_now: Option<String>) -> Decision {
        let mut next = self.clone();
        next.secret_present = secret_present;
        match self.phase {
            Phase::Discovering { .. } | Phase::PendingConsent { .. } => {
                next.phase = Phase::ConsentNeeded;
                next.attempt = None;
            }
            Phase::Exchanging { .. } => {
                next.phase = Phase::AuthorizationIncomplete {
                    obligation: Obligation::ExchangeUnknown,
                };
                next.dispatch_fenced = true;
                next.candidate_retained = true;
            }
            Phase::Ready { .. } if self.refresh_dispatched => {
                next.phase = Phase::AuthorizationIncomplete {
                    obligation: Obligation::RefreshUnknown,
                };
                next.dispatch_fenced = true;
                next.candidate_retained = true;
            }
            Phase::Ready {
                refresh: RefreshActivity::Refreshing,
                scope_required,
                ..
            } => {
                next.phase = Phase::Ready {
                    availability: TokenAvailability::Unavailable,
                    refresh: RefreshActivity::Idle,
                    scope_required,
                };
            }
            Phase::Ready { scope_required, .. } => {
                let matches_resource = resource_now.as_deref() == Some(self.resource.as_str());
                if !matches_resource || !secret_present || self.dispatch_fenced {
                    next.dispatch_fenced = true;
                    next.phase = Phase::ConsentNeeded;
                } else if scope_required {
                    next.phase = Phase::Ready {
                        availability: TokenAvailability::Unavailable,
                        refresh: RefreshActivity::Idle,
                        scope_required: true,
                    };
                } else if expired(self.expires_at_ms, now_ms) {
                    next.phase = Phase::Ready {
                        availability: TokenAvailability::Unavailable,
                        refresh: RefreshActivity::Idle,
                        scope_required: false,
                    };
                } else {
                    next.phase = Phase::Ready {
                        availability: TokenAvailability::Usable,
                        refresh: RefreshActivity::Idle,
                        scope_required: false,
                    };
                }
            }
            Phase::Unauthenticated => {}
            Phase::ConsentNeeded => {
                next.dispatch_fenced = secret_present || self.dispatch_fenced;
            }
            Phase::AuthorizationIncomplete { .. }
            | Phase::Revoking { .. }
            | Phase::RevocationIncomplete { .. } => {
                next.dispatch_fenced = true;
            }
        }
        Decision {
            auth: next,
            effects: Vec::new(),
            refusal: None,
        }
    }

    fn settle(
        &self,
        cause: RevokeCause,
        settlement: Settlement,
        already_revoking: bool,
    ) -> Decision {
        let mut next = self.clone();
        next.dispatch_fenced = true;
        if settlement.settled() {
            next.phase = Phase::ConsentNeeded;
            next.dispatch_fenced = false;
            next.attempt = None;
            next.secret_present = false;
            next.candidate_retained = false;
            next.refresh_dispatched = false;
            Decision {
                auth: next,
                effects: Vec::new(),
                refusal: None,
            }
        } else if already_revoking {
            next.phase = Phase::Revoking { cause, settlement };
            Decision {
                auth: next,
                effects: Vec::new(),
                refusal: None,
            }
        } else {
            next.phase = Phase::Revoking { cause, settlement };
            Decision {
                auth: next,
                effects: vec![
                    Effect::RecordIntent { action: "revoke" },
                    Effect::DrainSessions,
                    Effect::RevokeRemote,
                    Effect::DeleteSecret,
                ],
                refusal: None,
            }
        }
    }

    fn incomplete_revoke(&self, cause: RevokeCause, settlement: Settlement) -> Decision {
        let mut next = self.clone();
        next.dispatch_fenced = true;
        next.phase = Phase::RevocationIncomplete { cause, settlement };
        Decision {
            auth: next,
            effects: Vec::new(),
            refusal: None,
        }
    }

    fn incomplete(&self, obligation: Obligation) -> Decision {
        let mut next = self.clone();
        next.dispatch_fenced = true;
        next.candidate_retained = true;
        next.phase = Phase::AuthorizationIncomplete { obligation };
        next.refresh_dispatched = false;
        Decision {
            auth: next,
            effects: vec![Effect::RecordOutcome {
                action: "incomplete",
            }],
            refusal: None,
        }
    }

    fn back_to_consent(&self, refusal: Refusal) -> Decision {
        let mut next = self.clone();
        next.phase = Phase::ConsentNeeded;
        next.attempt = None;
        Decision {
            auth: next,
            effects: vec![Effect::AbandonAttempt],
            refusal: Some(refusal),
        }
    }

    fn same(&self, refusal: Refusal) -> Decision {
        Decision {
            auth: self.clone(),
            effects: Vec::new(),
            refusal: Some(refusal),
        }
    }
}

fn expired(expires_at_ms: Option<u64>, now_ms: u64) -> bool {
    expires_at_ms.is_some_and(|expires| now_ms >= expires)
}

/// What `bearer` may do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    /// The server needs no token.
    NoneRequired,
    /// Send the secret at this generation.
    Bearer { generation: u64 },
    /// Consent is required, or the token must not be sent.
    Unauthorized,
    /// A broader scope needs a new consent. Do not refresh or replay.
    InsufficientScope,
    /// Join or start the one refresh of the current generation.
    NeedsRefresh,
}

/// A discovery the owner checked against the configured resource.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcceptedDiscovery {
    pub issuer: String,
    pub scopes: Vec<String>,
    pub pkce_s256: bool,
    pub registration_offered: bool,
    pub revocation_offered: bool,
    pub resource_matches: bool,
    pub issuer_matches: bool,
}

/// What the owner asked the chart to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Authorize {
        store_available: bool,
    },
    ProbedNoAuth,
    DiscoveryAccepted(AcceptedDiscovery),
    DiscoveryFailed {
        unsupported_registration: bool,
    },
    Registered {
        client_id: String,
    },
    ConsentReady {
        state: String,
        deadline_ms: u64,
    },
    Callback {
        state: String,
        now_ms: u64,
        denied: bool,
        /// Resource URL of the attempt that issued this state.
        resource: String,
    },
    ExchangeRefused,
    ExchangeUncertain,
    SecretPublication {
        publication: Publication,
        generation: u64,
        expires_at_ms: Option<u64>,
        attempt: u64,
        resource: String,
    },
    Evidence {
        acked: bool,
    },
    RefreshRequested {
        now_ms: u64,
        rejected: bool,
    },
    RefreshNotDispatched,
    RefreshLost,
    RefreshPublication {
        from_generation: u64,
        publication: Publication,
        expires_at_ms: Option<u64>,
        resource: String,
    },
    InvalidGrant,
    InsufficientScope,
    Revoke,
    ResourceChanged {
        url: String,
    },
    Removed,
    RemoteObserved(RemoteObservation),
    SecretDeletion {
        deletion: Deletion,
    },
    LocalDrained,
    AcknowledgeDomains {
        digest: String,
    },
    Restart {
        now_ms: u64,
        secret_present: bool,
        resource_now: Option<String>,
    },
}

/// Work the owner performs after a step. Order is the order to run them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    RecordIntent { action: &'static str },
    RecordOutcome { action: &'static str },
    Discover,
    Register,
    PrepareConsent,
    Exchange,
    Refresh { generation: u64 },
    RevokeRemote,
    DeleteSecret,
    DeleteCandidate,
    DrainSessions,
    AbandonAttempt,
}

/// Why a command did not move the chart the way the caller hoped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    StoreUnavailable,
    AlreadyReady,
    ReconciliationRequired,
    Busy,
    BindingRejected,
    RegistrationUnsupported,
    DiscoveryFailed,
    Stale,
    StaleCallback,
    Expired,
    Denied,
    ExchangeRefused,
    StoreRefused,
    AuditUnavailable,
    JoinRefresh,
    Unauthorized,
    JoinRevoke,
}

/// The chart's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision {
    pub auth: ServerAuth,
    pub effects: Vec<Effect>,
    pub refusal: Option<Refusal>,
}
