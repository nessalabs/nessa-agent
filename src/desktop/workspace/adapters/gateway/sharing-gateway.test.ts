/**
 * Sharing over the gateway client: what is read and how, what is sent, and
 * each refusal as the kind the dialog says in words.
 */
import {
  ConversationErrorCode,
  NessaConversationControlError,
  NessaConversationMutationError,
  NessaRpcError,
  sharePage,
  toolPolicyBounds,
  type AgentsListResult,
  type ConversationCapability,
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
  changeRevision: 1,
  items: [
    { credentialId: "phone", capabilities: ["read"], policyRevision: 1, grantedAtMs: 5 },
    {
      credentialId: "tablet",
      capabilities: ["read", "comment", "drive"],
      policy: { allow: ["Read"], deny: [] },
      policyRevision: 3,
      grantedAtMs: 6,
    },
    {
      credentialId: "laptop",
      capabilities: ["drive", "read", "comment"],
      policy: { preset: "no_shell", commandHosts: ["devbox"] },
      policyRevision: 1,
      grantedAtMs: 7,
    },
  ],
}

function client(
  partial: {
    shares?: SharingClient["conversation"]["shares"]
    list?: SharingClient["credentials"]["list"]
  } = {},
) {
  const fake = {
    conversation: {
      shares: vi.fn(partial.shares ?? (async () => shares)),
      share: vi.fn(async () => ({ requestId: "r", applied: true, policyRevision: 9 })),
      unshare: vi.fn(async () => ({ requestId: "r", applied: true })),
    },
    credentials: {
      list: vi.fn(
        partial.list ??
          (async () => ({
            credentials: [
              credential("session-credential", ["conversation.read"]),
              credential("phone", ["conversation.read"]),
              credential("revoked", ["conversation.read"], 1_700_000_100),
              credential("admin", ["conversation.read", "conversation.write"]),
              credential("owner-phone", [
                "conversation.read",
                "conversation.write",
                "server.read",
              ]),
              credential("tablet", ["conversation.read"]),
            ],
          })),
      ),
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
    const source = sharingGateway({ connected: async () => gateway })
    const grants = await source.grants("c1")
    const read = await source.choices()
    expect(gateway.conversation.shares).toHaveBeenCalledWith("c1", {
      pageSize: sharePage.pageSize,
    })
    expect(grants).toEqual([
      { credentialId: "phone", role: "read", policyRevision: 1, grantedAtMs: 5 },
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
    // Not the session's own, not revoked, not one holding more than reading,
    // and not the owner's phone, which reads every chat already.
    expect(read.targets.map((target) => target.credentialId)).toEqual(["phone", "tablet"])
    expect(read.commandHosts).toEqual(["devbox"])
    expect(read.limits).toBe(toolPolicyBounds)
    expect(read.limits).toEqual({
      patterns: 16,
      patternBytes: 48,
      hosts: 16,
      hostBytes: 253,
    })
  })

  it("D12: reads every page of grants and stops when the list finishes", async () => {
    const first = shares.items[0]
    const rest = shares.items.slice(1)
    const pages = vi.fn(
      async (_id: string, options: { cursor?: { credentialId: string } }) => {
        if (options.cursor === undefined)
          return {
            changeRevision: 1,
            items: [first],
            cursor: {
              grantedAtMs: first.grantedAtMs,
              credentialId: first.credentialId,
            },
          }
        return { changeRevision: 1, items: rest }
      },
    )
    const gateway = client({ shares: pages })
    const source = sharingGateway({ connected: async () => gateway })
    await expect(source.grants("c1")).resolves.toHaveLength(3)
    expect(pages).toHaveBeenCalledTimes(2)
    expect(pages).toHaveBeenLastCalledWith("c1", {
      pageSize: sharePage.pageSize,
      cursor: { grantedAtMs: first.grantedAtMs, credentialId: first.credentialId },
    })
  })

  it("D12: is unanswered when a later page repeats a grant or passes the grant cap", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
    try {
      const repeated = shares.items[0]
      const source = sharingGateway({
        connected: async () =>
          client({
            shares: async () => ({
              changeRevision: 1,
              items: [repeated],
              cursor: {
                grantedAtMs: repeated.grantedAtMs,
                credentialId: repeated.credentialId,
              },
            }),
          }),
      })
      await expect(source.grants("c1")).rejects.toEqual(new SharingError("unanswered"))
      let n = 0
      const endless = sharingGateway({
        connected: async () =>
          client({
            shares: async () => {
              n += 1
              const credentialId = `c${n}`
              return {
                changeRevision: 1,
                items: [
                  {
                    credentialId,
                    capabilities: ["read"],
                    policyRevision: 1,
                    grantedAtMs: n,
                  },
                ],
                cursor: { credentialId, grantedAtMs: n },
              }
            },
          }),
      })
      await expect(endless.grants("c1")).rejects.toEqual(new SharingError("unanswered"))
      expect(n).toBe(sharePage.grants + 1)
    } finally {
      warn.mockRestore()
    }
  })

  it("D12: starts again when a later page names another catalogue revision", async () => {
    const dropped = shares.items[0]
    const kept = {
      credentialId: "watch",
      capabilities: ["read"] as ConversationCapability[],
      policyRevision: 4,
      grantedAtMs: 8,
    }
    let calls = 0
    const pages = vi.fn(async (_id: string, options: { cursor?: unknown }) => {
      calls += 1
      if (calls === 1)
        return {
          changeRevision: 1,
          items: [dropped],
          cursor: {
            grantedAtMs: dropped.grantedAtMs,
            credentialId: dropped.credentialId,
          },
        }
      if (calls === 2) return { changeRevision: 2, items: [kept] }
      expect(options.cursor).toBeUndefined()
      return { changeRevision: 2, items: [kept] }
    })
    const source = sharingGateway({
      connected: async () => client({ shares: pages }),
    })
    await expect(source.grants("c1")).resolves.toEqual([
      { credentialId: "watch", role: "read", policyRevision: 4, grantedAtMs: 8 },
    ])
    expect(calls).toBe(3)
  })

  it("D12: is unanswered when the catalogue revision keeps moving", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
    try {
      let n = 0
      const source = sharingGateway({
        connected: async () =>
          client({
            shares: async () => {
              n += 1
              const credentialId = `c${n}`
              const page = {
                changeRevision: n,
                items: [
                  {
                    credentialId,
                    capabilities: ["read"] as ConversationCapability[],
                    policyRevision: 1,
                    grantedAtMs: n,
                  },
                ],
              }
              return n % 2 === 1
                ? { ...page, cursor: { credentialId, grantedAtMs: n } }
                : page
            },
          }),
      })
      await expect(source.grants("c1")).rejects.toEqual(new SharingError("unanswered"))
      // Two restarts are kept. The third drift — a page, a drift, twice, then
      // one more page and its drift — is unanswered.
      expect(n).toBe(6)
    } finally {
      warn.mockRestore()
    }
  })

  it("is refused as a typed kind when a part is, and the other part still reads", async () => {
    const gateway = client({
      shares: async () => {
        throw new NessaRpcError("conversation_not_found", "no")
      },
      list: async () => {
        throw new NessaRpcError("forbidden", "no")
      },
    })
    const source = sharingGateway({ connected: async () => gateway })
    await expect(source.grants("c1")).rejects.toEqual(new SharingError("not-found"))
    await expect(source.choices()).rejects.toEqual(new SharingError("refused"))
    const grantsOnly = sharingGateway({
      connected: async () =>
        client({
          list: async () => {
            throw new NessaRpcError("forbidden", "no")
          },
        }),
    })
    await expect(grantsOnly.grants("c1")).resolves.toHaveLength(3)
  })

  it("is unanswered, and logged, when no connection could be made or an answer failed the client's validation", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
    try {
      const offline = new Error("offline")
      const source = sharingGateway({ connected: () => Promise.reject(offline) })
      await expect(source.grants("c1")).rejects.toEqual(new SharingError("unanswered"))
      expect(warn).toHaveBeenLastCalledWith(
        "Sharing got no answer it could read",
        offline,
      )
      // The client refuses an answer it cannot validate as a mutation error
      // with no code; the dialog says only that no answer came.
      const invalid = new NessaConversationMutationError(
        "c1",
        "r",
        undefined,
        new Error("Invalid conversation policyRevision"),
        async () => undefined,
      )
      const gateway = client()
      gateway.conversation.share.mockRejectedValueOnce(invalid)
      await expect(
        sharingGateway({ connected: async () => gateway }).share(
          "c1",
          { credentialId: "phone", role: "read", expectedPolicyRevision: 0 },
          "r",
        ),
      ).rejects.toEqual(new SharingError("unanswered"))
      expect(warn).toHaveBeenLastCalledWith(
        "Sharing got no answer it could read",
        invalid,
      )
      // A refusal the dialog has words for is not logged.
      warn.mockClear()
      gateway.conversation.unshare.mockRejectedValueOnce(rpc("forbidden"))
      await expect(
        sharingGateway({ connected: async () => gateway }).unshare("c1", "x", 1, "r"),
      ).rejects.toEqual(new SharingError("refused"))
      expect(warn).not.toHaveBeenCalled()
    } finally {
      warn.mockRestore()
    }
  })

  it("lets a fault in reading an answer through as one, not as the gateway's refusal", async () => {
    // An answer this build cannot read: no credentials list in it.
    const gateway = {
      ...client(),
      credentials: { list: async () => ({}) },
    } as unknown as SharingClient
    const failure = await sharingGateway({ connected: async () => gateway })
      .choices()
      .catch((error: unknown) => error)
    expect(failure).toBeInstanceOf(TypeError)
  })
})

