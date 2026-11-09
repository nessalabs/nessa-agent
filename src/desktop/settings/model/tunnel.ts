/**
 * Settings › Linked devices › Web tunnel: the gateway's tunnel, which lets a
 * paired phone reach it from anywhere (`docs/design/auth/tunnel.md`). One
 * reducer, as the linking switch beside it has: the tab sends the one
 * request the state names, and an answer for a request since replaced
 * changes nothing.
 *
 * Nothing is shown optimistically. A change reads again, and that read is
 * what is shown. A change the gateway applies by restarting holds the switch
 * at its new value until the gateway that comes back answers a read, as the
 * Native linking switch does. The token is typed here and sent once; no read
 * ever brings it back, only whether one is saved.
 */
import { noticeText, sentences as linkingSentences } from "./linked-devices"
import type { FailureKind, LinkingApply, Outcome, Switching } from "./linked-devices"

/** How the tunnel is carried: a free quick address, or the person's own hostname. */
export type TunnelMode = "quick" | "named"

/** Where the running gateway's tunnel stands. */
export type TunnelStatus = "off" | "starting" | "ready" | "failed"

/** Why the running tunnel's last attempt failed, in the window's words. */
export type TunnelFailure =
  | "unsupported"
  | "download"
  | "unverified"
  | "ingress"
  | "storage"
  | "token"
  | "start"
  | "notReady"
  | "other"

/** What `pairing.tunnel` answered: the saved setting and the running tunnel. */
export interface TunnelRead {
  readonly enabled: boolean
  readonly mode: TunnelMode
  readonly hostname?: string
  readonly hasToken: boolean
  /** The loopback address a named tunnel's hostname must point to, host:port. */
  readonly ingress: string
  readonly status: TunnelStatus
  readonly address?: string
  readonly failure?: TunnelFailure
}

/** A change to send. A field left out keeps what is saved. */
export interface TunnelChange {
  readonly enabled: boolean
  readonly mode?: TunnelMode
  readonly hostname?: string
  readonly token?: string
}

/** A change the gateway saved. */
export interface TunnelSaved {
  readonly enabled: boolean
  readonly mode: TunnelMode
  readonly applies: LinkingApply
}

/** What the person has typed or picked and not yet sent. */
export interface TunnelDraft {
  readonly mode: TunnelMode
  readonly hostname: string
  readonly token: string
}

export type TunnelPending =
  | { readonly kind: "read"; readonly seq: number }
  | { readonly kind: "set"; readonly seq: number; readonly change: TunnelChange }

/** What the tunnel's last change said, shown under it. */
export type TunnelNoticeKind =
  FailureKind | "restarting" | "stalled" | "unconfirmed" | "tunnelBusy"

export interface TunnelState {
  readonly connection: "connecting" | "connected" | "unreachable"
  /** The last read that answered; `null` before one has. */
  readonly read: TunnelRead | null
  readonly draft: TunnelDraft
  readonly pending: TunnelPending | null
  readonly seq: number
  /** Set while the gateway starts again to apply a change. */
  readonly switching: Switching | null
  readonly notice: TunnelNoticeKind | null
}

export type TunnelEvent =
  | { readonly type: "connected" }
  | { readonly type: "unreachable" }
  | {
      readonly type: "answered"
      readonly seq: number
      readonly outcome: Outcome<TunnelRead> | Outcome<TunnelSaved>
    }
  /** The person flipped the Web tunnel switch. */
  | { readonly type: "toggle"; readonly enabled: boolean }
  | { readonly type: "mode"; readonly mode: TunnelMode }
  | { readonly type: "hostname"; readonly hostname: string }
  | { readonly type: "token"; readonly token: string }
  /** Apply the draft to a tunnel that is on. */
  | { readonly type: "apply" }
  | { readonly type: "poll" }
  /** A restart has not been seen through within its wait. */
  | { readonly type: "switchStalled" }

const emptyDraft: TunnelDraft = { mode: "quick", hostname: "", token: "" }

export function initialTunnelState(): TunnelState {
  return {
    connection: "connecting",
    read: null,
    draft: emptyDraft,
    pending: null,
    seq: 0,
    switching: null,
    notice: null,
  }
}

/* ——— What the window says. None of these sentences names a wire code. ——— */

export const tunnelSentences = {
  title: "Web tunnel",
  checking: "Checking the tunnel.",
  off: "Your phone reaches this gateway only on the same network.",
  savedOn: "The tunnel is on and starts with the gateway.",
  starting: "Starting the tunnel.",
  ready: "Your paired phone reaches this gateway from anywhere.",
  retried: "It is tried again.",
  address: "Address",
  mode: "Kind",
  quick: "Quick",
  named: "Named",
  quickNote:
    "A free address from Cloudflare, with no account. It changes whenever cloudflared has to start again; a gateway restart keeps it.",
  namedNote:
    "Your own hostname, from a tunnel in your Cloudflare account. Its address never changes.",
  hostname: "Hostname",
  hostnamePlaceholder: "https://nessa.example.com",
  token: "Token",
  tokenPlaceholder: "Paste the tunnel's token from Cloudflare",
  tokenSavedPlaceholder: "Saved. Paste a new one to replace it",
  route: (ingress: string) => `In Cloudflare, route this hostname to http://${ingress}.`,
  apply: "Apply",
  unapplied: "Not applied yet",
  restarting: "Restarting the gateway to apply this.",
  stalled:
    "The gateway has not come back yet. The change is saved and applies when it starts.",
  unconfirmed:
    "The gateway has not answered yet. The switch shows where the tunnel stood when it last did.",
  tunnelBusy: "The gateway is applying another change. Try again in a moment.",
} as const

