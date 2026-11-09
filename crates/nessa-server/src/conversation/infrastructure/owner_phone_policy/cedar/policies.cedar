// Raw identity and receiver facts; no computed access verdict enters this bundle.
@id("owner-phone.invalid-linkage")
forbid (principal is NessaPhone::Phone, action, resource is NessaPhone::Scope)
when {
    principal.owner != principal.purposeOwner ||
    principal.organization != principal.purposeOrganization ||
    principal.membership != principal.purposeMembership ||
    principal.audience != principal.purposeAudience ||
    principal.credential != principal.purposeCredential ||
    principal.purposeAudience != resource.gatewayId ||
    principal.receiverId != principal.purposeReceiver ||
    principal.receiverOwner != principal.purposeOwner ||
    principal.receiverOrganization != principal.purposeOrganization ||
    principal.receiverCredential != principal.purposeCredential ||
    !principal.receiverActive
};

@id("owner-phone.foreign-organization")
forbid (principal is NessaPhone::Phone, action, resource is NessaPhone::Scope)
when { principal.purposeOrganization != resource.organization };

@id("owner-phone.current-linkage")
permit (principal is NessaPhone::Phone, action == NessaPhone::Action::"phone.context", resource is NessaPhone::Scope);

// Current and future methods default deny. Coarse grants are separately asked
// of Auth; these published action facts prevent stale classification permits.
@id("owner-phone.chat-methods")
permit (principal is NessaPhone::Phone, action == NessaPhone::Action::"product.request", resource is NessaPhone::Scope)
when {
    context has method && context has publishedAction &&
    context.publishedAction == "conversation.write" &&
    ["conversation.create", "conversation.read", "conversation.list", "conversation.observe",
     "conversation.send", "conversation.steer", "conversation.remove", "conversation.stop",
     "conversation.receipt", "conversation.message", "conversation.messageStatus",
     "conversation.answer", "conversation.permissionReview", "conversation.answerQuestion",
     "conversation.questionReview", "conversation.cancel",
     "conversation.close", "conversation.archive", "conversation.unarchive", "conversation.delete",
     "conversation.rename", "conversation.reorder", "conversation.subscribe", "conversation.subscribeList",
     "conversation.subscribeEvents", "conversation.peek", "conversation.readEarlier",
     "conversation.unsubscribe", "attachment.begin",
     "attachment.write"].contains(context.method)
};
// A live conversation is conversation.subscribe. The chat list is
// conversation.subscribeList. The payloadless watch methods are not this purpose.

// The phone's sidebar lists the owner's channels. Listing reads only the
// catalogue; making a channel names a folder on the owner's machine, which
// stays the desktop's to do, so channel.create is not this purpose.
@id("owner-phone.channel-list")
permit (principal is NessaPhone::Phone, action == NessaPhone::Action::"product.request", resource is NessaPhone::Scope)
when {
    context has method && context has publishedAction &&
    context.publishedAction == "conversation.write" && context.method == "channel.list"
};

@id("owner-phone.native-read-methods")
permit (principal is NessaPhone::Phone, action == NessaPhone::Action::"product.request", resource is NessaPhone::Scope)
when {
    context has method && context has publishedAction &&
    context.publishedAction == "conversation.read" &&
    ["conversation.recordsHead", "conversation.recordsPage", "conversation.catalogueHead",
     "conversation.catalogueManifest", "conversation.catalogueResolve", "artifact.read"].contains(context.method)
};

@id("owner-phone.current-session")
permit (principal is NessaPhone::Phone, action == NessaPhone::Action::"product.request", resource is NessaPhone::Scope)
when { context has method && context.method == "auth.session" && !(context has publishedAction) };

@id("owner-phone.configured-runtimes")
permit (principal is NessaPhone::Phone, action == NessaPhone::Action::"product.request", resource is NessaPhone::Scope)
when {
    context has method && context has publishedAction && context.publishedAction == "server.read" &&
    ["server.health", "server.reach", "agents.list", "agents.subscribe", "agents.unsubscribe"].contains(context.method)
};

// The service owns the default fact; Cedar owns which creation selections this purpose permits.
@id("owner-phone.creation-approval")
permit (principal is NessaPhone::Phone, action == NessaPhone::Action::"conversation.create", resource is NessaPhone::Scope)
when {
    (context has requestedMode && context.requestedMode == "ask") ||
    (!(context has requestedMode) && context.defaultMode == "ask")
};

@id("owner-phone.submission-input")
permit (principal is NessaPhone::Phone, action == NessaPhone::Action::"conversation.input", resource is NessaPhone::Scope)
when { context.linkedFiles == 0 };

// A phone answer may be once. Remembering a permission rule is this action,
// and this purpose does not grant it. A later permit would be the decision
// that a phone may remember; this forbid is that decision today.
@id("owner-phone.remember")
forbid (principal is NessaPhone::Phone, action == NessaPhone::Action::"conversation.remember", resource is NessaPhone::Scope);