describe("sharing and revoking", () => {
  it("sends the capability set, the policy as the wire takes it, the revision it was built on and the request id, and answers the grant's revision", async () => {
    const gateway = client()
    const source = sharingGateway({ connected: async () => gateway })
    expect(
      await source.share(
        "c1",
        { credentialId: "phone", role: "read", expectedPolicyRevision: 0 },
        "req-1",
      ),
    ).toBe(9)
    expect(gateway.conversation.share).toHaveBeenLastCalledWith(
      "c1",
      "phone",
      ["read"],
      undefined,
      // 0: a first grant, built on a list where the device held none.
      { requestId: "req-1", expectedPolicyRevision: 0 },
    )
    await source.share(
      "c1",
      {
        credentialId: "phone",
        role: "drive",
        policy: { kind: "rules", allow: ["Read"], deny: [], commandHosts: ["devbox"] },
        expectedPolicyRevision: 4,
      },
      "req-2",
    )
    expect(gateway.conversation.share).toHaveBeenLastCalledWith(
      "c1",
      "phone",
      ["read", "comment", "drive"],
      { allow: ["Read"], deny: [], commandHosts: ["devbox"] },
      { requestId: "req-2", expectedPolicyRevision: 4 },
    )
    await source.unshare("c1", "phone", 9, "req-3")
    expect(gateway.conversation.unshare).toHaveBeenCalledWith("c1", "phone", {
      requestId: "req-3",
      expectedPolicyRevision: 9,
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

const rpc = (code: string) => new NessaRpcError(code, "refused")

describe("a refusal", () => {
  const mutation = (cause: unknown) =>
    new NessaConversationMutationError("c1", "r", undefined, cause, async () => undefined)

  it.each([
    [ConversationErrorCode.ShareTargetNotPaired, "not-paired"],
    [ConversationErrorCode.ShareGrantChanged, "changed"],
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
      source.share(
        "c1",
        { credentialId: "x", role: "read", expectedPolicyRevision: 0 },
        "r",
      ),
    ).rejects.toEqual(new SharingError("not-paired"))
    await expect(source.unshare("c1", "x", 1, "r")).rejects.toEqual(
      new SharingError("refused"),
    )
  })
})
