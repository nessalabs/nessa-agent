import { createAction, createAsyncThunk, createSlice } from "@reduxjs/toolkit"

import {
  ControlFailedError,
  ConversationReadFailedError,
  type ConversationEffects,
} from "../../application/ports"
import {
  controlFailureMessage,
  deletedAnyway,
  deletedMessage,
} from "../../application/usecases"
import type { ConversationSummary } from "../../application/view"
import type { ReadFailure } from "../../model"

/**
 * The gateway's list of the caller's conversations, as last read, and what
 * the list says about the archives and deletes asked for from it.
 *
 * Its own slice rather than a field on the tabs: the tabs are what this window
 * holds open, and the list is what the gateway holds, closed conversations
 * included. A failed read keeps the rows it had — a stale list is still true
 * about everything it names — and the list says, beside them, that it could
 * not be refreshed.
 *
 * What each answer does here is the state table in
 * docs/adr/todo/182-conversation-deletion.md ("In the panel"). A lost answer
 * is said to be unconfirmed and nothing more: the list read that follows every
 * answer shows where the conversation stands.
 */
export type ConversationHistory = {
  /** Null until the first list arrives. */
  rows: ConversationSummary[] | null
  /**
   * The conversations the gateway lists as archived, by identity: what keeps a
   * tab open on one of them from standing in the list as if it were new.
   */
  archivedIds: string[]
  failure: ReadFailure | null
  /** The read in flight, so an older answer arriving late cannot replace a newer one. */
  requestId: string | null
  /** What the latest archive or delete's answer has to say, in the panel's words. */
  commandError: string | null
  /**
   * The conversation this list last archived, while undoing that is still on
   * offer: the panel shows no archived conversations, so this is its one way
   * back. Any later action, or leaving the Messages tab, withdraws it — except
   * the undo itself, which keeps it on offer until the unarchive is done, so a
   * refused or unanswered undo can be tried again.
   */
  undoable: { serverConversationId: string; title: string } | null
  /**
   * The archive or delete asked for last, by request: only its answer says
   * anything or offers an undo, so an earlier action answering late cannot
   * replace or outlive what came after it.
   */
  latestAction: string | null
  /**
   * Conversations an archive or delete was asked for and has not answered.
   * Kept here rather than in the list, so leaving the Messages tab mid-action
   * does not put the row back before the gateway has said anything.
   */
  leavingIds: string[]
  /**
   * Conversations this window knows were deleted: its own delete was answered,
   * or the gateway refused another action on them as deleted. A delete this
   * window made lets go of their tabs; otherwise a tab may stay open — a draft
   * in one is still there to copy — but the list never names them again,
   * which it would otherwise do as a tab it has not caught up with.
   */
  deletedIds: string[]
}

const initialState: ConversationHistory = {
  rows: null,
  archivedIds: [],
  failure: null,
  requestId: null,
  commandError: null,
  undoable: null,
  latestAction: null,
  leavingIds: [],
  deletedIds: [],
}

type Extra = { extra: { conversation: ConversationEffects } }

/**
 * The conversations the list shows — those not archived — and which ones are
 * archived. Both, or neither: half a list would put an archived tab back in it.
 */
export const listConversations = createAsyncThunk<
  { rows: ConversationSummary[]; archivedIds: string[] },
  void,
  Extra & { rejectValue: ReadFailure }
>("conversationHistory/list", async (_, { extra, rejectWithValue }) => {
  try {
    const [rows, archived] = await Promise.all([
      extra.conversation.list(false),
      extra.conversation.list(true),
    ])
    return { rows, archivedIds: archived.map((row) => row.conversationId) }
  } catch (error) {
    // The port promises a typed reason for every rejected list; anything else
    // is a stale list whose cause this build cannot name.
    return rejectWithValue(
      error instanceof ConversationReadFailedError ? error.reason : "unavailable",
    )
  }
})

/**
 * A conversation the gateway deleted, at this window's request. The tabs slice
 * lets go of every tab bound to it: its identity is refused from now on, so a
 * tab kept open would only ever show that.
 */
export const conversationDeleted = createAction<string>("conversationHistory/deleted")

/**
 * An archive or unarchive the gateway applied, applied to the list at once: an
 * archived row leaves it before the list is read again, and stays out if that
 * read fails, as a deleted one does.
 */
