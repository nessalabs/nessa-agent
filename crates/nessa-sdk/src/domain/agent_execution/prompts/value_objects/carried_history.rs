#![deny(missing_docs)]

use crate::domain::agent_execution::{executions::AgentSource, ExecutionError};

/// One earlier turn of a conversation, as a native session that never saw it
/// is given it: what was said, and what the agent said last in reply.
///
/// The input keeps its agent origin when present. Tool steps, images and files stay in the conversation's
/// records and are not carried.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CarriedTurn {
    said: Option<Box<str>>,
    reply: Option<Box<str>>,
    agent_source: Option<AgentSource>,
}
impl CarriedTurn {
    /// Most UTF-8 bytes of either part: what was said, or the reply.
    pub const MAX_PART_BYTES: usize = 8 * 1024;

    /// `agent_source` identifies the agent that wrote the input, when one did.
    /// A turn of `said` and `reply`, each within [`Self::MAX_PART_BYTES`].
    ///
    /// # Errors
    ///
    /// [`ExecutionError::EmptyValue`] when neither part holds text that is
    /// not blank; [`ExecutionError::ValueTooLong`] for a part past
    /// [`Self::MAX_PART_BYTES`].
    ///
    /// # Examples
    ///
    /// ```
    /// use nessa_sdk::domain::agent_execution::prompts::CarriedTurn;
    ///
    /// let turn = CarriedTurn::new(Some("Plot May"), Some("Done: may.png"), None)?;
    /// assert_eq!(turn.said(), Some("Plot May"));
    /// assert_eq!(turn.reply(), Some("Done: may.png"));
    /// assert!(CarriedTurn::new(Some(" "), None, None).is_err());
    /// # Ok::<(), nessa_sdk::domain::agent_execution::ExecutionError>(())
    /// ```
    pub fn new(
        said: Option<&str>,
        reply: Option<&str>,
        agent_source: Option<AgentSource>,
    ) -> Result<Self, ExecutionError> {
        let said = part(said, "carried turn text")?;
        let reply = part(reply, "carried turn reply")?;
        if said.is_none() && reply.is_none() {
            return Err(ExecutionError::EmptyValue("carried turn"));
        }
        Ok(Self {
            said,
            reply,
            agent_source,
        })
    }

    /// Retain `agent_source` as the input origin while clipping only text.
    /// A turn of `said` and `reply`, each cut at a character boundary to
    /// [`Self::MAX_PART_BYTES`] when it is longer; `None` when neither holds
    /// text that is not blank. How a turn read from a conversation's records
    /// is carried: a long message keeps its start.
    ///
    /// ```
    /// use nessa_sdk::domain::agent_execution::prompts::CarriedTurn;
    ///
    /// let long = "é".repeat(CarriedTurn::MAX_PART_BYTES);
    /// let turn = CarriedTurn::clipped(Some(&long), None, None).expect("a turn");
    /// assert!(turn.said().expect("said").len() <= CarriedTurn::MAX_PART_BYTES);
    /// assert!(CarriedTurn::clipped(None, Some("  "), None).is_none());
    /// ```
    pub fn clipped(
        said: Option<&str>,
        reply: Option<&str>,
        agent_source: Option<AgentSource>,
    ) -> Option<Self> {
        let clip = |text: Option<&str>| {
            text.map(str::trim)
                .filter(|text| !text.is_empty())
                .map(|text| Box::from(cut(text, Self::MAX_PART_BYTES)))
        };
        let (said, reply) = (clip(said), clip(reply));
        (said.is_some() || reply.is_some()).then_some(Self {
            said,
            reply,
            agent_source,
        })
    }

    /// What was said in the turn, when it said anything in text.
    pub fn said(&self) -> Option<&str> {
        self.said.as_deref()
    }
    /// What the agent said last in reply, when it said anything.
    pub fn reply(&self) -> Option<&str> {
        self.reply.as_deref()
    }
    /// Verified source of the input when another turn's agent wrote it.
    /// This is provenance and grants no authority to the receiving session.
    pub fn agent_source(&self) -> Option<&AgentSource> {
        self.agent_source.as_ref()
    }
    /// Variable bytes retained for text and agent identity.
    pub fn payload_bytes(&self) -> usize {
        self.text_bytes().saturating_add(
            self.agent_source
                .as_ref()
                .map_or(0, AgentSource::payload_bytes),
        )
    }
    /// Bytes of text the turn holds, both parts together.
    pub fn text_bytes(&self) -> usize {
        self.said.as_deref().map_or(0, str::len) + self.reply.as_deref().map_or(0, str::len)
    }
}

