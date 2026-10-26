/** The index commands: loading, a read overtaken, following, marking read, and a resync. */
import { describe, expect, it, vi } from "vitest"
import { WorkspaceSourceError } from "../../application/ports"

import { emptyTranscript } from "../../model/transcript"
import type { WorkspaceIndex } from "../../model/workspace-index"
import {
  fakeSource,
  ready,
  settle,
  shown,
  summary,
  testIndex,
  testStore,
} from "../../testing"

import {
  followWorkspace,
  loadWorkspace,
  openBeside,
  openSession,
  retryTranscript,
} from "./commands"

describe("loading", () => {
  it("opens on the index, and reads the conversation the pane shows", async () => {
    const { store, source } = await ready()
    expect(shown(store)).toEqual(["a"])
    expect(source.calls).toContainEqual(["transcript", "a"])
    expect(store.getState().workspace.transcripts.a).toEqual({
      ...emptyTranscript("a"),
      revision: 1,
    })
  })

  it("says why when the index cannot be read", async () => {
    const source = fakeSource()
    source.refuse("index", "unavailable")
    const { store } = await ready(source)
    expect(store.getState().workspace.status).toBe("failed")
    expect(store.getState().workspace.failure).toBe("unavailable")
  })

  it("says why a shown conversation cannot be read", async () => {
    const source = fakeSource()
    source.refuse("transcript", "unknown-session")
    const { store } = await ready(source)
    expect(store.getState().workspace.transcriptFailures.a).toBe("unknown-session")
  })

  it("reads a conversation once however often the panes change while it is read", async () => {
    const source = fakeSource()
    const { store } = await ready(source)
    source.hold("transcript")
    store.dispatch(openBeside({ sessionId: "c" }))
    store.dispatch(openSession({ sessionId: "a" }))
    store.dispatch(openSession({ sessionId: "c" }))
    await source.release("transcript")
    await settle()
    expect(
      source.calls.filter((call) => call[0] === "transcript" && call[1] === "c"),
    ).toHaveLength(1)
  })
})

