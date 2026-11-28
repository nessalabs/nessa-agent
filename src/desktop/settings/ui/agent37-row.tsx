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
import { useVaultSettled, useVaultUnlock } from "../../../vault"
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
 * Agent37's connection, read and changed in one place. Every page that shows
 * it uses this hook, so a status read that a save, removal or unlock has
 * overtaken is dropped here, once, for all of them.
 */
export function useAgent37Connection() {
  const gateway = useContext(Agent37Context)
  const ensure = useVaultUnlock()
  const settled = useVaultSettled()
  const reads = useRef(0)
  const [status, setStatus] = useState<Agent37Status>()
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
  }, [gateway, round, settled])
  /** Runs one change; a read still out when it starts can no longer land. */
  const act = async (
    action: (gateway: Agent37Gateway) => Promise<Agent37Status>,
    fallback: (cause: unknown) => string,
  ): Promise<boolean> => {
    if (!gateway || busy) return false
    reads.current += 1
    setBusy(true)
    setError("")
    try {
      if (!(await ensure())) return false
      setStatus(await action(gateway))
      return true
    } catch (cause) {
      setError(fallback(cause))
      return false
    } finally {
      setBusy(false)
    }
  }
  return {
    gateway,
    status,
    state: status?.state,
    unread,
    busy,
    error,
    save: (key: string) => act((gateway) => gateway.save(key.trim()), failure),
    disconnect: () =>
      act(
        (gateway) => gateway.disconnect(),
        () => "Agent37 could not be disconnected. Try again.",
      ),
    /** Opens the vault, then reads again through the one read above. */
    unlock: () => void ensure().then((open) => open && setRound((n) => n + 1)),
    retry: () => {
      setError("")
      setRound((n) => n + 1)
    },
  }
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
  const { gateway, state, unread, busy, error, save, disconnect, unlock, retry } =
    useAgent37Connection()
  const found = useContext(FoundSetting) === "agent37"
  const entry = setting("agent37")
  const keyId = useId()
  const [key, setKey] = useState("")
  const [asking, setAsking] = useState(false)
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
              onClick={() => void disconnect()}
            >
              Disconnect
            </RowAction>
          ) : unread && state === undefined ? (
            // Whether Agent37 is connected is unknown: never offer to
            // replace a key that may be saved.
            <RowAction data-agent37-action="retry" onClick={retry}>
              Try again
            </RowAction>
          ) : state === "locked" ? (
            <RowAction disabled={busy} data-agent37-action="unlock" onClick={unlock}>
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
              void save(key).then((saved) => {
                if (!saved) return
                setKey("")
                setAsking(false)
              })
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
