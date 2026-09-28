/**
 * The commands through a real store and a fake source: what an agent's
 * dispatch does to the window, and what the source is asked, including when
 * it refuses or answers late.
 */
import { describe, expect, it } from "vitest"
import { WorkspaceSourceError } from "../../application/ports"
import { panesOf } from "../../../split-panes/model/pane-layout"
import { emptyTranscript } from "../../model/transcript"
import type { WorkspaceIndex } from "../../model/workspace-index"
import {
  astra,
  fakeSource,
  keptFilter,
  settle,
  summary,
  testIndex,
  testStore,
} from "../../testing"
import { retention } from "../../model/retention"
import {
  approve,
  archiveSession,
  canNudge,
  canOpenBeside,
  chooseModel,
  closePane,
  deny,
  discardUnsent,
  dropSession,
  fitPanes,
  followWorkspace,
  loadWorkspace,
  movePane,
  newSession,
  nudgePane,
  openBeside,
  openChannel,
  openSession,
  pinSession,
  resendMessage,
  resizePanes,
  retryTranscript,
  selectChannel,
  sendMessage,
  showContent,
  focusPane,
  equalizePanes,
  filterOverview,
  selectInOverview,
  setComposerText,
  toggleSessionList,
  toggleSidebar,
} from "./commands"

async function ready(source = fakeSource()) {
  const store = testStore(source)
  await store.dispatch(loadWorkspace())
  await settle()
  return { store, source }
}

const shown = (store: ReturnType<typeof testStore>) =>
  panesOf(store.getState().workspace.panes!).map((pane) => pane.sessionId)

const outboxOf = (store: ReturnType<typeof testStore>, sessionId: string) =>
  store.getState().workspace.outbox[sessionId] ?? []

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

describe("new sessions and closing", () => {
  it("starts a draft under a fresh id, and closes the focused pane by default", async () => {
    const { store } = await ready()
    store.dispatch(newSession({ beside: "right" }))
    const draftId = shown(store)[1]
    expect(store.getState().workspace.drafts[draftId]).toBeDefined()
    store.dispatch(closePane())
    expect(shown(store)).toEqual(["a"])
    expect(store.getState().workspace.drafts).toEqual({})
  })

  it("opens a channel with no sessions on a new session's home", async () => {
    const { store } = await ready()
    store.dispatch(openChannel({ channelId: "empty" }))
    const [shownId] = shown(store)
    expect(store.getState().workspace.drafts[shownId].channelId).toBe("empty")
  })
})

