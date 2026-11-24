/**
 * Multi-gateway registry adapter (`GatewayRegistryPort`) managing the local
 * host gateway alongside 0..N remote `nessa-server` gateways over `wss://`
 * (ADR 1417, #1419).
 *
 * Responsibilities:
 * - Holds the local `GatewaySource` / `TerminalSource` / `ChannelSource` entry
 *   under `LOCAL_GATEWAY_ID` (never persisted to browser storage).
 * - Restores persisted non-secret metadata from `RemoteGatewayStore` and reads
 *   credentials from `RemoteGatewayCredentialStore` (never from `localStorage`).
 * - Verifies handshake + non-empty, non-local `productSession.gatewayId` before
 *   persisting on `add()`, deduplicating by stable `gatewayId` (G7, M6).
 * - Uses canonical `isSignedOut` / `isAuthenticationFailure` (Gate 1 & Gate 13)
 *   and disposes `gatewaySource` immediately on `auth-refused` so retry timers
 *   halt until re-paired (G9, M3).
 * - Remounts `gatewaySource` cleanly on explicit `reconnect()` / `updateEndpoint()`
 *   / `wake()` so backoff never swallows explicit user/wake reconnects (G3, G8, M2).
 * - Calls `entry.source.dispose()` and closes sockets on `remove()`, re-pair
 *   `add()`, and `dispose()` (G11, B3).
 */
import {
  NessaClient,
  type ConnectionState,
  type NessaClient as NessaClientType,
} from "@nessa/client"
import { isSignedOut } from "../../../../session"
import {
  LOCAL_GATEWAY_ID,
  normalizeGatewayUrl,
  removeRemoteGateway,
  upsertRemoteGateway,
  type GatewayConnectionState,
  type GatewaySnapshot,
  type PersistedRemoteGateway,
} from "../../model/gateway-id"
import {
  GatewayRegistryError,
  type AddGatewayInput,
  type CreateCloudBoxInput,
  type GatewayRegistryPort,
} from "../../application/gateway-registry"
import type {
  ChannelSource,
  TerminalSource,
  WorkspaceSource,
} from "../../application/ports"
import {
  inMemoryGatewayCredentialStore,
  type RemoteGatewayCredentialStore,
  type RemoteGatewayStore,
} from "../storage/gateway-store"
import {
  gatewaySource,
  type GatewayClient,
  type GatewaySource,
  type ImageBytes,
} from "./gateway-source"
import { terminalGateway, type TerminalClient } from "./terminal-gateway"
import { channelGateway } from "./channel-gateway"
import type { ManagedWorkspaceGateway } from "./multi-gateway-source"
import type { ManagedTerminalGateway } from "./multi-terminal-gateway"
import type { ManagedChannelGateway } from "./multi-channel-gateway"

export type RemoteClient = GatewayClient &
  TerminalClient & {
    readonly productSession?: { readonly gatewayId: string }
    readonly connectionState?: ConnectionState
    onConnectionStateChange?(listener: (state: ConnectionState) => void): () => void
    close(): void | Promise<void>
  }

export type RemoteClientConnector = (params: {
  readonly url: string
  readonly credential: string
}) => Promise<RemoteClient>

/** Process-boundary port for persistent Cloud Box VM lifecycle orchestration (#1420). */
export interface CloudBoxesLifecyclePort {
  /**
   * Create, or finish creating, the box the caller named by `boxId`. The same
   * `boxId` asked again resumes that creation on the gateway, so a lost answer
   * or a failed enrollment is retried, never answered with a second VM.
   * Rejects with an error whose `refusal` names a gateway refusal code, or
   * none for a transport failure or a timeout.
   */
  create(input: CreateCloudBoxInput & { readonly boxId: string }): Promise<{
    readonly boxId: string
    readonly name: string
    readonly cloud: "boat" | "agent37"
    readonly url: string
    readonly credential: string
  }>
  wake(boxId: string): Promise<{ readonly url: string }>
  sleep(boxId: string): Promise<void>
  delete(boxId: string): Promise<void>
}

