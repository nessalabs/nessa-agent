/**
 * Sharing over the gateway client: what is read and how, what is sent, and
 * each refusal as the kind the dialog says in words.
 */
import {
  ConversationErrorCode,
  NessaConversationControlError,
  NessaConversationMutationError,
  NessaRpcError,
  toolPolicyBounds,
  type AgentsListResult,
  type ConversationSharesResult,
} from "@nessa/client"
import { describe, expect, it, vi } from "vitest"
import { SharingError } from "../../application/ports"
import {
  failureOf,
  sharingGateway,
  wirePolicy,
  type SharingClient,
} from "./sharing-gateway"

function credential(id: string, actions: string[], revokedAt: number | null = null) {
  return {
    id,
    principalId: "owner",
    organizationId: "personal",
    audienceId: "gateway",
    issuedAt: 1_700_000_000,
    expiresAt: null,
    revokedAt,
    grants: actions.map((action) => ({
      action,
      resource: { organizationId: "personal", id: "gateway" },
    })),
  }
}

const shares: ConversationSharesResult = {
  items: [
    { credentialId: "phone", role: "read", policyRevision: 0, grantedAtMs: 5 },
    {
      credentialId: "tablet",
      role: "drive",
      policy: { allow: ["Read"], deny: [] },
      policyRevision: 3,
      grantedAtMs: 6,
    },
    {
      credentialId: "laptop",
      role: "drive",
      policy: { preset: "no_shell", commandHosts: ["devbox"] },
      policyRevision: 1,
      grantedAtMs: 7,
    },
  ],
}

function client(partial: { shares?: SharingClient["conversation"]["shares"] } = {}) {
  const fake = {
    conversation: {
      shares: vi.fn(partial.shares ?? (async () => shares)),
      share: vi.fn(async () => ({ requestId: "r", applied: true })),
      unshare: vi.fn(async () => ({ requestId: "r", applied: true })),
    },
    credentials: {
      list: vi.fn(async () => ({
        credentials: [
          credential("session-credential", ["conversation.read"]),
          credential("phone", ["conversation.read"]),
          credential("revoked", ["conversation.read"], 1_700_000_100),
          credential("admin", ["conversation.read", "conversation.write"]),
          credential("tablet", ["conversation.read"]),
        ],
      })),
    },
    auth: {
      session: vi.fn(async () => ({
        version: 1 as const,
        gatewayId: "gateway",
        principalId: "owner",
        organizationId: "personal",
        membershipId: "membership",
        credentialId: "session-credential",
        audienceId: "gateway",
        expiresAt: null,
        grants: [],
        methods: [],
      })),
    },
    agents: {
      list: vi.fn(
        async () =>
          ({
            agents: [],
            environments: ["here", "devbox"],
            commandHosts: ["devbox"],
            nativeHooks: [],
          }) as unknown as AgentsListResult,
      ),
    },
  }
  return fake as typeof fake & SharingClient
}

describe("reading a session's sharing", () => {
  it("reads its grants, the linked devices, the owner's command hosts and the schema's bounds", async () => {
    const gateway = client()
    const read = await sharingGateway({ connected: async () => gateway }).read("c1")
    expect(gateway.conversation.shares).toHaveBeenCalledWith("c1")
    expect(read.grants).toEqual([
      { credentialId: "phone", role: "read", policyRevision: 0, grantedAtMs: 5 },
      {
        credentialId: "tablet",
        role: "drive",
        policy: { kind: "rules", allow: ["Read"], deny: [], commandHosts: [] },
        policyRevision: 3,
        grantedAtMs: 6,
      },
      {
        credentialId: "laptop",
        role: "drive",
        policy: { kind: "preset", preset: "no_shell", commandHosts: ["devbox"] },
        policyRevision: 1,
        grantedAtMs: 7,
      },
    ])
    // Not the session's own, not revoked, not one holding more than reading.
    expect(read.targets.map((target) => target.credentialId)).toEqual(["phone", "tablet"])
    expect(read.commandHosts).toEqual(["devbox"])
    expect(read.limits).toBe(toolPolicyBounds)
    expect(read.limits).toMatchObject({
      patterns: 16,
      hosts: 16,
      hostBytes: 96,
      entries: 32,
    })
  })

  it("is refused as a typed kind when any part is", async () => {
    const gateway = client({
      shares: async () => {
        throw new NessaRpcError("conversation_not_found", "no")
      },
    })
    await expect(
      sharingGateway({ connected: async () => gateway }).read("c1"),
    ).rejects.toEqual(new SharingError("not-found"))
  })

  it("is unanswered when no connection could be made", async () => {
    const source = sharingGateway({
      connected: () => Promise.reject(new Error("offline")),
    })
    await expect(source.read("c1")).rejects.toEqual(new SharingError("unanswered"))
  })

  it("lets a fault in reading an answer through as one, not as the gateway's refusal", async () => {
    // An answer this build cannot read: no credentials list in it.
    const gateway = {
      ...client(),
      credentials: { list: async () => ({}) },
    } as unknown as SharingClient
    const failure = await sharingGateway({ connected: async () => gateway })
      .read("c1")
      .catch((error: unknown) => error)
    expect(failure).toBeInstanceOf(TypeError)
  })
})

