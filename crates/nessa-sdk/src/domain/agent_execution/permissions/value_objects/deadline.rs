//! How long a pending review waits, and what happens when nobody answers.
#![deny(missing_docs)]

use super::{PermissionEffect, PermissionOptionId, PermissionOptions};
use crate::domain::agent_execution::{tools::ToolKind, ExecutionError};
use std::time::Duration;

/// The longest a review may wait. Longer than this can saturate the clock and
/// leave the review waiting, which is the stall a deadline exists to end.
pub const MAX_PERMISSION_DEADLINE: Duration = Duration::from_secs(24 * 60 * 60);

/// The longest reason a deadline may record, in UTF-8 bytes.
pub const MAX_DEADLINE_REASON_BYTES: usize = 512;

/// The sentence a standard read-only deadline records when it allows.
pub const STANDARD_ALLOW_REASON: &str =
    "No human answered in time. This read-only tool was allowed so the turn could continue.";

/// The sentence a standard denial records. A missing allow uses this too:
/// a deadline that denies does not say the tool was allowed.
pub const STANDARD_DENY_REASON: &str =
    "No human answered in time. Continue without this tool or try another approach.";

/// The sentence recorded when the only refusal the agent offered stops the turn.
pub const STANDARD_STOP_REASON: &str =
    "No human answered in time. The only refusal this agent offered stops the turn.";

/// Which reviews share a deadline. This is policy, not the provider's display
/// category: a kind is mapped into a class once, at the review.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionToolClass {
    /// A read or a search. Arming keeps this class only when every target
    /// path resolves inside the session workspace. The content is sent to
    /// the model provider; the class is about what the tool changes.
    ReadOnly,
    /// Everything else, including a review whose kind was not named, a read
    /// or search with no path, and one whose path leaves the workspace.
    Other,
}

impl PermissionToolClass {
    /// The class a provider-reported kind belongs to. A missing kind is
    /// [`Self::Other`]: an unnamed tool is not treated as a read.
    pub fn of(kind: Option<ToolKind>) -> Self {
        match kind {
            Some(ToolKind::Read | ToolKind::Search) => Self::ReadOnly,
            _ => Self::Other,
        }
    }

    /// Stable name recorded as the deadline actor's surface.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::Other => "other",
        }
    }
}

/// The reason a deadline records. Bounded at construction; the live event
/// carries this type rather than an unchecked string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeadlineReason(Box<str>);

impl DeadlineReason {
    /// Retain `reason` when it is a sentence a deadline may record.
    pub fn new(reason: impl Into<String>) -> Result<Self, ExecutionError> {
        checked_deadline_reason(reason.into())
    }

    /// The recorded sentence.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DeadlineReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl AsRef<str> for DeadlineReason {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl PartialEq<str> for DeadlineReason {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for DeadlineReason {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

/// One class's wait, the outcome taken when it ends, and the reason recorded
/// with that outcome. The reason is what the audit and the after-the-fact
/// notice show. A pending card builds its own line from the outcome and the wait.
#[derive(Clone, Debug)]
pub struct PermissionReviewDeadline {
    class: PermissionToolClass,
    wait: Duration,
    effect: PermissionEffect,
    reason: DeadlineReason,
    /// The selected refusal stops the agent's turn. A configured deadline
    /// does not; only a settlement that had to select such a refusal does.
    stops_turn: bool,
    /// Wall-clock millisecond when a live worker will decide, so a reloaded
    /// client can count down. Absent on a policy template.
    due_at_ms: Option<u64>,
    /// A live worker is holding this review and will decide it. A restored
    /// deadline keeps its recorded facts and is not armed: nothing in the new
    /// process will answer the provider request that died with the old one,
    /// so a card must not promise a decision.
    armed: bool,
}

impl PermissionReviewDeadline {
    /// Retain a wait, an effect, and a reason for `class`.
    ///
    /// The wait must be positive and at most [`MAX_PERMISSION_DEADLINE`]. The
    /// reason must contain non-whitespace text, no control character, and at
    /// most [`MAX_DEADLINE_REASON_BYTES`] UTF-8 bytes.
    pub fn new(
        class: PermissionToolClass,
        wait: Duration,
        effect: PermissionEffect,
        reason: impl Into<String>,
    ) -> Result<Self, ExecutionError> {
        if wait.is_zero() {
            return Err(ExecutionError::ZeroPermissionDeadline);
        }
        if wait > MAX_PERMISSION_DEADLINE {
            return Err(ExecutionError::PermissionDeadlineTooLong);
        }
        Ok(Self {
            class,
            wait,
            effect,
            reason: checked_deadline_reason(reason.into())?,
            stops_turn: false,
            due_at_ms: None,
            armed: false,
        })
    }

