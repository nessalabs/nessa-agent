/**
 * Settings › Connections › Linked devices: pairing and revoking, as the
 * window manages them (#462). One reducer, from the desktop ordering table
 * in `docs/design/auth/device-pairing.md` (rows L1–L21); its tests are one
 * row at least one test, in `linked-devices.test.ts`.
 *
 * The window never retypes a gateway rule. Whether an enrollment may take a
 * step, and whether this sign-in may take it, is the gateway's answer, read
 * into the sentences here. Nothing is updated optimistically: a write that
 * may have changed what is open reads again, and that read is what is shown.
 *
 * The state names the one request in flight (`pending`). The tab sends it.
 * An answer comes back as `answered` with the request's `seq`, so an answer
 * for a request since replaced changes nothing. A poll read does not clear
 * what an action just said.
 */

/** Whether native linking is on, as `pairing.pending` last answered. */
export type Linking = "unknown" | "off" | "on" | "refused"

/** What the host knows about a code it was shown. Expiry is the clock's. */
export type CodeState = "open" | "used" | "cancelled"

/** A code `pairing.create` returned. It is shown once; a later read has none. */
export interface OpenCode {
  readonly invitationId: string
  readonly code: string
  readonly expiresAtMs: number
  readonly state: CodeState
}

/** An enrollment the owner can still approve or deny. */
export interface WaitingDevice {
  readonly invitationId: string
  readonly phase: "claimed" | "approved" | "staging"
  /** SHA-256 of the key's SubjectPublicKeyInfo, hex. Absent: Approve is withheld. */
  readonly fingerprint?: string
  /** The claimed key, hex, which approval must name. */
  readonly deviceKey?: string
  readonly credentialId?: string
  /** Set when approval committed and activation stopped. */
  readonly activation?: "retryable" | "permanent"
}

/**
 * A linked device: a non-revoked credential that is not this session's and
 * whose only grant is `conversation.read`. The wire's credential list has no
 * device marker, so a credential issued by hand with only that grant looks
 * the same. The fingerprint is joined from an enrollment that still names
 * the credential, which a finished one usually does not.
 */
export interface LinkedDevice {
  readonly credentialId: string
  readonly issuedAt: number
  readonly fingerprint?: string
}

/** Where one enrollment stands, after the adapter has hashed its key. */
export type EnrollmentPhase =
  "available" | "claimed" | "approved" | "staging" | "active" | "terminal"

export type TerminalCause = "expired" | "cancelled" | "denied" | "revoked" | "restarted"

export interface Enrollment {
  readonly invitationId: string
  readonly phase: EnrollmentPhase
  readonly expiresAtMs: number
  readonly fingerprint?: string
  readonly deviceKey?: string
  readonly credentialId?: string
  readonly terminal?: TerminalCause
}

/** What a read of linking, enrollments and devices answered. */
export type ReadValue =
  | { readonly kind: "off" }
  | { readonly kind: "refused" }
  | {
      readonly kind: "on"
      readonly enrollments: readonly Enrollment[]
      readonly devices: readonly LinkedDevice[]
    }

/** A code just created. The enrollment is the invitation it belongs to. */
export interface CreatedCode {
  readonly code: string
  readonly enrollment: Enrollment
}

/** An approval: the enrollment after it, and why activation stopped if it did. */
export interface Approval {
  readonly enrollment: Enrollment
  readonly activationStopped?: "retryable" | "permanent"
}

/**
 * How a request failed, in the window's words. `unanswered`: no answer this
 * window can read, so a write may or may not have been applied.
 */
export type FailureKind =
  | "notConfigured"
  | "refused"
  | "busy"
  | "unavailable"
  | "notFound"
  | "slotOccupied"
  | "capacity"
  | "conflict"
  | "ineligible"
  | "unauthorized"
  | "invalid"
  | "temporarilyUnavailable"
  | "credentialMissing"
  | "unanswered"

export interface Failure {
  readonly kind: FailureKind
}

export type Outcome<T> =
  | { readonly ok: true; readonly value: T }
  | {
      readonly ok: false
      readonly failure: Failure
      /** A refresh whose cancel succeeded and whose create did not. */
      readonly cancelled?: boolean
    }

/** What the last answer said, when it said something the rows do not. */
export type NoticeKind =
  | FailureKind
  | "tryAgain"
  | "pairAgain"
  | "codeHidden"
  | "revoked"
  | "revokeUncertain"
  | "unreachable"

