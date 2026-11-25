/**
 * Composite `TerminalSource` across 1..N gateways (ADR 1417, #1419).
 *
 * Qualifies terminal, channel, and conversation IDs by `(gatewayId, rawId)`
 * (G10), isolates failures across gateways (G1), routes `open`/`view`/`input`/
 * `close`/`rename` strictly to the owning gateway without fallback (G2, G9),
 * and removes terminals cleanly when a gateway is removed (G11).
 */
import {
  LOCAL_GATEWAY_ID,
  parseQualifiedId,
  qualifyEntityId,
  type GatewayConnectionState,
} from "../../model/gateway-id"
import type { TerminalEntry } from "../../model/terminal"
import {
  TerminalOpenError,
  type TerminalFit,
  type TerminalOpenRequest,
  type TerminalProtocolMessage,
  type TerminalSource,
  type TerminalView,
} from "../../application/ports"

export interface ManagedTerminalGateway {
  readonly gatewayId: string
  readonly label: string
  readonly status: GatewayConnectionState
  readonly terminals: TerminalSource
}

export interface MultiGatewayTerminalBacking {
  entries(): readonly ManagedTerminalGateway[]
  entry(gatewayId: string): ManagedTerminalGateway | undefined
  onTopologyChange(listener: () => void): () => void
}

function qualifyTerminal(
  gateway: ManagedTerminalGateway,
  entry: TerminalEntry,
): TerminalEntry {
  if (gateway.gatewayId === LOCAL_GATEWAY_ID) {
    return {
      ...entry,
      gatewayId: LOCAL_GATEWAY_ID,
      gatewayLabel: gateway.label,
    }
  }
  return {
    ...entry,
    id: qualifyEntityId(gateway.gatewayId, entry.id),
    channelId: qualifyEntityId(gateway.gatewayId, entry.channelId),
    conversationId:
      entry.conversationId !== undefined
        ? qualifyEntityId(gateway.gatewayId, entry.conversationId)
        : undefined,
    gatewayId: gateway.gatewayId,
    gatewayLabel: gateway.label,
    runsOn: entry.runsOn === "here" ? gateway.label : entry.runsOn,
  }
}

/** Hold the pane's view across changes to its owning gateway's connection. */
function reconnectingTerminalView(
  backing: MultiGatewayTerminalBacking,
  gatewayId: string,
  rawId: string,
  focusedViews: Map<string, symbol>,
): TerminalView {
  const listeners = new Set<(message: TerminalProtocolMessage) => void>()
  const terminalId = qualifyEntityId(gatewayId, rawId)
  const viewOwner = Symbol()
  const focused = () => focusedViews.get(terminalId) === viewOwner
  interface Binding {
    readonly source: TerminalSource
    readonly view: TerminalView
    stopOutput?: () => void
  }
  let binding: Binding | undefined
  let fit: TerminalFit | undefined
  let pending: "fit" | "focus" | undefined
  let released = false

  const availableSource = () => {
    const gateway = backing.entry(gatewayId)
    return gateway?.status === "connected" ? gateway.terminals : undefined
  }
  const activeView = () =>
    binding?.source === availableSource() ? binding?.view : undefined

  const unbind = () => {
    const old = binding
    binding = undefined
    old?.stopOutput?.()
    old?.view.release()
  }

  const bindOutput = (current: Binding) => {
    const stop = current.view.subscribe((message) => {
      if (binding !== current || !activeView()) return
      for (const listener of listeners) listener(message)
    })
    if (binding === current) current.stopOutput = stop
    else stop()
  }

  const sync = () => {
    if (released) return
    const source = availableSource()
    if (binding?.source !== source) {
      unbind()
      if (source) {
        binding = { source, view: source.view(rawId) }
        pending = focused() ? "focus" : "fit"
        if (listeners.size > 0) bindOutput(binding)
      }
    }
    if (!binding || !fit || !pending) return
    const needsFocus = pending === "focus" && focused()
    pending = undefined
    if (needsFocus) binding.view.focus(fit)
    else binding.view.fit(fit)
  }

  const stopTopology = backing.onTopologyChange(sync)
  sync()
  return {
    subscribe(listener) {
      if (released) return () => {}
      listeners.add(listener)
      if (binding && !binding.stopOutput) bindOutput(binding)
      return () => listeners.delete(listener)
    },
    fit(next) {
      if (released) return
      fit = next
      pending ??= "fit"
      if (activeView()) sync()
    },
    focus(next) {
      if (released) return
      fit = next
      focusedViews.set(terminalId, viewOwner)
      pending = "focus"
      if (activeView()) sync()
    },
    input(bytes) {
      activeView()?.input(bytes)
    },
    history(start, count) {
      activeView()?.history(start, count)
    },
    release() {
      if (released) return
      released = true
      stopTopology()
      listeners.clear()
      if (focused()) focusedViews.delete(terminalId)
      unbind()
    },
  }
}

/**
 * Create a composite `TerminalSource` backed by all gateways in `backing`.
 */