/// The earlier turns of a conversation that the native session it now runs
/// in has not seen, oldest first: what a turn after a fork or an agent switch
/// is given ahead of its message, and what a fork copies from its source.
///
/// Bounded by [`Self::MAX_TURNS`] and [`Self::MAX_TEXT_BYTES`]. Built with
/// [`Self::newest`], which keeps the newest turns that fit and counts the
/// rest in [`Self::omitted`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CarriedHistory {
    turns: Box<[CarriedTurn]>,
    omitted: u32,
}
impl CarriedHistory {
    /// Most turns carried.
    pub const MAX_TURNS: usize = 256;
    /// Most bytes of text carried, every turn's parts together.
    pub const MAX_TEXT_BYTES: usize = 64 * 1024;

    /// The newest of `turns` (oldest first) that fit both bounds, with the
    /// rest counted as omitted on top of `omitted`, the turns already left out
    /// before the first of these.
    ///
    /// ```
    /// use nessa_sdk::domain::agent_execution::prompts::{CarriedHistory, CarriedTurn};
    ///
    /// let turns: Vec<_> = (0..300)
    ///     .map(|n| CarriedTurn::new(Some(&format!("turn {n}")), None, None).unwrap())
    ///     .collect();
    /// let history = CarriedHistory::newest(2, turns);
    /// assert_eq!(history.turns().len(), CarriedHistory::MAX_TURNS);
    /// assert_eq!(history.turns()[0].said(), Some("turn 44"));
    /// assert_eq!(history.omitted(), 46);
    /// ```
    pub fn newest(omitted: u32, turns: Vec<CarriedTurn>) -> Self {
        let mut bytes = 0usize;
        let mut keep = 0usize;
        for turn in turns.iter().rev() {
            if keep == Self::MAX_TURNS || bytes + turn.text_bytes() > Self::MAX_TEXT_BYTES {
                break;
            }
            bytes += turn.text_bytes();
            keep += 1;
        }
        let dropped = turns.len() - keep;
        let mut turns = turns;
        let kept = turns.split_off(dropped);
        Self {
            turns: kept.into_boxed_slice(),
            omitted: u32::try_from(dropped)
                .map_or(u32::MAX, |dropped| omitted.saturating_add(dropped)),
        }
    }

    /// History read back exactly: `turns` within both bounds, and `omitted`.
    ///
    /// # Errors
    ///
    /// [`ExecutionError::TooManyValues`] past [`Self::MAX_TURNS`];
    /// [`ExecutionError::ValueTooLong`] past [`Self::MAX_TEXT_BYTES`].
    pub fn new(turns: Vec<CarriedTurn>, omitted: u32) -> Result<Self, ExecutionError> {
        if turns.len() > Self::MAX_TURNS {
            return Err(ExecutionError::TooManyValues {
                field: "carried history turns",
                max: Self::MAX_TURNS,
            });
        }
        if turns.iter().map(CarriedTurn::text_bytes).sum::<usize>() > Self::MAX_TEXT_BYTES {
            return Err(ExecutionError::ValueTooLong {
                field: "carried history",
                max_bytes: Self::MAX_TEXT_BYTES,
            });
        }
        Ok(Self {
            turns: turns.into_boxed_slice(),
            omitted,
        })
    }

    /// The turns carried, oldest first.
    pub fn turns(&self) -> &[CarriedTurn] {
        &self.turns
    }
    /// How many earlier turns were left out for the bounds.
    pub fn omitted(&self) -> u32 {
        self.omitted
    }
    /// Whether nothing is carried and nothing was left out: a conversation
    /// with no earlier turns.
    pub fn is_empty(&self) -> bool {
        self.turns.is_empty() && self.omitted == 0
    }
    /// Variable bytes retained for text and each copied input's agent identity.
    pub fn payload_bytes(&self) -> usize {
        self.turns.iter().map(CarriedTurn::payload_bytes).sum()
    }
    /// Bytes of text carried, every turn's parts together.
    pub fn text_bytes(&self) -> usize {
        self.turns.iter().map(CarriedTurn::text_bytes).sum()
    }
}

fn part(value: Option<&str>, field: &'static str) -> Result<Option<Box<str>>, ExecutionError> {
    match value {
        None => Ok(None),
        Some(text) if text.trim().is_empty() => Ok(None),
        Some(text) if text.len() > CarriedTurn::MAX_PART_BYTES => {
            Err(ExecutionError::ValueTooLong {
                field,
                max_bytes: CarriedTurn::MAX_PART_BYTES,
            })
        }
        Some(text) => Ok(Some(text.into())),
    }
}

fn cut(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
