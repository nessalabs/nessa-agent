/**
 * Settings › Connections › Linked devices (#462). The rules are the
 * reducer's (`model/linked-devices.ts`); this draws its state and sends the
 * one request it names. The code, the fingerprint and the orbs are the UI
 * kit's: this screen only wires them to `pairing.*` and says what went wrong.
 * With no gateway — the sample preview — the page stays pending.
 *
 * The phone's scanner, and how a phone finds the gateway, are not this
 * screen. The orbs carry the same 8-character code the gateway showed.
 */
import {
  createContext,
  useContext,
  useEffect,
  useReducer,
  useRef,
  type ReactNode,
} from "react"
import { KeyFingerprint } from "@nessa-ui/react/key-fingerprint"
import { PairingCode } from "@nessa-ui/react/pairing-code"
import { QrOrb } from "@nessa-ui/react/qr-orb"
import { SettingsGroup, SettingsRow } from "@nessa-ui/react/settings-group"
import { SignalOrb } from "@nessa-ui/react/signal-orb"
import { Switch } from "@nessa-ui/react/switch"
import type { LinkedDevicesGateway } from "../adapters/linked-devices-gateway"
import {
  approvable,
  canPair,
  canReplace,
  initialLinkedDevicesState,
  linkedDevicesReducer,
  noticeText,
  sentences,
  type LinkedDevice,
  type LinkedDevicesEvent,
  type LinkedDevicesState,
  type WaitingDevice,
} from "../model/linked-devices"
import { FoundSetting } from "./settings-controls"

/** The window's pairing gateway, given by composition (`main.tsx`); none without one. */
const LinkedDevicesContext = createContext<LinkedDevicesGateway | undefined>(undefined)

export function LinkedDevicesProvider({
  gateway,
  children,
}: {
  gateway: LinkedDevicesGateway | undefined
  children: ReactNode
}) {
  return (
    <LinkedDevicesContext.Provider value={gateway}>
      {children}
    </LinkedDevicesContext.Provider>
  )
}

const orbSize = 168

export function LinkedDevicesTab() {
  const gateway = useContext(LinkedDevicesContext)
  const found = useContext(FoundSetting) === "linked-devices"
  if (!gateway)
    return (
      <section
        className="settings-group"
        data-setting="linked-devices"
        data-pending
        data-found={found || undefined}
        data-linked="pending"
      >
        <h2>Linked devices</h2>
        <p className="settings-footnote">{sentences.unavailableHere}</p>
      </section>
    )
  return <ManagedDevices gateway={gateway} found={found} />
}

type Dispatch = (event: LinkedDevicesEvent) => void

/** Sends the request the state names, once for each `seq`. */
function useRequests(
  gateway: LinkedDevicesGateway,
  state: LinkedDevicesState,
  dispatch: Dispatch,
) {
  const sent = useRef(0)
  const pending = state.pending
  useEffect(() => {
    if (!pending || sent.current === pending.seq) return
    sent.current = pending.seq
    const answer = requestOf(gateway, pending)
    void answer.then((outcome) =>
      dispatch({ type: "answered", seq: pending.seq, outcome }),
    )
  }, [gateway, pending, dispatch])
}

function requestOf(
  gateway: LinkedDevicesGateway,
  pending: NonNullable<LinkedDevicesState["pending"]>,
) {
  switch (pending.kind) {
    case "read":
      return gateway.read()
    case "create":
      return gateway.create()
    case "cancel":
      return gateway.cancel(pending.invitationId)
    case "refresh":
      return gateway.refresh(pending.invitationId)
    case "approve":
      return gateway.approve(pending.invitationId, pending.deviceKey)
    case "deny":
      return gateway.deny(pending.invitationId)
    case "status":
      return gateway.status(pending.invitationId)
    case "revoke":
      return gateway.revoke(pending.credentialId)
  }
}

/** While linking is on and nothing is in flight, read again. */
function usePoll(
  gateway: LinkedDevicesGateway,
  state: LinkedDevicesState,
  dispatch: Dispatch,
) {
  const quiet =
    gateway.pollMs > 0 &&
    state.connection === "connected" &&
    state.linking === "on" &&
    state.pending === null &&
    state.confirmRevoke === null
  useEffect(() => {
    if (!quiet) return
    const timer = window.setTimeout(() => dispatch({ type: "poll" }), gateway.pollMs)
    return () => window.clearTimeout(timer)
  }, [quiet, gateway, dispatch])
}

/**
 * Puts focus on Cancel once a code is shown and Cancel can be used. The
 * read that follows create leaves Cancel resting until it returns, and a
 * disabled button does not take focus.
 */
