import { describe, expect, it } from "vitest"
import { panesOf, paneShowing } from "../../model/pane-layout"
import { retention } from "../../model/retention"
import { emptyTranscript, type Transcript } from "../../model/transcript"
import { astra, roomyGrid as roomy, summary, testIndex } from "../../testing"
import {
  initialWorkspace,
  modelForNextTurn,
  type WorkspaceState,
} from "../workspace-state"
import { closePane, createDraft, openBeside } from "./panes"
import { chooseModel, messageSent } from "./sessions"
import {
  indexFailed,
  indexLoaded,
  indexRequested,
  sessionRemoved,
  shownSessionIds,
  transcriptFailed,
  transcriptLoaded,
  transcriptRetried,
  unreadShown,
  updateReceived,
} from "./updates"

const loaded = () =>
  indexLoaded(initialWorkspace, {
    index: testIndex(),
    draftId: "unused",
  })

const shown = (state: WorkspaceState) =>
  panesOf(state.panes!).map((pane) => pane.sessionId)

const transcript = (
  sessionId: string,
  revision: number,
  text = `r${revision}`,
): Transcript => ({
  ...emptyTranscript(sessionId),
  revision,
  messages: [
    { id: `m${revision}`, role: "agent", at: revision, parts: [{ kind: "text", text }] },
  ],
})

describe("the index arriving", () => {
  it("opens on the first channel's newest session and discloses what waits", () => {
    const state = loaded()
    expect(state.status).toBe("ready")
    expect(state.view).toEqual({ kind: "channel", channelId: "desktop" })
    expect(shown(state)).toEqual(["a"])
    expect(state.tree.expandedChannels).toEqual(["desktop"])
  })

  it("opens on a new session's home when there are no sessions", () => {
    const state = indexLoaded(initialWorkspace, {
      index: { ...testIndex(), sessions: [] },
      draftId: "first",
    })
    expect(shown(state)).toEqual(["first"])
    expect(state.drafts.first.channelId).toBe("desktop")
  })

  it("shows nothing at all with no channels to start in", () => {
    const state = indexLoaded(initialWorkspace, {
      index: { sections: [], channels: [], sessions: [] },
      draftId: "first",
    })
    expect(state.panes).toBeNull()
    expect(state.status).toBe("ready")
  })

  it("keeps the panes it has when the index is read again", () => {
    const two = openBeside(loaded(), { sessionId: "c", room: roomy })
    const again = indexLoaded(two, {
      index: testIndex(),
      draftId: "x",
    })
    expect(again.panes).toBe(two.panes)
  })

  it("says why it could not be read", () => {
    const state = indexFailed(initialWorkspace, { reason: "unavailable" })
    expect(state.status).toBe("failed")
    expect(state.failure).toBe("unavailable")
  })
})

