//! One receiver's grant subscription. The log is written in the same
//! transaction as the grant. This task reads that log.
//!
//! A frame that would not fit is cut on a grant boundary and marked
//! incomplete. That prefix does not advance `after`. `grants.page` continues
//! it while the log head is the one the snapshot pinned. A later grant
//! replaces the set.

use super::delivery::{Deadline, Outgoing, SubscriptionDeliveries};
use super::target::{Cursor, Run, Target, DELIVERY_TIMEOUT};
use crate::conversation::application::{
    GrantLogEntry, GrantSnapshotEntry, GrantStream, ResourceKind, ShareGrants,
    EMPTY_GRANT_INCARNATION,
};
use crate::product::{
    read_access::{self, paired_receiver},
    socket::{failure, success},
};
use crate::timer::clock;
use nessa_protocol::product::generated::{
    product_event, ConversationSubscribeResult, ConversationSubscriptionEndReason,
    ConversationSubscriptionEnded, ConversationSubscriptionErrorCode, GrantFrameEntry,
    GrantStreamCursor, GrantsChanged, MAX_PAYLOAD_BYTES,
};
use nessa_protocol::protocol::{EventFrame, OutgoingMessage};
use std::sync::Arc;
use tokio::sync::{mpsc, OwnedSemaphorePermit};

/// A `grants.page` for the open subscription.
pub(super) struct GrantPage {
    pub snapshot_id: String,
    #[allow(dead_code)]
    pub after: Option<Cursor>,
}

struct Pin {
    id: u64,
    replaces: bool,
    incarnation: String,
    head: u64,
    /// Entries not yet sent.
    pending: Vec<GrantFrameEntry>,
    /// Log position already covered. Incremental pages advance it.
    covered: u64,
    incremental: bool,
}

