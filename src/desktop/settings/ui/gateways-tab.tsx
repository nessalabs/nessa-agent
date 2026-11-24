/**
 * Settings › Connections › Gateways (ADR 1417, #1419).
 *
 * Lists the local host gateway alongside every registered remote `nessa-server`
 * gateway with live connection status, reconnect, and remove actions, plus an
 * "Add remote gateway" form that verifies the handshake before saving (G7).
 */
import {
  createContext,
  useContext,
  useEffect,
  useState,
  type FormEvent,
  type ReactNode,
} from "react"
import {
  gatewayStatusLabel,
  type GatewayRegistryPort,
  type GatewaySnapshot,
} from "../../workspace"
import { DesktopIcon } from "../../ui/icons"
import { Group, ItemRow, RowAction } from "./settings-controls"

const GatewayRegistryContext = createContext<GatewayRegistryPort | undefined>(undefined)

export function GatewayRegistryProvider({
  registry,
  children,
}: {
  registry: GatewayRegistryPort | undefined
  children: ReactNode
}) {
  return (
    <GatewayRegistryContext.Provider value={registry}>
      {children}
    </GatewayRegistryContext.Provider>
  )
}

export function GatewaysTab() {
  const registry = useContext(GatewayRegistryContext)
  const [gateways, setGateways] = useState<readonly GatewaySnapshot[]>(() =>
    registry ? registry.list() : [],
  )
  const [label, setLabel] = useState("")
  const [url, setUrl] = useState("")
  const [credential, setCredential] = useState("")
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [removingIds, setRemovingIds] = useState<ReadonlySet<string>>(() => new Set())

  useEffect(() => {
    if (!registry) return
    return registry.subscribe(setGateways)
  }, [registry])

  if (!registry) {
    return (
      <div className="settings-empty">
        <DesktopIcon name="connections" />
        <p>Connect to a gateway to manage additional remote gateways.</p>
      </div>
    )
  }

  const submitAdd = async () => {
    if (busy) return
    if (label.trim() === "" || url.trim() === "" || credential.trim() === "") return
    setError(null)
    setBusy(true)
    try {
      await registry.add({ label, url, credential })
      setLabel("")
      setUrl("")
      setCredential("")
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not connect to gateway.")
    } finally {
      setBusy(false)
    }
  }

  const handleSubmit = (event: FormEvent) => {
    event.preventDefault()
    void submitAdd()
  }

  const handleRemove = async (gatewayId: string) => {
    if (removingIds.has(gatewayId)) return
    setError(null)
    setRemovingIds((prev) => new Set(prev).add(gatewayId))
    try {
      await registry.remove(gatewayId)
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not remove gateway.")
    } finally {
      setRemovingIds((prev) => {
        const next = new Set(prev)
        next.delete(gatewayId)
        return next
      })
    }
  }

  return (
    <>
      <Group
        title="Connected gateways"
        note="Each gateway runs its own channels, agents, and terminals. Disconnecting one never disturbs the others."
      >
        {gateways.map((gateway) => (
          <ItemRow
            key={gateway.gatewayId}
            label={gateway.label}
            detail={
              gateway.isLocal
                ? `Local host · ${gatewayStatusLabel(gateway.status)}`
                : `${gateway.url} · ${gatewayStatusLabel(gateway.status)}${
                    gateway.detail ? ` — ${gateway.detail}` : ""
                  }`
            }
            control={
              <div style={{ display: "flex", gap: "8px", alignItems: "center" }}>
                {!gateway.isLocal &&
                  gateway.status !== "connected" &&
                  gateway.status !== "auth-refused" && (
                    <RowAction onClick={() => registry.reconnect(gateway.gatewayId)}>
                      Reconnect
                    </RowAction>
                  )}
                {!gateway.isLocal && (
                  <RowAction
                    disabled={removingIds.has(gateway.gatewayId)}
                    onClick={() => void handleRemove(gateway.gatewayId)}
                  >
                    Remove
                  </RowAction>
                )}
              </div>
            }
          />
        ))}
      </Group>

      <Group
        title="Add remote gateway"
        note={
          error ??
          "Connect a headless Linux nessa-server over wss:// (or ws://127.0.0.1 for local testing). The handshake is verified before saving."
        }
      >
        <form onSubmit={handleSubmit}>
          <ItemRow
            label="Name"
            control={
              <input
                type="text"
                aria-label="Gateway name"
                placeholder="Cloud Box"
                value={label}
                onChange={(event) => setLabel(event.target.value)}
                disabled={busy}
              />
            }
          />
          <ItemRow
            label="Address"
            control={
              <input
                type="url"
                aria-label="Gateway address"
                placeholder="wss://box.example.com"
                value={url}
                onChange={(event) => setUrl(event.target.value)}
                disabled={busy}
              />
            }
          />
          <ItemRow
            label="Credential"
            control={
              <input
                type="password"
                aria-label="Gateway credential"
                placeholder="Device or owner token"
                value={credential}
                onChange={(event) => setCredential(event.target.value)}
                disabled={busy}
              />
            }
          />
          <ItemRow
            label=""
            control={
              <RowAction
                disabled={
                  busy ||
                  label.trim() === "" ||
                  url.trim() === "" ||
                  credential.trim() === ""
                }
                onClick={() => void submitAdd()}
              >
                {busy ? "Connecting…" : "Connect"}
              </RowAction>
            }
          />
        </form>
      </Group>
    </>
  )
}
