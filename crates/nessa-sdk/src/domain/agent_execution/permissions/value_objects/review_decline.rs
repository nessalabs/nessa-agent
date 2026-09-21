//! Why one tool review was answered "no" without ever being offered to a host.

/// The longest provider tool name this decline will retain.
///
/// A name arrives on a provider frame and can be as large as that frame. The
/// point of retaining one is to say which tool was refused, so the limit is
/// what a tool is plausibly called rather than what a frame can hold; anything
/// longer is not a name that would have meant something to a reader.
const MAX_TOOL_NAME_BYTES: usize = 128;

/// Why a review could not be offered.
///
/// Each reason answers a different question about the same frame: whether the
/// tool is one this host reviews at all, whether the request could be described
/// to somebody deciding, and whether there was any choice left to offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewDeclineReason {
    /// The named tool is outside what this binding will put to a host — its own
    /// denials, or a namespace no configured server owns.
    ToolNotReviewable,
    /// The request could not be described: an identity, an input, or a name
    /// this adapter could not read as one.
    UnreadableRequest,
    /// Nothing offerable remained once the provider's choices were filtered,
    /// so there was no decision a host could have made.
    UnusableOptions,
}

/// One refused review: which tool, and why it was refused.
///
/// A decline is a decision, so it is evidence. It is *not* a permission
/// cancellation: nothing was pending, because the request never became one,
/// and there is no permission identity to correlate it with. What it carries
/// instead is the provider's own
/// name where that name was readable, which is the only thing that tells a
/// reader afterwards which tool the agent was refused.
///
/// That name is the provider's claim, and this binding does not verify it. The
/// refusal can be decided from what was observed earlier under the same call
/// identity, while the name comes from the frame asking for the review, and a
/// provider is free to disagree with itself between the two. Retaining the
/// observed name instead would mean holding an unbounded one for the rest of
/// the execution, which is part of what a refused call is refused for. So the
/// claim is recorded as a claim — [`declared`](Self::declared) says whose it
/// is — rather than dressed up as something checked.
///
/// It is optional for a related reason: a frame that could not be read as a
/// request may not carry a readable name either, and inventing one would be
/// worse than saying plainly that the tool could not be named.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewDecline {
    tool: Option<Box<str>>,
    reason: ReviewDeclineReason,
}

impl ReviewDecline {
    /// Refuse a review of `tool` for `reason`.
    ///
    /// `tool` is the provider's own name for it. A name that is empty, longer
    /// than 128 bytes, or not printable on one line is not retained: the
    /// decline still records its reason, and [`declared`](Self::declared) answers
    /// `None`. Construction performs no I/O and never fails, because a refusal
    /// must always be recordable — including the refusal of a frame that was
    /// unreadable in the first place.
    pub fn new(tool: Option<&str>, reason: ReviewDeclineReason) -> Self {
        Self {
            tool: tool.filter(|name| nameable(name)).map(Box::from),
            reason,
        }
    }

    /// The name the provider gave the refused tool, where it was readable.
    ///
    /// Provider-asserted and unverified; the type's own documentation says why
    /// this binding cannot check it against what it observed.
    pub fn declared(&self) -> Option<&str> {
        self.tool.as_deref()
    }

    /// Why the review was refused.
    pub fn reason(&self) -> ReviewDeclineReason {
        self.reason
    }

    /// Whether a name was readable, kept separate from what the name was.
    ///
    /// "No name" and "a name nobody should read" are the same absence to a
    /// caller rendering this, and neither is an error.
    pub fn named(&self) -> bool {
        self.tool.is_some()
    }
}

/// Whether this is a name worth keeping: bounded, printable, and on one line.
fn nameable(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_TOOL_NAME_BYTES
        && name.chars().all(|c| !c.is_control() && c != '\u{7f}')
}

#[cfg(test)]
#[path = "../../../../../tests/domain/agent_execution/review_decline.rs"]
mod tests;
