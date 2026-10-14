//! One attach, named — and the name is a type rather than a convention.
//!
//! ```text
//!   `+` pressed ──▶ panel UUID ──▶ Batch::picked ──┐
//!                                                  ├──▶ Announce::began ──▶
//!   drop landed ──▶ Batch::next ───────────────────┘                       panel
//!                            "{batch}:{index}" for each waiting file ──────▶
//! ```
//!
//! Arrows are "produces". Everything downstream of a gesture carries the name
//! the gesture was given, so an answer arriving three quarters of a minute
//! later still knows which draft it belongs to rather than reading whatever tab
//! happens to be open.
//!
//! # Why this is a type and not a `String`
//!
//! The name has one invariant — it is never empty — and an empty one is not
//! inert: the panel ignores an unknown name instead of guessing from the open tab,
//! which is exactly the guess the name exists to remove. Two of five `Dropped`
//! answers once shipped an empty one, both through a `..Default::default()`
//! that had no name to give, and both were the slowest paths and so the ones
//! most likely to land after the tab had changed.
//!
//! Losing that `Default` was a speed bump rather than enforcement: every field
//! of `Dropped` is public, so a struct literal with `String::new()` still
//! compiled. The field here is private and this module mints the only values,
//! so no caller can write one down. Drops get a host-minted value; a picked
//! gesture gets the already registered, validated request identity.
//!
//! What no type can check is that a caller passes the *right* picker identity.
//! The UI registers it before the command; the host validates its format and
//! carries it unchanged through every event.
use std::fmt::{self, Display, Formatter};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// A picker identity the panel made before asking the host to open a dialog.
///
/// The prefix keeps it disjoint from host-minted drop batches. Its UUID is
/// validated at the command boundary before the picker or any file port runs.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub(crate) struct PickerRequestId(String);

#[derive(Debug)]
pub(crate) struct InvalidPickerRequestId;

impl std::fmt::Display for InvalidPickerRequestId {
    fn fmt(&self, out: &mut Formatter<'_>) -> fmt::Result {
        out.write_str("picker request ID must be p- followed by a canonical UUID")
    }
}

impl std::error::Error for InvalidPickerRequestId {}

impl TryFrom<String> for PickerRequestId {
    type Error = InvalidPickerRequestId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let uuid = value.strip_prefix("p-").ok_or(InvalidPickerRequestId)?;
        if !canonical_uuid(uuid) {
            return Err(InvalidPickerRequestId);
        }
        Ok(Self(value))
    }
}

fn canonical_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')
            }
        })
}

/// What one attach is called, from the gesture that began it to the answer.
///
/// Opaque to everyone but the panel, which uses it as a key. It is never shown
/// to anybody and never becomes part of a path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub(super) struct Batch(String);

impl Batch {
    /// Mint the next name for a host drop.
    ///
    /// A counter rather than a random token: it only has to be unique within
    /// the run of one host talking to one page, and a number that cannot repeat
    /// is easier to read in a log than a token that merely should not. The `b`
    /// namespace cannot collide with the panel's `p-` picker identities.
    pub(super) fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(format!("b{}", NEXT.fetch_add(1, Ordering::Relaxed)))
    }

    /// Keep the panel's validated picker ID as the identity for its events.
    pub(super) fn picked(request_id: PickerRequestId) -> Self {
        Self(request_id.0)
    }
}

impl Display for Batch {
    fn fmt(&self, out: &mut Formatter<'_>) -> fmt::Result {
        out.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_request_ids_validate_command_arguments_before_any_picker_call() {
        let request_id: PickerRequestId =
            serde_json::from_str(r#""p-01234567-89ab-cdef-0123-456789abcdef""#)
                .expect("canonical picker request ID");
        let batch = Batch::picked(request_id);
        assert_eq!(batch.to_string(), "p-01234567-89ab-cdef-0123-456789abcdef");
        assert!(serde_json::from_str::<PickerRequestId>(r#""b1""#).is_err());
        assert!(serde_json::from_str::<PickerRequestId>(
            r#""p-01234567-89AB-cdef-0123-456789abcdef""#
        )
        .is_err());
        assert!(serde_json::from_str::<PickerRequestId>(
            r#""p-01234567-89ab-cdef-0123-456789abcdef-extra""#
        )
        .is_err());
    }

    /// Two gestures never share a name, whichever order they happen in.
    ///
    /// A shared name would put one gesture's files on the other's draft, which
    /// is the whole failure this exists to prevent. Host drops use this counter;
    /// picked batches carry independently generated, format-checked UUIDs.
    #[test]
    fn every_minted_name_is_its_own_and_none_is_empty() {
        let minted: Vec<Batch> = (0..64).map(|_| Batch::next()).collect();

        for (index, batch) in minted.iter().enumerate() {
            assert!(!batch.to_string().is_empty());
            assert!(
                !minted[index + 1..].contains(batch),
                "{batch} was minted twice"
            );
        }
    }

    /// It crosses the seam as the bare string the panel keys on, not as an
    /// object wrapping one. The panel splits `"{batch}:{index}"` on `:`, so a
    /// name that arrived quoted inside a struct would not match anything.
    #[test]
    fn a_name_is_written_down_as_the_text_the_panel_reads() {
        let batch = Batch::next();

        assert_eq!(
            serde_json::to_string(&batch).expect("a name serializes"),
            format!("\"{batch}\"")
        );
    }
}
