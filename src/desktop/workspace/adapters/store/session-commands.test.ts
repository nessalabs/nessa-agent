/** The session commands: sending, approvals, and pin, archive, rename, delete. */
import { describe, expect, it, vi } from "vitest"

import { emptyTranscript } from "../../model/transcript"

import {
  astra,
  fakeSource,
  outboxOf,
  ready,
  settle,
  shown,
  summary,
  testIndex,
} from "../../testing"

import {
  DRAFT_IMAGE_LIMITS,
  addComposerImages,
  approve,
  archiveSession,
  chooseModel,
  deny,
  discardUnsent,
  followWorkspace,
  newSession,
  openBeside,
  openSession,
  pinSession,
  removeComposerImage,
  resendMessage,
  sendMessage,
  setComposerText,
} from "./commands"
import { workspaceActions } from "./slice"

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

  it("takes the images pasted into a composer onto the person's message, which may be images alone", async () => {
    const { store, source } = await ready()
    store.dispatch(newSession())
    const [draftId] = shown(store)
    const png = new File([new Uint8Array([1, 2])], "shot.png", { type: "image/png" })
    const text = new File(["notes"], "notes.txt", { type: "text/plain" })
    // Only the images among what was pasted are taken.
    expect(
      store.dispatch(addComposerImages({ sessionId: draftId, files: [png, text] })),
    ).toBe(null)
    const [image] = store.getState().workspace.composerImages[draftId]
    expect(image).toMatchObject({ name: "shot.png", mimeType: "image/png", size: 2 })
    // Past a message's bounds none are taken, and the composer keeps what it had.
    expect(
      store.dispatch(
        addComposerImages({
          sessionId: draftId,
          files: Array(DRAFT_IMAGE_LIMITS.images).fill(png),
        }),
      ),
    ).toBe("too-many")
    expect(store.getState().workspace.composerImages[draftId]).toEqual([image])
    await store.dispatch(
      sendMessage({
        initiator: "person",
        sessionId: draftId,
        text: " ",
        withImages: true,
      }),
    )
    expect(source.calls.find((call) => call[0] === "send")?.[1]).toMatchObject({
      sessionId: draftId,
      text: "",
      images: [image],
      // Images alone name the session by what they are.
      start: { title: "Image" },
    })
    expect(store.getState().workspace.composerImages[draftId]).toBeUndefined()
    expect(outboxOf(store, draftId)[0]).toMatchObject({ localImages: [image] })
  })

  it("puts a refused message's images, and its words, back in the composer when it is let go", async () => {
    const { store, source } = await ready()
    store.dispatch(newSession())
    const [draftId] = shown(store)
    const png = new File([new Uint8Array([1])], "shot.png", { type: "image/png" })
    store.dispatch(addComposerImages({ sessionId: draftId, files: [png] }))
    const [image] = store.getState().workspace.composerImages[draftId]
    source.refuse("send", "not-supported")
    await store.dispatch(
      sendMessage({
        initiator: "person",
        sessionId: draftId,
        text: "Look",
        withImages: true,
      }),
    )
    const refused = outboxOf(store, draftId)[0]
    expect(refused.delivery).toMatchObject({ state: "failed" })
    store.dispatch(discardUnsent({ sessionId: draftId, messageId: refused.id }))
    const workspace = store.getState().workspace
    expect(workspace.composerImages[draftId]).toEqual([image])
    expect(workspace.composerText[draftId]).toBe("Look")
  })

  it("lets a pasted image's bytes go only once nothing names it: not on a refusal, but once the conversation holds the message", async () => {
    const revoke = vi.spyOn(URL, "revokeObjectURL")
    try {
      const { store, source } = await ready()
      store.dispatch(followWorkspace())
      store.dispatch(openSession({ sessionId: "a" }))
      await settle()
      const png = new File([new Uint8Array([1])], "shot.png", { type: "image/png" })
      store.dispatch(addComposerImages({ sessionId: "a", files: [png] }))
      const [image] = store.getState().workspace.composerImages.a
      source.refuse("send", "unavailable")
      await store.dispatch(
        sendMessage({
          initiator: "person",
          sessionId: "a",
          text: "See",
          withImages: true,
        }),
      )
      const [refused] = outboxOf(store, "a")
      // Refused, then let go: the image goes back to the composer, still alive.
      store.dispatch(discardUnsent({ sessionId: "a", messageId: refused.id }))
      expect(store.getState().workspace.composerImages.a).toEqual([image])
      expect(revoke).not.toHaveBeenCalled()
      source.refuse("send", undefined)
      await store.dispatch(
        sendMessage({
          initiator: "person",
          sessionId: "a",
          text: "See",
          withImages: true,
        }),
      )
      const [sent] = outboxOf(store, "a")
      expect(revoke).not.toHaveBeenCalled()
      // The conversation holds it: nothing names the pasted copy any more.
      const held = {
        ...emptyTranscript("a"),
        revision: 9,
        messages: [{ ...sent, localImages: undefined, delivery: undefined }],
      }
      source.transcripts.set("a", held)
      source.emit({ kind: "transcript", transcript: held })
      await settle()
      expect(outboxOf(store, "a")).toEqual([])
      expect(revoke.mock.calls).toEqual([[image.url]])
    } finally {
      revoke.mockRestore()
    }
  })

  it("leaves a composer's images for the person when an agent, or a reply sent elsewhere, sends, and takes one off when asked", async () => {
    const { store, source } = await ready()
    store.dispatch(newSession())
    const [draftId] = shown(store)
    const png = new File([new Uint8Array([1])], "shot.png", { type: "image/png" })
    store.dispatch(addComposerImages({ sessionId: draftId, files: [png, png] }))
    const [first, second] = store.getState().workspace.composerImages[draftId]
    await store.dispatch(
      sendMessage({ initiator: "agent", sessionId: draftId, text: "Go." }),
    )
    expect(source.calls.find((call) => call[0] === "send")?.[1]).not.toHaveProperty(
      "images",
    )
    expect(store.getState().workspace.composerImages[draftId]).toEqual([first, second])
    // A person's reply from outside the composer does not carry them unseen.
    await store.dispatch(
      sendMessage({ initiator: "person", sessionId: draftId, text: "And this." }),
    )
    expect(
      source.calls.filter((call) => call[0] === "send").at(-1)?.[1],
    ).not.toHaveProperty("images")
    expect(store.getState().workspace.composerImages[draftId]).toEqual([first, second])
    store.dispatch(removeComposerImage({ sessionId: draftId, imageId: first.id }))
    expect(store.getState().workspace.composerImages[draftId]).toEqual([second])
    store.dispatch(removeComposerImage({ sessionId: draftId, imageId: second.id }))
    expect(store.getState().workspace.composerImages).not.toHaveProperty(draftId)
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

  it("does not send a retained draft after its channel disappears", async () => {
    const { store, source } = await ready()
    store.dispatch(newSession())
    const [draftId] = shown(store)
    store.dispatch(
      workspaceActions.indexLoaded({
        index: { sections: [], channels: [], sessions: [] },
        draftId: "unused",
        read: "r",
      }),
    )
    expect(store.getState().workspace.drafts[draftId]).toBeDefined()
    expect(
      await store.dispatch(
        sendMessage({ initiator: "person", sessionId: draftId, text: "hi" }),
      ),
    ).toBe("not-asked")
    expect(source.calls.some((call) => call[0] === "send")).toBe(false)
  })

  it("does not deliver a staged first send after a resync removes its channel", async () => {
    const { store, source } = await ready()
    store.dispatch(newSession())
    const [draftId] = shown(store)
    const sending = store.dispatch(
      sendMessage({ initiator: "person", sessionId: draftId, text: "Keep this" }),
    )
    store.dispatch(
      workspaceActions.indexLoaded({
        index: { sections: [], channels: [], sessions: [] },
        draftId: "unused",
        read: "r",
      }),
    )
    expect(await sending).toBe("not-asked")
    expect(source.calls.some((call) => call[0] === "send")).toBe(false)
    expect(outboxOf(store, draftId)[0].delivery).toMatchObject({
      state: "failed",
      reason: "unavailable",
      diagnostic: { message: "This channel is no longer available." },
    })
  })

  it("does not deliver a staged retry after a resync removes its channel", async () => {
    const source = fakeSource()
    source.refuse("send", "unavailable")
    const { store } = await ready(source)
    store.dispatch(newSession())
    const [draftId] = shown(store)
    await store.dispatch(
      sendMessage({ initiator: "person", sessionId: draftId, text: "Keep this" }),
    )
    const messageId = outboxOf(store, draftId)[0].id
    const before = source.calls.filter((call) => call[0] === "send").length
    source.refuse("send", undefined)
    const retrying = store.dispatch(
      resendMessage({ initiator: "person", sessionId: draftId, messageId }),
    )
    store.dispatch(
      workspaceActions.indexLoaded({
        index: { sections: [], channels: [], sessions: [] },
        draftId: "unused",
        read: "r",
      }),
    )
    expect(await retrying).toBe("not-asked")
    expect(source.calls.filter((call) => call[0] === "send")).toHaveLength(before)
    expect(outboxOf(store, draftId)[0].delivery).toMatchObject({
      state: "failed",
      reason: "unavailable",
      diagnostic: { message: "This channel is no longer available." },
    })
  })

  it("settles a first send already dispatched when its channel is later removed", async () => {
    const source = fakeSource()
    source.hold("send")
    const { store } = await ready(source)
    store.dispatch(newSession())
    const [draftId] = shown(store)
    const sending = store.dispatch(
      sendMessage({ initiator: "person", sessionId: draftId, text: "Keep this" }),
    )
    await settle()
    expect(source.calls.filter((call) => call[0] === "send")).toHaveLength(1)
    store.dispatch(
      workspaceActions.indexLoaded({
        index: { sections: [], channels: [], sessions: [] },
        draftId: "unused",
        read: "r",
      }),
    )
    await source.release("send")
    expect(await sending).toBe("sent")
    expect(outboxOf(store, draftId)[0].delivery).toBeUndefined()
  })

  it("shows the message as sending until the source takes it", async () => {
    const source = fakeSource()
    const { store } = await ready(source)
    source.hold("send")
    const sending = store.dispatch(
      sendMessage({ initiator: "person", sessionId: "a", text: "hi" }),
    )
    expect(outboxOf(store, "a").at(-1)?.delivery).toMatchObject({ state: "sending" })
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

  it("applies a staged model before an agent's message, and sends on it", async () => {
    const { store, source } = await ready()
    // Following the source, as the window does: its summary says the model is made.
    store.dispatch(followWorkspace())
    store.dispatch(chooseModel({ sessionId: "a", model: astra }))
    await store.dispatch(sendMessage({ initiator: "agent", sessionId: "a", text: "hi" }))
    const order = source.calls.map((call) => call[0])
    expect(order.indexOf("setModel")).toBeLessThan(order.indexOf("send"))
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
    expect(outboxOf(store, "a").at(-1)?.delivery).toMatchObject({
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
      approvals: [
        {
          id: "ap",
          command: "cargo test",
          reason: "Runs tests.",
          origin: { kind: "agent" },
          options: [],
          ask: "tool",
        },
      ],
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
      approve({
        sessionId: "b",
        approvalId: "ap",
        scope: "always",
        initiator: "person",
        optionId: "always",
      }),
    )
    await store.dispatch(
      approve({
        sessionId: "b",
        approvalId: "ap",
        initiator: "person",
        optionId: "once",
      }),
    )
    await source.release("approve")
    await first
    expect(source.calls.filter((call) => call[0] === "approve")).toEqual([
      ["approve", "b", "ap", "always", "person", "always"],
    ])
  })

  it("asks again, saying why, when the answer does not reach the agent", async () => {
    const { store, source } = await withApproval()
    source.refuse("deny", "not-waiting")
    expect(
      await store.dispatch(
        deny({ sessionId: "b", approvalId: "ap", initiator: "person", optionId: "deny" }),
      ),
    ).toBe("refused")
    expect(store.getState().workspace.answers.b?.[0]).toMatchObject({
      approvalId: "ap",
      failure: "not-waiting",
    })
    source.refuse("deny", undefined)
    await store.dispatch(
      deny({ sessionId: "b", approvalId: "ap", initiator: "person", optionId: "deny" }),
    )
    expect(source.calls.filter((call) => call[0] === "deny")).toHaveLength(2)
    expect(store.getState().workspace.answers.b?.[0]).toMatchObject({ approvalId: "ap" })
  })

  it("lets the source's newer conversation settle the approval, before the answer resolves", async () => {
    const { store, source } = await withApproval()
    store.dispatch(followWorkspace())
    source.hold("approve")
    const answering = store.dispatch(
      approve({
        sessionId: "b",
        approvalId: "ap",
        initiator: "person",
        optionId: "once",
      }),
    )
    expect(store.getState().workspace.answers.b?.[0]).toMatchObject({ approvalId: "ap" })
    await source.release("approve")
    // Resolved: the conversation that no longer asks has already arrived.
    expect(await answering).toBe("sent")
    expect(store.getState().workspace.transcripts.b.approvals).toEqual([])
    expect(store.getState().workspace.answers.b).toBeUndefined()
    expect(
      await store.dispatch(
        approve({
          sessionId: "b",
          approvalId: "ap",
          initiator: "agent",
          optionId: "once",
        }),
      ),
    ).toBe("not-asked")
  })

  it("keeps one answer at a time across its pane showing another session and back", async () => {
    const { store, source } = await withApproval()
    source.hold("approve")
    source.refuse("approve", "unavailable")
    const agents = store.dispatch(
      approve({ sessionId: "b", approvalId: "ap", initiator: "agent", optionId: "once" }),
    )
    store.dispatch(openSession({ sessionId: "c" }))
    store.dispatch(openSession({ sessionId: "b" }))
    await settle()
    // The agent's answer is still on its way: the person's waits.
    expect(
      await store.dispatch(
        deny({ sessionId: "b", approvalId: "ap", initiator: "person", optionId: "deny" }),
      ),
    ).toBe("answering")
    await source.release("approve")
    expect(await agents).toBe("unknown")
    source.refuse("approve", undefined)
    expect(
      await store.dispatch(
        deny({ sessionId: "b", approvalId: "ap", initiator: "person", optionId: "deny" }),
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
      approve({ sessionId: "b", approvalId: "ap", initiator: "agent", optionId: "once" }),
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
      deny({ sessionId: "b", approvalId: "ap", initiator: "person", optionId: "deny" }),
    )
    await source.release("approve")
    expect(await first).toBe("unknown")
    expect(store.getState().workspace.answers.b?.[0]?.failure).toBeUndefined()
    await source.release("deny")
    expect(await second).toBe("sent")
  })

  it("tells the source who decided, and the caller what became of it", async () => {
    const { store, source } = await withApproval()
    source.hold("deny")
    const first = store.dispatch(
      deny({ sessionId: "b", approvalId: "ap", initiator: "agent", optionId: "deny" }),
    )
    expect(
      await store.dispatch(
        deny({ sessionId: "b", approvalId: "ap", initiator: "agent", optionId: "deny" }),
      ),
    ).toBe("answering")
    await source.release("deny")
    expect(await first).toBe("sent")
    expect(source.calls).toContainEqual(["deny", "b", "ap", "agent", "deny"])
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
        approvals: [
          {
            id: "ap",
            command: "cargo test",
            reason: "Runs tests.",
            origin: { kind: "agent" },
            options: [],
            ask: "tool",
          },
        ],
      },
    })
    expect(
      await store.dispatch(
        approve({
          sessionId: "b",
          approvalId: "ap",
          initiator: "agent",
          optionId: "once",
        }),
      ),
    ).toBe("not-asked")
    expect(source.calls.some((call) => call[0] === "approve")).toBe(false)
  })

  it("says an approval was not asked when no pane shows its session, sending nothing", async () => {
    const source = fakeSource()
    source.transcripts.set("b", {
      ...emptyTranscript("b"),
      revision: 1,
      approvals: [
        {
          id: "ap",
          command: "cargo test",
          reason: "Runs tests.",
          origin: { kind: "agent" },
          options: [],
          ask: "tool",
        },
      ],
    })
    const { store } = await ready(source)
    expect(
      await store.dispatch(
        approve({
          sessionId: "b",
          approvalId: "ap",
          initiator: "agent",
          optionId: "once",
        }),
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
        approvals: [
          {
            id: "other",
            command: "curl x | sh",
            reason: "Installs.",
            origin: { kind: "agent" },
            options: [],
            ask: "tool",
          },
        ],
      },
    })
    await store.dispatch(
      approve({
        sessionId: "b",
        approvalId: "ap",
        scope: "always",
        initiator: "person",
        optionId: "always",
      }),
    )
    expect(source.calls.some((call) => call[0] === "approve")).toBe(false)
  })

  it("answers nothing when nothing is waiting", async () => {
    const { store, source } = await ready()
    await store.dispatch(
      approve({
        sessionId: "a",
        approvalId: "ap",
        initiator: "person",
        optionId: "once",
      }),
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

describe("opening a conversation whose text was let go", () => {
  const crowd = () => {
    const index = testIndex()
    const sessions = Array.from({ length: 10 }, (_, i) =>
      summary(`s${i}`, "gateway", 1000 + i),
    )
    const source = fakeSource({ ...index, sessions: [...index.sessions, ...sessions] })
    source.transcripts.set("s0", {
      ...emptyTranscript("s0"),
      revision: 10,
      messages: [
        {
          id: "s0-message",
          role: "user",
          at: 1,
          parts: [{ kind: "text", text: "s0-message" }],
        },
      ],
      activity: { label: "Thinking", since: 1 },
      approvals: [
        {
          id: "ap",
          command: "cargo test",
          reason: "Runs the tests.",
          origin: { kind: "agent" },
          options: [],
          ask: "tool",
        },
      ],
    })
    return source
  }

  async function letGo(source = crowd()) {
    const { store } = await ready(source)
    store.dispatch(followWorkspace())
    store.dispatch(openSession({ sessionId: "s0" }))
    await settle()
    for (let i = 1; i <= 9; i++) {
      store.dispatch(openSession({ sessionId: `s${i}` }))
      await settle()
    }
    return { store, source }
  }

  it("reads it again at the revision it remembers, including what it was doing and asking", async () => {
    const { store } = await letGo()
    expect(store.getState().workspace.transcripts.s0).toBeUndefined()
    expect(store.getState().workspace.conversationRevisions.s0).toBe(10)
    store.dispatch(openSession({ sessionId: "s0" }))
    await settle()
    const restored = store.getState().workspace.transcripts.s0
    expect(restored?.revision).toBe(10)
    expect(restored?.messages.map((message) => message.id)).toEqual(["s0-message"])
    expect(restored?.activity).toEqual({ label: "Thinking", since: 1 })
    expect(restored?.approvals[0]?.id).toBe("ap")
    expect(store.getState().workspace.transcriptFailures.s0).toBeUndefined()
  })

  it("rejects an older reread, then takes a newer one from a resync, without a failure either way", async () => {
    const source = crowd()
    const { store } = await letGo(source)
    source.transcripts.set("s0", {
      ...emptyTranscript("s0"),
      revision: 9,
      messages: [
        { id: "old", role: "user", at: 1, parts: [{ kind: "text", text: "old" }] },
      ],
    })
    store.dispatch(openSession({ sessionId: "s0" }))
    await settle()
    expect(store.getState().workspace.transcripts.s0).toBeUndefined()
    expect(store.getState().workspace.conversationRevisions.s0).toBe(10)
    expect(store.getState().workspace.transcriptFailures.s0).toBeUndefined()
    source.transcripts.set("s0", {
      ...emptyTranscript("s0"),
      revision: 11,
      messages: [
        {
          id: "s0-message",
          role: "user",
          at: 1,
          parts: [{ kind: "text", text: "s0-message" }],
        },
      ],
    })
    source.emit({ kind: "resync" })
    await settle(10)
    expect(store.getState().workspace.transcripts.s0?.revision).toBe(11)
    expect(
      store.getState().workspace.transcripts.s0?.messages.map((message) => message.id),
    ).toEqual(["s0-message"])
    expect(store.getState().workspace.transcriptFailures.s0).toBeUndefined()
  })

  it("leaves a conversation already held at that revision unchanged when resync reads it again", async () => {
    const { store, source } = await ready()
    store.dispatch(followWorkspace())
    const held = store.getState().workspace.transcripts.a
    source.transcripts.set("a", {
      ...emptyTranscript("a"),
      revision: held?.revision ?? 1,
      messages: [
        { id: "other", role: "agent", at: 1, parts: [{ kind: "text", text: "other" }] },
      ],
    })
    source.emit({ kind: "resync" })
    await settle(10)
    expect(store.getState().workspace.transcripts.a).toBe(held)
  })
})