describe("updates from the source", () => {
  it("replaces or adds a session's summary", () => {
    const changed = summary("a", "desktop", 900, "idle", { revision: 2 })
    const state = updateReceived(loaded(), {
      update: { kind: "session", session: changed },
    })
    expect(state.sessions.a).toBe(changed)
    const added = updateReceived(loaded(), {
      update: { kind: "session", session: summary("z", "gateway", 1) },
    })
    expect(Object.keys(added.sessions)).toContain("z")
  })

  it("holds a conversation's update whether or not it was read yet", () => {
    const update = {
      ...emptyTranscript("a"),
      revision: 1,
      activity: { label: "Thinking", since: 1 },
    }
    const unread = updateReceived(loaded(), {
      update: { kind: "transcript", transcript: update },
    })
    expect(unread.transcripts.a).toBe(update)
  })

  it("refuses a replacement at a revision the source could not have sent", () => {
    const state = loaded()
    for (const revision of [0, -1, 1.5, Number.NaN]) {
      expect(
        updateReceived(state, {
          update: {
            kind: "session",
            session: summary("x", "desktop", 1, "idle", { revision }),
          },
        }),
      ).toBe(state)
      expect(
        updateReceived(state, {
          update: {
            kind: "transcript",
            transcript: { ...emptyTranscript("a"), revision },
          },
        }),
      ).toBe(state)
      expect(sessionRemoved(state, { sessionId: "a", revision, draftId: "fresh" })).toBe(
        state,
      )
    }
  })

  it("closes the pane of a removed session, and starts the last pane over: no pane shows a session that is not there", () => {
    const two = openBeside(loaded(), { sessionId: "c", room: roomy })
    const removed = sessionRemoved(two, { sessionId: "c", revision: 2, draftId: "fresh" })
    expect(shown(removed)).toEqual(["a"])
    expect(Object.keys(removed.sessions)).not.toContain("c")
    // The only legal outcome for the last pane: a new session's home in the same channel.
    const last = sessionRemoved(loaded(), {
      sessionId: "a",
      revision: 2,
      draftId: "fresh",
    })
    expect(shown(last)).toEqual(["fresh"])
    expect(last.drafts.fresh.channelId).toBe("desktop")
    expect(last.sessions.a).toBeUndefined()
    // A removal of a session not yet listed is remembered, in case an older read lists it.
    const unknown = loaded()
    const remembered = sessionRemoved(unknown, {
      sessionId: "missing",
      revision: 1,
      draftId: "fresh",
    })
    expect(remembered.sessions).toBe(unknown.sessions)
    expect(remembered.removed).toEqual([{ sessionId: "missing", revision: 1 }])
  })

  it("remembers why a conversation could not be read, until it is", () => {
    const failed = transcriptFailed(loaded(), {
      sessionId: "a",
      reason: "unknown-session",
    })
    expect(failed.transcriptFailures.a).toBe("unknown-session")
    const read = transcriptLoaded(failed, { transcript: emptyTranscript("a") })
    expect(read.transcriptFailures).toEqual({})
  })

  it("lists only the shown sessions the source has, drafts never among them", () => {
    const drafted = createDraft(loaded(), {
      draftId: "new",
      beside: "right",
      room: roomy,
    })
    expect(shownSessionIds(drafted)).toEqual(["a"])
    expect(shownSessionIds(initialWorkspace)).toEqual([])
  })
})

