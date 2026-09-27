import { describe, expect, it } from "vitest"
import { panesOf, paneShowing } from "../../model/pane-layout"
import { emptyTranscript, type Transcript } from "../../model/transcript"
import { astra, summary, testOrganisation } from "../../testing"
import {
  initialWorkspace,
  modelForNextTurn,
  type WorkspaceState,
} from "../workspace-state"
import { closePane, createDraft, openBeside } from "./panes"
import { chooseModel, messageSent } from "./sessions"
import {
  organisationFailed,
  organisationLoaded,
  organisationRequested,
  shownSessionIds,
  transcriptFailed,
  transcriptLoaded,
  transcriptRetried,
  unreadShown,
  updateReceived,
} from "./updates"

const loaded = () =>
  organisationLoaded(initialWorkspace, {
    organisation: testOrganisation(),
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

describe("the organisation arriving", () => {
  it("opens on the first channel's newest session and discloses what waits", () => {
    const state = loaded()
    expect(state.status).toBe("ready")
    expect(state.view).toEqual({ kind: "channel", channelId: "desktop" })
    expect(shown(state)).toEqual(["a"])
    expect(state.tree.expandedChannels).toEqual(["desktop"])
  })

  it("opens on a new session's home when there are no sessions", () => {
    const state = organisationLoaded(initialWorkspace, {
      organisation: { ...testOrganisation(), sessions: [] },
      draftId: "first",
    })
    expect(shown(state)).toEqual(["first"])
    expect(state.drafts.first.channelId).toBe("desktop")
  })

  it("shows nothing at all with no channels to start in", () => {
    const state = organisationLoaded(initialWorkspace, {
      organisation: { sections: [], channels: [], sessions: [] },
      draftId: "first",
    })
    expect(state.panes).toBeNull()
    expect(state.status).toBe("ready")
  })

  it("keeps the panes it has when the organisation is read again", () => {
    const two = openBeside(loaded(), { sessionId: "c" })
    const again = organisationLoaded(two, {
      organisation: testOrganisation(),
      draftId: "x",
    })
    expect(again.panes).toBe(two.panes)
  })

  it("says why it could not be read", () => {
    const state = organisationFailed(initialWorkspace, { reason: "unavailable" })
    expect(state.status).toBe("failed")
    expect(state.failure).toMatch(/couldn’t reach/)
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
      expect(
        updateReceived(state, {
          update: { kind: "session-removed", sessionId: "a", revision },
        }),
      ).toBe(state)
    }
  })

  it("closes the pane of a removed session, and leaves the last pane showing what is left", () => {
    const two = openBeside(loaded(), { sessionId: "c" })
    const removed = updateReceived(two, {
      update: { kind: "session-removed", sessionId: "c", revision: 2 },
    })
    expect(shown(removed)).toEqual(["a"])
    expect(Object.keys(removed.sessions)).not.toContain("c")
    const last = updateReceived(loaded(), {
      update: { kind: "session-removed", sessionId: "a", revision: 2 },
    })
    expect(shown(last)).toEqual(["a"])
    // A removal of a session not yet listed is remembered, in case an older read lists it.
    const unknown = loaded()
    const remembered = updateReceived(unknown, {
      update: { kind: "session-removed", sessionId: "missing", revision: 1 },
    })
    expect(remembered.sessions).toBe(unknown.sessions)
    expect(remembered.removed).toEqual({ missing: 1 })
  })

  it("remembers why a conversation could not be read, until it is", () => {
    const failed = transcriptFailed(loaded(), {
      sessionId: "a",
      reason: "unknown-session",
    })
    expect(failed.transcriptFailures.a).toMatch(/no longer there/)
    const read = transcriptLoaded(failed, { transcript: emptyTranscript("a") })
    expect(read.transcriptFailures).toEqual({})
  })

  it("lists only the shown sessions the source has, drafts never among them", () => {
    const drafted = createDraft(loaded(), { draftId: "new", beside: "right" })
    expect(shownSessionIds(drafted)).toEqual(["a"])
    expect(shownSessionIds(initialWorkspace)).toEqual([])
  })
})

describe("what goes with a session", () => {
  it("names the shown sessions still marked unread, the one the workspace opens on included", () => {
    const organisation = testOrganisation()
    const state = organisationLoaded(initialWorkspace, {
      organisation: {
        ...organisation,
        sessions: organisation.sessions.map((session) =>
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
    const two = openBeside(state, { sessionId: "c" })
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
      sessionId: "new",
      message: { id: "m1", role: "user", at: 1, parts: [{ kind: "text", text: "hi" }] },
    })
    const listed = updateReceived(sent, {
      update: { kind: "session", session: summary("new", "desktop", 900, "running") },
    })
    expect(listed.sessions.new.model).not.toEqual(astra)
    expect(modelForNextTurn(listed, "new")).toEqual(astra)
  })

  it("lists a summary the same way whether the organisation read or the stream brings it", () => {
    const drafted = createDraft(loaded(), { draftId: "new" })
    const reread = organisationLoaded(drafted, {
      organisation: {
        ...testOrganisation(),
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
    const two = openBeside(loaded(), { sessionId: "c" })
    const closed = updateReceived(two, {
      update: { kind: "session-removed", sessionId: "c", revision: 3 },
      draftId: "x",
    })
    expect(panesOf(closed.panes!).map((pane) => pane.sessionId)).toEqual(["a"])
    const last = updateReceived(loaded(), {
      update: { kind: "session-removed", sessionId: "a", revision: 3 },
      draftId: "fresh",
    })
    expect(panesOf(last.panes!).map((pane) => pane.sessionId)).toEqual(["fresh"])
    expect(last.drafts.fresh.channelId).toBe("desktop")
    const chosen = updateReceived(
      { ...loaded(), chosenModels: { a: astra } },
      {
        update: { kind: "session-removed", sessionId: "a", revision: 3 },
        draftId: "fresh",
      },
    )
    // The model the person chose for its next turn carries over to the new home.
    expect(chosen.drafts.fresh.model).toEqual(astra)
    const waiting = updateReceived(
      { ...loaded(), view: { kind: "status", status: "running" } },
      {
        update: { kind: "session-removed", sessionId: "a", revision: 3 },
        draftId: "fresh",
      },
    )
    expect(waiting.view).toEqual({ kind: "status", status: "running" })
  })

  it("drops an answer and a chosen model with a removed session", () => {
    const state = {
      ...loaded(),
      answers: { c: { approvalId: "ap", token: "t1" } },
      chosenModels: { c: testOrganisation().sessions[3].model },
    }
    const removed = updateReceived(state, {
      update: { kind: "session-removed", sessionId: "c", revision: 3 },
    })
    expect(removed.answers).toEqual({})
    expect(removed.chosenModels).toEqual({})
  })

  it("drops a removed session's conversation, failure and outbox, and a read that outlived it", () => {
    const failed = transcriptFailed(loaded(), { sessionId: "c", reason: "unavailable" })
    const state = { ...failed, transcripts: { c: transcript("c", 2) } }
    const removed = updateReceived(state, {
      update: { kind: "session-removed", sessionId: "c", revision: 3 },
    })
    expect(removed.transcripts.c).toBeUndefined()
    expect(removed.transcriptFailures.c).toBeUndefined()
    expect(transcriptLoaded(removed, { transcript: transcript("c", 4) })).toBe(removed)
  })

  it("keeps an open workspace open when a later read of the organisation fails", () => {
    const state = loaded()
    expect(organisationFailed(state, { reason: "unavailable" })).toBe(state)
  })

  it("says nothing while the organisation is read again after a failure", () => {
    const failed = organisationFailed(initialWorkspace, { reason: "unavailable" })
    expect(organisationRequested(failed)).toMatchObject({
      status: "loading",
      failure: null,
    })
    const ready = loaded()
    expect(organisationRequested(ready)).toBe(ready)
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

  it("keeps summaries the stream brought before an older organisation read", () => {
    const early = updateReceived(initialWorkspace, {
      update: {
        kind: "session",
        session: summary("a", "desktop", 999, "idle", { revision: 7 }),
      },
    })
    const state = organisationLoaded(early, {
      organisation: testOrganisation(),
      draftId: "x",
    })
    expect(state.sessions.a).toMatchObject({ status: "idle", revision: 7 })
    expect(state.sessions.b.revision).toBe(1)
  })

  it("does not bring back a session removed before an older organisation read", () => {
    const removed = updateReceived(initialWorkspace, {
      update: { kind: "session-removed", sessionId: "b", revision: 2 },
    })
    const state = organisationLoaded(removed, {
      organisation: testOrganisation(),
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
    const stale = updateReceived(newer, {
      update: { kind: "session-removed", sessionId: "b", revision: 4 },
    })
    expect(stale).toBe(newer)
    const removed = updateReceived(loaded(), {
      update: { kind: "session-removed", sessionId: "c", revision: 6 },
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
    expect(relisted.removed).toEqual({})
  })

  it("applies a removal the source repeats once", () => {
    const removed = updateReceived(loaded(), {
      update: { kind: "session-removed", sessionId: "c", revision: 2 },
    })
    expect(
      updateReceived(removed, {
        update: { kind: "session-removed", sessionId: "c", revision: 2 },
      }).sessions,
    ).toBe(removed.sessions)
  })

  it("keeps the view it has when the organisation is read again", () => {
    const viewing = { ...loaded(), view: { kind: "status", status: "running" } as const }
    const again = organisationLoaded(viewing, {
      organisation: testOrganisation(),
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