export interface Notice {
  readonly kind: NoticeKind
  /** An action's survives the read that follows it. A poll clears neither. */
  readonly from: "read" | "action"
}

/** The one request in flight. `poll`: a read that must not clear an action notice. */
export type PendingRequest =
  | { readonly kind: "read"; readonly seq: number; readonly poll?: true }
  | { readonly kind: "create"; readonly seq: number }
  | { readonly kind: "cancel"; readonly seq: number; readonly invitationId: string }
  | { readonly kind: "refresh"; readonly seq: number; readonly invitationId: string }
  | {
      readonly kind: "approve"
      readonly seq: number
      readonly invitationId: string
      readonly deviceKey: string
    }
  | { readonly kind: "deny"; readonly seq: number; readonly invitationId: string }
  | { readonly kind: "status"; readonly seq: number; readonly invitationId: string }
  | { readonly kind: "revoke"; readonly seq: number; readonly credentialId: string }

export interface LinkedDevicesState {
  readonly connection: "connecting" | "connected" | "unreachable"
  readonly linking: Linking
  readonly code: OpenCode | null
  /** An open invitation whose code this window was not shown. */
  readonly hiddenInvitation: string | null
  readonly waiting: readonly WaitingDevice[]
  readonly devices: readonly LinkedDevice[]
  readonly notice: Notice | null
  /** The credential a revoke is asking about. */
  readonly confirmRevoke: string | null
  readonly pending: PendingRequest | null
  readonly seq: number
}

export type LinkedDevicesEvent =
  | { readonly type: "connected" }
  | { readonly type: "unreachable" }
  | {
      readonly type: "answered"
      readonly seq: number
      readonly outcome: Outcome<unknown>
    }
  | { readonly type: "pair" }
  | { readonly type: "cancel" }
  | { readonly type: "refresh" }
  | { readonly type: "approve"; readonly invitationId: string }
  | { readonly type: "deny"; readonly invitationId: string }
  /** The shown code's clock passed its expiry: ask the gateway to record it. */
  | { readonly type: "expired" }
  | { readonly type: "askRevoke"; readonly credentialId: string }
  | { readonly type: "cancelRevoke" }
  | { readonly type: "confirmRevoke" }
  | { readonly type: "poll" }

export function initialLinkedDevicesState(): LinkedDevicesState {
  return {
    connection: "connecting",
    linking: "unknown",
    code: null,
    hiddenInvitation: null,
    waiting: [],
    devices: [],
    notice: null,
    confirmRevoke: null,
    pending: null,
    seq: 0,
  }
}

/* ——— What the window says. None of these sentences names a wire code. ——— */

const EXAMPLE_ADDRESS = "127.0.0.1:47650"

export const sentences = {
  checking: "Checking whether linking is on.",
  notConfigured: "Linking is off on this gateway.",
  enable: `To turn it on, add "native": { "listenAddress": "${EXAMPLE_ADDRESS}" } to this gateway's config.json and restart it. The address has to be numbers, not a name.`,
  disable:
    "To turn it off, remove native.listenAddress from config.json and restart the gateway.",
  on: "Devices can pair with this gateway.",
  refused:
    "This sign-in cannot manage credentials and read conversations. Run auth recover-owner, then sign in again.",
  tryAgain: "Approval is saved. Activation stopped. Try again.",
  pairAgain:
    "Approval is saved. Activation cannot finish this way. Cancel this request and pair again.",
  codeHidden:
    "A code is already open, but it was not shown. Cancel it and pair again to see a new one.",
  busy: "Another pairing is in progress. Try again in a moment.",
  unavailable: "Linking is unavailable right now. Try again in a moment.",
  notFound: "That request is no longer open.",
  slotOccupied: "A code is already open. Cancel it before opening another.",
  capacity: "Too many unfinished pairings. Finish or cancel one first.",
  conflict: "That no longer matches what the gateway has. The list was read again.",
  ineligible: "That request can no longer take this step.",
  unauthorized: "This sign-in is no longer accepted. Sign in again.",
  invalid: "The gateway refused that as it was sent.",
  temporarilyUnavailable: "The gateway is busy. Try again in a moment.",
  credentialMissing: "That device is no longer linked.",
  unanswered: "No answer. The list is read again to show where it stands.",
  revoked:
    "Revoked. The device can no longer sign in, and an open connection is closed the next time it reads.",
  revokeUncertain:
    "Not confirmed. The device may or may not still be linked. The list shows where it stands.",
  unreachable: "Cannot reach the gateway.",
  empty: "No linked devices.",
  pair: "Pair a device",
  cancel: "Cancel",
  refresh: "New code",
  approve: "Approve",
  deny: "Deny",
  revoke: "Revoke",
  keep: "Keep",
  revokeAsk:
    "Revoke this device? It can no longer sign in, and an open connection is closed the next time it reads.",
  fingerprintMissing: "The device key was not shown, so it cannot be approved from here.",
  unavailableHere: "Not available yet",
} as const