export const conversationArchived = createAction<{
  serverConversationId: string
  archived: boolean
}>("conversationHistory/archived")

/** Why an archive or delete was not done as asked: the sentence, and what it means for the list. */
export type CommandRejection = {
  message: string
  /** Refused for a reason no retry changes: the conversation is gone or not the caller's. */
  permanent?: boolean
  /** Refused because the conversation was deleted: known here from now on. */
  deleted?: boolean
}

/**
 * Archive or unarchive, then read the list again to show where it stands.
 * Resolves with whether the gateway changed anything.
 */
export const archiveConversation = createAsyncThunk<
  boolean,
  { serverConversationId: string; archived: boolean; title: string },
  Extra & { rejectValue: CommandRejection }
>(
  "conversationHistory/archive",
  async (
    { serverConversationId, archived, title },
    { dispatch, extra, rejectWithValue },
  ) => {
    let applied: boolean
    try {
      applied = await extra.conversation.archive(serverConversationId, archived)
    } catch (error) {
      void dispatch(listConversations())
      return rejectWithValue(rejection(archived ? "archive" : "unarchive", title, error))
    }
    void dispatch(listConversations())
    if (applied) dispatch(conversationArchived({ serverConversationId, archived }))
    return applied
  },
)

/**
 * Delete permanently. A delete whose erasure did not finish still happened, so
 * its tabs are let go of all the same, and what remains is said beside the list.
 * Any other failure leaves the tabs alone: a tab onto a conversation that was
 * deleted after all says so itself the next time it reads.
 */
export const deleteConversation = createAsyncThunk<
  void,
  { serverConversationId: string; title: string },
  Extra & { rejectValue: CommandRejection }
>(
  "conversationHistory/delete",
  async ({ serverConversationId, title }, { dispatch, extra, rejectWithValue }) => {
    try {
      await extra.conversation.delete(serverConversationId)
    } catch (error) {
      void dispatch(listConversations())
      // A delete that happened: its tabs go, and what the list says is news
      // about it, not a failure to delete.
      if (error instanceof ControlFailedError && deletedAnyway(error.reason)) {
        dispatch(conversationDeleted(serverConversationId))
        return rejectWithValue({ message: deletedMessage(error.reason, nameOf(title)) })
      }
      return rejectWithValue(rejection("delete", title, error))
    }
    dispatch(conversationDeleted(serverConversationId))
    void dispatch(listConversations())
  },
)

/**
 * What the list says of an action the gateway did not answer as done. "Could
 * not" only when the gateway refused; anything else — an answer lost, or one
 * whose outcome the gateway itself could not vouch for — is said to be
 * unconfirmed, and the list read that follows shows where it stands. The row
 * may be gone by the time this is read, so the sentence carries the name.
 */
function rejection(
  action: "archive" | "unarchive" | "delete",
  title: string,
  error: unknown,
): CommandRejection {
  const name = nameOf(title)
  if (!(error instanceof ControlFailedError && error.outcome === "refused"))
    return {
      message: `The gateway did not confirm whether ${name} was ${action === "delete" ? "deleted" : `${action}d`}. The list shows where it stands.`,
    }
  return {
    message: `Could not ${action} ${name}: ${sentenceOf(error)}`,
    permanent:
      error.reason === "conversation-deleted" ||
      error.reason === "conversation-not-found",
    deleted: error.reason === "conversation-deleted",
  }
}

/** The conversation a sentence is about, by the title the list showed. */
function nameOf(title: string) {
  return `“${title}”`
}

/** A refused or unfinished archive or delete, in the panel's words. */
function sentenceOf(error: ControlFailedError) {
  return controlFailureMessage(error.reason, error.outcome) ?? error.message
}

/** Start an action: its row leaves the list, and it is now the one that speaks. */
function asking(
  state: ConversationHistory,
  action: { meta: { requestId: string; arg: { serverConversationId: string } } },
) {
  state.commandError = null
  state.latestAction = action.meta.requestId
  state.leavingIds.push(action.meta.arg.serverConversationId)
}

/** One answered action lets go of its row; another out on the same row keeps it. */
function settle(state: ConversationHistory, serverConversationId: string) {
  const at = state.leavingIds.indexOf(serverConversationId)
  if (at !== -1) state.leavingIds.splice(at, 1)
}

