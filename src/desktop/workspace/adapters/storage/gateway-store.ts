/**
 * Persists non-secret remote gateway metadata between desktop launches, and
 * defines the credential store seam (`RemoteGatewayCredentialStore`) so remote
 * tokens are never written in plaintext to `window.localStorage` (B4, Gate 9).
 */
import {
  LOCAL_GATEWAY_ID,
  normalizeGatewayUrl,
  type PersistedRemoteGateway,
} from "../../model/gateway-id"

const storageKey = "nessa.desktop.remoteGateways.v1"

export interface GatewayStorage {
  getItem(key: string): string | null
  setItem(key: string, value: string): void
}

export interface RemoteGatewayStore {
  read(): readonly PersistedRemoteGateway[]
  write(gateways: readonly PersistedRemoteGateway[]): void
}

/**
 * Process-boundary seam for remote gateway credentials.
 * Default implementation keeps credentials strictly in memory for the window's
 * lifetime so `localStorage` never holds plaintext secrets; a native host or
 * vault-backed implementation can be injected in composition.
 */
export interface RemoteGatewayCredentialStore {
  read(gatewayId: string): string | null
  write(gatewayId: string, credential: string): void
  remove(gatewayId: string): void
}

export function inMemoryGatewayCredentialStore(
  initial: Readonly<Record<string, string>> = {},
): RemoteGatewayCredentialStore {
  const map = new Map<string, string>(
    Object.entries(initial).filter(
      ([id, cred]) => id !== "" && id !== LOCAL_GATEWAY_ID && cred !== "",
    ),
  )
  return {
    read: (gatewayId) => map.get(gatewayId) ?? null,
    write: (gatewayId, credential) => {
      if (gatewayId !== "" && gatewayId !== LOCAL_GATEWAY_ID && credential !== "") {
        map.set(gatewayId, credential)
      }
    },
    remove: (gatewayId) => {
      map.delete(gatewayId)
    },
  }
}

function browserStorage(): GatewayStorage | null {
  try {
    if (typeof window === "undefined" || !window.localStorage) return null
    return window.localStorage
  } catch {
    return null
  }
}

function parseEntry(raw: unknown): PersistedRemoteGateway | null {
  if (typeof raw !== "object" || raw === null) return null
  const record = raw as Record<string, unknown>
  if (
    typeof record.gatewayId !== "string" ||
    record.gatewayId.trim() === "" ||
    record.gatewayId === LOCAL_GATEWAY_ID ||
    typeof record.label !== "string" ||
    record.label.trim() === "" ||
    typeof record.url !== "string"
  ) {
    return null
  }
  const url = normalizeGatewayUrl(record.url)
  if (url === null) return null
  const addedAtMs =
    typeof record.addedAtMs === "number" && Number.isFinite(record.addedAtMs)
      ? record.addedAtMs
      : 0
  return {
    gatewayId: record.gatewayId,
    label: record.label.trim(),
    url,
    addedAtMs,
  }
}

/**
 * Storage-backed store for persisted non-secret remote gateway metadata.
 * Deduplicates by `gatewayId` and rejects `LOCAL_GATEWAY_ID` on read (M6).
 */
export function remoteGatewayStore(
  storage: GatewayStorage | null = browserStorage(),
): RemoteGatewayStore {
  return {
    read(): readonly PersistedRemoteGateway[] {
      if (!storage) return []
      try {
        const raw = storage.getItem(storageKey)
        if (!raw) return []
        const parsed: unknown = JSON.parse(raw)
        if (!Array.isArray(parsed)) return []
        const seen = new Set<string>()
        const entries: PersistedRemoteGateway[] = []
        for (const item of parsed) {
          const entry = parseEntry(item)
          if (entry && !seen.has(entry.gatewayId)) {
            seen.add(entry.gatewayId)
            entries.push(entry)
          }
        }
        return entries
      } catch {
        return []
      }
    },
    write(gateways: readonly PersistedRemoteGateway[]): void {
      if (!storage) return
      try {
        const sanitized = gateways
          .filter(
            (entry) => entry.gatewayId !== "" && entry.gatewayId !== LOCAL_GATEWAY_ID,
          )
          .map(({ gatewayId, label, url, addedAtMs }) => ({
            gatewayId,
            label,
            url,
            addedAtMs,
          }))
        storage.setItem(storageKey, JSON.stringify(sanitized))
      } catch {
        // Storage refusal (e.g. private mode / quota) is survivable.
      }
    },
  }
}
