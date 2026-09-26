//! Why an ask was answered "no" without ever being put to anybody.

/// Why an ask could not be offered.
///
/// Each is a local decision with an effect on the agent — told `cancel`, it
/// abandons the tool call that asked — so each is evidence, and each says which
/// limit of this binding the ask ran into rather than what the agent did wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuestionRefusalReason {
    /// As many asks were already open as a surface can show; one more would
    /// have been admitted where nobody could see it, let alone answer it.
    TooManyOpen,
    /// The ask, beside those already open, costs more to carry than a surface
    /// can show at once. Admitted, it could not be shown whole, and an ask
    /// that cannot be shown cannot be answered.
    TooLarge,
    /// The ask is a kind this binding does not put to people — a page to visit,
    /// or a form that requires something only the answerer could write.
    Unsupported,
    /// The ask could not be read as questions with answers to choose from.
    UnreadableQuestion,
    /// The session was ending when it arrived, so no answer could have come.
    SessionEnding,
}
