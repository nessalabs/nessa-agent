/**
 * The session commands: what the source is asked to do to a session — send,
 * answer, stop, pin, archive, rename, delete — and whether this host can
 * open a provider's login. Re-exported by `commands.ts`, the one list.
 */
import { applyStagedModel, modelChanging } from "./model-commands"
import {
  atWork,
  messageText,
  type TurnMessage,
  type QuestionChoice,
} from "../../model/transcript"
import { differentFlight } from "../../application/question-flights"
import {
  failureReason,
  WorkspaceSourceError,
  type ApprovalScope,
  type Initiator,
  type WorkspaceDependencies,
} from "../../application/ports"
import type { WorkspaceFailureReason } from "../../model/failure"
import type { ModelRef } from "../../model/workspace-index"
import {
  draftImagesRefusal,
  pastedImage,
  type DraftImage,
  type DraftImageLimits,
  type DraftImageRefusal,
} from "../../model/draft-images"
import {
  IMAGE_ATTACHMENT_TYPES,
  MAX_IMAGE_ATTACHMENT_BYTES,
  MAX_MESSAGE_IMAGE_BYTES,
  MAX_MESSAGE_IMAGES,
} from "@nessa/client"
import { knownToSource } from "../../model/revision"
import {
  answerOf,
  answering,
  channelOf,
  draftOf,
  entry,
  modelForNextTurn,
  onScreen,
  sessionOf,
  type WorkspaceState,
} from "../../application/workspace-state"
import { accessToStart } from "../../application/usecases/access"
import type { WorkspaceCommand } from "./commands"
import { workspaceActions } from "./slice"

const {
  accessCaptured,
  approvalAnswering,
  approvalFailed,
  composerImagesAdded,
  messageDelivered,
  messageResent,
  messageSent,
  sendFailed,
} = workspaceActions

/** Local drafts and unconfirmed sessions still need a channel in the current index. */
function hasCurrentHome(state: WorkspaceState, sessionId: string): boolean {
  const draft = draftOf(state, sessionId)
  const session = sessionOf(state, sessionId)
  const local = draft ?? (session && !knownToSource(session) ? session : undefined)
  return !local || Boolean(channelOf(state, local.channelId))
}

/**
 * Asks the source to take a message already in the outbox. While the source
 * has not spoken of the session — it began here, at revision 0 — the message
 * carries what the session starts with, which the source takes once however
 * often it is sent. A refusal marks the message as not sent, with the reason.
 */
function deliver(
  sessionId: string,
  message: TurnMessage,
  model: ModelRef,
  initiator: Initiator,
): WorkspaceCommand<Promise<SendOutcome>> {
  return async (dispatch, getState, { workspace }) => {
    const state = getState().workspace
    const session = sessionOf(state, sessionId)
    if (!session) return "not-asked"
    // The index may have removed a local session's channel while the model
    // step yielded. Check at the effect boundary, not only at command entry.
    if (!hasCurrentHome(state, sessionId)) {
      dispatch(
        sendFailed({
          sessionId,
          messageId: message.id,
          reason: "unavailable",
          model,
          diagnostic: { message: "This channel is no longer available." },
        }),
      )
      return "not-asked"
    }
    const access = knownToSource(session) ? undefined : accessToStart(state, sessionId)
    // Captured by this message: a pick after it is the newer word, not this one.
    if (access) dispatch(accessCaptured({ sessionId, mode: access }))
    try {
      await workspace.send({
        sessionId,
        initiator,
        messageId: message.id,
        text: messageText(message),
        ...(message.localImages?.length ? { images: message.localImages } : {}),
        model,
        start: knownToSource(session)
          ? undefined
          : {
              channelId: session.channelId,
              title: session.title,
              ...(access ? { access } : {}),
            },
      })
      dispatch(messageDelivered({ sessionId, messageId: message.id }))
      return "sent"
    } catch (error) {
      const reason = failureReason(error)
      dispatch(
        sendFailed({
          sessionId,
          messageId: message.id,
          reason,
          model,
          diagnostic:
            error instanceof WorkspaceSourceError
              ? error.diagnostic
              : {
                  message:
                    error instanceof Error
                      ? error.message
                      : "The gateway request failed.",
                },
        }),
      )
      return outcomeOf(reason)
    }
  }
}

/**
 * What became of a message, so a caller can tell what it did: `sent` and
 * taken, `refused` or `unknown` (`outcomeOf`) — the message stays in the
 * outbox, marked not sent — or `not-asked`: nothing to send, or no session
 * or draft to send it to.
 */
