import {
  createContext,
  useContext,
  useEffect,
  useId,
  useRef,
  useState,
  type ReactNode,
} from "react"
import { NessaAgent37Error, type Agent37Status } from "@nessa/client"
import { Dialog } from "@nessa-ui/react/dialog"
import type { Agent37Gateway } from "../adapters/agent37-gateway"
import { useVaultUnlock } from "../../../vault"
import { setting } from "../model/settings-catalogue"
import { FoundSetting, ItemRow, RowAction } from "./settings-controls"

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
export function useAgent37Gateway() {
  return useContext(Agent37Context)
}

function failure(cause: unknown): string {
  const refusal = cause instanceof NessaAgent37Error ? cause.refusal : undefined
  return refusal === "agent37_unavailable"
    ? "Agent37 refused this key, or its runtime template is not published in this workspace."
    : "Agent37 could not be connected. Check your key and connection, then try again."
}

/**
 * Settings › Accounts › Agent37: where it stands and one button. The key is
 * pasted in a dialog that Connect… opens. Which cloud a thread uses is its
 * channel's choice, or the default cloud's (`DefaultCloudRow`).
 */
export function Agent37Row({
  onChange,
}: {
  /** Hears each state the row comes to read, so a page around it can follow. */
  onChange?: (state: Agent37Status["state"]) => void
} = {}) {
  const gateway = useContext(Agent37Context)
  const ensure = useVaultUnlock()
  const found = useContext(FoundSetting) === "agent37"
  const entry = setting("agent37")
  const reads = useRef(0)
  const keyId = useId()
  const [status, setStatus] = useState<Agent37Status>()
  const [key, setKey] = useState("")
  const [asking, setAsking] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState("")
  const [round, setRound] = useState(0)
  const [unread, setUnread] = useState(false)
  useEffect(() => {
    let current = true
    const revision = ++reads.current
    setUnread(false)
    void gateway
      ?.read()
      .then((value) => {
        if (current && revision === reads.current) setStatus(value)
      })
      .catch(() => {
        if (current && revision === reads.current) {
          setUnread(true)
          setError("Agent37 status could not be read.")
        }
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
  useEffect(() => {
    if (state !== undefined) onChange?.(state)
  }, [state, onChange])
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
                  async (gateway) => setStatus(await gateway.disconnect()),
                  () => "Agent37 could not be disconnected. Try again.",
                )
              }
            >
              Disconnect
            </RowAction>
          ) : unread && state === undefined ? (
            // Whether Agent37 is connected is unknown: never offer to
            // replace a key that may be saved.
            <RowAction
              data-agent37-action="retry"
              onClick={() => {
                setError("")
                setRound((n) => n + 1)
              }}
            >
              Try again
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
