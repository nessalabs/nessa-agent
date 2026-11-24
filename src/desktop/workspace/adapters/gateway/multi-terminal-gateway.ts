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

function offlineTerminalView(): TerminalView {
  return {
    subscribe: () => () => {},
    fit: () => {},
    focus: () => {},
    input: () => {},
    history: () => {},
    release: () => {},
  }
}

/**
 * Create a composite `TerminalSource` backed by all gateways in `backing`.
 */
export function multiTerminalGateway(
  backing: MultiGatewayTerminalBacking,
): TerminalSource {
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
            rawListsByGateway.delete(entry.gatewayId)
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
      const gateway = backing.entry(gatewayId)
      if (
        !gateway ||
        (gateway.gatewayId !== LOCAL_GATEWAY_ID &&
          (gateway.status === "auth-refused" ||
            gateway.status === "disconnected" ||
            gateway.status === "refused"))
      ) {
        return offlineTerminalView()
      }
      return gateway.terminals.view(rawId)
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
