//! The follower's choice of which granted conversations to replicate, and
//! which of those stay subscribed on one connection.
//!
//! The rule is local. It does not grant a conversation, and it is not sent
//! to the gateway that owns the chats. A missing rule follows nothing.
//! The live set is still the connection's conversation cap: opening one more
//! unsubscribes the least recently opened chat first.
use nessa_protocol::conversation::domain::ConversationId;
use nessa_protocol::product::generated::MAX_CONNECTION_CONVERSATION_SUBSCRIPTIONS;

/// Smallest replica a follow rule may keep, in bytes.
pub const FOLLOW_KEEP_MIN: u64 = 1024 * 1024;
/// Largest replica a follow rule may keep, in bytes. The retained cache's
/// own ceiling.
pub const FOLLOW_KEEP_MAX: u64 = 256 * 1024 * 1024;
/// The most conversations one follow rule names. The live set on one
/// connection is smaller; the rule may name chats that are not subscribed.
pub const FOLLOW_CONVERSATIONS: usize = 256;

/// Why a follow rule was refused before it was stored.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FollowRuleError {
    /// A conversation id was not a canonical UUID, or the same id was named twice.
    Conversation,
    /// More conversations than [`FOLLOW_CONVERSATIONS`].
    TooMany,
    /// `keep_bytes` is outside [`FOLLOW_KEEP_MIN`]..=[`FOLLOW_KEEP_MAX`].
    Keep,
}

/// Which granted conversations one peer's replica holds, and how many bytes
/// that replica may use. Empty conversations follow nothing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FollowRule {
    conversations: Vec<String>,
    keep_bytes: u64,
}

impl FollowRule {
    /// Follow nothing, keeping the largest replica the cache allows. A stored
    /// rule replaces this; the poller uses this only when the person has not
    /// chosen.
    pub fn none() -> Self {
        Self {
            conversations: Vec::new(),
            keep_bytes: FOLLOW_KEEP_MAX,
        }
    }

    /// `conversations` are the chats to replicate. Duplicates and ids the
    /// conversation owner would refuse are [`FollowRuleError::Conversation`].
    /// The list is stored in id order.
    pub fn new(conversations: Vec<String>, keep_bytes: u64) -> Result<Self, FollowRuleError> {
        if !(FOLLOW_KEEP_MIN..=FOLLOW_KEEP_MAX).contains(&keep_bytes) {
            return Err(FollowRuleError::Keep);
        }
        if conversations.len() > FOLLOW_CONVERSATIONS {
            return Err(FollowRuleError::TooMany);
        }
        let mut conversations = conversations;
        for id in &conversations {
            let parsed = ConversationId::new(id).map_err(|_| FollowRuleError::Conversation)?;
            if parsed.to_string() != *id {
                return Err(FollowRuleError::Conversation);
            }
        }
        conversations.sort();
        if conversations.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(FollowRuleError::Conversation);
        }
        Ok(Self {
            conversations,
            keep_bytes,
        })
    }

    /// The conversations this rule replicates, in id order.
    pub fn conversations(&self) -> &[String] {
        &self.conversations
    }

    /// How many bytes of the local replica to keep.
    pub fn keep_bytes(&self) -> u64 {
        self.keep_bytes
    }

    /// Whether `id` is one of the conversations this rule replicates.
    pub fn contains(&self, id: &str) -> bool {
        self.conversations
            .binary_search_by(|kept| kept.as_str().cmp(id))
            .is_ok()
    }

    /// Every cached id is one this rule replicates. An id the rule names and
    /// the cache does not hold is still covered: the grant, not the rule,
    /// decides whether it is there.
    pub fn covers<'a>(&self, ids: impl IntoIterator<Item = &'a str>) -> bool {
        ids.into_iter().all(|id| self.contains(id))
    }
}

/// What opening one chat on the live set does. The caller unsubscribes
/// before it subscribes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LiveChoice {
    /// This chat is already in the live set. It becomes the most recently opened.
    AlreadyOpen,
    /// Subscribe this chat after unsubscribing another, when the live set is full.
    Subscribe {
        /// The least recently opened chat to unsubscribe first. Absent when the live set has room.
        unsubscribe: Option<String>,
    },
}

/// The chats subscribed on one connection, oldest explicit open first.
/// At most [`MAX_CONNECTION_CONVERSATION_SUBSCRIPTIONS`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveFollow {
    open: Vec<String>,
}

