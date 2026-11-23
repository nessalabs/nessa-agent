/**
 * Settings › Connections › Gateways (ADR 1417, #1419, #1420).
 *
 * Lists the local host gateway alongside every registered remote `nessa-server`
 * gateway (both self-hosted and managed Cloud Box VMs) with live connection
 * status, wake/sleep/reconnect/delete/remove actions, an "Add remote gateway"
 * form (G7), and a "Create Cloud Box" form (#1420).
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
  const [boxName, setBoxName] = useState("")
  const [boxCloud, setBoxCloud] = useState<"agent37" | "boat">("agent37")
  const [creatingBox, setCreatingBox] = useState(false)
  const [boxError, setBoxError] = useState<string | null>(null)
  const [pendingIds, setPendingIds] = useState<ReadonlySet<string>>(() => new Set())

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

  const runRowAction = async (
    gatewayId: string,
    action: () => Promise<void>,
    fallbackError: string,
  ) => {
    if (pendingIds.has(gatewayId)) return
    setError(null)
    setPendingIds((prev) => new Set(prev).add(gatewayId))
    try {
      await action()
    } catch (err) {
      setError(err instanceof Error ? err.message : fallbackError)
    } finally {
      setPendingIds((prev) => {
        const next = new Set(prev)
        next.delete(gatewayId)
        return next
      })
    }
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

  const submitCreateBox = async () => {
    if (!registry.createCloudBox || creatingBox) return
    if (boxName.trim() === "") return
    setBoxError(null)
    setCreatingBox(true)
    try {
      await registry.createCloudBox({ name: boxName.trim(), cloud: boxCloud })
      setBoxName("")
    } catch (err) {
      setBoxError(err instanceof Error ? err.message : "Could not create Cloud Box.")
    } finally {
      setCreatingBox(false)
    }
  }

  const handleCreateBoxSubmit = (event: FormEvent) => {
    event.preventDefault()
    void submitCreateBox()
  }

  return (
    <>
      <Group
        title="Connected gateways"
        note="Each gateway runs its own channels, agents, and terminals. Disconnecting one never disturbs the others."
      >
        {gateways.map((gateway) => {
          const isPending = pendingIds.has(gateway.gatewayId)
          const cloudBox = gateway.cloudBox
          const wake = registry.wake
          const sleepCloudBox = registry.sleepCloudBox
          const deleteCloudBox = registry.deleteCloudBox
          return (
            <ItemRow
              key={gateway.gatewayId}
              label={gateway.label}
              detail={
                gateway.isLocal
                  ? `Local host · ${gatewayStatusLabel(gateway.status)}`
                  : `${cloudBox ? `Cloud Box (${cloudBox.cloud}) · ` : ""}${gateway.url} · ${gatewayStatusLabel(gateway.status)}${
                      gateway.detail ? ` — ${gateway.detail}` : ""
                    }`
              }
              control={
                <div style={{ display: "flex", gap: "8px", alignItems: "center" }}>
                  {cloudBox &&
                    wake &&
                    gateway.status !== "connected" &&
                    gateway.status !== "waking" &&
                    gateway.status !== "auth-refused" && (
                      <RowAction
                        disabled={isPending}
                        onClick={() =>
                          void runRowAction(
                            gateway.gatewayId,
                            () => wake(gateway.gatewayId),
                            "Could not wake Cloud Box.",
                          )
                        }
                      >
                        Wake
                      </RowAction>
                    )}
                  {cloudBox && sleepCloudBox && gateway.status === "connected" && (
                    <RowAction
                      disabled={isPending}
                      onClick={() =>
                        void runRowAction(
                          gateway.gatewayId,
                          () => sleepCloudBox(gateway.gatewayId),
                          "Could not sleep Cloud Box.",
                        )
                      }
                    >
                      Sleep
                    </RowAction>
                  )}
                  {!gateway.isLocal &&
                    !cloudBox &&
                    gateway.status !== "connected" &&
                    gateway.status !== "auth-refused" && (
                      <RowAction onClick={() => registry.reconnect(gateway.gatewayId)}>
                        Reconnect
                      </RowAction>
                    )}
                  {cloudBox && deleteCloudBox && (
                    <RowAction
                      disabled={isPending}
                      onClick={() =>
                        void runRowAction(
                          gateway.gatewayId,
                          () => deleteCloudBox(gateway.gatewayId),
                          "Could not delete Cloud Box VM.",
                        )
                      }
                    >
                      Delete VM
                    </RowAction>
                  )}
                  {!gateway.isLocal && (
                    <RowAction
                      disabled={isPending}
                      onClick={() =>
                        void runRowAction(
                          gateway.gatewayId,
                          () => registry.remove(gateway.gatewayId),
                          "Could not remove gateway.",
                        )
                      }
                    >
                      Remove
                    </RowAction>
                  )}
                </div>
              }
            />
          )
        })}
      </Group>

      {registry.createCloudBox && (
        <Group
          title="Create Cloud Box"
          note={
            boxError ??
            "Provision a persistent cloud VM running nessa-server. Your channels, files, and terminals stay on its disk across sleep and wake."
          }
        >
          <form onSubmit={handleCreateBoxSubmit}>
            <ItemRow
              label="Name"
              control={
                <input
                  type="text"
                  aria-label="Cloud Box name"
                  placeholder="Cloud Box"
                  value={boxName}
                  onChange={(event) => setBoxName(event.target.value)}
                  disabled={creatingBox}
                />
              }
            />
            <ItemRow
              label="Cloud"
              control={
                <select
                  aria-label="Cloud provider"
                  value={boxCloud}
                  onChange={(event) =>
                    setBoxCloud(event.target.value === "boat" ? "boat" : "agent37")
                  }
                  disabled={creatingBox}
                >
                  <option value="agent37">Agent37</option>
                  <option value="boat">Boat</option>
                </select>
              }
            />
            <ItemRow
              label=""
              control={
                <RowAction
                  disabled={creatingBox || boxName.trim() === ""}
                  onClick={() => void submitCreateBox()}
                >
                  {creatingBox ? "Creating…" : "Create Cloud Box"}
                </RowAction>
              }
            />
          </form>
        </Group>
      )}

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
                placeholder="Work Devbox"
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
