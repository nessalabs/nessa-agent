import { describe, expect, it } from "vitest"
import type { Message } from "../../model/transcript"
import { emptyTranscript } from "../../model/transcript"
import { astra, shownIn, summary, testIndex } from "../../testing"
import {
  initialWorkspace,
  modelForNextTurn,
  type WorkspaceState,
} from "../workspace-state"
import { createDraft, openSession } from "./panes"
import {
  approvalAnswering,
  approvalFailed,
  chooseModel,
  composerTextChanged,
  messageDelivered,
  messageResent,
  messageSent,
  sendFailed,
  sessionRead,
  unsentDiscarded,
} from "./sessions"
import { indexLoaded, sessionRemoved, transcriptLoaded, updateReceived } from "./updates"

const loaded = () =>
  indexLoaded(initialWorkspace, {
    index: testIndex(),
    draftId: "unused",
    read: "r",
  })

const shown = (state: WorkspaceState) => shownIn(state.panes)

const message = (id: string, text: string, at = 500): Message => ({
  id,
  role: "user",
  at,
  parts: [{ kind: "text", text }],
  delivery: { state: "sending" },
})

describe("sending", () => {
  it("starts a draft with its first message: titled, listed at revision 0, running, keeping its id", () => {
    const drafted = createDraft(loaded(), { draftId: "new" })
    const sent = messageSent(drafted, {
      initiator: "person",
      sessionId: "new",
      message: message("m1", "fix the rain. then the steam"),
    })
    expect(sent.drafts).toEqual({})
    expect(sent.sessions.new).toMatchObject({
      title: "Fix the rain",
      status: "running",
      channelId: "desktop",
      revision: 0,
    })
    expect(sent.outbox.new.map((m) => m.id)).toEqual(["m1"])
    expect(sent.transcripts.new).toBeUndefined()
    expect(shown(sent)).toEqual(["new"])
  })

  it("puts a message in the outbox, leaving the source's session and conversation as they were", () => {
    const withC = transcriptLoaded(loaded(), {
      transcript: { ...emptyTranscript("c"), revision: 1 },
    })
    const sent = messageSent(withC, {
      initiator: "person",
      sessionId: "c",
      message: message("m1", "again", 700),
    })
    expect(sent.sessions.c).toBe(withC.sessions.c)
    expect(sent.transcripts.c).toBe(withC.transcripts.c)
    expect(sent.outbox.c).toHaveLength(1)
  })

  it("keeps a sent message through any replacement until the source's conversation holds it", () => {
    const shownC = openSession(loaded(), { sessionId: "c" })
    const sent = messageSent(shownC, {
      initiator: "person",
      sessionId: "c",
      message: message("m1", "hi"),
    })
    const read = transcriptLoaded(sent, {
      transcript: { ...emptyTranscript("c"), revision: 5 },
    })
    expect(read.outbox.c.map((m) => m.id)).toEqual(["m1"])
    const held = updateReceived(read, {
      update: {
        kind: "transcript",
        transcript: {
          ...emptyTranscript("c"),
          revision: 6,
          messages: [{ ...message("m1", "hi"), delivery: undefined }],
        },
      },
    })
    expect(held.outbox.c).toBeUndefined()
    expect(held.transcripts.c.messages.map((m) => m.id)).toEqual(["m1"])
  })

  it("ignores a message to a session it does not have", () => {
    const state = loaded()
    expect(
      messageSent(state, {
        initiator: "person",
        sessionId: "missing",
        message: message("m", "x"),
      }),
    ).toBe(state)
  })

  it("clears the sending mark once delivered, and only a sending one", () => {
    const sent = messageSent(loaded(), {
      initiator: "person",
      sessionId: "c",
      message: message("m1", "hi"),
    })
    const delivered = messageDelivered(sent, { sessionId: "c", messageId: "m1" })
    expect(delivered.outbox.c[0].delivery).toBeUndefined()
    expect(messageDelivered(delivered, { sessionId: "c", messageId: "m1" })).toBe(
      delivered,
    )
    expect(messageDelivered(delivered, { sessionId: "missing", messageId: "m1" })).toBe(
      delivered,
    )
  })

  it("marks a refused message not sent, with the reason, leaving the source's session as it said", () => {
    const sent = messageSent(loaded(), {
      initiator: "person",
      sessionId: "c",
      message: message("m1", "hi"),
    })
    const failed = sendFailed(sent, {
      sessionId: "c",
      messageId: "m1",
      reason: "unavailable",
    })
    expect(failed.outbox.c[0].delivery).toEqual({
      state: "failed",
      reason: "unavailable",
    })
    expect(failed.sessions.c).toBe(sent.sessions.c)
  })

  it("puts a session the source never began at rest when its first message is refused", () => {
    const drafted = createDraft(loaded(), { draftId: "new" })
    const sent = messageSent(drafted, {
      initiator: "person",
      sessionId: "new",
      message: message("m1", "hi"),
    })
    const failed = sendFailed(sent, {
      sessionId: "new",
      messageId: "m1",
      reason: "unavailable",
    })
    expect(failed.sessions.new).toMatchObject({ status: "idle", revision: 0 })
  })

  it("shows a session the source never began running while a message that starts it is on its way", () => {
    const drafted = createDraft(loaded(), { draftId: "new" })
    const sent = messageSent(drafted, {
      initiator: "person",
      sessionId: "new",
      message: message("m1", "hi"),
    })
    const refused = sendFailed(sent, {
      sessionId: "new",
      messageId: "m1",
      reason: "unavailable",
    })
    expect(refused.sessions.new.status).toBe("idle")
    const again = messageSent(refused, {
      initiator: "person",
      sessionId: "new",
      message: message("m2", "again"),
    })
    expect(again.sessions.new.status).toBe("running")
    const resent = messageResent(refused, { sessionId: "new", messageId: "m1" })
    expect(resent.sessions.new.status).toBe("running")
    expect(resent.outbox.new).toEqual([{ ...message("m1", "hi"), observedInput: null }])
  })

  it("keeps a session the source never began running while another message may still begin it", () => {
    const drafted = createDraft(loaded(), { draftId: "new" })
    const first = messageSent(drafted, {
      initiator: "person",
      sessionId: "new",
      message: message("m1", "one"),
    })
    const delivered = messageDelivered(first, { sessionId: "new", messageId: "m1" })
    const second = messageSent(delivered, {
      initiator: "person",
      sessionId: "new",
      message: message("m2", "two"),
    })
    const refused = sendFailed(second, {
      sessionId: "new",
      messageId: "m2",
      reason: "unavailable",
    })
    expect(refused.sessions.new.status).toBe("running")
    const sending = messageSent(drafted, {
      initiator: "person",
      sessionId: "new",
      message: message("m1", "one"),
    })
    const alsoSent = messageSent(sending, {
      initiator: "person",
      sessionId: "new",
      message: message("m2", "two"),
    })
    expect(
      sendFailed(alsoSent, { sessionId: "new", messageId: "m2", reason: "unavailable" })
        .sessions.new.status,
    ).toBe("running")
  })

  it("sends a refused message again in its place, and only a refused one", () => {
    const one = messageSent(loaded(), {
      initiator: "person",
      sessionId: "c",
      message: message("m1", "one"),
    })
    const two = messageSent(one, {
      initiator: "person",
      sessionId: "c",
      message: message("m2", "two"),
    })
    const refused = sendFailed(two, {
      sessionId: "c",
      messageId: "m1",
      reason: "unavailable",
    })
    const resent = messageResent(refused, { sessionId: "c", messageId: "m1" })
    expect(resent.outbox.c.map((m) => [m.id, m.delivery?.state])).toEqual([
      ["m1", "sending"],
      ["m2", "sending"],
    ])
    expect(messageResent(two, { sessionId: "c", messageId: "m1" })).toBe(two)
  })

  it("lets a refused message go, and only a refused one", () => {
    const sent = messageSent(loaded(), {
      initiator: "person",
      sessionId: "c",
      message: message("m1", "hi"),
    })
    expect(unsentDiscarded(sent, { sessionId: "c", messageId: "m1" })).toBe(sent)
    const failed = sendFailed(sent, {
      sessionId: "c",
      messageId: "m1",
      reason: "unavailable",
    })
    expect(
      unsentDiscarded(failed, { sessionId: "c", messageId: "m1" }).outbox.c,
    ).toBeUndefined()
  })

  it("keeps a refused message when a newer conversation arrives without it", () => {
    const sent = messageSent(loaded(), {
      initiator: "person",
      sessionId: "c",
      message: message("m1", "hi"),
    })
    const failed = sendFailed(sent, {
      sessionId: "c",
      messageId: "m1",
      reason: "unavailable",
    })
    const newer = updateReceived(failed, {
      update: {
        kind: "transcript",
        transcript: { ...emptyTranscript("c"), revision: 9 },
      },
    })
    expect(newer.outbox.c[0].delivery?.state).toBe("failed")
  })
})

