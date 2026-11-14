// Who may share a conversation, and with whom. Sharing is the owner's: a
// paired reader is never permitted it, whoever it signs in as.
// `context.target` is the party a share grants to, as the receiver
// authority and the credential registry answer for it: the organization
// and owner its receiver binding names, the grants its credential holds,
// and this gateway, the resource those grants must be on.

// The owner shares a conversation that is not deleted with a party paired
// by them, in their organization, that holds `conversation.read` on this
// gateway: an environment's credential serves and never reads, so a share
// to it would record a reader that is not one.
@id("owner.share")
permit (
    principal is NessaConversation::Person,
    action == NessaConversation::Action::"conversation.share",
    resource is NessaConversation::Conversation
)
when {
    principal.organization == resource.organization &&
    principal.id == resource.owner &&
    !resource.deleted &&
    context has target &&
    context.target.organization == principal.organization &&
    context.target.bindingOwner == principal.id &&
    context.target.grants.contains({
        action: "conversation.read",
        organization: context.target.gateway.organization,
        resource: context.target.gateway.resource
    })
};

// Taking a share away needs no more than ownership: a deleted
// conversation's shares can still be revoked.
@id("owner.unshare")
permit (
    principal is NessaConversation::Person,
    action == NessaConversation::Action::"conversation.unshare",
    resource is NessaConversation::Conversation
)
when {
    principal.organization == resource.organization &&
    principal.id == resource.owner
};

// The owner lists a conversation's shares, deleted or not.
@id("owner.shares")
permit (
    principal is NessaConversation::Person,
    action == NessaConversation::Action::"conversation.shares",
    resource is NessaConversation::Conversation
)
when {
    principal.organization == resource.organization &&
    principal.id == resource.owner
};
