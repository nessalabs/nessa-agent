/**
 * The composer shield's command through a real store and a fake source: a
 * choice for a begun session is asked of the source and shown only while it
 * is the newer word; one for a session not begun goes with its first message.
 */
import { describe, expect, it } from "vitest"
import { fakeSource, settle, summary, testStore, type FakeSource } from "../../testing"
import { WorkspaceSourceError } from "../../application/ports"
import { emptyTranscript, type Transcript } from "../../model/transcript"
import {
  chooseAccess,
  discardUnsent,
  followWorkspace,
  setComposerText,
  loadWorkspace,
  newSession,
  sendMessage,
} from "./commands"
import { selectAccess, selectAccessNotice } from "./selectors"
import { accessRefusalCopy, steppedAccessCopy } from "../../ui/panes/composer-access"

async function ready(source = fakeSource()) {
  const store = testStore(source)
  await store.dispatch(loadWorkspace())
  store.dispatch(followWorkspace())
  await settle()
  return { store, source }
}

describe("choosing access", () => {
  it("asks the source to change a begun session, and shows the mode its conversation says", async () => {
    const { store, source } = await ready()
    expect(selectAccess(store.getState(), "a")).toBe("ask")
    const outcome = await store.dispatch(
      chooseAccess({ sessionId: "a", mode: "full", initiator: "person" }),
    )
    expect(outcome).toBe("taken")
    expect(source.calls.filter((call) => call[0] === "setAccess")).toEqual([
      ["setAccess", "a", "full", "person"],
    ])
    expect(selectAccess(store.getState(), "a")).toBe("full")
    // The conversation says it now: the choice has settled into the source's word.
    expect(store.getState().workspace.accessChoices.a).toBeUndefined()
    expect(store.getState().workspace.transcripts.a?.access?.mode).toBe("full")
  })

  it("shows the source's mode again when the source refuses, and says why", async () => {
    const { store, source } = await ready()
    // The gateway refuses a change while a turn runs (`turn_running`).
    source.refuse("setAccess", "unavailable")
    const outcome = await store.dispatch(
      chooseAccess({ sessionId: "a", mode: "full", initiator: "person" }),
    )
    expect(outcome).toBe("refused")
    expect(selectAccess(store.getState(), "a")).toBe("ask")
    expect(selectAccessNotice(store.getState(), "a")).toMatchObject({
      reason: "unavailable",
    })
  })

  it("shows the change on its way, and asks for one change at a time", async () => {
    const { store, source } = await ready()
    source.hold("setAccess")
    const first = store.dispatch(
      chooseAccess({ sessionId: "a", mode: "full", initiator: "person" }),
    )
    await settle()
    expect(selectAccess(store.getState(), "a")).toBe("full")
    expect(
      await store.dispatch(
        chooseAccess({ sessionId: "a", mode: "auto", initiator: "person" }),
      ),
    ).toBe("changing")
    await source.release("setAccess")
    expect(await first).toBe("taken")
    expect(source.calls.filter((call) => call[0] === "setAccess")).toHaveLength(1)
  })

  it("holds a new session's choice for the message that begins it", async () => {
    const { store, source } = await ready()
    const draftId = store.dispatch(newSession())
    if (!draftId) throw new Error("no draft")
    expect(
      await store.dispatch(
        chooseAccess({ sessionId: draftId, mode: "full", initiator: "person" }),
      ),
    ).toBe("chosen")
    expect(selectAccess(store.getState(), draftId)).toBe("full")
    expect(source.calls.some((call) => call[0] === "setAccess")).toBe(false)
    await store.dispatch(
      sendMessage({ initiator: "person", sessionId: draftId, text: "Fix the rain" }),
    )
    const sent = source.calls.find((call) => call[0] === "send")?.[1]
    expect(sent).toMatchObject({ start: { access: "full" } })
  })

  it("sends no access with a new session's first message when none was chosen", async () => {
    const { store, source } = await ready()
    const draftId = store.dispatch(newSession())
    if (!draftId) throw new Error("no draft")
    await store.dispatch(
      sendMessage({ initiator: "person", sessionId: draftId, text: "Fix the rain" }),
    )
    const sent = source.calls.find((call) => call[0] === "send")?.[1] as {
      start?: object
    }
    expect(sent.start).not.toHaveProperty("access")
  })
})

