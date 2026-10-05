/**
 * Settings › Linked devices, one test at least per ordering row L1–L21.
 * L1 (no gateway) is the tab's. L8's "cancel does not create" and L20's
 * credential filter are the adapter's; the rows they share with the reducer
 * are here.
 */
import { describe, expect, it } from "vitest"
import {
  approvable,
  canPair,
  initialLinkedDevicesState,
  linkedDevicesReducer,
  noticeText,
  sentences,
  type Enrollment,
  type LinkedDevicesEvent,
  type LinkedDevicesState,
  type NoticeKind,
  type Outcome,
} from "./linked-devices"

const invitation = "00112233445566778899aabbccddeeff"
const other = "ffeeddccbbaa99887766554433221100"
const key = "11".repeat(32)
const fingerprint = "182ff9da701fd144e2fd2cd41da8ddba979eb01b2bf7fcc3376f4b1b2ecee4e7"
const later = 1_900_000_000_000

function enrollment(patch: Partial<Enrollment> & Pick<Enrollment, "phase">): Enrollment {
  return { invitationId: invitation, expiresAtMs: later, ...patch }
}

function reduce(state: LinkedDevicesState, ...events: LinkedDevicesEvent[]) {
  return events.reduce(linkedDevicesReducer, state)
}

function connected(
  value: Outcome<unknown> = {
    ok: true,
    value: { kind: "on", enrollments: [], devices: [] },
  },
) {
  return reduce(
    initialLinkedDevicesState(),
    { type: "connected" },
    { type: "answered", seq: 1, outcome: value },
  )
}

const on = () => connected()

const wireCodes = [
  "pairing_not_configured",
  "pairing_not_found",
  "pairing_busy",
  "pairing_unavailable",
  "pairing_slot_occupied",
  "pairing_capacity",
  "pairing_conflict",
  "pairing_ineligible",
  "forbidden",
  "unauthorized",
  "invalid_request",
  "temporarily_unavailable",
  "credential_not_found",
  "credential_conflict",
  "credential_capacity",
]

