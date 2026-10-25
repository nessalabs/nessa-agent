@id("github-current-repository-access")
permit(principal is Relay::User, action == Relay::Action::"read", resource is Relay::Repository)
when { principal.repositories.contains(resource) && (!resource.isPrivate || principal.oauthScopes.contains("repo")) };
