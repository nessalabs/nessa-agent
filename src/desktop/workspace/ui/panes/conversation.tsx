import { modelChanging, modelRequesting } from "../../adapters/store/model-commands"
import { draftOf, entry } from "../../application/workspace-state"
import { WorkspaceSendFailure } from "../send-failure"
import { memo, useCallback, useLayoutEffect, useRef, useState } from "react"
import { playArrival, type Arrival } from "../../adapters/dom/arrival"
import { Composer } from "../../../ui/composer"
import {
  changeModel as requestModel,
  sendMessage,
  setComposerText,
  stopTurn,
} from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import {
  selectAtWork,
  selectCanChooseModel,
  selectComposerText,
  selectNextModel,
} from "../../adapters/store/selectors"
import type { ModelRef } from "../../model/workspace-index"
import { Home } from "../../../ui/home"
import { Transcript } from "../transcript/transcript"
import { useComposerAccess } from "./composer-access"
import "./conversation.css"

/** The docked composer is always a card; only a home opens into a page. */
const stayCard = () => {}

/**
 * The model a session's composer shows: the one its next message is sent
 * with (`modelForNextTurn`), selected, so a choice made anywhere — the
 * picker, an agent's `chooseModel`, a newer summary from the source — is
 * what the picker shows while it stays mounted (`conversation.test.tsx`).
 */
function useNextModel(sessionId: string): ModelRef | undefined {
  return useWorkspaceSelector((state) => selectNextModel(state, sessionId))
}

/** A session's unsent text, held in the workspace so it outlives a change of layout. */
function useComposerText(sessionId: string) {
  const dispatch = useWorkspaceDispatch()
  const text = useWorkspaceSelector((state) => selectComposerText(state, sessionId))
  const onTextChange = useCallback(
    (next: string) => dispatch(setComposerText({ sessionId, text: next })),
    [dispatch, sessionId],
  )
  return { text, onTextChange }
}

function useComposerActions(sessionId: string) {
  const dispatch = useWorkspaceDispatch()
  const canChangeModel = useWorkspaceSelector((state) =>
    selectCanChooseModel(state, sessionId),
  )
  const send = useCallback(
    (text: string) =>
      void dispatch(sendMessage({ sessionId, text, initiator: "person" })),
    [dispatch, sessionId],
  )
  const changeModel = useCallback(
    (model: ModelRef) => void dispatch(requestModel({ sessionId, model })),
    [dispatch, sessionId],
  )
  return { send, changeModel: canChangeModel ? changeModel : undefined }
}

/**
 * Stop, while the session's agent is at work (`atWork`): what the docked
 * composer's send button becomes. Held as asked until the stop settles, so
 * one press asks once; the turn's end arrives as the source's update.
 */
function useStop(sessionId: string) {
  const dispatch = useWorkspaceDispatch()
  const working = useWorkspaceSelector((state) => selectAtWork(state, sessionId))
  const [stopping, setStopping] = useState(false)
  const stop = useCallback(() => {
    setStopping(true)
    void dispatch(stopTurn({ sessionId, initiator: "person" })).finally(() =>
      setStopping(false),
    )
  }, [dispatch, sessionId])
  return working ? { stop, stopping } : undefined
}

/**
 * A new session's home in a pane: the window's scene, greeting and composer,
 * the composer wired to this session, so the model chosen here is the one it
 * keeps and its first message starts it. `onSend` lets the pane measure the
 * home before the conversation replaces it. Its composer sits in the dock's
 * box, so a small pane docks it at the foot as a conversation's
 * (`conversation.css`).
 */
