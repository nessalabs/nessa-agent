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

// The owner lists their own conversations. A paired reader lists through
// its catalogue, never the owner's list, so no permit names one.
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