describe("sending", () => {
  it("starts a draft with its first message and asks the source to start it under the same id", async () => {
    const { store, source } = await ready()
    store.dispatch(newSession())
    const [draftId] = shown(store)
    await store.dispatch(
      sendMessage({ initiator: "person", sessionId: draftId, text: "  Fix the rain.  " }),
    )
    const sent = source.calls.find((call) => call[0] === "send")?.[1]
    expect(sent).toMatchObject({
      sessionId: draftId,
      text: "Fix the rain.",
      start: { channelId: "desktop", title: "Fix the rain" },
    })
    const workspace = store.getState().workspace
    expect(workspace.sessions[draftId].title).toBe("Fix the rain")
    expect(outboxOf(store, draftId)[0].delivery).toBeUndefined()
    // Not read before the source has spoken of it: there is nothing to read yet.
    expect(
      source.calls.some((call) => call[0] === "transcript" && call[1] === draftId),
    ).toBe(false)
  })

  it("sends nothing blank, and nothing to a session it does not have", async () => {
    const { store, source } = await ready()
    expect(
      await store.dispatch(
        sendMessage({ initiator: "person", sessionId: "a", text: "   " }),
      ),
    ).toBe("not-asked")
    expect(
      await store.dispatch(
        sendMessage({ initiator: "agent", sessionId: "missing", text: "hi" }),
      ),
    ).toBe("not-asked")
    expect(source.calls.some((call) => call[0] === "send")).toBe(false)
  })

  it("shows the message as sending until the source takes it", async () => {
    const source = fakeSource()
    const { store } = await ready(source)
    source.hold("send")
    const sending = store.dispatch(
      sendMessage({ initiator: "person", sessionId: "a", text: "hi" }),
    )
    expect(outboxOf(store, "a").at(-1)?.delivery).toEqual({ state: "sending" })
    await source.release("send")
    await sending
    expect(outboxOf(store, "a").at(-1)?.delivery).toBeUndefined()
  })

  it("keeps a message sent while the history is still being read", async () => {
    const source = fakeSource()
    source.transcripts.set("c", { ...emptyTranscript("c"), revision: 5 })
    const { store } = await ready(source)
    source.hold("transcript")
    store.dispatch(openSession({ sessionId: "c" }))
    await store.dispatch(sendMessage({ initiator: "person", sessionId: "c", text: "hi" }))
    await source.release("transcript")
    await settle()
    const workspace = store.getState().workspace
    expect(workspace.transcripts.c.revision).toBe(5)
    expect(outboxOf(store, "c").map((message) => message.parts[0])).toEqual([
      { kind: "text", text: "hi" },
    ])
  })

  it("keeps a draft refused on its first message listed, and starts it when sent again", async () => {
    const source = fakeSource()
    source.refuse("send", "unavailable")
    const { store } = await ready(source)
    store.dispatch(newSession())
    const [draftId] = shown(store)
    await store.dispatch(
      sendMessage({ initiator: "person", sessionId: draftId, text: "Fix the rain" }),
    )
    const workspace = store.getState().workspace
    expect(workspace.drafts).toEqual({})
    expect(workspace.sessions[draftId]).toMatchObject({
      title: "Fix the rain",
      status: "idle",
      revision: 0,
    })
    expect(outboxOf(store, draftId)[0].delivery).toMatchObject({ state: "failed" })
    source.refuse("send", undefined)
    await store.dispatch(
      sendMessage({ initiator: "person", sessionId: draftId, text: "Try again" }),
    )
    const again = source.calls.filter((call) => call[0] === "send").at(-1)?.[1]
    expect(again).toMatchObject({
      sessionId: draftId,
      start: { channelId: "desktop", title: "Fix the rain" },
    })
  })

  it("carries what the session starts with on every message until the source speaks of it", async () => {
    const source = fakeSource()
    const { store } = await ready(source)
    store.dispatch(newSession())
    const [draftId] = shown(store)
    source.hold("send")
    const first = store.dispatch(
      sendMessage({ initiator: "person", sessionId: draftId, text: "Fix the rain" }),
    )
    const second = store.dispatch(
      sendMessage({ initiator: "person", sessionId: draftId, text: "And the steam" }),
    )
    await source.release("send")
    await Promise.all([first, second])
    const starts = source.calls
      .filter((call) => call[0] === "send")
      .map((call) => (call[1] as { start?: unknown }).start)
    expect(starts).toEqual([
      { channelId: "desktop", title: "Fix the rain" },
      { channelId: "desktop", title: "Fix the rain" },
    ])
  })

  it("sends with the model chosen for the next turn", async () => {
    const { store, source } = await ready()
    store.dispatch(chooseModel({ sessionId: "a", model: astra }))
    await store.dispatch(sendMessage({ initiator: "agent", sessionId: "a", text: "hi" }))
    const sent = source.calls.find((call) => call[0] === "send")?.[1]
    expect(sent).toMatchObject({ sessionId: "a", model: astra, initiator: "agent" })
    expect(sent).not.toHaveProperty("start", expect.anything())
  })

  it("tells its caller what became of a message, and of a message sent again", async () => {
    const source = fakeSource()
    const { store } = await ready(source)
    expect(
      await store.dispatch(
        sendMessage({ initiator: "agent", sessionId: "a", text: "one" }),
      ),
    ).toBe("sent")
    source.refuse("send", "unknown-session")
    expect(
      await store.dispatch(
        sendMessage({ initiator: "agent", sessionId: "a", text: "two" }),
      ),
    ).toBe("refused")
    source.refuse("send", "unavailable")
    expect(
      await store.dispatch(
        sendMessage({ initiator: "agent", sessionId: "a", text: "three" }),
      ),
    ).toBe("unknown")
    const [, two] = store.getState().workspace.outbox.a ?? []
    source.refuse("send", undefined)
    expect(
      await store.dispatch(
        resendMessage({ initiator: "agent", sessionId: "a", messageId: two.id }),
      ),
    ).toBe("sent")
    expect(
      await store.dispatch(
        resendMessage({ initiator: "agent", sessionId: "a", messageId: two.id }),
      ),
    ).toBe("not-asked")
  })

  it("marks a refused message as not sent, saying why, and leaves the session as the source said", async () => {
    const source = fakeSource()
    source.refuse("send", "unknown-session")
    const { store } = await ready(source)
    const before = store.getState().workspace.sessions.a
    await store.dispatch(sendMessage({ initiator: "person", sessionId: "a", text: "hi" }))
    expect(outboxOf(store, "a").at(-1)?.delivery).toEqual({
      state: "failed",
      reason: "unknown-session",
    })
    expect(store.getState().workspace.sessions.a).toBe(before)
  })

  it("waits for the source's summary of a new session before holding its conversation", async () => {
    const source = fakeSource()
    const { store } = await ready(source)
    store.dispatch(followWorkspace())
    const draftId = store.dispatch(newSession())
    if (!draftId) throw new Error("no draft")
    source.hold("send")
    const first = store.dispatch(
      sendMessage({ initiator: "person", sessionId: draftId, text: "First" }),
    )
    const [m1] = outboxOf(store, draftId)
    const begun = {
      ...emptyTranscript(draftId),
      revision: 1,
      messages: [{ ...m1, delivery: undefined }],
    }
    source.transcripts.set(draftId, begun)
    // The conversation comes before the summary: not held, so m1 may still begin it.
    source.emit({ kind: "transcript", transcript: begun })
    await source.release("send")
    await first
    source.refuse("send", "unavailable")
    await store.dispatch(
      sendMessage({ initiator: "person", sessionId: draftId, text: "Second" }),
    )
    const refused = outboxOf(store, draftId).at(-1)
    expect(store.getState().workspace.sessions[draftId].status).toBe("running")
    store.dispatch(discardUnsent({ sessionId: draftId, messageId: refused?.id ?? "" }))
    expect(store.getState().workspace.sessions[draftId]).toBeDefined()
    expect(store.getState().workspace.drafts[draftId]).toBeUndefined()
    // The summary arrives: the conversation is read, and it retires m1.
    source.emit({
      kind: "session",
      session: summary(draftId, "desktop", 2000, "running", { revision: 1 }),
    })
    await settle()
    const workspace = store.getState().workspace
    expect(workspace.transcripts[draftId]).toEqual(begun)
    expect(outboxOf(store, draftId)).toEqual([])
    expect(workspace.sessions[draftId].revision).toBe(1)
  })

  it("sends a refused message again in its place and under its id, or lets it go", async () => {
    const source = fakeSource()
    source.refuse("send", "unavailable")
    const { store } = await ready(source)
    await store.dispatch(
      sendMessage({ initiator: "person", sessionId: "a", text: "one" }),
    )
    await store.dispatch(
      sendMessage({ initiator: "person", sessionId: "a", text: "two" }),
    )
    const [one, two] = outboxOf(store, "a")
    source.refuse("send", undefined)
    await store.dispatch(
      resendMessage({ initiator: "person", sessionId: "a", messageId: one.id }),
    )
    const after = outboxOf(store, "a")
    expect(after.map((message) => [message.id, message.delivery?.state])).toEqual([
      [one.id, undefined],
      [two.id, "failed"],
    ])
    const resent = source.calls.filter((call) => call[0] === "send").at(-1)?.[1]
    expect(resent).toMatchObject({ messageId: one.id, text: "one" })
    store.dispatch(discardUnsent({ sessionId: "a", messageId: two.id }))
    expect(outboxOf(store, "a").map((message) => message.id)).toEqual([one.id])
    // Only a refused message is sent again.
    await store.dispatch(
      resendMessage({ initiator: "person", sessionId: "a", messageId: one.id }),
    )
    expect(source.calls.filter((call) => call[0] === "send")).toHaveLength(3)
  })
})