export const PaneHome = memo(function PaneHome({
  sessionId,
  onSend,
}: {
  sessionId: string
  onSend: (text: string) => void
}) {
  const model = useNextModel(sessionId)
  const { changeModel } = useComposerActions(sessionId)
  const { text, onTextChange } = useComposerText(sessionId)
  const access = useComposerAccess(sessionId)

  return (
    <Home
      model={model}
      onSend={onSend}
      onModelChange={changeModel}
      access={access}
      text={text}
      onTextChange={onTextChange}
      // Docked as a conversation's composer is, where the pane is small (`conversation.css`).
      composerClassName="workspace-dock"
    />
  )
})

/**
 * A conversation in a pane: the transcript, and a composer docked at its
 * foot. The workspace's focus owner follows the field removed with a home;
 * this view does not focus or measure a second composer during that commit.
 */
export const Conversation = memo(function Conversation({
  sessionId,
  onHeadingVisible,
  arrival = null,
  onArrivalDone = stayCard,
}: {
  sessionId: string
  onHeadingVisible: (visible: boolean) => void
  arrival?: Arrival | null
  onArrivalDone?: () => void
}) {
  const scrollRef = useRef<HTMLDivElement>(null)
  const conversationRef = useRef<HTMLDivElement>(null)
  useLayoutEffect(() => {
    const conversation = conversationRef.current
    if (conversation && arrival) return playArrival(conversation, arrival, onArrivalDone)
  }, [arrival, onArrivalDone])

  return (
    // Its transcript and its composer are each a part of the pane to a drag's preview.
    <div ref={conversationRef} className="workspace-conversation" data-split-through>
      <Transcript
        sessionId={sessionId}
        scrollRef={scrollRef}
        onHeadingVisible={onHeadingVisible}
      />
      <div className="workspace-dock" data-split-keeps="foot">
        <DockComposer sessionId={sessionId} />
      </div>
    </div>
  )
})

/** The docked composer, showing the model the session's next message is sent with. */
const DockComposer = memo(function DockComposer({ sessionId }: { sessionId: string }) {
  const model = useNextModel(sessionId)
  const { send, changeModel } = useComposerActions(sessionId)
  const { text, onTextChange } = useComposerText(sessionId)
  const stop = useStop(sessionId)
  const access = useComposerAccess(sessionId)
  const modelChoice = useWorkspaceSelector((state) =>
    entry(state.workspace.modelChoices, sessionId),
  )
  const modelControl = useWorkspaceSelector(
    (state) => entry(state.workspace.transcripts, sessionId)?.modelControl,
  )
  const modelRequest = useWorkspaceSelector((state) =>
    modelRequesting(state.workspace, sessionId),
  )
  const modelBusy = useWorkspaceSelector((state) =>
    modelChanging(state.workspace, sessionId),
  )
  const isDraft = useWorkspaceSelector((state) => !!draftOf(state.workspace, sessionId))
  const modelNotice =
    modelControl?.pending?.status === "recovery_required" ||
    modelChoice?.state === "recovery_required" ||
    modelChoice?.state === "acknowledged"
      ? "The model change is not confirmed. Choose again to check and try."
      : modelBusy
        ? "Changing model…"
        : modelChoice?.state === "refused"
          ? modelChoice.code === "model_change_not_applied"
            ? "Model didn’t change. Choose again to try."
            : "The model change could not be confirmed. Choose again to check and try."
          : modelControl?.effect === "nextTurn"
            ? "Model applies to the next turn."
            : modelControl?.effect === "subsequentResponses"
              ? "Model applies to subsequent responses."
              : undefined

  return (
    <>
      <WorkspaceSendFailure sessionId={sessionId} />
      <Composer
        text={text}
        onTextChange={onTextChange}
        page={false}
        onPageChange={stayCard}
        model={model}
        onModelChange={changeModel}
        modelProvider={!isDraft ? model?.provider : undefined}
        modelDisabled={modelRequest}
        modelNotice={modelNotice}
        onSend={modelBusy ? undefined : send}
        onStop={stop}
        access={access}
        // In a conversation the field is a reply, and says so in fewer words than the home's question.
        placeholder="Reply…"
      />
    </>
  )
})
