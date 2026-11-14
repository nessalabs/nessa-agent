// Role, activity, organization, and grants are facts. No Rust verdict is copied in.
// A forbid wins over the permit, so a member who holds the grant is still refused.

@id("usage-limit-preference.member")
forbid (
    principal is NessaUsageLimit::Caller,
    action == NessaUsageLimit::Action::"preference.set",
    resource is NessaUsageLimit::Gateway
)
when { principal.role == "member" };

@id("usage-limit-preference.inactive")
forbid (
    principal is NessaUsageLimit::Caller,
    action == NessaUsageLimit::Action::"preference.set",
    resource is NessaUsageLimit::Gateway
)
when { !principal.active };

@id("usage-limit-preference.foreign-organization")
forbid (
    principal is NessaUsageLimit::Caller,
    action == NessaUsageLimit::Action::"preference.set",
    resource is NessaUsageLimit::Gateway
)
when { principal.organizationId != resource.organizationId };

@id("usage-limit-preference.admin-grant")
permit (
    principal is NessaUsageLimit::Caller,
    action == NessaUsageLimit::Action::"preference.set",
    resource is NessaUsageLimit::Gateway
)
when {
    principal.active &&
    principal.role == "admin" &&
    principal.organizationId == resource.organizationId &&
    principal.grants.contains({
        action: "credential.manage",
        resourceId: resource.id,
        organizationId: resource.organizationId
    })
};
