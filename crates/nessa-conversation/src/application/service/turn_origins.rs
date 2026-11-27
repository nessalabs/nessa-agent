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
//!   A's turn 7 ──scheduled store: continues turn 5──▶ A's turn 5
//!   A's turn 5 ──sender: person, credential c──▶ ends: { origin: "person", device: phone }
//! ```
//!
//! A turn the gateway submits for the person also says "person" as its
//! sender, so three more saved facts tell those apart, each kept by its one
//! owner and never changed after the turn is admitted:
//!
//! - The scheduled-message store minted the turn's execution id for a
//!   usage-limit continuation: it continues the turn the store names.
//! - The turn's surface is [`EVENT_SURFACE_PREFIX`] and an event source: the
//!   gateway admitted an external event. A credential is asked first, so a
//!   credential is never read as an event.
//! - The turn's surface is [`AUTO_PICK_SURFACE`]: auto-pick sent the
//!   recommended choice for the owner, who had not answered.
//! - Otherwise the surface is the credential the person signed in with, and
//!   the credential registry says what it was issued to: their phone, a token
//!   on a computer, or another device.
use super::{ConversationCaller, ConversationError, ConversationService};
use nessa_auth::{
    application::pairing::{CredentialDevice, CredentialDeviceSource},
    domain::CredentialId,
};
use nessa_protocol::conversation::{domain::ConversationId, view::AUTO_PICK_SURFACE};
use nessa_sdk::{
    application::agent_execution::sessions::InvocationRecord,
    domain::agent_execution::{executions::Initiator, prompts::MessageSender},
};
use std::{collections::HashMap, sync::Arc};

/// The most turns one read follows. A relay chain longer than this is
/// answered as cut, with the turns read so far.
pub const MAX_TURN_HOPS: usize = 8;

/// The most earlier messages of the person one read adds for context.
pub const MAX_EARLIER_TURNS: usize = 3;

/// The surface every turn the gateway admits for an external event is saved
/// with, followed by the event source's id. The gateway writes it; nothing a
/// person or an agent sends can choose a surface.
pub const EVENT_SURFACE_PREFIX: &str = "event:";

/// Which device a person's turn was sent from, as the credential registry
/// says the credential it was sent with was issued.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Device {
    /// The owner's phone, paired for read and write.
    Phone,
    /// A token this gateway wrote on the computer it runs on: the desktop
    /// app or the command line.
    Computer,
    /// Another of the owner's paired devices.
    PairedDevice,
    /// A peer gateway.
    Peer,
    /// The registry has no such credential, or none is bound.
    Unknown,
}

impl From<CredentialDevice> for Device {
    fn from(device: CredentialDevice) -> Self {
        match device {
            CredentialDevice::Phone => Self::Phone,
            CredentialDevice::Token => Self::Computer,
            CredentialDevice::PairedDevice => Self::PairedDevice,
            CredentialDevice::Peer => Self::Peer,
        }
    }
}

