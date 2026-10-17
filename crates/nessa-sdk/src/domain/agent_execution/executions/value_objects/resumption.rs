//! Which turn continues one that a restart or the host's own shutdown cut off.
#![deny(missing_docs)]

use super::ExecutionId;
use sha2::{Digest, Sha256};

/// One cut-off turn, or a successor that continues it, in its chain of resumes.
///
/// A successor's identity is derived from the turn it continues: the first
/// successor of `id` is `resume-1-<sha256(id)>`, and the successor of
/// `resume-n-<hash>` is `resume-(n+1)-<hash>`. Deriving it, rather than
/// minting one, makes resuming idempotent: a second resume of the same turn,
/// after a crash between recording the cut-off and saving its successor or
/// from a racing opener, names the same successor and is refused as a
/// duplicate instead of running the work twice
/// (`resume_twice_queues_one_successor`). It also lets a retry of the original
/// message find the turn that continued it.
///
/// A chain ends after [`Self::MAX_RESUMES`] successors: a turn cut off that
/// many times running is left ended, so work that brings the gateway down
/// as it runs cannot do so on every start
/// (`a_turn_cut_off_on_every_start_is_resumed_at_most_three_times`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resumption {
    root: String,
    attempt: u32,
}

impl Resumption {
    /// Most successors one original turn gets.
    pub const MAX_RESUMES: u32 = 3;
    const PREFIX: &'static str = "resume-";

    /// Where `id` stands in its chain: a successor's own attempt, or the
    /// original turn's attempt zero.
    pub fn of(id: &ExecutionId) -> Self {
        Self::parse(id.as_str()).unwrap_or_else(|| Self {
            root: hex(&Sha256::digest(id.as_str().as_bytes())),
            attempt: 0,
        })
    }

    /// How many successors precede this turn in its chain; zero for the
    /// turn a person or app submitted.
    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    /// The identity of the turn that continues this one, or `None` once the
    /// chain has had its [`Self::MAX_RESUMES`] successors.
    pub fn successor(&self) -> Option<ExecutionId> {
        let attempt = self.attempt.saturating_add(1);
        (attempt <= Self::MAX_RESUMES).then(|| {
            ExecutionId::new(format!("{}{attempt}-{}", Self::PREFIX, self.root))
                .expect("a derived successor identity is short and nonblank")
        })
    }

    fn parse(value: &str) -> Option<Self> {
        let (digits, root) = value.strip_prefix(Self::PREFIX)?.split_once('-')?;
        let attempt: u32 = digits.parse().ok()?;
        let canonical = attempt.to_string() == digits
            && (1..=Self::MAX_RESUMES).contains(&attempt)
            && root.len() == 64
            && root
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        canonical.then(|| Self {
            root: root.to_owned(),
            attempt,
        })
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
#[path = "../../../../../tests/domain/agent_execution/resumption.rs"]
mod tests;
