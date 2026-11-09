//! Complete contextual restoration, total over the closed audit representation.
use super::{
    audit::{self, DeliveryEvidence, Record},
    cursor,
    publication::SavedAudit,
    registry, retirement,
    stored::{self, StoredDelivery},
    unavailable, uuid,
};
use crate::webhooks::{
    application::{OutboxError, MAX_REGISTRATIONS},
    domain::{Phase, RegistrationPhase},
};
use nessa_local_database::rusqlite::{Connection, OptionalExtension};
use std::collections::BTreeSet;

pub(super) mod progress;

pub(super) fn validate_owed(connection: &Connection) -> Result<(), OutboxError> {
    super::intents::validate(connection)?;
    validate_queued(connection)?;
    super::source_reset::validate_owed(connection)?;
    validate_records(connection, true)?;
    validate_histories(connection, true)
}

fn validate_records(connection: &Connection, owed: bool) -> Result<(), OutboxError> {
    retirement::validate(connection)?;
    let mut statement = connection
        .prepare(audit_selection(owed))
        .map_err(unavailable)?;
    let rows = statement
        .query_map([], SavedAudit::row)
        .map_err(unavailable)?;
    let mut initial_registrations = BTreeSet::new();
    for row in rows {
        let saved = row.map_err(unavailable)?;
        let is_owed = saved.published == 0;
        let (_, text) = saved.checked()?;
        let record = Record::decode(&text)?;
        contextual(connection, &record, is_owed)?;
        if progress::is_initial(&record) {
            initial_registrations.insert(record.keys()?.1);
            if initial_registrations.len() > 2 * MAX_REGISTRATIONS {
                return Err(OutboxError::Invalid);
            }
        }
    }
    for registration in initial_registrations {
        progress::validate(connection, registration)?;
    }
    Ok(())
}