describe("what goes with a session", () => {
  it("names the shown sessions still marked unread, the one the workspace opens on included", () => {
    const index = testIndex()
    const state = indexLoaded(initialWorkspace, {
      index: {
        ...index,
        sessions: index.sessions.map((session) =>
          session.id === "a" ? { ...session, unread: true } : session,
        ),
      },
      draftId: "x",
    })
    expect(unreadShown(state)).toEqual(["a"])
    expect(unreadShown(loaded())).toEqual([])
    expect(unreadShown(initialWorkspace)).toEqual([])
  })

  it("holds no conversation, update or failure for a session it does not list", () => {
    const state = loaded()
    expect(
      updateReceived(state, {
        update: { kind: "transcript", transcript: transcript("missing", 4) },
      }),
    ).toBe(state)
    expect(transcriptLoaded(state, { transcript: transcript("missing", 4) })).toBe(state)
    expect(transcriptFailed(state, { sessionId: "missing", reason: "unavailable" })).toBe(
      state,
    )
  })

  it("lets a listed summary replace a new session's home under the same id", () => {
    const drafted = createDraft(loaded(), { draftId: "new" })
    const listed = updateReceived(drafted, {
      update: { kind: "session", session: summary("new", "desktop", 900, "running") },
    })
    expect(listed.drafts).toEqual({})
    expect(listed.sessions.new.revision).toBe(1)
  })

  it("records no failed read for a session no pane shows, and forgets one when its pane closes", () => {
    const state = loaded()
    expect(transcriptFailed(state, { sessionId: "c", reason: "unavailable" })).toBe(state)
    const two = openBeside(state, { sessionId: "c", room: roomy })
    const failed = transcriptFailed(two, { sessionId: "c", reason: "unavailable" })
    expect(failed.transcriptFailures.c).toBeDefined()
    const closed = closePane(failed, { pane: paneShowing(failed.panes!, "c")!.key })
    expect(closed.transcriptFailures).toEqual({})
  })

  it("records no failed read once a conversation is held", () => {
    const held = transcriptLoaded(loaded(), { transcript: transcript("a", 3) })
    expect(transcriptFailed(held, { sessionId: "a", reason: "unavailable" })).toBe(held)
  })

  it("carries a model chosen in a home over when the source lists a session under its id", () => {
    const drafted = chooseModel(createDraft(loaded(), { draftId: "new" }), {
      sessionId: "new",
      model: astra,
    })
    const listed = updateReceived(drafted, {
      update: { kind: "session", session: summary("new", "desktop", 900, "running") },
    })
    expect(listed.drafts).toEqual({})
    expect(listed.chosenModels.new).toEqual(astra)
  })

  it("keeps the model chosen in a home once its first message starts it, whatever the source's summary names", () => {
    const drafted = chooseModel(createDraft(loaded(), { draftId: "new" }), {
      sessionId: "new",
      model: astra,
    })
    const sent = messageSent(drafted, {
      initiator: "person",
      sessionId: "new",
      message: { id: "m1", role: "user", at: 1, parts: [{ kind: "text", text: "hi" }] },
    })
    const listed = updateReceived(sent, {
      update: { kind: "session", session: summary("new", "desktop", 900, "running") },
    })
    expect(listed.sessions.new.model).not.toEqual(astra)
    expect(modelForNextTurn(listed, "new")).toEqual(astra)
  })

  it("lists a summary the same way whether the index read or the stream brings it", () => {
    const drafted = createDraft(loaded(), { draftId: "new" })
    const reread = indexLoaded(drafted, {
      index: {
        ...testIndex(),
        sessions: [summary("new", "desktop", 900, "running")],
      },
      draftId: "unused",
    })
    expect(reread.drafts).toEqual({})
    expect(reread.sessions.new.revision).toBe(1)
  })

  it("forgets a failed read once the conversation arrives by update", () => {
    const failed = transcriptFailed(loaded(), { sessionId: "a", reason: "unavailable" })
    const arrived = updateReceived(failed, {
      update: { kind: "transcript", transcript: transcript("a", 2) },
    })
    expect(arrived.transcriptFailures).toEqual({})
  })

  it("closes the pane showing a removed session, and starts the last over under the id given", () => {
    const two = openBeside(loaded(), { sessionId: "c", room: roomy })
    const closed = sessionRemoved(two, { sessionId: "c", revision: 3, draftId: "x" })
    expect(panesOf(closed.panes!).map((pane) => pane.sessionId)).toEqual(["a"])
    const last = sessionRemoved(loaded(), {
      sessionId: "a",
      revision: 3,
      draftId: "fresh",
    })
    expect(panesOf(last.panes!).map((pane) => pane.sessionId)).toEqual(["fresh"])
    expect(last.drafts.fresh.channelId).toBe("desktop")
    const chosen = sessionRemoved(
      { ...loaded(), chosenModels: { a: astra } },
      { sessionId: "a", revision: 3, draftId: "fresh" },
    )
    // The model the person chose for its next turn carries over to the new home.
    expect(chosen.drafts.fresh.model).toEqual(astra)
    const waiting = sessionRemoved(
      { ...loaded(), view: { kind: "status", status: "running" } },
      { sessionId: "a", revision: 3, draftId: "fresh" },
    )
    expect(waiting.view).toEqual({ kind: "status", status: "running" })
  })

  it("drops an answer and a chosen model with a removed session", () => {
    const state = {
      ...loaded(),
      answers: { c: { approvalId: "ap", token: "t1" } },
      chosenModels: { c: testIndex().sessions[3].model },
    }
    const removed = sessionRemoved(state, {
      sessionId: "c",
      revision: 3,
      draftId: "fresh",
    })
    expect(removed.answers).toEqual({})
    expect(removed.chosenModels).toEqual({})
  })

  it("drops a removed session's conversation, failure and outbox, and a read that outlived it", () => {
    const failed = transcriptFailed(loaded(), { sessionId: "c", reason: "unavailable" })
    const state = { ...failed, transcripts: { c: transcript("c", 2) } }
    const removed = sessionRemoved(state, {
      sessionId: "c",
      revision: 3,
      draftId: "fresh",
    })
    expect(removed.transcripts.c).toBeUndefined()
    expect(removed.transcriptFailures.c).toBeUndefined()
    expect(transcriptLoaded(removed, { transcript: transcript("c", 4) })).toBe(removed)
  })

  it("keeps an open workspace open when a later read of the index fails", () => {
    const state = loaded()
    expect(indexFailed(state, { reason: "unavailable" })).toBe(state)
  })

  it("says nothing while the index is read again after a failure", () => {
    const failed = indexFailed(initialWorkspace, { reason: "unavailable" })
    expect(indexRequested(failed)).toMatchObject({
      status: "loading",
      failure: null,
    })
    const ready = loaded()
    expect(indexRequested(ready)).toMatchObject({
      status: "ready",
      failure: null,
      reading: { reads: 1, heard: [] },
    })
  })

  it("does not read a session the source has not spoken of yet", () => {
    const drafted = createDraft(loaded(), { draftId: "new" })
    const started = {
      ...drafted,
      sessions: {
        ...drafted.sessions,
        new: summary("new", "desktop", 1, "running", { revision: 0 }),
      },
      drafts: {},
    }
    expect(shownSessionIds(started)).toEqual([])
  })
})

