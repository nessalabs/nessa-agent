//! How agents talk (issue #1382): the person's Nessa-level and per-channel
//! choice for each role, and the style each conversation starts with.
//!
//! ```text
//! replyStyle.read/set ──▶ channel? ── channel.settings (Cedar) ──▶ ReplyStyleStore
//!                         └─ none: the Nessa level, admitted by the method's grant
//! conversation.create ──▶ role (coordinator or thread) ──▶ effective style ──▶ pin
//! opening ──▶ pinned style ──▶ AgentBrief.style ──▶ Nessa's launch prompt
//! ```
//!
//! A conversation's style is fixed when it is created. Its prompt is part of
//! its saved session's identity, so changing it later would start a fresh
//! session that has forgotten the conversation. A change reaches the
//! conversations created after it.
use super::{ConversationCaller, ConversationError, ConversationService};
use crate::application::{
    ConversationFuture, SettingsAuditRecord, SettingsChange, SettingsOutcome,
};
use crate::domain::{
    Channel, ChannelId, ReplyStyle, ReplyStyleSettings, RoleStyles, SettingsScope, StyleRole,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::domain::ConversationId;

/// Where reply style choices, and each conversation's starting style, are kept.
pub trait ReplyStyleStore: Send + Sync {
    /// `owner`'s own choices at the Nessa level (`channel` `None`) or for one
    /// channel. A role with no choice is `None`.
    fn styles(
        &self,
        organization: &OrganizationId,
        owner: &PrincipalId,
        channel: Option<&ChannelId>,
    ) -> ConversationFuture<'_, RoleStyles>;
    /// Keep `choice` for `role`, or remove it (`None`) so the level inherits.
    fn set_style(
        &self,
        organization: &OrganizationId,
        owner: &PrincipalId,
        channel: Option<&ChannelId>,
        role: StyleRole,
        choice: Option<ReplyStyle>,
    ) -> ConversationFuture<'_, ()>;
    /// Fix the role, style and job text `conversation` starts with. The
    /// first pin stays: a retried creation keeps what the first attempt fixed.
    fn pin(&self, conversation: &ConversationId, start: PinnedStart) -> ConversationFuture<'_, ()>;
    /// What `conversation` was started with; `None` for one created before
    /// styles were kept, which is told none. A deleted conversation's pin
    /// goes with its channel placement (`ChannelStore::forget`).
    fn pinned(&self, conversation: &ConversationId) -> ConversationFuture<'_, Option<PinnedStart>>;
}

/// What a conversation was started with, fixed once: its role, its style,
/// and the job text it was told (`None` for a thread, and for a coordinator
/// pinned before jobs were told). The job is kept as told rather than read
/// from `StyleRole::job` at each opening, so neither a coordinator from
/// before nor a later rewording ever changes a saved prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinnedStart {
    pub role: StyleRole,
    pub style: ReplyStyle,
    pub job: Option<String>,
}

/// The store reply styles are kept in, bound once at startup.
#[derive(Default)]
pub struct ReplyStyleBinding {
    store: std::sync::OnceLock<std::sync::Arc<dyn ReplyStyleStore>>,
    /// Held across a change's read of `before`, its audit and its write, so
    /// two changes at once never both record the same `before`.
    changes: tokio::sync::Mutex<()>,
}

impl ConversationService {
    /// Keep reply styles in `store`. Unbound, no agent is told a style and
    /// nothing can be changed.
    pub fn bind_reply_styles(&self, store: std::sync::Arc<dyn ReplyStyleStore>) {
        let _ = self.inner.reply_styles.store.set(store);
    }

    fn reply_style_store(&self) -> Option<&std::sync::Arc<dyn ReplyStyleStore>> {
        self.inner.reply_styles.store.get()
    }

