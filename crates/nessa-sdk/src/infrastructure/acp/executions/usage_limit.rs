//! Classify a usage limit from ACP fields the pinned harnesses actually send.
//!
//! Claude ACP 0.76.0 forwards `usage_update._meta["_claude/rateLimit"]` from
//! Claude Agent SDK 0.3.257. Codex ACP 1.12.0 puts `codexErrorInfo` on the
//! prompt error for `usageLimitExceeded` and does not forward `resetsAt`.
//! OpenCode is not classified.

use crate::application::agent_execution::agents::{
    ProviderDiagnostic, UsageLimit, UsageLimitKind, UsageLimitProvider,
};
use crate::infrastructure::acp::profile::UsageLimitWire;
use serde_json::Value;

/// Unix milliseconds for 2024-01-01T00:00:00Z.
const EARLIEST_RESET_MS: u64 = 1_704_067_200_000;
/// Unix milliseconds for 2100-01-01T00:00:00Z.
const LATEST_RESET_MS: u64 = 4_102_444_800_000;
/// A number below this is Unix seconds. At and above it, Unix milliseconds.
const MILLISECOND_THRESHOLD: u64 = 100_000_000_000;

const WINDOW_PREFIXES: &[&str] = &["You've hit your", "You've reached your"];
/// The rest of SDK 0.3.257 `USAGE_LIMIT_ERROR_PREFIXES`. These need a person.
const ACTION_PREFIXES: &[&str] = &[
    "You're out of usage credits",
    "Your org is out of usage · add funds to continue",
    "Your org is out of usage · contact your admin",
    "Your seat type doesn't include usage credits",
    "Your seat type doesn't include usage",
    "Your usage allocation has been disabled by your admin",
    "Your group's usage limit is set to $0",
    "Fable 5 requires usage credits",
    "You're out of extra usage",
    "Your seat type doesn't include extra usage",
];

const USER_ACTION_REASONS: &[&str] = &[
    "overage_not_provisioned",
    "org_level_disabled",
    "org_level_disabled_until",
    "out_of_credits",
    "seat_tier_level_disabled",
    "member_level_disabled",
    "seat_tier_zero_credit_limit",
    "group_zero_credit_limit",
    "member_zero_credit_limit",
    "org_service_level_disabled",
    "no_limits_configured",
];

/// What the worker saw on this turn before the prompt settled.
#[derive(Clone, Debug, Default)]
pub(in crate::infrastructure::acp) struct ObservedRateLimit {
    /// Claude `_claude/rateLimit`, when a usage update carried one.
    pub claude: Option<Value>,
}

/// The RPC error plus any rate-limit object observed on the same turn.
pub(in crate::infrastructure::acp) struct UsageLimitFrame<'a> {
    pub code: i64,
    pub message: Option<&'a str>,
    pub data: Option<&'a Value>,
    pub observed: Option<&'a ObservedRateLimit>,
}

/// `Some` when `wire` is the harness this frame belongs to and the frame is a
/// usage limit for that harness. `None` is every other profile, including
/// OpenCode and a custom ACP agent. Auth (`-32000`) is not asked here.
pub(in crate::infrastructure::acp) fn classify(
    wire: Option<UsageLimitWire>,
    frame: UsageLimitFrame<'_>,
) -> Option<UsageLimit> {
    let diagnostic = frame.message.map(ProviderDiagnostic::new);
    match wire {
        Some(UsageLimitWire::Codex) => codex_limit(frame.data, diagnostic),
        Some(UsageLimitWire::Claude) => claude_limit(
            frame.code,
            frame.message,
            frame.data,
            frame.observed,
            diagnostic,
        ),
        None => None,
    }
}

fn codex_limit(data: Option<&Value>, diagnostic: Option<ProviderDiagnostic>) -> Option<UsageLimit> {
    let data = data?;
    if truth(data.get("willRetry")) {
        return None;
    }
    let info = data.get("codexErrorInfo")?;
    let Value::String(kind) = info else {
        return None;
    };
    if kind != "usageLimitExceeded" {
        return None;
    }
    Some(UsageLimit::unknown(
        UsageLimitProvider::Codex,
        UsageLimitKind::Unspecified,
        diagnostic,
    ))
}