function useCodeFocus(state: LinkedDevicesState) {
  const container = useRef<HTMLDivElement>(null)
  const aimed = useRef(false)
  useEffect(() => {
    if (state.code === null) {
      aimed.current = false
      return
    }
    if (aimed.current) return
    const button = container.current?.querySelector<HTMLButtonElement>(
      '[data-linked-action="cancel"]',
    )
    if (!button || button.disabled) return
    button.focus()
    aimed.current = true
  })
  return container
}

function ManagedDevices({
  gateway,
  found,
}: {
  gateway: LinkedDevicesGateway
  found: boolean
}) {
  const [state, dispatch] = useReducer(
    linkedDevicesReducer,
    undefined,
    initialLinkedDevicesState,
  )
  useEffect(() => gateway.follow(dispatch), [gateway])
  useRequests(gateway, state, dispatch)
  usePoll(gateway, state, dispatch)
  const container = useCodeFocus(state)
  const phase =
    state.connection === "unreachable" && state.linking === "unknown"
      ? "unreachable"
      : state.linking === "unknown"
        ? "checking"
        : state.linking
  return (
    <div
      ref={container}
      className="settings-group linked-devices"
      data-setting="linked-devices"
      data-found={found || undefined}
      data-linked={phase}
      data-linking={state.linking}
      data-connection={state.connection}
      aria-busy={state.pending !== null || undefined}
    >
      <LinkingGroup state={state} />
      {state.linking === "on" ? (
        <>
          <PairingGroup state={state} dispatch={dispatch} />
          <WaitingGroup state={state} dispatch={dispatch} />
          <DevicesGroup state={state} dispatch={dispatch} />
        </>
      ) : null}
      {state.notice && state.linking !== "off" && state.linking !== "refused" ? (
        <p className="settings-footnote" role="status" data-linked-notice>
          {state.connection === "unreachable" && state.notice.kind === "unreachable"
            ? sentences.unreachable
            : noticeText[state.notice.kind]}
        </p>
      ) : state.connection === "unreachable" ? (
        <p className="settings-footnote" role="status" data-linked-notice>
          {sentences.unreachable}
        </p>
      ) : null}
    </div>
  )
}

function LinkingGroup({ state }: { state: LinkedDevicesState }) {
  const on = state.linking === "on"
  const detail =
    state.linking === "unknown"
      ? sentences.checking
      : state.linking === "off"
        ? sentences.notConfigured
        : state.linking === "refused"
          ? sentences.refused
          : sentences.on
  const footnote =
    state.linking === "off"
      ? sentences.enable
      : state.linking === "on"
        ? sentences.disable
        : undefined
  return (
    <SettingsGroup title="Native linking" footnote={footnote}>
      <SettingsRow
        label="Native linking"
        detail={detail}
        control={<Switch checked={on} disabled aria-label="Native linking" />}
      />
    </SettingsGroup>
  )
}

function PairingGroup({
  state,
  dispatch,
}: {
  state: LinkedDevicesState
  dispatch: Dispatch
}) {
  const busy = state.pending !== null
  return (
    <SettingsGroup title="Pair a device">
      {state.code ? (
        <SettingsRow
          label="Pairing code"
          detail="Enter it on the device, or scan either orb."
        >
          <PairingCode
            code={state.code.code}
            expiresAt={state.code.expiresAtMs}
            state={state.code.state}
            onExpire={() => dispatch({ type: "expired" })}
            actions={
              <span className="linked-actions">
                <button
                  type="button"
                  className="settings-button"
                  data-linked-action="refresh"
                  disabled={!canReplace(state)}
                  onClick={() => dispatch({ type: "refresh" })}
                >
                  {sentences.refresh}
                </button>
                <button
                  type="button"
                  className="settings-button"
                  data-linked-action="cancel"
                  disabled={!canReplace(state)}
                  onClick={() => dispatch({ type: "cancel" })}
                >
                  {sentences.cancel}
                </button>
              </span>
            }
          />
          {state.code.state === "open" ? (
            <span className="linked-orbs">
              <SignalOrb
                code={state.code.code}
                size={orbSize}
                still
                aria-label="Signal orb"
              />
              <QrOrb value={state.code.code} size={orbSize} aria-label="QR orb" />
            </span>
          ) : null}
        </SettingsRow>
      ) : state.hiddenInvitation ? (
        <SettingsRow
          label="Pairing code"
          detail={sentences.codeHidden}
          control={
            <span className="linked-actions">
              <button
                type="button"
                className="settings-button"
                data-linked-action="refresh"
                data-invitation={state.hiddenInvitation}
                disabled={!canReplace(state)}
                onClick={() => dispatch({ type: "refresh" })}
              >
                {sentences.refresh}
              </button>
              <button
                type="button"
                className="settings-button"
                data-linked-action="cancel"
                data-invitation={state.hiddenInvitation}
                disabled={!canReplace(state)}
                onClick={() => dispatch({ type: "cancel" })}
              >
                {sentences.cancel}
              </button>
            </span>
          }
        />
      ) : (
        <SettingsRow
          label="Pair a device"
          detail="Shows a one-use code and two orbs. The code is shown once."
          control={
            <button
              type="button"
              className="settings-button settings-button-primary"
              data-linked-action="pair"
              disabled={!canPair(state) || busy}
              onClick={() => dispatch({ type: "pair" })}
            >
              {sentences.pair}
            </button>
          }
        />
      )}
    </SettingsGroup>
  )
}

