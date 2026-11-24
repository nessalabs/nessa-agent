//! What getting a conversation's cloud machine ready is doing now, step by
//! step, for its view: making or waking the machine, putting this build on
//! it, reaching it, starting the agent there. A send to a chat on a cloud
//! machine can wait a minute or more on these, and a person shown only
//! "Thinking" cannot tell setup from work.
//!
//! This is the one owner of that fact. Only the conversation service begins
//! an opening ([`SetupSteps::begin`]), as it starts opening the agent, and
//! only it ends one, ready or failed, as the agent attached or did not; the
//! machine's connector (`infrastructure::workspaces`) says when each machine
//! step begins inside it. A step said with no opening under way (a command's
//! link redialed while the agent works) is not setup, and is not recorded. Views
//! read it (`ConversationView::setup`), and each change wakes their readers
//! through [`LiveChanges`].
//!
//! It is kept in memory only: a step is about now, and the last finished
//! opening is kept only so a reply can say how long its machine took. A
//! gateway that restarts starts with none.

use super::LiveChanges;
use nessa_auth::application::ports::Clock;
use nessa_protocol::conversation::{
    domain::ConversationId,
    view::{ConversationSetup, ConversationSetupState, ConversationSetupStep},
};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, OnceLock, PoisonError},
};

/// Most conversations whose openings are kept. A conversation with an
/// opening under way is never let go for room; the oldest finished one is.
const MAX_CONVERSATIONS: usize = 1024;
/// Most steps one opening lists. Every step but `Connecting` happens once;
/// a reconnect may repeat it. One past this is left out, and the step
/// before it keeps running.
const MAX_STEPS: usize = 8;
/// An opening with no step for this long, and never ended, is not shown:
/// whatever was making the machine ready gave up without saying so.
const ABANDONED_AFTER_MS: u64 = 15 * 60 * 1000;

/// How long an opening that ended serving no message may still be claimed
/// by the next send: the send it was made ready for comes right after it.
const UNCLAIMED_FOR_MS: u64 = 60 * 1000;

pub use nessa_protocol::conversation::view::ConversationSetupStepKind as SetupStep;

/// One opening, as [`SetupSteps::begin`] began it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetupOpening(u64);

/// How an opening ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupEnd {
    /// The agent started.
    Ready,
    /// The machine or the agent did not start.
    Failed,
}

#[derive(Clone, Debug)]
struct Step {
    step: SetupStep,
    started_at_ms: u64,
    ended_at_ms: Option<u64>,
}

#[derive(Clone, Debug, Default)]
struct Opening {
    /// Which of the service's openings this is ([`SetupSteps::begin`]).
    generation: u64,
    execution: Option<String>,
    /// Begun by a chat's creation, for no message: its first send, right
    /// after, may claim it ([`SetupSteps::sending`]).
    for_creation: bool,
    steps: Vec<Step>,
    end: Option<SetupEnd>,
}

impl Opening {
    fn view(&self) -> ConversationSetup {
        ConversationSetup {
            state: match self.end {
                None => ConversationSetupState::Running,
                Some(SetupEnd::Ready) => ConversationSetupState::Ready,
                Some(SetupEnd::Failed) => ConversationSetupState::Failed,
            },
            execution_id: self.execution.clone(),
            steps: self
                .steps
                .iter()
                .map(|step| ConversationSetupStep {
                    step: step.step,
                    started_at_ms: step.started_at_ms,
                    ended_at_ms: step.ended_at_ms,
                })
                .collect(),
        }
    }

    fn last_moved_at(&self) -> u64 {
        self.steps
            .last()
            .map_or(0, |step| step.ended_at_ms.unwrap_or(step.started_at_ms))
    }
}

/// One conversation's openings: the one under way and the last that ended
/// with any step, and the message whose send is waiting on an opening now.
#[derive(Default)]
struct Openings {
    /// Set while a send of this message waits for the agent to open
    /// ([`SetupSteps::sending`]); taken by the opening it starts.
    sending: Option<String>,
    current: Option<Opening>,
    last: Option<Opening>,
}

#[derive(Default)]
struct State {
    by: HashMap<ConversationId, Openings>,
    /// Least recently changed first.
    order: VecDeque<ConversationId>,
}

