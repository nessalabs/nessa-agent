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

/// Which reviews share a deadline. This is policy, not the provider's display
/// category: a kind is mapped into a class once, at the review.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionToolClass {
    /// Inspects, and does not change the workspace or leave the machine.
    ReadOnly,
    /// Everything else, including a review whose kind was not named.
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

/// One class's wait, the outcome taken when it ends, and the reason recorded
/// with that outcome. The reason is what a person and the audit are shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionReviewDeadline {
    class: PermissionToolClass,
    wait: Duration,
    effect: PermissionEffect,
    reason: Box<str>,
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
        })
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

    /// Why that outcome was taken, shown to a person and recorded.
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

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
                "No human answered in time. This read-only tool was allowed so the turn could continue.",
            )
            .expect("standard read-only deadline"),
            other: PermissionReviewDeadline::new(
                PermissionToolClass::Other,
                Duration::from_secs(30),
                PermissionEffect::Deny,
                "No human answered in time. Continue without this tool or try another approach.",
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
    /// this review offered.
    ///
    /// The configured effect is taken when that option is offered. An allow
    /// whose option is missing becomes a deny when a deny is offered, and the
    /// reason is then the deny sentence, not the allow sentence. When no deny
    /// is offered either, the settlement names a deny and no option: the
    /// review is cancelled instead of being allowed with nothing to select.
    pub fn settlement(
        &self,
        class: PermissionToolClass,
        options: &PermissionOptions,
    ) -> PermissionDeadlineSettlement {
        let configured = self.for_class(class);
        if let Some(option) = offered(options, configured.effect) {
            return PermissionDeadlineSettlement {
                deadline: configured.clone(),
                option: Some(option),
            };
        }
        let deny = self.deny_for(class);
        PermissionDeadlineSettlement {
            deadline: deny,
            option: offered(options, PermissionEffect::Deny),
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
            self.read_only.reason.clone()
        };
        PermissionReviewDeadline {
            class,
            wait: configured.wait,
            effect: PermissionEffect::Deny,
            reason,
        }
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

fn offered(options: &PermissionOptions, effect: PermissionEffect) -> Option<PermissionOptionId> {
    options
        .choices()
        .iter()
        .find(|option| option.decision().effect() == effect)
        .map(|option| option.id().clone())
}

pub(crate) fn checked_deadline_reason(reason: String) -> Result<Box<str>, ExecutionError> {
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
    Ok(reason.into_boxed_str())
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
        for kind in [
            Some(ToolKind::Edit),
            Some(ToolKind::Fetch),
            Some(ToolKind::Execute),
            Some(ToolKind::Other),
            None,
        ] {
            assert_eq!(PermissionToolClass::of(kind), PermissionToolClass::Other);
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

    #[test]
    fn a_missing_allow_keeps_the_read_only_sentence_when_nothing_denies() {
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
        let settled = policy.settlement(
            PermissionToolClass::ReadOnly,
            &options(&[PermissionEffect::Deny]),
        );
        assert_eq!(settled.deadline().effect(), PermissionEffect::Deny);
        assert_eq!(settled.deadline().reason(), "read sentence");
        assert_eq!(settled.deadline().wait(), Duration::from_secs(8));
        assert!(settled.option().is_some());
    }

    #[test]
    fn a_missing_allow_settles_as_a_deny_when_one_is_offered() {
        let policy = PermissionDeadlinePolicy::standard();
        let both = options(&[PermissionEffect::Allow, PermissionEffect::Deny]);
        let allow = policy.settlement(PermissionToolClass::ReadOnly, &both);
        assert_eq!(allow.deadline().effect(), PermissionEffect::Allow);
        assert!(allow.option().is_some());
        let deny_only = options(&[PermissionEffect::Deny]);
        let fallen = policy.settlement(PermissionToolClass::ReadOnly, &deny_only);
        assert_eq!(fallen.deadline().effect(), PermissionEffect::Deny);
        assert!(fallen
            .deadline()
            .reason()
            .contains("Continue without this tool"));
        assert!(fallen.option().is_some());
        let allow_only = options(&[PermissionEffect::Allow]);
        let cancelled = policy.settlement(PermissionToolClass::Other, &allow_only);
        assert_eq!(cancelled.deadline().effect(), PermissionEffect::Deny);
        assert!(cancelled.option().is_none());
    }
}