describe("replacements out of order", () => {
  it("keeps an update that overtook a read, and a read that overtook an update", () => {
    const overtaken = transcriptLoaded(
      updateReceived(loaded(), {
        update: { kind: "transcript", transcript: transcript("a", 5) },
      }),
      { transcript: transcript("a", 3) },
    )
    expect(overtaken.transcripts.a.revision).toBe(5)
    const late = updateReceived(
      transcriptLoaded(loaded(), { transcript: transcript("a", 5) }),
      {
        update: { kind: "transcript", transcript: transcript("a", 3) },
      },
    )
    expect(late.transcripts.a.revision).toBe(5)
  })

  it("applies the same replacement twice without harm", () => {
    const once = transcriptLoaded(loaded(), { transcript: transcript("a", 4) })
    const twice = updateReceived(once, {
      update: { kind: "transcript", transcript: transcript("a", 4) },
    })
    expect(twice.transcripts.a).toEqual(once.transcripts.a)
  })

  it("keeps summaries the stream brought before an older index read", () => {
    const early = updateReceived(initialWorkspace, {
      update: {
        kind: "session",
        session: summary("a", "desktop", 999, "idle", { revision: 7 }),
      },
    })
    const state = indexLoaded(early, {
      index: testIndex(),
      draftId: "x",
    })
    expect(state.sessions.a).toMatchObject({ status: "idle", revision: 7 })
    expect(state.sessions.b.revision).toBe(1)
  })

  it("does not bring back a session removed before an older index read", () => {
    const removed = sessionRemoved(initialWorkspace, {
      sessionId: "b",
      revision: 2,
      draftId: "fresh",
    })
    const state = indexLoaded(removed, {
      index: testIndex(),
      draftId: "x",
    })
    expect(state.sessions.b).toBeUndefined()
  })

  it("ignores a removal older than the summary held, and a summary older than a removal", () => {
    const newer = updateReceived(loaded(), {
      update: {
        kind: "session",
        session: summary("b", "desktop", 1, "idle", { revision: 9 }),
      },
    })
    const stale = sessionRemoved(newer, { sessionId: "b", revision: 4, draftId: "fresh" })
    expect(stale).toBe(newer)
    const removed = sessionRemoved(loaded(), {
      sessionId: "c",
      revision: 6,
      draftId: "fresh",
    })
    const older = updateReceived(removed, {
      update: {
        kind: "session",
        session: summary("c", "desktop", 1, "idle", { revision: 5 }),
      },
    })
    expect(older.sessions.c).toBeUndefined()
    const relisted = updateReceived(removed, {
      update: {
        kind: "session",
        session: summary("c", "desktop", 1, "idle", { revision: 7 }),
      },
    })
    expect(relisted.sessions.c.revision).toBe(7)
    expect(relisted.removed).toEqual([])
  })

  it("applies a removal the source repeats once", () => {
    const removed = sessionRemoved(loaded(), {
      sessionId: "c",
      revision: 2,
      draftId: "fresh",
    })
    expect(
      sessionRemoved(removed, { sessionId: "c", revision: 2, draftId: "fresh" }).sessions,
    ).toBe(removed.sessions)
  })

  it("keeps the view it has when the index is read again", () => {
    const viewing = { ...loaded(), view: { kind: "status", status: "running" } as const }
    const again = indexLoaded(viewing, {
      index: testIndex(),
      draftId: "x",
    })
    expect(again.view).toEqual(viewing.view)
  })

  it("retries a refused read only when asked", () => {
    const failed = transcriptFailed(loaded(), { sessionId: "a", reason: "unavailable" })
    expect(transcriptRetried(failed, { sessionId: "a" }).transcriptFailures).toEqual({})
    const state = loaded()
    expect(transcriptRetried(state, { sessionId: "a" })).toBe(state)
  })
})

