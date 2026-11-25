import { describe, expect, it, vi } from "vitest"
import { LOCAL_GATEWAY_ID, qualifyEntityId } from "../../model/gateway-id"
import type { TerminalEntry } from "../../model/terminal"
import type { TerminalProtocolMessage, TerminalSource } from "../../application/ports"
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
  view: ReturnType<typeof fakeTerminalView>
} {
  const listeners = new Set<(list: readonly TerminalEntry[]) => void>()
  const opened: Array<{ kind: string; channelId?: string; conversationId?: string }> = []
  const closed: string[] = []
  const view = fakeTerminalView()
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
    view: vi.fn(() => view),
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
    view,
  }
}

function fakeTerminalView(initial?: TerminalProtocolMessage) {
  const listeners = new Set<(message: TerminalProtocolMessage) => void>()
  return {
    subscribe(listener: (message: TerminalProtocolMessage) => void) {
      listeners.add(listener)
      if (initial) listener(initial)
      return () => listeners.delete(listener)
    },
    emit(message: TerminalProtocolMessage) {
      for (const listener of listeners) listener(message)
    },
    fit: vi.fn(),
    focus: vi.fn(),
    input: vi.fn(),
    history: vi.fn(),
    release: vi.fn(),
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

  it("attaches a mounted offline pane to its owner on recovery and outlives a source replacement", () => {
    const local = fakeTerminalSource()
    const cloud = fakeTerminalSource()
    const replacement = fakeTerminalSource()
    const localEntry: ManagedTerminalGateway = {
      gatewayId: LOCAL_GATEWAY_ID,
      label: "This Mac",
      status: "connected",
      terminals: local.source,
    }
    let remote: ManagedTerminalGateway = {
      gatewayId: "cloud-1",
      label: "Cloud Box",
      status: "disconnected",
      terminals: cloud.source,
    }
    const topology = new Set<() => void>()
    const backing: MultiGatewayTerminalBacking = {
      entries: () => [localEntry, remote],
      entry: (id) => [localEntry, remote].find((entry) => entry.gatewayId === id),
      onTopologyChange: (listener) => {
        topology.add(listener)
        return () => topology.delete(listener)
      },
    }
    const terminals = multiTerminalGateway(backing)
    const terminalId = qualifyEntityId("cloud-1", RAW_TERM_ID)
    const abandoned = terminals.view(terminalId)
    abandoned.fit({ cols: 60, rows: 20 })
    abandoned.release()
    const mounted = terminals.view(terminalId)
    const output = vi.fn()
    mounted.subscribe(output)
    mounted.focus({ cols: 80, rows: 24 })
    const latest = { cols: 100, rows: 30 }
    mounted.fit(latest)
    mounted.input(Uint8Array.of(13))
    expect(cloud.source.view).not.toHaveBeenCalled()

    remote = { ...remote, status: "connected" }
    for (const listener of topology) listener()
    expect(cloud.source.view).toHaveBeenCalledExactlyOnceWith(RAW_TERM_ID)
    expect(cloud.view.focus).toHaveBeenCalledExactlyOnceWith(latest)
    expect(cloud.view.input).not.toHaveBeenCalled()
    mounted.input(Uint8Array.of(65))
    mounted.history(5, 10)
    expect(cloud.view.input).toHaveBeenCalledExactlyOnceWith(Uint8Array.of(65))
    expect(cloud.view.history).toHaveBeenCalledExactlyOnceWith(5, 10)
    cloud.view.emit({ type: "checkpoint" })
    expect(output).toHaveBeenCalledExactlyOnceWith({ type: "checkpoint" })

    const otherView = fakeTerminalView({ type: "first" })
    vi.spyOn(cloud.source, "view").mockReturnValueOnce(otherView)
    const otherPane = terminals.view(terminalId)
    const otherOutput = vi.fn()
    otherPane.subscribe(otherOutput)
    expect(otherOutput).toHaveBeenCalledExactlyOnceWith({ type: "first" })
    const otherFit = { cols: 50, rows: 10 }
    otherPane.focus(otherFit)
    mounted.focus(latest)

    const otherReplacement = fakeTerminalView()
    vi.spyOn(replacement.source, "view")
      .mockReturnValueOnce(replacement.view)
      .mockReturnValueOnce(otherReplacement)
    remote = { ...remote, status: "reconnecting", terminals: replacement.source }
    for (const listener of topology) listener()
    remote = { ...remote, status: "connected" }
    for (const listener of topology) listener()
    expect(cloud.view.release).toHaveBeenCalledTimes(1)
    expect(replacement.view.focus).toHaveBeenCalledExactlyOnceWith(latest)
    expect(otherReplacement.focus).not.toHaveBeenCalled()
    expect(otherReplacement.fit).toHaveBeenCalledExactlyOnceWith(otherFit)
    cloud.view.emit({ type: "stale" })
    replacement.view.emit({ type: "frame" })
    expect(output.mock.calls).toEqual([[{ type: "checkpoint" }], [{ type: "frame" }]])
    expect(local.source.view).not.toHaveBeenCalled()

    mounted.release()
    mounted.release()
    otherPane.release()
    expect(replacement.view.release).toHaveBeenCalledTimes(1)
    expect(topology.size).toBe(0)
  })
})
