//! Authenticated vault control. No command returns a secret value or encryption key.
use super::{
    socket::{failure, success},
    state::ProductRouteState,
};
use crate::vault::application::{verify_and_retire, Migration};
use nessa_auth::application::session::AuthenticatedSession;
use nessa_protocol::{
    product::generated::{
        VaultPassphraseChangeState, VaultPassphraseChangeStatus, VaultPassphraseParams, VaultState,
        VaultStatus,
    },
    protocol::{OutgoingMessage, RequestFrame},
};
use nessa_vault::{
    application::{Caller, ChangeReceipt, Lease, Session, Vault},
    domain::{ChangePublication, Failure, Passphrase, Status},
};
use serde_json::json;
use std::sync::Arc;
/// Composed before providers so onboarding can create the vault independently.
pub struct VaultRoutes {
    pub vault: Arc<Vault>,
    pub migration: Arc<dyn Migration>,
}
/// Unique connection-owner guard. Request state contains only its non-owning lease.
pub(super) struct ConnectionVault {
    session: Session,
    github: Option<Arc<super::GithubRoutes>>,
}
impl ConnectionVault {
    pub(super) fn attach(
        state: &mut ProductRouteState,
        session: &AuthenticatedSession,
    ) -> Option<Self> {
        let vault = state.vault.as_ref()?;
        let context = session.context();
        let owner = vault.vault.session(
            uuid::Uuid::new_v4().to_string(),
            Caller {
                principal: context.principal_id().as_str().to_owned(),
                credential: context.credential_id().as_str().to_owned(),
            },
        );
        state.vault_lease = Some(owner.lease());
        Some(Self {
            session: owner,
            github: state.github.clone(),
        })
    }
    pub(super) fn close(&mut self) {
        if self.session.close().is_err() {
            tracing::error!("vault key cleared but connection-close audit failed");
        }
        if let Some(github) = &self.github {
            if github.connections.clear_locked().is_err() {
                tracing::error!("vault sealed but GitHub cleanup audit failed");
            }
        }
    }
}
impl Drop for ConnectionVault {
    fn drop(&mut self) {
        self.close();
    }
}
pub(super) async fn dispatch(
    state: &ProductRouteState,
    mut frame: RequestFrame,
) -> OutgoingMessage {
    let Some(routes) = state.vault.as_ref() else {
        return failure(&frame.id, "vault_unavailable");
    };
    let Some(lease) = state.vault_lease.clone() else {
        return failure(&frame.id, "vault_session_closed");
    };
    if frame.method == "vault.changePassphrase" {
        return change_passphrase(state, routes.clone(), lease, frame).await;
    }
    let operation = match frame.method.as_str() {
        "vault.status" => Operation::Status,
        "vault.setup" => Operation::Setup,
        "vault.unlock" => Operation::Unlock,
        "vault.lock" => Operation::Lock,
        _ => return failure(&frame.id, "unknown_method"),
    };
    let passphrase = if matches!(operation, Operation::Setup | Operation::Unlock) {
        let Ok(params) = serde_json::from_value::<VaultPassphraseParams>(std::mem::replace(
            &mut frame.params,
            json!({}),
        )) else {
            return failure(&frame.id, "invalid_request");
        };
        match Passphrase::new(params.passphrase) {
            Ok(passphrase) => Some(passphrase),
            Err(problem) => return failure(&frame.id, code(problem)),
        }
    } else {
        if frame.params != json!({}) {
            return failure(&frame.id, "invalid_request");
        }
        None
    };
    let routes = routes.clone();
    let result = tokio::task::spawn_blocking(move || run(&routes, &lease, operation, passphrase))
        .await
        .unwrap_or(Err(Failure::Unavailable));
    let cleanup = if matches!(operation, Operation::Lock) {
        state
            .github
            .as_ref()
            .map(|github| github.connections.clear_locked())
            .transpose()
    } else {
        Ok(None)
    };
    if cleanup.is_err() && result.is_ok() {
        return failure(&frame.id, "vault_audit_unavailable");
    }
    match result {
        Ok(status) => {
            if matches!(operation, Operation::Setup | Operation::Unlock) {
                if let Some(github) = &state.github {
                    if github.connections.restore().await.is_err() {
                        tracing::warn!(
                            "vault unlocked; saved GitHub connection could not be restored"
                        );
                    }
                }
            }
            success(
                &frame.id,
                &VaultStatus {
                    state: match status {
                        Status::Unconfigured => VaultState::Unconfigured,
                        Status::Locked => VaultState::Locked,
                        Status::Unlocked => VaultState::Unlocked,
                    },
                },
            )
        }
        Err(problem) => failure(&frame.id, code(problem)),
    }
}
/// Cleanup belongs to the blocking attempt, so dropping its response future does
/// not leave an admitted change's old GitHub cache awaiting another request.
async fn change_passphrase(
    state: &ProductRouteState,
    routes: Arc<VaultRoutes>,
    lease: Lease,
    mut frame: RequestFrame,
) -> OutgoingMessage {
    let Ok(params) = serde_json::from_value::<VaultPassphraseParams>(std::mem::replace(
        &mut frame.params,
        json!({}),
    )) else {
        return failure(&frame.id, "invalid_request");
    };
    let passphrase = match Passphrase::new(params.passphrase) {
        Ok(value) => value,
        Err(problem) => return failure(&frame.id, code(problem)),
    };
    let github = state.github.clone();
    let result = tokio::task::spawn_blocking(move || {
        let changed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            routes.vault.change_passphrase(&lease, passphrase)
        }));
        let cleanup_failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            github
                .as_ref()
                .is_some_and(|github| github.connections.clear_locked().is_err())
        }))
        .unwrap_or(true);
        match changed {
            Ok(Ok(receipt)) => change_result(&receipt, cleanup_failed),
            Ok(Err(problem)) => Err(code(problem)),
            Err(_) => Err(if cleanup_failed {
                "vault_change_not_confirmed_audit_unavailable"
            } else {
                "vault_change_not_confirmed"
            }),
        }
    })
    .await
    .unwrap_or(Err("vault_change_not_confirmed"));
    match result {
        Ok(()) => success(
            &frame.id,
            &VaultPassphraseChangeStatus {
                state: VaultPassphraseChangeState::Locked,
            },
        ),
        Err(code) => failure(&frame.id, code),
    }
}
fn change_result(receipt: &ChangeReceipt, cleanup_failed: bool) -> Result<(), &'static str> {
    match (
        receipt.publication(),
        receipt.audit_failed() || cleanup_failed,
    ) {
        (ChangePublication::Committed, false) => Ok(()),
        (ChangePublication::Committed, true) => Err("vault_changed_audit_unavailable"),
        (ChangePublication::Unchanged(_), false) => Err("vault_change_unchanged"),
        (ChangePublication::Unchanged(_), true) => Err("vault_change_unchanged_audit_unavailable"),
        (ChangePublication::Uncertain(_), false) => Err("vault_change_not_confirmed"),
        (ChangePublication::Uncertain(_), true) => {
            Err("vault_change_not_confirmed_audit_unavailable")
        }
    }
}
#[derive(Clone, Copy)]
enum Operation {
    Status,
    Setup,
    Unlock,
    Lock,
}
fn run(
    routes: &VaultRoutes,
    lease: &Lease,
    operation: Operation,
    passphrase: Option<Passphrase>,
) -> Result<Status, Failure> {
    match operation {
        Operation::Status => routes.vault.status(),
        Operation::Lock => routes.vault.lock(lease),
        Operation::Setup | Operation::Unlock => {
            let passphrase = passphrase.ok_or(Failure::PassphrasePolicy)?;
            let snapshot = routes.migration.capture()?;
            if matches!(operation, Operation::Setup)
                && snapshot
                    .required()
                    .iter()
                    .any(|name| !snapshot.entries().iter().any(|(entry, _)| entry == name))
            {
                return Err(Failure::MigrationUnavailable);
            }
            let result = if matches!(operation, Operation::Setup) {
                routes.vault.create(lease, passphrase, snapshot.entries())
            } else {
                routes.vault.unlock(lease, passphrase)
            }?;
            if let Err(problem) = verify_and_retire(&routes.vault, snapshot.as_ref()) {
                let _ = routes.vault.lock(lease);
                return Err(problem);
            }
            Ok(result)
        }
    }
}
pub(crate) fn code(problem: Failure) -> &'static str {
    match problem {
        Failure::Locked => "vault_locked",
        Failure::PassphrasePolicy => "vault_passphrase_policy",
        Failure::WrongPassphrase => "vault_wrong_passphrase",
        Failure::Busy => "vault_busy",
        Failure::Cooldown => "vault_cooldown",
        Failure::SessionClosed => "vault_session_closed",
        Failure::SessionOwned => "vault_session_owned",
        Failure::AlreadyConfigured => "vault_already_configured",
        Failure::Capacity => "vault_capacity",
        Failure::AuditUnavailable => "vault_audit_unavailable",
        Failure::MigrationUnavailable => "vault_migration_unavailable",
        Failure::InvalidEntry | Failure::Unavailable => "vault_unavailable",
    }
}
