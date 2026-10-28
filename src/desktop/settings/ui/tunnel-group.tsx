/**
 * Settings › Linked devices › Web tunnel. The rules are the reducer's
 * (`model/tunnel.ts`); this draws its state and sends the one request it
 * names, through the same gateway as the rest of the tab. It is drawn only
 * while native linking is on, since the tunnel carries the native listener.
 *
 * A quick tunnel needs nothing typed. A named one takes the hostname the
 * person made in Cloudflare and its token; the token field is never filled
 * from the gateway, which keeps it and only says whether one is saved.
 */
import { useEffect, useId, useReducer, useRef } from "react"
import {
  SegmentedControl,
  SegmentedControlOption,
} from "@nessa-ui/react/segmented-control"
import { SettingsGroup, SettingsRow } from "@nessa-ui/react/settings-group"
import { Switch } from "@nessa-ui/react/switch"
import type { LinkedDevicesGateway } from "../adapters/linked-devices-gateway"
import {
  canApply,
  canChange,
  canPoll,
  canToggle,
  dirty,
  initialTunnelState,
  tunnelDetail,
  tunnelFootnote,
  tunnelReducer,
  tunnelSentences,
  tunnelShown,
  type TunnelEvent,
  type TunnelMode,
  type TunnelState,
} from "../model/tunnel"

type Dispatch = (event: TunnelEvent) => void

/** Sends the request the state names, once for each `seq`. */
function useRequests(
  gateway: LinkedDevicesGateway,
  state: TunnelState,
  dispatch: Dispatch,
) {
  const sent = useRef(0)
  const pending = state.pending
  useEffect(() => {
    if (!pending || sent.current === pending.seq) return
    sent.current = pending.seq
    const answer =
      pending.kind === "read" ? gateway.tunnel() : gateway.setTunnel(pending.change)
    void answer.then((outcome) =>
      dispatch({ type: "answered", seq: pending.seq, outcome }),
    )
  }, [gateway, pending, dispatch])
}

/** A tunnel that is on is read again while nothing is in flight: `canPoll`. */
function usePoll(gateway: LinkedDevicesGateway, state: TunnelState, dispatch: Dispatch) {
  const quiet = gateway.pollMs > 0 && canPoll(state)
  useEffect(() => {
    if (!quiet) return
    const timer = window.setTimeout(() => dispatch({ type: "poll" }), gateway.pollMs)
    return () => window.clearTimeout(timer)
  }, [quiet, gateway, dispatch])
}

/** A restart not seen through within `switchWaitMs` stops holding the switch. */
function useSwitchWait(
  gateway: LinkedDevicesGateway,
  state: TunnelState,
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

const modes: readonly { id: TunnelMode; label: string }[] = [
  { id: "quick", label: tunnelSentences.quick },
  { id: "named", label: tunnelSentences.named },
]

export function TunnelGroup({ gateway }: { gateway: LinkedDevicesGateway }) {
  const [state, dispatch] = useReducer(tunnelReducer, undefined, initialTunnelState)
  useEffect(() => gateway.follow(dispatch), [gateway])
  useRequests(gateway, state, dispatch)
  usePoll(gateway, state, dispatch)
  useSwitchWait(gateway, state, dispatch)
  const ids = { hostname: useId(), token: useId() }
  const read = state.read
  const editable = canChange(state)
  const named = state.draft.mode === "named"
  return (
    <SettingsGroup
      title={tunnelSentences.title}
      footnote={tunnelFootnote(state)}
      data-tunnel
      data-tunnel-status={read?.status ?? "unknown"}
      data-tunnel-enabled={read ? String(read.enabled) : undefined}
      aria-busy={state.pending !== null || undefined}
    >
      <SettingsRow
        label={tunnelSentences.title}
        detail={tunnelDetail(state)}
        pending={state.pending?.kind === "set" || state.switching !== null}
        control={
          <Switch
            checked={tunnelShown(state)}
            disabled={!canToggle(state)}
            aria-label={tunnelSentences.title}
            data-tunnel-switch
            onCheckedChange={(enabled) => dispatch({ type: "toggle", enabled })}
          />
        }
      />
      {read?.status === "ready" && read.address ? (
        <SettingsRow
          label={tunnelSentences.address}
          control={
            <span className="tunnel-address" data-tunnel-address>
              {read.address}
            </span>
          }
        />
      ) : null}
      <SettingsRow
        label={tunnelSentences.mode}
        control={
          <SegmentedControl
            variant="glass"
            className="settings-segmented"
            value={state.draft.mode}
            aria-label={tunnelSentences.mode}
            data-tunnel-mode
            onValueChange={(next) => {
              const mode = modes.find((each) => each.id === next)
              if (mode) dispatch({ type: "mode", mode: mode.id })
            }}
          >
            {modes.map((mode) => (
              <SegmentedControlOption key={mode.id} value={mode.id} disabled={!editable}>
                {mode.label}
              </SegmentedControlOption>
            ))}
          </SegmentedControl>
        }
      />
      {named ? (
        <>
          <SettingsRow
            label={tunnelSentences.hostname}
            detail={read ? tunnelSentences.route(read.ingress) : undefined}
            control={
              <input
                id={ids.hostname}
                className="settings-input tunnel-input"
                data-tunnel-field="hostname"
                value={state.draft.hostname}
                placeholder={tunnelSentences.hostnamePlaceholder}
                autoComplete="off"
                spellCheck={false}
                disabled={!editable}
                aria-label={tunnelSentences.hostname}
                onChange={(event) =>
                  dispatch({ type: "hostname", hostname: event.target.value })
                }
              />
            }
          />
          <SettingsRow
            label={tunnelSentences.token}
            control={
              <input
                id={ids.token}
                type="password"
                className="settings-input tunnel-input"
                data-tunnel-field="token"
                value={state.draft.token}
                placeholder={
                  read?.hasToken
                    ? tunnelSentences.tokenSavedPlaceholder
                    : tunnelSentences.tokenPlaceholder
                }
                autoComplete="off"
                spellCheck={false}
                disabled={!editable}
                aria-label={tunnelSentences.token}
                onChange={(event) =>
                  dispatch({ type: "token", token: event.target.value })
                }
              />
            }
          />
        </>
      ) : null}
      {read?.enabled && dirty(state) ? (
        <SettingsRow
          label={tunnelSentences.unapplied}
          control={
            <button
              type="button"
              className="settings-button settings-button-primary"
              data-tunnel-action="apply"
              disabled={!canApply(state)}
              onClick={() => dispatch({ type: "apply" })}
            >
              {tunnelSentences.apply}
            </button>
          }
        />
      ) : null}
    </SettingsGroup>
  )
}