describe("reads that fail or are overtaken", () => {
  it("keeps the newer conversation when a read begun earlier answers after its pane closed and reopened", async () => {
    const source = fakeSource()
    const { store } = await ready(source)
    store.dispatch(followWorkspace())
    source.hold("transcript")
    source.transcripts.set("c", { ...emptyTranscript("c"), revision: 2 })
    store.dispatch(openBeside({ sessionId: "c" }))
    await settle()
    source.emit({
      kind: "transcript",
      transcript: { ...emptyTranscript("c"), revision: 3 },
    })
    store.dispatch(openSession({ sessionId: "a" }))
    store.dispatch(openSession({ sessionId: "c" }))
    await source.release("transcript")
    await settle()
    expect(store.getState().workspace.transcripts.c.revision).toBe(3)
  })

  it("lets go of a read begun before its session was removed, and reads it afresh when listed again", async () => {
    const source = fakeSource()
    const { store } = await ready(source)
    store.dispatch(followWorkspace())
    // The first read of "b" answers only when the test says, and refuses.
    const read = source.transcript
    let refuseFirst: () => void = () => {}
    source.transcript = (sessionId) => {
      if (
        sessionId !== "b" ||
        source.calls.some((call) => call[0] === "transcript" && call[1] === "b")
      )
        return read(sessionId)
      source.calls.push(["transcript", "b"])
      return new Promise((_, reject) => {
        refuseFirst = () => reject(new WorkspaceSourceError("unknown-session"))
      })
    }
    store.dispatch(openSession({ sessionId: "b" }))
    source.emit({ kind: "session-removed", sessionId: "b", revision: 2 })
    source.emit({
      kind: "session",
      session: summary("b", "desktop", 900, "idle", { revision: 3 }),
    })
    source.hold("transcript")
    store.dispatch(openSession({ sessionId: "b" }))
    await settle()
    // The old read answers first: it belongs to the session as it was, and is let go.
    refuseFirst()
    await settle()
    expect(store.getState().workspace.transcriptFailures.b).toBeUndefined()
    await source.release("transcript")
    await settle()
    expect(
      source.calls.filter((call) => call[0] === "transcript" && call[1] === "b"),
    ).toHaveLength(2)
    expect(store.getState().workspace.transcriptFailures.b).toBeUndefined()
    expect(store.getState().workspace.transcripts.b).toBeDefined()
  })

  it("asks again for a conversation whose read threw instead of answering", async () => {
    const source = fakeSource()
    const read = source.transcript
    source.transcript = (sessionId) => {
      if (sessionId === "a") throw new WorkspaceSourceError("unavailable")
      return read(sessionId)
    }
    const { store } = await ready(source)
    expect(store.getState().workspace.transcriptFailures.a).toBe("unavailable")
  })

  it("logs a failed read mark that is anything but the source's typed refusal", async () => {
    const source = fakeSource()
    source.markRead = () => Promise.reject(new TypeError("broken"))
    const { store } = await ready(source)
    const error = console.error
    const warn = console.warn
    const logged: unknown[] = []
    console.error = (...args: unknown[]) => void logged.push(args)
    console.warn = () => {}
    try {
      store.dispatch(openSession({ sessionId: "b" }))
      await settle()
    } finally {
      console.error = error
      console.warn = warn
    }
    expect(logged).toHaveLength(1)
  })

  it("offers Try Again for a read the window cannot use, logged as a fault", async () => {
    const source = fakeSource()
    const read = source.transcript
    source.transcript = (sessionId) =>
      read(sessionId).then((transcript) => ({ ...transcript, revision: 0 }))
    const error = console.error
    const logged: unknown[] = []
    console.error = (...args: unknown[]) => void logged.push(args)
    try {
      const { store } = await ready(source)
      expect(store.getState().workspace.transcriptFailures.a).toBe("unavailable")
    } finally {
      console.error = error
    }
    expect(logged).toHaveLength(1)
  })

  it("logs a read that fails with anything but the source's typed refusal", async () => {
    const source = fakeSource()
    source.transcript = () => Promise.reject(new Error("socket closed"))
    const error = console.error
    const logged: unknown[] = []
    console.error = (...args: unknown[]) => void logged.push(args)
    try {
      const { store } = await ready(source)
      expect(store.getState().workspace.transcriptFailures.a).toBe("unavailable")
    } finally {
      console.error = error
    }
    expect(logged).toHaveLength(1)
  })

  it("reads a refused conversation again only when asked, not on every update", async () => {
    const source = fakeSource()
    source.refuse("transcript", "unavailable")
    const { store } = await ready(source)
    const reads = () =>
      source.calls.filter((call) => call[0] === "transcript" && call[1] === "a").length
    expect(reads()).toBe(1)
    store.dispatch(followWorkspace())
    source.emit({
      kind: "session",
      session: summary("d", "gateway", 999, "running", { revision: 2 }),
    })
    store.dispatch(openBeside({ sessionId: "c" }))
    await settle()
    expect(reads()).toBe(1)
    source.refuse("transcript", undefined)
    store.dispatch(retryTranscript({ sessionId: "a" }))
    await settle()
    expect(reads()).toBe(2)
    expect(store.getState().workspace.transcripts.a).toBeDefined()
    // Only the pane opened while reads were refused still says so.
    expect(Object.keys(store.getState().workspace.transcriptFailures)).toEqual(["c"])
  })

  it("keeps an update that arrives while the conversation is being read", async () => {
    const source = fakeSource()
    source.hold("transcript")
    const store = testStore(source)
    store.dispatch(followWorkspace())
    await store.dispatch(loadWorkspace())
    source.emit({
      kind: "transcript",
      transcript: {
        ...emptyTranscript("a"),
        revision: 3,
        activity: { label: "Thinking", since: 1 },
      },
    })
    await source.release("transcript")
    await settle()
    expect(store.getState().workspace.transcripts.a).toMatchObject({
      revision: 3,
      activity: { label: "Thinking" },
    })
  })

  it("keeps what the stream said before the index was read", async () => {
    const source = fakeSource()
    source.hold("index")
    const store = testStore(source)
    store.dispatch(followWorkspace())
    const loading = store.dispatch(loadWorkspace())
    source.emit({ kind: "session-removed", sessionId: "c", revision: 2 })
    source.emit({
      kind: "session",
      session: summary("a", "desktop", 999, "idle", { revision: 4 }),
    })
    await source.release("index")
    await loading
    const workspace = store.getState().workspace
    expect(workspace.sessions.c).toBeUndefined()
    expect(workspace.sessions.a).toMatchObject({ status: "idle", revision: 4 })
  })
})

describe("following the source", () => {
  it("applies updates until unsubscribed", async () => {
    const { store, source } = await ready()
    const stop = store.dispatch(followWorkspace())
    source.emit({
      kind: "session",
      session: summary("a", "desktop", 999, "idle", { revision: 2 }),
    })
    expect(store.getState().workspace.sessions.a.status).toBe("idle")
    stop()
    source.emit({
      kind: "session",
      session: summary("a", "desktop", 999, "running", { revision: 3 }),
    })
    expect(store.getState().workspace.sessions.a.status).toBe("idle")
  })
})