    /// This deadline as the one a live worker will decide at `due_at_ms`.
    ///
    /// `due_at_ms` is Unix milliseconds. The returned value is the one a card
    /// may promise; a deadline that was only restored is not this.
    pub fn promise(self, due_at_ms: u64) -> Self {
        Self {
            due_at_ms: Some(due_at_ms),
            armed: true,
            ..self
        }
    }

    /// A denial of `class` after `wait` whose only offered refusal stops the turn.
    pub fn stopping(class: PermissionToolClass, wait: Duration) -> Self {
        Self {
            stops_turn: true,
            ..Self::new(class, wait, PermissionEffect::Deny, STANDARD_STOP_REASON)
                .expect("stop sentence")
        }
    }

    /// The class this deadline was configured for.
    pub fn class(&self) -> PermissionToolClass {
        self.class
    }

    /// How long the review waits before this outcome is taken.
    pub fn wait(&self) -> Duration {
        self.wait
    }

    /// The outcome taken when the wait ends with no admitted answer.
    pub fn effect(&self) -> PermissionEffect {
        self.effect
    }

    /// Why that outcome was taken, recorded in the audit and the notice.
    pub fn reason(&self) -> &str {
        self.reason.as_str()
    }

    /// The reason as the type a live event stores.
    pub fn recorded_reason(&self) -> DeadlineReason {
        self.reason.clone()
    }

    /// Whether the refusal this settlement selects stops the agent's turn.
    pub fn stops_turn(&self) -> bool {
        self.stops_turn
    }

    /// Unix milliseconds when a live worker decides, once this deadline was promised.
    pub fn due_at_ms(&self) -> Option<u64> {
        self.due_at_ms
    }

    /// Whether a live worker will decide this review.
    pub fn armed(&self) -> bool {
        self.armed
    }

    /// The facts saved with a review, not a promise that this process will decide it.
    pub fn restored(self, stops_turn: bool, due_at_ms: Option<u64>) -> Self {
        Self {
            stops_turn,
            due_at_ms,
            armed: false,
            ..self
        }
    }
}

impl PartialEq for PermissionReviewDeadline {
    /// `armed` is whether this process will decide the review. A save folds
    /// the live review back through restore, and restore does not arm it,
    /// because a restarted process has no provider request to answer. The
    /// recorded decision is the same either way.
    fn eq(&self, other: &Self) -> bool {
        self.class == other.class
            && self.wait == other.wait
            && self.effect == other.effect
            && self.reason == other.reason
            && self.stops_turn == other.stops_turn
            && self.due_at_ms == other.due_at_ms
    }
}

impl Eq for PermissionReviewDeadline {}

/// The two class deadlines a host installs. [`Self::standard`] is what
/// composition installs when a host does not replace it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionDeadlinePolicy {
    read_only: PermissionReviewDeadline,
    other: PermissionReviewDeadline,
}

impl PermissionDeadlinePolicy {
    /// Read-only reviews are allowed after eight seconds. Every other review
    /// is denied after thirty. See ADR 141 for why those are the defaults.
    pub fn standard() -> Self {
        Self {
            read_only: PermissionReviewDeadline::new(
                PermissionToolClass::ReadOnly,
                Duration::from_secs(8),
                PermissionEffect::Allow,
                STANDARD_ALLOW_REASON,
            )
            .expect("standard read-only deadline"),
            other: PermissionReviewDeadline::new(
                PermissionToolClass::Other,
                Duration::from_secs(30),
                PermissionEffect::Deny,
                STANDARD_DENY_REASON,
            )
            .expect("standard other deadline"),
        }
    }

    /// Install the two class deadlines. Each must already name its class.
    pub fn new(
        read_only: PermissionReviewDeadline,
        other: PermissionReviewDeadline,
    ) -> Result<Self, ExecutionError> {
        if read_only.class != PermissionToolClass::ReadOnly
            || other.class != PermissionToolClass::Other
        {
            return Err(ExecutionError::MismatchedPermissionDeadline);
        }
        Ok(Self { read_only, other })
    }

