import type {
  GitHubIdentitySource,
  RelayInbox,
  RelayPolicyEvaluator,
  WakeTickets,
  RelayReadAudit,
} from "./ports.js"
export async function readRelay(
  input: { token: string; repositoryId: string; after: number; expectedUser: string },
  dependencies: {
    identities: GitHubIdentitySource
    policy: RelayPolicyEvaluator
    inbox: RelayInbox
    tickets: WakeTickets
    audit: RelayReadAudit
  },
) {
  const verified = await dependencies.identities.verify(input.token, input.repositoryId)
  if (verified.kind !== "verified") return { kind: verified.kind } as const
  if (verified.identity.userId !== input.expectedUser)
    return { kind: "identity_changed" } as const
  const decision = dependencies.policy.read(verified.identity, input.repositoryId)
  await dependencies.audit.record({
    userId: verified.identity.userId,
    repositoryId: input.repositoryId,
    allowed: decision.allowed,
    rules: decision.rules,
    after: input.after,
  })
  if (!decision.allowed) return { kind: "forbidden", rules: decision.rules } as const
  const page = await dependencies.inbox.page(input.repositoryId, input.after)
  if (input.after < page.expiredThrough)
    return { kind: "gap", expiredThrough: page.expiredThrough } as const
  return {
    kind: "page",
    userId: verified.identity.userId,
    events: page.events,
    hasMore: page.hasMore,
    next: page.events.at(-1)?.sequence ?? input.after,
    ticket: await dependencies.tickets.issue(
      verified.identity.userId,
      input.repositoryId,
    ),
  } as const
}
