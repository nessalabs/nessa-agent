import { describe, expect, it } from "vitest"
import {
  canCancel,
  canReplace,
  initialLinkedDevicesState,
  linkedDevicesReducer as reduce,
  type Enrollment,
  type LinkedDevicesState,
  type Outcome,
  type PendingRequest,
  type ReadValue,
} from "./linked-devices"

const invitationId = "00112233445566778899aabbccddeeff"
const otherId = "ffeeddccbbaa99887766554433221100"
const available = (id = invitationId): Enrollment => ({
  invitationId: id,
  phase: "available",
  expiresAtMs: 2_000_000_000_000,
})
const listed = (enrollments: Enrollment[] = [available()]): Outcome<ReadValue> => ({
  ok: true,
  value: { kind: "on", enrollments, devices: [] },
})

function answer(state: LinkedDevicesState, outcome: Outcome<unknown>) {
  return reduce(state, { type: "answered", seq: state.seq, outcome })
}

function opened() {
  const connected = answer(
    reduce(initialLinkedDevicesState(), { type: "connected" }),
    listed([]),
  )
  return answer(
    answer(reduce(connected, { type: "pair" }), {
      ok: true,
      value: { code: "ABCD-2345", enrollment: available() },
    }),
    listed(),
  )
}

const oldReads: [string, Outcome<ReadValue>][] = [
  ["available", listed()],
  ["different invitation", listed([available(otherId)])],
  ["removed", listed([])],
  ["off", { ok: true, value: { kind: "off" } }],
  ["refused", { ok: true, value: { kind: "refused" } }],
  ["failed", { ok: false, failure: { kind: "unavailable" } }],
]

describe("cancel supersedes only a quiet pairing poll", () => {
  it("L29: captures the held invitation once while other actions stay held", () => {
    const polling = reduce(opened(), { type: "poll" })
    expect(polling.pending).toMatchObject({ kind: "read", poll: true })
    expect(canCancel(polling)).toBe(true)
    expect(canReplace(polling)).toBe(false)
    expect(reduce(polling, { type: "refresh" })).toBe(polling)
    const cancelling = reduce(polling, { type: "cancel", invitationId })
    expect(cancelling.pending).toEqual({
      kind: "cancel",
      invitationId,
      seq: polling.seq + 1,
    })
    expect(reduce(cancelling, { type: "cancel", invitationId })).toBe(cancelling)
    const back = reduce(reduce(cancelling, { type: "unreachable" }), {
      type: "connected",
    })
    expect(back.pending).toEqual(cancelling.pending)
    expect(back.seq).toBe(cancelling.seq)
  })

  it.each(oldReads)(
    "L30: ignores an old %s poll before cancellation answers",
    (_, outcome) => {
      const polling = reduce(opened(), { type: "poll" })
      const cancelling = reduce(polling, { type: "cancel", invitationId })
      expect(reduce(cancelling, { type: "answered", seq: polling.seq, outcome })).toBe(
        cancelling,
      )
    },
  )

  for (const [label, outcome] of [
    [
      "confirmed",
      { ok: true, value: { ...available(), phase: "terminal", terminal: "cancelled" } },
    ],
    ["refused", { ok: false, failure: { kind: "busy" } }],
    ["unanswered", { ok: false, failure: { kind: "unanswered" } }],
  ] satisfies [string, Outcome<Enrollment>][]) {
    it.each(oldReads)(
      `L30: ${label} cancellation fences a late %s poll through reconciliation`,
      (_, old) => {
        const polling = reduce(opened(), { type: "poll" })
        const reconciling = answer(
          reduce(polling, { type: "cancel", invitationId }),
          outcome,
        )
        expect(reconciling.pending).toMatchObject({ kind: "read" })
        expect(canCancel(reconciling)).toBe(false)
        expect(
          reduce(reconciling, { type: "answered", seq: polling.seq, outcome: old }),
        ).toBe(reconciling)
        const current = answer(reconciling, listed([]))
        expect(current.code).toBeNull()
        expect(current.hiddenInvitation).toBeNull()
        expect(current.pending).toBeNull()
        expect(
          reduce(current, { type: "answered", seq: polling.seq, outcome: old }),
        ).toBe(current)
        expect(current.notice).toEqual(reconciling.notice)
      },
    )
  }

  it("L31: a poll that settles first owns which invitation can be cancelled", () => {
    const polling = reduce(opened(), { type: "poll" })
    const removed = answer(polling, listed([]))
    expect(canCancel(removed)).toBe(false)
    expect(reduce(removed, { type: "cancel", invitationId })).toBe(removed)
    const replaced = answer(polling, listed([available(otherId)]))
    expect(replaced.code).toBeNull()
    expect(reduce(replaced, { type: "cancel", invitationId })).toBe(replaced)
    expect(
      reduce(replaced, { type: "cancel", invitationId: otherId }).pending,
    ).toMatchObject({
      kind: "cancel",
      invitationId: otherId,
    })
  })

  it("L32: holds non-poll reads and every mutation/status request", () => {
    const requests: PendingRequest[] = [
      { kind: "read", seq: 20 },
      { kind: "create", seq: 20 },
      { kind: "cancel", seq: 20, invitationId },
      { kind: "refresh", seq: 20, invitationId },
      { kind: "approve", seq: 20, invitationId, deviceKey: "11".repeat(32) },
      { kind: "deny", seq: 20, invitationId },
      { kind: "status", seq: 20, invitationId },
      { kind: "revoke", seq: 20, credentialId: "device" },
      { kind: "setLinking", seq: 20, enabled: false },
    ]
    for (const pending of requests) {
      const state = { ...opened(), seq: 20, pending }
      expect(canCancel(state), pending.kind).toBe(false)
      expect(reduce(state, { type: "cancel", invitationId })).toBe(state)
    }
  })

  it("L32: withholds cancellation across authority, restart and confirmation boundaries", () => {
    const polling = reduce(opened(), { type: "poll" })
    const held: LinkedDevicesState[] = [
      { ...polling, connection: "connecting" },
      { ...polling, connection: "unreachable" },
      ...(["unknown", "off", "refused"] as const).map((linking) => ({
        ...polling,
        linking,
      })),
      { ...polling, switching: { enabled: true, dropped: false, confirmed: true } },
      { ...polling, confirmRevoke: "device" },
      { ...polling, code: null, hiddenInvitation: null },
    ]
    for (const state of held) {
      expect(canCancel(state)).toBe(false)
      expect(reduce(state, { type: "cancel", invitationId })).toBe(state)
    }
  })

  it("L33: cancels a hidden invitation without reconstructing a secret", () => {
    const hidden = answer(
      reduce(initialLinkedDevicesState(), { type: "connected" }),
      listed(),
    )
    expect(hidden.code).toBeNull()
    const polling = reduce(hidden, { type: "poll" })
    const cancelled = answer(reduce(polling, { type: "cancel", invitationId }), {
      ok: true,
      value: { ...available(), phase: "terminal", terminal: "cancelled" },
    })
    expect(cancelled.code).toBeNull()
    expect(cancelled.hiddenInvitation).toBeNull()
    expect(
      reduce(cancelled, { type: "answered", seq: polling.seq, outcome: listed() }),
    ).toBe(cancelled)
  })
})