describe("approvals", () => {
  const withApproval = async () => {
    const source = fakeSource()
    source.transcripts.set("b", {
      ...emptyTranscript("b"),
      revision: 1,
      approval: { id: "ap", command: "cargo test", reason: "Runs tests." },
    })
    const result = await ready(source)
    result.store.dispatch(openSession({ sessionId: "b" }))
    await settle()
    return result
  }

  it("answers once, not twice while the first answer is on its way", async () => {
    const { store, source } = await withApproval()
    source.hold("approve")
    const first = store.dispatch(
      approve({ sessionId: "b", approvalId: "ap", scope: "always", initiator: "person" }),
    )
    await store.dispatch(
      approve({ sessionId: "b", approvalId: "ap", initiator: "person" }),
    )
    await source.release("approve")
    await first
    expect(source.calls.filter((call) => call[0] === "approve")).toEqual([
      ["approve", "b", "ap", "always", "person"],
    ])
  })

  it("asks again, saying why, when the answer does not reach the agent", async () => {
    const { store, source } = await withApproval()
    source.refuse("deny", "not-waiting")
    expect(
      await store.dispatch(
        deny({ sessionId: "b", approvalId: "ap", initiator: "person" }),
      ),
    ).toBe("refused")
    expect(store.getState().workspace.answers.b).toMatchObject({
      approvalId: "ap",
      failure: "not-waiting",
    })
    source.refuse("deny", undefined)
    await store.dispatch(deny({ sessionId: "b", approvalId: "ap", initiator: "person" }))
    expect(source.calls.filter((call) => call[0] === "deny")).toHaveLength(2)
    expect(store.getState().workspace.answers.b).toMatchObject({ approvalId: "ap" })
  })

  it("lets the source's newer conversation settle the approval, before the answer resolves", async () => {
    const { store, source } = await withApproval()
    store.dispatch(followWorkspace())
    source.hold("approve")
    const answering = store.dispatch(
      approve({ sessionId: "b", approvalId: "ap", initiator: "person" }),
    )
    expect(store.getState().workspace.answers.b).toMatchObject({ approvalId: "ap" })
    await source.release("approve")
    // Resolved: the conversation that no longer asks has already arrived.
    expect(await answering).toBe("sent")
    expect(store.getState().workspace.transcripts.b.approval).toBeNull()
    expect(store.getState().workspace.answers.b).toBeUndefined()
    expect(
      await store.dispatch(
        approve({ sessionId: "b", approvalId: "ap", initiator: "agent" }),
      ),
    ).toBe("not-asked")
  })

  it("keeps one answer at a time across its pane showing another session and back", async () => {
    const { store, source } = await withApproval()
    source.hold("approve")
    source.refuse("approve", "unavailable")
    const agents = store.dispatch(
      approve({ sessionId: "b", approvalId: "ap", initiator: "agent" }),
    )
    store.dispatch(openSession({ sessionId: "c" }))
    store.dispatch(openSession({ sessionId: "b" }))
    await settle()
    // The agent's answer is still on its way: the person's waits.
    expect(
      await store.dispatch(
        deny({ sessionId: "b", approvalId: "ap", initiator: "person" }),
      ),
    ).toBe("answering")
    await source.release("approve")
    expect(await agents).toBe("unknown")
    source.refuse("approve", undefined)
    expect(
      await store.dispatch(
        deny({ sessionId: "b", approvalId: "ap", initiator: "person" }),
      ),
    ).toBe("sent")
    expect(
      source.calls.filter((call) => call[0] === "approve" || call[0] === "deny"),
    ).toHaveLength(2)
  })

  it("lets an earlier answer's late refusal leave a later answer alone", async () => {
    const { store, source } = await withApproval()
    store.dispatch(followWorkspace())
    source.hold("approve")
    source.refuse("approve", "unavailable")
    const first = store.dispatch(
      approve({ sessionId: "b", approvalId: "ap", initiator: "agent" }),
    )
    // Removed and listed again: what the window held of it is forgotten, then read afresh.
    source.emit({ kind: "session-removed", sessionId: "b", revision: 2 })
    source.emit({
      kind: "session",
      session: summary("b", "desktop", 900, "needs-you", { revision: 3 }),
    })
    store.dispatch(openSession({ sessionId: "b" }))
    await settle()
    source.hold("deny")
    const second = store.dispatch(
      deny({ sessionId: "b", approvalId: "ap", initiator: "person" }),
    )
    await source.release("approve")
    expect(await first).toBe("unknown")
    expect(store.getState().workspace.answers.b?.failure).toBeUndefined()
    await source.release("deny")
    expect(await second).toBe("sent")
  })

  it("tells the source who decided, and the caller what became of it", async () => {
    const { store, source } = await withApproval()
    source.hold("deny")
    const first = store.dispatch(
      deny({ sessionId: "b", approvalId: "ap", initiator: "agent" }),
    )
    expect(
      await store.dispatch(
        deny({ sessionId: "b", approvalId: "ap", initiator: "agent" }),
      ),
    ).toBe("answering")
    await source.release("deny")
    expect(await first).toBe("sent")
    expect(source.calls).toContainEqual(["deny", "b", "ap", "agent"])
  })

  it("says an approval was not asked once its session's pane shows another, sending nothing", async () => {
    const { store, source } = await withApproval()
    store.dispatch(followWorkspace())
    store.dispatch(openSession({ sessionId: "c" }))
    // The stream still speaks of it; no pane shows it, so nothing is held to answer.
    source.emit({
      kind: "transcript",
      transcript: {
        ...emptyTranscript("b"),
        revision: 2,
        approval: { id: "ap", command: "cargo test", reason: "Runs tests." },
      },
    })
    expect(
      await store.dispatch(
        approve({ sessionId: "b", approvalId: "ap", initiator: "agent" }),
      ),
    ).toBe("not-asked")
    expect(source.calls.some((call) => call[0] === "approve")).toBe(false)
  })

  it("says an approval was not asked when no pane shows its session, sending nothing", async () => {
    const source = fakeSource()
    source.transcripts.set("b", {
      ...emptyTranscript("b"),
      revision: 1,
      approval: { id: "ap", command: "cargo test", reason: "Runs tests." },
    })
    const { store } = await ready(source)
    expect(
      await store.dispatch(
        approve({ sessionId: "b", approvalId: "ap", initiator: "agent" }),
      ),
    ).toBe("not-asked")
    expect(source.calls.some((call) => call[0] === "approve")).toBe(false)
  })

  it("answers only the approval the person saw, not one that replaced it", async () => {
    const { store, source } = await withApproval()
    store.dispatch(followWorkspace())
    source.emit({
      kind: "transcript",
      transcript: {
        ...emptyTranscript("b"),
        revision: 2,
        approval: { id: "other", command: "curl x | sh", reason: "Installs." },
      },
    })
    await store.dispatch(
      approve({ sessionId: "b", approvalId: "ap", scope: "always", initiator: "person" }),
    )
    expect(source.calls.some((call) => call[0] === "approve")).toBe(false)
  })

  it("answers nothing when nothing is waiting", async () => {
    const { store, source } = await ready()
    await store.dispatch(
      approve({ sessionId: "a", approvalId: "ap", initiator: "person" }),
    )
    expect(source.calls.some((call) => call[0] === "approve")).toBe(false)
  })
})