describe("linked devices", () => {
  it("L2: linking off shows how to turn it on, and does not offer pairing", () => {
    const state = connected({ ok: true, value: { kind: "off" } })
    expect(state.linking).toBe("off")
    expect(state.pending).toBeNull()
    expect(canPair(state)).toBe(false)
    expect(sentences.notConfigured).toMatch(/off/)
    expect(sentences.enable).toContain("127.0.0.1:47650")
  })

  it("L3: a refused owner is said in plain words, with no devices", () => {
    const state = connected({ ok: true, value: { kind: "refused" } })
    expect(state.linking).toBe("refused")
    expect(state.devices).toEqual([])
    expect(canPair(state)).toBe(false)
    expect(sentences.refused).toContain("auth recover-owner")
  })

  it("L4: linking on and nothing open offers Pair a device", () => {
    const state = on()
    expect(state.linking).toBe("on")
    expect(canPair(state)).toBe(true)
  })

  it("L5: a created code is kept, and the read that follows does not drop it", () => {
    const opened = reduce(
      on(),
      { type: "pair" },
      {
        type: "answered",
        seq: 2,
        outcome: {
          ok: true,
          value: {
            code: "ABCD-2345",
            enrollment: enrollment({ phase: "available" }),
          },
        },
      },
    )
    expect(opened.code).toMatchObject({ code: "ABCD-2345", state: "open" })
    expect(opened.pending).toMatchObject({ kind: "read", seq: 3 })
    const read = reduce(opened, {
      type: "answered",
      seq: 3,
      outcome: {
        ok: true,
        value: {
          kind: "on",
          enrollments: [enrollment({ phase: "available" })],
          devices: [],
        },
      },
    })
    expect(read.code).toMatchObject({ code: "ABCD-2345", state: "open" })
    expect(read.hiddenInvitation).toBeNull()
    expect(read.pending).toBeNull()
  })

  it("L6: a slot already open creates no code", () => {
    const state = reduce(
      on(),
      { type: "pair" },
      {
        type: "answered",
        seq: 2,
        outcome: { ok: false, failure: { kind: "slotOccupied" } },
      },
    )
    expect(state.code).toBeNull()
    expect(state.notice).toEqual({ kind: "slotOccupied", from: "action" })
    expect(state.pending).toMatchObject({ kind: "read" })
  })

  it("L7: cancel clears the code", () => {
    const held = reduce(
      on(),
      { type: "pair" },
      {
        type: "answered",
        seq: 2,
        outcome: {
          ok: true,
          value: { code: "ABCD-2345", enrollment: enrollment({ phase: "available" }) },
        },
      },
    )
    const cancelled = reduce(
      { ...held, pending: null },
      { type: "cancel" },
      {
        type: "answered",
        seq: held.seq + 1,
        outcome: {
          ok: true,
          value: enrollment({ phase: "terminal", terminal: "cancelled" }),
        },
      },
    )
    expect(cancelled.code).toBeNull()
  })

  it("L8: a refresh whose cancel failed keeps the code; one whose create failed clears it", () => {
    const held = {
      ...on(),
      code: {
        invitationId: invitation,
        code: "ABCD-2345",
        expiresAtMs: later,
        state: "open" as const,
      },
    }
    const kept = reduce(
      held,
      { type: "refresh" },
      {
        type: "answered",
        seq: held.seq + 1,
        outcome: { ok: false, failure: { kind: "busy" } },
      },
    )
    expect(kept.code?.code).toBe("ABCD-2345")
    const cleared = reduce(
      held,
      { type: "refresh" },
      {
        type: "answered",
        seq: held.seq + 1,
        outcome: { ok: false, failure: { kind: "unavailable" }, cancelled: true },
      },
    )
    expect(cleared.code).toBeNull()
  })

  it("L9: a create that was not answered shows no code, and a later open invitation says so", () => {
    const lost = reduce(
      on(),
      { type: "pair" },
      {
        type: "answered",
        seq: 2,
        outcome: { ok: false, failure: { kind: "unanswered" } },
      },
    )
    expect(lost.code).toBeNull()
    expect(lost.pending?.kind).toBe("read")
    const found = reduce(lost, {
      type: "answered",
      seq: lost.seq,
      outcome: {
        ok: true,
        value: {
          kind: "on",
          enrollments: [enrollment({ phase: "available" })],
          devices: [],
        },
      },
    })
    expect(found.code).toBeNull()
    expect(found.hiddenInvitation).toBe(invitation)
    expect(found.notice?.kind).toBe("codeHidden")
  })

  it("L10: a claimed device shows its fingerprint and can be approved", () => {
    const state = connected({
      ok: true,
      value: {
        kind: "on",
        enrollments: [enrollment({ phase: "claimed", deviceKey: key, fingerprint })],
        devices: [],
      },
    })
    expect(state.waiting).toEqual([
      expect.objectContaining({ invitationId: invitation, fingerprint, deviceKey: key }),
    ])
    expect(approvable(state.waiting[0])).toBe(true)
    const asked = reduce(state, { type: "approve", invitationId: invitation })
    expect(asked.pending).toMatchObject({ kind: "approve", deviceKey: key })
  })

  it("L11: a retryable activation stop keeps Approve and says to try again", () => {
    const waiting = connected({
      ok: true,
      value: {
        kind: "on",
        enrollments: [enrollment({ phase: "claimed", deviceKey: key, fingerprint })],
        devices: [],
      },
    })
    const stopped = reduce(
      waiting,
      { type: "approve", invitationId: invitation },
      {
        type: "answered",
        seq: waiting.seq + 1,
        outcome: {
          ok: true,
          value: {
            enrollment: enrollment({ phase: "approved", deviceKey: key, fingerprint }),
            activationStopped: "retryable",
          },
        },
      },
    )
    expect(stopped.notice).toEqual({ kind: "tryAgain", from: "action" })
    expect(stopped.waiting[0]?.activation).toBe("retryable")
    const read = reduce(stopped, {
      type: "answered",
      seq: stopped.seq,
      outcome: {
        ok: true,
        value: {
          kind: "on",
          enrollments: [enrollment({ phase: "approved", deviceKey: key, fingerprint })],
          devices: [],
        },
      },
    })
    expect(read.notice?.kind).toBe("tryAgain")
    expect(approvable(read.waiting[0])).toBe(true)
  })

  it("L12: a permanent activation stop says to cancel and pair again", () => {
    const waiting = connected({
      ok: true,
      value: {
        kind: "on",
        enrollments: [enrollment({ phase: "claimed", deviceKey: key, fingerprint })],
        devices: [],
      },
    })
    const stopped = reduce(
      waiting,
      { type: "approve", invitationId: invitation },
      {
        type: "answered",
        seq: waiting.seq + 1,
        outcome: {
          ok: true,
          value: {
            enrollment: enrollment({ phase: "approved", deviceKey: key, fingerprint }),
            activationStopped: "permanent",
          },
        },
      },
    )
    expect(stopped.notice?.kind).toBe("pairAgain")
    expect(noticeText.pairAgain).toMatch(/Cancel this request and pair again/)
  })

  it("L13: an active approval followed by the list shows the device", () => {
    const waiting = connected({
      ok: true,
      value: {
        kind: "on",
        enrollments: [enrollment({ phase: "claimed", deviceKey: key, fingerprint })],
        devices: [],
      },
    })
    const approved = reduce(
      waiting,
      { type: "approve", invitationId: invitation },
      {
        type: "answered",
        seq: waiting.seq + 1,
        outcome: {
          ok: true,
          value: {
            enrollment: enrollment({ phase: "active", credentialId: "device-1" }),
          },
        },
      },
    )
    const listed = reduce(approved, {
      type: "answered",
      seq: approved.seq,
      outcome: {
        ok: true,
        value: {
          kind: "on",
          enrollments: [],
          devices: [{ credentialId: "device-1", issuedAt: 1_700_000_000 }],
        },
      },
    })
    expect(listed.waiting).toEqual([])
    expect(listed.devices).toEqual([
      { credentialId: "device-1", issuedAt: 1_700_000_000 },
    ])
    expect(listed.code).toBeNull()
  })

  it("L14: deny drops the waiting device", () => {
    const waiting = connected({
      ok: true,
      value: {
        kind: "on",
        enrollments: [enrollment({ phase: "claimed", deviceKey: key, fingerprint })],
        devices: [],
      },
    })
    const denied = reduce(
      waiting,
      { type: "deny", invitationId: invitation },
      {
        type: "answered",
        seq: waiting.seq + 1,
        outcome: {
          ok: true,
          value: enrollment({ phase: "terminal", terminal: "denied" }),
        },
      },
    )
    expect(denied.waiting).toEqual([])
  })

  it("L15: revoke says access ends on the next read, and the next list drops the device", () => {
    const listed = connected({
      ok: true,
      value: {
        kind: "on",
        enrollments: [],
        devices: [{ credentialId: "device-1", issuedAt: 10 }],
      },
    })
    const revoked = reduce(
      listed,
      { type: "askRevoke", credentialId: "device-1" },
      { type: "confirmRevoke" },
      {
        type: "answered",
        seq: listed.seq + 1,
        outcome: { ok: true, value: { credentialId: "device-1" } },
      },
    )
    expect(revoked.notice?.kind).toBe("revoked")
    expect(noticeText.revoked).toMatch(/next time it reads/)
    const after = reduce(revoked, {
      type: "answered",
      seq: revoked.seq,
      outcome: { ok: true, value: { kind: "on", enrollments: [], devices: [] } },
    })
    expect(after.devices).toEqual([])
    expect(after.notice?.kind).toBe("revoked")
  })

  it("L16: a revoke that was not answered says it may or may not have happened", () => {
    const listed = connected({
      ok: true,
      value: {
        kind: "on",
        enrollments: [],
        devices: [{ credentialId: "device-1", issuedAt: 10 }],
      },
    })
    const uncertain = reduce(
      listed,
      { type: "askRevoke", credentialId: "device-1" },
      { type: "confirmRevoke" },
      {
        type: "answered",
        seq: listed.seq + 1,
        outcome: { ok: false, failure: { kind: "unanswered" } },
      },
    )
    expect(uncertain.notice?.kind).toBe("revokeUncertain")
    expect(uncertain.pending?.kind).toBe("read")
  })

  it("L17: a status that says expired puts the code in the past", () => {
    const held = {
      ...on(),
      code: {
        invitationId: invitation,
        code: "ABCD-2345",
        expiresAtMs: later,
        state: "open" as const,
      },
    }
    const expired = reduce(
      held,
      { type: "expired" },
      {
        type: "answered",
        seq: held.seq + 1,
        outcome: {
          ok: true,
          value: enrollment({
            phase: "terminal",
            terminal: "expired",
            expiresAtMs: later,
          }),
        },
      },
    )
    expect(expired.code).toMatchObject({ state: "open", expiresAtMs: 0 })
  })

  it("L18: every refusal has a sentence, and none of them is a wire code", () => {
    const kinds = Object.keys(noticeText) as NoticeKind[]
    expect(kinds.length).toBeGreaterThan(0)
    const text = [...Object.values(noticeText), ...Object.values(sentences)].join("\n")
    for (const code of wireCodes) expect(text).not.toContain(code)
  })

  it("L19: an answer for a request since replaced changes nothing", () => {
    const pairing = reduce(on(), { type: "pair" })
    const stale = reduce(pairing, {
      type: "answered",
      seq: pairing.seq - 1,
      outcome: { ok: false, failure: { kind: "busy" } },
    })
    expect(stale).toBe(pairing)
  })

  it("a failed first read is asked again, and a settled off is not", () => {
    const failed = connected({ ok: false, failure: { kind: "unavailable" } })
    expect(failed.linking).toBe("unknown")
    expect(failed.notice?.kind).toBe("unavailable")
    const retry = reduce(failed, { type: "poll" })
    expect(retry.pending).toEqual({ kind: "read", seq: failed.seq + 1 })
    const back = reduce(retry, {
      type: "answered",
      seq: retry.seq,
      outcome: { ok: true, value: { kind: "on", enrollments: [], devices: [] } },
    })
    expect(back.linking).toBe("on")
    expect(back.notice).toBeNull()
    const off = connected({ ok: true, value: { kind: "off" } })
    expect(reduce(off, { type: "poll" })).toBe(off)
  })

  it("L21: unreachable is said, and an action's notice is kept", () => {
    const quiet = reduce(on(), { type: "unreachable" })
    expect(quiet.connection).toBe("unreachable")
    expect(quiet.notice?.kind).toBe("unreachable")
    expect(canPair(quiet)).toBe(false)
    const said = {
      ...on(),
      notice: { kind: "tryAgain" as const, from: "action" as const },
    }
    expect(reduce(said, { type: "unreachable" }).notice?.kind).toBe("tryAgain")
  })

  it("a poll read does not clear what an action said", () => {
    const said = {
      ...on(),
      notice: { kind: "tryAgain" as const, from: "action" as const },
    }
    const polled = reduce(
      said,
      { type: "poll" },
      {
        type: "answered",
        seq: said.seq + 1,
        outcome: { ok: true, value: { kind: "on", enrollments: [], devices: [] } },
      },
    )
    expect(polled.notice?.kind).toBe("tryAgain")
  })

  it("does not approve a claimed device whose fingerprint was not computed", () => {
    const state = connected({
      ok: true,
      value: {
        kind: "on",
        enrollments: [enrollment({ phase: "claimed", deviceKey: key })],
        devices: [],
      },
    })
    expect(approvable(state.waiting[0])).toBe(false)
    expect(
      reduce(state, { type: "approve", invitationId: invitation }).pending,
    ).toBeNull()
    expect(reduce(state, { type: "deny", invitationId: invitation }).pending?.kind).toBe(
      "deny",
    )
  })

  it("joins a fingerprint from an enrollment onto the device it issued", () => {
    const state = connected({
      ok: true,
      value: {
        kind: "on",
        enrollments: [
          enrollment({
            phase: "staging",
            credentialId: "device-1",
            fingerprint,
            deviceKey: key,
          }),
        ],
        devices: [{ credentialId: "device-1", issuedAt: 10 }],
      },
    })
    expect(state.devices[0]?.fingerprint).toBe(fingerprint)
  })

  it("ignores a second pair while one request is in flight", () => {
    const pairing = reduce(on(), { type: "pair" })
    expect(reduce(pairing, { type: "pair" })).toBe(pairing)
  })

  it("a hidden invitation can be cancelled", () => {
    const hidden = {
      ...on(),
      hiddenInvitation: other,
      notice: { kind: "codeHidden" as const, from: "read" as const },
    }
    const cancelling = reduce(hidden, { type: "cancel" })
    expect(cancelling.pending).toMatchObject({ kind: "cancel", invitationId: other })
  })
})
