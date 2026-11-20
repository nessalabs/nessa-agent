// What a conversation's owner may do with it. The principal is a person on
// one of their own surfaces: `organization` and `id` are who their session
// is. A conversation is `organization`, `owner` and whether it was
// `deleted`; knowing its id grants nothing. Nothing is allowed unless a
// permit says so, so anybody else is refused and is told what a missing
// conversation is told.

// The owner may be told about it, deleted or not. Only someone this lets
// locate a conversation learns that it was deleted.
@id("owner.locate")
permit (
    principal is NessaConversation::Person,
    action == NessaConversation::Action::"conversation.locate",
    resource is NessaConversation::Conversation
)
when {
    principal.organization == resource.organization &&
    principal.id == resource.owner
};

// The owner acts on it until it is deleted: every command on it.
@id("owner.act")
permit (
    principal is NessaConversation::Person,
    action == NessaConversation::Action::"conversation.act",
    resource is NessaConversation::Conversation
)
when {
    principal.organization == resource.organization &&
    principal.id == resource.owner &&
    !resource.deleted
};

// The owner finishes deleting it after the tombstone is committed. A repeat
// delete returns the same receipt and carries on erasure. A live conversation
// is still `conversation.act`.
@id("owner.erase")
permit (
    principal is NessaConversation::Person,
    action == NessaConversation::Action::"conversation.erase",
    resource is NessaConversation::Conversation
)
when {
    principal.organization == resource.organization &&
    principal.id == resource.owner &&
    resource.deleted
};

// The owner reads it until it is deleted.
@id("owner.read")
permit (
    principal is NessaConversation::Person,
    action == NessaConversation::Action::"conversation.read",
    resource is NessaConversation::Conversation
)
when {
    principal.organization == resource.organization &&
    principal.id == resource.owner &&
    !resource.deleted
};

// An unpaired person's own conversations supply their waiting reviews.
@id("review.owner")
permit (
    principal is NessaConversation::ReviewRecipient,
    action == NessaConversation::Action::"review.notice",
    resource is NessaConversation::Conversation
)
when {
    principal.organization == resource.organization &&
    principal.id == resource.owner &&
    !resource.deleted
};

// Current admin membership admits an unpaired surface to its organization's
// waiting reviews. Role interpretation belongs here, not in the Rust caller.
@id("review.admin")
permit (
    principal is NessaConversation::ReviewRecipient,
    action == NessaConversation::Action::"review.notice",
    resource is NessaConversation::Conversation
)
when {
    principal.organization == resource.organization &&
    principal.membership == "admin" &&
    !resource.deleted
};

// The owner reads back a file it holds until it is deleted.
@id("owner.artifact_read")
permit (
    principal is NessaConversation::Person,
    action == NessaConversation::Action::"artifact.read",
    resource is NessaConversation::Conversation
)
when {
    principal.organization == resource.organization &&
    principal.id == resource.owner &&
    !resource.deleted
};

// The owner lists their own conversations. Devices ask grant.list for a
// grant-scoped catalogue under their distinct paired subject.
@id("owner.list")
permit (
    principal is NessaConversation::Person,
    action == NessaConversation::Action::"conversation.list",
    resource is NessaConversation::Catalogue
)
when {
    principal.organization == resource.organization &&
    principal.id == resource.owner
};

// The source turn can represent its owner only when that owner initiated it.
@id("owner.agent_tools")
permit (
    principal is NessaConversation::AgentTurn,
    action == NessaConversation::Action::"conversation.agent_tools",
    resource is NessaConversation::Conversation
)
when {
    principal.organization == resource.organization &&
    principal.id == resource.owner &&
    principal.ownerInitiated &&
    !resource.deleted
};

// Where the owner's saved turns came from is read by any turn of the owner's
// agents, one another agent started included: reading the chain that reached
// a turn is how its agent tells what the owner asked. It changes nothing.
@id("owner.read_turns")
permit (
    principal is NessaConversation::AgentTurn,
    action == NessaConversation::Action::"conversation.read_turns",
    resource is NessaConversation::Conversation
)
when {
    principal.organization == resource.organization &&
    principal.id == resource.owner &&
    !resource.deleted
};

// Any turn of the owner's agents may ask the owner a question (`ask_owner`),
// one another agent started included: a coordinator woken by a thread's
// report is who most needs to. It only asks; the owner's own next message is
// the answer, and changes nothing else.
@id("owner.ask_owner")
permit (
    principal is NessaConversation::AgentTurn,
    action == NessaConversation::Action::"conversation.ask_owner",
    resource is NessaConversation::Conversation
)
when {
    principal.organization == resource.organization &&
    principal.id == resource.owner &&
    !resource.deleted
};

