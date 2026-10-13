/**
 * The share dialog's commands over a store, by the rows of the table in
 * `docs/design/read-grants.md` › "Dialog state and order": what each sends,
 * and which answers land.
 */
import { describe, expect, it } from "vitest"
import { makeDesktopStore } from "../../../store"
import type { SharingSource } from "../../application/ports"
import { SharingError } from "../../application/ports"
import type { SharingRead } from "../../model/sharing"
import { fakeSource, keptFilter, roomyGrid, settle } from "../../testing"
import { inMemorySharing } from "../in-memory/in-memory-sharing"
import { closeShare, openShare, readShares, revokeShare, shareSession } from "./commands"

function storeWith(sharing: SharingSource | undefined) {
  let next = 0
  return makeDesktopStore({
    workspace: fakeSource(),
    now: () => 1000,
    newId: () => `id-${++next}`,
    measure: () => roomyGrid,
    overviewFilter: keptFilter(),
    sharing,
  })
}

/** A source whose every call waits until the test answers it. */
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
    read: hold("read"),
    share: hold("share"),
    unshare: hold("unshare"),
  } as unknown as SharingSource
  return { source, waiting }
}

const limits = { patterns: 16, hosts: 16, hostBytes: 96, entries: 32 }
const readOf = (...credentialIds: string[]): SharingRead => ({
  grants: credentialIds.map((credentialId) => ({
    credentialId,
    role: "read",
    policyRevision: 0,
    grantedAtMs: 1,
  })),
  targets: [{ credentialId: "sample-phone", issuedAt: 1 }],
  commandHosts: [],
  limits,
})