export interface GatewayRegistryOptions {
  readonly local: {
    readonly label: string
    readonly url: string
    readonly workspace: WorkspaceSource
    readonly terminals: TerminalSource
    readonly channels: ChannelSource
  }
  readonly store: RemoteGatewayStore
  /** Process-boundary credential store seam (defaults to in-memory, never localStorage). */
  readonly credentialStore?: RemoteGatewayCredentialStore
  readonly now: () => number
  readonly after: (ms: number, run: () => void) => () => void
  readonly newId?: () => string
  /** Attachment byte reader + digest for remote conversations (Codex P2). */
  readonly imageBytes?: ImageBytes
  /** Injectable connector seam for tests; defaults to `NessaClient.connect`. */
  readonly connectRemote?: RemoteClientConnector
  /** Optional wake provider seam for sleeping Cloud VM gateways (Phase 3). */
  readonly wakeGateway?: (gatewayId: string) => Promise<void>
  /** Optional Cloud Box orchestration port backed by `cloud.boxes.*` (#1420). */
  readonly cloudBoxes?: CloudBoxesLifecyclePort
}

interface ManagedRemoteEntry {
  record: PersistedRemoteGateway
  status: GatewayConnectionState
  detail?: string
  updatedAtMs: number
  source: GatewaySource
  terminals: TerminalSource
  channels: ChannelSource
  activeClient: RemoteClient | null
  unsubState: (() => void) | null
  disposed: boolean
  wakeInFlight: Promise<void> | null
}

function defaultConnectRemote(params: {
  readonly url: string
  readonly credential: string
}): Promise<RemoteClient> {
  return NessaClient.connect({
    url: params.url,
    auth: { credential: params.credential },
    role: "surface",
    surface: { kind: "desktop", instance: crypto.randomUUID() },
    client: { id: "nessa-desktop-remote", version: "0.1.0", platform: "browser" },
  }) as unknown as Promise<RemoteClient>
}

/** How many times `createCloudBox` asks for the same box before giving up. */
export const CLOUD_BOX_CREATE_ATTEMPTS = 3

/** Gateway refusals that asking again with the same box id cannot change. */
const FINAL_CLOUD_BOX_REFUSALS: ReadonlySet<string> = new Set([
  "invalid_request",
  "vault_locked",
  "cloud_not_configured",
  "cloud_box_already_exists",
  "cloud_box_not_active",
  "cloud_box_admission_refused",
])

function isFinalCloudBoxRefusal(error: unknown): boolean {
  const refusal =
    typeof error === "object" && error !== null && "refusal" in error
      ? (error as { refusal?: unknown }).refusal
      : undefined
  return typeof refusal === "string" && FINAL_CLOUD_BOX_REFUSALS.has(refusal)
}

function isAuthError(error: unknown): boolean {
  return isSignedOut(error)
}

export type ManagedDesktopGateway = ManagedWorkspaceGateway &
  ManagedTerminalGateway &
  ManagedChannelGateway

export interface DesktopGatewayRegistry extends GatewayRegistryPort {
  entries(): readonly ManagedDesktopGateway[]
  entry(gatewayId: string): ManagedDesktopGateway | undefined
  onTopologyChange(listener: () => void): () => void
}

/**
 * Create the Desktop's multi-gateway registry.
 */