describe("marking read", () => {
  const markReads = (source: ReturnType<typeof fakeSource>) =>
    source.calls.filter((call) => call[0] === "markRead")

  it("tells the source when an unread session is opened, and only then", async () => {
    const { store, source } = await ready()
    store.dispatch(openSession({ sessionId: "b" }))
    store.dispatch(openSession({ sessionId: "c" }))
    await settle()
    expect(markReads(source)).toEqual([["markRead", "b"]])
    expect(store.getState().workspace.sessions.b.unread).toBe(false)
  })

  it("tells the source once for each shown session read in one change", async () => {
    const { store, source } = await ready()
    store.dispatch(openBeside({ sessionId: "c" }))
    await settle()
    const index = testIndex()
    // One read of the index marks both shown sessions unread at once.
    source.index = () =>
      Promise.resolve({
        ...index,
        sessions: index.sessions.map((session) =>
          session.id === "a" || session.id === "c"
            ? { ...session, unread: true, revision: 2 }
            : session,
        ),
      })
    await store.dispatch(loadWorkspace())
    await settle()
    expect(markReads(source).sort()).toEqual([
      ["markRead", "a"],
      ["markRead", "c"],
    ])
  })

  it("marks the session the workspace opens on read, here and at the source", async () => {
    const index = testIndex()
    const source = fakeSource({
      ...index,
      sessions: index.sessions.map((session) =>
        session.id === "a" ? { ...session, unread: true } : session,
      ),
    })
    const { store } = await ready(source)
    expect(store.getState().workspace.sessions.a.unread).toBe(false)
    expect(markReads(source)).toEqual([["markRead", "a"]])
  })

  it("reads a shown session the source marks unread again", async () => {
    const { store, source } = await ready()
    store.dispatch(followWorkspace())
    source.emit({
      kind: "session",
      session: summary("a", "desktop", 900, "idle", { unread: true, revision: 2 }),
    })
    await settle()
    expect(store.getState().workspace.sessions.a.unread).toBe(false)
    expect(markReads(source)).toEqual([["markRead", "a"]])
  })

  it("keeps the mark cleared when the source refuses", async () => {
    const source = fakeSource()
    source.refuse("markRead", "unavailable")
    const { store } = await ready(source)
    const warn = console.warn
    console.warn = () => {}
    store.dispatch(openSession({ sessionId: "b" }))
    await settle()
    console.warn = warn
    expect(store.getState().workspace.sessions.b.unread).toBe(false)
  })
})

describe("the index read again", () => {
  it("takes out what it no longer lists and reads every shown conversation again", async () => {
    const { store, source } = await ready()
    store.dispatch(openBeside({ sessionId: "c" }))
    await settle()
    const reads = () => source.calls.filter((call) => call[0] === "transcript").length
    const before = reads()
    const index = testIndex()
    // The stream lost c's removal; the next read of the index does not list it.
    source.index = () =>
      Promise.resolve({
        ...index,
        sessions: index.sessions.filter((session) => session.id !== "c"),
      })
    await store.dispatch(loadWorkspace())
    await settle()
    expect(store.getState().workspace.sessions.c).toBeUndefined()
    expect(shown(store)).toEqual(["a"])
    // The one shown conversation, read again although it was held.
    expect(reads()).toBe(before + 1)
    expect(source.calls.at(-1)).toEqual(["transcript", "a"])
  })

  it("lets an update the source could not have sent go, and says so", async () => {
    const source = fakeSource()
    const store = testStore(source)
    store.dispatch(followWorkspace())
    await store.dispatch(loadWorkspace())
    await settle()
    // Freeze elapsed-time diagnostics while checking the source warnings exactly.
    const clock = vi.spyOn(Date, "now").mockReturnValue(1000)
    const warn = console.warn
    const logged: unknown[][] = []
    console.warn = (...args: unknown[]) => void logged.push(args)
    const before = store.getState().workspace
    try {
      source.emit({ kind: "session-removed", sessionId: "a", revision: 0 })
      source.emit({
        kind: "session",
        session: summary("a", "desktop", 1, "idle", { revision: -1 }),
      })
      source.emit({
        kind: "transcript",
        transcript: {
          ...emptyTranscript("a"),
          revision: 0,
          messages: [
            {
              id: "m1",
              role: "user",
              at: 1,
              parts: [{ kind: "text", text: "the release keychain password" }],
            },
          ],
        },
      })
    } finally {
      console.warn = warn
      clock.mockRestore()
    }
    expect(store.getState().workspace).toBe(before)
    expect(logged).toHaveLength(3)
    // Said by what it is and whose, never with what it holds.
    expect(logged[2]).toEqual([
      "The workspace source sent an update it could not have sent",
      { kind: "transcript", sessionId: "a", revision: 0 },
    ])
    expect(JSON.stringify(logged)).not.toContain("keychain password")
  })

  it("says, by id, what an index that contradicts itself had left out", async () => {
    const source = fakeSource()
    const index = testIndex()
    source.index = () =>
      Promise.resolve({
        ...index,
        channels: [
          ...index.channels,
          { ...index.channels[0], id: "orphan", sectionId: "gone" },
        ],
        sessions: [...index.sessions, summary("stranded", "orphan", 1)],
      })
    // Freeze elapsed-time diagnostics while checking the source warnings exactly.
    const clock = vi.spyOn(Date, "now").mockReturnValue(1000)
    const warn = console.warn
    const logged: unknown[][] = []
    console.warn = (...args: unknown[]) => void logged.push(args)
    try {
      const { store } = await ready(source)
      expect(store.getState().workspace.sessions.stranded).toBeUndefined()
    } finally {
      console.warn = warn
      clock.mockRestore()
    }
    expect(logged).toEqual([
      [
        "The index contradicted itself; left out, by id:",
        { sections: [], channels: ["orphan"], sessions: ["stranded"] },
      ],
    ])
  })

  it("says so when the index lists a session at a revision the source could not have sent", async () => {
    const source = fakeSource()
    const index = testIndex()
    source.index = () =>
      Promise.resolve({
        ...index,
        sessions: [
          ...index.sessions,
          summary("z", "desktop", 1, "idle", { revision: 0 }),
        ],
      })
    // Freeze elapsed-time diagnostics while checking the source warnings exactly.
    const clock = vi.spyOn(Date, "now").mockReturnValue(1000)
    const warn = console.warn
    const logged: unknown[][] = []
    console.warn = (...args: unknown[]) => void logged.push(args)
    try {
      const { store } = await ready(source)
      expect(store.getState().workspace.sessions.z).toBeUndefined()
    } finally {
      console.warn = warn
      clock.mockRestore()
    }
    expect(logged).toEqual([
      ["The index listed sessions at a revision the source could not have sent", ["z"]],
    ])
  })
})