describe("the index read again: the resync", () => {
  const without = (...ids: string[]) => ({
    ...testIndex(),
    sessions: testIndex().sessions.filter((session) => !ids.includes(session.id)),
  })

  it("takes out a session it no longer lists, closing its pane, and keeps it out", () => {
    const two = openBeside(loaded(), { sessionId: "c", room: roomy })
    const resynced = indexLoaded(indexRequested(two), {
      index: without("c"),
      draftId: "x",
    })
    expect(resynced.sessions.c).toBeUndefined()
    expect(shown(resynced)).toEqual(["a"])
    // Taken out at the revision held: an older summary stays out, a newer one lists it again.
    expect(resynced.removed).toEqual([{ sessionId: "c", revision: 1 }])
    const stale = updateReceived(resynced, {
      update: { kind: "session", session: summary("c", "desktop", 1) },
    })
    expect(stale.sessions.c).toBeUndefined()
    const relisted = updateReceived(resynced, {
      update: {
        kind: "session",
        session: summary("c", "desktop", 1, "idle", { revision: 2 }),
      },
    })
    expect(relisted.sessions.c.revision).toBe(2)
  })

  it("starts the last pane over when the session it shows is no longer listed", () => {
    const resynced = indexLoaded(indexRequested(loaded()), {
      index: without("a"),
      draftId: "fresh",
    })
    expect(shown(resynced)).toEqual(["fresh"])
    expect(resynced.drafts.fresh.channelId).toBe("desktop")
  })

  it("takes out a session listed in a channel the index no longer lists", () => {
    const index = testIndex()
    const resynced = indexLoaded(indexRequested(loaded()), {
      index: {
        ...index,
        channels: index.channels.filter((channel) => channel.id !== "gateway"),
      },
      draftId: "x",
    })
    expect(resynced.sessions.d).toBeUndefined()
    expect(Object.keys(resynced.sessions).sort()).toEqual(["a", "b", "c"])
  })

  it("keeps a session the stream brought while the read was on its way", () => {
    const asked = indexRequested(loaded())
    const brought = updateReceived(asked, {
      update: { kind: "session", session: summary("z", "gateway", 900) },
    })
    const resynced = indexLoaded(brought, { index: testIndex(), draftId: "x" })
    expect(resynced.sessions.z).toBeDefined()
    expect(resynced.reading).toBeNull()
    // Heard before the read was asked, it is not: the read is the newer word.
    const early = updateReceived(loaded(), {
      update: { kind: "session", session: summary("z", "gateway", 900) },
    })
    const reread = indexLoaded(indexRequested(early), {
      index: testIndex(),
      draftId: "x",
    })
    expect(reread.sessions.z).toBeUndefined()
  })

  it("keeps what the stream brought until the last of two reads on their way answers", () => {
    const twice = indexRequested(indexRequested(loaded()))
    const brought = updateReceived(twice, {
      update: { kind: "session", session: summary("z", "gateway", 900) },
    })
    const first = indexLoaded(brought, { index: testIndex(), draftId: "x" })
    expect(first.sessions.z).toBeDefined()
    expect(first.reading).toEqual({ reads: 1, heard: ["z"] })
    const second = indexLoaded(first, { index: testIndex(), draftId: "y" })
    expect(second.sessions.z).toBeDefined()
    expect(second.reading).toBeNull()
  })

  it("keeps a session the source has not spoken of yet: it is the window's own", () => {
    const drafted = createDraft(loaded(), { draftId: "new" })
    const started = messageSent(drafted, {
      initiator: "person",
      sessionId: "new",
      message: { id: "m1", role: "user", at: 1, parts: [{ kind: "text", text: "hi" }] },
    })
    const resynced = indexLoaded(indexRequested(started), {
      index: testIndex(),
      draftId: "x",
    })
    expect(resynced.sessions.new.revision).toBe(0)
  })

  it("looks at another channel when the one looked at is no longer listed", () => {
    const index = testIndex()
    const resynced = indexLoaded(indexRequested(loaded()), {
      index: {
        ...index,
        channels: index.channels.filter((channel) => channel.id !== "desktop"),
        sessions: index.sessions.filter((session) => session.channelId !== "desktop"),
      },
      draftId: "x",
    })
    expect(resynced.view).toEqual({ kind: "channel", channelId: "empty" })
  })
})

