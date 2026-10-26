export type GitHubIdentity = Readonly<{
  userId: string
  repositoryIds: readonly string[]
  oauthScopes: readonly string[]
  installationActive?: boolean
  appRepositoryIds?: readonly string[]
  installationPermissions?: readonly string[]
  repositoryIsPrivate: boolean
}>
export type IdentityResult =
  | { kind: "verified"; identity: GitHubIdentity }
  | { kind: "unauthenticated" | "unavailable" }
export interface GitHubIdentitySource {
  verify(token: string, repositoryId: string): Promise<IdentityResult>
}
export interface RelayPolicyEvaluator {
  read(
    identity: GitHubIdentity,
    repositoryId: string,
  ): { allowed: boolean; rules: readonly string[] }
}
export interface RelayEvent {
  sequence: number
  deliveryId: string
  event: string
  payload: string
  receivedAt: string
  repositoryId: string
}
export type RelayPage = Readonly<{
  expiredThrough: number
  hasMore: boolean
  events: readonly RelayEvent[]
}>
export interface RelayInbox {
  page(repositoryId: string, after: number): Promise<RelayPage>
  expire(before: string): Promise<void>
}
export interface WakeTickets {
  issue(userId: string, repositoryId: string): Promise<string>
  verify(
    ticket: string,
  ): Promise<{ userId: string; repositoryId: string; expiresAt: number } | null>
}

export interface RelayReadAudit {
  record(input: {
    userId: string
    repositoryId: string
    allowed: boolean
    rules: readonly string[]
    after: number
  }): Promise<void>
}

export interface RelayReadBudget {
  admit(userId: string): Promise<boolean>
}
