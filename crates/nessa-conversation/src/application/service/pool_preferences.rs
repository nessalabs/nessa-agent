//! Reading and saving the pool's preferences for the person who owns this
//! gateway: the Nessa default (the method's own grant admits that), or one
//! channel's override, which Cedar's channel-settings action admits first.
use super::{ConversationCaller, ConversationError, ConversationService};
use crate::application::PoolCaller;
use crate::domain::{ChannelId, PoolPreference, PoolSettings, SettingsScope};

impl ConversationService {
    async fn pool_scope(
        &self,
        caller: &ConversationCaller,
        channel: Option<ChannelId>,
    ) -> Result<SettingsScope, ConversationError> {
        match channel {
            None => Ok(SettingsScope::Global),
            Some(channel) => {
                self.authorize_channel_settings(caller, &channel).await?;
                Ok(SettingsScope::Channel(channel))
            }
        }
    }

    /// The default, `channel`'s override and the busy limits, as one read.
    ///
    /// # Errors
    /// `ChannelNotFound` for a channel the caller may not change;
    /// `Unavailable` when no preferences are kept or they cannot be read.
    pub async fn pool_preferences(
        &self,
        caller: &ConversationCaller,
        channel: Option<ChannelId>,
    ) -> Result<PoolSettings, ConversationError> {
        let scope = self.pool_scope(caller, channel).await?;
        let store = self
            .inner
            .environment
            .pool_store()
            .ok_or(ConversationError::Unavailable)?;
        store
            .read(&scope)
            .await
            .map_err(|_| ConversationError::Unavailable)
    }

    /// Save `choice` for the default or `channel` (`None` there: the
    /// channel follows the default again), answering what was saved.
    ///
    /// # Errors
    /// As [`Self::pool_preferences`]; a save not known to be durable is
    /// `Unavailable`, never a success.
    pub async fn set_pool_preferences(
        &self,
        caller: &ConversationCaller,
        channel: Option<ChannelId>,
        choice: Option<PoolPreference>,
        asked_by: PoolCaller,
    ) -> Result<PoolSettings, ConversationError> {
        let scope = self.pool_scope(caller, channel).await?;
        let store = self
            .inner
            .environment
            .pool_store()
            .ok_or(ConversationError::Unavailable)?;
        store
            .save(scope, choice, asked_by)
            .await
            .map_err(|_| ConversationError::Unavailable)
    }
}