export function createGatewayRegistry(
  options: GatewayRegistryOptions,
): DesktopGatewayRegistry {
  const connectRemote = options.connectRemote ?? defaultConnectRemote
  const credentialStore = options.credentialStore ?? inMemoryGatewayCredentialStore()
  const newId = options.newId ?? (() => crypto.randomUUID())
  const listeners = new Set<(gateways: readonly GatewaySnapshot[]) => void>()
  const topologyListeners = new Set<() => void>()
  const remotes = new Map<string, ManagedRemoteEntry>()
  let registryDisposed = false
  let localLabel = options.local.label
  let persisted: readonly PersistedRemoteGateway[] = options.store.read()

  const localEntry = (): ManagedDesktopGateway => ({
    gatewayId: LOCAL_GATEWAY_ID,
    label: localLabel,
    status: "connected",
    workspace: options.local.workspace,
    terminals: options.local.terminals,
    channels: options.local.channels,
  })

  function snapshotOf(entry: ManagedRemoteEntry): GatewaySnapshot {
    return {
      gatewayId: entry.record.gatewayId,
      label: entry.record.label,
      url: entry.record.url,
      isLocal: false,
      status: entry.status,
      detail: entry.detail,
      serverGatewayId: entry.record.gatewayId,
      ...(entry.record.cloudBox ? { cloudBox: entry.record.cloudBox } : {}),
      updatedAtMs: entry.updatedAtMs,
    }
  }

  function listSnapshots(): readonly GatewaySnapshot[] {
    const localSnapshot: GatewaySnapshot = {
      gatewayId: LOCAL_GATEWAY_ID,
      label: localLabel,
      url: options.local.url,
      isLocal: true,
      status: "connected",
      updatedAtMs: options.now(),
    }
    return [localSnapshot, ...Array.from(remotes.values(), snapshotOf)]
  }

  function notifySnapshots(): void {
    const snapshots = listSnapshots()
    for (const listener of listeners) {
      listener(snapshots)
    }
  }

  function notifyTopology(): void {
    notifySnapshots()
    for (const listener of topologyListeners) {
      listener()
    }
  }

  function disposeRemoteEntry(entry: ManagedRemoteEntry): void {
    entry.disposed = true
    entry.unsubState?.()
    entry.unsubState = null
    try {
      entry.source?.dispose()
    } catch {
      // Disposing an already-disposed source is harmless.
    }
    if (entry.activeClient) {
      try {
        void entry.activeClient.close()
      } catch {
        // Closing an already-closed socket is harmless.
      }
      entry.activeClient = null
    }
  }

  function markAuthRefused(entry: ManagedRemoteEntry, detail: string): void {
    if (entry.disposed) return
    entry.status = "auth-refused"
    entry.detail = detail
    entry.updatedAtMs = options.now()
    entry.unsubState?.()
    entry.unsubState = null
    // G9 / M3: dispose gatewaySource immediately so retry timers halt until re-paired.
    try {
      entry.source?.dispose()
    } catch {
      // Ignore already-disposed source.
    }
    if (entry.activeClient) {
      try {
        void entry.activeClient.close()
      } catch {
        // Ignore already-closed client.
      }
      entry.activeClient = null
    }
    notifyTopology()
  }

  function mountRemote(
    record: PersistedRemoteGateway,
    preconnected?: RemoteClient,
  ): ManagedRemoteEntry {
    let initialClient: RemoteClient | null = preconnected ?? null
    const hasCredential = credentialStore.read(record.gatewayId) !== null

    const entry: ManagedRemoteEntry = {
      record,
      status: preconnected ? "connected" : hasCredential ? "connecting" : "auth-refused",
      detail:
        !preconnected && !hasCredential
          ? "Re-pair this gateway to reconnect."
          : undefined,
      updatedAtMs: options.now(),
      source: undefined as unknown as GatewaySource,
      terminals: undefined as unknown as TerminalSource,
      channels: undefined as unknown as ChannelSource,
      activeClient: preconnected ?? null,
      unsubState: null,
      disposed: false,
      wakeInFlight: null,
    }

    const bindClientState = (client: RemoteClient) => {
      entry.unsubState?.()
      entry.activeClient = client
      if (client.onConnectionStateChange) {
        entry.unsubState = client.onConnectionStateChange((state) => {
          if (entry.disposed) return
          if (state.status === "connected") {
            const wasConnected = entry.status === "connected"
            entry.status = "connected"
            entry.detail = undefined
            entry.updatedAtMs = options.now()
            if (!wasConnected) notifyTopology()
            else notifySnapshots()
          } else if (state.status === "reconnecting") {
            if (entry.status !== "auth-refused") {
              entry.status = "reconnecting"
              entry.updatedAtMs = options.now()
              notifySnapshots()
            }
          } else if (state.status === "closed") {
            if (isAuthError(state.error)) {
              markAuthRefused(entry, "Credentials were rejected — re-pair this gateway.")
            } else if (entry.status !== "auth-refused") {
              entry.status = "disconnected"
              entry.updatedAtMs = options.now()
              notifySnapshots()
            }
          }
        })
      }
    }

    if (preconnected) {
      bindClientState(preconnected)
    }

    const connect = async (): Promise<GatewayClient> => {
      if (entry.disposed) {
        throw new GatewayRegistryError("offline", "Gateway was removed")
      }
      if (entry.status === "auth-refused") {
        throw new GatewayRegistryError("auth-refused", entry.detail)
      }
      if (initialClient) {
        const reused = initialClient
        initialClient = null
        return reused
      }
      const credential = credentialStore.read(entry.record.gatewayId)
      if (!credential) {
        markAuthRefused(entry, "Re-pair this gateway to reconnect.")
        throw new GatewayRegistryError("auth-refused", entry.detail)
      }
      entry.status = entry.activeClient ? "reconnecting" : "connecting"
      entry.updatedAtMs = options.now()
      notifySnapshots()
      try {
        const client = await connectRemote({
          url: entry.record.url,
          credential,
        })
        if (entry.disposed) {
          void client.close()
          throw new GatewayRegistryError("offline", "Gateway was removed")
        }
        const reportedGatewayId = client.productSession?.gatewayId?.trim() ?? ""
        if (reportedGatewayId !== entry.record.gatewayId) {
          try {
            void client.close()
          } catch {
            // Ignore close failure on rejected identity.
          }
          entry.status = "refused"
          entry.detail = "Connected endpoint reported a different gateway identity."
          entry.updatedAtMs = options.now()
          notifySnapshots()
          throw new GatewayRegistryError("connect-failed", entry.detail)
        }
        bindClientState(client)
        entry.status = "connected"
        entry.detail = undefined
        entry.updatedAtMs = options.now()
        notifyTopology()
        return client
      } catch (error) {
        if (isAuthError(error)) {
          markAuthRefused(entry, "Credentials were rejected — re-pair this gateway.")
          throw error
        }
        if (entry.status !== "refused") {
          entry.status = "disconnected"
          entry.detail =
            error instanceof Error ? error.message : "Could not reach gateway"
          entry.updatedAtMs = options.now()
          notifySnapshots()
        }
        throw error
      }
    }

    const source = gatewaySource({
      connect,
      clock: { now: options.now, after: options.after },
      imageBytes: options.imageBytes,
    })
    entry.source = source
    entry.terminals = terminalGateway({
      connected: () => source.connected() as unknown as Promise<TerminalClient>,
      newId,
    })
    entry.channels = channelGateway({
      connected: () => source.connected() as unknown as Promise<NessaClientType>,
    })
    if (entry.status === "auth-refused") {
      // R2-M1: halt retry timers immediately when restored without a credential.
      source.dispose()
    }
    return entry
  }

  for (const record of persisted) {
    remotes.set(record.gatewayId, mountRemote(record))
  }

  const registry: DesktopGatewayRegistry = {
    list: listSnapshots,

    subscribe(listener) {
      listeners.add(listener)
      listener(listSnapshots())
      return () => {
        listeners.delete(listener)
      }
    },

    async add(input: AddGatewayInput): Promise<GatewaySnapshot> {
      const label = input.label.trim()
      if (label === "") {
        throw new GatewayRegistryError("invalid-label", "Give this gateway a name.")
      }
      const url = normalizeGatewayUrl(input.url)
      if (url === null) {
        throw new GatewayRegistryError(
          "invalid-url",
          "Use wss:// for remote gateways (ws:// is allowed only for 127.0.0.1 or [::1]).",
        )
      }
      const credential = input.credential.trim()
      if (credential === "") {
        throw new GatewayRegistryError(
          "invalid-credential",
          "Enter a device credential or token.",
        )
      }

      if (registryDisposed) {
        throw new GatewayRegistryError("offline", "Gateway registry was disposed.")
      }

      // G7: Verify connectivity and handshake before persisting anything.
      let client: RemoteClient
      try {
        client = await connectRemote({ url, credential })
      } catch (error) {
        if (isAuthError(error)) {
          throw new GatewayRegistryError(
            "auth-refused",
            "The gateway rejected this credential. Check the token or re-pair.",
          )
        }
        throw new GatewayRegistryError(
          "connect-failed",
          error instanceof Error ? error.message : "Could not connect to gateway.",
        )
      }

      const verifiedGatewayId = client.productSession?.gatewayId?.trim() ?? ""
      if (verifiedGatewayId === "" || verifiedGatewayId === LOCAL_GATEWAY_ID) {
        try {
          void client.close()
        } catch {
          // Ignore close failure on rejected handshake.
        }
        throw new GatewayRegistryError(
          "connect-failed",
          "Gateway did not report a valid remote gatewayId.",
        )
      }

      if (registryDisposed) {
        try {
          void client.close()
        } catch {
          // Ignore close failure on teardown.
        }
        throw new GatewayRegistryError("offline", "Gateway registry was disposed.")
      }

      const existing = remotes.get(verifiedGatewayId)
      const cloudBox = input.cloudBox ?? existing?.record.cloudBox
      if (existing) {
        disposeRemoteEntry(existing)
      }

      credentialStore.write(verifiedGatewayId, credential)
      const record: PersistedRemoteGateway = {
        gatewayId: verifiedGatewayId,
        label,
        url,
        ...(cloudBox ? { cloudBox } : {}),
        addedAtMs: options.now(),
      }
      const mounted = mountRemote(record, client)
      remotes.set(verifiedGatewayId, mounted)
      persisted = upsertRemoteGateway(persisted, record)
      options.store.write(persisted)
      notifyTopology()
      return snapshotOf(mounted)
    },

    rename(gatewayId: string, label: string): void {
      const trimmed = label.trim()
      if (trimmed === "") return
      if (gatewayId === LOCAL_GATEWAY_ID) {
        localLabel = trimmed
        notifyTopology()
        return
      }
      const entry = remotes.get(gatewayId)
      if (!entry) return
      entry.record = { ...entry.record, label: trimmed }
      entry.updatedAtMs = options.now()
      persisted = upsertRemoteGateway(persisted, entry.record)
      options.store.write(persisted)
      notifyTopology()
    },

    async remove(gatewayId: string): Promise<void> {
      if (gatewayId === LOCAL_GATEWAY_ID) {
        throw new GatewayRegistryError(
          "cannot-remove-local",
          "The local host gateway cannot be removed.",
        )
      }
      const entry = remotes.get(gatewayId)
      if (!entry) return
      disposeRemoteEntry(entry)
      remotes.delete(gatewayId)
      credentialStore.remove(gatewayId)
      persisted = removeRemoteGateway(persisted, gatewayId)
      options.store.write(persisted)
      notifyTopology()
    },

    reconnect(gatewayId: string): void {
      if (gatewayId === LOCAL_GATEWAY_ID) return
      const existing = remotes.get(gatewayId)
      if (!existing || existing.disposed) return
      // G9 / M2: never reconnect silently when credentials were refused.
      if (existing.status === "auth-refused") return
      const record = existing.record
      disposeRemoteEntry(existing)
      const remounted = mountRemote(record)
      remounted.status = "reconnecting"
      remounted.updatedAtMs = options.now()
      remotes.set(gatewayId, remounted)
      notifyTopology()
      void remounted.source.connected().catch(() => {
        // Error state is recorded by `connect()`.
      })
    },

    updateEndpoint(gatewayId: string, nextUrl: string): void {
      const url = normalizeGatewayUrl(nextUrl)
      if (!url) return
      const entry = remotes.get(gatewayId)
      if (!entry || entry.disposed) return
      entry.record = { ...entry.record, url }
      persisted = upsertRemoteGateway(persisted, entry.record)
      options.store.write(persisted)
      registry.reconnect(gatewayId)
    },

    async wake(gatewayId: string): Promise<void> {
      if (gatewayId === LOCAL_GATEWAY_ID) return
      const entry = remotes.get(gatewayId)
      if (!entry || entry.disposed) {
        throw new GatewayRegistryError("not-found", "Unknown gateway")
      }
      // G9 / R2-M2: never wake/reconnect a gateway whose credentials were refused.
      if (entry.status === "auth-refused") {
        throw new GatewayRegistryError("auth-refused", entry.detail)
      }
      const boxId = entry.record.cloudBox?.boxId
      const canWakeCloudBox = Boolean(boxId && options.cloudBoxes)
      if (!canWakeCloudBox && !options.wakeGateway) return
      // G3: single-flight wake per gatewayId.
      if (entry.wakeInFlight) return entry.wakeInFlight
      entry.status = "waking"
      entry.detail = undefined
      entry.updatedAtMs = options.now()
      notifySnapshots()
      const performWake = async (): Promise<void> => {
        if (canWakeCloudBox && boxId && options.cloudBoxes) {
          // Which gateway answers at the woken address is checked by the
          // reconnect's handshake against this entry's gatewayId.
          const woken = await options.cloudBoxes.wake(boxId)
          if (entry.disposed) return
          const normalized = normalizeGatewayUrl(woken.url)
          if (!normalized) {
            throw new GatewayRegistryError(
              "invalid-url",
              "Woken Cloud Box returned an invalid endpoint URL.",
            )
          }
          if (normalized !== entry.record.url) {
            // G8: rotate persisted endpoint URL and reconnect.
            registry.updateEndpoint?.(gatewayId, normalized)
          } else {
            registry.reconnect(gatewayId)
          }
          return
        }
        if (options.wakeGateway) {
          await options.wakeGateway(gatewayId)
          if (!entry.disposed) {
            registry.reconnect(gatewayId)
          }
        }
      }
      const flight = performWake()
        .catch((error) => {
          if (!entry.disposed) {
            entry.status = "refused"
            entry.detail =
              error instanceof Error ? error.message : "Could not wake gateway"
            entry.updatedAtMs = options.now()
            notifySnapshots()
          }
          throw error
        })
        .finally(() => {
          entry.wakeInFlight = null
        })
      entry.wakeInFlight = flight
      return flight
    },

    ...(options.cloudBoxes
      ? (() => {
          const cloudBoxes = options.cloudBoxes
          return {
            async createCloudBox(input: CreateCloudBoxInput): Promise<GatewaySnapshot> {
              if (registryDisposed) {
                throw new GatewayRegistryError(
                  "offline",
                  "Gateway registry was disposed.",
                )
              }
              const name = input.name.trim()
              if (name === "") {
                throw new GatewayRegistryError(
                  "invalid-label",
                  "Cloud Box name is required.",
                )
              }
              // The box is named here, so every attempt below is the same
              // creation on the gateway: a lost answer, a failed audit or a
              // failed enrollment is finished by asking again, and only a box
              // that still cannot be enrolled is deleted.
              const boxId = newId()
              let lastError: unknown
              for (let attempt = 0; attempt < CLOUD_BOX_CREATE_ATTEMPTS; attempt++) {
                if (registryDisposed) break
                let created: Awaited<ReturnType<CloudBoxesLifecyclePort["create"]>>
                try {
                  created = await cloudBoxes.create({ boxId, name, cloud: input.cloud })
                } catch (error) {
                  if (isFinalCloudBoxRefusal(error)) throw error
                  lastError = error
                  continue
                }
                try {
                  return await registry.add({
                    label: created.name,
                    url: created.url,
                    credential: created.credential,
                    cloudBox: { boxId, cloud: created.cloud },
                  })
                } catch (enrollError) {
                  lastError = enrollError
                }
              }
              try {
                await cloudBoxes.delete(boxId)
              } catch {
                throw new GatewayRegistryError(
                  "offline",
                  `Cloud Box ${boxId} could not be enrolled and rollback deletion failed; remove it manually.`,
                )
              }
              throw lastError
            },

            async sleepCloudBox(gatewayId: string): Promise<void> {
              const entry = remotes.get(gatewayId)
              const boxId = entry?.record.cloudBox?.boxId
              if (!entry || entry.disposed || !boxId) {
                throw new GatewayRegistryError("not-found", "Unknown Cloud Box gateway")
              }
              if (entry.wakeInFlight) {
                throw new GatewayRegistryError(
                  "offline",
                  "Cloud Box is currently waking; wait before sleeping.",
                )
              }
              await cloudBoxes.sleep(boxId)
              if (registryDisposed) return
              const current = remotes.get(gatewayId)
              if (!current || current !== entry || current.disposed) {
                return
              }
              current.unsubState?.()
              current.unsubState = null
              try {
                current.source.dispose()
              } catch {
                // Ignore already-disposed source.
              }
              if (current.activeClient) {
                try {
                  void current.activeClient.close()
                } catch {
                  // Ignore already-closed client.
                }
                current.activeClient = null
              }
              current.status = "disconnected"
              current.detail = "Sleeping"
              current.updatedAtMs = options.now()
              notifyTopology()
            },

            async deleteCloudBox(gatewayId: string): Promise<void> {
              const entry = remotes.get(gatewayId)
              const boxId = entry?.record.cloudBox?.boxId
              if (!entry || entry.disposed || !boxId) {
                throw new GatewayRegistryError("not-found", "Unknown Cloud Box gateway")
              }
              await cloudBoxes.delete(boxId)
              if (registryDisposed) {
                credentialStore.remove(gatewayId)
                persisted = removeRemoteGateway(persisted, gatewayId)
                options.store.write(persisted)
                return
              }
              await registry.remove(gatewayId)
            },
          }
        })()
      : {}),

    dispose(): void {
      registryDisposed = true
      for (const entry of remotes.values()) {
        disposeRemoteEntry(entry)
      }
      remotes.clear()
      listeners.clear()
      topologyListeners.clear()
    },

    entries() {
      const list: ManagedDesktopGateway[] = [localEntry()]
      for (const entry of remotes.values()) {
        list.push({
          gatewayId: entry.record.gatewayId,
          label: entry.record.label,
          status: entry.status,
          workspace: entry.source,
          terminals: entry.terminals,
          channels: entry.channels,
        })
      }
      return list
    },

    entry(gatewayId: string) {
      if (gatewayId === LOCAL_GATEWAY_ID) return localEntry()
      const entry = remotes.get(gatewayId)
      if (!entry) return undefined
      return {
        gatewayId: entry.record.gatewayId,
        label: entry.record.label,
        status: entry.status,
        workspace: entry.source,
        terminals: entry.terminals,
        channels: entry.channels,
      }
    },

    onTopologyChange(listener: () => void) {
      topologyListeners.add(listener)
      return () => {
        topologyListeners.delete(listener)
      }
    },
  }

  return registry
}
