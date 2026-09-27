/// Provider acknowledgement of an attempted mid-execution message.
/// Injected belongs to the targeted invocation's output. PromptRequired confirms
/// the message was not consumed; only the host may schedule a separate invocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SteeringOutcome {
    Injected,
    PromptRequired,
}