    /// The deadline configured for `class`, before the offer is consulted.
    pub fn for_class(&self, class: PermissionToolClass) -> &PermissionReviewDeadline {
        match class {
            PermissionToolClass::ReadOnly => &self.read_only,
            PermissionToolClass::Other => &self.other,
        }
    }

    /// The deadline that will actually be taken for `class` given the options
    /// this review offered and which denials `deny` says stop the turn.
    ///
    /// An allow is taken when that option is offered. An allow whose option is
    /// missing becomes a deny, and the reason is then a deny sentence. Among
    /// denials, `deny` picks: a preferred id first, then any denial that does
    /// not stop the turn, then a denial that does. Selecting that last one
    /// records [`STANDARD_STOP_REASON`], because "continue without this tool"
    /// would be false. When no denial is offered, the settlement names a deny
    /// and no option: the review is cancelled instead of being allowed with
    /// nothing to select.
    pub fn settlement(
        &self,
        class: PermissionToolClass,
        options: &PermissionOptions,
        deny: &DeadlineDenyPreference,
    ) -> PermissionDeadlineSettlement {
        let configured = self.for_class(class);
        if configured.effect == PermissionEffect::Allow {
            if let Some(option) = first_effect(options, PermissionEffect::Allow) {
                return PermissionDeadlineSettlement {
                    deadline: configured.clone(),
                    option: Some(option),
                };
            }
        }
        let mut deadline = self.deny_for(class);
        let chosen = deny.select(
            options
                .choices()
                .iter()
                .filter(|option| option.decision().effect() == PermissionEffect::Deny)
                .map(|option| option.id()),
        );
        if chosen.as_ref().is_some_and(|(_, stops)| *stops) {
            deadline = PermissionReviewDeadline::stopping(deadline.class(), deadline.wait());
        }
        PermissionDeadlineSettlement {
            deadline,
            option: chosen.map(|(id, _)| id),
        }
    }

    fn deny_for(&self, class: PermissionToolClass) -> PermissionReviewDeadline {
        let configured = self.for_class(class);
        if configured.effect == PermissionEffect::Deny {
            return configured.clone();
        }
        let reason = if self.other.effect == PermissionEffect::Deny {
            self.other.reason.clone()
        } else {
            checked_deadline_reason(STANDARD_DENY_REASON.to_owned()).expect("deny sentence")
        };
        PermissionReviewDeadline {
            class,
            wait: configured.wait,
            effect: PermissionEffect::Deny,
            reason,
            stops_turn: false,
            due_at_ms: None,
            armed: false,
        }
    }
}

/// Which denial a deadline selects, and which of those stop the agent's turn.
///
/// A profile builds this from the option ids its harness actually offers.
/// Two ids can both be a one-request refusal and still mean different things:
/// one lets the turn continue, and one ends it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeadlineDenyPreference {
    preferred: Vec<Box<str>>,
    stops_turn: Vec<Box<str>>,
}

impl DeadlineDenyPreference {
    /// Every denial continues the turn. The first one offered is selected.
    /// Claude and OpenCode are this: a `reject_once` does not stop the turn.
    pub fn continuing() -> Self {
        Self::default()
    }

    /// Prefer `preferred` ids in order. An id in `stops_turn` ends the turn
    /// when it is the one selected.
    pub fn new(
        preferred: impl IntoIterator<Item = impl AsRef<str>>,
        stops_turn: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Self {
        Self {
            preferred: preferred
                .into_iter()
                .map(|id| id.as_ref().to_owned().into_boxed_str())
                .collect(),
            stops_turn: stops_turn
                .into_iter()
                .map(|id| id.as_ref().to_owned().into_boxed_str())
                .collect(),
        }
    }

    /// The denial to select, and whether selecting it stops the turn.
    ///
    /// Preferred ids win, in the order they were given. Otherwise a denial
    /// that does not stop the turn. Otherwise the first remaining denial,
    /// which does stop the turn. No denial means the review is cancelled.
    pub fn select<'a>(
        &self,
        denies: impl IntoIterator<Item = &'a PermissionOptionId>,
    ) -> Option<(PermissionOptionId, bool)> {
        let denies: Vec<&PermissionOptionId> = denies.into_iter().collect();
        for preferred in &self.preferred {
            if let Some(id) = denies
                .iter()
                .copied()
                .find(|id| id.as_str() == preferred.as_ref())
            {
                return Some((id.clone(), self.stops_turn(id.as_str())));
            }
        }
        if let Some(id) = denies
            .iter()
            .copied()
            .find(|id| !self.stops_turn(id.as_str()))
        {
            return Some((id.clone(), false));
        }
        denies.first().map(|id| ((*id).clone(), true))
    }

