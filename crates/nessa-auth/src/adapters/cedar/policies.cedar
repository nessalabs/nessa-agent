@id("organization.same")
forbid (
    principal is Nessa::Actor,
    action,
    resource is Nessa::Resource
)
when { principal.organizationId != resource.organizationId };

@id("server.read")
permit (
    principal is Nessa::Actor,
    action == Nessa::Action::"server.read",
    resource is Nessa::Resource
)
when {
    principal.active &&
    (principal.role == "member" || principal.role == "admin") &&
    principal.grants.contains({ action: "server.read", resourceId: resource.id, organizationId: resource.organizationId })
};

@id("credential.manage")
permit (
    principal is Nessa::Actor,
    action == Nessa::Action::"credential.manage",
    resource is Nessa::Resource
)
when {
    principal.active &&
    principal.role == "admin" &&
    principal.grants.contains({ action: "credential.manage", resourceId: resource.id, organizationId: resource.organizationId })
};

@id("conversation.read")
permit (
    principal is Nessa::Actor,
    action == Nessa::Action::"conversation.read",
    resource is Nessa::Resource
)
when {
    principal.active &&
    (principal.role == "member" || principal.role == "admin") &&
    principal.grants.contains({ action: "conversation.read", resourceId: resource.id, organizationId: resource.organizationId })
};

@id("conversation.write")
permit (
    principal is Nessa::Actor,
    action == Nessa::Action::"conversation.write",
    resource is Nessa::Resource
)
when {
    principal.active &&
    (principal.role == "member" || principal.role == "admin") &&
    principal.grants.contains({ action: "conversation.write", resourceId: resource.id, organizationId: resource.organizationId })
};

@id("environment.serve")
permit (
    principal is Nessa::Actor,
    action == Nessa::Action::"environment.serve",
    resource is Nessa::Resource
)
when {
    principal.active &&
    principal.role == "member" &&
    principal.grants.contains({ action: "environment.serve", resourceId: resource.id, organizationId: resource.organizationId })
};

// Which grants a credential may carry at all (`GrantPolicyEvaluator::may_hold`).
// A person, agent or integration may hold anything but serving as an
// environment; a peer gateway may only read what it is granted and serve as an
// environment, never the conversation authority's actions. `context.path` is
// the way the credential comes in: "bootstrap", "product", "provision",
// "recovery", "webhook", or "stored" for one already in the registry.
@id("hold.person")
permit (
    principal is Nessa::Holder,
    action == Nessa::Action::"credential.hold",
    resource is Nessa::Resource
)
when {
    ["human", "agent", "integration"].contains(principal.kind) &&
    ["server.read", "credential.manage", "conversation.read", "conversation.write"].contains(context.action)
};

@id("hold.peer")
permit (
    principal is Nessa::Holder,
    action == Nessa::Action::"credential.hold",
    resource is Nessa::Resource
)
when {
    principal.kind == "gateway" &&
    ["conversation.read", "environment.serve"].contains(context.action)
};

// An administrator signed in over the product protocol may not hand out
// administration: only the local operator's offline paths may.
@id("hold.administration")
forbid (
    principal is Nessa::Holder,
    action == Nessa::Action::"credential.hold",
    resource is Nessa::Resource
)
when { context.action == "credential.manage" && context.path == "product" };

// Who a new credential may be issued to (`GrantPolicyEvaluator::may_issue`).
// A peer gateway is never issued one: it is enrolled only by pairing, which
// binds its credential to the key it pinned, and an issued bearer secret
// would be a second way in. The webhook path further restricts members to
// integrations. An admin requires the local operator's path (bootstrap, surface
// provisioning and owner recovery), never over the product protocol.
@id("issue.member")
permit (
    principal is Nessa::Recipient,
    action == Nessa::Action::"credential.issue",
    resource is Nessa::Resource
)
when {
    ["human", "agent", "integration"].contains(principal.kind) &&
    principal.role == "member"
};

@id("issue.admin")
permit (
    principal is Nessa::Recipient,
    action == Nessa::Action::"credential.issue",
    resource is Nessa::Resource
)
when {
    ["human", "agent", "integration"].contains(principal.kind) &&
    principal.role == "admin" &&
    ["bootstrap", "provision", "recovery"].contains(context.path)
};

// Outbound registration asks the same credential owner with its issuing path.
// It cannot turn a person, agent or paired peer into a webhook principal, or
// give that credential administration, catalogue, write or serving authority.
@id("hold.webhook_read_only")
forbid (
    principal is Nessa::Holder,
    action == Nessa::Action::"credential.hold",
    resource is Nessa::Resource
)
when {
    context.path == "webhook" &&
    (principal.kind != "integration" || context.action != "conversation.read")
};

@id("issue.webhook_integration")
forbid (
    principal is Nessa::Recipient,
    action == Nessa::Action::"credential.issue",
    resource is Nessa::Resource
)
when { context.path == "webhook" && principal.kind != "integration" };

// Issuance purpose is the original journal fact, not current registration presence.
@id("session.product_origin")
permit (
    principal is Nessa::Actor,
    action == Nessa::Action::"session.enter",
    resource is Nessa::Resource
)
when {
    principal.active &&
    ["bootstrap", "admin_issue", "device_pairing", "surface_provision", "owner_recovery"].contains(principal.issuance)
};