// Only a turn the owner started asks the owner to raise the conversation's
// access (`request_access`): the red card is in Nessa's own words, so an
// agent nobody asked to work never puts "Allow full access" in front of the
// owner. Asking changes nothing; the owner's own access command raises it.
@id("owner.request_access")
permit (
    principal is NessaConversation::AgentTurn,
    action == NessaConversation::Action::"conversation.request_access",
    resource is NessaConversation::Conversation
)
when {
    principal.organization == resource.organization &&
    principal.id == resource.owner &&
    principal.ownerInitiated &&
    !resource.deleted
};

// A turn the owner started closes a thread its own thread's agent created
// (`close_thread` with a `threadId`). The SDK's record of the spawn names
// that creator; a thread no agent created, or another one created, is not
// this turn's to close.
@id("owner.close_created")
permit (
    principal is NessaConversation::AgentTurn,
    action == NessaConversation::Action::"conversation.close_created",
    resource is NessaConversation::Conversation
)
when {
    principal.organization == resource.organization &&
    principal.id == resource.owner &&
    principal.ownerInitiated &&
    resource has createdBy &&
    resource.createdBy == principal.conversation &&
    !resource.deleted
};

// The owner's agent reads what the owner's desktop windows show: which
// panes are open, where, and what each holds. An agent tool acts as the
// person whose turn admitted it (`conversation.agent_tools`), so the reader
// here is that person.
@id("owner.workspace_read")
permit (
    principal is NessaConversation::Person,
    action == NessaConversation::Action::"workspace.read",
    resource is NessaConversation::Workspace
)
when {
    principal.organization == resource.organization &&
    principal.id == resource.owner
};

// The owner's agent, or the owner's own CLI, changes what the owner's
// desktop windows show: opens and closes the side columns and the panel,
// opens, focuses and closes panes. Nobody else's windows, and never a paired
// device's session (refused before this, as a catalogue write). A pane's
// conversation is asked about separately, as `conversation.read`.
@id("owner.workspace_control")
permit (
    principal is NessaConversation::Person,
    action == NessaConversation::Action::"workspace.control",
    resource is NessaConversation::Workspace
)
when {
    principal.organization == resource.organization &&
    principal.id == resource.owner
};

// The owner makes channels in their own catalogue. Only this action takes a
// folder path; every later request names the channel, never the path.
@id("owner.channel_create")
permit (
    principal is NessaConversation::Person,
    action == NessaConversation::Action::"channel.create",
    resource is NessaConversation::Catalogue
)
when {
    principal.organization == resource.organization &&
    principal.id == resource.owner
};

// The owner lists their own channels.
@id("owner.channel_list")
permit (
    principal is NessaConversation::Person,
    action == NessaConversation::Action::"channel.list",
    resource is NessaConversation::Catalogue
)
when {
    principal.organization == resource.organization &&
    principal.id == resource.owner
};

// The owner starts conversations in their own channels, so their agents work
// in that channel's folder.
@id("owner.channel_use")
permit (
    principal is NessaConversation::Person,
    action == NessaConversation::Action::"channel.use",
    resource is NessaConversation::Channel
)
when {
    principal.organization == resource.organization &&
    principal.id == resource.owner
};

// The owner manages ordinary runtime settings for this channel.
@id("owner.channel_settings")
permit (
    principal is NessaConversation::Person,
    action == NessaConversation::Action::"channel.settings",
    resource is NessaConversation::Channel
)
when {
    principal.organization == resource.organization &&
    principal.id == resource.owner
};

// The owner changes any of their own settings, whatever its level: on their
// own surface the last step is always theirs.
@id("owner.setting_change")
permit (
    principal is NessaConversation::Person,
    action == NessaConversation::Action::"setting.change",
    resource is NessaConversation::Setting
)
when {
    principal.organization == resource.organization &&
    principal.id == resource.owner
};

// A turn the owner started changes one of their settings only at the level
// an agent may change: a reply style, a model. A setting the agent fills is
// saved by the owner, one only the owner types is never the agent's, and
// one that raises access waits for the owner's Approve on the red card.
@id("agent.setting_change")
permit (
    principal is NessaConversation::AgentTurn,
    action == NessaConversation::Action::"setting.change",
    resource is NessaConversation::Setting
)
when {
    principal.organization == resource.organization &&
    principal.id == resource.owner &&
    principal.ownerInitiated &&
    resource.level == "agent_changes"
};
