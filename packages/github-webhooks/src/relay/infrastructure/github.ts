import { relayLimits } from "../domain/cursor.js"
import type { GitHubIdentitySource, IdentityResult } from "../application/ports.js"

export class GitHubIdentityAdapter implements GitHubIdentitySource {
  constructor(
    private readonly fetcher: typeof fetch,
    private readonly appId: number,
  ) {}
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
      const profile = (await providerJson(user)) as { id?: unknown }
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
      const resource = (await providerJson(repo)) as { id?: unknown; private?: unknown }
      if (!Number.isSafeInteger(resource.id) || String(resource.id) !== repositoryId)
        return { kind: "unavailable" }
      const installation =
        resource.private !== false && !oauthScopes.includes("repo")
          ? await this.installation(repositoryId, init)
          : {
              appRepositoryIds: [],
              installationPermissions: [],
              installationActive: false,
            }
      return {
        kind: "verified",
        identity: {
          ...installation,
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
  private async installation(repositoryId: string, init: RequestInit) {
    const empty = {
      appRepositoryIds: [] as string[],
      installationPermissions: [] as string[],
      installationActive: false,
    }
    const response = await this.fetcher(
      "https://api.github.com/user/installations?per_page=100",
      init,
    )
    if (response.status === 403 || response.status === 404) return empty
    if (!response.ok) throw new Error("installation lookup unavailable")
    const data = (await providerJson(response)) as {
      total_count: number
      installations: {
        id: number
        app_id: number
        suspended_at: string | null
        permissions: Record<string, string>
      }[]
    }
    if (
      !Array.isArray(data.installations) ||
      data.total_count !== data.installations.length ||
      data.installations.length > 100
    )
      throw new Error("installation lookup incomplete")
    // Bound discovery work. A repository outside this window is refused, not
    // authorized from metadata visibility. The provider supplies every fact.
    let remaining = 8
    for (const installation of data.installations) {
      if (
        installation.app_id !== this.appId ||
        !Number.isSafeInteger(installation.id) ||
        installation.id <= 0
      )
        continue
      for (let page = 1; remaining > 0; page++) {
        remaining--
        const response = await this.fetcher(
          `https://api.github.com/user/installations/${installation.id}/repositories?per_page=100&page=${page}`,
          init,
        )
        if (response.status === 403 || response.status === 404) break
        if (!response.ok) throw new Error("installation repositories unavailable")
        const data = (await providerJson(response)) as {
          total_count: number
          repositories: { id: number }[]
        }
        if (
          !Array.isArray(data.repositories) ||
          data.repositories.length > 100 ||
          !Number.isSafeInteger(data.total_count)
        )
          throw new Error("installation repositories malformed")
        if (
          data.repositories.some(
            (repo) => Number.isSafeInteger(repo.id) && String(repo.id) === repositoryId,
          )
        )
          return {
            appRepositoryIds: [repositoryId],
            installationPermissions: Object.entries(installation.permissions ?? {}).map(
              ([name, value]) => `${name}:${value}`,
            ),
            installationActive: installation.suspended_at === null,
          }
        if (page * 100 >= data.total_count) break
      }
    }
    return empty
  }
}

async function providerJson(response: Response): Promise<unknown> {
  const reader = response.body?.getReader()
  if (!reader) throw new Error("provider body absent")
  const chunks: Uint8Array[] = []
  let size = 0
  try {
    while (true) {
      const next = await reader.read()
      if (next.done) break
      size += next.value.byteLength
      if (size > relayLimits.pageBytes) {
        await reader.cancel()
        throw new Error("provider response too large")
      }
      chunks.push(next.value)
    }
  } finally {
    reader.releaseLock()
  }
  const bytes = new Uint8Array(size)
  let at = 0
  for (const chunk of chunks) {
    bytes.set(chunk, at)
    at += chunk.byteLength
  }
  return JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes)) as unknown
}