/// Serve `grants.subscribe` until the connection drops it.
pub(super) async fn run(run: Run) {
    let Some(mut pages) = run.pages else {
        return;
    };
    let Run {
        state,
        session,
        id,
        request,
        target,
        deliveries,
        slot,
        reply_by,
        ..
    } = run;
    let Target::Grants { after } = target else {
        return;
    };
    let refuse = |code: &str, slot: Arc<OwnedSemaphorePermit>| {
        drop(deliveries.offer(
            &id,
            Outgoing::Reply(failure(&request, code)),
            Deadline::Last(clock::now() + DELIVERY_TIMEOUT),
            Some(slot),
        ));
    };
    let receiver = match paired_receiver(&state, &session).await {
        Ok(Some(receiver)) => receiver,
        Ok(None) => return refuse("forbidden", slot),
        Err(code) => return refuse(code, slot),
    };
    if let Err(code) = read_access::admit_paired_read(&state, &session, "grants.subscribe").await {
        return refuse(code, slot);
    }
    let Some((_, _, grants)) = state.reads.passive_read.clone() else {
        return refuse("conversations_not_configured", slot);
    };
    let wake = grants.grant_log_notify();
    let mut reply = Some(slot);
    let mut pin: Option<Pin> = None;
    let mut snapshots = 0_u64;
    let mut announced = false;
    // Incarnation and head last delivered complete. A wake for another
    // receiver does not send this one another snapshot.
    let mut delivered: Option<(String, u64)> = None;
    // The next frame continues `pin` only when `grants.page` asked. A log
    // wake drops the pin and sends a replacement snapshot.
    let mut continue_pin = false;
    loop {
        // Register before the read. A commit during the read or the send
        // marks this waiter, and the wait below returns at once.
        let notified = wake.notified();
        tokio::pin!(notified);
        if let Err(code) =
            read_access::admit_paired_read(&state, &session, "grants.subscribe").await
        {
            end_subscription(
                &deliveries,
                &id,
                ConversationSubscriptionEndReason::Refused,
                Some(code.to_owned()),
            )
            .await;
            return;
        }
        let stream = match grants.grant_stream(&receiver).await {
            Ok(stream) => stream,
            Err(_) => {
                end_subscription(
                    &deliveries,
                    &id,
                    ConversationSubscriptionEndReason::SourceClosed,
                    None,
                )
                .await;
                return;
            }
        };
        if let Some(code) = resume_refusal(&stream, after.as_ref()) {
            match reply.take() {
                Some(slot) => return refuse(code, slot),
                None => {
                    end_subscription(
                        &deliveries,
                        &id,
                        ConversationSubscriptionEndReason::Refused,
                        Some(code.to_owned()),
                    )
                    .await;
                    return;
                }
            }
        }
        let delivered_this = delivered.as_ref().is_some_and(|(incarnation, head)| {
            incarnation == &stream.incarnation && *head == stream.head
        });
        let pin_same = pin
            .as_ref()
            .is_some_and(|open| open.incarnation == stream.incarnation && open.head == stream.head);
        // This receiver's log did not move. A page the follower asked for
        // still continues; any other wake waits.
        if announced && !continue_pin && (delivered_this || pin_same) {
            let Some(paged) = wait_registered(&mut notified, &mut pages).await else {
                return;
            };
            continue_pin = page_continues(
                paged.as_ref().map(|page| page.snapshot_id.as_str()),
                pin.as_ref().map(|open| open.id),
            );
            continue;
        }
        let resume = after.as_ref().filter(|_| !continue_pin || pin.is_none());
        if !continue_pin {
            pin = None;
        }
        let changed = if let Some(open) = pin.as_mut() {
            if open.incarnation != stream.incarnation || open.head != stream.head {
                match fresh_frame(grants.as_ref(), &receiver, &stream, None, &mut snapshots).await {
                    Ok((next, rest)) => {
                        pin = rest;
                        next
                    }
                    Err(()) => {
                        end_subscription(
                            &deliveries,
                            &id,
                            ConversationSubscriptionEndReason::SourceClosed,
                            None,
                        )
                        .await;
                        return;
                    }
                }
            } else {
                let frame = take_page(open);
                if open.pending.is_empty() {
                    pin = None;
                }
                frame
            }
        } else {
            match fresh_frame(grants.as_ref(), &receiver, &stream, resume, &mut snapshots).await {
                Ok((next, rest)) => {
                    pin = rest;
                    next
                }
                Err(()) => {
                    end_subscription(
                        &deliveries,
                        &id,
                        ConversationSubscriptionEndReason::SourceClosed,
                        None,
                    )
                    .await;
                    return;
                }
            }
        };
        if let Some(slot) = reply.take() {
            let answer = success(
                &request,
                &ConversationSubscribeResult {
                    subscription_id: id.clone(),
                },
            );
            if deliveries
                .offer(
                    &id,
                    Outgoing::Reply(answer),
                    Deadline::Reply(reply_by),
                    Some(slot),
                )
                .await
                .is_err()
            {
                return;
            }
        }
        let complete = changed.complete;
        if !send_changed(&deliveries, &id, changed).await {
            return;
        }
        if complete {
            delivered = Some((stream.incarnation.clone(), stream.head));
        }
        announced = true;
        let Some(paged) = wait_registered(&mut notified, &mut pages).await else {
            return;
        };
        continue_pin = page_continues(
            paged.as_ref().map(|page| page.snapshot_id.as_str()),
            pin.as_ref().map(|open| open.id),
        );
    }
}

/// A `grants.page` continues the open pin only when it names that pin.
/// A wake, or a page for another snapshot, does not.
fn page_continues(snapshot_id: Option<&str>, pin_id: Option<u64>) -> bool {
    snapshot_id.is_some_and(|id| pin_id.is_some_and(|pin| pin > 0 && pin.to_string() == id))
}

fn resume_refusal(stream: &GrantStream, after: Option<&Cursor>) -> Option<&'static str> {
    let after = after?;
    let position = after.position;
    if after.incarnation != stream.incarnation {
        return Some(ConversationSubscriptionErrorCode::IncarnationChanged.as_str());
    }
    if position > stream.head {
        return Some(ConversationSubscriptionErrorCode::CursorAhead.as_str());
    }
    if stream
        .oldest
        .is_some_and(|oldest| position.saturating_add(1) < oldest)
    {
        return Some(ConversationSubscriptionErrorCode::GrantLogFloor.as_str());
    }
    None
}

fn caught_up(stream: &GrantStream, after: Option<&Cursor>) -> bool {
    after.is_some_and(|cursor| {
        cursor.incarnation == stream.incarnation && cursor.position == stream.head
    })
}

