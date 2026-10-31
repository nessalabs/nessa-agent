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
import { BoatRuntimeControl } from "./boat-runtime-control"
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
export function useBoatGateway() {
  return useContext(BoatContext)
}
/** What the Boat connection reads and does: its status, the token being entered, save and disconnect. */
export interface BoatConnection {
  readonly gateway: BoatGateway | undefined
  readonly status: BoatStatus | undefined
  /** No status will come: there is no gateway, or its read failed. */
  readonly unread: boolean
  readonly token: string
  readonly setToken: (token: string) => void
  readonly busy: boolean
  readonly error: string
  readonly save: () => Promise<void>
  readonly disconnect: () => Promise<void>
  /** Asks for the vault, then reads the status again. */
  readonly unlock: () => Promise<void>
}

/**
 * The token stays in this transient field, then in the encrypted gateway vault.
 * A saved infrastructure connection is distinct from model authentication.
 * `onChange` hears each status a save or disconnect answers; a change of
 * `seen`, what another read last said about the connection, reads it again.
 */
export function useBoatConnection(
  onChange?: (status: BoatStatus) => void,
  seen?: unknown,
): BoatConnection {
  const gateway = useContext(BoatContext)
  const ensure = useVaultUnlock()
  const reads = useRef(0)
  const [status, setStatus] = useState<BoatStatus>()
  const [token, setToken] = useState("")
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState("")
  const [readFailed, setReadFailed] = useState(false)
  const [round, setRound] = useState(0)
  useEffect(() => {
    let current = true
    const revision = ++reads.current
    setReadFailed(false)
    void gateway
      ?.read()
      .then((value) => {
        if (current && revision === reads.current) setStatus(value)
      })
      .catch(() => {
        if (current && revision === reads.current) {
          setReadFailed(true)
          setError("Boat status could not be read.")
        }
      })
    return () => {
      current = false
    }
  }, [gateway, round, seen])
  const answered = (value: BoatStatus) => {
    setStatus(value)
    onChange?.(value)
  }
  const save = async () => {
    if (!gateway || busy) return
    reads.current += 1
    setBusy(true)
    setError("")
    try {
      if (!(await ensure())) return
      answered(await gateway.save(token.trim()))
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
  const disconnect = async () => {
    if (!gateway || busy) return
    reads.current += 1
    setBusy(true)
    setError("")
    try {
      if (!(await ensure())) return
      answered(await gateway.disconnect())
      setToken("")
    } catch {
      setError("Boat could not be disconnected. Try again.")
    } finally {
      setBusy(false)
    }
  }
  const unlock = async () => {
    if (await ensure()) setRound((value) => value + 1)
  }
  return {
    gateway,
    status,
    unread: status === undefined && (!gateway || readFailed),
    token,
    setToken,
    busy,
    error,
    save,
    disconnect,
    unlock,
  }
}

/** The token form, the VM lifetime, and any error: Boat's settings wherever they are shown. */
export function BoatConnectionFields({ connection }: { connection: BoatConnection }) {
  const tokenId = useId()
  const { gateway, status, token, setToken, busy, error } = connection
  return (
    <div className="settings-boat">
      {!gateway && (
        <p role="status">Connect this window to a Nessa gateway to manage Boat.</p>
      )}
      <p>
        Connect ChatGPT or Claude in{" "}
        <a href="https://boat.dev/dashboard?tab=agents" target="_blank" rel="noreferrer">
          Boat
        </a>{" "}
        once, then choose Boat when creating a channel.
      </p>
      {status?.state === "locked" ? (
        // Whether a token is saved is unknown until the vault opens: unlock,
        // never a form that would replace a token already there.
        <div className="settings-boat-fields">
          <button
            className="settings-button settings-button-primary"
            type="button"
            disabled={busy}
            onClick={() => void connection.unlock()}
            data-boat-action="unlock"
          >
            Unlock
          </button>
        </div>
      ) : (
        <form
          onSubmit={(event) => {
            event.preventDefault()
            void connection.save()
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
      )}
      <p className="settings-boat-note">
        The token is stored in Nessa’s encrypted vault. Disconnecting forgets it here;
        existing Boat VMs keep running until stopped in Boat.
      </p>
      <BoatRuntimeControl gateway={gateway} />
      {error && <p role="alert">{error}</p>}
    </div>
  )
}

/** "Connected" and its Disconnect, once Boat is connected. */
export function BoatConnected({ connection }: { connection: BoatConnection }) {
  if (connection.status?.state !== "connected") return null
  return (
    <span className="settings-boat-status" role="status">
      Connected{" "}
      <RowAction
        disabled={connection.busy}
        onClick={() => {
          void connection.disconnect()
        }}
      >
        Disconnect
      </RowAction>
    </span>
  )
}

export function BoatRow() {
  const connection = useBoatConnection()
  const entry = setting("boat")
  const found = useContext(FoundSetting) === "boat"
  return (
    <ItemRow
      unavailable={!connection.gateway}
      setting="boat"
      found={found}
      label={entry.label}
      detail={entry.detail}
      control={
        connection.status?.state === "connected" ? (
          <BoatConnected connection={connection} />
        ) : undefined
      }
    >
      <BoatConnectionFields connection={connection} />
    </ItemRow>
  )
}
