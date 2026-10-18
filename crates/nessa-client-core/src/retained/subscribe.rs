//! A linked device's live read of one conversation.
//!
//! `RetainedCache::read` still copies records into the private cache. This
//! feed does not. It opens `conversation.subscribe` on a new pinned session
//! to the same gateway and yields each `conversation.view` the gateway sends. Dropping
//! the feed drops the socket, which ends the subscription. `close` also
//! sends `conversation.unsubscribe` when the socket is still up.
//!
//! A view that arrives in the same turn as the subscribe reply is kept and
//! comes out of [`ConversationFeed::next_event`] first. The budget and
//! [`crate::retained::ReadStop`] bound every wait. When either cuts the feed
//! short, the caller subscribes again with the last cursor it applied.
use super::{failure, ReadFailure, ReadStop, ReaderAccess, RetainedCache, Stopping};
use crate::read_only_sync::application::device::asks_status;
use crate::read_only_sync::application::{GatewayError, SubscriptionCode, SubscriptionRefusal};
use crate::read_only_sync::infrastructure::gateway::Session;
use crate::read_only_sync::infrastructure::gateway::{subscription_refusal, SubscriptionDelivery};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::product::generated::{
    ConversationSubscribeParams, ConversationView, ConversationViewCursor,
};
use nessa_protocol::product_contract::generated::ConversationErrorCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Why a subscription did not deliver. `failure` is how the caller acts on
/// it. `asks_status` is set only when the gateway refused this credential's
/// authority, which is when pinned status is asked again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SubscribeError {
    failure: ReadFailure,
    authority: bool,
}
impl SubscribeError {
    /// How the caller acts on this refusal.
    pub fn failure(self) -> ReadFailure {
        self.failure
    }
    /// The gateway refused this credential, so the caller asks pinned status
    /// again. A conversation this device was not granted does not.
    pub fn asks_status(self) -> bool {
        self.authority
    }
}

/// One frame from a conversation subscription, in arrival order.
pub enum FeedEvent {
    /// The view that replaces what the device shows for this conversation.
    View {
        /// Where the fold had read when this view was taken.
        cursor: ConversationViewCursor,
        /// What `conversation.read` would return for a follower that opens
        /// no agent.
        view: Box<ConversationView>,
    },
    /// The subscription ended. No later [`ConversationFeed::next_event`] delivers
    /// a view. `last_delivered` is where a lagging feed resumes.
    Ended {
        /// `lagging`, `refused`, `source_closed`, or `too_large`.
        reason: String,
        /// Why a `refused` end was refused, when the gateway sent a code.
        code: Option<String>,
        /// The cursor of the last view written, or the `after` this feed was
        /// opened with when none was written.
        last_delivered: Option<ConversationViewCursor>,
    },
}
impl FeedEvent {
    /// A terminal end as the caller acts on it. A view is not a failure.
    pub fn terminal_failure(&self) -> Option<SubscribeError> {
        let Self::Ended { reason, code, .. } = self else {
            return None;
        };
        Some(ended_failure(reason, code.as_deref()))
    }
}