/** Add an id once. */
function known(ids: string[], id: string) {
  if (!ids.includes(id)) ids.push(id)
}

/** Withdraw the undo, if it is the undo of this conversation. */
function withdrawUndoOf(state: ConversationHistory, serverConversationId: string) {
  if (state.undoable?.serverConversationId === serverConversationId) state.undoable = null
}

/**
 * An archive or delete that was not done as asked, or not confirmed. Its row
 * comes back to the list; only the latest action's answer says why.
 */
function rejected(
  state: ConversationHistory,
  action: {
    payload?: CommandRejection
    meta: { requestId: string; arg: { serverConversationId: string } }
  },
) {
  const id = action.meta.arg.serverConversationId
  settle(state, id)
  // The gateway said it was deleted, elsewhere: the list never names it again,
  // though a tab onto it stays until it reads that for itself.
  if (action.payload?.deleted) known(state.deletedIds, id)
  if (state.latestAction !== action.meta.requestId) return
  state.commandError = action.payload?.message ?? null
  // An undo refused for good is no longer on offer: pressing it again would
  // only be refused the same way.
  if (action.payload?.permanent) withdrawUndoOf(state, id)
}

const historySlice = createSlice({
  name: "conversationHistory",
  initialState,
  reducers: {
    /**
     * The Messages tab was left: what the list said about the latest action
     * has had its chance to be read, and the undo goes with it. Dispatched on
     * that transition rather than on the list unmounting, which development
     * mode also does on mount.
     */
    commandErrorCleared(state) {
      state.commandError = null
      state.undoable = null
    },
  },
  extraReducers: (builder) => {
    builder
      .addCase(listConversations.pending, (state, action) => {
        state.requestId = action.meta.requestId
      })
      .addCase(listConversations.fulfilled, (state, action) => {
        if (state.requestId !== action.meta.requestId) return
        state.rows = action.payload.rows
        state.archivedIds = action.payload.archivedIds
        state.failure = null
        state.requestId = null
      })
      .addCase(listConversations.rejected, (state, action) => {
        if (state.requestId !== action.meta.requestId) return
        state.failure = action.payload ?? "unavailable"
        state.requestId = null
      })
      .addCase(archiveConversation.pending, (state, action) => {
        // Undoing the archive on offer keeps the offer until the undo is done.
        const { serverConversationId, archived } = action.meta.arg
        if (archived || state.undoable?.serverConversationId !== serverConversationId)
          state.undoable = null
        asking(state, action)
      })
      .addCase(archiveConversation.fulfilled, (state, action) => {
        const { serverConversationId, archived, title } = action.meta.arg
        settle(state, serverConversationId)
        if (state.latestAction !== action.meta.requestId) return
        const applied = action.payload
        state.undoable = applied && archived ? { serverConversationId, title } : null
        if (!applied)
          state.commandError = `The gateway changed nothing for ${nameOf(title)}.`
      })
      .addCase(archiveConversation.rejected, rejected)
      .addCase(deleteConversation.pending, (state, action) => {
        state.undoable = null
        asking(state, action)
      })
      .addCase(deleteConversation.fulfilled, (state, action) => {
        settle(state, action.meta.arg.serverConversationId)
      })
      .addCase(deleteConversation.rejected, rejected)
      .addCase(conversationArchived, (state, action) => {
        const { serverConversationId, archived } = action.payload
        state.archivedIds = archived
          ? [...new Set([...state.archivedIds, serverConversationId])]
          : state.archivedIds.filter((id) => id !== serverConversationId)
        if (archived)
          state.rows =
            state.rows?.filter((row) => row.conversationId !== serverConversationId) ??
            null
      })
      // Out of the list at once, whether or not the next read of it succeeds;
      // an undo of it could only be refused now.
      .addCase(conversationDeleted, (state, action) => {
        known(state.deletedIds, action.payload)
        withdrawUndoOf(state, action.payload)
        state.rows =
          state.rows?.filter((row) => row.conversationId !== action.payload) ?? null
      })
  },
})

export const conversationHistoryReducer = historySlice.reducer
export const { commandErrorCleared } = historySlice.actions
