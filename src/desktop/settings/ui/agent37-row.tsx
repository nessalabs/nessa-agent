import {
  createContext,
  useContext,
  useEffect,
  useId,
  useRef,
  useState,
  type ReactNode,
} from "react"
import { NessaAgent37Error, type Agent37Status, type Cloud } from "@nessa/client"
import { Dialog } from "@nessa-ui/react/dialog"
import type { Agent37Gateway } from "../adapters/agent37-gateway"
import { useVaultUnlock } from "../../../vault"
import { setting } from "../model/settings-catalogue"
import { Choice, FoundSetting, ItemRow, RowAction } from "./settings-controls"

const Agent37Context = createContext<Agent37Gateway | undefined>(undefined)
export function Agent37Provider({
  gateway,
  children,
}: {
  gateway?: Agent37Gateway
  children: ReactNode
}) {
  return <Agent37Context.Provider value={gateway}>{children}</Agent37Context.Provider>
}

function failure(cause: unknown): string {
  const refusal = cause instanceof NessaAgent37Error ? cause.refusal : undefined
  return refusal === "agent37_unavailable"
    ? "Agent37 refused this key, or its runtime template is not published in this workspace."
    : "Agent37 could not be connected. Check your key and connection, then try again."
}

/**
 * Settings › Connections › Accounts › Agent37: where it stands and one
 * button. The key is pasted in a dialog that Connect… opens. Below it, which
 * cloud new thread VMs are made on.
 */
export function Agent37Row() {
  const gateway = useContext(Agent37Context)
  const ensure = useVaultUnlock()
  const found = useContext(FoundSetting) === "agent37"
  const entry = setting("agent37")
  const reads = useRef(0)
  const keyId = useId()
  const [status, setStatus] = useState<Agent37Status>()
  const [cloud, setCloud] = useState<Cloud>()
  const [key, setKey] = useState("")
  const [asking, setAsking] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState("")
  const [round, setRound] = useState(0)
  useEffect(() => {
    let current = true
    const revision = ++reads.current
    void gateway
      ?.read()
      .then(async (value) => {
        const choice = await gateway.cloud()
        if (current && revision === reads.current) {
          setStatus(value)
          setCloud(choice.cloud)
        }
      })
      .catch(() => {
        if (current && revision === reads.current)
          setError("Agent37 status could not be read.")
      })
    return () => {
      current = false
    }
  }, [gateway, round])
  const act = async (
    action: (gateway: Agent37Gateway) => Promise<void>,
    fallback: (cause: unknown) => string,
  ) => {
    if (!gateway || busy) return
    reads.current += 1
    setBusy(true)
    setError("")
    try {
      if (!(await ensure())) return
      await action(gateway)
    } catch (cause) {
      setError(fallback(cause))
    } finally {
      setBusy(false)
    }
  }
  const state = status?.state
  return (
    <>
      <ItemRow
        unavailable={!gateway}
        setting="agent37"
        found={found}
        label={entry.label}
        data-agent37-state={state ?? "reading"}
        detail={
          !asking && error
            ? error
            : state === "connected"
              ? "Connected"
              : state === "locked"
                ? "Locked"
                : state === undefined
                  ? "Loading…"
                  : "Not connected"
        }
        control={
          state === "connected" ? (
            <RowAction
              disabled={busy}
              data-agent37-action="disconnect"
              onClick={() =>
                void act(
                  async (gateway) => {
                    setStatus(await gateway.disconnect())
                    setCloud((await gateway.cloud()).cloud)
                  },
                  () => "Agent37 could not be disconnected. Try again.",
                )
              }
            >
              Disconnect
            </RowAction>
          ) : state === "locked" ? (
            <RowAction
              disabled={busy}
              data-agent37-action="unlock"
              onClick={() => void ensure().then((open) => open && setRound((n) => n + 1))}
            >
              Unlock
            </RowAction>
          ) : (
            <RowAction data-agent37-action="connect" onClick={() => setAsking(true)}>
              Connect…
            </RowAction>
          )
        }
      />
      <ItemRow
        label="Cloud for new thread VMs"
        detail="A thread whose VM exists stays on the cloud it was made on."
        unavailable={!gateway || cloud === undefined || busy}
        data-cloud-choice={cloud ?? "reading"}
        control={
          <Choice
            label="Cloud for new thread VMs"
            value={cloud}
            options={[
              { id: "boat", label: "Boat" },
              { id: "agent37", label: "Agent37", disabled: state !== "connected" },
            ]}
            onChange={(next: Cloud) =>
              void act(
                async (gateway) => setCloud((await gateway.chooseCloud(next)).cloud),
                () => "The cloud could not be changed. Connect it first, then try again.",
              )
            }
          />
        }
      />
      {asking ? (
        <Dialog
          open
          size="sm"
          title="Connect Agent37"
          data-agent37-dialog=""
          onOpenChange={(open) => {
            if (!open) setAsking(false)
          }}
        >
          <form
            className="settings-surface settings-boat"
            onSubmit={(event) => {
              event.preventDefault()
              void act(async (gateway) => {
                setStatus(await gateway.save(key.trim()))
                setKey("")
                setAsking(false)
              }, failure)
            }}
          >
            <label htmlFor={keyId}>API key</label>
            <div className="settings-boat-fields">
              <input
                id={keyId}
                className="settings-input"
                type="password"
                autoComplete="off"
                spellCheck={false}
                placeholder="Paste your sk_live_ key"
                value={key}
                onChange={(event) => setKey(event.target.value)}
                disabled={busy || !gateway}
              />
              <button
                className="settings-button"
                type="submit"
                disabled={!key.trim() || busy || !gateway}
              >
                {busy ? "Connecting…" : "Connect Agent37"}
              </button>
            </div>
            <p className="settings-boat-note">
              Make one in{" "}
              <a
                href="https://www.agent37.com/dashboard/cloud/api-keys"
                target="_blank"
                rel="noreferrer"
              >
                Agent37
              </a>
              .
            </p>
            {error && <p role="alert">{error}</p>}
          </form>
        </Dialog>
      ) : null}
    </>
  )
}