impl State {
    fn touch(&mut self, id: &ConversationId) -> &mut Openings {
        if let Some(at) = self.order.iter().position(|known| known == id) {
            self.order.remove(at);
        }
        self.order.push_back(id.clone());
        if self.by.len() >= MAX_CONVERSATIONS && !self.by.contains_key(id) {
            let idle = self.order.iter().position(|known| {
                self.by.get(known).is_some_and(|openings| {
                    openings.current.is_none() && openings.sending.is_none()
                })
            });
            if let Some(at) = idle {
                if let Some(gone) = self.order.remove(at) {
                    self.by.remove(&gone);
                }
            }
        }
        self.by.entry(id.clone()).or_default()
    }
}

/// See the module documentation.
pub struct SetupSteps {
    clock: Arc<dyn Clock>,
    state: Mutex<State>,
    /// Bound by the conversation service as it starts: a change wakes the
    /// readers of that conversation's view.
    changes: OnceLock<Arc<LiveChanges>>,
    /// The last opening begun, of any conversation.
    generations: std::sync::atomic::AtomicU64,
}

impl SetupSteps {
    pub fn new(clock: Arc<dyn Clock>) -> Arc<Self> {
        Arc::new(Self {
            clock,
            state: Mutex::default(),
            changes: OnceLock::new(),
            generations: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// Wake `changes`' readers of a conversation when its steps change. A
    /// second bind is ignored.
    pub fn publish_to(&self, changes: Arc<LiveChanges>) {
        let _ = self.changes.set(changes);
    }

    fn changed(&self, id: &ConversationId) {
        if let Some(changes) = self.changes.get() {
            changes.publish(id);
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        f(&mut self.state.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// A send of `execution` to `id` may open its agent now: an opening that
    /// begins while the returned guard is held, one already under way that
    /// names no message, or one that just ended naming none, is that send's.
    pub fn sending(self: &Arc<Self>, id: &ConversationId, execution: &str) -> Sending {
        let now = self.clock.unix_milliseconds();
        let bound = self.with(|state| {
            let openings = state.touch(id);
            openings.sending = Some(execution.to_owned());
            // An earlier attempt's opening for this message is not what this
            // send of it waits on: it no longer names it.
            let mut changed = false;
            if let Some(last) = openings.last.as_mut() {
                if last.execution.as_deref() == Some(execution) {
                    last.execution = None;
                    last.for_creation = false;
                    changed = true;
                }
            }
            changed
                | match openings.current.as_mut() {
                    Some(current) if current.execution.is_none() => {
                        current.execution = Some(execution.to_owned());
                        true
                    }
                    Some(_) => false,
                    // A machine made ready (or not) just now for a chat's
                    // creation was for its first send: this one.
                    None => match openings.last.as_mut() {
                        Some(last)
                            if last.for_creation
                                && last.execution.is_none()
                                && now.saturating_sub(last.last_moved_at()) < UNCLAIMED_FOR_MS =>
                        {
                            last.execution = Some(execution.to_owned());
                            true
                        }
                        _ => false,
                    },
                }
        });
        if bound {
            self.changed(id);
        }
        Sending {
            steps: self.clone(),
            id: id.clone(),
            execution: execution.to_owned(),
        }
    }

    /// The service begins opening `id`'s agent, for the message whose send
    /// waits on it if one does, or for its chat's creation. An opening still
    /// under way is let go: only one opens a conversation's agent at a time.
    /// It is shown once a machine step is said in it. What is returned
    /// names it to [`Self::end`].
    pub fn begin(&self, id: &ConversationId, for_creation: bool) -> SetupOpening {
        let generation = 1 + self
            .generations
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let shown = self.with(|state| {
            let openings = state.touch(id);
            let execution = openings.sending.clone();
            openings
                .current
                .replace(Opening {
                    generation,
                    execution,
                    for_creation,
                    ..Opening::default()
                })
                .is_some_and(|gone| !gone.steps.is_empty())
        });
        if shown {
            self.changed(id);
        }
        SetupOpening(generation)
    }

    /// `step` begins now for `id`'s machine, in the opening under way: the
    /// step before it ends. With none under way it is not setup, and is not
    /// recorded.
    pub fn step(&self, id: &ConversationId, step: SetupStep) {
        let now = self.clock.unix_milliseconds();
        let recorded = self.with(|state| {
            let Some(current) = state
                .by
                .get_mut(id)
                .and_then(|openings| openings.current.as_mut())
            else {
                return false;
            };
            if current
                .steps
                .last()
                .is_some_and(|last| last.step == step && last.ended_at_ms.is_none())
            {
                return false;
            }
            if current.steps.len() >= MAX_STEPS {
                return false;
            }
            if let Some(last) = current.steps.last_mut() {
                last.ended_at_ms.get_or_insert(now);
            }
            current.steps.push(Step {
                step,
                started_at_ms: now,
                ended_at_ms: None,
            });
            true
        });
        if recorded {
            self.changed(id);
        }
    }

    /// The agent is being started now, in the opening under way: a setup
    /// step only where a machine step came before it.
    pub fn starting(&self, id: &ConversationId) {
        let machine = self.with(|state| {
            state
                .by
                .get(id)
                .and_then(|openings| openings.current.as_ref())
                .is_some_and(|current| !current.steps.is_empty())
        });
        if machine {
            self.step(id, SetupStep::Starting);
        }
    }

    /// `id`'s opening under way ended as `end`: its last step ends now. An
    /// opening with no step is forgotten; one with steps is kept as the
    /// last, for its reply. One that failed after its machine was ready
    /// (the agent was being started) ended ready, as far as setup goes.
    pub fn end(&self, id: &ConversationId, opening: SetupOpening, end: SetupEnd) {
        let now = self.clock.unix_milliseconds();
        let ended = self.with(|state| {
            let Some(openings) = state.by.get_mut(id) else {
                return false;
            };
            // One begun since is not this one's to end.
            if openings
                .current
                .as_ref()
                .is_none_or(|current| current.generation != opening.0)
            {
                return false;
            }
            let Some(mut current) = openings.current.take() else {
                return false;
            };
            if let Some(last) = current.steps.last_mut() {
                last.ended_at_ms.get_or_insert(now);
            }
            // The machine was ready once the agent was being started there:
            // an agent that then did not start (not signed in, say) is its
            // own failure, said by the send's refusal, not the container's.
            let machine_ready = current
                .steps
                .iter()
                .any(|step| step.step == SetupStep::Starting);
            current.end = Some(if machine_ready { SetupEnd::Ready } else { end });
            if current.steps.is_empty() {
                return false;
            }
            openings.last = Some(current);
            true
        });
        if ended {
            self.changed(id);
        }
    }

    /// Whether an opening of `id`'s machine is under way now.
    pub fn running(&self, id: &ConversationId) -> bool {
        let now = self.clock.unix_milliseconds();
        self.with(|state| {
            state
                .by
                .get(id)
                .and_then(|openings| openings.current.as_ref())
                .is_some_and(|current| {
                    !current.steps.is_empty()
                        && now.saturating_sub(current.last_moved_at()) < ABANDONED_AFTER_MS
                })
        })
    }

    /// What `id`'s view says of its openings, oldest first.
    pub fn view(&self, id: &ConversationId) -> Vec<ConversationSetup> {
        let now = self.clock.unix_milliseconds();
        self.with(|state| {
            let Some(openings) = state.by.get(id) else {
                return Vec::new();
            };
            let current = openings.current.as_ref().filter(|current| {
                !current.steps.is_empty()
                    && now.saturating_sub(current.last_moved_at()) < ABANDONED_AFTER_MS
            });
            // An ended opening names the message it served, which a reply
            // or a failed send reads it by; a chat's creation's names none
            // yet. Any other says nothing to anyone.
            let last = openings
                .last
                .as_ref()
                .filter(|last| last.execution.is_some() || last.for_creation);
            last.into_iter().chain(current).map(Opening::view).collect()
        })
    }

    fn sent(&self, id: &ConversationId, execution: &str) {
        self.with(|state| {
            if let Some(openings) = state.by.get_mut(id) {
                if openings.sending.as_deref() == Some(execution) {
                    openings.sending = None;
                }
            }
        });
    }
}

/// Held while a send waits for its agent to open ([`SetupSteps::sending`]).
pub struct Sending {
    steps: Arc<SetupSteps>,
    id: ConversationId,
    execution: String,
}

impl Drop for Sending {
    fn drop(&mut self) {
        self.steps.sent(&self.id, &self.execution);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixed(AtomicU64);
    impl Clock for Fixed {
        fn unix_milliseconds(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    fn id(n: u8) -> ConversationId {
        ConversationId::new(&format!("00000000-0000-4000-8000-0000000000{n:02}")).unwrap()
    }

    fn steps() -> (Arc<Fixed>, Arc<SetupSteps>) {
        let clock = Arc::new(Fixed(AtomicU64::new(1_000)));
        (clock.clone(), SetupSteps::new(clock))
    }

    #[test]
    fn steps_end_as_the_next_begins_and_the_opening_is_kept_once_ready() {
        let (clock, steps) = steps();
        let sending = steps.sending(&id(1), "turn-1");
        let opening = steps.begin(&id(1), false);
        steps.step(&id(1), SetupStep::Creating);
        clock.0.store(19_000, Ordering::SeqCst);
        steps.step(&id(1), SetupStep::Connecting);
        drop(sending);
        clock.0.store(23_000, Ordering::SeqCst);
        steps.starting(&id(1));
        assert!(steps.running(&id(1)));
        let view = steps.view(&id(1));
        assert_eq!(view.len(), 1);
        assert_eq!(view[0].state, ConversationSetupState::Running);
        assert_eq!(view[0].execution_id.as_deref(), Some("turn-1"));
        assert_eq!(view[0].steps[0].ended_at_ms, Some(19_000));
        assert_eq!(view[0].steps[2].ended_at_ms, None);
        clock.0.store(29_000, Ordering::SeqCst);
        steps.end(&id(1), opening, SetupEnd::Ready);
        assert!(!steps.running(&id(1)));
        let view = steps.view(&id(1));
        assert_eq!(view[0].state, ConversationSetupState::Ready);
        assert_eq!(view[0].steps[2].ended_at_ms, Some(29_000));
    }

    #[test]
    fn the_agent_starting_is_a_step_only_where_a_machine_was_made_ready() {
        let (_, steps) = steps();
        let opening = steps.begin(&id(2), false);
        steps.starting(&id(2));
        steps.end(&id(2), opening, SetupEnd::Ready);
        assert!(steps.view(&id(2)).is_empty());
    }

    #[test]
    fn an_opening_ends_only_by_its_own_end_not_by_an_earlier_one_s() {
        let (_, steps) = steps();
        let earlier = steps.begin(&id(12), false);
        let _later = steps.begin(&id(12), false);
        steps.step(&id(12), SetupStep::Waking);
        steps.end(&id(12), earlier, SetupEnd::Failed);
        assert!(steps.running(&id(12)), "the opening begun since goes on");
    }

    #[test]
    fn a_machine_step_with_no_opening_under_way_is_not_setup() {
        let (_, steps) = steps();
        // A command's link redialed while the agent works.
        steps.step(&id(8), SetupStep::Connecting);
        assert!(!steps.running(&id(8)));
        assert!(steps.view(&id(8)).is_empty());
    }

    #[test]
    fn an_agent_that_did_not_start_on_a_ready_machine_is_not_the_container_s_failure() {
        let (_, steps) = steps();
        let _sending = steps.sending(&id(9), "turn-1");
        let opening = steps.begin(&id(9), false);
        steps.step(&id(9), SetupStep::Waking);
        steps.starting(&id(9));
        steps.end(&id(9), opening, SetupEnd::Failed);
        assert_eq!(steps.view(&id(9))[0].state, ConversationSetupState::Ready);
    }

    #[test]
    fn a_message_sent_again_no_longer_names_the_opening_that_failed_it() {
        let (_, steps) = steps();
        drop(steps.sending(&id(10), "turn-1"));
        let sending = steps.sending(&id(10), "turn-1");
        let opening = steps.begin(&id(10), false);
        steps.step(&id(10), SetupStep::Waking);
        steps.end(&id(10), opening, SetupEnd::Failed);
        drop(sending);
        assert_eq!(
            steps.view(&id(10))[0].execution_id.as_deref(),
            Some("turn-1")
        );
        let _again = steps.sending(&id(10), "turn-1");
        assert!(steps.view(&id(10)).is_empty());
    }

    #[test]
    fn an_opening_a_send_did_not_start_names_no_message_until_one_waits_on_it() {
        let (_, steps) = steps();
        steps.begin(&id(3), false);
        steps.step(&id(3), SetupStep::Waking);
        assert_eq!(steps.view(&id(3))[0].execution_id, None);
        let _sending = steps.sending(&id(3), "turn-9");
        assert_eq!(
            steps.view(&id(3))[0].execution_id.as_deref(),
            Some("turn-9")
        );
    }

    #[test]
    fn a_send_right_after_its_chat_s_creation_claims_that_opening_and_no_other_does() {
        let (clock, steps) = steps();
        let opening = steps.begin(&id(6), true);
        steps.step(&id(6), SetupStep::Creating);
        steps.end(&id(6), opening, SetupEnd::Ready);
        drop(steps.sending(&id(6), "first"));
        assert_eq!(steps.view(&id(6))[0].execution_id.as_deref(), Some("first"));
        // One a read opened is not a send's.
        let opening = steps.begin(&id(11), false);
        steps.step(&id(11), SetupStep::Waking);
        steps.end(&id(11), opening, SetupEnd::Ready);
        drop(steps.sending(&id(11), "after-a-read"));
        assert!(steps.view(&id(11)).is_empty());
        let opening = steps.begin(&id(7), true);
        steps.step(&id(7), SetupStep::Creating);
        steps.end(&id(7), opening, SetupEnd::Ready);
        clock.0.store(1_000 + UNCLAIMED_FOR_MS, Ordering::SeqCst);
        drop(steps.sending(&id(7), "much-later"));
        assert_eq!(steps.view(&id(7))[0].execution_id, None);
    }

    #[test]
    fn a_reply_sent_again_does_not_say_how_long_an_earlier_attempt_s_machine_took() {
        let (_, steps) = steps();
        let sending = steps.sending(&id(13), "turn-1");
        let opening = steps.begin(&id(13), false);
        steps.step(&id(13), SetupStep::Waking);
        steps.starting(&id(13));
        steps.end(&id(13), opening, SetupEnd::Failed);
        drop(sending);
        let _again = steps.sending(&id(13), "turn-1");
        assert!(steps.view(&id(13)).is_empty());
    }

    #[test]
    fn a_failed_opening_stays_beside_the_next_one_under_way() {
        let (_, steps) = steps();
        let _sending = steps.sending(&id(4), "turn-1");
        let opening = steps.begin(&id(4), false);
        steps.step(&id(4), SetupStep::Waking);
        steps.end(&id(4), opening, SetupEnd::Failed);
        steps.begin(&id(4), false);
        steps.step(&id(4), SetupStep::Waking);
        let view = steps.view(&id(4));
        assert_eq!(view.len(), 2);
        assert_eq!(view[0].state, ConversationSetupState::Failed);
        assert_eq!(view[1].state, ConversationSetupState::Running);
    }

    #[test]
    fn an_opening_nobody_ended_is_not_shown_once_it_stopped_moving() {
        let (clock, steps) = steps();
        steps.begin(&id(5), false);
        steps.step(&id(5), SetupStep::Creating);
        clock.0.store(1_000 + ABANDONED_AFTER_MS, Ordering::SeqCst);
        assert!(!steps.running(&id(5)));
        assert!(steps.view(&id(5)).is_empty());
    }

    #[test]
    fn a_step_is_a_change_to_its_conversation_s_view() {
        let (_, steps) = steps();
        let changes = Arc::new(LiveChanges::default());
        steps.publish_to(changes.clone());
        let mut receiver = changes.subscribe(&id(6));
        let opening = steps.begin(&id(6), false);
        steps.step(&id(6), SetupStep::Creating);
        assert!(receiver.has_changed().unwrap());
        receiver.borrow_and_update();
        steps.end(&id(6), opening, SetupEnd::Ready);
        assert!(receiver.has_changed().unwrap());
    }

    #[test]
    fn room_is_made_from_the_oldest_conversation_with_nothing_under_way() {
        let (_, steps) = steps();
        steps.begin(&id(0), false);
        steps.step(&id(0), SetupStep::Creating);
        for n in 0..MAX_CONVERSATIONS {
            let other = ConversationId::new(&format!("00000000-0000-4000-9000-{n:012}")).unwrap();
            let opening = steps.begin(&other, false);
            steps.step(&other, SetupStep::Creating);
            steps.end(&other, opening, SetupEnd::Ready);
        }
        assert!(
            steps.running(&id(0)),
            "an opening under way keeps its place"
        );
        assert!(steps.with(|state| state.by.len()) <= MAX_CONVERSATIONS);
    }
}