describe("a refused change, in words", () => {
  it("says a running turn in its own words, and any other refusal in the window's", () => {
    expect(accessRefusalCopy({ reason: "unavailable", code: "turn_running" })).toBe(
      "Access can’t change while the agent is working. Try again when it stops.",
    )
    expect(accessRefusalCopy({ reason: "not-supported" })).toBe(
      "Access didn’t change. This isn’t available for this session.",
    )
  })

  it("names both modes when a model change moved the chat to Ask", () => {
    expect(steppedAccessCopy("auto")).toBe(
      "Access is now Ask for approval. This model doesn’t offer Auto approval here.",
    )
    expect(steppedAccessCopy("full")).toBe(
      "Access is now Ask for approval. This model doesn’t offer Full access here.",
    )
  })
})

/** The source says a's conversation anew, at its next revision: at work, or not. */
function sayConversation(source: FakeSource, working: boolean): Transcript {
  const held = source.transcripts.get("a") ?? { ...emptyTranscript("a"), revision: 1 }
  const next: Transcript = {
    ...held,
    activity: working ? { label: "Running the tests", since: 1 } : null,
    revision: held.revision + 1,
  }
  source.transcripts.set("a", next)
  source.emit({ kind: "transcript", transcript: next })
  return next
}

describe("access chosen while the agent is at work", () => {
  it("is asked of the source at once, so the turn's next tool call is decided at it", async () => {
    const { store, source } = await ready()
    sayConversation(source, true)
    await settle()
    expect(
      await store.dispatch(
        chooseAccess({ sessionId: "a", mode: "full", initiator: "person" }),
      ),
    ).toBe("taken")
    expect(source.calls.filter((call) => call[0] === "setAccess")).toEqual([
      ["setAccess", "a", "full", "person"],
    ])
    expect(selectAccess(store.getState(), "a")).toBe("full")
    // Nothing to say: the change is made, not held.
    expect(selectAccessNotice(store.getState(), "a")).toBeUndefined()
    // The turn ends: nothing is asked again.
    sayConversation(source, false)
    await settle(10)
    expect(source.calls.filter((call) => call[0] === "setAccess")).toHaveLength(1)
  })

  for (const code of ["turn_running", "approval_mode_not_applied"])
    it(`waits for the turn to end when the turn could not take it (${code})`, async () => {
      const source = fakeSource()
      const setAccess = source.setAccess.bind(source)
      let asked = 0
      source.setAccess = (...args) => {
        asked++
        // A message queued behind the running turn: the agent cannot change under it.
        return asked === 1
          ? Promise.reject(
              new WorkspaceSourceError("unavailable", undefined, {
                message: "Not while this turn runs",
                code,
              }),
            )
          : setAccess(...args)
      }
      const { store } = await ready(source)
      sayConversation(source, true)
      await settle()
      expect(
        await store.dispatch(
          chooseAccess({ sessionId: "a", mode: "full", initiator: "person" }),
        ),
      ).toBe("pending")
      expect(asked).toBe(1)
      // Still the person's pick, shown and not refused.
      expect(selectAccess(store.getState(), "a")).toBe("full")
      expect(selectAccessNotice(store.getState(), "a")?.state).toBe("pending")
      // Not sent again while that turn is at work.
      store.dispatch(setComposerText({ sessionId: "a", text: "x" }))
      sayConversation(source, true)
      await settle(10)
      expect(asked).toBe(1)
      sayConversation(source, false)
      await settle(10)
      expect(asked).toBe(2)
      expect(selectAccess(store.getState(), "a")).toBe("full")
      expect(store.getState().workspace.accessChoices.a).toBeUndefined()
    })

  it("says an access the model does not offer at once, mid-turn too", async () => {
    const source = fakeSource()
    source.setAccess = () =>
      Promise.reject(
        new WorkspaceSourceError("not-supported", undefined, {
          message: "This model does not offer that access",
          code: "approval_mode_unavailable",
        }),
      )
    const { store } = await ready(source)
    sayConversation(source, true)
    await settle()
    expect(
      await store.dispatch(
        chooseAccess({ sessionId: "a", mode: "full", initiator: "person" }),
      ),
    ).toBe("refused")
    expect(selectAccess(store.getState(), "a")).toBe("ask")
    expect(selectAccessNotice(store.getState(), "a")).toMatchObject({
      state: "refused",
      code: "approval_mode_unavailable",
    })
  })

  it("says a refusal until the next conversation with no turn at work", async () => {
    const { store, source } = await ready()
    source.refuse("setAccess", "unavailable")
    await store.dispatch(
      chooseAccess({ sessionId: "a", mode: "full", initiator: "person" }),
    )
    expect(selectAccessNotice(store.getState(), "a")?.state).toBe("refused")
    sayConversation(source, true)
    await settle()
    expect(selectAccessNotice(store.getState(), "a")?.state).toBe("refused")
    sayConversation(source, false)
    await settle()
    expect(selectAccessNotice(store.getState(), "a")).toBeUndefined()
  })

  it("lets a refusal go with the next pick", async () => {
    const { store, source } = await ready()
    source.refuse("setAccess", "unavailable")
    await store.dispatch(
      chooseAccess({ sessionId: "a", mode: "full", initiator: "person" }),
    )
    source.refuse("setAccess", undefined)
    expect(
      await store.dispatch(
        chooseAccess({ sessionId: "a", mode: "full", initiator: "person" }),
      ),
    ).toBe("taken")
    expect(selectAccessNotice(store.getState(), "a")).toBeUndefined()
  })
})