/** What a notice says. Total over {@link NoticeKind}. */
export const noticeText: Record<NoticeKind, string> = {
  notConfigured: sentences.notConfigured,
  refused: sentences.refused,
  busy: sentences.busy,
  unavailable: sentences.unavailable,
  notFound: sentences.notFound,
  slotOccupied: sentences.slotOccupied,
  capacity: sentences.capacity,
  conflict: sentences.conflict,
  ineligible: sentences.ineligible,
  unauthorized: sentences.unauthorized,
  invalid: sentences.invalid,
  temporarilyUnavailable: sentences.temporarilyUnavailable,
  credentialMissing: sentences.credentialMissing,
  unanswered: sentences.unanswered,
  tryAgain: sentences.tryAgain,
  pairAgain: sentences.pairAgain,
  codeHidden: sentences.codeHidden,
  revoked: sentences.revoked,
  revokeUncertain: sentences.revokeUncertain,
  unreachable: sentences.unreachable,
}

/* ——— What may be done now ——— */

/** One request at a time, while linking is on and the gateway can be reached. */
export function canAct(state: LinkedDevicesState): boolean {
  return (
    state.connection === "connected" && state.linking === "on" && state.pending === null
  )
}

/**
 * A quiet poll may read. Linking is on, or a failed first read left it
 * unknown. Off and refused are answers. An open confirm is left alone.
 */
export function canPoll(state: LinkedDevicesState): boolean {
  return (
    state.connection === "connected" &&
    state.pending === null &&
    state.confirmRevoke === null &&
    (state.linking === "on" || (state.linking === "unknown" && state.notice !== null))
  )
}

/** Whether Pair a device may be sent: nothing is already open. */
export function canPair(state: LinkedDevicesState): boolean {
  return (
    canAct(state) &&
    state.code === null &&
    state.hiddenInvitation === null &&
    state.confirmRevoke === null
  )
}

const openInvitation = (state: LinkedDevicesState) =>
  state.code?.invitationId ?? state.hiddenInvitation

/** Whether the open invitation may be cancelled or replaced. */
export function canReplace(state: LinkedDevicesState): boolean {
  return canAct(state) && openInvitation(state) !== null && state.confirmRevoke === null
}

/** A waiting device Approve may name: its key was shown and hashed. */
export function approvable(device: WaitingDevice): boolean {
  return device.deviceKey !== undefined && device.fingerprint !== undefined
}

/* ——— The reducer ——— */

/** `Omit` on a union keeps only the fields every member has. Drop `seq` from each member. */
type RequestBody<T> = T extends unknown ? Omit<T, "seq"> : never

function begin(
  state: LinkedDevicesState,
  request: RequestBody<PendingRequest>,
  notice: Notice | null,
): LinkedDevicesState {
  const seq = state.seq + 1
  return { ...state, seq, notice, pending: { ...request, seq } }
}

const said = (kind: NoticeKind, from: Notice["from"]): Notice => ({ kind, from })

function sortedDevices(devices: readonly LinkedDevice[]): readonly LinkedDevice[] {
  return [...devices].sort(
    (a, b) => b.issuedAt - a.issuedAt || a.credentialId.localeCompare(b.credentialId),
  )
}

/** A read that found linking off or this sign-in refused: nothing else is shown. */
function closed(
  state: LinkedDevicesState,
  linking: "off" | "refused",
): LinkedDevicesState {
  return {
    ...state,
    pending: null,
    linking,
    code: null,
    hiddenInvitation: null,
    waiting: [],
    devices: [],
    confirmRevoke: null,
    notice: null,
  }
}

