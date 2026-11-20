//! Where a turn's input came from, followed back to the person who typed it
//! (`read_turn`).
//!
//! Every saved turn already says who sent its input: the SDK record's sender
//! and initiation, written once when the turn was admitted, and never changed
//! after. A message one agent sends another names the turn it was sent from,
//! so a chain of relays leads back, one saved turn at a time, to the input
//! that started it. This reads that chain. It keeps no copy and decides
//! nothing: the records are the one authority, and the agent reading the
//! chain decides whether the words it ends at cover what it was asked.
//!
//! ```text
//! child turn ──sender: agent {thread A, turn 7}──▶ A's turn 7
//!   A's turn 7 ──sender: person, owner──▶ ends: { origin: "person", said }
//! ```
use super::{ConversationCaller, ConversationError, ConversationService};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::{
    application::agent_execution::sessions::InvocationRecord,
    domain::agent_execution::{executions::Initiator, prompts::MessageSender},
};

/// The most turns one read follows. A relay chain longer than this is
/// answered as cut, with the turns read so far.
pub const MAX_TURN_HOPS: usize = 8;

/// The most earlier messages of the person one read adds for context.
pub const MAX_EARLIER_TURNS: usize = 3;

/// How one turn's input arrived, as its saved record says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TurnOrigin {
    /// The conversation's owner sent it from one of their own surfaces.
    Person,
    /// Someone the conversation is shared with sent it, under this grant.
    Grant(String),
    /// An agent sent it from the turn this links to. A thread's report back to
    /// its sender links to the reporting thread's turn.
    Agent { thread: String, execution: String },
    /// The turn picks up the one this links to, in the same thread, after a
    /// restart or a provider failure.
    Resumed { execution: String },
    /// An app sent it on the person's behalf, from this tool call.
    App { server: String, tool: String },
    /// The gateway wrote it itself, such as a background command's outcome.
    Internal,
}

/// One saved turn of a chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnReading {
    pub thread: ConversationId,
    pub execution: String,
    pub origin: TurnOrigin,
    /// The input exactly as saved.
    pub said: String,
}

/// What one `read_turn` answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnChain {
    /// The turn asked about first, then each turn its input came from.
    pub turns: Vec<TurnReading>,
    pub end: ChainEnd,
    /// The person's messages before the one the chain starts at, oldest
    /// first, when asked for and the chain reached a person.
    pub earlier: Vec<TurnReading>,
}

/// Why a chain stopped before an input nobody relayed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChainEnd {
    /// The last turn's input was not relayed: it is where the chain starts.
    Start,
    /// It went on past [`MAX_TURN_HOPS`].
    Cut,
    /// The next turn is in a thread this caller cannot read, or is not saved.
    Unreadable,
}

impl ConversationService {
    /// Read turn `execution` of `thread`, then each turn its input came from,
    /// until one whose input nobody relayed. Every thread read is checked
    /// against `caller` as a `conversation.read` would be; the first one it
    /// may not read ends the chain there, as [`ChainEnd::Unreadable`].
    ///
    /// # Errors
    /// [`ConversationError::NotFound`] when `caller` may not read `thread`,
    /// or `thread` has no saved turn `execution`.
    ///
    /// With `earlier` above zero, a chain that reached the turn the person
    /// started it with also brings that thread's messages from the person
    /// before it, up to `earlier` (at most [`MAX_EARLIER_TURNS`]), and the
    /// thread's first one, oldest first: what the person had said, for the
    /// reading agent to tell what they meant.
    pub async fn read_turn_chain(
        &self,
        caller: &ConversationCaller,
        thread: ConversationId,
        execution: String,
        earlier: usize,
    ) -> Result<TurnChain, ConversationError> {
        let mut chain: Vec<TurnReading> = Vec::new();
        let mut next = Some((thread, execution));
        let ended = |turns, end| TurnChain {
            turns,
            end,
            earlier: Vec::new(),
        };
        while let Some((thread, execution)) = next.take() {
            if chain.len() == MAX_TURN_HOPS {
                return Ok(ended(chain, ChainEnd::Cut));
            }
            let (history, at) = match self.saved_turns(caller, &thread, &execution).await {
                Ok(found) => found,
                Err(error) if chain.is_empty() => return Err(error),
                Err(_) => return Ok(ended(chain, ChainEnd::Unreadable)),
            };
            let record = &history[at];
            let origin = origin(record);
            next = match &origin {
                TurnOrigin::Agent { thread, execution } => ConversationId::new(thread.as_str())
                    .ok()
                    .map(|thread| (thread, execution.clone())),
                TurnOrigin::Resumed { execution } => Some((thread.clone(), execution.clone())),
                _ => None,
            };
            let relayed = matches!(
                origin,
                TurnOrigin::Agent { .. } | TurnOrigin::Resumed { .. }
            );
            chain.push(TurnReading {
                thread: thread.clone(),
                execution,
                origin,
                said: record.request.user_message.text_str().to_owned(),
            });
            if relayed && next.is_none() {
                return Ok(ended(chain, ChainEnd::Unreadable));
            }
            if !relayed {
                let person = matches!(
                    chain.last().map(|turn| &turn.origin),
                    Some(TurnOrigin::Person | TurnOrigin::Grant(_))
                );
                let earlier = if person {
                    earlier_said(&thread, &history[..at], earlier.min(MAX_EARLIER_TURNS))
                } else {
                    Vec::new()
                };
                return Ok(TurnChain {
                    turns: chain,
                    end: ChainEnd::Start,
                    earlier,
                });
            }
        }
        Ok(ended(chain, ChainEnd::Start))
    }

