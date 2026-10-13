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

// Which grants a credential may carry at all (`GrantPolicyEvaluator`). A
// person, agent or integration may hold anything but serving as an
// environment; a peer gateway may only read what it is granted and serve as an
// environment, never the conversation authority's actions.
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

@id("hold.administration")
forbid (
    principal is Nessa::Holder,
    action == Nessa::Action::"credential.hold",
    resource is Nessa::Resource
)
when { context.action == "credential.manage" && !context.administrative };
