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
    supersedes(transcript, entry(state.transcripts, transcript.sessionId))
  )
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
 * at the revision held, unless the stream brought it while the read was on
 * its way — the read may be the older of the two. A session the source has
 * not spoken of yet (revision 0) is the window's own, and stays.
 */
export function indexLoaded(
  state: WorkspaceState,
  { index, draftId }: { index: WorkspaceIndex; draftId: string },
): WorkspaceState {
  const channels = new Set(index.channels.map((channel) => channel.id))
  const inChannel = index.sessions.filter((session) => channels.has(session.channelId))
  const listed = new Set(inChannel.map((session) => session.id))
  const kept = new Set(state.reading?.heard ?? [])
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
    reading: readAnswered(state.reading),
  }
  if (state.panes) {
    // A channel the index no longer lists is not looked at.
    const view = loaded.view
    return view.kind === "channel" && !channelOf(loaded, view.channelId)
      ? {
          ...loaded,
          view: { kind: "channel", channelId: index.channels[0]?.id ?? "" },
        }
      : loaded
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
    view: { kind: "channel", channelId },
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

/** One read of the index answered: what the stream brought is let go with the last. */
function readAnswered(reading: WorkspaceState["reading"]): WorkspaceState["reading"] {
  if (!reading) return null
  return reading.reads > 1 ? { ...reading, reads: reading.reads - 1 } : null
}

/**
 * The index is being read: after a failure, nothing to say until it
 * answers; and until it does, what the stream brings is noted, so the read
 * does not take out a session it may predate.
 */
export function indexRequested(state: WorkspaceState): WorkspaceState {
  const reading = {
    reads: (state.reading?.reads ?? 0) + 1,
    heard: state.reading?.heard ?? [],
  }
  return state.status === "failed"
    ? { ...state, status: "loading", failure: null, reading }
    : { ...state, reading }
}

/** The index could not be read. A workspace already open stays open. */
export function indexFailed(
  state: WorkspaceState,
  { reason }: { reason: WorkspaceFailureReason },
): WorkspaceState {
  const reading = readAnswered(state.reading)
  if (state.status === "ready")
    return reading === state.reading ? state : { ...state, reading }
  return { ...state, status: "failed", failure: reason, reading }
}

/**
 * An update from the source, applied when it is newer than what is held. A
 * summary brought while the index is being read is noted, so that read
 * cannot take the session out.
 */
export function updateReceived(
  state: WorkspaceState,
  { update }: { update: Exclude<WorkspaceUpdate, { kind: "session-removed" }> },
): WorkspaceState {
  switch (update.kind) {
    case "session": {
      const heard = summaryHeard(state, update.session)
      const reading = heard.reading
      if (heard === state || !reading || reading.heard.includes(update.session.id))
        return heard
      return {
        ...heard,
        reading: { ...reading, heard: [...reading.heard, update.session.id] },
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
 * A conversation a pane shows could not be read; the pane says why, and it
 * is not read again until someone asks (`transcriptRetried`).
 */
export function transcriptFailed(
  state: WorkspaceState,
  { sessionId, reason }: { sessionId: string; reason: WorkspaceFailureReason },
): WorkspaceState {
  // A read that failed after a conversation arrived another way, or once no pane
  // shows the session, changes nothing: shown again, it is read afresh.
  if (
    !sessionOf(state, sessionId) ||
    entry(state.transcripts, sessionId) ||
    !state.panes ||
    !paneShowing(state.panes, sessionId)
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

/** The ids of the sessions shown in a pane that are listed, in pane order. */
function shownSessions(state: WorkspaceState): SessionSummary[] {
  if (!state.panes) return []
  return panesOf(state.panes).flatMap((pane) => {
    const session = sessionOf(state, pane.sessionId)
    return session ? [session] : []
  })
}

/**
 * The sessions shown in a pane whose conversations are the source's to give:
 * listed, and spoken of by the source — a session whose first message is
 * still on its way has nothing to read yet.
 */
export function shownSessionIds(state: WorkspaceState): string[] {
  return shownSessions(state)
    .filter(knownToSource)
    .map((session) => session.id)
}

/**
 * The sessions shown in a pane and still marked unread: showing one is
 * reading it, whoever opened it and however (`effects.ts` tells the source).
 */
export function unreadShown(state: WorkspaceState): string[] {
  return shownSessions(state)
    .filter((session) => session.unread)
    .map((session) => session.id)
}