/**
 * What a read of an open gateway shows. A code this window holds stays while
 * its invitation is still available; once the invitation is gone, so is the
 * code. An available invitation with no code held is one whose code was not
 * shown.
 */
function project(
  state: LinkedDevicesState,
  value: Extract<ReadValue, { kind: "on" }>,
  poll: boolean,
): LinkedDevicesState {
  const enrollments = value.enrollments
  let code = state.code
  if (code) {
    const found = enrollments.find((each) => each.invitationId === code?.invitationId)
    if (!found) code = null
    else if (found.phase === "available")
      code = { ...code, expiresAtMs: found.expiresAtMs, state: "open" }
    else if (found.phase === "terminal" && found.terminal === "cancelled")
      code = { ...code, state: "cancelled" }
    else if (found.phase === "terminal" && found.terminal === "expired")
      code = { ...code, expiresAtMs: 0, state: "open" }
    else if (found.phase === "terminal") code = null
    else code = { ...code, state: "used" }
  }
  const hidden =
    code === null
      ? (enrollments.find((each) => each.phase === "available")?.invitationId ?? null)
      : null
  const waiting = enrollments.flatMap((each): WaitingDevice[] => {
    if (each.phase !== "claimed" && each.phase !== "approved" && each.phase !== "staging")
      return []
    const previous = state.waiting.find((row) => row.invitationId === each.invitationId)
    return [
      {
        invitationId: each.invitationId,
        phase: each.phase,
        ...(each.fingerprint ? { fingerprint: each.fingerprint } : {}),
        ...(each.deviceKey ? { deviceKey: each.deviceKey } : {}),
        ...(each.credentialId ? { credentialId: each.credentialId } : {}),
        ...(previous?.activation ? { activation: previous.activation } : {}),
      },
    ]
  })
  const devices = sortedDevices(
    value.devices.map((device) => {
      const match = enrollments.find(
        (each) => each.credentialId === device.credentialId && each.fingerprint,
      )
      return match?.fingerprint ? { ...device, fingerprint: match.fingerprint } : device
    }),
  )
  const notice = noticeAfterRead(state, hidden !== null, poll)
  const confirm =
    state.confirmRevoke !== null &&
    devices.some((device) => device.credentialId === state.confirmRevoke)
      ? state.confirmRevoke
      : null
  return {
    ...state,
    pending: null,
    linking: "on",
    code,
    hiddenInvitation: hidden,
    waiting,
    devices,
    notice,
    confirmRevoke: confirm,
  }
}

/** A poll keeps whatever was said. Any other read keeps an action's words. */
function noticeAfterRead(
  state: LinkedDevicesState,
  hidden: boolean,
  poll: boolean,
): Notice | null {
  if (poll) {
    if (state.notice) return state.notice
    return hidden ? said("codeHidden", "read") : null
  }
  if (state.notice?.from === "action") return state.notice
  return hidden ? said("codeHidden", "read") : null
}

function wipeFor(failure: Failure): Partial<LinkedDevicesState> | null {
  if (failure.kind === "notConfigured")
    return {
      linking: "off",
      code: null,
      hiddenInvitation: null,
      waiting: [],
      devices: [],
      confirmRevoke: null,
    }
  if (failure.kind === "refused")
    return {
      linking: "refused",
      code: null,
      hiddenInvitation: null,
      waiting: [],
      devices: [],
      confirmRevoke: null,
    }
  return null
}

/**
 * A failed write. Linking that is off or refused is not read again. Anything
 * else is: the write may have changed what is open.
 */
function failed(
  state: LinkedDevicesState,
  failure: Failure,
  options: {
    readonly clearCode?: boolean
    readonly thenRead?: boolean
    readonly from?: Notice["from"]
  },
): LinkedDevicesState {
  const notice = said(failure.kind, options.from ?? "action")
  const wipe = wipeFor(failure)
  const cleared = options.clearCode ? { code: null, hiddenInvitation: null } : {}
  const next: LinkedDevicesState = {
    ...state,
    ...cleared,
    ...wipe,
    pending: null,
    notice,
  }
  if (options.thenRead && next.linking === "on")
    return begin(next, { kind: "read" }, notice)
  return next
}

