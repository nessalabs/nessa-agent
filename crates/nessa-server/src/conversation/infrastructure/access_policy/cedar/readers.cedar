// What a paired party may do in its grantor's conversations. The principal
// is a reader: the `organization` its binding and session are in, the
// principal it `signedInAs`, the `bindingOwner` whose conversations the
// binding reads, and its `receiver`. A device signs in as its binding's
// owner; a peer gateway signs in as itself. `context.role` is the role of
// the share grant it holds on the conversation, read now: "read",
// "comment" or "drive", absent when it holds none. Each role allows
// everything the roles before it do. `context.request` is what a command
// carries.

// Any grant reads the conversation it names, until it is deleted.
@id("grant.read")
permit (
    principal is NessaConversation::Reader,
    action == NessaConversation::Action::"conversation.read",
    resource is NessaConversation::Conversation
)
when {
    context has role &&
    principal.organization == resource.organization &&
    principal.bindingOwner == resource.owner &&
    !resource.deleted
};

// Any grant shows the conversation it names in the reader's catalogue, a
// tombstone included, so the reader learns it was deleted.
@id("grant.catalogue_entry")
permit (
    principal is NessaConversation::Reader,
    action == NessaConversation::Action::"catalogue.entry",
    resource is NessaConversation::Conversation
)
when {
    context has role &&
    principal.organization == resource.organization &&
    principal.bindingOwner == resource.owner
};

// A Comment or Drive grant leaves notes in the conversation it names.
@id("grant.comment")
permit (
    principal is NessaConversation::Reader,
    action == NessaConversation::Action::"conversation.comment",
    resource is NessaConversation::Conversation
)
when {
    context has role &&
    ["comment", "drive"].contains(context.role) &&
    principal.organization == resource.organization &&
    principal.bindingOwner == resource.owner &&
    !resource.deleted
};

// A Drive grant starts, steers and stops turns in the conversation it names.
@id("grant.drive")
permit (
    principal is NessaConversation::Reader,
    action == NessaConversation::Action::"conversation.drive",
    resource is NessaConversation::Conversation
)
when {
    context has role &&
    context.role == "drive" &&
    principal.organization == resource.organization &&
    principal.bindingOwner == resource.owner &&
    !resource.deleted
};

// A peer gateway signs in as its own principal and never comments or
// drives: forwarding its people's commands is a later slice.
@id("paired.peer_gateway")
forbid (
    principal is NessaConversation::Reader,
    action in [
        NessaConversation::Action::"conversation.comment",
        NessaConversation::Action::"conversation.drive"
    ],
    resource
)
when { principal.signedInAs != principal.bindingOwner };

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

// A catalogue watch fires on every change to the owner's catalogue, granted
// or not. A device signs in as that owner and may watch; a peer gateway is
// another party, and the timing of the owner's ungranted work is not its to
// see, so it polls its head instead.
@id("grant.catalogue_watch")
permit (
    principal is NessaConversation::Reader,
    action == NessaConversation::Action::"catalogue.watch",
    resource is NessaConversation::Catalogue
)
when {
    principal.signedInAs == principal.bindingOwner &&
    principal.organization == resource.organization &&
    principal.bindingOwner == resource.owner
};

// A note a Comment or Drive grant left is delivered while the grant still
// holds that role and the conversation is not deleted.
@id("grant.note_deliver")
permit (
    principal is NessaConversation::GrantHolder,
    action == NessaConversation::Action::"note.deliver",
    resource is NessaConversation::Conversation
)
when {
    context has role &&
    ["comment", "drive"].contains(context.role) &&
    !resource.deleted
};