export type SendOutcome = "sent" | "refused" | "unknown" | "not-asked"

/**
 * Sends a message. It shows at once in the outbox, marked as sending; a draft
 * starts with it. The source's own updates then carry the reply; a refused
 * message stays, to be sent again or let go.
 */
export function sendMessage({
  sessionId,
  text,
  initiator,
  withImages = false,
}: {
  sessionId: string
  text: string
  /** Who sent it, for the source's record: the person at the composer, or an agent. */
  initiator: Initiator
  /**
   * Whether the message takes the images on the session's composer: only a
   * send from the composer that shows them does, so a reply sent from
   * elsewhere never carries images the person cannot see.
   */
  withImages?: boolean
}): WorkspaceCommand<Promise<SendOutcome>> {
  return async (dispatch, getState, { now, newId }) => {
    const content = text.trim()
    const state = getState().workspace
    const model = modelForNextTurn(state, sessionId)
    // The person's send from the composer takes what they pasted with it;
    // any other message leaves the composer's images where they are.
    const images =
      withImages && initiator === "person"
        ? (entry(state.composerImages, sessionId) ?? [])
        : []
    if (
      (!content && images.length === 0) ||
      !model ||
      !hasCurrentHome(state, sessionId) ||
      modelChanging(state, sessionId)
    )
      return "not-asked"
    const message: TurnMessage = {
      id: newId(),
      role: "user",
      at: now(),
      parts: [{ kind: "text", text: content }],
      ...(images.length ? { localImages: images } : {}),
      delivery: { state: "sending" },
    }
    dispatch(messageSent({ sessionId, message, initiator }))
    return dispatch(deliverOnStagedModel(sessionId, message, model, initiator))
  }
}

/** A message's image bounds, as the message contract sets them. */
export const DRAFT_IMAGE_LIMITS: DraftImageLimits = {
  images: MAX_MESSAGE_IMAGES,
  bytes: MAX_IMAGE_ATTACHMENT_BYTES,
  totalBytes: MAX_MESSAGE_IMAGE_BYTES,
  types: IMAGE_ATTACHMENT_TYPES,
}

/**
 * Puts the images among `files` — pasted into a session's composer — on its
 * next message. All of them or none: answers why none were taken, or `null`
 * once they are on it (or there were no images to take).
 */
export function addComposerImages({
  sessionId,
  files,
}: {
  sessionId: string
  files: readonly File[]
}): WorkspaceCommand<DraftImageRefusal | null> {
  return (dispatch, getState, { newId }) => {
    const offered = files.filter(pastedImage)
    if (offered.length === 0) return null
    const held = entry(getState().workspace.composerImages, sessionId) ?? []
    const refusal = draftImagesRefusal(
      held,
      offered.map((file) => ({ size: file.size, mimeType: file.type })),
      DRAFT_IMAGE_LIMITS,
    )
    if (refusal) return refusal
    const images: DraftImage[] = offered.map((file) => ({
      id: newId(),
      // A pasted screenshot can come with no name of its own.
      name: file.name || "Pasted image",
      mimeType: file.type,
      size: file.size,
      url: URL.createObjectURL(file),
    }))
    dispatch(composerImagesAdded({ sessionId, images }))
    // A session the window does not hold took none: nobody will name these.
    const kept = new Set(
      (entry(getState().workspace.composerImages, sessionId) ?? []).map(
        (image) => image.url,
      ),
    )
    for (const image of images) if (!kept.has(image.url)) URL.revokeObjectURL(image.url)
    return null
  }
}

/** Takes one image off a session's next message. */
export const removeComposerImage = workspaceActions.composerImageRemoved

/**
 * Delivers a message already in the outbox after the model staged in its
 * session's composer is applied (`applyStagedModel`): the switch happens
 * when the message is sent, not when the model is picked. A change the
 * source did not confirm marks the message not sent, with why.
 */
function deliverOnStagedModel(
  sessionId: string,
  message: TurnMessage,
  model: ModelRef,
  initiator: Initiator,
): WorkspaceCommand<Promise<SendOutcome>> {
  return async (dispatch) => {
    const staged = await dispatch(applyStagedModel(sessionId))
    if (staged.applied)
      return dispatch(deliver(sessionId, message, staged.model, initiator))
    dispatch(
      sendFailed({
        sessionId,
        messageId: message.id,
        reason: staged.reason,
        model,
        ...(staged.diagnostic ? { diagnostic: staged.diagnostic } : {}),
      }),
    )
    return outcomeOf(staged.reason)
  }
}

