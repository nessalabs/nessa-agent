//! Persistent cloud gateway VM domain types and durable receipt history
//! (ADR 1417, #1420).
//!
//! A cloud box is a long-lived remote VM hosting a full headless gateway
//! (`nessa-server`) with persistent workspace storage, channels, threads, and
//! native terminals. Its lifecycle follows ADR 1160: intent is persisted before
//! provider creation, every state transition is append-only and validated, and
//! secret enrollment credentials are never written to receipt history.

use super::{Admitted, Cloud};
use std::fmt;
use zeroize::Zeroizing;

/// Maximum character length of a cloud box display name.
pub const MAX_CLOUD_BOX_NAME_CHARS: usize = 64;

/// Typed failures for persistent cloud box orchestration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloudBoxError {
    InvalidName,
    InvalidId,
    InvalidUrl,
    AlreadyExists,
    NotFound,
    NotActive,
    BoxRefused,
    ActiveWorkInFlight,
    AccountDisconnected,
    VaultLocked,
    AdmissionRefused(String),
    ProviderFailed(String),
    BootstrapFailed(String),
    ReceiptUnavailable(String),
    CleanupUncertain(String),
    /// Both a lifecycle operation (or refusal/cleanup) and its required audit
    /// delivery failed; preserves both typed causes without silent audit loss.
    AuditDeliveryFailed {
        operation: Box<CloudBoxError>,
        audit: String,
    },
}

impl fmt::Display for CloudBoxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName => f.write_str("cloud box name is empty or exceeds bounds"),
            Self::InvalidId => f.write_str("cloud box id is not a valid UUID"),
            Self::InvalidUrl => f.write_str("cloud box endpoint must be a valid wss:// URL"),
            Self::AlreadyExists => f.write_str("cloud box already exists"),
            Self::NotFound => f.write_str("cloud box was not found"),
            Self::NotActive => f.write_str("cloud box is not active"),
            Self::BoxRefused => f.write_str("cloud box was previously refused and cannot reopen"),
            Self::ActiveWorkInFlight => {
                f.write_str("cloud box has active conversations or terminals and cannot sleep")
            }
            Self::AccountDisconnected => f.write_str("cloud provider account is not connected"),
            Self::VaultLocked => f.write_str("vault is locked; unlock before managing cloud boxes"),
            Self::AdmissionRefused(detail) => write!(f, "cloud box admission refused: {detail}"),
            Self::ProviderFailed(detail) => write!(f, "cloud provider failed: {detail}"),
            Self::BootstrapFailed(detail) => write!(f, "gateway bootstrap failed: {detail}"),
            Self::ReceiptUnavailable(detail) => write!(f, "cloud box receipt error: {detail}"),
            Self::CleanupUncertain(detail) => write!(f, "cloud box cleanup uncertain: {detail}"),
            Self::AuditDeliveryFailed { operation, audit } => {
                write!(f, "{operation}; audit delivery failed: {audit}")
            }
        }
    }
}

impl std::error::Error for CloudBoxError {}

/// Validated UUID identifying a persistent cloud gateway VM.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CloudBoxId(String);