export function multiTerminalGateway(
  backing: MultiGatewayTerminalBacking,
): TerminalSource {
  const focusedViews = new Map<string, symbol>()

  function resolveOwner(qualifiedId: string): {
    gateway: ManagedTerminalGateway
    rawId: string
  } {
    const { gatewayId, rawId } = parseQualifiedId(qualifiedId)
    const gateway = backing.entry(gatewayId)
    if (!gateway) {
      throw new TerminalOpenError("unavailable")
    }
    if (
      gateway.gatewayId !== LOCAL_GATEWAY_ID &&
      (gateway.status === "auth-refused" ||
        gateway.status === "disconnected" ||
        gateway.status === "refused")
    ) {
      throw new TerminalOpenError("unavailable")
    }
    return { gateway, rawId }
  }

  return {
    follow(onList: (terminals: readonly TerminalEntry[]) => void): () => void {
      const rawListsByGateway = new Map<string, readonly TerminalEntry[]>()
      const childUnsubs = new Map<
        string,
        { readonly terminals: TerminalSource; readonly unsub: () => void }
      >()
      let disposed = false

      const emit = () => {
        if (disposed) return
        const merged: TerminalEntry[] = []
        for (const entry of backing.entries()) {
          const raw = rawListsByGateway.get(entry.gatewayId)
          if (raw) {
            for (const terminal of raw) {
              merged.push(qualifyTerminal(entry, terminal))
            }
          }
        }
        onList(merged)
      }

      const sync = () => {
        if (disposed) return
        const current = backing.entries()
        const activeIds = new Set(current.map((entry) => entry.gatewayId))

        let changed = false
        for (const [gatewayId, held] of childUnsubs) {
          if (!activeIds.has(gatewayId)) {
            held.unsub()
            childUnsubs.delete(gatewayId)
            rawListsByGateway.delete(gatewayId)
            changed = true
          }
        }
        for (const entry of current) {
          const existing = childUnsubs.get(entry.gatewayId)
          if (existing && existing.terminals !== entry.terminals) {
            existing.unsub()
            childUnsubs.delete(entry.gatewayId)
            // The replacement confirms which terminals it still holds on its first list.
            changed = true
          }
          if (!childUnsubs.has(entry.gatewayId)) {
            const unsub = entry.terminals.follow((list) => {
              if (disposed) return
              rawListsByGateway.set(entry.gatewayId, list)
              emit()
            })
            childUnsubs.set(entry.gatewayId, { terminals: entry.terminals, unsub })
          }
        }
        if (changed || rawListsByGateway.size > 0) emit()
      }

      sync()
      const unsubTopology = backing.onTopologyChange(sync)

      return () => {
        disposed = true
        unsubTopology()
        for (const held of childUnsubs.values()) {
          held.unsub()
        }
        childUnsubs.clear()
        rawListsByGateway.clear()
      }
    },

    async open(
      request: TerminalOpenRequest,
      requestId: string,
      fit: TerminalFit,
    ): Promise<TerminalEntry> {
      if (request.kind === "run") {
        const { gateway, rawId } = resolveOwner(request.conversationId)
        const opened = await gateway.terminals.open(
          { kind: "run", conversationId: rawId, command: request.command },
          requestId,
          fit,
        )
        return qualifyTerminal(gateway, opened)
      }
      if (request.channelId !== undefined) {
        const { gateway, rawId } = resolveOwner(request.channelId)
        const opened = await gateway.terminals.open(
          { kind: "shell", channelId: rawId },
          requestId,
          fit,
        )
        return qualifyTerminal(gateway, opened)
      }
      const local = backing.entry(LOCAL_GATEWAY_ID)
      if (!local) throw new TerminalOpenError("unavailable")
      const opened = await local.terminals.open({ kind: "shell" }, requestId, fit)
      return qualifyTerminal(local, opened)
    },

    view(terminalId: string): TerminalView {
      const { gatewayId, rawId } = parseQualifiedId(terminalId)
      if (gatewayId === LOCAL_GATEWAY_ID) {
        const local = backing.entry(gatewayId)
        if (local) return local.terminals.view(rawId)
      }
      return reconnectingTerminalView(backing, gatewayId, rawId, focusedViews)
    },

    input(terminalId: string, bytes: Uint8Array): void {
      const { gatewayId, rawId } = parseQualifiedId(terminalId)
      const gateway = backing.entry(gatewayId)
      if (
        !gateway ||
        (gateway.gatewayId !== LOCAL_GATEWAY_ID &&
          (gateway.status === "auth-refused" ||
            gateway.status === "disconnected" ||
            gateway.status === "refused"))
      ) {
        return
      }
      gateway.terminals.input(rawId, bytes)
    },

    async close(terminalId: string): Promise<void> {
      const { gateway, rawId } = resolveOwner(terminalId)
      await gateway.terminals.close(rawId)
    },

    async rename(terminalId: string, title: string): Promise<void> {
      const { gateway, rawId } = resolveOwner(terminalId)
      await gateway.terminals.rename(rawId, title)
    },
  }
}
