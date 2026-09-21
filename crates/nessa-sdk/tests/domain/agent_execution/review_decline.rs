//! A refusal must always be recordable, including the refusal of a frame that
//! could not be read — so these check what a decline keeps, not whether it
//! succeeds.
use super::*;

#[test]
fn a_readable_name_is_kept_with_its_reason() {
    for (tool, reason) in [
        ("Monitor", ReviewDeclineReason::ToolNotReviewable),
        ("mcp__other__shell", ReviewDeclineReason::ToolNotReviewable),
        ("WebSearch", ReviewDeclineReason::UnusableOptions),
        ("Read", ReviewDeclineReason::UnreadableRequest),
    ] {
        let decline = ReviewDecline::new(Some(tool), reason);
        assert_eq!(decline.declared(), Some(tool));
        assert_eq!(decline.reason(), reason);
        assert!(decline.named());
    }
}

#[test]
fn an_unreadable_name_leaves_the_reason_standing_and_retains_nothing() {
    // A frame that could not be read as a request may not carry a readable name
    // either. Each of these is refused as a name while the decline survives:
    // the reason is what a reader needs, and it is still there.
    let oversized = "M".repeat(129);
    let frame_sized = "M".repeat(1024 * 1024);
    for tool in [
        None,
        Some(""),
        Some(oversized.as_str()),
        Some(frame_sized.as_str()),
        Some("two\nlines"),
        Some("null\0byte"),
        Some("bell\u{7}"),
        Some("delete\u{7f}"),
    ] {
        let decline = ReviewDecline::new(tool, ReviewDeclineReason::UnreadableRequest);
        assert_eq!(decline.declared(), None, "retained {tool:?}");
        assert!(!decline.named());
        assert_eq!(decline.reason(), ReviewDeclineReason::UnreadableRequest);
    }
}

#[test]
fn the_retained_name_is_bounded_at_the_limit_not_past_it() {
    let at_limit = "M".repeat(128);
    let over_limit = "M".repeat(129);
    assert_eq!(
        ReviewDecline::new(Some(&at_limit), ReviewDeclineReason::ToolNotReviewable).declared(),
        Some(at_limit.as_str())
    );
    assert!(!ReviewDecline::new(Some(&over_limit), ReviewDeclineReason::ToolNotReviewable).named());
    // Multibyte text is bounded by the bytes retained, not by character count,
    // and a name at the limit in characters but over it in bytes is refused.
    let multibyte = "é".repeat(64);
    assert_eq!(multibyte.len(), 128);
    assert!(ReviewDecline::new(Some(&multibyte), ReviewDeclineReason::ToolNotReviewable).named());
    let over = "é".repeat(65);
    assert!(!ReviewDecline::new(Some(&over), ReviewDeclineReason::ToolNotReviewable).named());
}

#[test]
fn a_decline_is_not_equal_across_different_reasons_for_the_same_tool() {
    // The reason is part of the evidence, not a label on it: two refusals of
    // the same tool for different reasons are different facts.
    assert_ne!(
        ReviewDecline::new(Some("Monitor"), ReviewDeclineReason::ToolNotReviewable),
        ReviewDecline::new(Some("Monitor"), ReviewDeclineReason::UnusableOptions)
    );
}
