/**
 * What the source says: the organisation, its updates, and the conversations
 * read for the panes that show them. Every one is a replacement with a
 * revision, and the newer revision wins whichever arrived first — an update
 * that overtook a read, a read answered late, an organisation read while
 * updates were already flowing (`model/revision.ts`).
 */
import {
  byRecency,
  defaultModel,
  type Organisation,
  type SessionSummary,
} from "../../model/organisation"
import {
  paneCount,
  panesOf,
  paneShowing,
  removePane,
  singlePane,
} from "../../model/pane-layout"
import { fromSource, knownToSource, supersedes } from "../../model/revision"
import type { Transcript } from "../../model/transcript"
import { failureText, type WorkspaceFailureReason, type WorkspaceUpdate } from "../ports"
import {
  entry,
  forgetSession,
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
  const removedAt = entry(state.removed, session.id)
  return removedAt !== undefined && removedAt >= session.revision
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
 * A summary from the source, by whichever path it came — the organisation
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
      removed: without(state.removed, session.id),
      drafts: without(state.drafts, session.id),
      chosenModels,
    },
    session,
  )
}

/**
 * The organisation has arrived. The first time, the workspace opens on the
 * first channel, showing its newest session — or, with no sessions anywhere,
 * a new session's home under `draftId`. A session already held from an
 * update newer than this read is kept, and one removed since stays out.
 */
export function organisationLoaded(
  state: WorkspaceState,
  { organisation, draftId }: { organisation: Organisation; draftId: string },
): WorkspaceState {
  const heard = organisation.sessions.reduce(summaryHeard, state)
  const first = organisation.channels.find(
    (channel) => channel.sectionId === organisation.sections[0]?.id,
  )
  const channelId = first?.id ?? organisation.channels[0]?.id ?? ""
  const listed = Object.values(heard.sessions)
  const opening = listed
    .filter((session) => session.channelId === channelId)
    .sort(byRecency)[0]
  // Disclosed at first: the channel being opened, and every channel with something waiting.
  const expandedChannels = [
    ...new Set([
      channelId,
      ...listed
        .filter((session) => session.status === "needs-you")
        .map((session) => session.channelId),
    ]),
  ].filter((id) => id !== "")
  const loaded: WorkspaceState = {
    ...heard,
    status: "ready",
    failure: null,
    sections: organisation.sections,
    channels: organisation.channels,
  }
  if (state.panes) return loaded
  const opened: WorkspaceState = {
    ...loaded,
    view: { kind: "channel", channelId },
    tree: { ...state.tree, expandedChannels },
  }
  if (opening) return withPanes(opened, singlePane(opening.id))
  if (!channelId) return opened
  return withPanes(
    {
      ...opened,
      drafts: { [draftId]: { id: draftId, channelId, model: defaultModel() } },
    },
    singlePane(draftId),
  )
}

/** The organisation is being read, again after a failure: nothing to say until it answers. */
export function organisationRequested(state: WorkspaceState): WorkspaceState {
  return state.status === "failed"
    ? { ...state, status: "loading", failure: null }
    : state
}

/** The organisation could not be read. A workspace already open stays open. */
export function organisationFailed(
  state: WorkspaceState,
  { reason }: { reason: WorkspaceFailureReason },
): WorkspaceState {
  if (state.status === "ready") return state
  return { ...state, status: "failed", failure: failureText(reason) }
}

/**
 * Takes a session out of the lists, with everything the window held of it —
 * listed again, it is read afresh. A pane showing it closes; the last pane
 * turns into a new session's home in the same channel when `draftId` is
 * given, and keeps showing what is left of it otherwise.
 */
export function removeSession(
  state: WorkspaceState,
  sessionId: string,
  draftId?: string,
): WorkspaceState {
  const session = sessionOf(state, sessionId)
  if (!session) return state
  const removed = forgetSession(state, sessionId)
  const panes = state.panes
  const showing = panes && paneShowing(panes, sessionId)
  if (!panes || !showing) return removed
  if (paneCount(panes) > 1) return withPanes(removed, removePane(panes, showing.key))
  if (!draftId) return removed
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
 * An update from the source, applied when it is newer than what is held. A
 * removal closes the pane showing the session; the last pane starts over as a
 * new session's home under `draftId`.
 */
export function updateReceived(
  state: WorkspaceState,
  { update, draftId }: { update: WorkspaceUpdate; draftId?: string },
): WorkspaceState {
  switch (update.kind) {
    case "session":
      return summaryHeard(state, update.session)
    case "session-removed": {
      const { sessionId, revision } = update
      if (!fromSource(update)) return state
      const removedAt = Math.max(revision, entry(state.removed, sessionId) ?? revision)
      const held = sessionOf(state, sessionId)
      // A removal older than the summary held says nothing about it now.
      if (held && held.revision > revision) return state
      const marked = { ...state, removed: { ...state.removed, [sessionId]: removedAt } }
      return removeSession(marked, sessionId, draftId)
    }
    case "transcript":
      return holdsConversation(state, update.transcript)
        ? withTranscript(state, update.transcript)
        : state
  }
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
    transcriptFailures: { ...state.transcriptFailures, [sessionId]: failureText(reason) },
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
