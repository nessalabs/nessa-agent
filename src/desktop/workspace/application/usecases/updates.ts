/**
 * What the source says: the index, its updates, and the conversations
 * read for the panes that show them. Every one is a replacement with a
 * revision, and the newer revision wins whichever arrived first — an update
 * that overtook a read, a read answered late, an index read while
 * updates were already flowing (`model/revision.ts`).
 */
import type { WorkspaceFailureReason } from "../../model/failure"
import {
  byRecency,
  defaultModel,
  type WorkspaceIndex,
  type SessionSummary,
} from "../../model/workspace-index"
import {
  paneCount,
  panesOf,
  paneShowing,
  removePane,
  singlePane,
} from "../../model/pane-layout"
import { forgotten, remembered, removedAt } from "../../model/retention"
import { fromSource, knownToSource, supersedes } from "../../model/revision"
import type { Transcript } from "../../model/transcript"
import type { WorkspaceUpdate } from "../ports"
import {
  channelOf,
  entry,
  forgetSession,
  listedSessions,
  modelForNextTurn,
  sessionOf,
  shownIds,
  withPanes,
  without,
  withSession,
  withTranscript,
  type WorkspaceState,
} from "../workspace-state"
import { createDraft } from "./panes"

/** Whether the session was removed at or after this revision of it: it stays out. */
function removedSince(state: WorkspaceState, session: SessionSummary): boolean {
  const at = removedAt(state.removed, session.id)
  return at !== undefined && at >= session.revision
}

/**
 * Whether a conversation from the source is one to hold: the source has
 * spoken of its session in a summary, and it is newer than the one held. One
 * rule for an update and for a read's answer, so a conversation never
 * outlives its session, and the newest heard always wins — a session's
 * summary is the one sign that the source has begun it: a conversation that
 * arrives first is read again once the summary does, while a pane shows the
 * session or when one first does (`effects.ts`).
 */
function holdsConversation(state: WorkspaceState, transcript: Transcript): boolean {
  const session = sessionOf(state, transcript.sessionId)
  return (
    session !== undefined &&
    knownToSource(session) &&
    fromSource(transcript) &&
    supersedes(transcript, heldConversation(state, transcript.sessionId))
  )
}

/**
 * The conversation revision held for a session: the content's, or — its
 * content let go to keep the window small — the newest revision it had.
 */
function heldConversation(state: WorkspaceState, sessionId: string) {
  const held = entry(state.transcripts, sessionId)
  if (held) return held
  const revision = entry(state.conversationRevisions, sessionId)
  return revision === undefined ? undefined : { revision }
}

/**
 * A summary from the source, by whichever path it came — the index
 * read or the stream — listed when it is newer than the one held and not
 * outranked by a removal. Listed again after its removal, the removal is
 * history. Listed at all, no new session's home stands under its id; a model
 * chosen in that home carries over as the one for the session's next turn.
 * A model the person chose stays chosen until they choose again: no summary,
 * whichever model it names, speaks for the person.
 */
function summaryHeard(state: WorkspaceState, session: SessionSummary): WorkspaceState {
  if (!fromSource(session) || removedSince(state, session)) return state
  if (!supersedes(session, sessionOf(state, session.id))) return state
  const home = entry(state.drafts, session.id)?.model
  const chosenModels = home
    ? { ...state.chosenModels, [session.id]: home }
    : state.chosenModels
  return withSession(
    {
      ...state,
      removed: forgotten(state.removed, session.id),
      drafts: without(state.drafts, session.id),
      chosenModels,
    },
    session,
  )
}

/**
 * Takes a session out of the lists, with everything the window held of it —
 * listed again, it is read afresh. A pane showing it closes; the last pane
 * turns into a new session's home in the same channel, under `draftId`, so
 * no pane is left showing a session that is not there.
 */
function removeSession(
  state: WorkspaceState,
  sessionId: string,
  draftId: string,
): WorkspaceState {
  const session = sessionOf(state, sessionId)
  if (!session) return state
  const removed = forgetSession(state, sessionId)
  const panes = state.panes
  const showing = panes && paneShowing(panes, sessionId)
  if (!panes || !showing) return removed
  if (paneCount(panes) > 1) return withPanes(removed, removePane(panes, showing.key))
  const restarted = createDraft(removed, {
    draftId,
    channelId: session.channelId,
    model: modelForNextTurn(state, sessionId) ?? session.model,
    target: showing.key,
  })
  // Nobody asked for a new session here: what the person was looking at stays.
  return { ...restarted, view: state.view }
}

