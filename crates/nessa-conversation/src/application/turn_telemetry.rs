//! Nessa's telemetry for turns: the events a turn's life is told as, in the
//! generic shape `nessa-telemetry` carries.
//!
//! A turn is one span, `nessa.turn`, in a trace of its own named by its
//! execution id, so any part of the gateway can name it without holding it.
//! The execution id is the one name every record of the turn carries: the
//! SDK's `Finished` record names the provider's session, not the
//! conversation.
//!
//! ```text
//! submit (fresh) ──turn_submitted──▶ span starts, with the origin
//!   execution audit: queue_admitted ──▶ point `nessa.turn.admitted` (mode, model, effort)
//!   execution audit: left the queue to run ──▶ point `nessa.turn.dispatched`
//!                    (only a turn that waited in the queue has this record)
//!   execution audit: finished ──▶ span ends, with outcome (`completed`, …, or `failed` and its cause)
//!   execution audit: settled / cancelled / injected before it ran ──▶ span ends, with outcome and cause
//! submit refused before admission ──turn_refused──▶ span ends, outcome `refused`
//! resume: successor queued ──turn_resumed──▶ the successor's span starts, linked
//! ```
//!
//! A turn sent from another turn (an agent's message, a thread's report, a
//! resume, a usage-limit continuation) links to that turn's span. Nothing
//! here carries what anyone wrote: ids, kinds, names and counts only.
//! The names an origin is told by are [`TurnOrigin::kind`] and
//! [`Device::name`](super::Device::name), the same ones its audit record and
//! `read_turn` use.
use super::{SentFrom, TurnOrigin};
use nessa_telemetry::{Event, SpanRef};
use std::time::{Duration, SystemTime};

/// The name of a turn's span.
pub const TURN_SPAN: &str = "nessa.turn";

/// The wall-clock instant `unix_ms` milliseconds after the epoch, as an
/// event's time. Callers read their own injected clock.
pub fn at(unix_ms: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_millis(unix_ms)
}

/// The span of turn `execution`.
pub fn turn_span(execution: &str) -> SpanRef {
    SpanRef::new(format!("turn/{execution}"), "turn")
}

/// Turn `execution` was submitted, from `origin` (`None`: it could not be
/// read), sent the way `sent_from` says.
pub fn turn_submitted(
    conversation: &str,
    execution: &str,
    origin: Option<&TurnOrigin>,
    sent_from: Option<&SentFrom>,
    unix_ms: u64,
) -> Event {
    let mut event = Event::span_start(TURN_SPAN, turn_span(execution), at(unix_ms))
        .attr("nessa.conversation.id", conversation.to_owned())
        .attr("nessa.turn.execution_id", execution.to_owned())
        .attr(
            "nessa.origin.kind",
            origin.map_or("unread", TurnOrigin::kind),
        );
    if let Some(origin) = origin {
        event = match origin {
            TurnOrigin::Person { device } => event.attr("nessa.origin.device", device.name()),
            TurnOrigin::Grant { grant, device } => event
                .attr("nessa.origin.grant", grant.clone())
                .attr("nessa.origin.device", device.name()),
            TurnOrigin::Agent { thread, execution } => event
                .attr("nessa.origin.thread", thread.clone())
                .link(turn_span(execution)),
            TurnOrigin::Resumed { execution } | TurnOrigin::Continued { execution } => {
                event.link(turn_span(execution))
            }
            TurnOrigin::Event { source } => event.attr("nessa.origin.event_source", source.clone()),
            TurnOrigin::App { server, tool } => event
                .attr("nessa.origin.app.server", server.clone())
                .attr("nessa.origin.app.tool", tool.clone()),
            TurnOrigin::AutoPick | TurnOrigin::Internal => event,
        };
    }
    if let Some(sent) = sent_from {
        event = event
            .attr("nessa.client", sent.client.clone())
            .attr("nessa.spoken", sent.spoken);
    }
    event
}

/// Turn `execution` was refused before the agent took it.
pub fn turn_refused(execution: &str, unix_ms: u64) -> Event {
    Event::span_end(TURN_SPAN, turn_span(execution), at(unix_ms))
        .attr("nessa.turn.outcome", "refused")
}

/// Something happened to turn `execution`, named `name`, with nothing else
/// to say yet; the caller adds attributes.
pub fn turn_point(execution: &str, name: &'static str, unix_ms: u64) -> Event {
    Event::point(name, at(unix_ms)).within(turn_span(execution))
}

/// Turn `execution` ended at `stage` because of `cause` (the execution
/// audit's own names for both).
pub fn turn_ended(
    execution: &str,
    stage: &'static str,
    cause: &'static str,
    unix_ms: u64,
) -> Event {
    Event::span_end(TURN_SPAN, turn_span(execution), at(unix_ms))
        .attr("nessa.turn.outcome", stage)
        .attr("nessa.turn.cause", cause)
}

/// `successor` was queued to continue turn `source`, which `cause` cut off.
pub fn turn_resumed(
    conversation: &str,
    source: &str,
    successor: &str,
    cause: &'static str,
    unix_ms: u64,
) -> Event {
    Event::span_start(TURN_SPAN, turn_span(successor), at(unix_ms))
        .link(turn_span(source))
        .attr("nessa.conversation.id", conversation.to_owned())
        .attr("nessa.turn.execution_id", successor.to_owned())
        .attr("nessa.origin.kind", "resumed")
        .attr("nessa.turn.resumed_after", cause)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::Device;
    use nessa_telemetry::{Shape, Value};

    fn attribute<'a>(event: &'a Event, key: &str) -> Option<&'a Value> {
        event
            .attributes()
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| value)
    }

    #[test]
    fn a_submitted_turn_says_its_origin_and_links_the_turn_it_came_from() {
        let relayed = turn_submitted(
            "c",
            "7",
            Some(&TurnOrigin::Agent {
                thread: "p".into(),
                execution: "3".into(),
            }),
            None,
            1_000,
        );
        assert_eq!(
            relayed.shape(),
            &Shape::SpanStart {
                span: turn_span("7"),
                parent: None,
                links: vec![turn_span("3")],
            }
        );
        assert_eq!(
            attribute(&relayed, "nessa.origin.kind"),
            Some(&"agent".into())
        );
        assert_eq!(relayed.at(), at(1_000));

        let spoken = turn_submitted(
            "c",
            "8",
            Some(&TurnOrigin::Person {
                device: Device::Computer,
            }),
            Some(&SentFrom {
                client: "nessa-browser".into(),
                spoken: true,
            }),
            0,
        );
        assert_eq!(
            attribute(&spoken, "nessa.origin.device"),
            Some(&"computer".into())
        );
        assert_eq!(
            attribute(&spoken, "nessa.client"),
            Some(&"nessa-browser".into())
        );
        assert_eq!(attribute(&spoken, "nessa.spoken"), Some(&true.into()));

        let unread = turn_submitted("c", "9", None, None, 0);
        assert_eq!(
            attribute(&unread, "nessa.origin.kind"),
            Some(&"unread".into())
        );
    }

    #[test]
    fn a_resumed_turn_starts_its_own_span_linked_to_the_one_it_continues() {
        let event = turn_resumed("c", "1", "1-r1", "interrupted", 5);
        let Shape::SpanStart { span, links, .. } = event.shape() else {
            panic!("a successor starts a span")
        };
        assert_eq!(span, &turn_span("1-r1"));
        assert_eq!(links, &vec![turn_span("1")]);
    }
}
