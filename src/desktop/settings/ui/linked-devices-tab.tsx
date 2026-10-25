/**
 * Settings › Connections › Linked devices (#462). The rules are the
 * reducer's (`model/linked-devices.ts`); this draws its state and sends the
 * one request it names. The code, the fingerprint and the orbs are the UI
 * kit's: this screen only wires them to `pairing.*` and says what went wrong.
 * With no gateway — the sample preview — the page stays pending.
 *
 * Pair a device is one row; the code it opens is drawn in a modal, beside
 * the one signal orb that carries the same 8-character code. The phone's
 * scanner, and how a phone finds the gateway, are not this screen.
 */
import {
  createContext,
  useContext,
  useEffect,
  useReducer,
  useRef,
  useState,
  type ReactNode,
} from "react"
import { Dialog } from "@nessa-ui/react/dialog"
import { KeyFingerprint } from "@nessa-ui/react/key-fingerprint"
import { PairingCode } from "@nessa-ui/react/pairing-code"
import { SettingsGroup, SettingsRow } from "@nessa-ui/react/settings-group"
import { SignalOrb } from "@nessa-ui/react/signal-orb"
import { Switch } from "@nessa-ui/react/switch"
import { linkedOn } from "../../model/linked-device"
import type { LinkedDevicesGateway } from "../adapters/linked-devices-gateway"
import {
  approvable,
  canAct,
  canPair,
  canPoll,
  canReplace,
  canToggle,
  initialLinkedDevicesState,
  linkedDevicesReducer,
  linkingNoticeText,
  linkingShown,
  noticeText,
  sentences,
  type LinkedDevice,
  type LinkedDevicesEvent,
  type LinkedDevicesState,
  type OpenCode,
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

const orbSize = 176

/**
 * The gateway's code is eight symbols with a hyphen between the groups
 * (`ABCD-2345`). PairingCode groups those eight itself, and the orb carries
 * only them.
 */
function pairingSymbols(code: string): string {
  return code.length === 9 && code[4] === "-"
    ? `${code.slice(0, 4)}${code.slice(5)}`
    : code
}

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
        data-title="hidden"
      >
        <h2>Linked devices</h2>
        <p className="settings-nothing">{sentences.unavailableHere}</p>
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
    case "setLinking":
      return gateway.setLinking(pending.enabled)
  }
}

/**
 * While nothing is in flight, read again: linking is on, or the first read
 * failed and linking is still unknown. Off and refused are answers, not
 * pauses. The same rule is `canPoll`.
 */
function usePoll(
  gateway: LinkedDevicesGateway,
  state: LinkedDevicesState,
  dispatch: Dispatch,
) {
  const quiet = gateway.pollMs > 0 && canPoll(state)
  useEffect(() => {
    if (!quiet) return
    const timer = window.setTimeout(() => dispatch({ type: "poll" }), gateway.pollMs)
    return () => window.clearTimeout(timer)
  }, [quiet, gateway, dispatch])
}

/**
 * A restart the window has not seen through within `switchWaitMs` stops
 * holding the switch, and says so: a gateway that never comes back must not
 * leave "Restarting" on screen.
 */