describe("sharing and revoking", () => {
  it("sends the role, the policy as the wire takes it, and the request id", async () => {
    const gateway = client()
    const source = sharingGateway({ connected: async () => gateway })
    await source.share("c1", { credentialId: "phone", role: "read" }, "req-1")
    expect(gateway.conversation.share).toHaveBeenLastCalledWith(
      "c1",
      "phone",
      "read",
      undefined,
      { requestId: "req-1" },
    )
    await source.share(
      "c1",
      {
        credentialId: "phone",
        role: "drive",
        policy: { kind: "rules", allow: ["Read"], deny: [], commandHosts: ["devbox"] },
      },
      "req-2",
    )
    expect(gateway.conversation.share).toHaveBeenLastCalledWith(
      "c1",
      "phone",
      "drive",
      { allow: ["Read"], deny: [], commandHosts: ["devbox"] },
      { requestId: "req-2" },
    )
    await source.unshare("c1", "phone", "req-3")
    expect(gateway.conversation.unshare).toHaveBeenCalledWith("c1", "phone", {
      requestId: "req-3",
    })
  })

  it("names hosts on the wire only when the policy has some", () => {
    expect(wirePolicy({ kind: "preset", preset: "all", commandHosts: [] })).toEqual({
      preset: "all",
    })
    expect(
      wirePolicy({ kind: "rules", allow: [], deny: ["Write"], commandHosts: [] }),
    ).toEqual({ allow: [], deny: ["Write"] })
  })
})

describe("a refusal", () => {
  const rpc = (code: string) => new NessaRpcError(code, "refused")
  const mutation = (cause: unknown) =>
    new NessaConversationMutationError("c1", "r", undefined, cause, async () => undefined)

  it.each([
    [ConversationErrorCode.ShareTargetNotPaired, "not-paired"],
    [ConversationErrorCode.InvalidRequest, "invalid"],
    [ConversationErrorCode.ConversationDeleted, "deleted"],
    [ConversationErrorCode.ConversationNotFound, "not-found"],
    [ConversationErrorCode.TemporarilyUnavailable, "busy"],
    [ConversationErrorCode.ConversationCapacity, "busy"],
    [ConversationErrorCode.ConversationStorageUnavailable, "storage"],
    [ConversationErrorCode.ConversationStateUnreadable, "storage"],
    [ConversationErrorCode.AuditUnavailable, "audit"],
    [ConversationErrorCode.ConversationsNotConfigured, "not-configured"],
    [ConversationErrorCode.DrivePolicyUnenforceable, "unenforceable"],
    [ConversationErrorCode.TurnRunning, "other"],
  ] as const)("%s is %s, however it is carried", (code, kind) => {
    expect(failureOf(rpc(code))).toBe(kind)
    expect(failureOf(mutation(rpc(code)))).toBe(kind)
    expect(
      failureOf(new NessaConversationControlError("c1", "r", undefined, rpc(code))),
    ).toBe(kind)
  })

  it.each([
    ["forbidden", "refused"],
    ["unauthorized", "signed-out"],
  ] as const)("the session's %s is %s", (code, kind) => {
    expect(failureOf(rpc(code))).toBe(kind)
    expect(failureOf(mutation(rpc(code)))).toBe(kind)
  })

  it("is unanswered for a code this build does not know, or no answer at all", () => {
    expect(failureOf(rpc("something_new"))).toBe("unanswered")
    expect(failureOf(mutation(new Error("socket closed")))).toBe("unanswered")
    expect(failureOf(new Error("socket closed"))).toBe("unanswered")
    expect(failureOf(undefined)).toBe("unanswered")
    // An own-property table: an inherited name is no code.
    expect(failureOf(rpc("toString"))).toBe("unanswered")
  })

  it("comes back as a typed SharingError from every call", async () => {
    const gateway = client()
    gateway.conversation.share.mockRejectedValueOnce(
      mutation(rpc(ConversationErrorCode.ShareTargetNotPaired)),
    )
    gateway.conversation.unshare.mockRejectedValueOnce(rpc("forbidden"))
    const source = sharingGateway({ connected: async () => gateway })
    await expect(
      source.share("c1", { credentialId: "x", role: "read" }, "r"),
    ).rejects.toEqual(new SharingError("not-paired"))
    await expect(source.unshare("c1", "x", "r")).rejects.toEqual(
      new SharingError("refused"),
    )
  })
})
