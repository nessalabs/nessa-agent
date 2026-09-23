//! Why one tool review was answered "no" without ever being offered to a host.
use crate::domain::agent_execution::ExecutionError;

/// The longest provider tool name this decline will retain.
///
/// A name arrives on a provider frame and can be as large as that frame. The
/// point of retaining one is to say which tool was refused, so the limit is
/// what a tool is plausibly called rather than what a frame can hold; anything
/// longer is not a name that would have meant something to a reader.
const MAX_TOOL_NAME_BYTES: usize = 128;

/// Stable local identity for one review the runtime declined before offering it.
///
/// The identity is scoped by its execution. It is deliberately distinct from a
/// provider RPC identity and from [`PermissionId`](super::PermissionId): the
/// declined frame never becomes an actionable permission. The ACP adapter mints
/// decimal sequence values so two otherwise identical declines remain separate.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ReviewDeclineId(Box<str>);

impl ReviewDeclineId {
    /// Restore or construct a checked session-local sequence identity.
    ///
    /// `value` is consumed and retained as compact owned text. Decimal text from
    /// `1` through `u64::MAX` is accepted. Keeping this shape bounded makes live
    /// and saved observations use the same identity contract. Construction
    /// performs no I/O and does not establish a provider or permission identity.
    ///
    /// Returns [`ExecutionError::InvalidReviewDeclineId`] when `value` is empty,
    /// zero, signed, non-decimal, non-canonical, longer than 20 bytes, or outside
    /// the positive `u64` range.
    pub fn new(value: impl Into<String>) -> Result<Self, ExecutionError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 20
            || value.starts_with('0')
            || !value.bytes().all(|byte| byte.is_ascii_digit())
            || value
                .parse::<u64>()
                .ok()
                .filter(|number| *number > 0)
                .is_none()
        {
            return Err(ExecutionError::InvalidReviewDeclineId);
        }
        Ok(Self(value.into_boxed_str()))
    }

    /// Borrow the decimal identity text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

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

/// Local evidence stage for a review declined before host presentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewDeclineStage {
    /// The runtime selected refusal and has no later response-write evidence.
    Selected,
    /// The response write completed; this is not provider acknowledgement.
    WriteConfirmed,
    /// A response write was attempted, but local observation cannot confirm completion.
    WriteUnconfirmed,
    /// The owning operation ended before attempting a response write.
    WriteNotAttempted,
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

/// One immutable local refusal observation and its current delivery evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewDeclineObservation {
    id: ReviewDeclineId,
    decline: ReviewDecline,
    stage: ReviewDeclineStage,
}

impl ReviewDeclineObservation {
    /// Create the first observation for one local refusal.
    ///
    /// Consumes the execution-scoped `id` and immutable `decline`, retaining
    /// both under [`ReviewDeclineStage::Selected`]. This performs no I/O and
    /// makes no response-write or provider-acknowledgement claim. A history must
    /// retain this selection before admitting a successor from
    /// [`advance`](Self::advance).
    pub fn selected(id: ReviewDeclineId, decline: ReviewDecline) -> Self {
        Self {
            id,
            decline,
            stage: ReviewDeclineStage::Selected,
        }
    }

    /// Restore one individually valid observation from checked constituent values.
    ///
    /// Consumes `id`, `decline`, and `stage` without performing I/O. This
    /// constructor deliberately does not assert that a final stage had a saved
    /// predecessor; restoration history must pair it with the preceding
    /// selection by calling [`advance`](Self::advance) and comparing the result.
    pub fn restore(id: ReviewDeclineId, decline: ReviewDecline, stage: ReviewDeclineStage) -> Self {
        Self { id, decline, stage }
    }

    /// Produce the one allowed replacement: the same refusal advancing from
    /// selection to a final local write fact.
    ///
    /// `stage` must be one of the three final write stages. The returned value
    /// clones the selected observation's identity and decline; `self` remains
    /// unchanged. This performs no I/O and establishes only local write
    /// evidence, never provider acknowledgement or tool behavior.
    ///
    /// Returns [`ExecutionError::InvalidReviewDeclineTransition`] when `self`
    /// is already final or `stage` is [`ReviewDeclineStage::Selected`].
    pub fn advance(&self, stage: ReviewDeclineStage) -> Result<Self, ExecutionError> {
        if self.stage != ReviewDeclineStage::Selected || stage == ReviewDeclineStage::Selected {
            return Err(ExecutionError::InvalidReviewDeclineTransition);
        }
        Ok(Self {
            id: self.id.clone(),
            decline: self.decline.clone(),
            stage,
        })
    }

    /// Stable identity distinguishing repeated otherwise identical refusals.
    pub fn id(&self) -> &ReviewDeclineId {
        &self.id
    }

    /// Provider label claim and local refusal reason.
    pub fn decline(&self) -> &ReviewDecline {
        &self.decline
    }

    /// Current local selection or response-write evidence.
    pub fn stage(&self) -> ReviewDeclineStage {
        self.stage
    }
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

    /// Restore exact retained evidence without silently dropping a corrupt label.
    ///
    /// Consumes the saved optional provider `tool` claim and `reason`. Live
    /// construction may omit an unusable provider claim so a refusal can always
    /// be recorded. Saved evidence has already claimed that a label was retained;
    /// an invalid saved label is corruption and must be rejected. This performs
    /// no I/O and preserves valid text exactly in compact owned storage.
    ///
    /// Returns [`ExecutionError::InvalidReviewDeclineToolName`] when a present
    /// label is empty, exceeds 128 UTF-8 bytes, or contains a control character.
    pub fn restore(
        tool: Option<String>,
        reason: ReviewDeclineReason,
    ) -> Result<Self, ExecutionError> {
        if tool.as_deref().is_some_and(|name| !nameable(name)) {
            return Err(ExecutionError::InvalidReviewDeclineToolName);
        }
        Ok(Self {
            tool: tool.map(String::into_boxed_str),
            reason,
        })
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
