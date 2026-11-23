import { describe, expect, it, vi } from "vitest"
import { LOCAL_GATEWAY_ID, qualifyEntityId } from "../../model/gateway-id"
import type { TerminalEntry } from "../../model/terminal"
import type { TerminalSource } from "../../application/ports"
import {
  multiTerminalGateway,
  type ManagedTerminalGateway,
  type MultiGatewayTerminalBacking,
} from "./multi-terminal-gateway"

const RAW_TERM_ID = "t_0123456789abcdef0123456789abcdef"

function fakeTerminalSource(): {
  source: TerminalSource
  pushList: (entries: readonly TerminalEntry[]) => void
  opened: Array<{ kind: string; channelId?: string; conversationId?: string }>
  closed: string[]
} {
  const listeners = new Set<(list: readonly TerminalEntry[]) => void>()
  const opened: Array<{ kind: string; channelId?: string; conversationId?: string }> = []
  const closed: string[] = []
  const source: TerminalSource = {
    follow: (onList) => {
      listeners.add(onList)
      return () => {
        listeners.delete(onList)
      }
    },
    open: vi.fn(async (req, requestId) => {
      opened.push(
        req.kind === "shell"
          ? { kind: "shell", channelId: req.channelId }
          : { kind: "run", conversationId: req.conversationId },
      )
      return {
        id: RAW_TERM_ID,
        title: "bash",
        kind: req.kind,
        channelId: req.kind === "shell" ? (req.channelId ?? "default") : "default",
        conversationId: req.kind === "run" ? req.conversationId : undefined,
        requestId,
        folder: "/workspace",
        shell: "/bin/bash",
        runsOn: "here",
        startedAtMs: 100,
        state: "running" as const,
      }
    }),
    view: vi.fn(() => ({
      subscribe: () => () => {},
      fit: () => {},
      focus: () => {},
      input: () => {},
      history: () => {},
      release: () => {},
    })),
    input: vi.fn(),
    close: vi.fn(async (id) => {
      closed.push(id)
    }),
    rename: vi.fn(async () => {}),
  }
  return {
    source,
    pushList: (entries) => {
      for (const l of listeners) l(entries)
    },
    opened,
    closed,
  }
}

describe("multiTerminalGateway (ADR 1417 G1, G2, G10, G11)", () => {
  it("qualifies identical terminal IDs across two gateways and drops removed gateway terminals cleanly", async () => {
    const local = fakeTerminalSource()
    const cloud = fakeTerminalSource()
    let entries: ManagedTerminalGateway[] = [
      {
        gatewayId: LOCAL_GATEWAY_ID,
        label: "This Mac",
        status: "connected",
        terminals: local.source,
      },
      {
        gatewayId: "cloud-1",
        label: "Cloud Box",
        status: "connected",
        terminals: cloud.source,
      },
    ]
    const topo = new Set<() => void>()
    const backing: MultiGatewayTerminalBacking = {
      entries: () => entries,
      entry: (id) => entries.find((e) => e.gatewayId === id),
      onTopologyChange: (l) => {
        topo.add(l)
        return () => topo.delete(l)
      },
    }

    const multi = multiTerminalGateway(backing)
    let latest: readonly TerminalEntry[] = []
    const unsub = multi.follow((list) => {
      latest = list
    })

    const rawEntry: TerminalEntry = {
      id: RAW_TERM_ID,
      title: "zsh",
      kind: "shell",
      channelId: "default",
      requestId: "r1",
      folder: "/work",
      shell: "/bin/zsh",
      runsOn: "here",
      startedAtMs: 1,
      state: "running",
    }
    local.pushList([rawEntry])
    cloud.pushList([rawEntry])

    expect(latest).toHaveLength(2)
    expect(latest[0].id).toBe(RAW_TERM_ID)
    expect(latest[1].id).toBe(qualifyEntityId("cloud-1", RAW_TERM_ID))
    expect(latest[1].runsOn).toBe("Cloud Box")

    // Open shell in remote channel routes only to cloud-1 (G2).
    const opened = await multi.open(
      { kind: "shell", channelId: qualifyEntityId("cloud-1", "default") },
      "req-2",
      { cols: 80, rows: 24 },
    )
    expect(opened.id).toBe(qualifyEntityId("cloud-1", RAW_TERM_ID))
    expect(cloud.opened).toEqual([{ kind: "shell", channelId: "default" }])
    expect(local.opened).toHaveLength(0)

    // Remove cloud-1 -> its terminals disappear, local stays intact (G11).
    entries = [entries[0]]
    for (const l of topo) l()
    expect(latest).toHaveLength(1)
    expect(latest[0].id).toBe(RAW_TERM_ID)

    // Closing a terminal on the removed gateway fails closed (never falls back to local).
    await expect(
      multi.close(qualifyEntityId("cloud-1", RAW_TERM_ID)),
    ).rejects.toMatchObject({
      reason: "unavailable",
    })
    expect(local.closed).toHaveLength(0)
    unsub()
  })
})