async fn fresh_frame(
    grants: &dyn ShareGrants,
    receiver: &str,
    stream: &GrantStream,
    after: Option<&Cursor>,
    snapshots: &mut u64,
) -> Result<(GrantsChanged, Option<Pin>), ()> {
    if caught_up(stream, after) {
        return Ok((
            GrantsChanged {
                subscription_id: String::new(),
                cursor: stream_cursor(stream),
                complete: true,
                replaces: false,
                snapshot_id: None,
                grants: Vec::new(),
            },
            None,
        ));
    }
    if let Some(after) = after {
        let position = after.position;
        let mut entries = grants
            .grant_log_after(receiver, position)
            .await
            .map_err(|_| ())?;
        entries.retain(|entry| entry.position <= stream.head);
        let entries = conversation_log(entries);
        if continues_cursor(&entries, stream.head, position) {
            return Ok(incremental_frame(&entries, stream, position, snapshots));
        }
    }
    let entries = conversation_snapshot(grants.grant_snapshot(receiver).await.map_err(|_| ())?);
    *snapshots = snapshots.saturating_add(1);
    let mut pin = Pin {
        id: *snapshots,
        replaces: true,
        incarnation: stream.incarnation.clone(),
        head: stream.head,
        pending: entries.iter().map(snapshot_entry).collect(),
        covered: 0,
        incremental: false,
    };
    let frame = take_page(&mut pin);
    let rest = (!pin.pending.is_empty()).then_some(pin);
    Ok((frame, rest))
}

/// The head and the log are two reads. If the log was trimmed between them,
/// the records right after the cursor are gone, and the caller sends a
/// replacement snapshot instead of a delta with a hole in it.
fn continues_cursor(entries: &[GrantLogEntry], head: u64, position: u64) -> bool {
    entries.first().map_or(head == position, |first| {
        first.position == position.saturating_add(1)
    })
}

fn incremental_frame(
    entries: &[GrantLogEntry],
    stream: &GrantStream,
    position: u64,
    snapshots: &mut u64,
) -> (GrantsChanged, Option<Pin>) {
    *snapshots = snapshots.saturating_add(1);
    let mut pin = Pin {
        id: *snapshots,
        replaces: false,
        incarnation: stream.incarnation.clone(),
        head: stream.head,
        pending: entries.iter().map(log_entry).collect(),
        covered: position,
        incremental: true,
    };
    let frame = take_page(&mut pin);
    let rest = (!pin.pending.is_empty()).then_some(pin);
    (frame, rest)
}

/// Move a fitting prefix of `pin` into one frame. A complete frame's cursor
/// is the log head. An incomplete frame keeps `after` unmoved by using the
/// position already covered, and names `snapshotId` when the pin has one.
fn take_page(pin: &mut Pin) -> GrantsChanged {
    let keep = fitting_prefix(pin);
    let complete = keep == pin.pending.len();
    let grants: Vec<GrantFrameEntry> = pin.pending.drain(..keep).collect();
    let position = if complete {
        pin.head
    } else if pin.incremental {
        pin.covered
            .saturating_add(u64::try_from(grants.len()).unwrap_or(0))
    } else {
        0
    };
    if pin.incremental && !complete {
        pin.covered = position;
    }
    let incarnation = if pin.incarnation.is_empty() {
        EMPTY_GRANT_INCARNATION.to_owned()
    } else {
        pin.incarnation.clone()
    };
    GrantsChanged {
        subscription_id: String::new(),
        cursor: GrantStreamCursor {
            incarnation,
            position: position.to_string(),
        },
        complete,
        replaces: pin.replaces,
        snapshot_id: (pin.id > 0).then(|| pin.id.to_string()),
        grants,
    }
}

/// How many leading entries fit one frame. Each entry is serialized once.
/// The whole set is measured as a complete frame; a cut uses the incomplete
/// envelope, which grows by each entry and a comma. One entry is sent even
/// when it is already over the bound.
fn fitting_prefix(pin: &Pin) -> usize {
    let total = pin.pending.len();
    if total == 0 || event_len(&preview(pin, total, true)) <= MAX_PAYLOAD_BYTES as usize {
        return total;
    }
    if total == 1 {
        return 1;
    }
    let mut size = event_len(&preview(pin, 0, false));
    let mut keep = 0;
    for (index, entry) in pin.pending.iter().take(total - 1).enumerate() {
        let entry_len = serde_json::to_string(entry)
            .map(|text| text.len())
            .unwrap_or(usize::MAX);
        let extra = entry_len.saturating_add(usize::from(index > 0));
        if keep > 0 && size.saturating_add(extra) > MAX_PAYLOAD_BYTES as usize {
            break;
        }
        size = size.saturating_add(extra);
        keep += 1;
    }
    keep.max(1)
}

fn preview(pin: &Pin, keep: usize, complete: bool) -> GrantsChanged {
    GrantsChanged {
        subscription_id: String::new(),
        cursor: stream_cursor_parts(
            &pin.incarnation,
            if complete { pin.head } else { pin.covered },
        ),
        complete,
        replaces: pin.replaces,
        snapshot_id: (pin.id > 0).then(|| pin.id.to_string()),
        grants: pin.pending.iter().take(keep).map(copy_entry).collect(),
    }
}

