/**
 * A `SharingSource` held in memory: the sample workspace's linked devices
 * and command hosts, and the grants made on its sessions while the window
 * lives. What the window shares through without a gateway — previews and
 * tests — and nothing a gateway would not say: its bounds are the ones the
 * schema publishes (`toolPolicyBounds`), and a refusal is one a test asks
 * for (`refuse`), as a typed `SharingError`.
 *
 * What it holds is replaced, never changed: a read hands out values that stay
 * as they were.
 */
import { toolPolicyBounds } from "@nessa/client"
import { SharingError, type SharingSource } from "../../application/ports"
import type {
  ShareGrant,
  ShareTarget,
  SharingFailure,
  SharingRead,
} from "../../model/sharing"

/** The sample's linked devices. */
export const sampleShareTargets: readonly ShareTarget[] = [
  { credentialId: "sample-phone", issuedAt: 1_760_000_000 },
  { credentialId: "sample-tablet", issuedAt: 1_759_000_000 },
]

/** The sample's hosts the owner's turns may run commands on. */
export const sampleCommandHosts: readonly string[] = ["devbox", "me@build.example"]

export interface InMemorySharing extends SharingSource {
  /** The calls made, in order, with what they named. */
  readonly calls: readonly {
    readonly method: "read" | "share" | "unshare"
    readonly sessionId: string
    readonly credentialId?: string
    readonly requestId?: string
  }[]
}

export function inMemorySharing(
  options: {
    readonly targets?: readonly ShareTarget[]
    readonly commandHosts?: readonly string[]
    readonly now?: () => number
    /** A refusal for a call, when a test asks for one; nothing is changed then. */
    readonly refuse?: (method: "read" | "share" | "unshare") => SharingFailure | undefined
  } = {},
): InMemorySharing {
  const targets = options.targets ?? sampleShareTargets
  const commandHosts = options.commandHosts ?? sampleCommandHosts
  const now = options.now ?? (() => Date.now())
  let grants: Readonly<Record<string, readonly ShareGrant[]>> = {}
  const calls: InMemorySharing["calls"][number][] = []
  const held = (sessionId: string) =>
    Object.hasOwn(grants, sessionId) ? grants[sessionId] : []
  const refused = (method: "read" | "share" | "unshare") => {
    const reason = options.refuse?.(method)
    if (reason) throw new SharingError(reason)
  }
  return {
    calls,
    async read(sessionId): Promise<SharingRead> {
      calls.push({ method: "read", sessionId })
      refused("read")
      return {
        grants: held(sessionId),
        targets,
        commandHosts,
        limits: toolPolicyBounds,
      }
    },
    async share(sessionId, request, requestId) {
      calls.push({
        method: "share",
        sessionId,
        credentialId: request.credentialId,
        requestId,
      })
      refused("share")
      if (!targets.some((target) => target.credentialId === request.credentialId))
        throw new SharingError("not-paired")
      const before = held(sessionId)
      const existing = before.find((grant) => grant.credentialId === request.credentialId)
      // As the gateway does: the same role and policy again changes nothing,
      // and the policy revision moves only with a change.
      if (
        existing &&
        existing.role === request.role &&
        JSON.stringify(existing.policy) === JSON.stringify(request.policy)
      )
        return
      const grant: ShareGrant = {
        credentialId: request.credentialId,
        role: request.role,
        ...(request.policy ? { policy: request.policy } : {}),
        policyRevision: existing ? existing.policyRevision + 1 : 0,
        grantedAtMs: existing?.grantedAtMs ?? now(),
      }
      grants = {
        ...grants,
        [sessionId]: existing
          ? before.map((each) => (each === existing ? grant : each))
          : [...before, grant],
      }
    },
    async unshare(sessionId, credentialId, requestId) {
      calls.push({ method: "unshare", sessionId, credentialId, requestId })
      refused("unshare")
      grants = {
        ...grants,
        [sessionId]: held(sessionId).filter(
          (grant) => grant.credentialId !== credentialId,
        ),
      }
    },
  }
}