describe("a removal's revision", () => {
  it("is counted against the summary's revision, never the conversation's", () => {
    // The summary is at 1 and the conversation at 5: a removal at 2 outranks the summary.
    const held = transcriptLoaded(loaded(), { transcript: transcript("a", 5) })
    const removed = sessionRemoved(held, { sessionId: "a", revision: 2, draftId: "x" })
    expect(removed.sessions.a).toBeUndefined()
    // The summary is at 3 and the conversation at 1: a removal at 2 is older than it.
    const newer = updateReceived(loaded(), {
      update: {
        kind: "session",
        session: summary("c", "desktop", 1, "idle", { revision: 3 }),
      },
    })
    expect(sessionRemoved(newer, { sessionId: "c", revision: 2, draftId: "x" })).toBe(
      newer,
    )
  })
})

describe("what is kept of sessions no pane shows", () => {
  it("keeps every shown conversation, and the most recently active few of the rest", () => {
    const sessions = Array.from({ length: retention.unshownConversations + 2 }, (_, i) =>
      summary(`s${i}`, "gateway", 1000 + i),
    )
    const state = indexLoaded(initialWorkspace, {
      index: {
        ...testIndex(),
        sessions: [...testIndex().sessions, ...sessions],
      },
      draftId: "x",
    })
    const held = sessions.reduce(
      (current, session) =>
        updateReceived(current, {
          update: { kind: "transcript", transcript: transcript(session.id, 2) },
        }),
      updateReceived(state, {
        update: { kind: "transcript", transcript: transcript("a", 2) },
      }),
    )
    const kept = Object.keys(held.transcripts)
    // "a" is shown, and the eight newest of the ten others stay.
    expect(kept).toContain("a")
    expect(kept).toHaveLength(retention.unshownConversations + 1)
    expect(kept).not.toContain("s0")
    expect(kept).not.toContain("s1")
    expect(kept).toContain("s2")
  })

  it("remembers the newest removals, up to its limit", () => {
    const removed = Array.from({ length: retention.removals + 1 }, (_, i) => i).reduce(
      (state, i) =>
        sessionRemoved(state, { sessionId: `gone-${i}`, revision: 1, draftId: "x" }),
      loaded(),
    )
    expect(removed.removed).toHaveLength(retention.removals)
    expect(removed.removed[0].sessionId).toBe("gone-1")
    expect(removed.removed.at(-1)?.sessionId).toBe(`gone-${retention.removals}`)
  })
})