/** Why a running tunnel failed. Total over {@link TunnelFailure}. */
export const tunnelFailureText: Record<TunnelFailure, string> = {
  unsupported: "There is no cloudflared build for this machine.",
  download: "cloudflared could not be downloaded.",
  unverified: "The cloudflared download did not match the build Nessa pins.",
  ingress: "Another program is using the tunnel's local port.",
  storage: "cloudflared could not be saved on this machine.",
  token: "The named tunnel's token could not be read. Save it again.",
  start: "cloudflared could not start.",
  notReady: "cloudflared started, but Cloudflare did not connect it.",
  other: "The tunnel could not start.",
}

/** What the notice says. Total over {@link TunnelNoticeKind}. */
export const tunnelNoticeText: Record<TunnelNoticeKind, string> = {
  ...noticeText,
  addressInUse:
    "The tunnel could not start: another program is using its local port. Nothing was changed.",
  auditUnavailable:
    "The gateway could not record this change. The switch shows where the tunnel stands now.",
  restarting: tunnelSentences.restarting,
  stalled: tunnelSentences.stalled,
  unconfirmed: tunnelSentences.unconfirmed,
  tunnelBusy: tunnelSentences.tunnelBusy,
}

/** The switch row's detail: where the saved setting and this run stand. */
export function tunnelDetail(state: TunnelState): string {
  const read = state.read
  if (!read)
    return state.notice ? tunnelNoticeText[state.notice] : tunnelSentences.checking
  if (!read.enabled) return tunnelSentences.off
  switch (read.status) {
    case "off":
      return tunnelSentences.savedOn
    case "starting":
      return tunnelSentences.starting
    case "ready":
      return tunnelSentences.ready
    case "failed":
      return `${tunnelFailureText[read.failure ?? "other"]} ${tunnelSentences.retried}`
  }
}

/** The footnote under the group: the last change's notice, else the mode's note. */
export function tunnelFootnote(state: TunnelState): string {
  if (state.notice && state.read) return tunnelNoticeText[state.notice]
  const note =
    state.draft.mode === "named" ? tunnelSentences.namedNote : tunnelSentences.quickNote
  return `${note} ${linkingSentences.restartNote}`
}

/* ——— What may be done now ——— */

/** A read answered, the gateway can be reached, nothing in flight, no restart under way. */
export function canChange(state: TunnelState): boolean {
  return (
    state.connection === "connected" &&
    state.read !== null &&
    state.pending === null &&
    state.switching === null
  )
}

/** A named tunnel needs a hostname and a token, typed or saved. */
function complete(state: TunnelState): boolean {
  if (state.draft.mode === "quick") return true
  const hostname = state.draft.hostname.trim() !== ""
  const token = state.draft.token.trim() !== "" || (state.read?.hasToken ?? false)
  return hostname && token
}

/** Whether the switch may be flipped now, either way. */
export function canToggle(state: TunnelState): boolean {
  if (!canChange(state)) return false
  return state.read?.enabled ? true : complete(state)
}

/**
 * The hostname typed, as the gateway takes it: Cloudflare's dashboard shows
 * a bare name (`nessa.example.com`), which is served over HTTPS.
 */
function originOf(hostname: string): string {
  const typed = hostname.trim()
  return typed === "" || typed.includes("://") ? typed : `https://${typed}`
}

/** Whether the draft differs from what is saved. */
export function dirty(state: TunnelState): boolean {
  const read = state.read
  if (!read) return false
  return (
    state.draft.mode !== read.mode ||
    originOf(state.draft.hostname) !== (read.hostname ?? "") ||
    state.draft.token.trim() !== ""
  )
}

/** Apply is offered for a tunnel that is on, with a complete draft that changed. */
export function canApply(state: TunnelState): boolean {
  return (
    canChange(state) && state.read?.enabled === true && dirty(state) && complete(state)
  )
}

/** Where the switch stands: the value a restart is moving to, else the gateway's. */
export function tunnelShown(state: TunnelState): boolean {
  return state.switching ? state.switching.enabled : state.read?.enabled === true
}

/** A tunnel that is on is read again now and then: its address appears, or moves. */
export function canPoll(state: TunnelState): boolean {
  return (
    state.connection === "connected" &&
    state.pending === null &&
    state.switching === null &&
    state.read?.enabled === true
  )
}

/* ——— The reducer ——— */

