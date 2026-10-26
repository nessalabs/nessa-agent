// What a paired party may do in its grantor's conversations. The principal
// is a reader: the `organization` its binding and session are in, the
// principal it `signedInAs`, the `bindingOwner` whose conversations the
// binding reads, and its `receiver`. A device signs in as its binding's
// owner; a peer gateway signs in as itself. `context.capabilities` is the
// capability set of the grant it holds on the conversation, read now:
// "read", and when granted "comment" and "drive", absent when it holds
// none. `context.request` is what a command carries.
//
// Comment and drive require the capability and a principal signed in as
// the binding owner. That is the device case. A gateway signs in as itself,
// so those permits do not match it even when a row names the capability.

// A device may ask for its scoped list even when it has no conversation grants.
// The list index narrows rows; grant.read still admits each candidate. A peer
// signed in as itself retains its existing catalogue-only read path.
@id("grant.list")
permit (
    principal is NessaConversation::Reader,
    action == NessaConversation::Action::"conversation.list",
    resource is NessaConversation::Catalogue
)
when {
    principal.organization == resource.organization &&
    principal.bindingOwner == resource.owner &&
    principal.signedInAs == principal.bindingOwner
};

// A grant that includes read reads the conversation it names, until it is deleted.
@id("grant.read")
permit (
    principal is NessaConversation::Reader,
    action == NessaConversation::Action::"conversation.read",
    resource is NessaConversation::Conversation
)
when {
    context has capabilities &&
    context.capabilities.contains("read") &&
    principal.organization == resource.organization &&
    principal.bindingOwner == resource.owner &&
    !resource.deleted
};

// A grant that includes read shows the conversation it names in the reader's
// catalogue, a tombstone included, so the reader learns it was deleted.
@id("grant.catalogue_entry")
permit (
    principal is NessaConversation::Reader,
    action == NessaConversation::Action::"catalogue.entry",
    resource is NessaConversation::Conversation
)
when {
    context has capabilities &&
    context.capabilities.contains("read") &&
    principal.organization == resource.organization &&
    principal.bindingOwner == resource.owner
};

// A grant that includes comment leaves notes in the conversation it names,
// and only when the principal is signed in as the binding owner.
@id("grant.comment")
permit (
    principal is NessaConversation::Reader,
    action == NessaConversation::Action::"conversation.comment",
    resource is NessaConversation::Conversation
)
when {
    context has capabilities &&
    context.capabilities.contains("comment") &&
    principal.signedInAs == principal.bindingOwner &&
    principal.organization == resource.organization &&
    principal.bindingOwner == resource.owner &&
    !resource.deleted
};

// A grant that includes drive starts, steers and stops turns in the
// conversation it names, and only when the principal is signed in as the
// binding owner.
@id("grant.drive")
permit (
    principal is NessaConversation::Reader,
    action == NessaConversation::Action::"conversation.drive",
    resource is NessaConversation::Conversation
)
when {
    context has capabilities &&
    context.capabilities.contains("drive") &&
    principal.signedInAs == principal.bindingOwner &&
    principal.organization == resource.organization &&
    principal.bindingOwner == resource.owner &&
    !resource.deleted
};

// A driver sends text. An image is named by a digest in the owner's
// attachment store and a linked file by a path on the owner's machine, so a
// driver could name what was never shared with it; and a receipt names only
// a command a driver may make, never a creation.
@id("drive.text_only")
forbid (
    principal is NessaConversation::Reader,
    action == NessaConversation::Action::"conversation.drive",
    resource
)
when {
    context.request.attachments ||
    context.request.files ||
    (context.request has receiptOperation &&
     context.request.receiptOperation == "create")
};

// A note a grant that includes comment left is delivered while the grant
// still holds that capability and the conversation is not deleted.
@id("grant.note_deliver")
permit (
    principal is NessaConversation::GrantHolder,
    action == NessaConversation::Action::"note.deliver",
    resource is NessaConversation::Conversation
)
when {
    context has capabilities &&
    context.capabilities.contains("comment") &&
    !resource.deleted
};

// Any other owner command a paired reader sends is `conversation.write` on
// the catalogue its binding reads. No capability names one, so no permit
// does: a device never deletes, archives, exports, re-shares or changes
// the model.