fn stream_cursor(stream: &GrantStream) -> GrantStreamCursor {
    stream_cursor_parts(&stream.incarnation, stream.head)
}

fn stream_cursor_parts(incarnation: &str, position: u64) -> GrantStreamCursor {
    GrantStreamCursor {
        incarnation: if incarnation.is_empty() {
            EMPTY_GRANT_INCARNATION.to_owned()
        } else {
            incarnation.to_owned()
        },
        position: position.to_string(),
    }
}

fn copy_entry(entry: &GrantFrameEntry) -> GrantFrameEntry {
    GrantFrameEntry {
        conversation_id: entry.conversation_id.clone(),
        absent: entry.absent,
        capabilities: entry.capabilities.clone(),
        policy_revision: entry.policy_revision.clone(),
    }
}

/// Conversation commits only. A directory record stays in the log and the
/// snapshot, and is not published as a conversation id.
fn conversation_log(entries: Vec<GrantLogEntry>) -> Vec<GrantLogEntry> {
    entries
        .into_iter()
        .filter(|entry| entry.resource_kind == ResourceKind::Conversation)
        .collect()
}

fn conversation_snapshot(entries: Vec<GrantSnapshotEntry>) -> Vec<GrantSnapshotEntry> {
    entries
        .into_iter()
        .filter(|entry| entry.resource_kind == ResourceKind::Conversation)
        .collect()
}

fn log_entry(entry: &GrantLogEntry) -> GrantFrameEntry {
    GrantFrameEntry {
        conversation_id: entry.conversation_id.clone(),
        absent: entry.capabilities.is_none(),
        capabilities: entry.capabilities.clone(),
        policy_revision: entry.policy_revision.map(|revision| revision.to_string()),
    }
}

fn snapshot_entry(entry: &GrantSnapshotEntry) -> GrantFrameEntry {
    GrantFrameEntry {
        conversation_id: entry.conversation_id.clone(),
        absent: false,
        capabilities: Some(entry.capabilities.clone()),
        policy_revision: Some(entry.policy_revision.to_string()),
    }
}

fn event_len(changed: &GrantsChanged) -> usize {
    let Ok(payload) = serde_json::to_value(changed) else {
        return usize::MAX;
    };
    let Ok(frame) = EventFrame::push(product_event::GRANTS_CHANGED, &payload, u64::MAX, u64::MAX)
    else {
        return usize::MAX;
    };
    OutgoingMessage::Event(frame)
        .to_wire_text()
        .map(|text| text.len())
        .unwrap_or(usize::MAX)
}

async fn send_changed(
    deliveries: &SubscriptionDeliveries,
    id: &str,
    mut changed: GrantsChanged,
) -> bool {
    changed.subscription_id = id.to_owned();
    let Ok(payload) = serde_json::to_value(&changed) else {
        return false;
    };
    deliveries
        .offer(
            id,
            Outgoing::Event {
                name: product_event::GRANTS_CHANGED,
                payload,
            },
            Deadline::None,
            None,
        )
        .await
        .is_ok()
}

async fn end_subscription(
    deliveries: &SubscriptionDeliveries,
    id: &str,
    reason: ConversationSubscriptionEndReason,
    code: Option<String>,
) {
    let ended = ConversationSubscriptionEnded {
        subscription_id: id.to_owned(),
        reason,
        code,
        last_event_cursor: None,
        last_delivered: None,
    };
    let Ok(payload) = serde_json::to_value(&ended) else {
        return;
    };
    let _ = deliveries
        .offer(
            id,
            Outgoing::Event {
                name: product_event::GRANTS_SUBSCRIPTION_ENDED,
                payload,
            },
            Deadline::Last(clock::now() + DELIVERY_TIMEOUT),
            None,
        )
        .await;
}