/**
 * Sends a refused message again, in its place and under its id, so a source
 * that did take it the first time takes it once.
 */
export function resendMessage({
  sessionId,
  messageId,
  initiator,
}: {
  sessionId: string
  messageId: string
  initiator: Initiator
}): WorkspaceCommand<Promise<SendOutcome>> {
  return async (dispatch, getState) => {
    const state = getState().workspace
    const refused = entry(state.outbox, sessionId)?.find(
      (message) => message.id === messageId && message.delivery?.state === "failed",
    )
    const model = modelForNextTurn(state, sessionId)
    if (
      !refused ||
      !model ||
      !hasCurrentHome(state, sessionId) ||
      modelChanging(state, sessionId)
    )
      return "not-asked"
    dispatch(messageResent({ sessionId, messageId }))
    return dispatch(deliverOnStagedModel(sessionId, refused, model, initiator))
  }
}

/**
 * What became of an answer to an approval, so a caller — an agent above all —
 * can tell what it did: `sent` and taken; `refused` by the source (the pane
 * says why while it shows the session); `unknown` when it is not known to be
 * taken (`outcomeOf`); `answering` already, a first answer on
 * its way; or `not-asked` — the window holds no such approval: the session is
 * not on screen (`onScreen`: in a pane, in the open overview, or as one of
 * the session list's request cards), or its
 * conversation has moved on. Nothing is sent
 * then, and the source records nothing, since nothing was asked of it.
 */
export type AnswerOutcome = "sent" | "refused" | "unknown" | "answering" | "not-asked"

/**
 * A call that failed, as its caller hears it: `unknown` for `unavailable`
 * (what that covers is `failure.ts`'s to say), `refused` for every other
 * reason. The source's updates say what became of it.
 */
function outcomeOf(reason: WorkspaceFailureReason): "refused" | "unknown" {
  return reason === "unavailable" ? "unknown" : "refused"
}

/**
 * Answers the approval named — the one seen. An approval the conversation no
 * longer asks, replaced meanwhile by another, is not answered: nobody has
 * seen the new one.
 */
function answer(
  sessionId: string,
  approvalId: string,
  send: (dependencies: WorkspaceDependencies) => Promise<void>,
): WorkspaceCommand<Promise<AnswerOutcome>> {
  return async (dispatch, getState, dependencies) => {
    const state = getState().workspace
    const approval = entry(state.transcripts, sessionId)?.approvals.find(
      (asked) => asked.id === approvalId,
    )
    // Only an approval on screen is answered: one a pane, the open overview or a list card shows.
    if (!onScreen(state, sessionId) || !approval) return "not-asked"
    // One answer to an approval at a time: a second waits until the first is refused.
    if (answering(answerOf(state, sessionId, approvalId), approvalId)) return "answering"
    const token = dependencies.newId()
    dispatch(approvalAnswering({ sessionId, approvalId: approval.id, token }))
    try {
      await send(dependencies)
      return "sent"
    } catch (error) {
      const reason = failureReason(error)
      dispatch(approvalFailed({ sessionId, approvalId: approval.id, token, reason }))
      return outcomeOf(reason)
    }
  }
}

/**
 * Allows what the session's agent is waiting to run: this once, or always.
 * Who decided goes with it — the person at the window's controls, or an agent
 * — for the source's record of the decision.
 */
export function approve({
  sessionId,
  approvalId,
  scope = "once",
  initiator,
  optionId,
}: {
  sessionId: string
  approvalId: string
  scope?: ApprovalScope
  initiator: Initiator
  /** The option the person chose. Two of one choice stay distinct by this. */
  optionId: string
}): WorkspaceCommand<Promise<AnswerOutcome>> {
  return answer(sessionId, approvalId, ({ workspace }) =>
    workspace.approve(sessionId, approvalId, scope, initiator, optionId),
  )
}

/** Reads again, now, a review whose content could not be read. */
export function retryReview({
  sessionId,
  approvalId,
}: {
  sessionId: string
  approvalId: string
}): WorkspaceCommand {
  return (_dispatch, _getState, { workspace }) =>
    workspace.retryReview?.(sessionId, approvalId)
}

/**
 * What became of an answer to a question the agent asked. `sent` once the
 * source has taken it. `not-asked` when this window is not showing that
 * question. `refused` when the source declined it, `unknown` when it is not
 * known to have been taken. `different` when another answer to this ask is
 * already on its way: that send is left alone, and this one is not reported
 * as delivered. A second card joins only when its choices and initiator
 * match the send already on its way, and then receives that same outcome.
 * The question leaves when a later read omits it.
 */
