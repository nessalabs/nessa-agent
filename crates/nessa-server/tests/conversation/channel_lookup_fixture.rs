//! Faulty channel lookup ports for cross-boundary identity and refusal tests.
use crate::conversation::{
    application::{ChannelStore, ConversationError, ConversationFuture},
    domain::{Channel, ChannelId},
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::domain::ConversationId;
use std::collections::HashMap;

pub(crate) enum Membership {
    Default,
    Channel(ChannelId),
    Failed,
}
pub(crate) enum ChannelRead {
    Found(Box<Channel>),
    Missing,
    Failed,
}
pub(crate) struct ChannelLookup {
    pub(crate) membership: Membership,
    pub(crate) channel: ChannelRead,
}
impl ChannelStore for ChannelLookup {
    fn channel_of(&self, _: &ConversationId) -> ConversationFuture<'_, Option<ChannelId>> {
        Box::pin(async move {
            match &self.membership {
                Membership::Default => Ok(None),
                Membership::Channel(id) => Ok(Some(id.clone())),
                Membership::Failed => Err(ConversationError::Unavailable),
            }
        })
    }
    fn get(&self, _: &ChannelId) -> ConversationFuture<'_, Option<Channel>> {
        Box::pin(async move {
            match &self.channel {
                ChannelRead::Found(channel) => Ok(Some(channel.as_ref().clone())),
                ChannelRead::Missing => Ok(None),
                ChannelRead::Failed => Err(ConversationError::Unavailable),
            }
        })
    }
    fn create(&self, _: Channel) -> ConversationFuture<'_, Channel> {
        panic!("lookup fixture cannot write")
    }
    fn by_request(
        &self,
        _: &OrganizationId,
        _: &PrincipalId,
        _: &str,
    ) -> ConversationFuture<'_, Option<Channel>> {
        panic!("lookup fixture cannot list")
    }
    fn list(&self, _: &OrganizationId, _: &PrincipalId) -> ConversationFuture<'_, Vec<Channel>> {
        panic!("lookup fixture cannot list")
    }
    fn place(&self, _: &ConversationId, _: &ChannelId) -> ConversationFuture<'_, ()> {
        panic!("lookup fixture cannot place")
    }
    fn channels_of(
        &self,
        _: Vec<ConversationId>,
    ) -> ConversationFuture<'_, HashMap<ConversationId, ChannelId>> {
        panic!("lookup fixture cannot list")
    }
    fn forget(&self, _: &ConversationId) -> ConversationFuture<'_, ()> {
        panic!("lookup fixture cannot delete")
    }
}
