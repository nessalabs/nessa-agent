import {
  createContext,
  useContext,
  useEffect,
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
export function useBoatGateway() {
  return useContext(BoatContext)
}
/** What the Boat connection reads and does: its status, signing in, and disconnecting. */
export interface BoatConnection {
  readonly gateway: BoatGateway | undefined
  readonly status: BoatStatus | undefined
  /** No status will come: there is no gateway, or its read failed. */
  readonly unread: boolean
  readonly busy: boolean
  readonly error: string
  /** Start Sign in with Boat and open Boat's page in the browser. */
  readonly signIn: () => Promise<void>
  /** Stop waiting for the browser. */
  readonly cancelSignIn: () => Promise<void>
  /** Open Boat's page again, for a sign-in still waiting. */
  readonly reopen: () => void
  readonly disconnect: () => Promise<void>
  /** Asks for the vault, then reads the status again. */
  readonly unlock: () => Promise<void>
}

/** What the person reads when a sign-in ended without a connection. */
export function signInFailureText(cause: string): string {
  switch (cause) {
    case "denied":
      return "Sign-in was cancelled on Boat."
    case "expired":
      return "Boat didn't answer in time. Try signing in again."
    case "boat_worker_unavailable":
      return "This Nessa build needs a matching Boat worker image. Update Nessa and try again."
    case "boat_environment_blocks_credentials":
      return "The Nessa environment in Boat blocks agent credentials. Allow connected agents there, then retry."
    default:
      return "Boat couldn't finish signing in. Try again."
  }
}

/**
 * The gateway holds the sign-in; the window only starts it, opens Boat's
 * page, and reads where it stands: every `pollMs` while it waits for the
 * browser, and when the window comes back to the front. `onChange` hears
 * each status a sign-in or disconnect answers; a change of `seen`, what
 * another read last said about the connection, reads it again.
 */
export function useBoatConnection(
  onChange?: (status: BoatStatus) => void,
  seen?: unknown,
): BoatConnection {
  const gateway = useContext(BoatContext)
  const ensure = useVaultUnlock()
  const reads = useRef(0)
  const [status, setStatus] = useState<BoatStatus>()
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState("")
  const [readFailed, setReadFailed] = useState(false)
  const [round, setRound] = useState(0)
  const heard = useRef(onChange)
  heard.current = onChange
  const waiting = status?.signIn?.state === "waiting"
  useEffect(() => {
    let current = true
    const revision = ++reads.current
    setReadFailed(false)
    void gateway
      ?.read()
      .then((value) => {
        if (!current || revision !== reads.current) return
        setStatus((before) => {
          // A sign-in that just finished is news to the tab around it.
          if (before?.state !== value.state) heard.current?.(value)
          return value
        })
        if (value.signIn?.state === "failed")
          setError(signInFailureText(value.signIn.cause ?? ""))
        else if (value.state === "connected") setError("")
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
  useEffect(() => {
    if (!gateway || !waiting) return
    const again = () => setRound((value) => value + 1)
    const timer = window.setInterval(again, gateway.pollMs)
    window.addEventListener("focus", again)
    return () => {
      window.clearInterval(timer)
      window.removeEventListener("focus", again)
    }
  }, [gateway, waiting])
  const answered = (value: BoatStatus) => {
    setStatus(value)
    onChange?.(value)
  }
  const act = async (work: (gateway: BoatGateway) => Promise<void>, failed: string) => {
    if (!gateway || busy) return
    reads.current += 1
    setBusy(true)
    setError("")
    try {
      if (!(await ensure())) return
      await work(gateway)
    } catch (cause) {
      setError(
        cause instanceof NessaBoatError && cause.refusal === "boat_sign_in_unavailable"
          ? "This Nessa build can't sign in to Boat."
          : failed,
      )
    } finally {
      setBusy(false)
    }
  }
  const signIn = () =>
    act(async (gateway) => {
      const value = await gateway.signIn()
      answered(value)
      if (value.signIn?.state === "waiting" && value.signIn.url)
        gateway.open(value.signIn.url)
    }, "Boat sign-in could not start. Try again.")
  const cancelSignIn = () =>
    act(async (gateway) => answered(await gateway.cancelSignIn()), "Try again.")
  const disconnect = () =>
    act(
      async (gateway) => answered(await gateway.disconnect()),
      "Boat could not be disconnected. Try again.",
    )
  const reopen = () => {
    const url = status?.signIn?.state === "waiting" ? status.signIn.url : undefined
    if (gateway && url) gateway.open(url)
  }
  const unlock = async () => {
    if (await ensure()) setRound((value) => value + 1)
  }
  return {
    gateway,
    status,
    unread: status === undefined && (!gateway || readFailed),
    busy,
    error,
    signIn,
    cancelSignIn,
    reopen,
    disconnect,
    unlock,
  }
}

/** Sign in with Boat and any error: Boat's settings wherever they are shown. */
export function BoatConnectionFields({ connection }: { connection: BoatConnection }) {
  return (
    <div className="settings-boat">
      <BoatSignIn connection={connection} />
      {connection.error && <p role="alert">{connection.error}</p>}
    </div>
  )
}

/**
 * Unlock, or Sign in with Boat: the one way Boat is connected, in Settings
 * and wherever Boat is offered while it is not connected. While the
 * browser is out, it says so and offers the page again and Cancel.
 */
export function BoatSignIn({ connection }: { connection: BoatConnection }) {
  const { gateway, status, busy } = connection
  if (!gateway) return <p role="status">Connect a gateway to manage Boat.</p>
  if (status?.state === "locked")
    return (
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
    )
  if (status?.state === "connected") return null
  if (status?.signInAvailable === false)
    return <p role="status">This Nessa build can't sign in to Boat yet.</p>
  if (status?.signIn?.state === "waiting")
    return (
      <div className="settings-boat-waiting" data-boat-sign-in="waiting">
        <p role="status">Finish signing in to Boat in your browser.</p>
        <div className="settings-boat-fields">
          <button
            className="settings-button"
            type="button"
            onClick={connection.reopen}
            data-boat-action="reopen"
          >
            Open Boat again
          </button>
          <button
            className="settings-button"
            type="button"
            disabled={busy}
            onClick={() => void connection.cancelSignIn()}
            data-boat-action="cancel-sign-in"
          >
            Cancel
          </button>
        </div>
      </div>
    )
  return (
    <div className="settings-boat-fields">
      <button
        className="settings-button settings-button-primary"
        type="button"
        disabled={busy || status === undefined}
        onClick={() => void connection.signIn()}
        data-boat-action="sign-in"
      >
        {busy ? "Opening Boat…" : "Sign in with Boat"}
      </button>
    </div>
  )
}

/**
 * Settings › Connections › Accounts › Boat: one word of where it stands and
 * one button. Sign in with Boat starts at once; while the browser is out the
 * row says so and offers the page again and Cancel.
 */
export function BoatRow({
  onChange,
}: {
  /** Hears each state the row comes to read, so the tab around it can follow. */
  onChange?: (state: BoatStatus["state"]) => void
} = {}) {
  const connection = useBoatConnection()
  const entry = setting("boat")
  const found = useContext(FoundSetting) === "boat"
  const { status } = connection
  const state = status?.state
  const waiting = status?.signIn?.state === "waiting"
  useEffect(() => {
    if (state !== undefined) onChange?.(state)
  }, [state, onChange])
  return (
    <ItemRow
      unavailable={!connection.gateway}
      setting="boat"
      found={found}
      label={entry.label}
      data-boat-state={state ?? (connection.unread ? "unread" : "reading")}
      data-boat-sign-in={waiting ? "waiting" : undefined}
      detail={
        connection.error
          ? connection.error
          : state === "connected"
            ? status?.login
              ? `Connected as ${status.login}`
              : "Connected"
            : state === "locked"
              ? "Locked"
              : state === undefined
                ? connection.unread
                  ? undefined
                  : "Loading…"
                : waiting
                  ? "Finish signing in in your browser"
                  : "Not connected"
      }
      control={
        state === "connected" ? (
          <RowAction
            disabled={connection.busy}
            onClick={() => void connection.disconnect()}
            data-boat-action="disconnect"
          >
            Disconnect
          </RowAction>
        ) : state === "locked" ? (
          <RowAction
            disabled={connection.busy}
            onClick={() => void connection.unlock()}
            data-boat-action="unlock"
          >
            Unlock
          </RowAction>
        ) : waiting ? (
          <span className="settings-boat-status">
            <RowAction onClick={connection.reopen} data-boat-action="reopen">
              Open Boat
            </RowAction>
            <RowAction
              disabled={connection.busy}
              onClick={() => void connection.cancelSignIn()}
              data-boat-action="cancel-sign-in"
            >
              Cancel
            </RowAction>
          </span>
        ) : status?.signInAvailable === false ? null : (
          <RowAction
            disabled={connection.busy || state === undefined}
            onClick={() => void connection.signIn()}
            data-boat-action="sign-in"
          >
            Sign in with Boat
          </RowAction>
        )
      }
    />
  )
}