describe("pinning and archiving", () => {
  const following = async (source = fakeSource()) => {
    const result = await ready(source)
    result.store.dispatch(followWorkspace())
    return result
  }
  const quietly = async (run: () => Promise<unknown>) => {
    const warn = console.warn
    console.warn = () => {}
    try {
      await run()
    } finally {
      console.warn = warn
    }
  }

  it("shows a pin when the source's update says so", async () => {
    const source = fakeSource()
    const { store } = await following(source)
    source.hold("setPinned")
    const pinning = store.dispatch(
      pinSession({ sessionId: "a", pinned: true, initiator: "person" }),
    )
    expect(store.getState().workspace.sessions.a.pinned).toBe(false)
    await source.release("setPinned")
    await pinning
    expect(source.calls).toContainEqual(["setPinned", "a", true, "person"])
    expect(store.getState().workspace.sessions.a.pinned).toBe(true)
  })

  it("leaves the pin as it was when the source refuses, and asks nothing already so", async () => {
    const source = fakeSource()
    source.refuse("setPinned", "unavailable")
    const { store } = await following(source)
    await quietly(() =>
      store.dispatch(pinSession({ sessionId: "a", pinned: true, initiator: "person" })),
    )
    expect(store.getState().workspace.sessions.a.pinned).toBe(false)
    await store.dispatch(
      pinSession({ sessionId: "d", pinned: true, initiator: "person" }),
    )
    expect(source.calls.filter((call) => call[0] === "setPinned")).toHaveLength(1)
  })

  it("tells the source who asked for a pin or an archive, and the caller what became of it", async () => {
    const { store, source } = await following()
    expect(
      await store.dispatch(
        pinSession({ sessionId: "a", pinned: true, initiator: "agent" }),
      ),
    ).toBe("sent")
    expect(
      await store.dispatch(
        pinSession({ sessionId: "a", pinned: true, initiator: "agent" }),
      ),
    ).toBe("not-asked")
    source.refuse("archive", "unavailable")
    const warn = console.warn
    console.warn = () => {}
    try {
      expect(
        await store.dispatch(archiveSession({ sessionId: "c", initiator: "agent" })),
      ).toBe("unknown")
    } finally {
      console.warn = warn
    }
    expect(source.calls).toContainEqual(["setPinned", "a", true, "agent"])
    expect(source.calls).toContainEqual(["archive", "c", "agent"])
  })

  it("asks nothing to pin a session the source has not spoken of yet", async () => {
    const source = fakeSource()
    source.refuse("send", "unavailable")
    const { store } = await following(source)
    store.dispatch(newSession())
    const [draftId] = shown(store)
    await store.dispatch(
      sendMessage({ initiator: "person", sessionId: draftId, text: "Fix the rain" }),
    )
    await store.dispatch(
      pinSession({ sessionId: draftId, pinned: true, initiator: "person" }),
    )
    expect(source.calls.some((call) => call[0] === "setPinned")).toBe(false)
  })

  it("takes an archived session out on the source's removal: its pane closes", async () => {
    const source = fakeSource()
    const { store } = await following(source)
    store.dispatch(openBeside({ sessionId: "c" }))
    source.hold("archive")
    const archiving = store.dispatch(
      archiveSession({ sessionId: "c", initiator: "person" }),
    )
    expect(store.getState().workspace.sessions.c).toBeDefined()
    await source.release("archive")
    await archiving
    expect(shown(store)).toEqual(["a"])
    expect(store.getState().workspace.sessions.c).toBeUndefined()
  })

  it("starts the last pane over when the session it shows is archived", async () => {
    const { store } = await following()
    await store.dispatch(archiveSession({ sessionId: "a", initiator: "person" }))
    const [shownId] = shown(store)
    expect(store.getState().workspace.sessions.a).toBeUndefined()
    expect(store.getState().workspace.drafts[shownId].channelId).toBe("desktop")
  })

  it("keeps out what the source said up to its removal, even when it overtook the answer", async () => {
    const source = fakeSource()
    const { store } = await following(source)
    source.hold("archive")
    const archiving = store.dispatch(
      archiveSession({ sessionId: "c", initiator: "person" }),
    )
    source.emit({
      kind: "session",
      session: summary("c", "desktop", 900, "running", { revision: 2 }),
    })
    await source.release("archive")
    await archiving
    expect(store.getState().workspace.sessions.c).toBeUndefined()
    source.emit({
      kind: "session",
      session: summary("c", "desktop", 900, "idle", { revision: 2 }),
    })
    expect(store.getState().workspace.sessions.c).toBeUndefined()
  })

  it("leaves a session where it is when the source refuses to archive it", async () => {
    const source = fakeSource()
    source.refuse("archive", "unavailable")
    const { store } = await following(source)
    await quietly(() =>
      store.dispatch(archiveSession({ sessionId: "d", initiator: "person" })),
    )
    expect(store.getState().workspace.sessions.d).toBeDefined()
  })

  it("asks nothing to archive a session the source has not spoken of: it is let go by discarding its message", async () => {
    const source = fakeSource()
    source.refuse("send", "unavailable")
    const { store } = await following(source)
    store.dispatch(newSession())
    const [draftId] = shown(store)
    await store.dispatch(
      sendMessage({ initiator: "person", sessionId: draftId, text: "Fix the rain" }),
    )
    await store.dispatch(archiveSession({ sessionId: draftId, initiator: "person" }))
    expect(store.getState().workspace.sessions[draftId]).toBeDefined()
    expect(source.calls.some((call) => call[0] === "archive")).toBe(false)
    const [refused] = outboxOf(store, draftId)
    store.dispatch(discardUnsent({ sessionId: draftId, messageId: refused.id }))
    expect(store.getState().workspace.sessions[draftId]).toBeUndefined()
    expect(store.getState().workspace.drafts[draftId]).toBeDefined()
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
    const warn = console.warn
    const logged: unknown[][] = []
    console.warn = (...args: unknown[]) => void logged.push(args)
    try {
      const { store } = await ready(source)
      expect(store.getState().workspace.sessions.stranded).toBeUndefined()
    } finally {
      console.warn = warn
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
    const warn = console.warn
    const logged: unknown[][] = []
    console.warn = (...args: unknown[]) => void logged.push(args)
    try {
      const { store } = await ready(source)
      expect(store.getState().workspace.sessions.z).toBeUndefined()
    } finally {
      console.warn = warn
    }
    expect(logged).toEqual([
      ["The index listed sessions at a revision the source could not have sent", ["z"]],
    ])
  })
})

describe("the room every change of layout is held to", () => {
  const widths = (store: ReturnType<typeof testStore>, gridWidth: number) => {
    const columns = store.getState().workspace.panes!.columns
    const total = columns.reduce((sum, column) => sum + column.share, 0)
    return columns.map(
      (column) => ((gridWidth - (columns.length - 1) * 8) * column.share) / total,
    )
  }

  it("holds an agent's split to the room the page measures, as it does a key's", async () => {
    const narrow = { width: 500, height: 800, spare: 0 }
    const store = testStore(fakeSource(), () => narrow)
    await store.dispatch(loadWorkspace())
    await settle()
    // No column fits: a split to the right is refused, whoever asks.
    expect(store.dispatch(newSession({ beside: "right" }))).toBeUndefined()
    expect(shown(store)).toEqual(["a"])
    // "Beside" with no side named stacks, where a row fits.
    store.dispatch(openBeside({ sessionId: "c" }))
    expect(store.getState().workspace.panes!.columns.map((c) => c.panes.length)).toEqual([
      2,
    ])
  })

  it("measures the room for an agent's move and drop, as a person's drag reads it", async () => {
    let measured = 0
    const store = testStore(fakeSource(), () => {
      measured++
      return { width: 1100, height: 800, spare: 0 }
    })
    await store.dispatch(loadWorkspace())
    await settle()
    store.dispatch(openBeside({ sessionId: "c", side: "right" }))
    const [first, second] = panesOf(store.getState().workspace.panes!).map((p) => p.key)
    // A side needs the room: given none, the agent's command measures it.
    const before = measured
    store.dispatch(movePane({ pane: first, target: second, zone: "bottom" }))
    expect(measured).toBe(before + 1)
    expect(store.getState().workspace.panes!.columns.map((c) => c.panes.length)).toEqual([
      2,
    ])
    store.dispatch(dropSession({ sessionId: "b", target: first, zone: "right" }))
    expect(measured).toBe(before + 2)
    expect(shown(store)).toContain("b")
    expect(store.getState().workspace.panes!.columns).toHaveLength(2)
  })

  it("moves nothing to a side for an agent when nothing was measured", async () => {
    let room: { width: number; height: number; spare: number } | undefined = {
      width: 1100,
      height: 800,
      spare: 0,
    }
    const store = testStore(fakeSource(), () => room)
    await store.dispatch(loadWorkspace())
    await settle()
    store.dispatch(openBeside({ sessionId: "c", side: "right" }))
    const layout = store.getState().workspace.panes
    const [first, second] = panesOf(layout!).map((p) => p.key)
    room = undefined
    store.dispatch(movePane({ pane: first, target: second, zone: "bottom" }))
    expect(store.getState().workspace.panes).toBe(layout)
  })

  it("places nothing new when nothing was measured", async () => {
    const store = testStore(fakeSource(), () => undefined)
    await store.dispatch(loadWorkspace())
    await settle()
    expect(store.dispatch(newSession({ beside: "right" }))).toBeUndefined()
    expect(store.dispatch(canOpenBeside())).toBe(false)
    // Opening beside with no room takes the focused pane's place, as it says.
    store.dispatch(openBeside({ sessionId: "c" }))
    expect(shown(store)).toEqual(["c"])
  })

  it("moves a pane out into a column of its own only where every pane stays readable", async () => {
    // Two columns of two, as a 1440 × 900 window with both side columns lays them out.
    const room = { width: 840, height: 850, spare: 256 }
    const store = testStore(fakeSource(), () => room)
    await store.dispatch(loadWorkspace())
    await settle()
    store.dispatch(newSession({ beside: "bottom" }))
    store.dispatch(openBeside({ sessionId: "c", side: "right" }))
    store.dispatch(newSession({ beside: "bottom" }))
    const first = store.getState().workspace.panes!.columns[0].panes[0].key
    expect(store.dispatch(canNudge({ pane: first, direction: "left" }))).toBe(true)
    store.dispatch(nudgePane({ pane: first, direction: "left" }))
    // Three columns fit only with the sidebar folded, and none is under 300.
    expect(store.getState().workspace.panes!.columns).toHaveLength(3)
    expect(store.getState().workspace.chrome.sidebar.folded).toBe(true)
    expect(Math.min(...widths(store, room.width + room.spare))).toBeGreaterThanOrEqual(
      299.5,
    )
  })

  it("refuses the move, and says so before it is asked, when no fold makes room", async () => {
    const room = { width: 840, height: 850, spare: 0 }
    const store = testStore(fakeSource(), () => room)
    await store.dispatch(loadWorkspace())
    await settle()
    store.dispatch(newSession({ beside: "bottom" }))
    store.dispatch(openBeside({ sessionId: "c", side: "right" }))
    store.dispatch(newSession({ beside: "bottom" }))
    const before = store.getState().workspace.panes
    const first = before!.columns[0].panes[0].key
    expect(store.dispatch(canNudge({ pane: first, direction: "left" }))).toBe(false)
    store.dispatch(nudgePane({ pane: first, direction: "left" }))
    expect(store.getState().workspace.panes).toBe(before)
  })

  it("fits the panes again when their room shrinks, rebalancing where it must", async () => {
    let room = { width: 1100, height: 800, spare: 0 }
    const store = testStore(fakeSource(), () => room)
    await store.dispatch(loadWorkspace())
    await settle()
    store.dispatch(openBeside({ sessionId: "c", side: "right" }))
    store.dispatch(
      resizePanes({ edge: { axis: "x", column: 0 }, fraction: 0.72, pair: 1092 }),
    )
    room = { width: 700, height: 800, spare: 0 }
    store.dispatch(fitPanes())
    expect(Math.min(...widths(store, room.width))).toBeGreaterThanOrEqual(299.5)
  })
})

describe("the Agents overview is a place in the sidebar, left by going anywhere else", () => {
  const ways = {
    "the channel already listed": selectChannel({ channelId: "desktop" }),
    "another channel": selectChannel({ channelId: "gateway" }),
    "a session": openSession({ sessionId: "c" }),
    "a session beside": openBeside({ sessionId: "c" }),
    "a channel from the tree": openChannel({ channelId: "gateway" }),
    "a new session": newSession(),
    "another pane": focusPane({ pane: 1 }),
    "closing a pane": closePane(),
    "moving a pane": nudgePane({ pane: 1, direction: "right" }),
    "evening the panes out": equalizePanes(),
    "resizing panes": resizePanes({
      edge: { axis: "x", column: 0 },
      fraction: 0.6,
      pair: 1,
    }),
  }
  for (const [way, action] of Object.entries(ways))
    it(`goes back to the panes on ${way}`, async () => {
      const { store } = await ready()
      // Two panes, unevenly: every change to them changes something.
      store.dispatch(openBeside({ sessionId: "c", side: "right" }))
      store.dispatch(
        resizePanes({ edge: { axis: "x", column: 0 }, fraction: 0.4, pair: 1 }),
      )
      store.dispatch(showContent({ content: "agents" }))
      store.dispatch(showContent({ content: "agents" }))
      expect(store.getState().workspace.content).toBe("agents")
      store.dispatch(action as Parameters<typeof store.dispatch>[0])
      expect(store.getState().workspace.content).toBe("panes")
    })

  it("stays when a change to the panes changes nothing: a move at the edge, a close of no pane", async () => {
    const { store } = await ready()
    const panes = store.getState().workspace.panes
    if (!panes) throw new Error("no panes")
    store.dispatch(showContent({ content: "agents" }))
    const edge = panesOf(panes)[0].key
    store.dispatch(nudgePane({ pane: edge, direction: "left" }))
    store.dispatch(closePane({ pane: 999 }))
    expect(store.getState().workspace.panes).toBe(panes)
    expect(store.getState().workspace.content).toBe("agents")
    // Going to the pane already focused is still going there.
    store.dispatch(focusPane({ pane: panes.focused }))
    expect(store.getState().workspace.content).toBe("panes")
  })

  it("stays over what only rearranges the window", async () => {
    const { store } = await ready()
    store.dispatch(showContent({ content: "agents" }))
    store.dispatch(toggleSidebar())
    store.dispatch(toggleSessionList())
    expect(store.getState().workspace.content).toBe("agents")
  })
})

describe("the Agents overview answers and reads what it shows, as a pane does", () => {
  const waitingOn = (source = fakeSource()) => {
    source.transcripts.set("b", {
      ...emptyTranscript("b"),
      revision: 1,
      approval: { id: "ap", command: "cargo test", reason: "Runs tests." },
    })
    return source
  }

  it("reads the conversation of a session waiting on the person once the overview opens, without marking it read", async () => {
    const { store, source } = await ready(waitingOn())
    expect(source.calls).not.toContainEqual(["transcript", "b"])
    store.dispatch(showContent({ content: "agents" }))
    await settle()
    expect(source.calls).toContainEqual(["transcript", "b"])
    expect(store.getState().workspace.transcripts.b?.approval?.id).toBe("ap")
    // A peek is not reading the session — not when it opens, nor when the session changes.
    expect(store.getState().workspace.sessions.b.unread).toBe(true)
    store.dispatch(followWorkspace())
    source.emit({
      kind: "session",
      session: summary("b", "desktop", 250, "needs-you", { unread: true, revision: 2 }),
    })
    await settle()
    expect(store.getState().workspace.sessions.b.unread).toBe(true)
    expect(source.calls).not.toContainEqual(["markRead", "b"])
  })

  it("answers an approval the open overview shows in no pane, as the person, through the one answer", async () => {
    const { store, source } = await ready(waitingOn())
    store.dispatch(showContent({ content: "agents" }))
    await settle()
    source.hold("approve")
    const answer = store.dispatch(
      approve({ sessionId: "b", approvalId: "ap", initiator: "person" }),
    )
    // A second answer, from a pane or the overview, waits on the first.
    expect(
      await store.dispatch(
        deny({ sessionId: "b", approvalId: "ap", initiator: "person" }),
      ),
    ).toBe("answering")
    await source.release("approve")
    expect(await answer).toBe("sent")
    expect(source.calls).toContainEqual(["approve", "b", "ap", "once", "person"])
    expect(source.calls.filter((call) => call[0] === "deny")).toEqual([])
    expect(store.getState().workspace.content).toBe("agents")
  })

  it("says an approval was not asked when neither a pane nor the open overview shows it", async () => {
    const { store, source } = await ready(waitingOn())
    // Open, but listing only sessions under a tag this one does not carry.
    store.dispatch(
      filterOverview({ filter: { scope: "all", range: "any", tags: ["ui"] } }),
    )
    store.dispatch(showContent({ content: "agents" }))
    expect(store.getState().workspace.overview.selected).not.toBe("b")
    await settle()
    expect(
      await store.dispatch(
        approve({ sessionId: "b", approvalId: "ap", initiator: "agent" }),
      ),
    ).toBe("not-asked")
    store.dispatch(showContent({ content: "panes" }))
    expect(
      await store.dispatch(
        approve({ sessionId: "b", approvalId: "ap", initiator: "agent" }),
      ),
    ).toBe("not-asked")
    expect(source.calls.some((call) => call[0] === "approve")).toBe(false)
  })

  it("keeps a refused answer's reason, and answers again, from the overview", async () => {
    const { store, source } = await ready(waitingOn())
    store.dispatch(showContent({ content: "agents" }))
    await settle()
    source.refuse("approve", "not-waiting")
    expect(
      await store.dispatch(
        approve({ sessionId: "b", approvalId: "ap", initiator: "person" }),
      ),
    ).toBe("refused")
    source.refuse("approve", "unavailable")
    expect(
      await store.dispatch(
        approve({ sessionId: "b", approvalId: "ap", initiator: "person" }),
      ),
    ).toBe("unknown")
    expect(store.getState().workspace.answers.b?.failure).toBe("unavailable")
    source.refuse("approve", undefined)
    expect(
      await store.dispatch(
        approve({ sessionId: "b", approvalId: "ap", initiator: "person" }),
      ),
    ).toBe("sent")
  })

  it("keeps an answer on its way when the overview closes, and lets it go on the conversation that no longer asks", async () => {
    const { store, source } = await ready(waitingOn())
    store.dispatch(followWorkspace())
    store.dispatch(showContent({ content: "agents" }))
    await settle()
    source.hold("approve")
    const answer = store.dispatch(
      approve({ sessionId: "b", approvalId: "ap", initiator: "person" }),
    )
    store.dispatch(showContent({ content: "panes" }))
    expect(store.getState().workspace.answers.b?.approvalId).toBe("ap")
    await source.release("approve")
    expect(await answer).toBe("sent")
    await settle()
    expect(store.getState().workspace.answers.b).toBeUndefined()
  })

  it("sends a reply from the overview to a waiting session no pane shows, as the person, and says when it is not sent", async () => {
    const { store, source } = await ready(waitingOn())
    store.dispatch(showContent({ content: "agents" }))
    await settle()
    source.hold("send")
    const reply = store.dispatch(
      sendMessage({ sessionId: "b", text: "Use the beta profile.", initiator: "person" }),
    )
    expect(outboxOf(store, "b").map((message) => message.delivery?.state)).toEqual([
      "sending",
    ])
    await source.release("send")
    expect(await reply).toBe("sent")
    expect(source.calls.find((call) => call[0] === "send")?.[1]).toMatchObject({
      sessionId: "b",
      initiator: "person",
    })
    source.refuse("send", "unavailable")
    expect(
      await store.dispatch(
        sendMessage({ sessionId: "b", text: "And again.", initiator: "person" }),
      ),
    ).toBe("unknown")
    expect(outboxOf(store, "b").at(-1)?.delivery).toEqual({
      state: "failed",
      reason: "unavailable",
    })
  })

  it("reads what a session chosen in the overview says, and lets it go when it is removed", async () => {
    const { store, source } = await ready()
    store.dispatch(showContent({ content: "agents" }))
    store.dispatch(selectInOverview({ sessionId: "d" }))
    await settle()
    expect(source.calls).toContainEqual(["transcript", "d"])
    // A session not listed is not chosen.
    store.dispatch(selectInOverview({ sessionId: "nowhere" }))
    expect(store.getState().workspace.overview.selected).toBe("d")
    store.dispatch(followWorkspace())
    await store.dispatch(archiveSession({ sessionId: "d", initiator: "person" }))
    // Gone, it is not chosen: the first the overview lists is, and is read.
    const chosen = store.getState().workspace.overview.selected
    expect(chosen).not.toBe("d")
    expect(chosen).not.toBeNull()
    await settle()
    expect(source.calls).toContainEqual(["transcript", chosen])
  })

  it("chooses the first it lists on opening, so what the peek shows is what the window reads", async () => {
    const { store, source } = await ready()
    expect(store.getState().workspace.overview.selected).toBeNull()
    store.dispatch(showContent({ content: "agents" }))
    const chosen = store.getState().workspace.overview.selected
    expect(chosen).not.toBeNull()
    await settle()
    expect(source.calls).toContainEqual(["transcript", chosen])
  })

  it("chooses nothing while it lists nothing", async () => {
    const { store } = await ready()
    store.dispatch(
      filterOverview({ filter: { scope: "all", range: "any", tags: ["nowhere"] } }),
    )
    store.dispatch(showContent({ content: "agents" }))
    expect(store.getState().workspace.overview.selected).toBeNull()
  })

  it("reads and keeps the conversations of at most so many waiting sessions, and a chosen one past them", async () => {
    const many = Array.from({ length: retention.overviewConversations + 6 }, (_, at) =>
      summary(`w${at}`, "desktop", 500 + at, "needs-you"),
    )
    const index = testIndex()
    const source = fakeSource({ ...index, sessions: [...index.sessions, ...many] })
    const { store } = await ready(source)
    store.dispatch(showContent({ content: "agents" }))
    await settle()
    const read = () =>
      new Set(
        source.calls.filter((call) => call[0] === "transcript").map((call) => call[1]),
      )
    // The most recently active, "b" and the oldest waiting left out.
    expect(read().has("w0")).toBe(false)
    expect(read().has(`w${many.length - 1}`)).toBe(true)
    const kept = Object.keys(store.getState().workspace.transcripts)
    expect(kept.length).toBeLessThanOrEqual(
      retention.overviewConversations + retention.unshownConversations + 1,
    )
    store.dispatch(selectInOverview({ sessionId: "w0" }))
    await settle()
    expect(read().has("w0")).toBe(true)
    expect(store.getState().workspace.transcripts.w0).toBeDefined()
  })

  it("keeps a failed read while the overview shows the session, and reads it afresh once shown again", async () => {
    const source = waitingOn()
    const { store } = await ready(source)
    source.refuse("transcript", "unavailable")
    store.dispatch(showContent({ content: "agents" }))
    await settle()
    expect(store.getState().workspace.transcriptFailures.b).toBe("unavailable")
    store.dispatch(showContent({ content: "panes" }))
    expect(store.getState().workspace.transcriptFailures.b).toBeUndefined()
    source.refuse("transcript", undefined)
    store.dispatch(showContent({ content: "agents" }))
    await settle()
    expect(store.getState().workspace.transcripts.b?.approval?.id).toBe("ap")
  })
})

describe("the overview's filter is the store's, kept between launches", () => {
  it("starts from what was kept, and keeps each change", async () => {
    const kept = keptFilter({ scope: "all", range: "week", tags: [] })
    const store = testStore(fakeSource(), undefined, kept)
    expect(store.getState().workspace.overview.filter).toEqual({
      scope: "all",
      range: "week",
      tags: [],
    })
    store.dispatch(
      filterOverview({ filter: { scope: "ongoing", range: "any", tags: [] } }),
    )
    expect(kept.writes).toEqual([{ scope: "ongoing", range: "any", tags: [] }])
  })
})

describe("what is typed goes when the person sends it", () => {
  it("empties the composer on the person's send, and keeps it when there is nothing to send it with", async () => {
    const { store } = await ready()
    store.dispatch(setComposerText({ sessionId: "a", text: "Run it." }))
    expect(
      await store.dispatch(
        sendMessage({ sessionId: "a", text: "Run it.", initiator: "person" }),
      ),
    ).toBe("sent")
    expect(store.getState().workspace.composerText.a).toBeUndefined()
    store.dispatch(setComposerText({ sessionId: "a", text: "  " }))
    expect(
      await store.dispatch(
        sendMessage({ sessionId: "a", text: "  ", initiator: "person" }),
      ),
    ).toBe("not-asked")
    expect(store.getState().workspace.composerText.a).toBe("  ")
  })

  it("keeps what the person is typing when an agent sends a message of its own", async () => {
    const { store } = await ready()
    store.dispatch(setComposerText({ sessionId: "a", text: "person half-typed" }))
    expect(
      await store.dispatch(
        sendMessage({ sessionId: "a", text: "agent says", initiator: "agent" }),
      ),
    ).toBe("sent")
    expect(store.getState().workspace.composerText.a).toBe("person half-typed")
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
