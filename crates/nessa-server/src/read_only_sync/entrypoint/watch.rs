//! `watch` lines on stdout, one JSON object per line in arrival order:
//! `registered`, `pass` (its `report` is the `sync-records` object), `hint`,
//! and a final `ended` (committed change watches, rows W2–W15). Refusals
//! before registration are not watch lines (rows W1, W16).
use super::online::{gateway_failure, watch_ended, write};
use super::CommandError;
use crate::read_only_sync::application::watch::{
    End, EndReason, OutputLost, Registered, Trigger, WatchEvents,
};
use serde_json::{json, Value};
use std::io::Write;

/// The watch's lines, written to the supplied output. `enrollment` is the
/// pinned status the run was admitted under.
pub(crate) struct WatchLines<'a> {
    output: &'a mut dyn Write,
    enrollment: Value,
    watch: Option<String>,
}
impl<'a> WatchLines<'a> {
    pub(crate) fn new(output: &'a mut dyn Write, enrollment: Value) -> Self {
        Self {
            output,
            enrollment,
            watch: None,
        }
    }
    fn line(&mut self, value: Value) -> Result<(), OutputLost> {
        write(value, self.output).map_err(|_| OutputLost)
    }
    /// The final line. `recheck` is the pinned status asked after an
    /// authority refusal, or `null`. Only a clean end succeeds.
    pub(crate) fn end(&mut self, end: End, recheck: Value) -> Result<(), CommandError> {
        let (reason, cause) = match end.reason {
            EndReason::PassesExhausted => ("passesExhausted", None),
            EndReason::Idle => ("idle", None),
            EndReason::RegistrationRefused => ("registrationRefused", None),
            EndReason::Unauthorized => ("unauthorized", None),
            EndReason::Unavailable => ("unavailable", None),
            EndReason::PassFailed => ("passFailed", None),
            EndReason::WatchEnded(reason) => ("watchEnded", Some(watch_ended(reason))),
            EndReason::ConnectionClosed => ("connectionClosed", None),
        };
        let cause = cause.or_else(|| end.cause.map(gateway_failure));
        write(
            json!({"kind":"ended","reason":reason,"cause":cause,"recheck":recheck}),
            self.output,
        )?;
        if end.clean() {
            Ok(())
        } else {
            Err(CommandError::OnlineRefused)
        }
    }
}
impl WatchEvents<Value> for WatchLines<'_> {
    fn registered(&mut self, registered: &Registered) -> Result<(), OutputLost> {
        self.watch = Some(registered.watch.clone());
        let line = json!({"kind":"registered","watchId":registered.watch,
            "connectionOperation":registered.operation.to_string(),"enrollment":self.enrollment});
        self.line(line)
    }
    fn pass(&mut self, trigger: Trigger, report: &Value) -> Result<(), OutputLost> {
        let trigger = match trigger {
            Trigger::Recheck => "recheck",
            Trigger::Hint => "hint",
            Trigger::Incomplete => "incomplete",
        };
        self.line(json!({"kind":"pass","trigger":trigger,"report":report}))
    }
    fn hint(&mut self, during_pass: bool) -> Result<(), OutputLost> {
        let line = json!({"kind":"hint","watchId":self.watch,"duringPass":during_pass});
        self.line(line)
    }
}