fn claude_limit(
    code: i64,
    message: Option<&str>,
    data: Option<&Value>,
    observed: Option<&ObservedRateLimit>,
    diagnostic: Option<ProviderDiagnostic>,
) -> Option<UsageLimit> {
    let error_kind = data
        .and_then(|value| value.get("errorKind"))
        .and_then(Value::as_str);
    let meta = observed.and_then(|observed| observed.claude.as_ref());
    if error_kind == Some("billing_error") || error_kind == Some("account_on_hold") {
        return Some(UsageLimit::user_action(
            UsageLimitProvider::Claude,
            UsageLimitKind::Unspecified,
            diagnostic,
        ));
    }
    if error_kind == Some("rate_limit") {
        return Some(merge_claude(meta, diagnostic));
    }
    if error_kind.is_some() || code != -32603 {
        return None;
    }
    let message = message?;
    if ACTION_PREFIXES
        .iter()
        .any(|prefix| message.starts_with(prefix))
    {
        return Some(UsageLimit::user_action(
            UsageLimitProvider::Claude,
            UsageLimitKind::Unspecified,
            diagnostic,
        ));
    }
    if WINDOW_PREFIXES
        .iter()
        .any(|prefix| message.starts_with(prefix))
    {
        return Some(merge_claude(meta, diagnostic));
    }
    None
}

fn merge_claude(meta: Option<&Value>, diagnostic: Option<ProviderDiagnostic>) -> UsageLimit {
    let Some(meta) = meta else {
        return UsageLimit::unknown(
            UsageLimitProvider::Claude,
            UsageLimitKind::Unspecified,
            diagnostic,
        );
    };
    if user_action_meta(meta) {
        return UsageLimit::user_action(
            UsageLimitProvider::Claude,
            kind_of(meta.get("rateLimitType").and_then(Value::as_str)),
            diagnostic,
        );
    }
    let mut candidates = Vec::new();
    if meta.get("status").and_then(Value::as_str) == Some("rejected") {
        candidates.push((
            kind_of(meta.get("rateLimitType").and_then(Value::as_str)),
            meta.get("resetsAt"),
        ));
    }
    if meta.get("overageStatus").and_then(Value::as_str) == Some("rejected") {
        candidates.push((UsageLimitKind::Overage, meta.get("overageResetsAt")));
    }
    let parsed: Vec<(UsageLimitKind, u64)> = candidates
        .into_iter()
        .filter_map(|(kind, instant)| absolute_reset_ms(instant).map(|instant| (kind, instant)))
        .collect();
    if let Some((kind, resets_at_ms)) = parsed.into_iter().max_by_key(|(_, instant)| *instant) {
        return UsageLimit::timed_reset(UsageLimitProvider::Claude, kind, resets_at_ms, diagnostic);
    }
    if meta.get("status").and_then(Value::as_str) == Some("rejected")
        || meta.get("overageStatus").and_then(Value::as_str) == Some("rejected")
    {
        return UsageLimit::unknown(
            UsageLimitProvider::Claude,
            kind_of(meta.get("rateLimitType").and_then(Value::as_str)),
            diagnostic,
        );
    }
    UsageLimit::unknown(
        UsageLimitProvider::Claude,
        UsageLimitKind::Unspecified,
        diagnostic,
    )
}

fn user_action_meta(meta: &Value) -> bool {
    meta.get("errorCode").and_then(Value::as_str) == Some("credits_required")
        || meta
            .get("overageDisabledReason")
            .and_then(Value::as_str)
            .is_some_and(|reason| USER_ACTION_REASONS.contains(&reason))
}

fn kind_of(value: Option<&str>) -> UsageLimitKind {
    match value {
        Some("five_hour") => UsageLimitKind::FiveHour,
        Some("seven_day" | "seven_day_overage_included") => UsageLimitKind::SevenDay,
        Some("seven_day_opus" | "seven_day_sonnet") => UsageLimitKind::ModelSpecific,
        Some("overage") => UsageLimitKind::Overage,
        _ => UsageLimitKind::Unspecified,
    }
}

