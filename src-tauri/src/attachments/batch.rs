//! One attach, named — and the name is a type rather than a convention.
//!
//! ```text
//!   `+` pressed ──┐
//!                 ├──▶ Batch::next() ──▶ Announce::began ──▶ the panel binds
//!   drop landed ──┘           │                               it to a draft
//!                             │
//!                             └──▶ "{batch}:{index}" for each waiting file,
//!                                  and the `Dropped` the panel is handed
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
//! inert: the panel resolves an unknown name by falling back to the open tab,
//! which is exactly the guess the name exists to remove. Two of five `Dropped`
//! answers once shipped an empty one, both through a `..Default::default()`
//! that had no name to give, and both were the slowest paths and so the ones
//! most likely to land after the tab had changed.
//!
//! Losing that `Default` was a speed bump rather than enforcement: every field
//! of `Dropped` is public, so a struct literal with `String::new()` still
//! compiled. The field here is private and this module mints the only values,
//! so no caller can write one down — a sixth answer must be handed a `Batch`
//! that already exists.
//!
//! What no type can check is that a caller passes the *right* name rather than
//! a freshly minted one. That is what `dropping`'s enumeration test is for, and
//! the two are doing different jobs on purpose.
use std::fmt::{self, Display, Formatter};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

/// What one attach is called, from the gesture that began it to the answer.
///
/// Opaque to everyone but the panel, which uses it as a key. It is never shown
/// to anybody and never becomes part of a path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub(super) struct Batch(String);

impl Batch {
    /// Mint the next one. The only way to obtain a `Batch`.
    ///
    /// A counter rather than a random token: it only has to be unique within
    /// the run of one host talking to one page, and a number that cannot repeat
    /// is easier to read in a log than a token that merely should not. One
    /// counter for both gestures, so a drop and a `+` can never collide.
    pub(super) fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(format!("b{}", NEXT.fetch_add(1, Ordering::Relaxed)))
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

    /// Two gestures never share a name, whichever order they happen in.
    ///
    /// A shared name would put one gesture's files on the other's draft, which
    /// is the whole failure this exists to prevent — and the counter is the
    /// only thing standing between them, since both gestures mint from here.
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
