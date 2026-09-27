/**
 * The commands through a real store and a fake source: what an agent's
 * dispatch does to the window, and what the source is asked, including when
 * it refuses or answers late.
 */
import { describe, expect, it } from "vitest"
import { WorkspaceSourceError } from "../../application/ports"
import { panesOf } from "../../model/pane-layout"
import { emptyTranscript } from "../../model/transcript"
import {
  astra,
  fakeSource,
  settle,
  summary,
  testOrganisation,
  testStore,
} from "../../testing"
import {
  approve,
  archiveSession,
  chooseModel,
  closePane,
  deny,
  discardUnsent,
  followWorkspace,
  loadWorkspace,
  newSession,
  openBeside,
  openChannel,
  openSession,
  pinSession,
  resendMessage,
  retryTranscript,
  sendMessage,
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
  it("opens on the organisation, and reads the conversation the pane shows", async () => {
    const { store, source } = await ready()
    expect(shown(store)).toEqual(["a"])
    expect(source.calls).toContainEqual(["transcript", "a"])
    expect(store.getState().workspace.transcripts.a).toEqual({
      ...emptyTranscript("a"),
      revision: 1,
    })
  })

  it("says why when the organisation cannot be read", async () => {
    const source = fakeSource()
    source.refuse("organisation", "unavailable")
    const { store } = await ready(source)
    expect(store.getState().workspace.status).toBe("failed")
    expect(store.getState().workspace.failure).toMatch(/couldn’t reach/)
  })

  it("says why a shown conversation cannot be read", async () => {
    const source = fakeSource()
    source.refuse("transcript", "unknown-session")
    const { store } = await ready(source)
    expect(store.getState().workspace.transcriptFailures.a).toMatch(/no longer there/)
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
    expect(store.getState().workspace.transcriptFailures.a).toMatch(/couldn’t reach/)
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
      expect(store.getState().workspace.transcriptFailures.a).toMatch(/couldn’t reach/)
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
      expect(store.getState().workspace.transcriptFailures.a).toMatch(/couldn’t reach/)
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

  it("keeps what the stream said before the organisation was read", async () => {
    const source = fakeSource()
    source.hold("organisation")
    const store = testStore(source)
    store.dispatch(followWorkspace())
    const loading = store.dispatch(loadWorkspace())
    source.emit({ kind: "session-removed", sessionId: "c", revision: 2 })
    source.emit({
      kind: "session",
      session: summary("a", "desktop", 999, "idle", { revision: 4 }),
    })
    await source.release("organisation")
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
    const organisation = testOrganisation()
    // One read of the organisation marks both shown sessions unread at once.
    source.organisation = () =>
      Promise.resolve({
        ...organisation,
        sessions: organisation.sessions.map((session) =>
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
    const organisation = testOrganisation()
    const source = fakeSource({
      ...organisation,
      sessions: organisation.sessions.map((session) =>
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
      reason: "This session is no longer there.",
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
      failure: "This was already answered.",
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
