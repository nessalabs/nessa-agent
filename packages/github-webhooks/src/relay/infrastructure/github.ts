import type { GitHubIdentitySource, IdentityResult } from "../application/ports.js"

export class GitHubIdentityAdapter implements GitHubIdentitySource {
  constructor(private readonly fetcher: typeof fetch) {}
  async verify(token: string, repositoryId: string): Promise<IdentityResult> {
    const init = {
      headers: {
        Authorization: `Bearer ${token}`,
        Accept: "application/vnd.github+json",
        "User-Agent": "Nessa-GitHub-relay",
        "X-GitHub-Api-Version": "2022-11-28",
      },
      redirect: "manual" as const,
      cache: "no-store" as const,
      signal: AbortSignal.timeout(10_000),
    }
    try {
      const user = await this.fetcher("https://api.github.com/user", init)
      if (user.status === 401 || user.status === 403) return { kind: "unauthenticated" }
      if (!user.ok) return { kind: "unavailable" }
      const oauthScopes = (user.headers.get("x-oauth-scopes") ?? "")
        .split(",")
        .map((value) => value.trim())
        .filter(Boolean)
      const profile = (await user.json()) as { id?: unknown }
      if (!Number.isSafeInteger(profile.id) || Number(profile.id) <= 0)
        return { kind: "unavailable" }
      const repo = await this.fetcher(
        `https://api.github.com/repositories/${repositoryId}`,
        init,
      )
      if (repo.status === 401) return { kind: "unauthenticated" }
      if (repo.status === 404 || repo.status === 403)
        return {
          kind: "verified",
          identity: {
            userId: String(profile.id),
            repositoryIds: [],
            oauthScopes,
            repositoryIsPrivate: true,
          },
        }
      if (!repo.ok) return { kind: "unavailable" }
      const resource = (await repo.json()) as { id?: unknown; private?: unknown }
      if (!Number.isSafeInteger(resource.id) || String(resource.id) !== repositoryId)
        return { kind: "unavailable" }
      return {
        kind: "verified",
        identity: {
          userId: String(profile.id),
          repositoryIds: [repositoryId],
          oauthScopes,
          repositoryIsPrivate: resource.private !== false,
        },
      }
    } catch {
      return { kind: "unavailable" }
    }
  }
}