fn absolute_reset_ms(value: Option<&Value>) -> Option<u64> {
    let number = value?
        .as_u64()
        .or_else(|| value?.as_i64().and_then(|value| u64::try_from(value).ok()))?;
    if number == 0 {
        return None;
    }
    let millis = if number < MILLISECOND_THRESHOLD {
        number.checked_mul(1000)?
    } else {
        number
    };
    (EARLIEST_RESET_MS..=LATEST_RESET_MS)
        .contains(&millis)
        .then_some(millis)
}

fn truth(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(value)) => *value,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn frame<'a>(
        code: i64,
        message: Option<&'a str>,
        data: Option<&'a Value>,
        observed: Option<&'a ObservedRateLimit>,
    ) -> UsageLimitFrame<'a> {
        UsageLimitFrame {
            code,
            message,
            data,
            observed,
        }
    }

    fn claude(frame: UsageLimitFrame<'_>) -> Option<UsageLimit> {
        classify(Some(UsageLimitWire::Claude), frame)
    }

    fn codex(frame: UsageLimitFrame<'_>) -> Option<UsageLimit> {
        classify(Some(UsageLimitWire::Codex), frame)
    }

    #[test]
    fn codex_usage_limit_is_unknown_and_keeps_no_invented_reset() {
        let data = json!({"codexErrorInfo": "usageLimitExceeded", "message": "limit"});
        let limit = codex(frame(-32603, Some("limit"), Some(&data), None)).unwrap();
        assert_eq!(limit.provider(), UsageLimitProvider::Codex);
        assert_eq!(
            limit.recovery(),
            crate::application::agent_execution::agents::UsageLimitRecovery::Unknown
        );
        assert_eq!(limit.resets_at_ms(), None);
    }

    #[test]
    fn codex_will_retry_and_other_codes_are_not_usage_limits() {
        for data in [
            json!({"codexErrorInfo": "usageLimitExceeded", "willRetry": true}),
            json!({"codexErrorInfo": "rateLimitExceeded"}),
            json!({"codexErrorInfo": "sessionBudgetExceeded"}),
            json!({"codexErrorInfo": "contextWindowExceeded"}),
            json!({"codexErrorInfo": {"responseTooManyFailedAttempts": {"httpStatusCode": 429}}}),
            json!({"codexErrorInfo": "serverOverloaded"}),
        ] {
            assert!(
                codex(frame(-32603, Some("no"), Some(&data), None)).is_none(),
                "{data}"
            );
        }
    }

    #[test]
    fn claude_rejected_window_keeps_the_later_reset_and_seconds() {
        let meta = json!({
            "status": "rejected",
            "rateLimitType": "five_hour",
            "resetsAt": 1_800_000_000,
            "overageStatus": "rejected",
            "overageResetsAt": 1_800_000_000_000_u64
        });
        let observed = ObservedRateLimit { claude: Some(meta) };
        let data = json!({"errorKind": "rate_limit"});
        let limit = claude(frame(-32603, Some("slow"), Some(&data), Some(&observed))).unwrap();
        assert_eq!(limit.kind(), UsageLimitKind::Overage);
        assert_eq!(limit.resets_at_ms(), Some(1_800_000_000_000));
    }

    #[test]
    fn claude_credits_and_billing_never_keep_a_reset() {
        let meta = json!({
            "status": "rejected",
            "resetsAt": 1_800_000_000_000_u64,
            "errorCode": "credits_required"
        });
        let observed = ObservedRateLimit { claude: Some(meta) };
        let data = json!({"errorKind": "rate_limit"});
        let limit = claude(frame(-32603, Some("buy"), Some(&data), Some(&observed))).unwrap();
        assert_eq!(
            limit.recovery(),
            crate::application::agent_execution::agents::UsageLimitRecovery::UserAction
        );
        assert_eq!(limit.resets_at_ms(), None);

        let billing = json!({"errorKind": "billing_error"});
        let held = json!({"errorKind": "account_on_hold"});
        for data in [&billing, &held] {
            let limit = claude(frame(-32603, Some("pay"), Some(data), Some(&observed))).unwrap();
            assert_eq!(limit.resets_at_ms(), None);
        }
    }

    #[test]
    fn synthetic_window_prefix_can_merge_and_action_prefixes_cannot() {
        let meta = json!({"status": "rejected", "rateLimitType": "seven_day", "resetsAt": 1_800_000_000_000_u64});
        let observed = ObservedRateLimit { claude: Some(meta) };
        let window = claude(frame(
            -32603,
            Some("You've hit your weekly limit"),
            None,
            Some(&observed),
        ))
        .unwrap();
        assert_eq!(window.resets_at_ms(), Some(1_800_000_000_000));
        let action = claude(frame(
            -32603,
            Some("Your org is out of usage · add funds to continue"),
            None,
            Some(&observed),
        ))
        .unwrap();
        assert_eq!(action.resets_at_ms(), None);
        assert!(claude(frame(
            -32603,
            Some("something else failed"),
            None,
            Some(&observed)
        ))
        .is_none());
        assert!(claude(frame(
            -32001,
            Some("You've hit your weekly limit"),
            None,
            None
        ))
        .is_none());
    }

    #[test]
    fn warnings_do_not_become_a_timed_reset_without_a_terminal_signal() {
        let meta = json!({"status": "allowed_warning", "resetsAt": 1_800_000_000_000_u64});
        let observed = ObservedRateLimit { claude: Some(meta) };
        assert!(claude(frame(-32603, Some("ok"), None, Some(&observed))).is_none());
    }

    #[test]
    fn the_published_reset_bounds_are_inclusive() {
        let earliest = json!({
            "status": "rejected",
            "rateLimitType": "five_hour",
            "resetsAt": 1_704_067_200_000_u64
        });
        let latest = json!({
            "status": "rejected",
            "rateLimitType": "five_hour",
            "resetsAt": 4_102_444_800_000_u64
        });
        let past = json!({
            "status": "rejected",
            "rateLimitType": "five_hour",
            "resetsAt": 4_102_444_800_001_u64
        });
        let data = json!({"errorKind": "rate_limit"});
        for (meta, kept) in [
            (earliest, Some(1_704_067_200_000)),
            (latest, Some(4_102_444_800_000)),
        ] {
            let observed = ObservedRateLimit { claude: Some(meta) };
            let limit = claude(frame(-32603, None, Some(&data), Some(&observed))).unwrap();
            assert_eq!(
                limit.recovery(),
                crate::application::agent_execution::agents::UsageLimitRecovery::TimedReset
            );
            assert_eq!(limit.resets_at_ms(), kept);
        }
        let observed = ObservedRateLimit { claude: Some(past) };
        let limit = claude(frame(-32603, None, Some(&data), Some(&observed))).unwrap();
        assert_eq!(
            limit.recovery(),
            crate::application::agent_execution::agents::UsageLimitRecovery::Unknown
        );
        assert_eq!(limit.resets_at_ms(), None);
    }

    #[test]
    fn a_rejected_window_without_a_parseable_reset_stays_unknown() {
        let meta = json!({"status": "rejected", "rateLimitType": "five_hour", "resetsAt": 1});
        let observed = ObservedRateLimit { claude: Some(meta) };
        let data = json!({"errorKind": "rate_limit"});
        let limit = claude(frame(-32603, None, Some(&data), Some(&observed))).unwrap();
        assert_eq!(
            limit.recovery(),
            crate::application::agent_execution::agents::UsageLimitRecovery::Unknown
        );
        assert_eq!(limit.kind(), UsageLimitKind::FiveHour);
    }

    #[test]
    fn another_profiles_matching_text_is_not_a_usage_limit() {
        let codex_data = json!({"codexErrorInfo": "usageLimitExceeded"});
        let claude_data = json!({"errorKind": "rate_limit"});
        let codex_frame = || frame(-32603, Some("limit"), Some(&codex_data), None);
        let claude_frame = || {
            frame(
                -32603,
                Some("You've hit your weekly limit"),
                Some(&claude_data),
                None,
            )
        };
        assert!(classify(None, codex_frame()).is_none());
        assert!(classify(None, claude_frame()).is_none());
        assert!(claude(codex_frame()).is_none());
        assert!(codex(claude_frame()).is_none());
        assert!(codex(codex_frame()).is_some());
        assert!(claude(claude_frame()).is_some());
    }
}