    /// Whether selecting this option id ends the turn.
    ///
    /// Codex `cancel` does. A profile built with [`Self::continuing`] has no
    /// such id, so a `reject_once` does not.
    pub fn stops_turn(&self, id: &str) -> bool {
        self.stops_turn.iter().any(|stop| stop.as_ref() == id)
    }
}

/// What the deadline will do when it wins: the outcome it publishes, and the
/// offered option it selects. No option means the review is cancelled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionDeadlineSettlement {
    deadline: PermissionReviewDeadline,
    option: Option<PermissionOptionId>,
}

impl PermissionDeadlineSettlement {
    /// The outcome, wait, and reason published while the review is pending
    /// and recorded if this settlement wins.
    pub fn deadline(&self) -> &PermissionReviewDeadline {
        &self.deadline
    }

    /// The offered option this settlement selects, when the review offered one
    /// for its effect.
    pub fn option(&self) -> Option<&PermissionOptionId> {
        self.option.as_ref()
    }
}

fn first_effect(
    options: &PermissionOptions,
    effect: PermissionEffect,
) -> Option<PermissionOptionId> {
    options
        .choices()
        .iter()
        .find(|option| option.decision().effect() == effect)
        .map(|option| option.id().clone())
}

/// How many milliseconds `wait` is, saturating past what a `u64` can hold.
pub fn wait_ms(wait: Duration) -> u64 {
    u64::try_from(wait.as_millis()).unwrap_or(u64::MAX)
}