function begin(
  state: TunnelState,
  request:
    { readonly kind: "read" } | { readonly kind: "set"; readonly change: TunnelChange },
): TunnelState {
  const seq = state.seq + 1
  return { ...state, seq, pending: { ...request, seq } }
}

/** The draft, sent as a change that turns the tunnel on. */
function changeOf(draft: TunnelDraft): TunnelChange {
  if (draft.mode === "quick") return { enabled: true, mode: "quick" }
  const hostname = originOf(draft.hostname)
  const token = draft.token.trim()
  return {
    enabled: true,
    mode: "named",
    ...(hostname ? { hostname } : {}),
    ...(token ? { token } : {}),
  }
}

/** What a read says the draft is, when the person has nothing unsent. */
function draftOf(read: TunnelRead): TunnelDraft {
  return { mode: read.mode, hostname: read.hostname ?? "", token: "" }
}

function answeredRead(before: TunnelState, outcome: Outcome<TunnelRead>): TunnelState {
  // A read after the restart's drop answers for the new gateway; one before
  // it still answers for the old one, as the linking switch's does.
  const state: TunnelState = before.switching?.dropped
    ? { ...before, switching: null, notice: null }
    : outcome.ok && before.switching === null && before.notice === "unconfirmed"
      ? { ...before, notice: null }
      : before
  const settled = { ...state, pending: null }
  if (!outcome.ok)
    return state.switching ? settled : { ...settled, notice: outcome.failure.kind }
  // What the person is typing stays; a draft with nothing unsent follows
  // the gateway.
  const untouched = !dirty(state)
  return {
    ...settled,
    read: outcome.value,
    draft: untouched ? draftOf(outcome.value) : state.draft,
  }
}

function answeredSet(
  state: TunnelState,
  pending: Extract<TunnelPending, { kind: "set" }>,
  outcome: Outcome<TunnelSaved>,
): TunnelState {
  if (!outcome.ok && outcome.failure.kind === "unanswered")
    return {
      ...state,
      pending: null,
      switching: {
        enabled: pending.change.enabled,
        dropped: state.connection === "unreachable",
        confirmed: false,
      },
      notice: null,
    }
  if (!outcome.ok)
    return begin(
      {
        ...state,
        pending: null,
        // Busy here is another change or a restart.
        notice: outcome.failure.kind === "busy" ? "tunnelBusy" : outcome.failure.kind,
      },
      { kind: "read" },
    )
  // Saved: the token typed is the gateway's now, and the draft follows the
  // next read.
  const sent = { ...state, pending: null, draft: { ...state.draft, token: "" } }
  if (outcome.value.applies === "restarting")
    return {
      ...sent,
      switching: { enabled: pending.change.enabled, dropped: false, confirmed: true },
      notice: "restarting",
    }
  return begin({ ...sent, notice: null }, { kind: "read" })
}

export function tunnelReducer(state: TunnelState, event: TunnelEvent): TunnelState {
  switch (event.type) {
    case "connected": {
      // A gateway back after a stalled restart has its own answer to give.
      const back =
        state.connection === "unreachable" &&
        (state.notice === "stalled" || state.notice === "unconfirmed")
      const next: TunnelState = {
        ...state,
        connection: "connected",
        notice: back ? null : state.notice,
      }
      return next.pending === null ? begin(next, { kind: "read" }) : next
    }
    case "unreachable":
      return {
        ...state,
        connection: "unreachable",
        switching: state.switching && { ...state.switching, dropped: true },
      }
    case "answered": {
      const pending = state.pending
      if (!pending || pending.seq !== event.seq) return state
      return pending.kind === "read"
        ? answeredRead(state, event.outcome as Outcome<TunnelRead>)
        : answeredSet(state, pending, event.outcome as Outcome<TunnelSaved>)
    }
    case "toggle": {
      if (!canToggle(state) || event.enabled === state.read?.enabled) return state
      const change = event.enabled ? changeOf(state.draft) : { enabled: false }
      return begin({ ...state, notice: null }, { kind: "set", change })
    }
    case "mode":
      return { ...state, draft: { ...state.draft, mode: event.mode } }
    case "hostname":
      return { ...state, draft: { ...state.draft, hostname: event.hostname } }
    case "token":
      return { ...state, draft: { ...state.draft, token: event.token } }
    case "apply":
      if (!canApply(state)) return state
      return begin(
        { ...state, notice: null },
        { kind: "set", change: changeOf(state.draft) },
      )
    case "poll":
      if (!canPoll(state)) return state
      return begin(state, { kind: "read" })
    case "switchStalled": {
      if (state.switching === null) return state
      if (
        state.switching.dropped &&
        state.connection === "connected" &&
        state.pending?.kind === "read"
      )
        return state
      const stalled: TunnelState = {
        ...state,
        switching: null,
        notice: state.switching.confirmed ? "stalled" : "unconfirmed",
      }
      return stalled.connection === "connected" && stalled.pending === null
        ? begin(stalled, { kind: "read" })
        : stalled
    }
  }
}