describe("the source's resync", () => {
  it("reads the index again when the source says its stream may have lost updates", async () => {
    const { store, source } = await ready()
    store.dispatch(followWorkspace())
    const index = testIndex()
    // c was archived while the stream was down; the source says so by a resync.
    source.index = () =>
      Promise.resolve({
        ...index,
        sessions: index.sessions.filter((session) => session.id !== "c"),
      })
    source.emit({ kind: "resync" })
    await settle(10)
    expect(store.getState().workspace.sessions.c).toBeUndefined()
  })

  it("reads a shown conversation again even with a read of it asked before the resync still on its way", async () => {
    const source = fakeSource()
    source.hold("transcript")
    const store = testStore(source)
    store.dispatch(followWorkspace())
    await store.dispatch(loadWorkspace())
    await settle()
    const reads = () =>
      source.calls.filter((call) => call[0] === "transcript" && call[1] === "a").length
    expect(reads()).toBe(1)
    source.emit({ kind: "resync" })
    await settle(10)
    // The first read may predate what was lost: it is set aside, and a is read again.
    expect(reads()).toBe(2)
    source.transcripts.set("a", { ...emptyTranscript("a"), revision: 4 })
    await source.release("transcript")
    await settle()
    expect(store.getState().workspace.transcripts.a?.revision).toBe(4)
  })

  it("pairs each index answer with the read that asked it, answered in either order", async () => {
    const source = fakeSource()
    const store = testStore(source)
    store.dispatch(followWorkspace())
    await store.dispatch(loadWorkspace())
    await settle()
    const answers: {
      resolve: (index: WorkspaceIndex) => void
      reject: (error: unknown) => void
    }[] = []
    source.index = () => {
      source.calls.push(["index"])
      return new Promise((resolve, reject) => answers.push({ resolve, reject }))
    }
    const readsOf = (id: string) =>
      source.calls.filter((call) => call[0] === "transcript" && call[1] === id).length
    source.emit({ kind: "resync" }) // the first read of the index
    await settle()
    // Opened between the two reads, c is read; the second read is asked after it.
    source.hold("transcript")
    store.dispatch(openBeside({ sessionId: "c" }))
    await settle()
    const before = readsOf("c")
    source.emit({ kind: "resync" }) // the second
    await settle()
    // The second answers first: c's read was asked before it, so it is set aside and asked again.
    answers[1].resolve(testIndex())
    await settle(10)
    expect(readsOf("c")).toBe(before + 1)
    // The first then fails: it changes nothing more.
    answers[0].reject(new WorkspaceSourceError("unavailable"))
    await settle(10)
    expect(readsOf("c")).toBe(before + 1)
  })

  it("does not read twice what it reads as it first opens", async () => {
    const { source } = await ready()
    expect(
      source.calls.filter((call) => call[0] === "transcript" && call[1] === "a"),
    ).toHaveLength(1)
  })
})