/**
 * The index has arrived — the source's whole list, and the resync for
 * whatever the stream lost. The first time, the workspace opens on the
 * first channel, showing its newest session — or, with no sessions anywhere,
 * a new session's home under `draftId`. A session already held from an
 * update newer than this read is kept, and one removed since stays out.
 *
 * A session the source spoke of that the index does not list, or lists
 * in a channel it does not list, is gone: it is taken out as any removal is,
 * at the revision held, unless the stream brought it since this read
 * (`read`) was asked — the read may be the older of the two. What the
 * stream brought before this read was asked, even while another read was
 * on its way, this read is the newer word on. A session the source has not
 * spoken of yet (revision 0) is the window's own, and stays.
 *
 * Answers may come in any order. One whose read was outrun — a read asked
 * after it answered first — is older than what the window holds, and is let
 * go unread: applied, it would take out a session the newer answer listed.
 */
export function indexLoaded(
  state: WorkspaceState,
  { index, draftId, read }: { index: WorkspaceIndex; draftId: string; read: string },
): WorkspaceState {
  const asked = state.reading.find((pending) => pending.read === read)
  if (asked?.outrun) return { ...state, reading: readAnswered(state.reading, read) }
  const channels = new Set(index.channels.map((channel) => channel.id))
  const inChannel = index.sessions.filter((session) => channels.has(session.channelId))
  const listed = new Set(inChannel.map((session) => session.id))
  const kept = new Set(asked?.heard ?? [])
  const heard = inChannel.reduce(summaryHeard, {
    ...state,
    sections: index.sections,
    channels: index.channels,
  })
  const reconciled = listedSessions(heard)
    .filter(
      (session) =>
        knownToSource(session) && !listed.has(session.id) && !kept.has(session.id),
    )
    .reduce(
      (current, session) =>
        removeSession(
          {
            ...current,
            removed: remembered(current.removed, {
              sessionId: session.id,
              revision: session.revision,
            }),
          },
          session.id,
          draftId,
        ),
      heard,
    )
  const loaded: WorkspaceState = {
    ...reconciled,
    status: "ready",
    failure: null,
    reading: outrunBy(readAnswered(state.reading, read), state.reading, read),
  }
  if (state.panes) {
    // A channel the index no longer lists is not looked at.
    const view = loaded.view
    return channelOf(loaded, view.channelId)
      ? loaded
      : { ...loaded, view: { channelId: index.channels[0]?.id ?? "" } }
  }
  const first = index.channels.find(
    (channel) => channel.sectionId === index.sections[0]?.id,
  )
  const channelId = first?.id ?? index.channels[0]?.id ?? ""
  const sessions = listedSessions(loaded)
  const opening = sessions
    .filter((session) => session.channelId === channelId)
    .sort(byRecency)[0]
  // Disclosed at first: the channel being opened, and every channel with something waiting.
  const expandedChannels = [
    ...new Set([
      channelId,
      ...sessions
        .filter((session) => session.status === "needs-you")
        .map((session) => session.channelId),
    ]),
  ].filter((id) => id !== "")
  const opened: WorkspaceState = {
    ...loaded,
    view: { channelId },
    tree: { ...state.tree, expandedChannels },
  }
  if (opening) return withPanes(opened, singlePane(opening.id))
  const model = defaultModel()
  if (!channelId || !model) return opened
  return withPanes(
    { ...opened, drafts: { [draftId]: { id: draftId, channelId, model } } },
    singlePane(draftId),
  )
}

/** Every read still on its way that was asked before `read`: outrun by its answer. */
function outrunBy(
  pending: WorkspaceState["reading"],
  asked: WorkspaceState["reading"],
  read: string,
): WorkspaceState["reading"] {
  const at = asked.findIndex((entry) => entry.read === read)
  if (at <= 0) return pending
  const earlier = new Set(asked.slice(0, at).map((entry) => entry.read))
  return pending.map((entry) =>
    earlier.has(entry.read) && !entry.outrun ? { ...entry, outrun: true } : entry,
  )
}

/** The read `read` answered, or failed: what the stream brought since it was asked goes with it. */
function readAnswered(
  reading: WorkspaceState["reading"],
  read: string,
): WorkspaceState["reading"] {
  return reading.some((asked) => asked.read === read)
    ? reading.filter((asked) => asked.read !== read)
    : reading
}

/**
 * The index is being read, as `read`: after a failure, nothing to say until
 * it answers; and until it does, what the stream brings is noted against
 * it, so the read does not take out a session it may predate.
 */
export function indexRequested(
  state: WorkspaceState,
  { read }: { read: string },
): WorkspaceState {
  const reading = [...state.reading, { read, heard: [], outrun: false }]
  return state.status === "failed"
    ? { ...state, status: "loading", failure: null, reading }
    : { ...state, reading }
}

