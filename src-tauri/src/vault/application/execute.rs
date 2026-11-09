use super::{Command, Failure, VaultConnection};
use std::{sync::Arc, time::Duration};
/// A deadline ends this request's native connection, not merely its waiting future.
pub async fn execute(
    vault: Arc<dyn VaultConnection>,
    request: Command,
    deadline: Duration,
) -> Result<serde_json::Value, Failure> {
    let unconfirmed = request.unconfirmed_failure();
    let request_id = vault.next_request();
    let pending = vault.clone();
    match tokio::time::timeout(
        deadline,
        tokio::task::spawn_blocking(move || pending.command(request, request_id)),
    )
    .await
    {
        Ok(Ok(result)) => result,
        Ok(Err(_)) | Err(_) => {
            vault.interrupt(request_id);
            Err(unconfirmed)
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/vault/deadline.rs"]
mod tests;
