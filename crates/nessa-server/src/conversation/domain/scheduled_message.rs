//! Whether a usage-limit continuation is accepted, moved, or cancelled.
//!
//! The schedule store records the row. This function is the only place that
//! decides which of those happens. A past reset does not create a row. A row
//! that was accepted while the reset was still ahead stays for delivery once
//! it is due.

/// A usage limit that is still the newest settled result of its conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservedUsageLimit {
    /// The turn that ended in the limit.
    pub causal_execution: String,
    /// Absolute Unix milliseconds, only when the recovery is a timed reset.
    pub resets_at_ms: Option<u64>,
    /// The conversation's agent when the limit was saved.
    pub agent: String,
    /// The conversation's model when the limit was saved.
    pub model: String,
}

/// An open schedule: accepted, or claimed for delivery and not finished.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenContinuation {
    pub causal_execution: String,
    pub due_at_ms: u64,
    pub delivering: bool,
    pub agent: String,
    pub model: String,
}

/// Why a schedule row changes. The audit record copies this text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContinuationCause {
    /// A provider limit named a future reset while the preference was on.
    ProviderLimit,
    /// The person turned the preference on for a turn already blocked.
    PreferenceEnabled,
    /// The person turned the preference off.
    PreferenceDisabled,
    /// A later turn replaced the blocked one.
    Superseded,
    /// The conversation's agent is no longer the one that was blocked.
    AgentChanged,
    /// The conversation's model is no longer the one that was blocked.
    ModelChanged,
    /// The same turn named a later reset.
    ResetMoved,
    /// The blocked turn is no longer a timed usage limit.
    NoLongerBlocked,
}

impl ContinuationCause {
    /// Stable audit text.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProviderLimit => "provider_limit",
            Self::PreferenceEnabled => "preference_enabled",
            Self::PreferenceDisabled => "preference_disabled",
            Self::Superseded => "superseded",
            Self::AgentChanged => "agent_changed",
            Self::ModelChanged => "model_changed",
            Self::ResetMoved => "reset_moved",
            Self::NoLongerBlocked => "no_longer_blocked",
        }
    }
}

/// What the store should do for one conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContinuationPlan {
    /// Leave the open row, or leave the absence of one.
    Hold,
    /// Accept one row due at this instant.
    Accept {
        due_at_ms: u64,
        cause: ContinuationCause,
    },
    /// The open row's due time moves. It stays scheduled.
    MoveDue { due_at_ms: u64 },
    /// A claimed row's reset moved later. Release the claim and move the due time.
    Defer { due_at_ms: u64 },
    /// The open row ends without a submit.
    Cancel { cause: ContinuationCause },
    /// The open row ends, and a different turn is accepted.
    Switch {
        cancel: ContinuationCause,
        due_at_ms: u64,
        cause: ContinuationCause,
    },
}

/// How one invocation bears on the usage-limit schedule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvocationLimit {
    /// No saved result. A queued, running, or stopped turn.
    Unsettled,
    /// The saved error is a usage limit.
    UsageLimit,
    /// A saved result that is not a usage limit.
    Settled,
}

/// The index of the usage limit that may schedule, when the newest invocation
/// is that limit. A later unsettled turn, or a later result that is not the
/// limit, clears it. An open row is then cancelled as no longer blocked.
pub fn observed_limit_index(invocations: &[InvocationLimit]) -> Option<usize> {
    match invocations.last() {
        Some(InvocationLimit::UsageLimit) => Some(invocations.len() - 1),
        _ => None,
    }
}