    /// `thread`'s saved turns, read as `caller`, and where `execution` is
    /// among them.
    async fn saved_turns(
        &self,
        caller: &ConversationCaller,
        thread: &ConversationId,
        execution: &str,
    ) -> Result<(Vec<InvocationRecord>, usize), ConversationError> {
        self.check_view_access(thread, caller)
            .await
            .map_err(|_| ConversationError::NotFound)?;
        let invocations = match self.live_now(thread, caller).await {
            Ok(Some(live)) => live
                .agent
                .session_manager()
                .snapshot()
                .await
                .map(|snapshot| snapshot.invocations),
            _ => None,
        };
        let invocations = match invocations {
            Some(invocations) => invocations,
            None => {
                let (committed, _) = self.committed_with_order(thread).await?;
                committed
                    .snapshot()
                    .map(|snapshot| snapshot.invocations.clone())
                    .unwrap_or_default()
            }
        };
        let at = invocations
            .iter()
            .position(|record| record.request.execution_id.as_str() == execution)
            .ok_or(ConversationError::NotFound)?;
        Ok((invocations, at))
    }
}

/// The person's messages in `before` (a thread's turns before the one a chain
/// starts at, oldest first): the last `most`, and the thread's first, oldest
/// first. Only what a person sent counts; relays, apps and the gateway's own
/// inputs are not what the person said.
fn earlier_said(
    thread: &ConversationId,
    before: &[InvocationRecord],
    most: usize,
) -> Vec<TurnReading> {
    if most == 0 {
        return Vec::new();
    }
    let people: Vec<TurnReading> = before
        .iter()
        .filter_map(|record| {
            let origin = origin(record);
            matches!(origin, TurnOrigin::Person | TurnOrigin::Grant(_)).then(|| TurnReading {
                thread: thread.clone(),
                execution: record.request.execution_id.as_str().to_owned(),
                origin,
                said: record.request.user_message.text_str().to_owned(),
            })
        })
        .collect();
    let last = people.len().saturating_sub(most);
    let mut said = Vec::new();
    if last > 0 {
        said.push(people[0].clone());
    }
    said.extend(people[last..].iter().cloned());
    said
}

/// How `record`'s input arrived. The sender is the one saved fact for this;
/// a resumption's source turn is the SDK's own saved link.
fn origin(record: &InvocationRecord) -> TurnOrigin {
    if let Some(source) = &record.resumed_from {
        return TurnOrigin::Resumed {
            execution: source.as_str().to_owned(),
        };
    }
    match record.request.user_message.sender() {
        MessageSender::Agent(source) => TurnOrigin::Agent {
            thread: source.conversation().to_owned(),
            execution: source.execution().as_str().to_owned(),
        },
        MessageSender::App(app) => TurnOrigin::App {
            server: app.tool().server().to_owned(),
            tool: app.tool().tool().to_owned(),
        },
        MessageSender::Internal => TurnOrigin::Internal,
        MessageSender::Person => match record.initiation.initiator() {
            Initiator::Grant(grant) => TurnOrigin::Grant(grant.grant().to_owned()),
            _ => TurnOrigin::Person,
        },
    }
}