function useSwitchWait(
  gateway: LinkedDevicesGateway,
  state: LinkedDevicesState,
  dispatch: Dispatch,
) {
  const switching = state.switching !== null
  useEffect(() => {
    if (!switching) return
    const timer = window.setTimeout(
      () => dispatch({ type: "switchStalled" }),
      gateway.switchWaitMs,
    )
    return () => window.clearTimeout(timer)
  }, [switching, gateway, dispatch])
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
  useSwitchWait(gateway, state, dispatch)
  // The invitation whose code the person closed the modal on. The code stays
  // open on the gateway; the row offers it again. A new code opens the modal.
  const [closed, setClosed] = useState<string | null>(null)
  const shown =
    state.code !== null && state.code.invitationId !== closed ? state.code : null
  const phase =
    state.connection === "unreachable" && state.linking === "unknown"
      ? "unreachable"
      : state.linking === "unknown"
        ? "checking"
        : state.linking
  return (
    <div
      className="settings-group linked-devices"
      data-setting="linked-devices"
      data-found={found || undefined}
      data-linked={phase}
      data-linking={state.linking}
      data-connection={state.connection}
      aria-busy={state.pending !== null || undefined}
    >
      <LinkingGroup state={state} dispatch={dispatch} />
      {state.linking === "on" ? (
        <>
          <PairingGroup
            state={state}
            dispatch={dispatch}
            onShow={() => setClosed(null)}
          />
          <PairingDialog
            state={state}
            dispatch={dispatch}
            shown={shown}
            onClose={() => setClosed(state.code?.invitationId ?? null)}
          />
          <WaitingGroup
            state={state}
            dispatch={dispatch}
            inDialog={shown?.invitationId ?? null}
          />
          <DevicesGroup state={state} dispatch={dispatch} />
        </>
      ) : null}
      {state.switching !== null ? null : state.notice &&
        state.linking !== "off" &&
        state.linking !== "refused" &&
        state.linking !== "unknown" ? (
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

/**
 * The Native linking switch. Flipping it saves the choice on the gateway,
 * which restarts itself to apply it; the switch holds the new value until
 * the gateway that comes back says where it stands. No group title: the
 * row's own label names it.
 */
function LinkingGroup({
  state,
  dispatch,
}: {
  state: LinkedDevicesState
  dispatch: Dispatch
}) {
  const detail =
    state.linking === "unknown"
      ? state.notice
        ? noticeText[state.notice.kind]
        : sentences.checking
      : state.linking === "off"
        ? sentences.notConfigured
        : state.linking === "refused"
          ? sentences.refused
          : sentences.on
  const footnote = state.linkingNotice
    ? linkingNoticeText[state.linkingNotice]
    : state.linking === "on" || state.linking === "off"
      ? sentences.restartNote
      : undefined
  return (
    <SettingsGroup footnote={footnote} data-linking-switch>
      <SettingsRow
        label="Native linking"
        detail={detail}
        control={
          <Switch
            checked={linkingShown(state)}
            disabled={!canToggle(state)}
            aria-label="Native linking"
            onCheckedChange={(enabled) => dispatch({ type: "setLinking", enabled })}
          />
        }
      />
    </SettingsGroup>
  )
}

function PairingGroup({
  state,
  dispatch,
  onShow,
}: {
  state: LinkedDevicesState
  dispatch: Dispatch
  onShow: () => void
}) {
  const busy = state.pending !== null
  const open = state.code?.invitationId ?? state.hiddenInvitation
  return (
    <SettingsGroup>
      <SettingsRow
        label={sentences.pair}
        detail={
          state.code
            ? sentences.codeOpen
            : state.hiddenInvitation
              ? sentences.codeHidden
              : sentences.pairDetail
        }
        data-pairing-row
        control={
          open === null ? (
            <button
              type="button"
              className="settings-button settings-button-primary"
              data-linked-action="pair"
              disabled={!canPair(state) || busy}
              onClick={() => dispatch({ type: "pair" })}
            >
              {sentences.pair}
            </button>
          ) : (
            <span className="linked-actions">
              {state.code ? (
                <button
                  type="button"
                  className="settings-button settings-button-primary"
                  data-linked-action="show"
                  onClick={onShow}
                >
                  {sentences.showCode}
                </button>
              ) : (
                <button
                  type="button"
                  className="settings-button"
                  data-linked-action="refresh"
                  data-invitation={open}
                  disabled={!canReplace(state)}
                  onClick={() => dispatch({ type: "refresh" })}
                >
                  {sentences.refresh}
                </button>
              )}
              <button
                type="button"
                className="settings-button"
                data-linked-action="cancel"
                data-invitation={open}
                disabled={!canReplace(state)}
                onClick={() => dispatch({ type: "cancel" })}
              >
                {sentences.cancel}
              </button>
            </span>
          )
        }
      />
    </SettingsGroup>
  )
}

/**
 * The code Pair a device opened, in a modal: the one signal orb on the left,
 * live, and the code with its clock and actions beside it. Once a device
 * uses the code, its approval is asked here too, so pairing ends where it
 * began. Closing the modal leaves the code open; the row shows it again.
 */
function PairingDialog({
  state,
  dispatch,
  shown,
  onClose,
}: {
  state: LinkedDevicesState
  dispatch: Dispatch
  shown: OpenCode | null
  onClose: () => void
}) {
  const symbols = shown ? pairingSymbols(shown.code) : ""
  const waiting = shown
    ? state.waiting.filter((device) => device.invitationId === shown.invitationId)
    : []
  return (
    <Dialog
      open={shown !== null}
      onOpenChange={(next) => {
        if (!next) onClose()
      }}
      title={sentences.pairingTitle}
      description={sentences.pairingDetail}
      size="lg"
      className="linked-pairing-dialog"
    >
      {shown ? (
        <div className="linked-pairing" data-linked-pairing data-state={shown.state}>
          {shown.state === "open" ? (
            <SignalOrb code={symbols} size={orbSize} aria-label="Signal orb" />
          ) : null}
          <div className="linked-pairing-code">
            <PairingCode
              code={symbols}
              expiresAt={shown.expiresAtMs}
              state={shown.state}
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
          </div>
          {waiting.length > 0 ? (
            <SettingsGroup className="linked-pairing-waiting">
              {waiting.map((device) => (
                <WaitingRow
                  key={device.invitationId}
                  device={device}
                  state={state}
                  dispatch={dispatch}
                />
              ))}
            </SettingsGroup>
          ) : null}
        </div>
      ) : null}
    </Dialog>
  )
}

/** Devices waiting for approval, less the one the pairing modal asks about. */
function WaitingGroup({
  state,
  dispatch,
  inDialog,
}: {
  state: LinkedDevicesState
  dispatch: Dispatch
  inDialog: string | null
}) {
  const waiting = state.waiting.filter((device) => device.invitationId !== inDialog)
  if (waiting.length === 0) return null
  return (
    <SettingsGroup title="Waiting for approval">
      {waiting.map((device) => (
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
  const ready = canAct(state)
  const mayApprove = approvable(device) && ready
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
            disabled={!ready}
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
    <SettingsGroup title="Your devices">
      {state.devices.length === 0 ? (
        <SettingsRow label={sentences.empty} />
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
  const ready = canAct(state)
  const issued = linkedOn(device.issuedAt)
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
              disabled={!ready}
              onClick={() => dispatch({ type: "confirmRevoke" })}
            >
              {sentences.revoke}
            </button>
            <button
              type="button"
              className="settings-button"
              data-linked-action="cancel-revoke"
              disabled={state.pending !== null}
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
            disabled={!ready || state.confirmRevoke !== null}
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
