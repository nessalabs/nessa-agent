import { memo, useCallback, useLayoutEffect, useRef, useState } from "react"
import { Composer } from "../../../ui/composer"
import { chooseModel, sendMessage, setComposerText } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectComposerText, selectNextModel } from "../../adapters/store/selectors"
import { useArrival, type Arrival } from "../../adapters/dom/arrival"
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
 * home before the conversation replaces it.
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
    />
  )
})

/**
 * A conversation in a pane: the transcript, and a composer docked at its
 * foot. Mounted with an `arrival` when its first message was just sent from
 * the home it replaces, which then plays once.
 */
export const Conversation = memo(function Conversation({
  sessionId,
  arrival,
  takeFocus,
  onHeadingVisible,
}: {
  sessionId: string
  arrival: Arrival | null
  /** Asked once, on mount: whether to take the caret, the person having typed in the home. */
  takeFocus: () => boolean
  onHeadingVisible: (visible: boolean) => void
}) {
  const scrollRef = useRef<HTMLDivElement>(null)
  const headingRef = useRef<HTMLDivElement>(null)
  const dockRef = useRef<HTMLDivElement>(null)
  // Whether this conversation arrived from a home is fixed at mount.
  const [arrived] = useState(arrival)
  useArrival(arrived, { dock: dockRef, scroller: scrollRef, heading: headingRef })

  const focusAtMount = useRef(takeFocus)
  useLayoutEffect(() => {
    if (focusAtMount.current())
      dockRef.current?.querySelector("textarea")?.focus({ preventScroll: true })
  }, [])

  return (
    <div className="workspace-conversation">
      <Transcript
        sessionId={sessionId}
        arriving={arrived !== null}
        scrollRef={scrollRef}
        headingRef={headingRef}
        onHeadingVisible={onHeadingVisible}
      />
      <div className="workspace-dock" ref={dockRef}>
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
