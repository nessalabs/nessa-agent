import { describe, expect, it, vi } from "vitest"
import {
  LOCAL_GATEWAY_ID,
  qualifyEntityId,
  type GatewayConnectionState,
} from "../../model/gateway-id"
import {
  WorkspaceSourceError,
  type WorkspaceSource,
  type WorkspaceUpdate,
} from "../../application/ports"
import type { Transcript } from "../../model/transcript"
import type { SessionSummary, WorkspaceIndex } from "../../model/workspace-index"
import {
  multiGatewaySource,
  type ManagedWorkspaceGateway,
  type MultiGatewayWorkspaceBacking,
} from "./multi-gateway-source"

function fakeWorkspaceSource(
  name: string,
  options: { failIndex?: boolean } = {},
): {
  source: WorkspaceSource
  emit: (update: WorkspaceUpdate) => void
  sent: Array<{ sessionId: string; channelId?: string; text: string }>
} {
  const listeners = new Set<(u: WorkspaceUpdate) => void>()
  const sent: Array<{ sessionId: string; channelId?: string; text: string }> = []
  const session: SessionSummary = {
    id: "sess-1",
    channelId: "default",
    title: `${name} chat`,
    model: { provider: "anthropic", modelId: "claude-sonnet-4-5" },
    status: "idle",
    startedAt: 1,
    updatedAt: 2,
    preview: "hello",
    pinned: false,
    unread: false,
    revision: 1,
  }
  const indexData: WorkspaceIndex = {
    sections: [{ id: "gateway", name: "Nessa" }],
    channels: [
      {
        id: "default",
        name: "general",
        sectionId: "gateway",
        private: false,
        topic: "",
        coordinatorConversationId: "coord-1",
      },
    ],
    sessions: [session],
  }
  const transcriptData: Transcript = {
    sessionId: "sess-1",
    messages: [],
    revision: 1,
  } as unknown as Transcript

  const source: WorkspaceSource = {
    index: vi.fn(async () => {
      if (options.failIndex) throw new WorkspaceSourceError("unavailable")
      return indexData
    }),
    transcript: vi.fn(async (id: string) => ({ ...transcriptData, sessionId: id })),
    subscribe: (listener) => {
      listeners.add(listener)
      return () => {
        listeners.delete(listener)
      }
    },
    send: vi.fn(async (msg) => {
      sent.push({
        sessionId: msg.sessionId,
        channelId: msg.start?.channelId,
        text: msg.text,
      })
    }),
    approve: vi.fn(async () => {}),
    deny: vi.fn(async () => {}),
    answerQuestion: vi.fn(async () => {}),
    stop: vi.fn(async () => {}),
    setModel: vi.fn(async () => ({ applied: true }) as never),
    setAccess: vi.fn(async () => {}),
    setUsageLimitAutoContinue: vi.fn(async () => {}),
    setPinned: vi.fn(async () => {}),
    archive: vi.fn(async () => {}),
    rename: vi.fn(async () => {}),
    fork: vi.fn(async (id: string) => `${id}-fork`),
    delete: vi.fn(async () => {}),
    markRead: vi.fn(async () => {}),
    keepFollowing: vi.fn(),
  }

  return {
    source,
    emit: (u) => {
      for (const l of listeners) l(u)
    },
    sent,
  }
}

function makeBacking(initial: ManagedWorkspaceGateway[]): {
  backing: MultiGatewayWorkspaceBacking
  setEntries: (next: ManagedWorkspaceGateway[]) => void
} {
  let current = initial
  const listeners = new Set<() => void>()
  return {
    backing: {
      entries: () => current,
      entry: (id) => current.find((g) => g.gatewayId === id),
      onTopologyChange: (l) => {
        listeners.add(l)
        return () => listeners.delete(l)
      },
    },
    setEntries: (next) => {
      current = next
      for (const l of listeners) l()
    },
  }
}