describe("sharing from the store", () => {
  it("D1: a window without sharing offers none and opens nothing", async () => {
    const store = storeWith(undefined)
    expect(store.getState().workspace.shareable).toBe(false)
    await store.dispatch(openShare({ sessionId: "a" }))
    expect(store.getState().workspace.share).toBeNull()
  })

  it("D1, D5: opens, reads, shares, and reads again", async () => {
    const sharing = inMemorySharing({ now: () => 7 })
    const store = storeWith(sharing)
    expect(store.getState().workspace.shareable).toBe(true)
    await store.dispatch(openShare({ sessionId: "a" }))
    expect(store.getState().workspace.share?.value?.grants).toEqual([])
    await store.dispatch(
      shareSession({ request: { credentialId: "sample-phone", role: "comment" } }),
    )
    const dialog = store.getState().workspace.share
    expect(dialog?.notice?.outcome).toBe("done")
    expect(dialog?.value?.grants).toEqual([
      {
        credentialId: "sample-phone",
        role: "comment",
        policyRevision: 0,
        grantedAtMs: 7,
      },
    ])
    expect(sharing.calls.map((call) => call.method)).toEqual(["read", "share", "read"])
    // Each change carries a request id of its own.
    expect(sharing.calls[1]?.requestId).toEqual(expect.any(String))
    await store.dispatch(revokeShare({ credentialId: "sample-phone" }))
    expect(store.getState().workspace.share?.value?.grants).toEqual([])
    expect(sharing.calls.at(-2)).toMatchObject({
      method: "unshare",
      sessionId: "a",
      credentialId: "sample-phone",
    })
  })

  it("D5: a refused change is said, and the grants are read again all the same", async () => {
    const sharing = inMemorySharing()
    const store = storeWith(sharing)
    await store.dispatch(openShare({ sessionId: "a" }))
    await store.dispatch(
      shareSession({ request: { credentialId: "not-linked", role: "read" } }),
    )
    expect(store.getState().workspace.share?.notice).toEqual({
      change: { kind: "share", request: { credentialId: "not-linked", role: "read" } },
      outcome: "not-paired",
    })
    expect(sharing.calls.map((call) => call.method)).toEqual(["read", "share", "read"])
  })

  it("D4: a failed read says why, and Try Again reads it", async () => {
    let refuse = true
    const sharing = inMemorySharing({
      refuse: (method) => (refuse && method === "read" ? "busy" : undefined),
    })
    const store = storeWith(sharing)
    await store.dispatch(openShare({ sessionId: "a" }))
    expect(store.getState().workspace.share?.readFailure).toBe("busy")
    refuse = false
    await store.dispatch(readShares())
    expect(store.getState().workspace.share).toMatchObject({
      readFailure: null,
      value: { grants: [] },
    })
  })

  it("D6: a second change while one is on its way sends nothing", async () => {
    const { source, waiting } = heldSharing()
    const store = storeWith(source)
    void store.dispatch(openShare({ sessionId: "a" }))
    waiting.shift()!.resolve(readOf())
    await settle()
    void store.dispatch(revokeShare({ credentialId: "x" }))
    void store.dispatch(revokeShare({ credentialId: "y" }))
    void store.dispatch(shareSession({ request: { credentialId: "z", role: "read" } }))
    await settle()
    expect(waiting.map((call) => [call.method, call.args[1]])).toEqual([["unshare", "x"]])
    expect(store.getState().workspace.shareChange).toEqual({
      sessionId: "a",
      change: { kind: "revoke", credentialId: "x" },
    })
  })

  it("D6: nothing is changed before the grants were read", async () => {
    const { source, waiting } = heldSharing()
    const store = storeWith(source)
    void store.dispatch(openShare({ sessionId: "a" }))
    await store.dispatch(revokeShare({ credentialId: "x" }))
    expect(waiting.map((call) => call.method)).toEqual(["read"])
  })

  it("D2: a read that a later one overtook is let go when it lands", async () => {
    const { source, waiting } = heldSharing()
    const store = storeWith(source)
    void store.dispatch(openShare({ sessionId: "a" }))
    void store.dispatch(readShares())
    await settle()
    const [first, second] = waiting.splice(0)
    second!.resolve(readOf("new"))
    await settle()
    first!.resolve(readOf("old"))
    await settle()
    expect(store.getState().workspace.share?.value).toEqual(readOf("new"))
  })

  it("D7: closing lets a change's answer go, and no read follows it", async () => {
    const { source, waiting } = heldSharing()
    const store = storeWith(source)
    void store.dispatch(openShare({ sessionId: "a" }))
    waiting.shift()!.resolve(readOf("x"))
    await settle()
    const revoking = store.dispatch(revokeShare({ credentialId: "x" }))
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
    waiting.shift()!.resolve(readOf("x"))
    await settle()
    const revoking = store.dispatch(revokeShare({ credentialId: "x" }))
    await settle()
    store.dispatch(closeShare())
    void store.dispatch(openShare({ sessionId: "a" }))
    await settle()
    waiting
      .splice(
        waiting.findIndex((call) => call.method === "read"),
        1,
      )[0]!
      .resolve(readOf("x"))
    await settle()
    await store.dispatch(shareSession({ request: { credentialId: "x", role: "drive" } }))
    expect(waiting.map((call) => call.method)).toEqual(["unshare"])
    waiting.shift()!.resolve(undefined)
    await settle()
    // Settled: the dialog opened since says so, and reads its grants again.
    expect(store.getState().workspace.share?.notice).toMatchObject({ outcome: "done" })
    expect(waiting.map((call) => call.method)).toEqual(["read"])
    waiting.shift()!.resolve(readOf("x"))
    await revoking
    // Read again: the dialog opened since may change again.
    void store.dispatch(revokeShare({ credentialId: "x" }))
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
    forA!.resolve(readOf("a-grant"))
    await settle()
    expect(store.getState().workspace.share).toMatchObject({
      sessionId: "b",
      value: null,
    })
    forB!.resolve(readOf("b-grant"))
    await settle()
    expect(store.getState().workspace.share?.value).toEqual(readOf("b-grant"))
  })
})
