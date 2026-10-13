//! Public credential facts. The registry file's verifier is never copied
//! into a record.

use nessa_auth::application::dto::CredentialMetadataDto;
use serde_json::Value;

use crate::backup::domain::{CredentialKind, Fingerprint};

/// One credential the cut may keep. `record` is public metadata only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicCredential {
    pub id: String,
    pub principal_id: String,
    pub revoked: bool,
    pub kind: CredentialKind,
    pub fingerprint: Fingerprint,
    pub record: Vec<u8>,
}

/// Credentials read from one registry file, plus rows that could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialRead {
    pub credentials: Vec<PublicCredential>,
    pub skipped: Vec<String>,
}

/// `bytes` is a credential registry file. A verifier field on a row is
/// ignored. The whole file failing to parse is [`Err`].
pub fn read_credentials(bytes: &[u8]) -> Result<CredentialRead, ()> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| ())?;
    let Some(rows) = value.get("credentials").and_then(Value::as_array) else {
        return Err(());
    };
    let mut credentials = Vec::new();
    let mut skipped = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let identity = format!("row-{}", index + 1);
        let Some(metadata) = row.get("metadata") else {
            skipped.push(identity);
            continue;
        };
        let Ok(metadata) = serde_json::from_value::<CredentialMetadataDto>(metadata.clone()) else {
            skipped.push(identity);
            continue;
        };
        if !nameable(&metadata.id) {
            skipped.push(identity);
            continue;
        }
        let actions: Vec<String> = metadata
            .grants
            .iter()
            .map(|grant| grant.action.clone())
            .collect();
        let kind = CredentialKind::from_registry(&metadata.principal_id, &actions);
        let fingerprint = Fingerprint::of(canonical(&metadata, kind).as_bytes());
        let record = serde_json::to_vec(&serde_json::json!({
            "id": metadata.id,
            "principalId": metadata.principal_id,
            "revoked": metadata.revoked_at.is_some(),
            "kind": kind.as_str(),
            "fingerprint": fingerprint.hex(),
        }))
        .map_err(|_| ())?;
        credentials.push(PublicCredential {
            id: metadata.id,
            principal_id: metadata.principal_id,
            revoked: metadata.revoked_at.is_some(),
            kind,
            fingerprint,
            record,
        });
    }
    Ok(CredentialRead {
        credentials,
        skipped,
    })
}

fn canonical(metadata: &CredentialMetadataDto, kind: CredentialKind) -> String {
    let mut actions: Vec<&str> = metadata
        .grants
        .iter()
        .map(|grant| grant.action.as_str())
        .collect();
    actions.sort_unstable();
    format!(
        "{}\n{}\n{}\n{}\n{}",
        metadata.id,
        metadata.principal_id,
        metadata
            .revoked_at
            .map(|at| at.to_string())
            .unwrap_or_else(|| "active".to_owned()),
        kind.as_str(),
        actions.join(",")
    )
}

fn nameable(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && value.bytes().all(|byte| byte >= 0x20)
}
