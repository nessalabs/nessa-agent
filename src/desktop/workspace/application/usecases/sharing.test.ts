/**
 * The share dialog's transitions, by the rows of the table in
 * `docs/design/read-grants.md` › "Dialog state and order".
 */
import { describe, expect, it } from "vitest"
import { emptyDraft, type ShareDraft, type SharingRead } from "../../model/sharing"
import { initialWorkspace, type WorkspaceState } from "../workspace-state"
import {
  canChange,
  formHeld,
  heldElsewhere,
  shareChanged,
  shareChanging,
  shareClosed,
  shareDrafted,
  shareOpened,
  shareRestarted,
  sharesRead,
  sharesReadFailed,
  sharesReading,
} from "./sharing"

const limits = { patterns: 16, hosts: 16, hostBytes: 96, entries: 32 }
const read = (credentialId: string): SharingRead => ({
  grants: [{ credentialId, role: "read", policyRevision: 0, grantedAtMs: 1 }],
  targets: [{ credentialId, issuedAt: 1 }],
  commandHosts: [],
  limits,
})

const opened = (): WorkspaceState =>
  shareOpened(initialWorkspace, { sessionId: "s1", opening: "o1", read: "r1" })

describe("the share dialog", () => {
  it("D1: opens for one session, named by its opening, reading with nothing read yet", () => {
    const state = opened()
    expect(state.share).toEqual({
      sessionId: "s1",
      opening: "o1",
      value: null,
      reading: "r1",
      readFailure: null,
      notice: null,
      draft: emptyDraft(null),
      stale: null,
    })
    expect(state.shareChange).toBeNull()
    expect(canChange(state, "o1")).toBe(false)
  })

  it("D2: applies only the newest read", () => {
    let state = sharesReading(opened(), { opening: "o1", read: "r2" })
    state = sharesRead(state, { opening: "o1", read: "r1", value: read("old") })
    expect(state.share?.value).toBeNull()
    expect(state.share?.reading).toBe("r2")
    state = sharesReadFailed(state, { opening: "o1", read: "r1", reason: "busy" })
    expect(state.share?.readFailure).toBeNull()
    state = sharesRead(state, { opening: "o1", read: "r2", value: read("new") })
    expect(state.share?.value).toEqual(read("new"))
    expect(state.share?.reading).toBeNull()
  })

  it("D3, D7: lets an answer for another opening, or after closing, go", () => {
    const state = opened()
    for (const next of [
      sharesRead(state, { opening: "o0", read: "r1", value: read("a") }),
      sharesReadFailed(state, { opening: "o0", read: "r1", reason: "busy" }),
      sharesReading(state, { opening: "o0", read: "r9" }),
      shareChanging(state, {
        opening: "o0",
        change: { kind: "revoke", credentialId: "a" },
      }),
      shareDrafted(state, { opening: "o0", draft: emptyDraft("a") }),
      shareRestarted(state, { opening: "o0" }),
      // Nothing on its way: a settle changes nothing.
      shareChanged(state, { outcome: "done" }),
    ])
      expect(next).toBe(state)
    const closed = shareClosed(state)
    expect(closed.share).toBeNull()
    expect(sharesRead(closed, { opening: "o1", read: "r1", value: read("a") })).toBe(
      closed,
    )
    expect(shareClosed(closed)).toBe(closed)
  })

  it("D4: says why the newest read failed and keeps what was read before", () => {
    let state = sharesRead(opened(), { opening: "o1", read: "r1", value: read("a") })
    state = sharesReading(state, { opening: "o1", read: "r2" })
    state = sharesReadFailed(state, { opening: "o1", read: "r2", reason: "storage" })
    expect(state.share).toMatchObject({
      value: read("a"),
      reading: null,
      readFailure: "storage",
    })
    state = sharesReading(state, { opening: "o1", read: "r3" })
    state = sharesRead(state, { opening: "o1", read: "r3", value: read("b") })
    expect(state.share?.readFailure).toBeNull()
  })

  it("D5: says a settled change's outcome and clears the last one's when the next begins", () => {
    const change = { kind: "revoke", credentialId: "a" } as const
    let state = sharesRead(opened(), { opening: "o1", read: "r1", value: read("a") })
    state = shareChanging(state, { opening: "o1", change })
    expect(state.shareChange).toEqual({ sessionId: "s1", change })
    state = shareChanged(state, { outcome: "not-paired" })
    expect(state.shareChange).toBeNull()
    expect(state.share).toMatchObject({
      notice: { change, outcome: "not-paired" },
      // The outcome changes nothing that was read: only the next read does.
      value: read("a"),
    })
    // The next begins once the read after it answers (D10).
    state = sharesReading(state, { opening: "o1", read: "r2" })
    state = sharesRead(state, { opening: "o1", read: "r2", value: read("a") })
    state = shareChanging(state, { opening: "o1", change })
    expect(state.share?.notice).toBeNull()
  })

  it("D6: begins one change at a time, and none before anything was read", () => {
    const change = { kind: "revoke", credentialId: "a" } as const
    expect(shareChanging(opened(), { opening: "o1", change })).toEqual(opened())
    let state = sharesRead(opened(), { opening: "o1", read: "r1", value: read("a") })
    state = shareChanging(state, { opening: "o1", change })
    expect(canChange(state, "o1")).toBe(false)
    const other = { kind: "revoke", credentialId: "b" } as const
    expect(shareChanging(state, { opening: "o1", change: other })).toBe(state)
    // A settle with nothing on its way changes nothing.
    const settled = shareChanged(state, { outcome: "done" })
    expect(shareChanged(settled, { outcome: "busy" })).toBe(settled)
  })

  it("D6: a dialog opened while a change is on its way begins none, and is told its outcome", () => {
    const change = { kind: "revoke", credentialId: "a" } as const
    let state = sharesRead(opened(), { opening: "o1", read: "r1", value: read("a") })
    state = shareChanging(state, { opening: "o1", change })
    state = shareOpened(shareClosed(state), {
      sessionId: "s1",
      opening: "o2",
      read: "r2",
    })
    state = sharesRead(state, { opening: "o2", read: "r2", value: read("a") })
    expect(canChange(state, "o2")).toBe(false)
    expect(shareChanging(state, { opening: "o2", change })).toBe(state)
    state = shareChanged(state, { outcome: "done" })
    expect(state.shareChange).toBeNull()
    expect(state.share).toMatchObject({
      opening: "o2",
      notice: { change, outcome: "done" },
    })
  })

  it("D7: a change settles for the window when the dialog shows another session", () => {
    const change = { kind: "revoke", credentialId: "a" } as const
    let state = sharesRead(opened(), { opening: "o1", read: "r1", value: read("a") })
    state = shareChanging(state, { opening: "o1", change })
    state = shareOpened(state, { sessionId: "s2", opening: "o2", read: "r2" })
    const settled = shareChanged(state, { outcome: "done" })
    expect(settled.shareChange).toBeNull()
    expect(settled.share).toBe(state.share)
  })

  it("D8: a new opening replaces the dialog, so the old one's answers are let go", () => {
    let state = opened()
    state = shareOpened(state, { sessionId: "s2", opening: "o2", read: "r2" })
    expect(sharesRead(state, { opening: "o1", read: "r1", value: read("a") })).toBe(state)
    expect(state.share?.sessionId).toBe("s2")
  })

  it("D10: held while a change is on its way and until the read after it answers, then built on what it left", () => {
    const shared = (role: "read" | "comment"): SharingRead => ({
      ...read("a"),
      grants: [
        {
          credentialId: "a",
          role,
          policyRevision: role === "read" ? 0 : 1,
          grantedAtMs: 1,
        },
      ],
    })
    let state = sharesRead(opened(), { opening: "o1", read: "r1", value: shared("read") })
    const draft: ShareDraft = { ...emptyDraft("a"), basis: 0, role: "comment" }
    state = shareDrafted(state, { opening: "o1", draft })
    expect(state.share?.draft).toEqual(draft)
    state = shareChanging(state, {
      opening: "o1",
      change: { kind: "share", request: { credentialId: "a", role: "comment" } },
    })
    expect(formHeld(state)).toBe(true)
    // An edit while held is not taken.
    expect(shareDrafted(state, { opening: "o1", draft: emptyDraft("b") })).toBe(state)
    state = shareChanged(state, { outcome: "done" })
    expect(formHeld(state)).toBe(true)
    expect(canChange(state, "o1")).toBe(false)
    // A read that fails keeps it held: what is shown is from before the change.
    state = sharesReading(state, { opening: "o1", read: "r2" })
    state = sharesReadFailed(state, { opening: "o1", read: "r2", reason: "busy" })
    expect(formHeld(state)).toBe(true)
    expect(canChange(state, "o1")).toBe(false)
    state = sharesReading(state, { opening: "o1", read: "r3" })
    state = sharesRead(state, { opening: "o1", read: "r3", value: shared("comment") })
    expect(formHeld(state)).toBe(false)
    expect(state.share?.draft).toEqual({ ...draft, basis: 1 })
  })

  it("D10: a revoke, or a change with no answer, keeps what the form showed and builds on what is held", () => {
    const draft: ShareDraft = { ...emptyDraft("a"), basis: 0, role: "comment" }
    const settle = (outcome: "done" | "unanswered" | "busy", value: SharingRead) => {
      let state = sharesRead(opened(), { opening: "o1", read: "r1", value: read("a") })
      state = shareDrafted(state, { opening: "o1", draft })
      state = shareChanging(state, {
        opening: "o1",
        change: { kind: "revoke", credentialId: "a" },
      })
      state = shareChanged(state, { outcome })
      state = sharesReading(state, { opening: "o1", read: "r2" })
      return sharesRead(state, { opening: "o1", read: "r2", value }).share?.draft
    }
    const gone = { ...read("a"), grants: [] }
    expect(settle("done", gone)).toEqual({ ...draft, basis: null })
    expect(settle("unanswered", gone)).toEqual({ ...draft, basis: null })
    // A refusal made nothing: the draft stays built on what it was.
    expect(settle("busy", gone)).toEqual(draft)
  })

  it("D10: Start Again starts a draft changed under it from what its device holds", () => {
    let state = sharesRead(opened(), { opening: "o1", read: "r1", value: read("a") })
    const draft: ShareDraft = { ...emptyDraft("a"), basis: 0, role: "comment" }
    state = shareDrafted(state, { opening: "o1", draft })
    state = sharesReading(state, { opening: "o1", read: "r2" })
    state = sharesRead(state, {
      opening: "o1",
      read: "r2",
      value: { ...read("a"), grants: [] },
    })
    expect(state.share?.draft).toEqual(draft)
    state = shareRestarted(state, { opening: "o1" })
    expect(state.share?.draft).toEqual(emptyDraft("a"))
  })

  it("D6: a dialog on another session says it waits on that change", () => {
    let state = sharesRead(opened(), { opening: "o1", read: "r1", value: read("a") })
    state = shareChanging(state, {
      opening: "o1",
      change: { kind: "revoke", credentialId: "a" },
    })
    expect(heldElsewhere(state)).toBe(false)
    state = shareOpened(state, { sessionId: "s2", opening: "o2", read: "r2" })
    expect(heldElsewhere(state)).toBe(true)
    expect(heldElsewhere(shareChanged(state, { outcome: "done" }))).toBe(false)
  })
})
