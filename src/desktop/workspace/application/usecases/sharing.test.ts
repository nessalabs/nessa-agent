/**
 * The share dialog's transitions, by the rows of the table in
 * `docs/design/read-grants.md` › "Dialog state and order".
 */
import { describe, expect, it } from "vitest"
import {
  emptyDraft,
  type ShareDraft,
  type ShareGrant,
  type SharingChoices,
} from "../../model/sharing"
import {
  initialWorkspace,
  type ShareChange,
  type WorkspaceState,
} from "../workspace-state"
import {
  canChange,
  formHeld,
  shareChanged,
  shareChanging,
  shareClosed,
  shareDrafted,
  shareOpened,
  shareRestarted,
  shareConfirming,
  shareConfirmCancelled,
  sharesRead,
  sharesReading,
  waitingOn,
} from "./sharing"

const limits = { patterns: 16, hosts: 16, hostBytes: 96 }
const grant = (
  credentialId: string,
  policyRevision = 1,
  role: ShareGrant["role"] = "read",
): ShareGrant => ({ credentialId, role, policyRevision, grantedAtMs: 1 })
const choices = (credentialId = "a"): SharingChoices => ({
  targets: [{ credentialId, issuedAt: 1, fullAccess: false }],
  commandHosts: [],
  limits,
})
const ok = <T>(value: T) => ({ ok: true as const, value })
const failed = (reason: "busy" | "storage" | "signed-out") => ({
  ok: false as const,
  reason,
})
/** Read `read` of opening `opening`, both parts answered. */
const read = (
  state: WorkspaceState,
  grants: readonly ShareGrant[],
  { opening = "o1", read = "r1" }: { opening?: string; read?: string } = {},
) => sharesRead(state, { opening, read, grants: ok(grants), choices: ok(choices()) })
const revokeA: ShareChange = {
  kind: "revoke",
  credentialId: "a",
  expectedPolicyRevision: 1,
}

const opened = (): WorkspaceState =>
  shareOpened(initialWorkspace, { sessionId: "s1", opening: "o1", read: "r1" })