describe("what the person changes", () => {
  it("chooses the model of a draft, or of a session's next turn beside its summary", () => {
    const drafted = createDraft(loaded(), { draftId: "new" })
    const draftChosen = chooseModel(drafted, { sessionId: "new", model: astra })
    expect(draftChosen.drafts.new.model).toEqual(astra)
    expect(modelForNextTurn(draftChosen, "new")).toEqual(astra)
    const state = loaded()
    const chosen = chooseModel(state, { sessionId: "a", model: astra })
    expect(chosen.sessions.a).toBe(state.sessions.a)
    expect(modelForNextTurn(chosen, "a")).toEqual(astra)
    expect(modelForNextTurn(state, "a")).toEqual(state.sessions.a.model)
    expect(modelForNextTurn(state, "missing")).toBeUndefined()
    expect(chooseModel(state, { sessionId: "missing", model: astra })).toBe(state)
    // No summary speaks for the person, whichever model it names.
    const same = updateReceived(chosen, {
      update: {
        kind: "session",
        session: summary("a", "desktop", 900, "running", { model: astra, revision: 2 }),
      },
    })
    const other = updateReceived(same, {
      update: {
        kind: "session",
        session: summary("a", "desktop", 901, "running", { revision: 3 }),
      },
    })
    expect(modelForNextTurn(other, "a")).toEqual(astra)
  })

  it("holds an answer beside the approval while it is on its way, and why it failed", () => {
    const approval = {
      id: "ap",
      command: "cargo test",
      reason: "Runs tests.",
      origin: { kind: "agent" } as const,
      options: [],
    }
    const state = transcriptLoaded(loaded(), {
      transcript: { ...emptyTranscript("b"), revision: 1, approval },
    })
    const answering = approvalAnswering(state, {
      sessionId: "b",
      approvalId: "ap",
      token: "t1",
    })
    expect(answering.answers.b).toEqual({ approvalId: "ap", token: "t1" })
    expect(answering.transcripts.b).toBe(state.transcripts.b)
    const failed = approvalFailed(answering, {
      sessionId: "b",
      approvalId: "ap",
      token: "t1",
      reason: "not-waiting",
    })
    expect(failed.answers.b).toEqual({
      approvalId: "ap",
      token: "t1",
      failure: "not-waiting",
    })
    // An earlier answer's late refusal says nothing of the one on its way.
    expect(
      approvalFailed(answering, {
        sessionId: "b",
        approvalId: "ap",
        token: "t0",
        reason: "unavailable",
      }),
    ).toBe(answering)
    expect(
      approvalFailed(answering, {
        sessionId: "b",
        approvalId: "other",
        token: "t1",
        reason: "not-waiting",
      }),
    ).toBe(answering)
    const again = approvalAnswering(failed, {
      sessionId: "b",
      approvalId: "ap",
      token: "t2",
    })
    expect(again.answers.b.failure).toBeUndefined()
  })

  it("lets an answer go once the source's conversation no longer asks", () => {
    const approval = {
      id: "ap",
      command: "cargo test",
      reason: "Runs tests.",
      origin: { kind: "agent" } as const,
      options: [],
    }
    const state = transcriptLoaded(openSession(loaded(), { sessionId: "b" }), {
      transcript: { ...emptyTranscript("b"), revision: 1, approval },
    })
    const answering = approvalAnswering(state, {
      sessionId: "b",
      approvalId: "ap",
      token: "t1",
    })
    const still = updateReceived(answering, {
      update: {
        kind: "transcript",
        transcript: { ...emptyTranscript("b"), revision: 2, approval },
      },
    })
    expect(still.answers.b).toEqual({ approvalId: "ap", token: "t1" })
    const answered = updateReceived(still, {
      update: {
        kind: "transcript",
        transcript: { ...emptyTranscript("b"), revision: 3 },
      },
    })
    expect(answered.answers.b).toBeUndefined()
    // Its pane showing another session meanwhile: the answer on its way stays.
    expect(openSession(still, { sessionId: "a" }).answers.b).toEqual(still.answers.b)
  })

  it("clears a session's unread mark, changing nothing when it is read", () => {
    const state = updateReceived(loaded(), {
      update: {
        kind: "session",
        session: summary("c", "desktop", 900, "idle", { revision: 5, unread: true }),
      },
    })
    const read = sessionRead(state, { sessionId: "c" })
    expect(read.sessions.c.unread).toBe(false)
    expect(sessionRead(read, { sessionId: "c" })).toBe(read)
    expect(sessionRead(read, { sessionId: "missing" })).toBe(read)
  })

  it("takes a session the source never began back to a new session's home when its last refused message goes", () => {
    const drafted = createDraft(loaded(), { draftId: "new" })
    const sent = messageSent(drafted, {
      initiator: "person",
      sessionId: "new",
      message: message("m1", "hi"),
    })
    const refused = sendFailed(sent, {
      sessionId: "new",
      messageId: "m1",
      reason: "unavailable",
    })
    const chosen = chooseModel(refused, { sessionId: "new", model: astra })
    const discarded = unsentDiscarded(chosen, { sessionId: "new", messageId: "m1" })
    expect(discarded.sessions.new).toBeUndefined()
    expect(discarded.chosenModels).toEqual({})
    expect(discarded.drafts.new).toEqual({
      id: "new",
      channelId: "desktop",
      model: astra,
    })
    expect(shown(discarded)).toEqual(["new"])
  })

  it("lets a session the source never began go from the lists when no pane shows it", () => {
    const drafted = createDraft(loaded(), { draftId: "new" })
    const sent = messageSent(drafted, {
      initiator: "person",
      sessionId: "new",
      message: message("m1", "hi"),
    })
    const refused = sendFailed(sent, {
      sessionId: "new",
      messageId: "m1",
      reason: "unavailable",
    })
    const elsewhere = openSession(refused, { sessionId: "a" })
    const discarded = unsentDiscarded(elsewhere, { sessionId: "new", messageId: "m1" })
    expect(discarded.sessions.new).toBeUndefined()
    expect(discarded.drafts).toEqual({})
  })

  it("keeps a session the source has spoken of listed when its refused message goes", () => {
    const sent = messageSent(loaded(), {
      initiator: "person",
      sessionId: "c",
      message: message("m1", "hi"),
    })
    const refused = sendFailed(sent, {
      sessionId: "c",
      messageId: "m1",
      reason: "unavailable",
    })
    const discarded = unsentDiscarded(refused, { sessionId: "c", messageId: "m1" })
    expect(discarded.sessions.c).toBe(refused.sessions.c)
    expect(discarded.drafts).toEqual({})
  })
})