/// How one turn's input arrived, as its saved record says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TurnOrigin {
    /// The conversation's owner sent it themselves, from this device.
    Person { device: Device },
    /// Someone the conversation is shared with sent it, under this grant.
    Grant { grant: String, device: Device },
    /// An agent sent it from the turn this links to. A thread's report back to
    /// its sender links to the reporting thread's turn.
    Agent { thread: String, execution: String },
    /// The turn picks up the one this links to, in the same thread, after a
    /// restart or a provider failure.
    Resumed { execution: String },
    /// The gateway sent it after a usage limit, to continue the turn this
    /// links to in the same thread.
    Continued { execution: String },
    /// The gateway admitted it for an external event from this source.
    Event { source: String },
    /// Auto-pick sent the recommended choice of a decision for the owner,
    /// who had not answered in the time they set.
    AutoPick,
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
    /// The id of the question waiting in `thread` that this message answers:
    /// the one waiting now, when the message is the owner's own, as its
    /// `origin` was read ([`Self::submission_origin`]). A usage-limit
    /// continuation, auto-pick, an external event, or someone the thread is
    /// shared with does not answer it, and nor does a message whose origin
    /// could not be read (`None`, said in the log where it was read). A
    /// waiting question that cannot be read answers nothing and is said in
    /// the log, so the question stays up rather than the message being
    /// refused.
    pub(super) async fn waiting_ask_answered(
        &self,
        thread: &ConversationId,
        origin: Option<&TurnOrigin>,
    ) -> Option<String> {
        if !matches!(origin, Some(TurnOrigin::Person { .. })) {
            return None;
        }
        match self.inner.summaries.load(thread).await {
            Ok(summary) => Some(summary?.ask()?.ask_id.clone()),
            Err(error) => {
                tracing::warn!(%thread, ?error, "the waiting ask could not be read");
                None
            }
        }
    }

    /// How a submission about to be saved arrived, read as `read_turn` will
    /// read its saved turn: the one reading of a turn's origin, asked before
    /// the turn exists so its origin can be recorded first.
    ///
    /// # Errors
    /// [`ConversationError::Unavailable`] when the credential registry cannot
    /// be read; the scheduled-message store's own error when it cannot.
    pub(super) async fn submission_origin(
        &self,
        thread: &ConversationId,
        execution: &str,
        sender: &MessageSender,
        surface: &str,
        initiator: &Initiator,
    ) -> Result<TurnOrigin, ConversationError> {
        Origins::new(self)
            .sent(thread, execution, sender, surface, initiator)
            .await
    }

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
        let mut origins = Origins::new(self);
        while let Some((thread, execution)) = next.take() {
            if chain.len() == MAX_TURN_HOPS {
                return Ok(ended(chain, ChainEnd::Cut));
            }
            let found = match self.saved_turns(caller, &thread, &execution).await {
                Ok((history, at)) => origins
                    .of(&thread, &history[at])
                    .await
                    .map(|origin| (history, at, origin)),
                Err(error) => Err(error),
            };
            let (history, at, origin) = match found {
                Ok(found) => found,
                Err(error) if chain.is_empty() => return Err(error),
                Err(_) => return Ok(ended(chain, ChainEnd::Unreadable)),
            };
            let record = &history[at];
            next = match &origin {
                TurnOrigin::Agent { thread, execution } => ConversationId::new(thread.as_str())
                    .ok()
                    .map(|thread| (thread, execution.clone())),
                TurnOrigin::Resumed { execution } | TurnOrigin::Continued { execution } => {
                    Some((thread.clone(), execution.clone()))
                }
                _ => None,
            };
            let relayed = matches!(
                origin,
                TurnOrigin::Agent { .. }
                    | TurnOrigin::Resumed { .. }
                    | TurnOrigin::Continued { .. }
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
                let person = chain.last().is_some_and(|turn| turn.origin.is_a_person());
                let earlier = if person {
                    origins
                        .earlier_said(&thread, &history[..at], earlier.min(MAX_EARLIER_TURNS))
                        .await?
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

impl Device {
    /// The device's one name, as `read_turn`, the turn-origin audit record
    /// and telemetry all say it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Phone => "phone",
            Self::Computer => "computer",
            Self::PairedDevice => "pairedDevice",
            Self::Peer => "peer",
            Self::Unknown => "unknown",
        }
    }
}

impl TurnOrigin {
    /// The origin's one name, as `read_turn`, the turn-origin audit record
    /// and telemetry all say it.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Person { .. } => "person",
            Self::Grant { .. } => "grant",
            Self::Agent { .. } => "agent",
            Self::Resumed { .. } => "resumed",
            Self::Continued { .. } => "continued",
            Self::Event { .. } => "event",
            Self::AutoPick => "autoPick",
            Self::App { .. } => "app",
            Self::Internal => "internal",
        }
    }

    /// A person sent it: the owner, or someone it is shared with.
    pub fn is_a_person(&self) -> bool {
        matches!(self, Self::Person { .. } | Self::Grant { .. })
    }
}

/// Reads turn origins for one `read_turn`, asking the credential registry
/// once per credential.
struct Origins<'a> {
    service: &'a ConversationService,
    devices: Option<Arc<dyn CredentialDeviceSource>>,
    known: HashMap<String, Device>,
}

impl<'a> Origins<'a> {
    fn new(service: &'a ConversationService) -> Self {
        Self {
            service,
            devices: service.credential_devices(),
            known: HashMap::new(),
        }
    }

    /// How `record`'s input arrived. A resumption's source turn is the SDK's
    /// own saved link; the sender says who sent the rest; a turn sent as the
    /// person is told apart as the module documentation says.
    async fn of(
        &mut self,
        thread: &ConversationId,
        record: &InvocationRecord,
    ) -> Result<TurnOrigin, ConversationError> {
        if let Some(source) = &record.resumed_from {
            return Ok(TurnOrigin::Resumed {
                execution: source.as_str().to_owned(),
            });
        }
        self.sent(
            thread,
            record.request.execution_id.as_str(),
            record.request.user_message.sender(),
            record.actor.surface_id(),
            record.initiation.initiator(),
        )
        .await
    }