describe("multiGatewaySource (ADR 1417 G1, G2, G6, G9, G10, G11)", () => {
  it("qualifies identical raw IDs across gateways without colliding (G10)", async () => {
    const local = fakeWorkspaceSource("Local")
    const cloud = fakeWorkspaceSource("Cloud")
    const { backing } = makeBacking([
      {
        gatewayId: LOCAL_GATEWAY_ID,
        label: "This Mac",
        status: "connected",
        workspace: local.source,
      },
      {
        gatewayId: "cloud-1",
        label: "Cloud Box",
        status: "connected",
        workspace: cloud.source,
      },
    ])
    const composite = multiGatewaySource(backing)
    const idx = await composite.index()

    expect(idx.sections.map((s) => s.name)).toEqual(["This Mac", "Cloud Box"])
    expect(idx.channels).toHaveLength(2)
    expect(idx.channels[0].id).toBe("default")
    expect(idx.channels[1].id).toBe(qualifyEntityId("cloud-1", "default"))
    expect(idx.sessions).toHaveLength(2)
    expect(idx.sessions[0].id).toBe("sess-1")
    expect(idx.sessions[1].id).toBe(qualifyEntityId("cloud-1", "sess-1"))
  })

  it("isolates gateway B failure while gateway A stays live (G1)", async () => {
    const local = fakeWorkspaceSource("Local")
    const cloud = fakeWorkspaceSource("Cloud", { failIndex: true })
    const { backing } = makeBacking([
      {
        gatewayId: LOCAL_GATEWAY_ID,
        label: "This Mac",
        status: "connected",
        workspace: local.source,
      },
      {
        gatewayId: "cloud-1",
        label: "Cloud Box",
        status: "connected",
        workspace: cloud.source,
      },
    ])
    const composite = multiGatewaySource(backing)
    const idx = await composite.index()

    expect(idx.sections.map((s) => s.name)).toEqual(["This Mac", "Cloud Box"])
    expect(idx.channels.map((c) => c.id)).toEqual(["default"])
    expect(idx.sessions.map((s) => s.id)).toEqual(["sess-1"])
  })

  it("never falls back to gateway A when sending to an offline or auth-refused gateway B (G2, G9)", async () => {
    const local = fakeWorkspaceSource("Local")
    const cloud = fakeWorkspaceSource("Cloud")
    const makeEntry = (status: GatewayConnectionState): ManagedWorkspaceGateway[] => [
      {
        gatewayId: LOCAL_GATEWAY_ID,
        label: "This Mac",
        status: "connected",
        workspace: local.source,
      },
      { gatewayId: "cloud-1", label: "Cloud Box", status, workspace: cloud.source },
    ]
    const { backing, setEntries } = makeBacking(makeEntry("disconnected"))
    const composite = multiGatewaySource(backing)
    const remoteSessionId = qualifyEntityId("cloud-1", "sess-1")

    await expect(
      composite.send({
        sessionId: remoteSessionId,
        initiator: "person",
        messageId: "m1",
        text: "hi",
        model: { provider: "anthropic", modelId: "claude-sonnet-4-5" },
      }),
    ).rejects.toMatchObject({ reason: "unavailable" })
    expect(local.sent).toHaveLength(0)

    setEntries(makeEntry("auth-refused"))
    await expect(composite.transcript(remoteSessionId)).rejects.toMatchObject({
      reason: "signed-out",
    })
    expect(local.sent).toHaveLength(0)
  })

  it("routes remote sends and forks to the owning gateway and emits resync on topology change (G6, G11)", async () => {
    const local = fakeWorkspaceSource("Local")
    const cloud = fakeWorkspaceSource("Cloud")
    const { backing, setEntries } = makeBacking([
      {
        gatewayId: LOCAL_GATEWAY_ID,
        label: "This Mac",
        status: "connected",
        workspace: local.source,
      },
      {
        gatewayId: "cloud-1",
        label: "Cloud Box",
        status: "connected",
        workspace: cloud.source,
      },
    ])
    const composite = multiGatewaySource(backing)
    const updates: WorkspaceUpdate[] = []
    const unsub = composite.subscribe((u) => updates.push(u))

    // New draft in remote channel routes to cloud-1 with unwrapped channelId.
    await composite.send({
      sessionId: "new-draft-uuid",
      initiator: "person",
      messageId: "m2",
      text: "run tests",
      model: { provider: "anthropic", modelId: "claude-sonnet-4-5" },
      start: { channelId: qualifyEntityId("cloud-1", "default"), title: "New" },
    })
    expect(cloud.sent).toEqual([
      { sessionId: "new-draft-uuid", channelId: "default", text: "run tests" },
    ])
    expect(local.sent).toHaveLength(0)

    const forked = await composite.fork(qualifyEntityId("cloud-1", "sess-1"), "person")
    expect(forked).toBe(qualifyEntityId("cloud-1", "sess-1-fork"))

    // Removing cloud-1 emits resync and stops forwarding its updates (G11).
    setEntries([
      {
        gatewayId: LOCAL_GATEWAY_ID,
        label: "This Mac",
        status: "connected",
        workspace: local.source,
      },
    ])
    expect(updates.some((u) => u.kind === "resync")).toBe(true)
    const countBefore = updates.length
    cloud.emit({ kind: "session-removed", sessionId: "sess-1", revision: 2 })
    expect(updates.length).toBe(countBefore)
    unsub()
  })

  it("re-subscribes when a re-paired gateway replaces its WorkspaceSource under the same gatewayId (B2, G7)", () => {
    const local = fakeWorkspaceSource("Local")
    const cloudV1 = fakeWorkspaceSource("Cloud v1")
    const cloudV2 = fakeWorkspaceSource("Cloud v2")
    const { backing, setEntries } = makeBacking([
      {
        gatewayId: LOCAL_GATEWAY_ID,
        label: "This Mac",
        status: "connected",
        workspace: local.source,
      },
      {
        gatewayId: "cloud-1",
        label: "Cloud Box",
        status: "connected",
        workspace: cloudV1.source,
      },
    ])
    const composite = multiGatewaySource(backing)
    const updates: WorkspaceUpdate[] = []
    const unsub = composite.subscribe((u) => updates.push(u))

    // Replace cloud-1's source (re-pair).
    setEntries([
      {
        gatewayId: LOCAL_GATEWAY_ID,
        label: "This Mac",
        status: "connected",
        workspace: local.source,
      },
      {
        gatewayId: "cloud-1",
        label: "Cloud Box",
        status: "connected",
        workspace: cloudV2.source,
      },
    ])
    const afterSwap = updates.length
    cloudV1.emit({ kind: "session-removed", sessionId: "old", revision: 1 })
    expect(updates.length).toBe(afterSwap)

    cloudV2.emit({ kind: "session-removed", sessionId: "fresh", revision: 2 })
    expect(updates[updates.length - 1]).toEqual({
      kind: "session-removed",
      sessionId: qualifyEntityId("cloud-1", "fresh"),
      revision: 2,
    })
    unsub()
  })

  it("preserves last known remote channels and sessions during transient reconnects, and drops them on removal", async () => {
    const local = fakeWorkspaceSource("Local")
    const cloud = fakeWorkspaceSource("Cloud")
    const { backing, setEntries } = makeBacking([
      {
        gatewayId: LOCAL_GATEWAY_ID,
        label: "This Mac",
        status: "connected",
        workspace: local.source,
      },
      {
        gatewayId: "cloud-1",
        label: "Cloud Box",
        status: "connected",
        workspace: cloud.source,
      },
    ])
    const composite = multiGatewaySource(backing)

    // First load populates cache for cloud-1.
    const initial = await composite.index()
    expect(initial.sessions.map((s) => s.id)).toEqual([
      "sess-1",
      qualifyEntityId("cloud-1", "sess-1"),
    ])

    // Transient reconnecting state preserves cloud-1's channels and sessions.
    setEntries([
      {
        gatewayId: LOCAL_GATEWAY_ID,
        label: "This Mac",
        status: "connected",
        workspace: local.source,
      },
      {
        gatewayId: "cloud-1",
        label: "Cloud Box",
        status: "reconnecting",
        workspace: cloud.source,
      },
    ])
    const duringReconnect = await composite.index()
    expect(duringReconnect.sessions.map((s) => s.id)).toEqual([
      "sess-1",
      qualifyEntityId("cloud-1", "sess-1"),
    ])

    // Explicit removal evicts cloud-1 from the index.
    setEntries([
      {
        gatewayId: LOCAL_GATEWAY_ID,
        label: "This Mac",
        status: "connected",
        workspace: local.source,
      },
    ])
    const afterRemove = await composite.index()
    expect(afterRemove.sessions.map((s) => s.id)).toEqual(["sess-1"])
  })
})