/// No wildcard arm: adding an audit variant requires its owning relationship.
pub(super) fn contextual(
    connection: &Connection,
    record: &Record,
    owed: bool,
) -> Result<(), OutboxError> {
    match record {
        Record::Registration {
            record_id,
            registration,
            credential,
            principal,
            organization,
            owner,
            cause,
            initiator,
            stop_cause,
            stop_at_ms,
            credential_stop,
            at_ms,
            ..
        } => {
            let id = uuid(registration)?;
            let live = connection
                .query_row(
                    &format!("{} FROM registrations WHERE id=?1", registry::SELECT),
                    [registration],
                    |r| registry::StoredRegistration::row(r, 0),
                )
                .optional()
                .map_err(unavailable)?
                .map(registry::StoredRegistration::checked)
                .transpose()?;
            let retired = retirement::read(connection, id)?;
            let (target, stop, issued_at) = match (&live, &retired) {
                (Some(live), None) => (
                    &live.registration.target,
                    live.stop
                        .as_ref()
                        .filter(|_| live.lifecycle.stopped().is_some()),
                    live.issued_at_ms,
                ),
                (None, Some(retired)) => {
                    (&retired.target, Some(&retired.stop), retired.issued_at_ms)
                }
                _ => return Err(OutboxError::Invalid),
            };
            if audit::checked_target(credential, principal, organization, owner)? != *target {
                return Err(OutboxError::Invalid);
            }
            if retired.as_ref().is_some_and(|facts| {
                facts.kind == retirement::RetirementKind::Cancelled && *cause != facts.kind.cause()
            }) {
                return Err(OutboxError::Invalid);
            }
            match cause {
                audit::Cause::CredentialIssue => {
                    if *at_ms != issued_at {
                        return Err(OutboxError::Invalid);
                    }
                }
                audit::Cause::CredentialExpiryObserved => {
                    if owed
                        && !matches!(&live, Some(live) if live.lifecycle.phase() == RegistrationPhase::ExpiryObserved)
                    {
                        return Err(OutboxError::Invalid);
                    }
                    let observed = match (&live, &retired) {
                        (Some(live), None) => live.stop.as_ref(),
                        (None, Some(retired)) => Some(&retired.stop),
                        _ => None,
                    };
                    if observed
                        != Some(&audit::check_stop(
                            &audit::Cause::CredentialExpired,
                            initiator,
                            *at_ms,
                            credential_stop.as_ref(),
                        )?)
                    {
                        return Err(OutboxError::Invalid);
                    }
                }
                audit::Cause::CredentialRevoked | audit::Cause::CredentialExpired => {
                    if stop
                        != Some(&audit::check_stop(
                            cause,
                            initiator,
                            *at_ms,
                            credential_stop.as_ref(),
                        )?)
                    {
                        return Err(OutboxError::Invalid);
                    }
                }
                audit::Cause::Drained | audit::Cause::RegistrationCancelled => {
                    let retired = retired.ok_or(OutboxError::Invalid)?;
                    if retired.audit != uuid(record_id)?
                        || *cause != retired.kind.cause()
                        || live.is_some()
                        || retired.at_ms != *at_ms
                        || retired.stop
                            != audit::check_stop(
                                stop_cause.as_ref().ok_or(OutboxError::Invalid)?,
                                initiator,
                                stop_at_ms.ok_or(OutboxError::Invalid)?,
                                credential_stop.as_ref(),
                            )?
                    {
                        return Err(OutboxError::Invalid);
                    }
                }
                audit::Cause::Admission
                | audit::Cause::Response
                | audit::Cause::Deadline
                | audit::Cause::AdmissionRefused
                | audit::Cause::AuditUnavailable
                | audit::Cause::Reconcile
                | audit::Cause::AdmissionExpired
                | audit::Cause::Restart
                | audit::Cause::SourceChanged
                | audit::Cause::SourceCheckpoint
                | audit::Cause::OwnerReset => return Err(OutboxError::Invalid),
            }
        }
        Record::Cursor {
            selection,
            source_reset_id,
            registration,
            conversation_id,
            prior_scope,
            prior_position,
            observed_scope,
            initiator,
            at_ms,
            ..
        } => {
            let fact = super::source_reset::read(connection, uuid(source_reset_id)?)?
                .ok_or(OutboxError::Invalid)?;
            if fact.registration.to_string() != *registration
                || fact.conversation.to_string() != *conversation_id
                || fact.prior_scope != prior_scope.checked()?
                || fact.prior_position != *prior_position
                || fact.observed_scope != observed_scope.checked()?
                || fact.selection != *selection
                || fact
                    .selection
                    .as_ref()
                    .map(|s| s.actor.clone())
                    .unwrap_or_else(|| fact.stop.initiator.label(fact.registration))
                    != *initiator
                || fact.selection.as_ref().map_or(fact.stop.at_ms, |s| s.at_ms) != *at_ms
            {
                return Err(OutboxError::Invalid);
            }
            if let Some(selection) = &fact.selection {
                let saved = registry::by_id(connection, fact.registration)?;
                if selection.credential != saved.registration.target.credential.as_str()
                    || selection.issued_at_ms != saved.issued_at_ms
                {
                    return Err(OutboxError::Invalid);
                }
                if let Some(original) =
                    super::source_reset::read(connection, uuid(&selection.request)?)?
                {
                    if original.selection.is_some()
                        || original.registration != fact.registration
                        || original.conversation != fact.conversation
                        || original.prior_scope != fact.prior_scope
                        || original.prior_position != fact.prior_position
                        || original.observed_scope != fact.observed_scope
                        || original.stop != fact.stop
                    {
                        return Err(OutboxError::Invalid);
                    }
                }
            }
            if owed {
                let current = cursor::read(connection, fact.registration, &fact.conversation)?
                    .ok_or(OutboxError::Invalid)?;
                if current.reset != Some(fact.id)
                    || fact.selection.as_ref().is_some_and(|selection| {
                        current.position != 0
                            || selection.scope.checked().as_ref() != Ok(&current.scope)
                    })
                {
                    return Err(OutboxError::Invalid);
                }
            }
        }
        Record::Delivery {
            cause,
            initiator,
            at_ms,
            ..
        } => {
            let evidence = record.delivery_evidence()?.ok_or(OutboxError::Invalid)?;
            let saved =
                stored::by_id(connection, evidence.after.id())?.ok_or(OutboxError::Invalid)?;
            let registration = registry::by_id(connection, saved.registration)?;
            if evidence.source_reset.is_some()
                && (evidence.source_reset != saved.source_reset
                    || evidence.source_stop != saved.source_stop)
            {
                return Err(OutboxError::Invalid);
            }
            if *cause == audit::Cause::SourceChanged {
                let capture = super::source_reset::delivery(
                    connection,
                    saved.registration,
                    &saved.conversation,
                    &saved.scope,
                    saved.position,
                    &saved.delivery,
                )?
                .ok_or(OutboxError::Invalid)?;
                if evidence.source_reset != Some(capture.fact.id)
                    || evidence.source_stop != Some(capture.fact.stop)
                    || *initiator != capture.fact.stop.initiator.label(capture.fact.registration)
                {
                    return Err(OutboxError::Invalid);
                }
                if evidence.before.as_ref() == Some(&capture.before) {
                    if *at_ms != capture.fact.stop.at_ms {
                        return Err(OutboxError::Invalid);
                    }
                } else if capture.before.phase() != Phase::Sending {
                    return Err(OutboxError::Invalid);
                }
            }
            if evidence.registration != saved.registration
                || evidence.target != registration.registration.target
                || evidence.conversation != saved.conversation
                || evidence.scope != saved.scope
                || evidence.position != saved.position
                || evidence.credential_stop.as_ref().is_some_and(|stop| {
                    registration.lifecycle.stopped().is_none()
                        || registration.stop.as_ref() != Some(stop)
                })
            {
                return Err(OutboxError::Invalid);
            }
        }
    }
    Ok(())
}