describe("what is typed and not sent", () => {
  const loadedState = () =>
    indexLoaded(initialWorkspace, { index: testIndex(), draftId: "unused", read: "r" })

  it("is kept beside the session, whoever writes it, and outlives the pane showing another", () => {
    const typed = composerTextChanged(loadedState(), {
      sessionId: "a",
      text: "half a thought",
    })
    const elsewhere = openSession(typed, { sessionId: "c" })
    expect(elsewhere.composerText.a).toBe("half a thought")
    const back = openSession(elsewhere, { sessionId: "a" })
    expect(back.composerText.a).toBe("half a thought")
  })

  it("keeps nothing for an empty field, or for a session the window does not hold", () => {
    const state = loadedState()
    const typed = composerTextChanged(state, { sessionId: "a", text: "x" })
    expect(composerTextChanged(typed, { sessionId: "a", text: "" }).composerText).toEqual(
      {},
    )
    expect(composerTextChanged(state, { sessionId: "missing", text: "x" })).toBe(state)
    expect(composerTextChanged(state, { sessionId: "constructor", text: "x" })).toBe(
      state,
    )
  })

  it("goes when the person sends it, and stays when an agent sends a message of its own", () => {
    const typed = composerTextChanged(loadedState(), {
      sessionId: "a",
      text: "half a thought",
    })
    const byAgent = messageSent(typed, {
      initiator: "agent",
      sessionId: "a",
      message: message("m1", "the agent's words"),
    })
    expect(byAgent.composerText.a).toBe("half a thought")
    const byPerson = messageSent(typed, {
      initiator: "person",
      sessionId: "a",
      message: message("m2", "half a thought"),
    })
    expect(byPerson.composerText.a).toBeUndefined()
  })

  it("goes with its session: removed, or a new session no pane shows any more", () => {
    const typed = composerTextChanged(loadedState(), { sessionId: "a", text: "x" })
    const removed = sessionRemoved(typed, {
      sessionId: "a",
      revision: 2,
      draftId: "fresh",
    })
    expect(removed.composerText).toEqual({})
    const drafted = composerTextChanged(createDraft(loadedState(), { draftId: "new" }), {
      sessionId: "new",
      text: "y",
    })
    expect(drafted.composerText.new).toBe("y")
    const replaced = openSession(drafted, { sessionId: "c" })
    expect(replaced.composerText.new).toBeUndefined()
  })
})
