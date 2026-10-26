/**
 * A `SharingSource` held in memory: the sample workspace's linked devices
 * and command hosts, and the grants made on its sessions while the window
 * lives. What the window shares through without a gateway — previews and
 * tests — and nothing a gateway would not say: its bounds are the ones the
 * schema publishes (`toolPolicyBounds`); its policy revisions, as the
 * gateway's, come from one count that only goes up, so a grant made after a
 * revoke never takes a revision seen before; a change naming a revision the
 * grant no longer holds is refused `changed`; and any other refusal is one a
 * test asks for (`refuse`), as a typed `SharingError`.
 *
 * What it holds is replaced, never changed: a read hands out values that stay
 * as they were.
 */
import { toolPolicyBounds } from "@nessa/client"
import { SharingError, type SharingSource } from "../../application/ports"
import type { ShareGrant, ShareTarget, SharingFailure } from "../../model/sharing"

/** The sample's linked devices. */
export const sampleShareTargets: readonly ShareTarget[] = [
  { credentialId: "sample-phone", issuedAt: 1_760_000_000, fullAccess: false },
  { credentialId: "sample-tablet", issuedAt: 1_759_000_000, fullAccess: false },
]

/** The sample's hosts the owner's turns may run commands on. */
export const sampleCommandHosts: readonly string[] = ["devbox", "me@build.example"]

type Method = "grants" | "choices" | "share" | "unshare"

export interface InMemorySharing extends SharingSource {
  /** The calls made, in order, with what they named. */
  readonly calls: readonly {
    readonly method: Method
    readonly sessionId?: string
    readonly credentialId?: string
    readonly expectedPolicyRevision?: number
    readonly requestId?: string
  }[]
}

export function inMemorySharing(options: {
  readonly targets?: readonly ShareTarget[]
  readonly commandHosts?: readonly string[]
  /** The window's clock, which composition injects; tests pass their own. */
  readonly now: () => number
  /** A refusal for a call, when a test asks for one; nothing is changed then. */
  readonly refuse?: (method: Method) => SharingFailure | undefined
}): InMemorySharing {
  const targets = options.targets ?? sampleShareTargets
  const commandHosts = options.commandHosts ?? sampleCommandHosts
  const now = options.now
  let grants: Readonly<Record<string, readonly ShareGrant[]>> = {}
  // The last revision handed out, for every grant of every session.
  let revision = 0
  const calls: InMemorySharing["calls"][number][] = []
  const held = (sessionId: string) =>
    Object.hasOwn(grants, sessionId) ? grants[sessionId] : []
  const refused = (method: Method) => {
    const reason = options.refuse?.(method)
    if (reason) throw new SharingError(reason)
  }
  /** The device's grant, once checked against the revision a change expects. */
  const expecting = (sessionId: string, credentialId: string, expected: number) => {
    const grant = held(sessionId).find((each) => each.credentialId === credentialId)
    // As the gateway: 0 names no grant held, since a revision starts at 1.
    if ((grant?.policyRevision ?? 0) !== expected) throw new SharingError("changed")
    return grant
  }
  return {
    calls,
    async grants(sessionId) {
      calls.push({ method: "grants", sessionId })
      refused("grants")
      return held(sessionId)
    },
    async choices() {
      calls.push({ method: "choices" })
      refused("choices")
      return { targets, commandHosts, limits: toolPolicyBounds }
    },
    async share(sessionId, request, requestId) {
      calls.push({
        method: "share",
        sessionId,
        credentialId: request.credentialId,
        expectedPolicyRevision: request.expectedPolicyRevision,
        requestId,
      })
      refused("share")
      if (!targets.some((target) => target.credentialId === request.credentialId))
        throw new SharingError("not-paired")
      const existing = expecting(
        sessionId,
        request.credentialId,
        request.expectedPolicyRevision,
      )
      // As the gateway does: the same role and policy again changes nothing,
      // and the policy revision moves only with a change.
      if (
        existing &&
        existing.role === request.role &&
        JSON.stringify(existing.policy) === JSON.stringify(request.policy)
      )
        return existing.policyRevision
      revision += 1
      const grant: ShareGrant = {
        credentialId: request.credentialId,
        role: request.role,
        ...(request.policy ? { policy: request.policy } : {}),
        policyRevision: revision,
        grantedAtMs: existing?.grantedAtMs ?? now(),
      }
      const before = held(sessionId)
      grants = {
        ...grants,
        [sessionId]: existing
          ? before.map((each) => (each === existing ? grant : each))
          : [...before, grant],
      }
      return revision
    },
    async unshare(sessionId, credentialId, expectedPolicyRevision, requestId) {
      calls.push({
        method: "unshare",
        sessionId,
        credentialId,
        expectedPolicyRevision,
        requestId,
      })
      refused("unshare")
      expecting(sessionId, credentialId, expectedPolicyRevision)
      grants = {
        ...grants,
        [sessionId]: held(sessionId).filter(
          (grant) => grant.credentialId !== credentialId,
        ),
      }
    },
  }
}