export type QuestionOutcome =
  | { readonly status: "sent" }
  | { readonly status: "not-asked" }
  | { readonly status: "different" }
  | { readonly status: "refused"; readonly reason: WorkspaceFailureReason }
  | { readonly status: "unknown"; readonly reason: WorkspaceFailureReason }

/**
 * Answers a question the session's agent is waiting on, or declines it
 * (`choices` null). Only a question on screen is answered. The card keeps
 * its own busy state; whether a send is already on its way belongs to this
 * store's `questionFlights`. The same choices and initiator join that send,
 * so a second card hears the refusal and can try again. A different answer
 * does not.
 */
export function answerQuestion({
  sessionId,
  executionId,
  questionId,
  choices,
  initiator,
  key,
}: {
  sessionId: string
  executionId: string
  questionId: string
  choices: readonly QuestionChoice[] | null
  initiator: Initiator
  /** The question's own key, as the source gave it (`TranscriptQuestion.key`). */
  key?: string
}): WorkspaceCommand<Promise<QuestionOutcome>> {
  return async (_dispatch, getState, { workspace, questionFlights }) => {
    const state = getState().workspace
    // Only the question on screen, read whole: one still loading offers no
    // answer, and one asked again under the same identities has another key.
    const asked = entry(state.transcripts, sessionId)?.questions?.some(
      (question) =>
        question.executionId === executionId &&
        question.questionId === questionId &&
        question.key === key &&
        question.loading === undefined,
    )
    if (!onScreen(state, sessionId) || !asked) return { status: "not-asked" }
    const flight = JSON.stringify([sessionId, executionId, questionId, key])
    const stamp = JSON.stringify([choices, initiator])
    const outcome = await questionFlights.join(flight, stamp, async () => {
      try {
        await workspace.answerQuestion(
          sessionId,
          executionId,
          questionId,
          choices,
          initiator,
          key,
        )
        return { status: "sent" as const }
      } catch (error) {
        const reason = failureReason(error)
        return reason === "unavailable"
          ? { status: "unknown" as const, reason }
          : { status: "refused" as const, reason }
      }
    })
    if (outcome === differentFlight) return { status: "different" }
    return outcome
  }
}

/** Refuses what the session's agent is waiting to run. */
export function deny({
  sessionId,
  approvalId,
  initiator,
  optionId,
}: {
  sessionId: string
  approvalId: string
  initiator: Initiator
  /** The option the person chose. */
  optionId: string
}): WorkspaceCommand<Promise<AnswerOutcome>> {
  return answer(sessionId, approvalId, ({ workspace }) =>
    workspace.deny(sessionId, approvalId, initiator, optionId),
  )
}

/**
 * What became of a pin or an archive asked of the source: `sent` and taken;
 * `refused` (logged); `unknown` when it is not known to be taken (`outcomeOf`); or
 * `not-asked` — nothing to change, or a session the source has not spoken
 * of. Each call answers for itself: of two archives of
 * one session asked at once, the second is `refused` as `unknown-session`
 * once the first has taken it; two pins asked at once are both `sent`, and
 * both on the source's record.
 */
export type ChangeOutcome = "sent" | "refused" | "unknown" | "not-asked"

/**
 * Stops the turn a session's agent is at work on, saying who asked for the
 * source's record. Only a session on screen whose conversation shows a turn
 * at work is asked (`atWork`); the conversation that no longer runs arrives
 * as the source's update, and a refusal leaves it as it was.
 */
export function stopTurn({
  sessionId,
  initiator,
}: {
  sessionId: string
  initiator: Initiator
}): WorkspaceCommand<Promise<ChangeOutcome>> {
  return async (_dispatch, getState, { workspace }) => {
    const state = getState().workspace
    if (!onScreen(state, sessionId) || !atWork(entry(state.transcripts, sessionId)))
      return "not-asked"
    try {
      await workspace.stop(sessionId, initiator)
      return "sent"
    } catch (error) {
      const reason = failureReason(error)
      console.warn("Could not stop a turn", sessionId, reason)
      return outcomeOf(reason)
    }
  }
}

/**
 * Pins a session to the top of its channel's list, or unpins it, saying who
 * asked for the source's record. The pin shows when the source's update says
 * so; a refusal leaves it as it was.
 */