/// The live `conversation.subscribe` feed for one conversation. It is not
/// `Send`: the pinned session stays on the thread that opened it. Dropping
/// it drops the socket. The server ends the subscription when the socket
/// closes, including when `close` cannot finish `conversation.unsubscribe`.
pub struct ConversationFeed {
    session: Session,
    stop: Arc<ReadStop>,
    ending: Arc<Stopping>,
    closed: bool,
    /// An end was already delivered. A later `next` does not read again.
    terminal: bool,
}
impl ConversationFeed {
    /// The next view, or the subscription's end once every kept view has
    /// been taken. A view kept with the subscribe reply comes out first.
    /// Named apart from [`Iterator::next`](std::iter::Iterator::next): a feed
    /// is not an iterator, because a wait can fail.
    ///
    /// # Errors
    ///
    /// `Stopped` when the stop was asked. `Unreachable` when the budget or
    /// the socket ended the wait, so the caller subscribes again from the
    /// last cursor it applied. `Refused` when the gateway refuses the
    /// subscribe; [`SubscribeError::asks_status`] is set only for this
    /// credential's authority. `CursorAhead` when `after` is past the
    /// gateway head: follow again with no cursor, and do not ask status.
    /// `SubscriptionCapacity` when this connection's subscription limit is
    /// full: follow again once it stops following something, and do not ask
    /// status.
    /// `Protocol` when a frame is outside the session, including a `next`
    /// after the feed has ended or been closed.
    pub fn next_event(&mut self) -> Result<FeedEvent, SubscribeError> {
        if self.closed || self.terminal {
            return Err(self.protocol());
        }
        if let Err(error) = self.session.begin() {
            return Err(self.mapped(error));
        }
        let delivery = self.session.next_delivery();
        let _ = self.session.finish();
        match delivery {
            Ok(SubscriptionDelivery::View(viewed)) => Ok(FeedEvent::View {
                cursor: viewed.cursor,
                view: Box::new(viewed.view),
            }),
            Ok(SubscriptionDelivery::Ended(ended)) => {
                self.terminal = true;
                Ok(FeedEvent::Ended {
                    reason: ended.reason.as_str().to_owned(),
                    code: ended.code,
                    last_delivered: ended.last_delivered,
                })
            }
            Err(error) => Err(self.mapped(error)),
        }
    }
    /// Stop this subscription. A frame still queued is dropped with it.
    /// `unknown_subscription` and a socket that is already gone are success:
    /// the server drops the subscription when the connection closes. Any other
    /// refusal is returned and the socket is dropped, because this feed will
    /// not retry `close` or read another frame, and the server ends the
    /// subscription with the connection.
    ///
    /// # Errors
    ///
    /// A typed refusal other than `unknown_subscription`, mapped the same
    /// way as [`Self::next_event`]. `forbidden` and `unauthorized` set
    /// [`SubscribeError::asks_status`].
    pub fn close(&mut self) -> Result<(), SubscribeError> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        let outcome = (|| {
            self.session.begin()?;
            let unsubscribed = self.session.unsubscribe();
            let _ = self.session.finish();
            unsubscribed
        })();
        self.stop.release();
        match outcome {
            Ok(())
            | Err(
                GatewayError::Busy
                | GatewayError::Transport
                | GatewayError::TimedOut
                | GatewayError::Cancelled
                | GatewayError::Closed(_),
            )
            | Err(GatewayError::Subscription(SubscriptionRefusal::Subscription(
                SubscriptionCode::Unknown,
            ))) => Ok(()),
            Err(error) => {
                self.session.disconnect();
                Err(self.mapped(error))
            }
        }
    }
    fn protocol(&self) -> SubscribeError {
        SubscribeError {
            failure: ReadFailure::Protocol,
            authority: false,
        }
    }
    fn mapped(&self, error: GatewayError) -> SubscribeError {
        let authority = asks_status(error);
        let failure = if self.stop.is_stopped() {
            ReadFailure::Stopped
        } else if self.ending.spent()
            && matches!(
                subscribe_failure(error),
                ReadFailure::Stopped | ReadFailure::Unreachable
            )
        {
            ReadFailure::Unreachable
        } else {
            subscribe_failure(error)
        };
        SubscribeError {
            failure,
            authority: authority && failure == ReadFailure::Refused,
        }
    }
}
impl Drop for ConversationFeed {
    fn drop(&mut self) {
        self.stop.release();
    }
}

impl RetainedCache {
    /// Follow `conversation` on a new pinned session until `budget` or
    /// `stop`. `after` is the last cursor the caller applied, or `None` to
    /// start from what the gateway holds now. The call shape of
    /// [`RetainedCache::read`] is unchanged: this is the live read beside it,
    /// and it does not advance a physical checkpoint.
    ///
    /// # Errors
    ///
    /// The same connection failures as a read. A conversation this device
    /// was not granted is `Refused` without [`SubscribeError::asks_status`].
    /// `forbidden` and `unauthorized` are `Refused` with it set.
    pub fn subscribe(
        &self,
        access: ReaderAccess<'_>,
        conversation: &ConversationId,
        after: Option<ConversationViewCursor>,
        stop: &Arc<ReadStop>,
        budget: Duration,
    ) -> Result<ConversationFeed, SubscribeError> {
        let ending = Arc::new(Stopping {
            stop: stop.clone(),
            clock: self.clock.clone(),
            until: self
                .clock
                .elapsed_ms()
                .saturating_add(u64::try_from(budget.as_millis()).unwrap_or(u64::MAX)),
        });
        let offline = Arc::new(AtomicBool::new(false));
        // A dial that succeeds registers its socket on `stop` before the
        // handshake. Either failure drops that clone with the session.
        let mut session = match self.open_session(&access, &ending, &offline) {
            Ok(session) => session,
            Err(ReadFailure::Unreachable) if offline.load(Ordering::SeqCst) => {
                stop.release();
                return Err(SubscribeError {
                    failure: ReadFailure::Away,
                    authority: false,
                });
            }
            Err(failure) => {
                stop.release();
                return Err(as_subscribe(failure));
            }
        };
        let params = ConversationSubscribeParams {
            conversation_id: conversation.to_string(),
            after,
        };
        if let Err(error) = session.begin() {
            stop.release();
            return Err(classify(&ending, stop, error));
        }
        let subscribed = session.subscribe(&params);
        let _ = session.finish();
        if let Err(error) = subscribed {
            stop.release();
            return Err(classify(&ending, stop, error));
        }
        Ok(ConversationFeed {
            session,
            stop: stop.clone(),
            ending,
            closed: false,
            terminal: false,
        })
    }
}

fn as_subscribe(failure: ReadFailure) -> SubscribeError {
    SubscribeError {
        failure,
        authority: failure == ReadFailure::Refused,
    }
}