function WaitingGroup({
  state,
  dispatch,
}: {
  state: LinkedDevicesState
  dispatch: Dispatch
}) {
  if (state.waiting.length === 0) return null
  return (
    <SettingsGroup title="Waiting for approval">
      {state.waiting.map((device) => (
        <WaitingRow
          key={device.invitationId}
          device={device}
          state={state}
          dispatch={dispatch}
        />
      ))}
    </SettingsGroup>
  )
}

function WaitingRow({
  device,
  state,
  dispatch,
}: {
  device: WaitingDevice
  state: LinkedDevicesState
  dispatch: Dispatch
}) {
  const busy = state.pending !== null
  const mayApprove = approvable(device) && !busy
  return (
    <SettingsRow
      label="Waiting device"
      detail={
        device.activation === "retryable"
          ? sentences.tryAgain
          : device.activation === "permanent"
            ? sentences.pairAgain
            : device.fingerprint
              ? "Compare this fingerprint with the one on the device."
              : sentences.fingerprintMissing
      }
      data-invitation={device.invitationId}
      control={
        <span className="linked-actions">
          {approvable(device) ? (
            <button
              type="button"
              className="settings-button settings-button-primary"
              data-linked-action="approve"
              disabled={!mayApprove}
              onClick={() =>
                dispatch({ type: "approve", invitationId: device.invitationId })
              }
            >
              {sentences.approve}
            </button>
          ) : null}
          <button
            type="button"
            className="settings-button"
            data-linked-action="deny"
            disabled={busy}
            onClick={() => dispatch({ type: "deny", invitationId: device.invitationId })}
          >
            {sentences.deny}
          </button>
        </span>
      }
    >
      {device.fingerprint ? <KeyFingerprint value={device.fingerprint} /> : null}
    </SettingsRow>
  )
}

function DevicesGroup({
  state,
  dispatch,
}: {
  state: LinkedDevicesState
  dispatch: Dispatch
}) {
  return (
    <SettingsGroup title="Linked devices">
      {state.devices.length === 0 ? (
        <SettingsRow label="Linked devices" detail={sentences.empty} />
      ) : (
        state.devices.map((device) => (
          <DeviceRow
            key={device.credentialId}
            device={device}
            state={state}
            dispatch={dispatch}
          />
        ))
      )}
    </SettingsGroup>
  )
}

function DeviceRow({
  device,
  state,
  dispatch,
}: {
  device: LinkedDevice
  state: LinkedDevicesState
  dispatch: Dispatch
}) {
  const confirming = state.confirmRevoke === device.credentialId
  const busy = state.pending !== null
  const issued = new Date(device.issuedAt * 1000).toISOString().slice(0, 10)
  return (
    <SettingsRow
      label="Linked device"
      detail={confirming ? sentences.revokeAsk : `Linked ${issued}`}
      data-credential={device.credentialId}
      control={
        confirming ? (
          <span className="linked-actions">
            <button
              type="button"
              className="settings-button settings-button-danger"
              data-linked-action="confirm-revoke"
              disabled={busy}
              onClick={() => dispatch({ type: "confirmRevoke" })}
            >
              {sentences.revoke}
            </button>
            <button
              type="button"
              className="settings-button"
              data-linked-action="cancel-revoke"
              disabled={busy}
              onClick={() => dispatch({ type: "cancelRevoke" })}
            >
              {sentences.keep}
            </button>
          </span>
        ) : (
          <button
            type="button"
            className="settings-button"
            data-linked-action="revoke"
            disabled={busy || state.confirmRevoke !== null}
            onClick={() =>
              dispatch({ type: "askRevoke", credentialId: device.credentialId })
            }
          >
            {sentences.revoke}
          </button>
        )
      }
    >
      {device.fingerprint ? <KeyFingerprint value={device.fingerprint} /> : null}
    </SettingsRow>
  )
}