impl LiveFollow {
    /// An empty live set.
    pub fn new() -> Self {
        Self { open: Vec::new() }
    }

    /// The chats in the live set, least recently opened first.
    pub fn open_ids(&self) -> &[String] {
        &self.open
    }

    /// Open `conversation`. A ninth chat unsubscribes the least recently
    /// opened one first. The caller performs that unsubscribe before the
    /// subscribe; this choice does not send either.
    pub fn open(&mut self, conversation: &str) -> LiveChoice {
        if let Some(index) = self.open.iter().position(|id| id == conversation) {
            let id = self.open.remove(index);
            self.open.push(id);
            return LiveChoice::AlreadyOpen;
        }
        let unsubscribe = if self.open.len() >= MAX_CONNECTION_CONVERSATION_SUBSCRIPTIONS {
            self.open.remove(0).into()
        } else {
            None
        };
        self.open.push(conversation.to_owned());
        LiveChoice::Subscribe { unsubscribe }
    }

    /// Drop `conversation` from the live set so a later open can subscribe it again.
    pub fn close(&mut self, conversation: &str) {
        self.open.retain(|id| id != conversation);
    }
}

impl Default for LiveFollow {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(byte: u8) -> String {
        format!("00000000-0000-4000-8000-{:012x}", byte)
    }

    #[test]
    fn a_rule_stores_a_sorted_set_and_refuses_a_wider_keep() {
        let rule = FollowRule::new(vec![id(2), id(1)], FOLLOW_KEEP_MIN).unwrap();
        assert_eq!(rule.conversations(), &[id(1), id(2)]);
        assert!(rule.contains(&id(1)));
        assert!(!rule.contains(&id(3)));
        assert!(rule.covers([id(1).as_str()]));
        assert!(!rule.covers([id(1).as_str(), id(3).as_str()]));
        assert_eq!(
            FollowRule::new(vec![id(1), id(1)], FOLLOW_KEEP_MAX).unwrap_err(),
            FollowRuleError::Conversation
        );
        assert_eq!(
            FollowRule::new(vec!["not-a-uuid".into()], FOLLOW_KEEP_MAX).unwrap_err(),
            FollowRuleError::Conversation
        );
        assert_eq!(
            FollowRule::new(vec![], FOLLOW_KEEP_MIN - 1).unwrap_err(),
            FollowRuleError::Keep
        );
        assert_eq!(
            FollowRule::new(vec![], FOLLOW_KEEP_MAX + 1).unwrap_err(),
            FollowRuleError::Keep
        );
        assert!(FollowRule::none().conversations().is_empty());
        assert!(FollowRule::none().covers(std::iter::empty()));
    }

    #[test]
    fn opening_a_ninth_chat_lets_the_oldest_go_first() {
        let mut live = LiveFollow::new();
        let mut ids = Vec::new();
        for byte in 0..MAX_CONNECTION_CONVERSATION_SUBSCRIPTIONS {
            let conversation = id(byte as u8);
            assert_eq!(
                live.open(&conversation),
                LiveChoice::Subscribe { unsubscribe: None }
            );
            ids.push(conversation);
        }
        let ninth = id(8);
        assert_eq!(
            live.open(&ninth),
            LiveChoice::Subscribe {
                unsubscribe: Some(ids[0].clone())
            }
        );
        assert_eq!(
            live.open_ids().len(),
            MAX_CONNECTION_CONVERSATION_SUBSCRIPTIONS
        );
        assert_eq!(live.open_ids()[0], ids[1]);
        assert_eq!(live.open(&ids[1]), LiveChoice::AlreadyOpen);
        assert_eq!(live.open_ids().last().unwrap(), &ids[1]);
        let next = id(9);
        assert_eq!(
            live.open(&next),
            LiveChoice::Subscribe {
                unsubscribe: Some(ids[2].clone())
            }
        );
    }

    #[test]
    fn closing_a_chat_lets_it_be_opened_again() {
        let mut live = LiveFollow::new();
        let conversation = id(1);
        live.open(&conversation);
        live.close(&conversation);
        assert!(live.open_ids().is_empty());
        assert_eq!(
            live.open(&conversation),
            LiveChoice::Subscribe { unsubscribe: None }
        );
    }
}