function withCode(
  state: LinkedDevicesState,
  created: CreatedCode,
  notice: Notice | null,
) {
  return begin(
    {
      ...state,
      pending: null,
      hiddenInvitation: null,
      code: {
        invitationId: created.enrollment.invitationId,
        code: created.code,
        expiresAtMs: created.enrollment.expiresAtMs,
        state: "open",
      },
    },
    { kind: "read" },
    notice,
  )
}

function waitingAfter(
  waiting: readonly WaitingDevice[],
  enrollment: Enrollment,
  activation: WaitingDevice["activation"],
): readonly WaitingDevice[] {
  const rest = waiting.filter((row) => row.invitationId !== enrollment.invitationId)
  if (
    enrollment.phase !== "claimed" &&
    enrollment.phase !== "approved" &&
    enrollment.phase !== "staging"
  )
    return rest
  return [
    ...rest,
    {
      invitationId: enrollment.invitationId,
      phase: enrollment.phase,
      ...(enrollment.fingerprint ? { fingerprint: enrollment.fingerprint } : {}),
      ...(enrollment.deviceKey ? { deviceKey: enrollment.deviceKey } : {}),
      ...(enrollment.credentialId ? { credentialId: enrollment.credentialId } : {}),
      ...(activation ? { activation } : {}),
    },
  ]
}

function answeredRead(
  state: LinkedDevicesState,
  pending: Extract<PendingRequest, { kind: "read" }>,
  outcome: Outcome<ReadValue>,
): LinkedDevicesState {
  if (!outcome.ok) {
    const wipe = wipeFor(outcome.failure)
    const notice =
      pending.poll && state.notice ? state.notice : said(outcome.failure.kind, "read")
    return { ...state, ...wipe, pending: null, notice }
  }
  if (outcome.value.kind === "off") return closed(state, "off")
  if (outcome.value.kind === "refused") return closed(state, "refused")
  return project(state, outcome.value, pending.poll === true)
}

function answeredCreate(
  state: LinkedDevicesState,
  outcome: Outcome<CreatedCode>,
): LinkedDevicesState {
  if (!outcome.ok)
    return failed(state, outcome.failure, {
      clearCode: outcome.cancelled === true,
      thenRead: true,
      // An unanswered create is replaced by "the code was not shown" once a
      // read finds the open invitation.
      from: outcome.failure.kind === "unanswered" ? "read" : "action",
    })
  return withCode(state, outcome.value, null)
}

function answeredApprove(
  state: LinkedDevicesState,
  outcome: Outcome<Approval>,
): LinkedDevicesState {
  if (!outcome.ok) return failed(state, outcome.failure, { thenRead: true })
  const { enrollment, activationStopped } = outcome.value
  const notice =
    activationStopped === "retryable"
      ? said("tryAgain", "action")
      : activationStopped === "permanent"
        ? said("pairAgain", "action")
        : null
  const code =
    state.code?.invitationId === enrollment.invitationId
      ? { ...state.code, state: "used" as const }
      : state.code
  return begin(
    {
      ...state,
      pending: null,
      code,
      waiting: waitingAfter(state.waiting, enrollment, activationStopped),
    },
    { kind: "read" },
    notice,
  )
}

function applyStatus(
  state: LinkedDevicesState,
  enrollment: Enrollment,
): LinkedDevicesState {
  let code = state.code
  if (code?.invitationId === enrollment.invitationId) {
    if (enrollment.phase === "terminal" && enrollment.terminal === "expired")
      code = { ...code, expiresAtMs: 0, state: "open" }
    else if (enrollment.phase === "terminal" && enrollment.terminal === "cancelled")
      code = { ...code, state: "cancelled" }
    else if (enrollment.phase === "available")
      code = { ...code, expiresAtMs: enrollment.expiresAtMs, state: "open" }
    else if (enrollment.phase === "terminal") code = null
    else code = { ...code, state: "used" }
  }
  return {
    ...state,
    pending: null,
    code,
    waiting: waitingAfter(state.waiting, enrollment, undefined),
  }
}

