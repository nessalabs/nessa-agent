import {
  createContext,
  useContext,
  useEffect,
  useId,
  useRef,
  useState,
  type ReactNode,
} from "react"
import { NessaBoatError, type BoatStatus } from "@nessa/client"
import type { BoatGateway } from "../adapters/boat-gateway"
import { useVaultUnlock } from "../../../vault"
import { setting } from "../model/settings-catalogue"
import { FoundSetting, ItemRow, RowAction } from "./settings-controls"
const BoatContext = createContext<BoatGateway | undefined>(undefined)
export function BoatProvider({
  gateway,
  children,
}: {
  gateway?: BoatGateway
  children: ReactNode
}) {
  return <BoatContext.Provider value={gateway}>{children}</BoatContext.Provider>
}
/** The token stays in this transient field, then in the encrypted gateway vault.
 * A saved infrastructure connection is distinct from model authentication. */
export function BoatRow() {
  const gateway = useContext(BoatContext)
  const ensure = useVaultUnlock()
  const tokenId = useId()
  const reads = useRef(0)
  const [status, setStatus] = useState<BoatStatus>()
  const [token, setToken] = useState("")
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState("")
  useEffect(() => {
    let current = true
    const revision = ++reads.current
    void gateway
      ?.read()
      .then((value) => {
        if (current && revision === reads.current) setStatus(value)
      })
      .catch(() => {
        if (current && revision === reads.current)
          setError("Boat status could not be read.")
      })
    return () => {
      current = false
    }
  }, [gateway])
  const save = async () => {
    if (!gateway || busy) return
    reads.current += 1
    setBusy(true)
    setError("")
    try {
      if (!(await ensure())) return
      setStatus(await gateway.save(token.trim()))
      setToken("")
    } catch (cause) {
      setError(
        cause instanceof NessaBoatError && cause.refusal === "boat_worker_unavailable"
          ? "This Nessa build needs a matching Boat worker image. Update Nessa and try again."
          : cause instanceof NessaBoatError &&
              cause.refusal === "boat_environment_blocks_credentials"
            ? "The Nessa environment in Boat blocks agent credentials. Allow connected agents there, then retry."
            : "Boat could not be connected. Check your token and Boat connection, then try again.",
      )
    } finally {
      setBusy(false)
    }
  }
  const entry = setting("boat")
  const found = useContext(FoundSetting) === "boat"
  const disconnect = async () => {
    if (!gateway || busy) return
    reads.current += 1
    setBusy(true)
    setError("")
    try {
      if (!(await ensure())) return
      setStatus(await gateway.disconnect())
      setToken("")
    } catch {
      setError("Boat could not be disconnected. Try again.")
    } finally {
      setBusy(false)
    }
  }
  return (
    <ItemRow
      unavailable={!gateway}
      setting="boat"
      found={found}
      label={entry.label}
      detail={entry.detail}
      control={
        status?.state === "connected" ? (
          <span className="settings-boat-status" role="status">
            Connected{" "}
            <RowAction
              disabled={busy}
              onClick={() => {
                void disconnect()
              }}
            >
              Disconnect
            </RowAction>
          </span>
        ) : undefined
      }
    >
      <div className="settings-boat">
        {!gateway && (
          <p role="status">Connect this window to a Nessa gateway to manage Boat.</p>
        )}
        <p>
          Connect ChatGPT or Claude in{" "}
          <a
            href="https://boat.dev/dashboard?tab=agents"
            target="_blank"
            rel="noreferrer"
          >
            Boat
          </a>{" "}
          once, then choose Boat when creating a channel.
        </p>
        {status?.state === "locked" && <p>Unlock Nessa to manage Boat.</p>}
        <form
          onSubmit={(event) => {
            event.preventDefault()
            void save()
          }}
        >
          <label htmlFor={tokenId}>Boat API token</label>
          <div className="settings-boat-fields">
            <input
              id={tokenId}
              className="settings-input settings-input-mono"
              type="password"
              autoComplete="off"
              spellCheck={false}
              value={token}
              onChange={(event) => setToken(event.target.value)}
              disabled={busy || !gateway}
            />
            <button
              className="settings-button"
              type="submit"
              disabled={!token.trim() || busy || !gateway}
            >
              {busy
                ? "Connecting…"
                : status?.state === "connected"
                  ? "Replace token"
                  : "Connect Boat"}
            </button>
          </div>
        </form>
        <p className="settings-boat-note">
          The token is stored in Nessa’s encrypted vault. Disconnecting forgets it here;
          existing Boat VMs keep running until stopped in Boat.
        </p>
        {error && <p role="alert">{error}</p>}
      </div>
    </ItemRow>
  )
}