    /// How an input sent by `sender` arrived, from the facts it is saved
    /// with: the same reading for a saved turn and for a submission about to
    /// be saved.
    async fn sent(
        &mut self,
        thread: &ConversationId,
        execution: &str,
        sender: &MessageSender,
        surface: &str,
        initiator: &Initiator,
    ) -> Result<TurnOrigin, ConversationError> {
        match sender {
            MessageSender::Agent(source) => {
                return Ok(TurnOrigin::Agent {
                    thread: source.conversation().to_owned(),
                    execution: source.execution().as_str().to_owned(),
                })
            }
            MessageSender::App(app) => {
                return Ok(TurnOrigin::App {
                    server: app.tool().server().to_owned(),
                    tool: app.tool().tool().to_owned(),
                })
            }
            MessageSender::Internal => return Ok(TurnOrigin::Internal),
            MessageSender::Person => {}
        }
        self.sent_as_person(thread, execution, surface, initiator)
            .await
    }

    /// How a turn sent with the person as its sender arrived: a usage-limit
    /// continuation, auto-pick, an external event, or the person (or a
    /// grant) on a device. Read from the facts saved at admission, which a submission
    /// also has before it is saved.
    async fn sent_as_person(
        &mut self,
        thread: &ConversationId,
        execution: &str,
        surface: &str,
        initiator: &Initiator,
    ) -> Result<TurnOrigin, ConversationError> {
        if let Some(store) = self.service.scheduled_messages() {
            if let Some(causal) = store.continued_from(&thread.to_string(), execution).await? {
                return Ok(TurnOrigin::Continued { execution: causal });
            }
        }
        if surface == AUTO_PICK_SURFACE {
            return Ok(TurnOrigin::AutoPick);
        }
        let device = self.device(surface).await?;
        if device == Device::Unknown {
            if let Some(source) = surface.strip_prefix(EVENT_SURFACE_PREFIX) {
                return Ok(TurnOrigin::Event {
                    source: source.to_owned(),
                });
            }
        }
        Ok(match initiator {
            Initiator::Grant(grant) => TurnOrigin::Grant {
                grant: grant.grant().to_owned(),
                device,
            },
            _ => TurnOrigin::Person { device },
        })
    }

    /// What the credential `surface` names was issued to.
    async fn device(&mut self, surface: &str) -> Result<Device, ConversationError> {
        if let Some(device) = self.known.get(surface) {
            return Ok(*device);
        }
        let device = match (&self.devices, CredentialId::new(surface)) {
            (Some(devices), Ok(credential)) => devices
                .credential_device(&credential)
                .await
                .map_err(|_| ConversationError::Unavailable)?
                .map_or(Device::Unknown, Device::from),
            _ => Device::Unknown,
        };
        self.known.insert(surface.to_owned(), device);
        Ok(device)
    }

    /// The person's messages in `before` (a thread's turns before the one a
    /// chain starts at, oldest first): the last `most`, and the thread's
    /// first, oldest first. Only what a person sent counts; relays,
    /// continuations, events, apps and the gateway's own inputs are not what
    /// the person said.
    async fn earlier_said(
        &mut self,
        thread: &ConversationId,
        before: &[InvocationRecord],
        most: usize,
    ) -> Result<Vec<TurnReading>, ConversationError> {
        if most == 0 {
            return Ok(Vec::new());
        }
        let mut last = Vec::new();
        let mut index = before.len();
        while index > 0 && last.len() < most {
            index -= 1;
            if let Some(reading) = self.person_said(thread, &before[index]).await? {
                last.push(reading);
            }
        }
        last.reverse();
        let mut said = Vec::new();
        for record in &before[..index] {
            if let Some(first) = self.person_said(thread, record).await? {
                said.push(first);
                break;
            }
        }
        said.extend(last);
        Ok(said)
    }

    async fn person_said(
        &mut self,
        thread: &ConversationId,
        record: &InvocationRecord,
    ) -> Result<Option<TurnReading>, ConversationError> {
        if record.resumed_from.is_some()
            || !matches!(record.request.user_message.sender(), MessageSender::Person)
        {
            return Ok(None);
        }
        let origin = self.of(thread, record).await?;
        Ok(origin.is_a_person().then(|| TurnReading {
            thread: thread.clone(),
            execution: record.request.execution_id.as_str().to_owned(),
            origin,
            said: record.request.user_message.text_str().to_owned(),
        }))
    }
}
