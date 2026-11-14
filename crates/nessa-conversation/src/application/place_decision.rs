//! Where a new conversation runs: the one place that reads what was asked,
//! the conversation's channel, the default device and what kind of
//! creation it is ([`decide_place`]), and, for the pool, whether it
//! overflows to a cloud VM ([`pool_choice`]). Both are pure: the
//! conversation service gathers what `decide_place` reads, and
//! `Environments` gathers what `pool_choice` reads and records the place.
//!
//! ```text
//! create ──▶ decide_place(name asked, channel, default device, creation)
//!   ──▶ the place asked for ──▶ Environments::place
//!         pool ──▶ pool_choice(pool now, cloud) ──▶ pool | cloud VM
//!         ──▶ ConversationPlacements::place (before the record exists)
//! ```
use super::ConversationError;
use crate::domain::Place;

/// What kind of creation asks for a new conversation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Creation {
    /// A person's, from a client.
    Person,
    /// An agent's thread of its own conversation (`create_thread`).
    Thread,
    /// A fork of another conversation, which runs where its source runs:
    /// this place, read from the source's record.
    Fork(Place),
}

/// What a new conversation's creation knows of where it could run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaceAsk<'a> {
    /// The name of the place its creation asked for, as given; `None` when
    /// it named none.
    pub asked: Option<&'a str>,
    /// Where its channel runs a conversation like it (its main agent's
    /// place for the main conversation, its threads' for any other);
    /// `None` outside a stored channel.
    pub channel: Option<Place>,
    /// The default device, while a conversation could be placed on it now;
    /// `None` when none is set or it is not offered.
    pub default_device: Option<Place>,
    pub creation: Creation,
}

/// The place a new conversation is asked to run on.
///
/// - A fork runs where its source runs, even after the source's channel
///   moved; it names no place of its own.
/// - A person names a place only outside a channel: a channel says where
///   its conversations run. Naming none, a person's runs where its channel
///   says, or outside one on the default device, else here.
/// - An agent's thread runs where the agent named, else where its channel
///   says, else here, on this gateway; it stays in its channel either way.
///
/// # Errors
/// [`ConversationError::InvalidInput`] for a person naming a place in a
/// channel or a fork naming one; [`ConversationError::EnvironmentNotConfigured`]
/// for a name that is no place.
pub fn decide_place(ask: PlaceAsk<'_>) -> Result<Place, ConversationError> {
    let PlaceAsk {
        asked,
        channel,
        default_device,
        creation,
    } = ask;
    // Refused for being named where it may not be before its name is read,
    // as a person naming a place in a channel always was.
    if asked.is_some()
        && matches!(
            (&creation, &channel),
            (Creation::Person, Some(_)) | (Creation::Fork(_), _)
        )
    {
        return Err(ConversationError::InvalidInput);
    }
    let asked = asked
        .map(Place::named)
        .transpose()
        .map_err(|_| ConversationError::EnvironmentNotConfigured)?;
    Ok(match creation {
        Creation::Fork(source) => source,
        Creation::Person => asked.or(channel).or(default_device).unwrap_or(Place::Here),
        Creation::Thread => asked.or(channel).unwrap_or(Place::Here),
    })
}

/// What the pool can take for a new conversation now, under its channel's
/// preferences.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PoolNow {
    /// No worker the pool may use serves and is not draining.
    Offline,
    /// Some worker it may use is below its busy limits.
    Free,
    /// Every worker it may use is at or past a busy limit.
    Busy,
}

/// Where a conversation that asked for the pool is placed, given the pool
/// now and whether a cloud VM can be had (`cloud`): the pool while some
/// worker is free; otherwise a cloud VM of its own, rather than not at all
/// (offline) or behind every busy worker (overflow); with no cloud, still
/// the pool while some worker serves, busy or not.
///
/// # Errors
/// [`ConversationError::EnvironmentNotConfigured`] with no worker and no
/// cloud.
pub fn pool_choice(pool: PoolNow, cloud: bool) -> Result<Place, ConversationError> {
    match (pool, cloud) {
        (PoolNow::Free, _) | (PoolNow::Busy, false) => Ok(Place::Pool),
        (PoolNow::Offline | PoolNow::Busy, true) => Ok(Place::CloudVm),
        (PoolNow::Offline, false) => Err(ConversationError::EnvironmentNotConfigured),
    }
}

#[cfg(test)]
#[path = "../../tests/conversation/place_decision.rs"]
mod tests;
