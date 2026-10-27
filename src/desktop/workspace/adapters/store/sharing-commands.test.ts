/**
 * The share dialog's commands over a store, by the rows of the table in
 * `docs/design/read-grants.md` › "Dialog state and order": what each sends,
 * and which answers land.
 */
import { describe, expect, it } from "vitest"
import { makeDesktopStore } from "../../../store"
import type { SharingSource } from "../../application/ports"
import { SharingError } from "../../application/ports"
import { createQuestionFlights } from "../../application/question-flights"
import {
  emptyDraft,
  type ShareDraft,
  type ShareGrant,
  type ShareRequest,
  type SharingChoices,
} from "../../model/sharing"
import { fakeSource, keptFilter, roomyGrid, settle } from "../../testing"
import { inMemorySharing } from "../in-memory/in-memory-sharing"
import {
  askRevoke,
  closeShare,
  draftShare,
  openShare,
  readShares,
  revokeShare,
  shareSession,
  type WorkspaceCommand,
} from "./commands"

/** A fixed clock, so a grant's time is the same on every run. */
const testNow = () => 1_700_000_000_000

function storeWith(sharing: SharingSource | undefined) {
  let next = 0
  return makeDesktopStore({
    workspace: fakeSource(),
    now: () => 1000,
    newId: () => `id-${++next}`,
    measure: () => roomyGrid,
    overviewFilter: keptFilter(),
    questionFlights: createQuestionFlights(),
    sharing,
  })
}

const limits = { patterns: 16, hosts: 16, hostBytes: 96 }
const choices: SharingChoices = {
  targets: [{ credentialId: "sample-phone", issuedAt: 1, fullAccess: false }],
  commandHosts: [],
  limits,
}

/**
 * A source whose grants, shares and revokes each wait until the test answers
 * them; what the form offers is answered at once.
 */
function heldSharing() {
  const waiting: {
    method: string
    args: unknown[]
    resolve: (value: unknown) => void
    reject: (error: unknown) => void
  }[] = []
  const hold =
    (method: string) =>
    (...args: unknown[]) =>
      new Promise<never>((resolve, reject) => {
        waiting.push({ method, args, resolve: resolve as never, reject })
      })
  const source = {
    grants: hold("grants"),
    choices: async () => choices,
    share: hold("share"),
    unshare: hold("unshare"),
  } as unknown as SharingSource
  return { source, waiting }
}

const grantsOf = (...credentialIds: string[]): ShareGrant[] =>
  credentialIds.map((credentialId) => ({
    credentialId,
    role: "read",
    policyRevision: 1,
    grantedAtMs: 1,
  }))
/** Set the draft, then share it, as the person does: the draft is what is sent. */
const shareDraft =
  (
    draft: Partial<ShareDraft> & { credentialId: string },
  ): WorkspaceCommand<Promise<void>> =>
  async (dispatch) => {
    dispatch(draftShare({ draft: { ...emptyDraft(draft.credentialId), ...draft } }))
    await dispatch(shareSession())
  }
/** Revoke, then confirm it, as the person does. */
const revoke =
  (credentialId: string): WorkspaceCommand<Promise<void>> =>
  async (dispatch) => {
    dispatch(askRevoke({ credentialId }))
    await dispatch(revokeShare())
  }

