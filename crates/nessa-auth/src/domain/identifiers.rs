//! Opaque identifiers and action names shared by the domain.
//! Values are preserved exactly: no trimming, case folding, or provider parsing.
//! Validation rejects empty text, surrounding whitespace, control characters, and
//! values over 256 bytes. Existence and uniqueness belong to the owning store.
//! A validated value is stored compactly, so what an identifier costs to keep is
//! the text it carries and not the buffer it happened to arrive in.
use super::DomainError;

const MAX_VALUE_LENGTH: usize = 256;

fn validate(value: String, field: &'static str) -> Result<Box<str>, DomainError> {
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
    // A length check passes on a short value inside a large allocation; the
    // allocation is what gets retained. Shrink to the text before keeping it.
    Ok(value.into_boxed_str())
}

macro_rules! string_value {
    ($name:ident, $field:literal) => {
        #[doc = concat!("Validated opaque ", $field, ". Equality compares the original text exactly.")]
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(Box<str>);

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

    /// The retained allocation, not the text length — `size_of_val` on the
    /// borrowed `str` reports the same number either way and would pass against
    /// a `String` field too.
    ///
    /// This pins the storage type rather than a runtime behaviour: taking
    /// `Box<str>` is what a `String`-backed field would fail to satisfy, and it
    /// would fail to compile rather than fail an assertion. `String::from` on a
    /// `Box<str>` reuses the boxed allocation, so the capacity it reports is the
    /// bytes actually held.
    fn retained_bytes(value: Box<str>) -> usize {
        String::from(value).capacity()
    }

    #[test]
    fn retains_only_the_validated_text_not_the_buffer_it_arrived_in() {
        let mut oversized = String::with_capacity(64 * 1024);
        oversized.push_str("principal-1");
        assert!(oversized.capacity() >= 64 * 1024);
        let identifier = PrincipalId::new(oversized).unwrap();
        assert_eq!(identifier.as_str(), "principal-1");
        assert_eq!(retained_bytes(identifier.0), "principal-1".len());
    }

    #[test]
    fn bounds_are_utf8_bytes_rather_than_characters() {
        // Multibyte text reaches the limit in fewer characters than bytes.
        let exact = "é".repeat(MAX_VALUE_LENGTH / 2);
        assert_eq!(exact.len(), MAX_VALUE_LENGTH);
        // Retention is the other test's subject; `repeat` already returns an
        // exactly sized buffer, so asserting it here would prove nothing.
        let identifier = CredentialId::new(exact.clone()).unwrap();
        assert_eq!(identifier.as_str().len(), MAX_VALUE_LENGTH);
        assert!(CredentialId::new(exact + "é").is_err());
        assert!(CredentialId::new("a".repeat(MAX_VALUE_LENGTH)).is_ok());
        assert!(CredentialId::new("a".repeat(MAX_VALUE_LENGTH + 1)).is_err());
    }
}