export function pinSession({
  sessionId,
  pinned,
  initiator,
}: {
  sessionId: string
  pinned: boolean
  initiator: Initiator
}): WorkspaceCommand<Promise<ChangeOutcome>> {
  return async (_dispatch, getState, { workspace }) => {
    const session = sessionOf(getState().workspace, sessionId)
    if (!session || session.pinned === pinned || !knownToSource(session))
      return "not-asked"
    try {
      await workspace.setPinned(sessionId, pinned, initiator)
      return "sent"
    } catch (error) {
      const reason = failureReason(error)
      console.warn("Could not pin a session", sessionId, reason)
      return outcomeOf(reason)
    }
  }
}

/**
 * Archives a session, saying who asked for the source's record. The source
 * reports the removal, and that update takes it out of the lists and closes
 * its pane, or starts the last over. A session the source has not spoken of
 * is not archived: it is let go by discarding its unsent message. A refusal
 * leaves the session where it is.
 */
export function archiveSession({
  sessionId,
  initiator,
}: {
  sessionId: string
  initiator: Initiator
}): WorkspaceCommand<Promise<ChangeOutcome>> {
  return async (_dispatch, getState, { workspace }) => {
    const session = sessionOf(getState().workspace, sessionId)
    if (!session || !knownToSource(session)) return "not-asked"
    try {
      await workspace.archive(sessionId, initiator)
      return "sent"
    } catch (error) {
      const reason = failureReason(error)
      console.warn("Could not archive a session", sessionId, reason)
      return outcomeOf(reason)
    }
  }
}

/**
 * Renames a session to what the person typed, saying who asked for the
 * source's record. The new title shows when the source's update says so;
 * what it keeps of the typed text, nothing among it, is the source's to say.
 */
export function renameSession({
  sessionId,
  title,
  initiator,
}: {
  sessionId: string
  title: string
  initiator: Initiator
}): WorkspaceCommand<Promise<ChangeOutcome>> {
  return async (_dispatch, getState, { workspace }) => {
    const session = sessionOf(getState().workspace, sessionId)
    if (!session || !knownToSource(session)) return "not-asked"
    try {
      await workspace.rename(sessionId, title, initiator)
      return "sent"
    } catch (error) {
      const reason = failureReason(error)
      console.warn("Could not rename a session", sessionId, reason)
      return outcomeOf(reason)
    }
  }
}

/**
 * Forks a session, saying who asked for the source's record, and opens the
 * new one in the focused pane once its summary has arrived. The new
 * session's transcript, its copied turns and the line after them, is the
 * source's to say. A session the source has not spoken of is not asked.
 */
export function forkSession({
  sessionId,
  initiator,
}: {
  sessionId: string
  initiator: Initiator
}): WorkspaceCommand<Promise<ChangeOutcome>> {
  return async (dispatch, getState, { workspace }) => {
    const session = sessionOf(getState().workspace, sessionId)
    if (!session || !knownToSource(session)) return "not-asked"
    try {
      const forked = await workspace.fork(sessionId, initiator)
      dispatch(workspaceActions.openSession({ sessionId: forked }))
      return "sent"
    } catch (error) {
      const reason = failureReason(error)
      console.warn("Could not fork a session", sessionId, reason)
      return outcomeOf(reason)
    }
  }
}

/**
 * Deletes a session for good, saying who asked for the source's record. As
 * for an archive, the source's removal takes it out of the lists; a session
 * the source has not spoken of is not asked, and a refusal leaves it where it
 * is. The person has confirmed it first (`ui/session-row-actions.tsx`).
 */
export function deleteSession({
  sessionId,
  initiator,
}: {
  sessionId: string
  initiator: Initiator
}): WorkspaceCommand<Promise<ChangeOutcome>> {
  return async (_dispatch, getState, { workspace }) => {
    const session = sessionOf(getState().workspace, sessionId)
    if (!session || !knownToSource(session)) return "not-asked"
    try {
      await workspace.delete(sessionId, initiator)
      return "sent"
    } catch (error) {
      const reason = failureReason(error)
      console.warn("Could not delete a session", sessionId, reason)
      return outcomeOf(reason)
    }
  }
}

/** Opens a provider's login, leaving the failed turn and its recovery notice intact. */
export const signInToProvider =
  (provider: "claude" | "codex"): WorkspaceCommand<Promise<void>> =>
  async (_dispatch, _getState, dependencies) => {
    if (!dependencies.signInToProvider)
      throw new Error("Provider sign-in is unavailable on this host.")
    await dependencies.signInToProvider(provider)
  }

/** Read the injected host's login capability without attempting a launch. */
export const providerLoginAvailable =
  (): WorkspaceCommand<Promise<boolean>> => async (_dispatch, _getState, dependencies) =>
    !!dependencies.signInToProvider &&
    (await (dependencies.providerLoginAvailable?.() ?? true))