describe("the share dialog", () => {
  it("D1: opens for one session, named by its opening, reading with nothing read yet", () => {
    const state = opened()
    expect(state.share).toEqual({
      sessionId: "s1",
      opening: "o1",
      grants: null,
      choices: null,
      reading: "r1",
      readFailure: null,
      settled: null,
      confirming: null,
      draft: emptyDraft(null),
    })
    expect(state.shareChange).toBeNull()
    expect(canChange(state, "o1")).toBe(false)
  })

  it("D2: applies only the newest read", () => {
    let state = sharesReading(opened(), { opening: "o1", read: "r2" })
    state = read(state, [grant("old")])
    expect(state.share?.grants).toBeNull()
    expect(state.share?.reading).toBe("r2")
    state = sharesRead(state, {
      opening: "o1",
      read: "r1",
      grants: failed("busy"),
      choices: failed("busy"),
    })
    expect(state.share?.readFailure).toBeNull()
    state = read(state, [grant("new")], { read: "r2" })
    expect(state.share?.grants).toEqual([grant("new")])
    expect(state.share?.choices).toEqual(choices())
    expect(state.share?.reading).toBeNull()
  })

  it("D3, D7: lets an answer for another opening, or after closing, go", () => {
    const state = opened()
    for (const next of [
      read(state, [grant("a")], { opening: "o0" }),
      sharesReading(state, { opening: "o0", read: "r9" }),
      shareChanging(state, { opening: "o0", change: revokeA }),
      shareDrafted(state, { opening: "o0", draft: emptyDraft("a") }),
      shareRestarted(state, { opening: "o0" }),
      // Nothing on its way: a settle changes nothing.
      shareChanged(state, { settled: { outcome: "done", held: null }, read: "r9" }),
    ])
      expect(next).toBe(state)
    const closed = shareClosed(state)
    expect(closed.share).toBeNull()
    expect(read(closed, [grant("a")])).toBe(closed)
    expect(shareClosed(closed)).toBe(closed)
  })

  it("D4: says why the newest read failed and keeps what was read before", () => {
    let state = read(opened(), [grant("a")])
    state = sharesReading(state, { opening: "o1", read: "r2" })
    state = sharesRead(state, {
      opening: "o1",
      read: "r2",
      grants: failed("storage"),
      choices: failed("busy"),
    })
    // The grants' failure is said first; what each part read stays.
    expect(state.share).toMatchObject({
      grants: [grant("a")],
      choices: choices(),
      reading: null,
      readFailure: { part: "grants", reason: "storage" },
    })
    state = sharesReading(state, { opening: "o1", read: "r3" })
    state = read(state, [grant("b")], { read: "r3" })
    expect(state.share?.readFailure).toBeNull()
  })

  it("D4: a failure in the devices and hosts leaves the grants, and a revoke of one", () => {
    let state = sharesRead(opened(), {
      opening: "o1",
      read: "r1",
      grants: ok([grant("a")]),
      choices: failed("signed-out"),
    })
    expect(state.share).toMatchObject({
      grants: [grant("a")],
      choices: null,
      readFailure: { part: "choices", reason: "signed-out" },
    })
    expect(canChange(state, "o1")).toBe(true)
    state = shareChanging(state, { opening: "o1", change: revokeA })
    expect(state.shareChange).toEqual({ sessionId: "s1", change: revokeA })
  })

  it("D5: says a settled change's outcome and clears it when the next begins", () => {
    let state = read(opened(), [grant("a")])
    state = shareChanging(state, { opening: "o1", change: revokeA })
    expect(state.shareChange).toEqual({ sessionId: "s1", change: revokeA })
    state = shareChanged(state, { settled: { outcome: "not-paired" }, read: "r2" })
    expect(state.shareChange).toBeNull()
    expect(state.share).toMatchObject({
      settled: { change: revokeA, outcome: "not-paired" },
      // The outcome changes nothing that was read: only the read it starts does.
      grants: [grant("a")],
      reading: "r2",
    })
    state = read(state, [grant("a")], { read: "r2" })
    state = shareChanging(state, { opening: "o1", change: revokeA })
    expect(state.share?.settled).toBeNull()
  })

  it("D6: begins one change at a time, and none before the grants were read", () => {
    expect(shareChanging(opened(), { opening: "o1", change: revokeA })).toEqual(opened())
    let state = read(opened(), [grant("a")])
    state = shareChanging(state, { opening: "o1", change: revokeA })
    expect(canChange(state, "o1")).toBe(false)
    const other: ShareChange = { ...revokeA, credentialId: "b" }
    expect(shareChanging(state, { opening: "o1", change: other })).toBe(state)
    // A settle with nothing on its way changes nothing.
    const settled = shareChanged(state, {
      settled: { outcome: "done", held: null },
      read: "r2",
    })
    expect(shareChanged(settled, { settled: { outcome: "busy" }, read: "r3" })).toBe(
      settled,
    )
  })

  it("D6: a dialog opened while a change is on its way begins none, says it waits, and is told its outcome", () => {
    let state = read(opened(), [grant("a")])
    state = shareChanging(state, { opening: "o1", change: revokeA })
    expect(waitingOn(state)).toBe("this-session")
    state = shareOpened(shareClosed(state), {
      sessionId: "s1",
      opening: "o2",
      read: "r2",
    })
    state = read(state, [grant("a")], { opening: "o2", read: "r2" })
    expect(waitingOn(state)).toBe("this-session")
    expect(canChange(state, "o2")).toBe(false)
    expect(shareChanging(state, { opening: "o2", change: revokeA })).toBe(state)
    state = shareChanged(state, { settled: { outcome: "done", held: null }, read: "r3" })
    expect(state.shareChange).toBeNull()
    expect(waitingOn(state)).toBeNull()
    expect(state.share).toMatchObject({
      opening: "o2",
      settled: { change: revokeA, outcome: "done" },
      reading: "r3",
    })
  })

  it("D6: a dialog on another session says it waits on that change", () => {
    let state = read(opened(), [grant("a")])
    state = shareChanging(state, { opening: "o1", change: revokeA })
    state = shareOpened(state, { sessionId: "s2", opening: "o2", read: "r2" })
    expect(waitingOn(state)).toBe("another-session")
    expect(
      waitingOn(
        shareChanged(state, { settled: { outcome: "done", held: null }, read: "r3" }),
      ),
    ).toBeNull()
  })

  it("D7: a change settles for the window when the dialog shows another session", () => {
    let state = read(opened(), [grant("a")])
    state = shareChanging(state, { opening: "o1", change: revokeA })
    state = shareOpened(state, { sessionId: "s2", opening: "o2", read: "r2" })
    const settled = shareChanged(state, {
      settled: { outcome: "done", held: null },
      read: "r3",
    })
    expect(settled.shareChange).toBeNull()
    expect(settled.share).toBe(state.share)
  })

  it("D8: a new opening replaces the dialog, so the old one's answers are let go", () => {
    let state = opened()
    state = shareOpened(state, { sessionId: "s2", opening: "o2", read: "r2" })
    expect(read(state, [grant("a")])).toBe(state)
    expect(state.share?.sessionId).toBe("s2")
  })

  it("D10: held while a change is on its way and while the read after it is, then built on the revision the gateway answered", () => {
    let state = read(opened(), [grant("a", 1)])
    const draft: ShareDraft = { ...emptyDraft("a"), basis: 1, role: "comment" }
    state = shareDrafted(state, { opening: "o1", draft })
    expect(state.share?.draft).toEqual(draft)
    state = shareChanging(state, {
      opening: "o1",
      change: {
        kind: "share",
        request: { credentialId: "a", role: "comment", expectedPolicyRevision: 1 },
      },
    })
    expect(formHeld(state)).toBe(true)
    // An edit while held is not taken.
    expect(shareDrafted(state, { opening: "o1", draft: emptyDraft("b") })).toBe(state)
    state = shareChanged(state, { settled: { outcome: "done", held: 4 }, read: "r2" })
    // The basis is the answer's, before any read.
    expect(state.share?.draft).toEqual({ ...draft, basis: 4 })
    expect(formHeld(state)).toBe(true)
    expect(canChange(state, "o1")).toBe(false)
    // The read after it holds another window's later change too: the draft
    // does not take it as its own.
    state = read(state, [grant("a", 6, "drive")], { read: "r2" })
    expect(formHeld(state)).toBe(false)
    expect(state.share?.draft).toEqual({ ...draft, basis: 4 })
  })

  it("D10: a read that fails after any settled change keeps the list as it was, held until Try Again", () => {
    for (const settled of [
      { outcome: "done", held: null },
      { outcome: "changed" },
      { outcome: "unanswered" },
    ] as const) {
      let state = read(opened(), [grant("a")])
      state = shareChanging(state, { opening: "o1", change: revokeA })
      state = shareChanged(state, { settled, read: "r2" })
      state = sharesRead(state, {
        opening: "o1",
        read: "r2",
        grants: failed("busy"),
        choices: failed("busy"),
      })
      expect(state.share?.reading).toBeNull()
      expect(state.share?.readFailure).toEqual({ part: "grants", reason: "busy" })
      expect(formHeld(state), settled.outcome).toBe(true)
      expect(state.share?.grants).toEqual([grant("a")])
    }
  })

  it("D11: a Revoke asked keeps the revision listed then, until a change begins or a read lists another", () => {
    let state = read(opened(), [grant("a", 2), grant("b", 3)])
    // Nothing is asked about a device not listed.
    expect(shareConfirming(state, { opening: "o1", credentialId: "c" })).toBe(state)
    state = shareConfirming(state, { opening: "o1", credentialId: "a" })
    expect(state.share?.confirming).toEqual({ credentialId: "a", policyRevision: 2 })
    // A read listing it as it was keeps it; one that failed keeps it too.
    state = sharesReading(state, { opening: "o1", read: "r2" })
    expect(formHeld(state)).toBe(true)
    state = read(state, [grant("a", 2)], { read: "r2" })
    expect(state.share?.confirming).toEqual({ credentialId: "a", policyRevision: 2 })
    state = sharesReading(state, { opening: "o1", read: "r3" })
    state = sharesRead(state, {
      opening: "o1",
      read: "r3",
      grants: failed("busy"),
      choices: ok(choices()),
    })
    expect(state.share?.confirming).toEqual({ credentialId: "a", policyRevision: 2 })
    // A read listing another revision, or none, ends it.
    state = sharesReading(state, { opening: "o1", read: "r4" })
    expect(read(state, [grant("a", 5)], { read: "r4" }).share?.confirming).toBeNull()
    expect(read(state, [], { read: "r4" }).share?.confirming).toBeNull()
    state = read(state, [grant("a", 2), grant("b", 3)], { read: "r4" })
    // Cancelled, it ends.
    expect(shareConfirmCancelled(state, { opening: "o1" }).share?.confirming).toBeNull()
    // Any change beginning ends it: one to the same device here.
    state = shareChanging(state, {
      opening: "o1",
      change: {
        kind: "share",
        request: { credentialId: "a", role: "comment", expectedPolicyRevision: 2 },
      },
    })
    expect(state.share?.confirming).toBeNull()
    // While held, nothing is asked.
    expect(shareConfirming(state, { opening: "o1", credentialId: "b" })).toBe(state)
  })

  it.each([
    [
      "a change",
      [grant("a", 2)],
      {
        kind: "share",
        request: { credentialId: "a", role: "comment", expectedPolicyRevision: 2 },
      } as ShareChange,
      5,
    ],
    [
      "a first share",
      [],
      {
        kind: "share",
        request: { credentialId: "a", role: "read", expectedPolicyRevision: 0 },
      } as ShareChange,
      5,
    ],
    ["a revoke", [grant("a", 2)], { ...revokeA, expectedPolicyRevision: 2 }, null],
  ] as const)(
    "D10: after %s, the list and the form are held until the grants are read again, also when that read fails",
    (_, before, change, held) => {
      let state = read(opened(), before)
      state = shareDrafted(state, {
        opening: "o1",
        draft: { ...emptyDraft("a"), basis: before[0]?.policyRevision ?? null },
      })
      state = shareChanging(state, { opening: "o1", change })
      state = shareChanged(state, { settled: { outcome: "done", held }, read: "r2" })
      // The draft is on the answer; the list still predates it.
      expect(state.share?.draft.basis).toBe(held)
      expect(formHeld(state)).toBe(true)
      // The grants' part fails: the list stays behind, so it stays held —
      // no edit, no change, no Revoke asked, Start Again does nothing.
      state = sharesRead(state, {
        opening: "o1",
        read: "r2",
        grants: failed("busy"),
        choices: ok(choices()),
      })
      const draft = state.share?.draft
      expect(formHeld(state)).toBe(true)
      expect(canChange(state, "o1")).toBe(false)
      // Choosing the device again would start from its stale grant: not taken.
      expect(
        shareDrafted(state, { opening: "o1", draft: { ...emptyDraft("a"), basis: 2 } }),
      ).toBe(state)
      expect(shareConfirming(state, { opening: "o1", credentialId: "a" })).toBe(state)
      expect(shareRestarted(state, { opening: "o1" })).toBe(state)
      // Try Again answers: the list is read after the change, and the draft
      // is as the change left it.
      state = sharesReading(state, { opening: "o1", read: "r3" })
      state = read(state, held === null ? [] : [grant("a", held)], { read: "r3" })
      expect(formHeld(state)).toBe(false)
      expect(canChange(state, "o1")).toBe(true)
      expect(state.share?.draft).toEqual(draft)
    },
  )

  it("D10: a revoke made builds on no grant; a refused or unanswered change moves no draft", () => {
    const draft: ShareDraft = { ...emptyDraft("a"), basis: 1, role: "comment" }
    const settle = (settled: Parameters<typeof shareChanged>[1]["settled"]) => {
      let state = read(opened(), [grant("a")])
      state = shareDrafted(state, { opening: "o1", draft })
      state = shareChanging(state, { opening: "o1", change: revokeA })
      return shareChanged(state, { settled, read: "r2" }).share?.draft
    }
    expect(settle({ outcome: "done", held: null })).toEqual({ ...draft, basis: null })
    expect(settle({ outcome: "unanswered" })).toEqual(draft)
    expect(settle({ outcome: "changed" })).toEqual(draft)
    expect(settle({ outcome: "busy" })).toEqual(draft)
  })

  it("D10: a change to another device leaves the draft as it was", () => {
    let state = read(opened(), [grant("a"), grant("b", 2)])
    const draft: ShareDraft = { ...emptyDraft("a"), basis: 1, role: "comment" }
    state = shareDrafted(state, { opening: "o1", draft })
    state = shareChanging(state, {
      opening: "o1",
      change: { kind: "revoke", credentialId: "b", expectedPolicyRevision: 2 },
    })
    state = shareChanged(state, { settled: { outcome: "done", held: null }, read: "r2" })
    expect(state.share?.draft).toEqual(draft)
  })

  it("D10: Start Again starts a draft changed under it from what its device holds", () => {
    let state = read(opened(), [grant("a")])
    const draft: ShareDraft = { ...emptyDraft("a"), basis: 1, role: "comment" }
    state = shareDrafted(state, { opening: "o1", draft })
    state = sharesReading(state, { opening: "o1", read: "r2" })
    state = read(state, [grant("a", 3, "drive")], { read: "r2" })
    expect(state.share?.draft).toEqual(draft)
    state = sharesReading(state, { opening: "o1", read: "r3" })
    state = read(state, [], { read: "r3" })
    state = shareRestarted(state, { opening: "o1" })
    expect(state.share?.draft).toEqual(emptyDraft("a"))
  })
})