/** The index could not be read. A workspace already open stays open. */
export function indexFailed(
  state: WorkspaceState,
  { reason, read }: { reason: WorkspaceFailureReason; read: string },
): WorkspaceState {
  const reading = readAnswered(state.reading, read)
  if (state.status === "ready")
    return reading === state.reading ? state : { ...state, reading }
  return { ...state, status: "failed", failure: reason, reading }
}

/**
 * An update from the source, applied when it is newer than what is held. A
 * summary brought while the index is being read is noted against every read
 * on its way, so none of them can take the session out.
 */
export function updateReceived(
  state: WorkspaceState,
  { update }: { update: Extract<WorkspaceUpdate, { kind: "session" | "transcript" }> },
): WorkspaceState {
  switch (update.kind) {
    case "session": {
      const heard = summaryHeard(state, update.session)
      const id = update.session.id
      if (heard === state || heard.reading.every((asked) => asked.heard.includes(id)))
        return heard
      return {
        ...heard,
        reading: heard.reading.map((asked) =>
          asked.heard.includes(id) ? asked : { ...asked, heard: [...asked.heard, id] },
        ),
      }
    }
    case "transcript":
      return holdsConversation(state, update.transcript)
        ? withTranscript(state, update.transcript)
        : state
  }
}

/**
 * The source removed a session, at this revision of its summary (the port
 * counts a removal with the summary, not the conversation). The pane showing
 * it closes; the last pane starts over as a new session's home under
 * `draftId`. A removal older than the summary held says nothing about it
 * now; one of a session not held is remembered, in case an older read lists
 * it.
 */
export function sessionRemoved(
  state: WorkspaceState,
  {
    sessionId,
    revision,
    draftId,
  }: { sessionId: string; revision: number; draftId: string },
): WorkspaceState {
  if (!fromSource({ revision })) return state
  const held = sessionOf(state, sessionId)
  if (held && held.revision > revision) return state
  const marked = { ...state, removed: remembered(state.removed, { sessionId, revision }) }
  return removeSession(marked, sessionId, draftId)
}

/** A session's conversation, read because a pane shows it; kept only if nothing newer is. */
export function transcriptLoaded(
  state: WorkspaceState,
  { transcript }: { transcript: Transcript },
): WorkspaceState {
  // A read that outlived its session, removed meanwhile, is let go.
  if (!sessionOf(state, transcript.sessionId)) return state
  const read = {
    ...state,
    transcriptFailures: without(state.transcriptFailures, transcript.sessionId),
  }
  return holdsConversation(read, transcript) ? withTranscript(read, transcript) : read
}

/**
 * A conversation on screen could not be read; the pane, or the overview's
 * row, says why, and it is not read again until someone asks
 * (`transcriptRetried`) or it is shown afresh.
 */
export function transcriptFailed(
  state: WorkspaceState,
  { sessionId, reason }: { sessionId: string; reason: WorkspaceFailureReason },
): WorkspaceState {
  // A read that failed after a conversation arrived another way, or once
  // nothing on screen shows the session, changes nothing: shown again, it is
  // read afresh.
  if (
    !sessionOf(state, sessionId) ||
    entry(state.transcripts, sessionId) ||
    !shownIds(state).has(sessionId)
  )
    return state
  return {
    ...state,
    transcriptFailures: { ...state.transcriptFailures, [sessionId]: reason },
  }
}

/** Asks for a conversation that could not be read to be read again. */
export function transcriptRetried(
  state: WorkspaceState,
  { sessionId }: { sessionId: string },
): WorkspaceState {
  const transcriptFailures = without(state.transcriptFailures, sessionId)
  return transcriptFailures === state.transcriptFailures
    ? state
    : { ...state, transcriptFailures }
}

/** The sessions shown in a pane that are listed, in pane order. */
function sessionsInPanes(state: WorkspaceState): SessionSummary[] {
  if (!state.panes) return []
  return panesOf(state.panes).flatMap((pane) => {
    const session = sessionOf(state, pane.sessionId)
    return session ? [session] : []
  })
}

/**
 * The sessions on screen — in a pane, or in the open overview, bounded
 * (`shownIds`) — whose conversations are the source's to give: listed, and
 * spoken of by the source — a session whose first message is still on its
 * way has nothing to read yet. What the window reads, and reads again on a
 * resync (`effects.ts`).
 */
export function shownSessionIds(state: WorkspaceState): string[] {
  return [...shownIds(state)].filter((sessionId) => {
    const session = sessionOf(state, sessionId)
    return session !== undefined && knownToSource(session)
  })
}

/**
 * The sessions shown in a pane and still marked unread: showing one is
 * reading it, whoever opened it and however (`effects.ts` tells the source).
 */
export function unreadShown(state: WorkspaceState): string[] {
  // A pane only: a peek in the overview is not reading the session.
  return sessionsInPanes(state)
    .filter((session) => session.unread)
    .map((session) => session.id)
}