fn classify(ending: &Stopping, stop: &ReadStop, error: GatewayError) -> SubscribeError {
    let authority = asks_status(error);
    let failure = if stop.is_stopped() {
        ReadFailure::Stopped
    } else if ending.spent()
        && matches!(
            subscribe_failure(error),
            ReadFailure::Stopped | ReadFailure::Unreachable
        )
    {
        ReadFailure::Unreachable
    } else {
        subscribe_failure(error)
    };
    SubscribeError {
        failure,
        authority: authority && failure == ReadFailure::Refused,
    }
}

fn subscribe_failure(error: GatewayError) -> ReadFailure {
    match error {
        GatewayError::Subscription(refusal) => match refusal {
            SubscriptionRefusal::Forbidden | SubscriptionRefusal::Unauthorized => {
                ReadFailure::Refused
            }
            SubscriptionRefusal::Subscription(SubscriptionCode::CursorAhead) => {
                ReadFailure::CursorAhead
            }
            SubscriptionRefusal::Subscription(SubscriptionCode::Capacity) => {
                ReadFailure::SubscriptionCapacity
            }
            SubscriptionRefusal::Subscription(_) => ReadFailure::Refused,
            SubscriptionRefusal::Conversation(
                ConversationErrorCode::ConversationNotFound
                | ConversationErrorCode::ConversationDeleted,
            ) => ReadFailure::Refused,
            SubscriptionRefusal::Conversation(ConversationErrorCode::TemporarilyUnavailable) => {
                ReadFailure::Unreachable
            }
            SubscriptionRefusal::Conversation(_) => ReadFailure::Protocol,
        },
        other => failure(other),
    }
}

fn ended_failure(reason: &str, code: Option<&str>) -> SubscribeError {
    match reason {
        "lagging" | "source_closed" => SubscribeError {
            failure: ReadFailure::Unreachable,
            authority: false,
        },
        "refused" => {
            let error = code.map_or(GatewayError::Protocol, |code| {
                subscription_refusal(code.to_owned())
            });
            SubscribeError {
                failure: subscribe_failure(error),
                authority: asks_status(error),
            }
        }
        _ => SubscribeError {
            failure: ReadFailure::Protocol,
            authority: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_granted_is_refused_without_asking_status() {
        let error = subscribe_failure(GatewayError::Subscription(
            SubscriptionRefusal::Conversation(ConversationErrorCode::ConversationNotFound),
        ));
        assert_eq!(error, ReadFailure::Refused);
        assert!(!asks_status(GatewayError::Subscription(
            SubscriptionRefusal::Conversation(ConversationErrorCode::ConversationNotFound),
        )));
    }

    #[test]
    fn authority_refusals_ask_status_and_cursor_refusals_do_not() {
        assert!(asks_status(GatewayError::Subscription(
            SubscriptionRefusal::Forbidden
        )));
        assert!(asks_status(GatewayError::Subscription(
            SubscriptionRefusal::Unauthorized
        )));
        assert!(!asks_status(GatewayError::Subscription(
            SubscriptionRefusal::Subscription(SubscriptionCode::CursorAhead)
        )));
        assert_eq!(
            subscribe_failure(GatewayError::Subscription(
                SubscriptionRefusal::Subscription(SubscriptionCode::CursorAhead)
            )),
            ReadFailure::CursorAhead
        );
        assert_eq!(
            subscribe_failure(GatewayError::Subscription(
                SubscriptionRefusal::Subscription(SubscriptionCode::Duplicate)
            )),
            ReadFailure::Refused
        );
        assert!(!asks_status(GatewayError::Subscription(
            SubscriptionRefusal::Subscription(SubscriptionCode::Capacity)
        )));
        assert_eq!(
            subscribe_failure(GatewayError::Subscription(
                SubscriptionRefusal::Subscription(SubscriptionCode::Capacity)
            )),
            ReadFailure::SubscriptionCapacity
        );
    }

    #[test]
    fn an_ended_subscription_uses_the_same_refusal() {
        let ended = ended_failure("refused", Some("conversation_not_found"));
        assert_eq!(ended.failure(), ReadFailure::Refused);
        assert!(!ended.asks_status());
        let revoked = ended_failure("refused", Some("forbidden"));
        assert!(revoked.asks_status());
        let ahead = ended_failure("refused", Some("cursor_ahead"));
        assert_eq!(ahead.failure(), ReadFailure::CursorAhead);
        assert!(!ahead.asks_status());
        let capacity = ended_failure("refused", Some("subscription_capacity"));
        assert_eq!(capacity.failure(), ReadFailure::SubscriptionCapacity);
        assert!(!capacity.asks_status());
        assert_eq!(
            ended_failure("lagging", None).failure(),
            ReadFailure::Unreachable
        );
        assert_eq!(
            ended_failure("too_large", None).failure(),
            ReadFailure::Protocol
        );
    }
}
