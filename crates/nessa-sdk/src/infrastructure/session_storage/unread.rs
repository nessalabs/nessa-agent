//! One placeholder per dropped save group, and the physical span a cache skips.
//!
//! A record this build cannot read drops its whole save group. Later records
//! are still read. A later record that only contradicts that dropped group
//! extends the same placeholder.

use super::save_group::SaveIdentity;
use crate::application::agent_execution::sessions::{
    records::FactKind, UnreadablePart, UnreadableReason,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SavedUnread {
    pub(super) start: u64,
    pub(super) through: u64,
    pub(super) after_invocation: u64,
    pub(super) reason: String,
    pub(super) found: Option<u64>,
    pub(super) identity: Option<SaveIdentity>,
    pub(super) open: bool,
}

#[derive(Clone)]
struct Item {
    part: UnreadablePart,
    through: u64,
    identity: Option<SaveIdentity>,
}

#[derive(Clone)]
struct Open {
    identity: SaveIdentity,
    index: usize,
}

/// Placeholders produced while reading one stream, oldest first.
#[derive(Clone, Default)]
pub(super) struct UnreadTracker {
    items: Vec<Item>,
    open: Option<Open>,
}

impl UnreadTracker {
    pub(super) fn parts(&self) -> Vec<UnreadablePart> {
        self.items.iter().map(|item| item.part.clone()).collect()
    }

    pub(super) fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// The rest of the open dropped group: a later unit or its completion.
    /// An ordinal-zero unit of that same identity is a new attempt, not the rest.
    pub(super) fn consume_rest(
        &self,
        identity: &SaveIdentity,
        kind: FactKind,
        ordinal: u64,
    ) -> bool {
        let Some(open) = &self.open else {
            return false;
        };
        &open.identity == identity && !(kind == FactKind::SaveUnit && ordinal == 0)
    }

    pub(super) fn extend(&mut self, through: u64) {
        if let Some(open) = &self.open {
            let item = &mut self.items[open.index];
            item.through = item.through.max(through);
        }
    }

    /// A successful save ends absorption. The next failure is its own row.
    pub(super) fn close(&mut self) {
        self.open = None;
    }

    /// Record `start`..=`through` as unreadable.
    ///
    /// A contradiction of an open group, or another failure of that group's
    /// identity, extends the open row. Anything else starts a new one.
    /// Returns whether a new row was added.
    pub(super) fn note(
        &mut self,
        start: u64,
        through: u64,
        after_invocation: u64,
        reason: UnreadableReason,
        identity: Option<SaveIdentity>,
        contradiction: bool,
    ) -> bool {
        if let Some(open) = &self.open {
            let same = identity.as_ref().is_some_and(|id| id == &open.identity);
            if contradiction || same {
                let item = &mut self.items[open.index];
                item.through = item.through.max(through);
                if item.identity.is_none() {
                    item.identity = identity;
                }
                return false;
            }
        }
        let index = self.items.len();
        self.items.push(Item {
            part: UnreadablePart::new(start, after_invocation, reason),
            through: through.max(start),
            identity: identity.clone(),
        });
        self.open = identity.map(|identity| Open { identity, index });
        true
    }

    pub(super) fn retains(&self, position: u64) -> bool {
        !self
            .items
            .iter()
            .any(|item| position >= item.part.position() && position <= item.through)
    }

    /// End of the span that contains `position`, when the cache must jump it.
    pub(super) fn through_at(&self, position: u64) -> Option<u64> {
        self.items.iter().find_map(|item| {
            (position >= item.part.position() && position <= item.through).then_some(item.through)
        })
    }

    pub(super) fn spans(&self) -> Vec<(u64, u64)> {
        self.items
            .iter()
            .map(|item| (item.part.position(), item.through))
            .collect()
    }

    pub(super) fn saved(&self) -> Vec<SavedUnread> {
        self.items
            .iter()
            .enumerate()
            .map(|(index, item)| SavedUnread {
                start: item.part.position(),
                through: item.through,
                after_invocation: item.part.after_invocation(),
                reason: item.part.reason().name().to_owned(),
                found: item.part.reason().found(),
                identity: item.identity.clone(),
                open: self.open.as_ref().is_some_and(|open| open.index == index),
            })
            .collect()
    }

    pub(super) fn restore(saved: Vec<SavedUnread>) -> Result<Self, ()> {
        let mut tracker = Self::default();
        let mut previous = 0u64;
        for (index, item) in saved.into_iter().enumerate() {
            if item.start == 0 || item.through < item.start || item.start <= previous {
                return Err(());
            }
            let reason = match item.reason.as_str() {
                "another_version" => UnreadableReason::AnotherVersion { found: item.found },
                "identity" if item.found.is_none() => UnreadableReason::Identity,
                "unreadable" if item.found.is_none() => UnreadableReason::Unreadable,
                _ => return Err(()),
            };
            if item.open && item.identity.is_none() {
                return Err(());
            }
            previous = item.through;
            tracker.items.push(Item {
                part: UnreadablePart::new(item.start, item.after_invocation, reason),
                through: item.through,
                identity: item.identity.clone(),
            });
            if item.open {
                if tracker.open.is_some() {
                    return Err(());
                }
                tracker.open = Some(Open {
                    identity: item.identity.expect("open identity checked"),
                    index,
                });
            }
        }
        if tracker
            .open
            .as_ref()
            .is_some_and(|open| open.index + 1 != tracker.items.len())
        {
            return Err(());
        }
        Ok(tracker)
    }
}