describe("sharing from the store", () => {
  it("D1: a window without sharing offers none and opens nothing", async () => {
    const store = storeWith(undefined)
    expect(store.getState().workspace.shareable).toBe(false)
    await store.dispatch(openShare({ sessionId: "a" }))
    expect(store.getState().workspace.share).toBeNull()
  })

  it("D1, D5, D10: opens, reads, shares, builds on the answer, and reads again", async () => {
    const sharing = inMemorySharing({ now: () => 7 })
    const store = storeWith(sharing)
    expect(store.getState().workspace.shareable).toBe(true)
    await store.dispatch(openShare({ sessionId: "a" }))
    expect(store.getState().workspace.share?.grants).toEqual([])
    expect(store.getState().workspace.share?.choices?.targets).toHaveLength(2)
    await store.dispatch(
      shareDraft({
        credentialId: "sample-phone",
        role: "comment",
        basis: null,
      }),
    )
    const dialog = store.getState().workspace.share
    expect(dialog?.settled?.outcome).toBe("done")
    expect(dialog?.grants).toEqual([
      {
        credentialId: "sample-phone",
        role: "comment",
        policyRevision: 1,
        grantedAtMs: 7,
      },
    ])
    expect(sharing.calls.map((call) => call.method)).toEqual([
      "grants",
      "choices",
      "share",
      "grants",
      "choices",
    ])
    // Each change carries a request id of its own.
    expect(sharing.calls[2]?.requestId).toEqual(expect.any(String))
    await store.dispatch(revoke("sample-phone"))
    expect(store.getState().workspace.share?.grants).toEqual([])
    expect(sharing.calls.at(-3)).toMatchObject({
      method: "unshare",
      sessionId: "a",
      credentialId: "sample-phone",
      expectedPolicyRevision: 1,
    })
  })

  it("D10: Share sends the draft as the store holds it when dispatched, and nothing else", async () => {
    const sharing = inMemorySharing({ now: testNow })
    const share = sharing.share
    const sent: ShareRequest[] = []
    sharing.share = (sessionId, request, requestId) => {
      sent.push(request)
      return share(sessionId, request, requestId)
    }
    const store = storeWith(sharing)
    await store.dispatch(openShare({ sessionId: "a" }))
    // A draft that cannot be sent sends nothing.
    await store.dispatch(shareSession())
    expect(sent).toEqual([])
    store.dispatch(draftShare({ draft: { ...emptyDraft("sample-phone"), role: "read" } }))
    // Changed again before Share: the later draft is the one sent.
    store.dispatch(
      draftShare({ draft: { ...emptyDraft("sample-phone"), role: "comment" } }),
    )
    await store.dispatch(shareSession())
    expect(sent).toEqual([
      { credentialId: "sample-phone", role: "comment", expectedPolicyRevision: 0 },
    ])
    // The draft builds on the answer to what it sent.
    expect(store.getState().workspace.share?.draft).toMatchObject({
      credentialId: "sample-phone",
      role: "comment",
      basis: 1,
    })
    expect(store.getState().workspace.share?.grants).toMatchObject([
      { credentialId: "sample-phone", role: "comment", policyRevision: 1 },
    ])
  })

  it("D5: a refused change is said, and the grants are read again all the same", async () => {
    const sharing = inMemorySharing({ now: testNow })
    const store = storeWith(sharing)
    await store.dispatch(openShare({ sessionId: "a" }))
    await store.dispatch(
      shareDraft({ credentialId: "not-linked", role: "read", basis: null }),
    )
    expect(store.getState().workspace.share?.settled).toEqual({
      change: {
        kind: "share",
        request: { credentialId: "not-linked", role: "read", expectedPolicyRevision: 0 },
      },
      outcome: "not-paired",
      // The read after it answered, so the list no longer predates it.
      listed: true,
    })
    expect(sharing.calls.map((call) => call.method)).toEqual([
      "grants",
      "choices",
      "share",
      "grants",
      "choices",
    ])
  })

  it("D5, D10: a change to a grant another window moved is refused as changed, and nothing is changed", async () => {
    const sharing = inMemorySharing({ now: testNow })
    const store = storeWith(sharing)
    await sharing.share(
      "a",
      { credentialId: "sample-phone", role: "read", expectedPolicyRevision: 0 },
      "seed",
    )
    await store.dispatch(openShare({ sessionId: "a" }))
    const read = store.getState().workspace.share?.grants?.[0]
    // Another window changes it after this one read it.
    await sharing.share(
      "a",
      {
        credentialId: "sample-phone",
        role: "drive",
        policy: { kind: "preset", preset: "all", commandHosts: [] },
        expectedPolicyRevision: read!.policyRevision,
      },
      "other",
    )
    await store.dispatch(
      shareDraft({
        credentialId: "sample-phone",
        role: "comment",
        basis: read!.policyRevision,
      }),
    )
    expect(store.getState().workspace.share?.settled?.outcome).toBe("changed")
    expect(store.getState().workspace.share?.grants).toMatchObject([
      { credentialId: "sample-phone", role: "drive" },
    ])
  })

  it("D11: a Revoke names the revision listed when it was asked, and is refused once another window moved it", async () => {
    const sharing = inMemorySharing({ now: testNow })
    const store = storeWith(sharing)
    await sharing.share(
      "a",
      { credentialId: "sample-phone", role: "read", expectedPolicyRevision: 0 },
      "seed",
    )
    await store.dispatch(openShare({ sessionId: "a" }))
    store.dispatch(askRevoke({ credentialId: "sample-phone" }))
    expect(store.getState().workspace.share?.confirming).toEqual({
      credentialId: "sample-phone",
      policyRevision: 1,
    })
    // Another window changes it while this one asks.
    await sharing.share(
      "a",
      { credentialId: "sample-phone", role: "comment", expectedPolicyRevision: 1 },
      "other",
    )
    await store.dispatch(revokeShare())
    expect(sharing.calls.find((call) => call.method === "unshare")).toMatchObject({
      expectedPolicyRevision: 1,
    })
    expect(store.getState().workspace.share).toMatchObject({
      settled: { outcome: "changed" },
      confirming: null,
      grants: [{ credentialId: "sample-phone", role: "comment" }],
    })
  })

  it("D10: a first grant names revision 0, and is refused once another window granted the device first", async () => {
    const sharing = inMemorySharing({ now: testNow })
    const store = storeWith(sharing)
    await store.dispatch(openShare({ sessionId: "a" }))
    expect(store.getState().workspace.share?.grants).toEqual([])
    // Another window grants the device after this one read it held none.
    await sharing.share(
      "a",
      {
        credentialId: "sample-phone",
        role: "drive",
        policy: { kind: "preset", preset: "all", commandHosts: [] },
        expectedPolicyRevision: 0,
      },
      "other",
    )
    await store.dispatch(
      shareDraft({
        credentialId: "sample-phone",
        role: "read",
        basis: null,
      }),
    )
    expect(sharing.calls.at(-3)).toMatchObject({
      method: "share",
      credentialId: "sample-phone",
      expectedPolicyRevision: 0,
    })
    expect(store.getState().workspace.share?.settled?.outcome).toBe("changed")
    expect(store.getState().workspace.share?.grants).toMatchObject([
      { credentialId: "sample-phone", role: "drive" },
    ])
  })

  it("D4: a failed read says why, and Try Again reads it", async () => {
    let refuse = true
    const sharing = inMemorySharing({
      now: testNow,
      refuse: (method) => (refuse && method === "grants" ? "busy" : undefined),
    })
    const store = storeWith(sharing)
    await store.dispatch(openShare({ sessionId: "a" }))
    expect(store.getState().workspace.share?.readFailure).toEqual({
      part: "grants",
      reason: "busy",
    })
    refuse = false
    await store.dispatch(readShares())
    expect(store.getState().workspace.share).toMatchObject({
      readFailure: null,
      grants: [],
    })
  })

  it("D4: the devices and hosts failing leaves the grants, and a revoke of one", async () => {
    const sharing = inMemorySharing({
      now: testNow,
      refuse: (method) => (method === "choices" ? "unanswered" : undefined),
    })
    const store = storeWith(sharing)
    await sharing.share(
      "a",
      { credentialId: "sample-phone", role: "read", expectedPolicyRevision: 0 },
      "seed",
    )
    await store.dispatch(openShare({ sessionId: "a" }))
    expect(store.getState().workspace.share).toMatchObject({
      grants: [{ credentialId: "sample-phone" }],
      choices: null,
      readFailure: { part: "choices", reason: "unanswered" },
    })
    await store.dispatch(revoke("sample-phone"))
    expect(store.getState().workspace.share?.grants).toEqual([])
  })

  it("D6: a second change while one is on its way sends nothing", async () => {
    const { source, waiting } = heldSharing()
    const store = storeWith(source)
    void store.dispatch(openShare({ sessionId: "a" }))
    waiting.shift()!.resolve(grantsOf("x", "y"))
    await settle()
    void store.dispatch(revoke("x"))
    void store.dispatch(revoke("y"))
    void store.dispatch(shareDraft({ credentialId: "z", role: "read", basis: null }))
    await settle()
    expect(waiting.map((call) => [call.method, call.args[1]])).toEqual([["unshare", "x"]])
    expect(store.getState().workspace.shareChange).toEqual({
      sessionId: "a",
      change: { kind: "revoke", credentialId: "x", expectedPolicyRevision: 1 },
    })
  })

  it("D6: nothing is changed before the grants were read", async () => {
    const { source, waiting } = heldSharing()
    const store = storeWith(source)
    void store.dispatch(openShare({ sessionId: "a" }))
    await store.dispatch(revoke("x"))
    expect(waiting.map((call) => call.method)).toEqual(["grants"])
  })

  it("D2: a read that a later one overtook is let go when it lands", async () => {
    const { source, waiting } = heldSharing()
    const store = storeWith(source)
    void store.dispatch(openShare({ sessionId: "a" }))
    void store.dispatch(readShares())
    await settle()
    const [first, second] = waiting.splice(0)
    second!.resolve(grantsOf("new"))
    await settle()
    first!.resolve(grantsOf("old"))
    await settle()
    expect(store.getState().workspace.share?.grants).toEqual(grantsOf("new"))
  })

  it("D7: closing lets a change's answer go, and no read follows it", async () => {
    const { source, waiting } = heldSharing()
    const store = storeWith(source)
    void store.dispatch(openShare({ sessionId: "a" }))
    waiting.shift()!.resolve(grantsOf("x"))
    await settle()
    const revoking = store.dispatch(revoke("x"))
    await settle()
    store.dispatch(closeShare())
    expect(store.getState().workspace.share).toBeNull()
    waiting.shift()!.reject(new SharingError("busy"))
    await revoking
    expect(store.getState().workspace.share).toBeNull()
    expect(store.getState().workspace.shareChange).toBeNull()
    expect(waiting).toEqual([])
  })

  it("D6: a dialog opened again begins no change while one from before is on its way", async () => {
    const { source, waiting } = heldSharing()
    const store = storeWith(source)
    void store.dispatch(openShare({ sessionId: "a" }))
    waiting.shift()!.resolve(grantsOf("x"))
    await settle()
    const revoking = store.dispatch(revoke("x"))
    await settle()
    store.dispatch(closeShare())
    void store.dispatch(openShare({ sessionId: "a" }))
    await settle()
    waiting
      .splice(
        waiting.findIndex((call) => call.method === "grants"),
        1,
      )[0]!
      .resolve(grantsOf("x"))
    await settle()
    await store.dispatch(shareDraft({ credentialId: "x", role: "drive", basis: null }))
    expect(waiting.map((call) => call.method)).toEqual(["unshare"])
    waiting.shift()!.resolve(undefined)
    await settle()
    // Settled: the dialog opened since says so, and reads its grants again.
    expect(store.getState().workspace.share?.settled).toMatchObject({ outcome: "done" })
    expect(waiting.map((call) => call.method)).toEqual(["grants"])
    waiting.shift()!.resolve(grantsOf("x"))
    await revoking
    // Read again: the dialog opened since may change again.
    void store.dispatch(revoke("x"))
    await settle()
    expect(waiting.map((call) => call.method)).toEqual(["unshare"])
  })

  it("D8: a new opening lets the earlier opening's answers go", async () => {
    const { source, waiting } = heldSharing()
    const store = storeWith(source)
    void store.dispatch(openShare({ sessionId: "a" }))
    void store.dispatch(openShare({ sessionId: "b" }))
    await settle()
    const [forA, forB] = waiting.splice(0)
    expect(forA?.args[0]).toBe("a")
    forA!.resolve(grantsOf("a-grant"))
    await settle()
    expect(store.getState().workspace.share).toMatchObject({
      sessionId: "b",
      grants: null,
    })
    forB!.resolve(grantsOf("b-grant"))
    await settle()
    expect(store.getState().workspace.share?.grants).toEqual(grantsOf("b-grant"))
  })
})
