forbid (
    principal is Nessa::Actor,
    action,
    resource is Nessa::Resource
)
when { principal.organizationId != resource.organizationId };

forbid (
    principal is Nessa::Actor,
    action,
    resource is Nessa::Resource
)
unless { context.credentialAllows };

forbid (
    principal is Nessa::Actor,
    action,
    resource is Nessa::Resource
)
unless { context.identityMatches };

permit (
    principal is Nessa::Actor,
    action == Nessa::Action::"server.read",
    resource is Nessa::Resource
)
when {
    principal.active &&
    (principal.role == "member" || principal.role == "admin")
};

permit (
    principal is Nessa::Actor,
    action == Nessa::Action::"credential.manage",
    resource is Nessa::Resource
)
when { principal.active && principal.role == "admin" };

permit (
    principal is Nessa::Actor,
    action == Nessa::Action::"conversation.write",
    resource is Nessa::Resource
)
when { principal.active && (principal.role == "member" || principal.role == "admin") };
