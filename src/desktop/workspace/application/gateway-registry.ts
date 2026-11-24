/**
 * Application-layer port for the Desktop's multi-gateway registry (ADR 1417, #1419).
 *
 * The registry manages the local host gateway alongside any number of remote
 * `nessa-server` gateways connected over `wss://` (or loopback `ws://`).
 * Implementations live in `adapters/gateway/gateway-registry-adapter.ts` so the
 * domain and application layers stay free of `@nessa/client` and transport details.
 */
import type { CloudBoxMetadata, GatewaySnapshot } from "../model/gateway-id"

export type GatewayRegistryFailure =
  | "invalid-url"
  | "invalid-label"
  | "invalid-credential"
  | "connect-failed"
  | "auth-refused"
  | "not-found"
  | "cannot-remove-local"
  | "offline"

export class GatewayRegistryError extends Error {
  constructor(
    readonly reason: GatewayRegistryFailure,
    message?: string,
  ) {
    super(message ?? `Gateway registry error: ${reason}`)
    this.name = "GatewayRegistryError"
  }
}

export interface AddGatewayInput {
  readonly label: string
  readonly url: string
  readonly credential: string
  readonly expectedGatewayId?: string
  readonly cloudBox?: CloudBoxMetadata
}

export interface CreateCloudBoxInput {
  readonly name: string
  readonly cloud: "boat" | "agent37"
}

/**
 * Multi-gateway connection registry exposed to Settings and the workspace store.
 */
export interface GatewayRegistryPort {
  /** Current snapshots of all registered gateways (local first, then remotes). */
  list(): readonly GatewaySnapshot[]
  /** Subscribe to status, addition, rename, or removal changes. */
  subscribe(listener: (gateways: readonly GatewaySnapshot[]) => void): () => void
  /**
   * Verify and register a remote gateway (G7). Connects and checks the
   * handshake's `gatewayId` before persisting; re-pairing an existing
   * `gatewayId` updates its endpoint and credential without duplicating it.
   */
  add(input: AddGatewayInput): Promise<GatewaySnapshot>
  /** Update the display label of a registered gateway. */
  rename(gatewayId: string, label: string): void
  /**
   * Close and forget a remote gateway (G11). Refuses to remove the local host
   * gateway (`cannot-remove-local`).
   */
  remove(gatewayId: string): Promise<void>
  /** Trigger an immediate reconnect attempt for a gateway. */
  reconnect(gatewayId: string): void
  /** Update a remote gateway's URL (e.g. after tunnel rotation, G8) and reconnect. */
  updateEndpoint?(gatewayId: string, url: string): void
  /** Single-flight wake hook for sleeping cloud VM gateways (Phase 3, G3–G5). */
  wake?(gatewayId: string): Promise<void>
  /** Provision, bootstrap, and enroll a persistent Cloud Box gateway VM (#1420). */
  createCloudBox?(input: CreateCloudBoxInput): Promise<GatewaySnapshot>
  /** Stop an idle persistent Cloud Box VM while keeping its gateway entry (#1420, G5). */
  sleepCloudBox?(gatewayId: string): Promise<void>
  /** Destroy a persistent Cloud Box VM and remove its gateway entry (#1420, G11). */
  deleteCloudBox?(gatewayId: string): Promise<void>
  /** Tear down all managed remote connections and listeners. */
  dispose(): void
}