function answered(
  state: LinkedDevicesState,
  seq: number,
  outcome: Outcome<unknown>,
): LinkedDevicesState {
  const pending = state.pending
  if (pending === null || pending.seq !== seq) return state
  switch (pending.kind) {
    case "read":
      return answeredRead(state, pending, outcome as Outcome<ReadValue>)
    case "create":
    case "refresh":
      return answeredCreate(state, outcome as Outcome<CreatedCode>)
    case "cancel":
      if (!outcome.ok) return failed(state, outcome.failure, { thenRead: true })
      return begin(
        {
          ...state,
          pending: null,
          code: state.code?.invitationId === pending.invitationId ? null : state.code,
          hiddenInvitation:
            state.hiddenInvitation === pending.invitationId
              ? null
              : state.hiddenInvitation,
        },
        { kind: "read" },
        null,
      )
    case "approve":
      return answeredApprove(state, outcome as Outcome<Approval>)
    case "deny":
      if (!outcome.ok) return failed(state, outcome.failure, { thenRead: true })
      return begin(
        {
          ...state,
          pending: null,
          waiting: state.waiting.filter(
            (row) => row.invitationId !== pending.invitationId,
          ),
          code: state.code?.invitationId === pending.invitationId ? null : state.code,
        },
        { kind: "read" },
        null,
      )
    case "status":
      if (!outcome.ok)
        return {
          ...state,
          pending: null,
          notice:
            state.notice?.from === "action"
              ? state.notice
              : said(outcome.failure.kind, "read"),
        }
      return applyStatus(state, outcome.value as Enrollment)
    case "revoke":
      if (!outcome.ok) {
        const asked = { ...state, confirmRevoke: null }
        if (outcome.failure.kind === "unanswered")
          return begin(asked, { kind: "read" }, said("revokeUncertain", "action"))
        return failed(asked, outcome.failure, { thenRead: true })
      }
      return begin(
        { ...state, pending: null, confirmRevoke: null },
        { kind: "read" },
        said("revoked", "action"),
      )
  }
}

export function linkedDevicesReducer(
  state: LinkedDevicesState,
  event: LinkedDevicesEvent,
): LinkedDevicesState {
  switch (event.type) {
    case "connected": {
      const next: LinkedDevicesState = { ...state, connection: "connected" }
      return next.pending === null ? begin(next, { kind: "read" }, null) : next
    }
    case "unreachable":
      return {
        ...state,
        connection: "unreachable",
        notice:
          state.notice?.from === "action" ? state.notice : said("unreachable", "read"),
      }
    case "answered":
      return answered(state, event.seq, event.outcome)
    case "pair":
      if (!canPair(state)) return state
      return begin(state, { kind: "create" }, null)
    case "cancel": {
      const invitationId = openInvitation(state)
      if (!canReplace(state) || invitationId === null) return state
      return begin(state, { kind: "cancel", invitationId }, null)
    }
    case "refresh": {
      const invitationId = openInvitation(state)
      if (!canReplace(state) || invitationId === null) return state
      return begin(state, { kind: "refresh", invitationId }, null)
    }
    case "approve": {
      const device = state.waiting.find((row) => row.invitationId === event.invitationId)
      if (
        !canAct(state) ||
        !device ||
        !approvable(device) ||
        device.deviceKey === undefined
      )
        return state
      return begin(
        state,
        {
          kind: "approve",
          invitationId: device.invitationId,
          deviceKey: device.deviceKey,
        },
        null,
      )
    }
    case "deny": {
      const device = state.waiting.find((row) => row.invitationId === event.invitationId)
      if (!canAct(state) || !device) return state
      return begin(state, { kind: "deny", invitationId: device.invitationId }, null)
    }
    case "expired":
      if (!canAct(state) || state.code?.state !== "open") return state
      return begin(
        state,
        { kind: "status", invitationId: state.code.invitationId },
        state.notice,
      )
    case "askRevoke":
      if (
        !canAct(state) ||
        !state.devices.some((device) => device.credentialId === event.credentialId)
      )
        return state
      return { ...state, confirmRevoke: event.credentialId, notice: null }
    case "cancelRevoke":
      if (state.pending) return state
      return { ...state, confirmRevoke: null }
    case "confirmRevoke": {
      if (!canAct(state) || state.confirmRevoke === null) return state
      return begin(state, { kind: "revoke", credentialId: state.confirmRevoke }, null)
    }
    case "poll":
      if (!canPoll(state)) return state
      // A quiet tab reads again. A poll must not clear what an action said.
      // A first read that failed left linking unknown: this one is an ordinary
      // read, so a success replaces that failure.
      if (state.linking === "on")
        return begin(state, { kind: "read", poll: true }, state.notice)
      return begin(state, { kind: "read" }, state.notice)
  }
}
