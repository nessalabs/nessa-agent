//! The reasoning effort levels a provider publishes for one model, kept in the
//! provider's own names and order.
#![deny(missing_docs)]

use super::super::MetadataError;
use std::collections::HashSet;

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
    /// Most levels one model may list. Providers publish a handful (six at
    /// most today); the bound keeps a catalogue read at startup from holding,
    /// or checking, an unbounded list.
    pub const MAX_LEVELS: usize = 16;

    /// `levels`: least effort first, in the provider's order; at least one,
    /// at most [`Self::MAX_LEVELS`], none repeated. The order is the
    /// provider's and is kept as given: names alone cannot say which of two
    /// levels is more.
    ///
    /// # Errors
    ///
    /// [`MetadataError::Invalid`] naming `reasoning effort` when `levels` is
    /// empty, longer than [`Self::MAX_LEVELS`], or names a level twice.
    pub fn new(levels: Vec<EffortLevel>) -> Result<Self, MetadataError> {
        if levels.is_empty() {
            return Err(invalid("must list at least one level"));
        }
        if levels.len() > Self::MAX_LEVELS {
            return Err(invalid("must list at most 16 levels"));
        }
        let mut seen = HashSet::with_capacity(levels.len());
        if !levels.iter().all(|level| seen.insert(level.as_str())) {
            return Err(invalid("levels must not repeat"));
        }
        Ok(Self(levels))
    }
    /// The levels, least effort first.
    pub fn levels(&self) -> &[EffortLevel] {
        &self.0
    }
    /// Whether `level` is one of these.
    pub fn contains(&self, level: &EffortLevel) -> bool {
        self.0.contains(level)
    }
    /// Which of these levels an agent also offers, given the names it
    /// `advertised` at runtime.
    ///
    /// The catalogue is the ceiling: a level is offered only when it is listed
    /// here and advertised under exactly the same name. An advertised name
    /// these levels do not list (a harness's own `default`, a level no provider
    /// publishes) is ignored, so the result never widens what the catalogue
    /// records. Order is not read from `advertised`: see
    /// [`Self::restricted_to`].
    pub fn offered_by<'a>(
        &self,
        advertised: impl IntoIterator<Item = &'a str>,
    ) -> OfferedEffortLevels {
        let advertised: HashSet<&str> = advertised.into_iter().collect();
        let positions = self
            .0
            .iter()
            .enumerate()
            .filter(|(_, level)| advertised.contains(level.as_str()))
            .fold(0_u16, |positions, (index, _)| positions | 1 << index);
        OfferedEffortLevels(positions)
    }
    /// The levels `offered` names, least effort first in this list's own
    /// order; `None` when it names none of them.
    ///
    /// `offered` must come from [`Self::offered_by`] on these same levels. A
    /// position past the end of this list names nothing and is ignored.
    pub fn restricted_to(&self, offered: OfferedEffortLevels) -> Option<EffortLevels> {
        let levels: Vec<EffortLevel> = self
            .0
            .iter()
            .enumerate()
            .filter(|(index, _)| offered.0 & (1 << index) != 0)
            .map(|(_, level)| level.clone())
            .collect();
        (!levels.is_empty()).then_some(Self(levels))
    }
}

// Offered levels are positions in one list, held as the bits of a u16.
const _: () = assert!(EffortLevels::MAX_LEVELS <= u16::BITS as usize);

/// Which of one model's catalogue effort levels its connected agent also
/// offers: positions in that model's [`EffortLevels`], never names of its own.
///
/// A set of positions cannot hold a level the catalogue does not list, so what
/// an agent advertises can narrow the catalogue's levels and never widen them.
/// It is `Copy` so it can travel in the provider's negotiated operation facts;
/// [`EffortLevels::restricted_to`] reads it back as levels. The default offers
/// none, which is also what an agent with no effort option negotiates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OfferedEffortLevels(u16);
impl OfferedEffortLevels {
    /// No level offered, usable where [`Default`] is not (a `const`).
    pub const NONE: Self = Self(0);
    /// Whether no level is offered.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}