describe("a new session's access", () => {
  it("keeps a pick made after the first message carried another, and applies it while the turn runs", async () => {
    const { store, source } = await ready()
    const draftId = store.dispatch(newSession())
    if (!draftId) throw new Error("no draft")
    await store.dispatch(
      chooseAccess({ sessionId: draftId, mode: "full", initiator: "person" }),
    )
    source.hold("send")
    const first = store.dispatch(
      sendMessage({ initiator: "person", sessionId: draftId, text: "Fix the rain" }),
    )
    await settle()
    expect(source.calls.find((call) => call[0] === "send")?.[1]).toMatchObject({
      start: { access: "full" },
    })
    // Chosen after the first message captured Full: the person's newer word.
    await store.dispatch(
      chooseAccess({ sessionId: draftId, mode: "ask", initiator: "person" }),
    )
    const running: Transcript = {
      ...emptyTranscript(draftId),
      revision: 1,
      activity: { label: "Thinking", since: 1 },
      access: { mode: "full", modes: ["ask", "auto", "full"] },
    }
    source.transcripts.set(draftId, running)
    source.emit({
      kind: "session",
      session: summary(draftId, "desktop", 2000, "running", { revision: 1 }),
    })
    await source.release("send")
    await first
    await settle(10)
    // The conversation began at the captured Full; the newer Ask is asked
    // of it at once, while its first turn is still at work.
    expect(source.calls.filter((call) => call[0] === "setAccess")).toEqual([
      ["setAccess", draftId, "ask", "person"],
    ])
    expect(selectAccess(store.getState(), draftId)).toBe("ask")
  })

  it("stays chosen when a refused first message is discarded and the home comes back", async () => {
    const { store, source } = await ready()
    const draftId = store.dispatch(newSession())
    if (!draftId) throw new Error("no draft")
    await store.dispatch(
      chooseAccess({ sessionId: draftId, mode: "full", initiator: "person" }),
    )
    source.refuse("send", "not-supported")
    await store.dispatch(
      sendMessage({ initiator: "person", sessionId: draftId, text: "Fix the rain" }),
    )
    const [refused] = store.getState().workspace.outbox[draftId] ?? []
    if (!refused) throw new Error("no refused message")
    store.dispatch(discardUnsent({ sessionId: draftId, messageId: refused.id }))
    expect(store.getState().workspace.drafts[draftId]).toBeDefined()
    expect(selectAccess(store.getState(), draftId)).toBe("full")
  })
})
