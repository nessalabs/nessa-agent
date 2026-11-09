//! Faulty channel lookup ports for cross-boundary identity and refusal tests.
use crate::conversation::{
    application::{ChannelStore, ConversationFuture},
    domain::{Channel, ChannelId},
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::domain::ConversationId;
use std::collections::HashMap;

pub(crate) struct ChannelLookup {
    pub(crate) channel: Box<Channel>,
}
impl ChannelStore for ChannelLookup {
    fn channel_of(&self, _: &ConversationId) -> ConversationFuture<'_, Option<ChannelId>> {
        Box::pin(async { Ok(None) })
    }
    fn get(&self, _: &ChannelId) -> ConversationFuture<'_, Option<Channel>> {
        Box::pin(async { Ok(Some(self.channel.as_ref().clone())) })
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
    fn set_execution(
        &self,
        _: &ChannelId,
        _: crate::conversation::domain::ChannelExecution,
    ) -> ConversationFuture<'_, Option<Channel>> {
        panic!("lookup fixture cannot write")
    }
}
