/**
 * Composite gateway identity and status model (ADR 1417, #1419).
 *
 * Every entity exposed to the Desktop workspace (section, channel, session,
 * terminal) belongs to one gateway. Local gateway IDs pass through unchanged
 * so single-gateway callers and existing layouts remain untouched; remote
 * gateway IDs are canonically qualified as:
 *
 * ```text
 *   gw:<encodeId(gatewayId)>:<encodeId(rawId)>
 * ```
 *
 * Because `encodeId` never emits `:` and is canonical (`model/id-encoding.ts`),
 * two gateways can each own `"default"`, `"gateway"`, or identical terminal IDs
 * without colliding in workspace state, split panes, or routing tables (G10).
 */
import { decodeId, encodeId } from "../../model/id-encoding"

/** Stable registry key for the primary local host gateway. */
export const LOCAL_GATEWAY_ID = "local"

const remotePrefix = "gw:"
const rawTerminalId = /^t_[0-9a-f]{32}$/

export interface QualifiedEntityId {
  readonly gatewayId: string
  readonly rawId: string
}

/**
 * Qualify a raw gateway entity ID with its owning `gatewayId`.
 * Local gateway IDs pass through unchanged; remote IDs use the canonical
 * `gw:<encodeId(gatewayId)>:<encodeId(rawId)>` form.
 */
export function qualifyEntityId(gatewayId: string, rawId: string): string {
  if (gatewayId === LOCAL_GATEWAY_ID) return rawId
  if (gatewayId === "") {
    throw new Error("gatewayId must not be empty")
  }
  return `${remotePrefix}${encodeId(gatewayId)}:${encodeId(rawId)}`
}

/**
 * Parse a workspace entity ID back into its owning `gatewayId` and `rawId`.
 * Non-prefixed IDs belong to `LOCAL_GATEWAY_ID`.
 *
 * Invariant: local gateway entity IDs (UUIDs, `t_<32-hex>`, `"gateway"`,
 * `"default"`, `"gateway-conversations"`) never carry the canonical
 * `gw:<encoded>:<encoded>` prefix reserved for remote gateways.
 */
export function parseQualifiedId(id: string): QualifiedEntityId {
  const remote = parseRemoteQualifiedId(id)
  if (remote !== null) return remote
  return { gatewayId: LOCAL_GATEWAY_ID, rawId: id }
}

/**
 * Parse a canonically encoded remote ID, or return `null` if `id` was not
 * produced by `qualifyEntityId` for a non-local gateway.
 */
export function parseRemoteQualifiedId(id: string): QualifiedEntityId | null {
  if (!id.startsWith(remotePrefix)) return null
  const parts = id.slice(remotePrefix.length).split(":")
  if (parts.length !== 2) return null
  const gatewayId = decodeId(parts[0])
  const rawId = decodeId(parts[1])
  if (
    gatewayId === null ||
    rawId === null ||
    gatewayId === "" ||
    gatewayId === LOCAL_GATEWAY_ID
  ) {
    return null
  }
  return { gatewayId, rawId }
}

/**
 * Whether `id` is a valid terminal ID for a pane key: either a local raw
 * `t_<32-hex>` ID or a canonically qualified remote terminal ID.
 */
export function isTerminalId(id: string): boolean {
  if (rawTerminalId.test(id)) return true
  const remote = parseRemoteQualifiedId(id)
  return remote !== null && rawTerminalId.test(remote.rawId)
}

/** The sidebar section ID for a gateway. */
export function gatewaySectionId(gatewayId: string): string {
  return qualifyEntityId(gatewayId, "gateway")
}

/** Lifecycle status of one registered gateway connection (ADR 1417 G1–G11). */
export type GatewayConnectionState =
  | "connecting"
  | "connected"
  | "reconnecting"
  | "disconnected"
  | "auth-refused"
  | "waking"
  | "refused"

/** Read-only snapshot of a registered gateway exposed to the workspace & UI. */
export interface GatewaySnapshot {
  readonly gatewayId: string
  readonly label: string
  readonly url: string
  readonly isLocal: boolean
  readonly status: GatewayConnectionState
  /** Plain-language detail when disconnected, auth-refused, or refused. */
  readonly detail?: string
  /** Server-reported gatewayId verified during handshake. */
  readonly serverGatewayId?: string
  readonly updatedAtMs: number
}

/**
 * Persisted non-secret metadata for a remote gateway (never holds credentials;
 * credentials live behind the `RemoteGatewayCredentialStore` seam).
 */
export interface PersistedRemoteGateway {
  readonly gatewayId: string
  readonly label: string
  readonly url: string
  readonly addedAtMs: number
}

/**
 * Upsert a remote gateway record by stable `gatewayId` (G7): re-pairing the
 * same gateway updates its label and URL in place without duplicating the entry.
 */
export function upsertRemoteGateway(
  entries: readonly PersistedRemoteGateway[],
  next: PersistedRemoteGateway,
): readonly PersistedRemoteGateway[] {
  const index = entries.findIndex((entry) => entry.gatewayId === next.gatewayId)
  if (index === -1) return [...entries, next]
  return entries.map((entry, at) => (at === index ? next : entry))
}

/** Remove a remote gateway record by `gatewayId` (G11). */
export function removeRemoteGateway(
  entries: readonly PersistedRemoteGateway[],
  gatewayId: string,
): readonly PersistedRemoteGateway[] {
  return entries.filter((entry) => entry.gatewayId !== gatewayId)
}

/**
 * Validate a remote gateway WebSocket URL, matching `@nessa/client`'s
 * `isLoopbackWebSocketUrl`:
 * - Rejects userinfo (`user:pass@host`).
 * - Allows `wss://` for any host.
 * - Allows `ws://` ONLY for numeric loopback (`127.0.0.1`, `[::1]`).
 */
export function normalizeGatewayUrl(input: string): string | null {
  const trimmed = input.trim()
  if (trimmed === "") return null
  let parsed: URL
  try {
    parsed = new URL(trimmed)
  } catch {
    return null
  }
  if (parsed.username !== "" || parsed.password !== "") return null
  if (parsed.protocol === "wss:") {
    return parsed.toString()
  }
  if (parsed.protocol === "ws:") {
    const host = parsed.hostname.toLowerCase()
    if (host === "127.0.0.1" || host === "::1" || host === "[::1]") {
      return parsed.toString()
    }
    return null
  }
  return null
}

const statusLabels: Readonly<Record<GatewayConnectionState, string>> = {
  connecting: "Connecting",
  connected: "Connected",
  reconnecting: "Reconnecting",
  disconnected: "Offline",
  "auth-refused": "Re-pair required",
  waking: "Waking",
  refused: "Unavailable",
}

/** Plain-language label for a gateway's connection status. */
export function gatewayStatusLabel(status: GatewayConnectionState): string {
  return statusLabels[status]
}
