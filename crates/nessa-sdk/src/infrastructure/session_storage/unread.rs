//! One placeholder per dropped save group, and the physical span a cache skips.
//!
//! A record this build cannot read drops its whole save group. Later records
//! are still read. A later record that only contradicts that dropped group
//! extends the same placeholder.

use super::{
    gap::{DropLink, DroppedTurns},
    save_group::SaveIdentity,
};
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
    turns: DroppedTurns,
}

#[derive(Clone)]
struct Open {
    identity: Option<SaveIdentity>,
    index: usize,
    /// A run of physical frames that are not a save. The next bad frame extends it.
    frame: bool,
}

/// One dropped save, as the fold records it.
pub(super) struct Placement {
    pub(super) start: u64,
    pub(super) through: u64,
    pub(super) after_invocation: u64,
    pub(super) reason: UnreadableReason,
    pub(super) identity: Option<SaveIdentity>,
    pub(super) link: Option<DropLink>,
    pub(super) turns: DroppedTurns,
    pub(super) own: bool,
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
        !open.frame && super::gap::rest_of(open.identity.as_ref(), identity, kind, ordinal)
    }

    /// The turns the open dropped save named, when one is open.
    pub(super) fn open_turns(&self) -> Option<&DroppedTurns> {
        self.open.as_ref().map(|open| &self.items[open.index].turns)
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
    /// A typed [`DropLink`], or another failure of the open save's identity,
    /// extends the open row. An envelope mismatch (`own`) is its own row.
    /// Returns whether a new row was added.
    pub(super) fn note(&mut self, placed: Placement) -> bool {
        let Placement {
            start,
            through,
            after_invocation,
            reason,
            identity,
            link,
            turns,
            own,
        } = placed;
        if let Some(open) = &self.open {
            let same = identity
                .as_ref()
                .is_some_and(|id| open.identity.as_ref() == Some(id));
            if !own && (link.is_some() || same) {
                let index = open.index;
                let learned = identity.is_some();
                let item = &mut self.items[index];
                item.through = item.through.max(through);
                if item.identity.is_none() {
                    item.identity = identity.clone();
                }
                item.turns.absorb(turns);
                if learned {
                    let identity = item.identity.clone();
                    self.open = Some(Open {
                        identity,
                        index,
                        frame: false,
                    });
                }
                return false;
            }
        }
        let index = self.items.len();
        self.items.push(Item {
            part: UnreadablePart::new(start, after_invocation, reason),
            through: through.max(start),
            identity: identity.clone(),
            turns,
        });
        self.open = Some(Open {
            identity,
            index,
            frame: false,
        });
        true
    }

    /// A physical frame that is not a fact. Consecutive bad frames are one row.
    /// A bad frame while a save is already dropped extends that row.
    pub(super) fn note_frame(&mut self, start: u64, through: u64, after_invocation: u64) -> bool {
        if self.open.is_some() {
            self.extend(through);
            return false;
        }
        let index = self.items.len();
        self.items.push(Item {
            part: UnreadablePart::new(start, after_invocation, UnreadableReason::Unreadable),
            through: through.max(start),
            identity: None,
            turns: DroppedTurns::unknown(),
        });
        self.open = Some(Open {
            identity: None,
            index,
            frame: true,
        });
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
            previous = item.through;
            let frame = item.open && item.identity.is_none();
            tracker.items.push(Item {
                part: UnreadablePart::new(item.start, item.after_invocation, reason),
                through: item.through,
                identity: item.identity.clone(),
                turns: DroppedTurns::unknown(),
            });
            if item.open {
                if tracker.open.is_some() {
                    return Err(());
                }
                tracker.open = Some(Open {
                    identity: item.identity,
                    index,
                    frame,
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
