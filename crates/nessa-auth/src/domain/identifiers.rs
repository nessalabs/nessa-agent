//! Opaque identifiers and action names shared by the domain.
//! Values are preserved exactly: no trimming, case folding, or provider parsing.
//! Validation rejects empty text, surrounding whitespace, control characters, and
//! values over 256 bytes. Existence and uniqueness belong to the owning store.
use super::DomainError;

const MAX_VALUE_LENGTH: usize = 256;

fn validate(value: String, field: &'static str) -> Result<String, DomainError> {
    if value.is_empty() {
        return Err(DomainError::InvalidValue {
            field,
            reason: "must not be empty",
        });
    }
    if value.trim() != value {
        return Err(DomainError::InvalidValue {
            field,
            reason: "must not have surrounding whitespace",
        });
    }
    if value.chars().any(char::is_control) {
        return Err(DomainError::InvalidValue {
            field,
            reason: "must not contain control characters",
        });
    }
    if value.len() > MAX_VALUE_LENGTH {
        return Err(DomainError::InvalidValue {
            field,
            reason: "is too long",
        });
    }
    Ok(value)
}

macro_rules! string_value {
    ($name:ident, $field:literal) => {
        #[doc = concat!("Validated opaque ", $field, ". Equality compares the original text exactly.")]
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            /// Validate `value` without normalizing it.
            ///
            /// Returns a domain error for empty, oversized, or ambiguous text.
            pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
                validate(value.into(), $field).map(Self)
            }

            /// Borrow the validated original text; no provider formatting is applied.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

string_value!(PrincipalId, "principal id");
string_value!(OrganizationId, "organization id");
string_value!(MembershipId, "membership id");
string_value!(CredentialId, "credential id");
string_value!(AudienceId, "audience id");
string_value!(Action, "action");
string_value!(ResourceId, "resource id");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_ambiguous_identifier_text() {
        assert!(PrincipalId::new("").is_err());
        assert!(OrganizationId::new(" org-1").is_err());
        assert!(Action::new("read\nwrite").is_err());
    }

    #[test]
    fn preserves_valid_values_exactly() {
        let resource = ResourceId::new("projects/project-1").unwrap();
        assert_eq!(resource.as_str(), "projects/project-1");
    }
}