/// `preference_on` is the gateway preference. `now_ms` is the gateway clock.
/// `observed` is set only when [`observed_limit_index`] names a usage limit.
/// `enabled_by_person` selects the accept cause when the preference change is
/// what made the row eligible.
pub fn plan_continuation(
    preference_on: bool,
    now_ms: u64,
    current_agent: &str,
    current_model: &str,
    observed: Option<&ObservedUsageLimit>,
    open: Option<&OpenContinuation>,
    enabled_by_person: bool,
) -> ContinuationPlan {
    if let Some(open) = open {
        if !preference_on {
            return ContinuationPlan::Cancel {
                cause: ContinuationCause::PreferenceDisabled,
            };
        }
        if open.agent != current_agent {
            return ContinuationPlan::Cancel {
                cause: ContinuationCause::AgentChanged,
            };
        }
        if open.model != current_model {
            return ContinuationPlan::Cancel {
                cause: ContinuationCause::ModelChanged,
            };
        }
    }

    let future = observed.and_then(|observed| {
        observed
            .resets_at_ms
            .filter(|resets_at_ms| *resets_at_ms > now_ms)
            .map(|due_at_ms| (observed, due_at_ms))
    });

    let Some((observed, due_at_ms)) = future else {
        // A timed reset that is no longer ahead does not create a row. A row
        // accepted while it was ahead is delivered: a due time still in the
        // future is brought to the clock so the claim can take it.
        return match open {
            Some(open)
                if observed.is_some_and(|observed| {
                    observed.causal_execution == open.causal_execution
                        && observed.resets_at_ms.is_some()
                }) =>
            {
                if open.due_at_ms <= now_ms || open.delivering {
                    ContinuationPlan::Hold
                } else {
                    ContinuationPlan::MoveDue { due_at_ms: now_ms }
                }
            }
            Some(_) => ContinuationPlan::Cancel {
                cause: ContinuationCause::NoLongerBlocked,
            },
            None => ContinuationPlan::Hold,
        };
    };

    if !preference_on {
        return ContinuationPlan::Hold;
    }
    if observed.agent != current_agent || observed.model != current_model {
        return match open {
            Some(_) => ContinuationPlan::Cancel {
                cause: if observed.agent != current_agent {
                    ContinuationCause::AgentChanged
                } else {
                    ContinuationCause::ModelChanged
                },
            },
            None => ContinuationPlan::Hold,
        };
    }

    let cause = if enabled_by_person {
        ContinuationCause::PreferenceEnabled
    } else {
        ContinuationCause::ProviderLimit
    };
    match open {
        None => ContinuationPlan::Accept { due_at_ms, cause },
        Some(open) if open.causal_execution != observed.causal_execution => {
            ContinuationPlan::Switch {
                cancel: ContinuationCause::Superseded,
                due_at_ms,
                cause,
            }
        }
        Some(open) if open.delivering && due_at_ms > open.due_at_ms => {
            ContinuationPlan::Defer { due_at_ms }
        }
        Some(open) if !open.delivering && due_at_ms != open.due_at_ms => {
            ContinuationPlan::MoveDue { due_at_ms }
        }
        Some(_) => ContinuationPlan::Hold,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observed(resets_at_ms: u64) -> ObservedUsageLimit {
        ObservedUsageLimit {
            causal_execution: "causal".into(),
            resets_at_ms: Some(resets_at_ms),
            agent: "claude".into(),
            model: "opus".into(),
        }
    }

    fn open(due_at_ms: u64, delivering: bool) -> OpenContinuation {
        OpenContinuation {
            causal_execution: "causal".into(),
            due_at_ms,
            delivering,
            agent: "claude".into(),
            model: "opus".into(),
        }
    }

    #[test]
    fn a_future_reset_with_the_preference_on_is_accepted_once() {
        let observed = observed(50);
        assert_eq!(
            plan_continuation(true, 10, "claude", "opus", Some(&observed), None, false),
            ContinuationPlan::Accept {
                due_at_ms: 50,
                cause: ContinuationCause::ProviderLimit,
            }
        );
        assert_eq!(
            plan_continuation(
                true,
                10,
                "claude",
                "opus",
                Some(&observed),
                Some(&open(50, false)),
                false
            ),
            ContinuationPlan::Hold
        );
    }

    #[test]
    fn the_preference_off_cancels_and_does_not_accept() {
        let observed = observed(50);
        assert_eq!(
            plan_continuation(
                false,
                10,
                "claude",
                "opus",
                Some(&observed),
                Some(&open(50, false)),
                false
            ),
            ContinuationPlan::Cancel {
                cause: ContinuationCause::PreferenceDisabled,
            }
        );
        assert_eq!(
            plan_continuation(false, 10, "claude", "opus", Some(&observed), None, false),
            ContinuationPlan::Hold
        );
    }

    #[test]
    fn a_past_reset_does_not_schedule_and_a_due_row_stays() {
        let past = observed(10);
        assert_eq!(
            plan_continuation(true, 10, "claude", "opus", Some(&past), None, false),
            ContinuationPlan::Hold
        );
        assert_eq!(
            plan_continuation(
                true,
                11,
                "claude",
                "opus",
                Some(&past),
                Some(&open(10, false)),
                false
            ),
            ContinuationPlan::Hold
        );
    }

    #[test]
    fn a_later_reset_moves_the_due_time_and_releases_a_claim() {
        let later = observed(80);
        assert_eq!(
            plan_continuation(
                true,
                10,
                "claude",
                "opus",
                Some(&later),
                Some(&open(50, false)),
                false
            ),
            ContinuationPlan::MoveDue { due_at_ms: 80 }
        );
        assert_eq!(
            plan_continuation(
                true,
                10,
                "claude",
                "opus",
                Some(&later),
                Some(&open(50, true)),
                false
            ),
            ContinuationPlan::Defer { due_at_ms: 80 }
        );
    }

    #[test]
    fn a_different_turn_replaces_the_open_row() {
        let mut next = observed(90);
        next.causal_execution = "next".into();
        assert_eq!(
            plan_continuation(
                true,
                10,
                "claude",
                "opus",
                Some(&next),
                Some(&open(50, false)),
                true
            ),
            ContinuationPlan::Switch {
                cancel: ContinuationCause::Superseded,
                due_at_ms: 90,
                cause: ContinuationCause::PreferenceEnabled,
            }
        );
    }

    #[test]
    fn a_later_turn_clears_the_limit_even_when_it_has_no_result() {
        use InvocationLimit::{Settled, Unsettled, UsageLimit};
        assert_eq!(observed_limit_index(&[UsageLimit, Unsettled]), None);
        assert_eq!(observed_limit_index(&[UsageLimit, Settled]), None);
        assert_eq!(observed_limit_index(&[Settled, UsageLimit]), Some(1));
        assert_eq!(observed_limit_index(&[]), None);
        assert_eq!(
            plan_continuation(
                true,
                10,
                "claude",
                "opus",
                None,
                Some(&open(50, false)),
                false
            ),
            ContinuationPlan::Cancel {
                cause: ContinuationCause::NoLongerBlocked,
            }
        );
    }

    #[test]
    fn a_cleared_limit_or_a_changed_model_cancels() {
        assert_eq!(
            plan_continuation(
                true,
                10,
                "claude",
                "opus",
                None,
                Some(&open(50, false)),
                false
            ),
            ContinuationPlan::Cancel {
                cause: ContinuationCause::NoLongerBlocked,
            }
        );
        let observed = observed(50);
        assert_eq!(
            plan_continuation(
                true,
                10,
                "claude",
                "sonnet",
                Some(&observed),
                Some(&open(50, false)),
                false
            ),
            ContinuationPlan::Cancel {
                cause: ContinuationCause::ModelChanged,
            }
        );
    }
}