/// `Some(None)` when the log woke, `Some(page)` when a page arrived, `None`
/// when both sources are gone.
async fn wait_registered(
    notified: &mut std::pin::Pin<&mut tokio::sync::futures::Notified<'_>>,
    pages: &mut mpsc::Receiver<GrantPage>,
) -> Option<Option<GrantPage>> {
    tokio::select! {
        _ = notified.as_mut() => Some(None),
        page = pages.recv() => page.map(Some),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        continues_cursor, conversation_log, conversation_snapshot, copy_entry, event_len,
        fitting_prefix, page_continues, preview, take_page, GrantLogEntry, GrantSnapshotEntry, Pin,
        MAX_PAYLOAD_BYTES,
    };
    use crate::conversation::application::ResourceKind;
    use nessa_protocol::product::generated::GrantFrameEntry;

    fn logged(position: u64) -> GrantLogEntry {
        GrantLogEntry {
            position,
            conversation_id: format!("conversation-{position}"),
            resource_kind: ResourceKind::Conversation,
            capabilities: None,
            policy: None,
            policy_revision: None,
        }
    }

    #[test]
    fn a_directory_record_is_not_replayed_or_snapshotted_as_a_conversation() {
        let directory = GrantLogEntry {
            position: 2,
            conversation_id: "directory-resource".into(),
            resource_kind: ResourceKind::Directory,
            capabilities: Some("read".into()),
            policy: None,
            policy_revision: Some(4),
        };
        let conversation = logged(1);
        let replay = conversation_log(vec![conversation.clone(), directory]);
        assert_eq!(replay, vec![conversation]);
        let snapshot = conversation_snapshot(vec![
            GrantSnapshotEntry {
                conversation_id: "directory-resource".into(),
                resource_kind: ResourceKind::Directory,
                capabilities: "read".into(),
                policy: None,
                policy_revision: 4,
            },
            GrantSnapshotEntry {
                conversation_id: "conversation-1".into(),
                resource_kind: ResourceKind::Conversation,
                capabilities: "read".into(),
                policy: None,
                policy_revision: 1,
            },
        ]);
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].resource_kind, ResourceKind::Conversation);
    }

    #[test]
    fn a_delta_with_a_trimmed_start_is_not_sent() {
        assert!(continues_cursor(&[logged(4), logged(5)], 5, 3));
        // Records 4 and 5 were trimmed after the head was read.
        assert!(!continues_cursor(&[logged(6)], 5, 3));
        assert!(!continues_cursor(&[], 5, 3));
    }

    #[test]
    fn a_page_continues_only_the_open_pin() {
        assert!(page_continues(Some("4"), Some(4)));
        assert!(!page_continues(None, Some(4)));
        assert!(!page_continues(Some("4"), None));
        assert!(!page_continues(Some("4"), Some(0)));
        assert!(!page_continues(Some("9"), Some(4)));
    }

    fn entry(index: usize) -> GrantFrameEntry {
        GrantFrameEntry {
            conversation_id: format!("conversation-{index:08}"),
            absent: false,
            capabilities: Some("read,comment,drive".to_owned()),
            policy_revision: Some("18446744073709551615".to_owned()),
        }
    }

    fn pin(count: usize) -> Pin {
        Pin {
            id: 4,
            replaces: true,
            incarnation: "incarnation".to_owned(),
            head: 9,
            pending: (0..count).map(entry).collect(),
            covered: 0,
            incremental: false,
        }
    }

    #[test]
    fn an_entry_grows_the_frame_by_its_own_text_and_a_comma() {
        let open = pin(2);
        let empty = event_len(&preview(&open, 0, false));
        let first = serde_json::to_string(&open.pending[0]).unwrap().len();
        let second = serde_json::to_string(&open.pending[1]).unwrap().len();
        assert_eq!(event_len(&preview(&open, 1, false)), empty + first);
        assert_eq!(
            event_len(&preview(&open, 2, false)),
            empty + first + 1 + second
        );
    }

    #[test]
    fn a_snapshot_page_is_a_fitting_prefix_measured_once_per_grant() {
        let mut small = pin(2);
        let frame = take_page(&mut small);
        assert!(frame.complete);
        assert!(small.pending.is_empty());
        assert!(event_len(&frame) <= MAX_PAYLOAD_BYTES as usize);

        let count = 4_000;
        let mut large = pin(count);
        let chosen = fitting_prefix(&large);
        assert!(chosen < count);
        assert!(chosen > 1);
        let frame = take_page(&mut large);
        assert!(!frame.complete);
        assert_eq!(frame.grants.len(), chosen);
        assert_eq!(large.pending.len(), count - chosen);
        assert!(event_len(&preview(&pin(count), chosen, false)) <= MAX_PAYLOAD_BYTES as usize);
        assert!(event_len(&preview(&pin(count), chosen + 1, false)) > MAX_PAYLOAD_BYTES as usize);
        let again = take_page(&mut large);
        assert_eq!(again.grants.len(), chosen);
        assert_eq!(
            again.grants[0].conversation_id,
            copy_entry(&entry(chosen)).conversation_id
        );
    }
}