impl CloudBoxId {
    pub fn parse(raw: impl Into<String>) -> Result<Self, CloudBoxError> {
        let value = raw.into();
        let parsed = uuid::Uuid::parse_str(value.trim()).map_err(|_| CloudBoxError::InvalidId)?;
        Ok(Self(parsed.hyphenated().to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CloudBoxId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Validated display label for a persistent cloud gateway VM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudBoxName(String);

impl CloudBoxName {
    pub fn new(raw: impl Into<String>) -> Result<Self, CloudBoxError> {
        let trimmed = raw.into().trim().to_owned();
        if trimmed.is_empty()
            || trimmed.chars().count() > MAX_CLOUD_BOX_NAME_CHARS
            || trimmed.chars().any(char::is_control)
        {
            return Err(CloudBoxError::InvalidName);
        }
        Ok(Self(trimmed))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CloudBoxName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Validated WebSocket endpoint URL for a cloud box gateway (`wss://` for
/// remote hosts, or numeric loopback `ws://127.0.0.1` / `ws://[::1]` for tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudBoxUrl(String);

impl CloudBoxUrl {
    pub fn new(raw: impl Into<String>) -> Result<Self, CloudBoxError> {
        let trimmed = raw.into().trim().to_owned();
        let parsed = url::Url::parse(&trimmed).map_err(|_| CloudBoxError::InvalidUrl)?;
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(CloudBoxError::InvalidUrl);
        }
        match parsed.scheme() {
            "wss" => {
                if parsed.host_str().is_none() {
                    return Err(CloudBoxError::InvalidUrl);
                }
                Ok(Self(parsed.to_string()))
            }
            "ws" => match parsed.host_str() {
                Some("127.0.0.1" | "[::1]" | "::1") => Ok(Self(parsed.to_string())),
                _ => Err(CloudBoxError::InvalidUrl),
            },
            _ => Err(CloudBoxError::InvalidUrl),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CloudBoxUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Observed lifecycle state of a persistent cloud gateway VM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudBoxState {
    Provisioning,
    Running,
    Sleeping,
    Refused,
    Deleted,
}

/// Verified remote gateway reachability endpoint returned on create or wake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudBoxEndpoint {
    pub gateway_id: String,
    pub url: CloudBoxUrl,
}

/// Non-secret summary of a persistent cloud gateway VM exposed to clients.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudBoxSummary {
    pub box_id: CloudBoxId,
    pub name: CloudBoxName,
    pub cloud: Cloud,
    pub vm: String,
    pub gateway_id: String,
    pub url: CloudBoxUrl,
    pub state: CloudBoxState,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

/// Returned once when creating a cloud box so the caller can enroll it into
/// `GatewayRegistry` without persisting the credential in plain receipt files.
#[derive(Debug)]
pub struct CloudBoxEnrollment {
    pub summary: CloudBoxSummary,
    pub credential: Zeroizing<String>,
}

/// Append-only durable receipt history record for one cloud box (ADR 1160).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloudBoxRecord {
    /// Durable intent persisted before any provider VM creation call.
    Provisioning {
        box_id: CloudBoxId,
        name: CloudBoxName,
        cloud: Cloud,
        created_at_ms: u64,
    },
    /// Active (running or sleeping) cloud box with verified admission and endpoint.
    Active {
        box_id: CloudBoxId,
        name: CloudBoxName,
        cloud: Cloud,
        vm: String,
        image: String,
        admission: Admitted,
        gateway_id: String,
        url: CloudBoxUrl,
        state: CloudBoxState,
        created_at_ms: u64,
        updated_at_ms: u64,
    },
    /// Terminal refusal recorded when provisioning or admission failed.
    Refused {
        box_id: CloudBoxId,
        name: CloudBoxName,
        cloud: Cloud,
        reason: String,
        updated_at_ms: u64,
    },
    /// Terminal deletion recorded after explicit VM destruction.
    Deleted {
        box_id: CloudBoxId,
        vm: Option<String>,
        updated_at_ms: u64,
    },
}

impl CloudBoxRecord {
    pub fn box_id(&self) -> &CloudBoxId {
        match self {
            Self::Provisioning { box_id, .. }
            | Self::Active { box_id, .. }
            | Self::Refused { box_id, .. }
            | Self::Deleted { box_id, .. } => box_id,
        }
    }

    /// Validate an append-only receipt history for one cloud box:
    /// - Must be non-empty and start with `Provisioning`.
    /// - Every record must share the same `box_id` and `cloud`.
    /// - Once `Active`, `vm` and `gateway_id` cannot silently relocate.
    /// - `Refused` and `Deleted` are terminal (no records may follow them).
    pub fn validate_history(history: &[Self]) -> Result<(), CloudBoxError> {
        let Some(Self::Provisioning {
            box_id: expected_id,
            cloud: expected_cloud,
            ..
        }) = history.first()
        else {
            return Err(CloudBoxError::ReceiptUnavailable(
                "history must begin with a provisioning intent".into(),
            ));
        };

        let mut pinned_vm: Option<&str> = None;
        let mut pinned_gateway: Option<&str> = None;
        let mut terminal = false;

        for record in history.iter().skip(1) {
            if terminal {
                return Err(CloudBoxError::ReceiptUnavailable(
                    "no record may follow a terminal refused or deleted receipt".into(),
                ));
            }
            if record.box_id() != expected_id {
                return Err(CloudBoxError::ReceiptUnavailable(
                    "receipt history mixes distinct box_ids".into(),
                ));
            }
            match record {
                Self::Provisioning { .. } => {
                    return Err(CloudBoxError::ReceiptUnavailable(
                        "duplicate provisioning intent in history".into(),
                    ));
                }
                Self::Active {
                    cloud,
                    vm,
                    admission,
                    gateway_id,
                    state,
                    ..
                } => {
                    if cloud != expected_cloud {
                        return Err(CloudBoxError::ReceiptUnavailable(
                            "cloud box cannot silently relocate across clouds".into(),
                        ));
                    }
                    let admission_matches = matches!(
                        (cloud, admission),
                        (Cloud::Boat, Admitted::Boat(_)) | (Cloud::Agent37, Admitted::Agent37(_))
                    );
                    if !admission_matches {
                        return Err(CloudBoxError::ReceiptUnavailable(
                            "cloud box admission proof does not match its cloud provider".into(),
                        ));
                    }
                    if vm.trim().is_empty() || gateway_id.trim().is_empty() {
                        return Err(CloudBoxError::ReceiptUnavailable(
                            "active receipt requires non-empty vm and gateway_id".into(),
                        ));
                    }
                    if !matches!(state, CloudBoxState::Running | CloudBoxState::Sleeping) {
                        return Err(CloudBoxError::ReceiptUnavailable(
                            "active receipt must have running or sleeping state".into(),
                        ));
                    }
                    if let Some(prev_vm) = pinned_vm {
                        if prev_vm != vm.as_str() {
                            return Err(CloudBoxError::ReceiptUnavailable(
                                "cloud box VM identifier cannot change after admission".into(),
                            ));
                        }
                    } else {
                        pinned_vm = Some(vm.as_str());
                    }
                    if let Some(prev_gw) = pinned_gateway {
                        if prev_gw != gateway_id.as_str() {
                            return Err(CloudBoxError::ReceiptUnavailable(
                                "cloud box gateway_id cannot change after admission".into(),
                            ));
                        }
                    } else {
                        pinned_gateway = Some(gateway_id.as_str());
                    }
                }
                Self::Refused { cloud, .. } => {
                    if cloud != expected_cloud {
                        return Err(CloudBoxError::ReceiptUnavailable(
                            "refused receipt cloud disagrees with intent".into(),
                        ));
                    }
                    terminal = true;
                }
                Self::Deleted { .. } => {
                    terminal = true;
                }
            }
        }
        Ok(())
    }

    /// Project the latest summary if the box is currently `Active`.
    pub fn as_summary(&self) -> Option<CloudBoxSummary> {
        match self {
            Self::Active {
                box_id,
                name,
                cloud,
                vm,
                gateway_id,
                url,
                state,
                created_at_ms,
                updated_at_ms,
                ..
            } => Some(CloudBoxSummary {
                box_id: box_id.clone(),
                name: name.clone(),
                cloud: *cloud,
                vm: vm.clone(),
                gateway_id: gateway_id.clone(),
                url: url.clone(),
                state: *state,
                created_at_ms: *created_at_ms,
                updated_at_ms: *updated_at_ms,
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspaces::domain::TemplateRelease;

    fn sample_id() -> CloudBoxId {
        CloudBoxId::parse("11111111-2222-3333-4444-555555555555").unwrap()
    }

    #[test]
    fn validates_name_id_and_url_boundaries() {
        assert!(CloudBoxName::new("  Cloud Box  ").is_ok());
        assert_eq!(CloudBoxName::new("   "), Err(CloudBoxError::InvalidName));
        assert_eq!(
            CloudBoxName::new("a".repeat(MAX_CLOUD_BOX_NAME_CHARS + 1)),
            Err(CloudBoxError::InvalidName)
        );
        assert_eq!(
            CloudBoxName::new("bad\nname"),
            Err(CloudBoxError::InvalidName)
        );

        assert!(CloudBoxId::parse("11111111-2222-3333-4444-555555555555").is_ok());
        assert_eq!(
            CloudBoxId::parse("not-a-uuid"),
            Err(CloudBoxError::InvalidId)
        );

        assert!(CloudBoxUrl::new("wss://box.example.com").is_ok());
        assert!(CloudBoxUrl::new("ws://127.0.0.1:18789").is_ok());
        assert!(CloudBoxUrl::new("ws://[::1]:18789").is_ok());
        assert_eq!(
            CloudBoxUrl::new("ws://remote.example.com:18789"),
            Err(CloudBoxError::InvalidUrl)
        );
        assert_eq!(
            CloudBoxUrl::new("wss://user:pass@box.example.com"),
            Err(CloudBoxError::InvalidUrl)
        );
    }

    #[test]
    fn receipt_history_forbids_vm_relocation_and_reopening_after_terminal() {
        let id = sample_id();
        let name = CloudBoxName::new("Cloud Box").unwrap();
        let url1 = CloudBoxUrl::new("wss://box-1.example.com").unwrap();
        let url2 = CloudBoxUrl::new("wss://box-2.example.com").unwrap();
        let admission =
            Admitted::Agent37(TemplateRelease::new("v1".into(), Some(1), None).unwrap());

        let intent = CloudBoxRecord::Provisioning {
            box_id: id.clone(),
            name: name.clone(),
            cloud: Cloud::Agent37,
            created_at_ms: 10,
        };
        let active1 = CloudBoxRecord::Active {
            box_id: id.clone(),
            name: name.clone(),
            cloud: Cloud::Agent37,
            vm: "vm-1".into(),
            image: "img-1".into(),
            admission: admission.clone(),
            gateway_id: "gw-1".into(),
            url: url1,
            state: CloudBoxState::Running,
            created_at_ms: 10,
            updated_at_ms: 20,
        };
        let active2_rotated_url = CloudBoxRecord::Active {
            box_id: id.clone(),
            name: name.clone(),
            cloud: Cloud::Agent37,
            vm: "vm-1".into(),
            image: "img-1".into(),
            admission: admission.clone(),
            gateway_id: "gw-1".into(),
            url: url2.clone(),
            state: CloudBoxState::Sleeping,
            created_at_ms: 10,
            updated_at_ms: 30,
        };
        assert!(CloudBoxRecord::validate_history(&[
            intent.clone(),
            active1.clone(),
            active2_rotated_url.clone()
        ])
        .is_ok());

        // Silent VM relocation is refused.
        let relocated = CloudBoxRecord::Active {
            box_id: id.clone(),
            name,
            cloud: Cloud::Agent37,
            vm: "vm-2".into(),
            image: "img-1".into(),
            admission,
            gateway_id: "gw-1".into(),
            url: url2,
            state: CloudBoxState::Sleeping,
            created_at_ms: 10,
            updated_at_ms: 30,
        };
        assert!(
            CloudBoxRecord::validate_history(&[intent.clone(), active1.clone(), relocated])
                .is_err()
        );

        // Reopening after Deleted is refused.
        let deleted = CloudBoxRecord::Deleted {
            box_id: id,
            vm: Some("vm-1".into()),
            updated_at_ms: 40,
        };
        assert!(
            CloudBoxRecord::validate_history(&[intent, active1, deleted, active2_rotated_url])
                .is_err()
        );
    }
}