pub(super) fn validate(connection: &Connection) -> Result<(), OutboxError> {
    validate_queued(connection)?;
    if connection.prepare("SELECT 1 FROM audit a WHERE a.delivery IS NOT NULL AND NOT EXISTS (SELECT 1 FROM deliveries d WHERE d.id=a.delivery) LIMIT 1").map_err(unavailable)?.exists([]).map_err(unavailable)? { return Err(OutboxError::Invalid); }
    validate_records(connection, false)?;
    validate_histories(connection, false)
}

/// Publication visits only histories named by owed rows; startup checks all.
fn validate_histories(connection: &Connection, owed: bool) -> Result<(), OutboxError> {
    let filter = if owed {
        " WHERE id IN (SELECT delivery FROM audit WHERE published=0 AND delivery IS NOT NULL)"
    } else {
        ""
    };
    let mut deliveries = connection
        .prepare(&format!("{} FROM deliveries{filter}", stored::SELECT))
        .map_err(unavailable)?;
    let rows = deliveries
        .query_map([], StoredDelivery::row)
        .map_err(unavailable)?;
    for row in rows {
        let saved = row.map_err(unavailable)?.checked(connection)?;
        let mut statement=connection.prepare("SELECT id,registration,delivery,record,published,source_reset FROM audit WHERE delivery=?1 ORDER BY rowid").map_err(unavailable)?;
        let history = statement
            .query_map([saved.delivery.id().to_string()], SavedAudit::row)
            .map_err(unavailable)?;
        let mut previous: Option<DeliveryEvidence> = None;
        let mut capture_seen = saved.capture_before.is_none();
        // Streaming chain validation keeps one predecessor. Delivery's finite
        // transition owner rejects any extension past its attempt/terminal budget.
        for row in history {
            let (_, text) = row.map_err(unavailable)?.checked()?;
            let record = Record::decode(&text)?;
            let next = record.delivery_evidence()?.ok_or(OutboxError::Invalid)?;
            if next.source_reset.is_some() && !capture_seen {
                return Err(OutboxError::Invalid);
            }
            if let Some(capture) = &saved.capture_before {
                capture_seen |= next.before.as_ref() == Some(capture) || &next.after == capture;
            }
            if previous
                .as_ref()
                .map_or(next.before.is_some(), |prior| !next.follows(prior))
            {
                return Err(OutboxError::Invalid);
            }
            previous = Some(next);
        }
        let last = previous.ok_or(OutboxError::Invalid)?;
        if !capture_seen
            || last.after != saved.delivery
            || (last.source_reset != saved.source_reset
                && !(saved.delivery.phase() == Phase::Sending
                    && last.source_reset.is_none()
                    && saved.capture_before.as_ref() == Some(&saved.delivery)))
            || (last.source_stop != saved.source_stop
                && !(saved.delivery.phase() == Phase::Sending && last.source_stop.is_none()))
        {
            return Err(OutboxError::Invalid);
        }
    }
    Ok(())
}

/// Separate SQL shapes let live publication search the published-state index.
pub(super) fn audit_selection(owed: bool) -> &'static str {
    if owed {
        "SELECT id,registration,delivery,record,published,source_reset FROM audit WHERE published=0"
    } else {
        "SELECT id,registration,delivery,record,published,source_reset FROM audit"
    }
}

/// Only current unstarted work participates. Late Sending outcomes can leave
/// valid Retry rows, and retained terminal histories do not need another scan.
pub(super) const QUEUED_REGISTRATIONS: &str =
    "SELECT DISTINCT registration FROM deliveries INDEXED BY ready_deliveries WHERE phase='queued' LIMIT ?1";
pub(super) fn validate_queued(connection: &Connection) -> Result<(), OutboxError> {
    let mut statement = connection
        .prepare(QUEUED_REGISTRATIONS)
        .map_err(unavailable)?;
    let rows = statement
        .query_map([(MAX_REGISTRATIONS + 1) as i64], |row| {
            row.get::<_, String>(0)
        })
        .map_err(unavailable)?;
    for (index, id) in rows.enumerate() {
        if index == MAX_REGISTRATIONS {
            return Err(OutboxError::Invalid);
        }
        let id = uuid(&id.map_err(unavailable)?)?;
        if !registry::by_id(connection, id)?.lifecycle.retains_queued() {
            return Err(OutboxError::Invalid);
        }
    }
    Ok(())
}
