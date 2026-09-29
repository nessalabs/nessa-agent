//! The reasoning effort levels a provider publishes for one model, kept in the
//! provider's own names and order.
#![deny(missing_docs)]

use super::super::MetadataError;

fn invalid(reason: &'static str) -> MetadataError {
    MetadataError::Invalid {
        field: "reasoning effort",
        reason,
    }
}

/// One reasoning effort level, named as its provider names it: `low`, `xhigh`,
/// `max`. Levels of different providers are never equated: `high` for one
/// provider says nothing about `high` for another.
///
/// The name is the value a provider's API or harness takes, so it is held to
/// the alphabet those values are written in rather than checked against a list
/// of known names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffortLevel(String);
impl EffortLevel {
    /// Longest level name accepted, in bytes.
    pub const MAX_LEN: usize = 32;

    /// `name`: a lowercase ASCII letter, then lowercase ASCII letters, digits,
    /// `-` or `_`, at most [`Self::MAX_LEN`] bytes in all.
    ///
    /// # Errors
    ///
    /// [`MetadataError::Invalid`] naming `reasoning effort` when `name` is
    /// empty, too long, or outside that alphabet.
    pub fn new(name: String) -> Result<Self, MetadataError> {
        if name.is_empty() || name.len() > Self::MAX_LEN {
            return Err(invalid("a level name must be 1 to 32 bytes"));
        }
        let mut bytes = name.bytes();
        let starts_with_letter = bytes.next().is_some_and(|byte| byte.is_ascii_lowercase());
        let rest_allowed = bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        });
        if !starts_with_letter || !rest_allowed {
            return Err(invalid(
                "a level name is a lowercase letter, then lowercase letters, digits, - or _",
            ));
        }
        Ok(Self(name))
    }
    /// The provider's name for this level.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The effort levels a model is published to accept, least effort first, as
/// its provider lists them.
///
/// A model that reasons but has none recorded holds no `EffortLevels` at all
/// ([`crate::domain::model_metadata::entities::ModelMetadata::effort_levels`]
/// is `None`), so a value of this type always offers at least one level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffortLevels(Vec<EffortLevel>);
impl EffortLevels {
    /// `levels`: least effort first, in the provider's order; at least one,
    /// none repeated. The order is the provider's and is kept as given: names
    /// alone cannot say which of two levels is more.
    ///
    /// # Errors
    ///
    /// [`MetadataError::Invalid`] naming `reasoning effort` when `levels` is
    /// empty or names a level twice.
    pub fn new(levels: Vec<EffortLevel>) -> Result<Self, MetadataError> {
        if levels.is_empty() {
            return Err(invalid("must list at least one level"));
        }
        if (1..levels.len()).any(|index| levels[..index].contains(&levels[index])) {
            return Err(invalid("levels must not repeat"));
        }
        Ok(Self(levels))
    }
    /// The levels, least effort first.
    pub fn levels(&self) -> &[EffortLevel] {
        &self.0
    }
}
