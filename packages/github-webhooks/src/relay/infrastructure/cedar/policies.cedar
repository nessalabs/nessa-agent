@id("github-current-repository-access")
permit(principal is Relay::User, action == Relay::Action::"read", resource is Relay::Repository)
when { principal.repositories.contains(resource) && (!resource.isPrivate || principal.oauthScopes.contains("repo") || (
    principal.installationActive && principal.appRepositories.contains(resource) &&
    (principal.installationPermissions.contains("contents:read") || principal.installationPermissions.contains("contents:write")) &&
    (principal.installationPermissions.contains("issues:read") || principal.installationPermissions.contains("issues:write")) &&
    (principal.installationPermissions.contains("pull_requests:read") || principal.installationPermissions.contains("pull_requests:write")) &&
    (principal.installationPermissions.contains("actions:read") || principal.installationPermissions.contains("actions:write")) &&
    (principal.installationPermissions.contains("checks:read") || principal.installationPermissions.contains("checks:write"))
)) };