    async fn style_scope(
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

    async fn read_styles(
        &self,
        organization: &OrganizationId,
        owner: &PrincipalId,
        scope: SettingsScope,
    ) -> Result<ReplyStyleSettings, ConversationError> {
        let Some(store) = self.reply_style_store() else {
            return Ok(ReplyStyleSettings::new(
                scope,
                RoleStyles::default(),
                RoleStyles::default(),
            ));
        };
        let nessa = store.styles(organization, owner, None).await?;
        let channel = match scope.channel() {
            Some(channel) => store.styles(organization, owner, Some(channel)).await?,
            None => RoleStyles::default(),
        };
        Ok(ReplyStyleSettings::new(scope, nessa, channel))
    }

    /// The caller's Nessa-level choices and, for `channel`, its own, as one
    /// read.
    ///
    /// # Errors
    /// `ChannelNotFound` for a channel the caller may not change;
    /// `Metadata` when the choices cannot be read.
    pub async fn reply_styles(
        &self,
        caller: &ConversationCaller,
        channel: Option<ChannelId>,
    ) -> Result<ReplyStyleSettings, ConversationError> {
        let scope = self.style_scope(caller, channel).await?;
        self.read_styles(&caller.organization_id, &caller.principal_id, scope)
            .await
    }

    /// Save `choice` for `role` at the Nessa level or for `channel` (`None`
    /// there: the level follows the one above again), answering what is
    /// kept now. Recorded in the settings audit before and after.
    ///
    /// # Errors
    /// As [`Self::reply_styles`]; `Unavailable` when no store is bound;
    /// `Audit` when the change could not be recorded.
    pub async fn set_reply_style(
        &self,
        caller: &ConversationCaller,
        channel: Option<ChannelId>,
        role: StyleRole,
        choice: Option<ReplyStyle>,
    ) -> Result<ReplyStyleSettings, ConversationError> {
        let scope = self.style_scope(caller, channel).await?;
        let store = self
            .reply_style_store()
            .ok_or(ConversationError::Unavailable)?
            .clone();
        let (organization, owner) = (&caller.organization_id, &caller.principal_id);
        let _change = self.inner.reply_styles.changes.lock().await;
        let before = store
            .styles(organization, owner, scope.channel())
            .await?
            .get(role)
            .cloned();
        if before == choice {
            return self.read_styles(organization, owner, scope).await;
        }
        let operation = uuid::Uuid::new_v4().to_string();
        let record = |outcome| SettingsAuditRecord {
            operation_id: operation.clone(),
            caller: caller.clone(),
            change: SettingsChange::ReplyStyle {
                channel: scope.channel().cloned(),
                role,
                before: before.clone(),
                requested: choice.clone(),
            },
            outcome,
        };
        self.record_setting(record(SettingsOutcome::Requested))
            .await?;
        if let Err(error) = store
            .set_style(organization, owner, scope.channel(), role, choice.clone())
            .await
        {
            if self
                .record_setting(record(SettingsOutcome::Failed))
                .await
                .is_err()
            {
                tracing::warn!(
                    role = role.as_str(),
                    "a failed reply style change was not recorded"
                );
            }
            return Err(error);
        }
        self.record_setting(record(SettingsOutcome::Applied {
            changed: true,
            coordinator: None,
        }))
        .await?;
        self.read_styles(organization, owner, scope).await
    }

    /// Fix the style a conversation being created starts with: its role's
    /// effective style for its owner and channel, read now.
    pub(super) async fn pin_reply_style(
        &self,
        id: &ConversationId,
        caller: &ConversationCaller,
        channel: Option<&Channel>,
    ) -> Result<(), ConversationError> {
        let Some(store) = self.reply_style_store() else {
            return Ok(());
        };
        let role = match channel {
            Some(channel) if *id == channel.coordinator_id() => StyleRole::Coordinator,
            _ => StyleRole::Thread,
        };
        let scope = channel.map_or(SettingsScope::Global, |channel| {
            SettingsScope::Channel(channel.id().clone())
        });
        let style = self
            .read_styles(&caller.organization_id, &caller.principal_id, scope)
            .await?
            .effective(role);
        let job = role.job().map(str::to_owned);
        store.pin(id, PinnedStart { role, style, job }).await
    }

    /// What `id`'s agent is told of its job and how to write, from the role
    /// and style it was started with. With no store bound, or for a
    /// conversation from before styles were kept, nothing: nothing was
    /// pinned, and telling it now would change its saved prompt.
    pub(super) async fn role_and_style(
        &self,
        id: &ConversationId,
    ) -> Result<(Option<String>, Option<String>), ConversationError> {
        let Some(store) = self.reply_style_store() else {
            return Ok((None, None));
        };
        Ok(match store.pinned(id).await? {
            Some(start) => (start.job, start.style.instructions(start.role)),
            None => (None, None),
        })
    }
}