pub(crate) fn checked_deadline_reason(reason: String) -> Result<DeadlineReason, ExecutionError> {
    if reason.trim().is_empty() {
        return Err(ExecutionError::EmptyValue("permission deadline reason"));
    }
    if reason.len() > MAX_DEADLINE_REASON_BYTES {
        return Err(ExecutionError::ValueTooLong {
            field: "permission deadline reason",
            max_bytes: MAX_DEADLINE_REASON_BYTES,
        });
    }
    if reason.chars().any(char::is_control) {
        return Err(ExecutionError::ControlCharacterInPermissionReason);
    }
    Ok(DeadlineReason(reason.into_boxed_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::agent_execution::permissions::{
        PermissionDecision, PermissionOfferPolicy, PermissionOption, PermissionScope,
    };

    fn options(effects: &[PermissionEffect]) -> PermissionOptions {
        let choices = effects
            .iter()
            .enumerate()
            .map(|(index, effect)| {
                PermissionOption::new(
                    PermissionOptionId::new(format!("option-{index}")).unwrap(),
                    "Choice",
                    PermissionDecision::new(*effect, PermissionScope::request()),
                )
                .unwrap()
            })
            .collect();
        PermissionOptions::new(choices, &PermissionOfferPolicy::once_only()).unwrap()
    }

    #[test]
    fn standard_policy_allows_reads_and_denies_the_rest() {
        let policy = PermissionDeadlinePolicy::standard();
        let read = policy.for_class(PermissionToolClass::ReadOnly);
        let other = policy.for_class(PermissionToolClass::Other);
        assert_eq!(read.wait(), Duration::from_secs(8));
        assert_eq!(read.effect(), PermissionEffect::Allow);
        assert!(read.reason().contains("No human answered in time"));
        assert!(read.reason().contains("read-only"));
        assert_eq!(other.wait(), Duration::from_secs(30));
        assert_eq!(other.effect(), PermissionEffect::Deny);
        assert!(other.reason().contains("Continue without this tool"));
        assert_eq!(PermissionToolClass::ReadOnly.as_str(), "read_only");
        assert_eq!(PermissionToolClass::Other.as_str(), "other");
        assert_eq!(
            PermissionToolClass::of(Some(ToolKind::Read)),
            PermissionToolClass::ReadOnly
        );
        assert_eq!(
            PermissionToolClass::of(Some(ToolKind::Search)),
            PermissionToolClass::ReadOnly
        );
        assert_eq!(PermissionToolClass::of(None), PermissionToolClass::Other);
        // Every kind is classified. A new kind fails to compile until it is.
        for kind in [
            ToolKind::Read,
            ToolKind::Search,
            ToolKind::Edit,
            ToolKind::Fetch,
            ToolKind::Execute,
            ToolKind::Think,
            ToolKind::Delete,
            ToolKind::Move,
            ToolKind::SwitchMode,
            ToolKind::Other,
        ] {
            let class = match kind {
                ToolKind::Read | ToolKind::Search => PermissionToolClass::ReadOnly,
                ToolKind::Edit
                | ToolKind::Fetch
                | ToolKind::Execute
                | ToolKind::Think
                | ToolKind::Delete
                | ToolKind::Move
                | ToolKind::SwitchMode
                | ToolKind::Other => PermissionToolClass::Other,
            };
            assert_eq!(PermissionToolClass::of(Some(kind)), class, "{kind:?}");
        }
    }

    #[test]
    fn a_deadline_rejects_a_wait_or_reason_it_cannot_show() {
        let reason = "No human answered in time.";
        assert_eq!(
            PermissionReviewDeadline::new(
                PermissionToolClass::Other,
                Duration::ZERO,
                PermissionEffect::Deny,
                reason,
            ),
            Err(ExecutionError::ZeroPermissionDeadline)
        );
        assert_eq!(
            PermissionReviewDeadline::new(
                PermissionToolClass::Other,
                MAX_PERMISSION_DEADLINE + Duration::from_secs(1),
                PermissionEffect::Deny,
                reason,
            ),
            Err(ExecutionError::PermissionDeadlineTooLong)
        );
        assert_eq!(
            PermissionReviewDeadline::new(
                PermissionToolClass::Other,
                Duration::from_secs(1),
                PermissionEffect::Deny,
                " ",
            ),
            Err(ExecutionError::EmptyValue("permission deadline reason"))
        );
        assert_eq!(
            PermissionReviewDeadline::new(
                PermissionToolClass::Other,
                Duration::from_secs(1),
                PermissionEffect::Deny,
                "line\nbreak",
            ),
            Err(ExecutionError::ControlCharacterInPermissionReason)
        );
        let oversized = "x".repeat(MAX_DEADLINE_REASON_BYTES + 1);
        assert_eq!(
            PermissionReviewDeadline::new(
                PermissionToolClass::Other,
                Duration::from_secs(1),
                PermissionEffect::Deny,
                oversized,
            ),
            Err(ExecutionError::ValueTooLong {
                field: "permission deadline reason",
                max_bytes: MAX_DEADLINE_REASON_BYTES,
            })
        );
        let other = PermissionReviewDeadline::new(
            PermissionToolClass::Other,
            Duration::from_secs(1),
            PermissionEffect::Deny,
            reason,
        )
        .unwrap();
        let read_only = PermissionReviewDeadline::new(
            PermissionToolClass::ReadOnly,
            Duration::from_secs(1),
            PermissionEffect::Allow,
            reason,
        )
        .unwrap();
        assert_eq!(
            PermissionDeadlinePolicy::new(other.clone(), other),
            Err(ExecutionError::MismatchedPermissionDeadline)
        );
        assert_eq!(
            PermissionDeadlinePolicy::new(read_only.clone(), read_only),
            Err(ExecutionError::MismatchedPermissionDeadline)
        );
        for error in [
            ExecutionError::ZeroPermissionDeadline,
            ExecutionError::PermissionDeadlineTooLong,
            ExecutionError::ControlCharacterInPermissionReason,
            ExecutionError::MismatchedPermissionDeadline,
        ] {
            assert!(error.to_string().starts_with("agent execution: "));
        }
    }

    fn settle(
        policy: &PermissionDeadlinePolicy,
        class: PermissionToolClass,
        offered: &PermissionOptions,
    ) -> PermissionDeadlineSettlement {
        policy.settlement(class, offered, &DeadlineDenyPreference::continuing())
    }

    #[test]
    fn a_missing_allow_uses_a_deny_sentence_when_nothing_denies() {
        let read_only = PermissionReviewDeadline::new(
            PermissionToolClass::ReadOnly,
            Duration::from_secs(8),
            PermissionEffect::Allow,
            "read sentence",
        )
        .unwrap();
        let other = PermissionReviewDeadline::new(
            PermissionToolClass::Other,
            Duration::from_secs(30),
            PermissionEffect::Allow,
            "other sentence",
        )
        .unwrap();
        let policy = PermissionDeadlinePolicy::new(read_only, other).unwrap();
        let settled = settle(
            &policy,
            PermissionToolClass::ReadOnly,
            &options(&[PermissionEffect::Deny]),
        );
        assert_eq!(settled.deadline().effect(), PermissionEffect::Deny);
        assert_eq!(settled.deadline().reason(), STANDARD_DENY_REASON);
        assert!(!settled.deadline().reason().contains("was allowed"));
        assert_eq!(settled.deadline().wait(), Duration::from_secs(8));
        assert!(settled.option().is_some());
        assert!(!settled.deadline().stops_turn());
    }

    #[test]
    fn a_missing_allow_settles_as_a_deny_when_one_is_offered() {
        let policy = PermissionDeadlinePolicy::standard();
        let both = options(&[PermissionEffect::Allow, PermissionEffect::Deny]);
        let allow = settle(&policy, PermissionToolClass::ReadOnly, &both);
        assert_eq!(allow.deadline().effect(), PermissionEffect::Allow);
        assert!(allow.option().is_some());
        let deny_only = options(&[PermissionEffect::Deny]);
        let fallen = settle(&policy, PermissionToolClass::ReadOnly, &deny_only);
        assert_eq!(fallen.deadline().effect(), PermissionEffect::Deny);
        assert_eq!(fallen.deadline().reason(), STANDARD_DENY_REASON);
        assert!(fallen.option().is_some());
        let allow_only = options(&[PermissionEffect::Allow]);
        let cancelled = settle(&policy, PermissionToolClass::Other, &allow_only);
        assert_eq!(cancelled.deadline().effect(), PermissionEffect::Deny);
        assert!(cancelled.option().is_none());
        assert_eq!(wait_ms(Duration::from_secs(8)), 8_000);
    }

    fn named(ids: &[(&str, PermissionEffect)]) -> PermissionOptions {
        let choices = ids
            .iter()
            .map(|(id, effect)| {
                PermissionOption::new(
                    PermissionOptionId::new(*id).unwrap(),
                    *id,
                    PermissionDecision::new(*effect, PermissionScope::request()),
                )
                .unwrap()
            })
            .collect();
        PermissionOptions::new(choices, &PermissionOfferPolicy::once_only()).unwrap()
    }

    /// Codex ACP 1.12.0 (`a7afd2ae`) offers both `decline` and `cancel` as
    /// `reject_once`. `decline` continues the turn. `cancel` stops it. File
    /// edits, MCP tool approvals, and extra-permission commands offer only
    /// `cancel`.
    #[test]
    fn codex_decline_is_preferred_and_a_cancel_only_denial_stops_the_turn() {
        let policy = PermissionDeadlinePolicy::standard();
        let codex = DeadlineDenyPreference::new(["decline"], ["cancel"]);
        assert!(codex.stops_turn("cancel"));
        assert!(!codex.stops_turn("decline"));
        assert!(!DeadlineDenyPreference::continuing().stops_turn("reject_once"));
        let command = named(&[
            ("allow_once", PermissionEffect::Allow),
            ("cancel", PermissionEffect::Deny),
            ("decline", PermissionEffect::Deny),
        ]);
        let continued = policy.settlement(PermissionToolClass::Other, &command, &codex);
        assert_eq!(continued.option().unwrap().as_str(), "decline");
        assert!(!continued.deadline().stops_turn());
        assert_eq!(continued.deadline().reason(), STANDARD_DENY_REASON);

        let file_change = named(&[
            ("allow_once", PermissionEffect::Allow),
            ("cancel", PermissionEffect::Deny),
        ]);
        let stopped = policy.settlement(PermissionToolClass::Other, &file_change, &codex);
        assert_eq!(stopped.option().unwrap().as_str(), "cancel");
        assert!(stopped.deadline().stops_turn());
        assert_eq!(stopped.deadline().reason(), STANDARD_STOP_REASON);
        assert!(!stopped
            .deadline()
            .reason()
            .contains("Continue without this tool"));

        let mcp_tool = named(&[
            ("allow_once", PermissionEffect::Allow),
            ("cancel", PermissionEffect::Deny),
        ]);
        let mcp = policy.settlement(PermissionToolClass::Other, &mcp_tool, &codex);
        assert!(mcp.deadline().stops_turn());
        assert_eq!(mcp.option().unwrap().as_str(), "cancel");

        let extra = named(&[
            ("accept", PermissionEffect::Allow),
            ("cancel", PermissionEffect::Deny),
        ]);
        let extra = policy.settlement(PermissionToolClass::Other, &extra, &codex);
        assert_eq!(extra.option().unwrap().as_str(), "cancel");
        assert!(extra.deadline().stops_turn());

        let prefers_the_stop = DeadlineDenyPreference::new(["cancel"], ["cancel"]);
        let preferred_stop =
            policy.settlement(PermissionToolClass::Other, &command, &prefers_the_stop);
        assert_eq!(preferred_stop.option().unwrap().as_str(), "cancel");
        assert!(preferred_stop.deadline().stops_turn());

        let claude = policy.settlement(
            PermissionToolClass::Other,
            &command,
            &DeadlineDenyPreference::continuing(),
        );
        assert_eq!(claude.option().unwrap().as_str(), "cancel");
        assert!(!claude.deadline().stops_turn());
        assert_eq!(PermissionEffect::Deny.as_str(), "deny");
        assert_eq!(PermissionEffect::Allow.as_str(), "allow");
        assert_eq!(<&'static str>::from(PermissionEffect::Allow), "allow");
        assert_eq!(
            PermissionEffect::try_from("allow"),
            Ok(PermissionEffect::Allow)
        );
        assert_eq!(
            PermissionEffect::try_from("deny"),
            Ok(PermissionEffect::Deny)
        );
        assert_eq!(PermissionEffect::try_from("nope"), Err(()));
        assert_eq!(<&'static str>::from(PermissionEffect::Deny), "deny");
    }

    #[test]
    fn a_reason_keeps_its_sentence_and_a_promised_deadline_can_be_disarmed() {
        let reason = DeadlineReason::new("kept").unwrap();
        assert_eq!(reason.as_str(), "kept");
        assert_eq!(reason.to_string(), "kept");
        assert_eq!(AsRef::<str>::as_ref(&reason), "kept");
        assert!(<DeadlineReason as PartialEq<str>>::eq(&reason, "kept"));
        assert!(<DeadlineReason as PartialEq<&str>>::eq(&reason, &"kept"));
        assert_eq!(
            DeadlineReason::new(" "),
            Err(ExecutionError::EmptyValue("permission deadline reason"))
        );
        assert_eq!(wait_ms(Duration::MAX), u64::MAX);
        let promised =
            PermissionReviewDeadline::stopping(PermissionToolClass::Other, Duration::from_secs(30))
                .promise(1_700_000_030_000);
        assert!(promised.armed());
        assert_eq!(promised.due_at_ms(), Some(1_700_000_030_000));
        assert_eq!(promised.recorded_reason().as_str(), STANDARD_STOP_REASON);
        let restored = promised.clone().restored(true, Some(1_700_000_030_000));
        assert!(!restored.armed());
        assert!(restored.stops_turn());
        assert_eq!(restored.due_at_ms(), Some(1_700_000_030_000));
        assert_eq!(promised, restored);
        let due = 1_700_000_030_000;
        let different_class = PermissionReviewDeadline::new(
            PermissionToolClass::ReadOnly,
            Duration::from_secs(30),
            PermissionEffect::Deny,
            STANDARD_STOP_REASON,
        )
        .unwrap()
        .restored(true, Some(due));
        assert_ne!(promised, different_class);
        let different_wait =
            PermissionReviewDeadline::stopping(PermissionToolClass::Other, Duration::from_secs(8))
                .promise(due);
        assert_ne!(promised, different_wait);
        let different_effect = PermissionReviewDeadline::new(
            PermissionToolClass::Other,
            Duration::from_secs(30),
            PermissionEffect::Allow,
            STANDARD_STOP_REASON,
        )
        .unwrap()
        .restored(true, Some(due));
        assert_ne!(promised, different_effect);
        let different_reason = PermissionReviewDeadline::new(
            PermissionToolClass::Other,
            Duration::from_secs(30),
            PermissionEffect::Deny,
            STANDARD_DENY_REASON,
        )
        .unwrap()
        .restored(true, Some(due));
        assert_ne!(promised, different_reason);
        let different_stop = PermissionReviewDeadline::new(
            PermissionToolClass::Other,
            Duration::from_secs(30),
            PermissionEffect::Deny,
            STANDARD_STOP_REASON,
        )
        .unwrap()
        .restored(false, Some(due));
        assert_ne!(promised, different_stop);
        let different_due = promised.clone().restored(true, None);
        assert_ne!(promised, different_due);
    }
}
