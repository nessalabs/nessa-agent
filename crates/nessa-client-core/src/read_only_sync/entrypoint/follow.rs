//! `follow` lines on stdout, one JSON object per line in arrival order:
//! `subscribed`, `pass` (its `report` is the `sync-records` object), `view`
//! for each view whose cursor moved, and a final `ended`. Refusals before
//! subscribing are not follow lines.
use super::online::{gateway_failure, subscription_ended, write};
use super::CommandError;
use crate::read_only_sync::application::follow::{
    End, EndReason, FollowEvents, OutputLost, Subscribed, Trigger, ViewCursor,
};
use serde_json::{json, Value};
use std::io::Write;

/// The follow loop's lines, written to the supplied output. `enrollment` is
/// the pinned status the run was admitted under.
pub(crate) struct FollowLines<'a> {
    output: &'a mut dyn Write,
    enrollment: Value,
    subscription: Option<String>,
}
impl<'a> FollowLines<'a> {
    pub(crate) fn new(output: &'a mut dyn Write, enrollment: Value) -> Self {
        Self {
            output,
            enrollment,
            subscription: None,
        }
    }
    fn line(&mut self, value: Value) -> Result<(), OutputLost> {
        write(value, self.output).map_err(|_| OutputLost)
    }
    /// The final line. `recheck` is the pinned status asked after an
    /// authority refusal, or `null`. Only a clean end succeeds.
    pub(crate) fn end(&mut self, end: End, recheck: Value) -> Result<(), CommandError> {
        let (reason, ended) = match end.reason {
            EndReason::PassesExhausted => ("passesExhausted", None),
            EndReason::Idle => ("idle", None),
            EndReason::SubscribeRefused => ("subscribeRefused", None),
            EndReason::Unauthorized => ("unauthorized", None),
            EndReason::Unavailable => ("unavailable", None),
            EndReason::PassFailed => ("passFailed", None),
            EndReason::SubscriptionEnded(reason) => {
                ("subscriptionEnded", Some(subscription_ended(reason)))
            }
            EndReason::ConnectionClosed => ("connectionClosed", None),
        };
        // A refused end names both: how the subscription ended, and the
        // typed refusal it carried.
        let cause = end.cause.map(gateway_failure);
        let mut line = json!({"kind":"ended","reason":reason,"cause":cause,"recheck":recheck});
        if let Some(ended) = ended {
            line["ended"] = ended;
        }
        write(line, self.output)?;
        if end.clean() {
            Ok(())
        } else {
            Err(CommandError::OnlineRefused)
        }
    }
}
impl FollowEvents<Value> for FollowLines<'_> {
    fn subscribed(&mut self, subscribed: &Subscribed) -> Result<(), OutputLost> {
        self.subscription = Some(subscribed.subscription.clone());
        let line = json!({"kind":"subscribed","subscriptionId":subscribed.subscription,
            "connectionOperation":subscribed.operation.to_string(),"enrollment":self.enrollment});
        self.line(line)
    }
    fn pass(&mut self, trigger: Trigger, report: &Value) -> Result<(), OutputLost> {
        let trigger = match trigger {
            Trigger::Recheck => "recheck",
            Trigger::View => "view",
            Trigger::Incomplete => "incomplete",
            Trigger::Preparing => "preparing",
        };
        self.line(json!({"kind":"pass","trigger":trigger,"report":report}))
    }
    fn view(&mut self, cursor: &ViewCursor, during_pass: bool) -> Result<(), OutputLost> {
        let line = json!({"kind":"view","subscriptionId":self.subscription,
            "cursor":{"incarnation":cursor.incarnation,"position":cursor.position},
            "duringPass":during_pass});
        self.line(line)
    }
}
