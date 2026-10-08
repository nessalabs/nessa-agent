import { memo, useCallback, useLayoutEffect, useRef } from "react"
import { playArrival, type Arrival } from "../../adapters/dom/arrival"
import { Composer } from "../../../ui/composer"
import { chooseModel, sendMessage, setComposerText } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectComposerText, selectNextModel } from "../../adapters/store/selectors"
import type { ModelRef } from "../../model/workspace-index"
import { Home } from "../../../ui/home"
import { Transcript } from "../transcript/transcript"
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
  const send = useCallback(
    (text: string) =>
      void dispatch(sendMessage({ sessionId, text, initiator: "person" })),
    [dispatch, sessionId],
  )
  const changeModel = useCallback(
    (model: ModelRef) => dispatch(chooseModel({ sessionId, model })),
    [dispatch, sessionId],
  )
  return { send, changeModel }
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
  return (
    <Home
      model={model}
      onSend={onSend}
      onModelChange={changeModel}
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
  return (
    <Composer
      text={text}
      onTextChange={onTextChange}
      page={false}
      onPageChange={stayCard}
      model={model}
      onModelChange={changeModel}
      onSend={send}
      // In a conversation the field is a reply, and says so in fewer words than the home's question.
      placeholder="Reply…"
    />
  )
})
