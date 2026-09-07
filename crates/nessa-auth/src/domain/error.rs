//! Structural validation failures. These errors describe metadata, never proof bytes.
use std::error::Error;
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
/// Failure to construct or update a valid domain value.
pub enum DomainError {
    InvalidValue {
        field: &'static str,
        reason: &'static str,
    },
    InvalidCredentialLifetime {
        issued_at: u64,
        expires_at: u64,
    },
    GrantOrganizationMismatch,
    RevokedBeforeIssued {
        issued_at: u64,
        revoked_at: u64,
    },
}

impl fmt::Display for DomainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidValue { field, reason } => {
                write!(formatter, "invalid {field}: {reason}")
            }
            Self::InvalidCredentialLifetime {
                issued_at,
                expires_at,
            } => write!(
                formatter,
                "credential expiration {expires_at} must be after issuance {issued_at}"
            ),
            Self::GrantOrganizationMismatch => {
                formatter.write_str("credential grant belongs to another organization")
            }
            Self::RevokedBeforeIssued {
                issued_at,
                revoked_at,
            } => write!(
                formatter,
                "credential revocation {revoked_at} precedes issuance {issued_at}"
            ),
        }
    }
}

impl Error for DomainError {}
