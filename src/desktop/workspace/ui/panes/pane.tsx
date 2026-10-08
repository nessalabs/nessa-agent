import {
  memo,
  startTransition,
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react"
import { sendMessage } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import {
  selectDraft,
  selectFocusedPaneKey,
  selectPaneSession,
  selectSession,
} from "../../adapters/store/selectors"
import { settleOnReshape } from "../../adapters/dom/home-shape"
import { focusedPaneAttribute } from "../../adapters/dom/focus"
import { captureArrival, type Arrival } from "../../adapters/dom/arrival"
import { usePictureInConversationsPreference } from "../../../adapters/window-preferences"
import { HeaderSliver } from "../../../ui/header-art"
import type { PaneFrame } from "../../../split-panes"
import type { PanePlacement } from "../../../split-panes/model/pane-sizing"
import { Conversation, PaneHome } from "./conversation"
import { PaneHeader } from "./pane-header"
import { usePaneFocus } from "./use-pane-focus"

/**
 * One pane: its header, and a new session's home or a conversation. It
 * subscribes to which session it shows and whether it is focused; the
 * session's own content is read further down, so a reply streaming here
 * renders this pane's transcript and nothing else.
 *
 * A session or another pane carried over it (`split-panes/adapters/dom/drag.ts`)
 * lands by zone: a side splits, the middle opens in place or swaps. Sending a
 * new session's first message replaces its home with the conversation; the
 * workspace's focus owner follows the removed field.
 */
export const Pane = memo(function Pane({
  placement,
  frame,
  multi,
}: {
  placement: PanePlacement
  /** What the grid puts on the pane's root: where it is placed, and its names (`SplitPanes`). */
  frame: PaneFrame
  multi: boolean
}) {
  const key = placement.key
  const dispatch = useWorkspaceDispatch()
  const focusHandlers = usePaneFocus(key)
  const sessionId = useWorkspaceSelector((state) => selectPaneSession(state, key)) ?? ""
  const focused = useWorkspaceSelector((state) => selectFocusedPaneKey(state) === key)
  const draft = useWorkspaceSelector(
    (state) => selectDraft(state, sessionId) !== undefined,
  )
  const sessionTitle = useWorkspaceSelector(
    (state) => selectSession(state, sessionId)?.title,
  )
  const listed = sessionTitle !== undefined
  const [headingVisible, setHeadingVisible] = useState(false)
  // A new pane answers on the frame it was asked for: its shell fades in at
  // once, and what it shows fills in over the next frames, under the fade, in
  // a transition React may slice, so a split never waits on a home's scene.
  const [filled, setFilled] = useState(false)
  useEffect(() => {
    const frame = requestAnimationFrame(() => startTransition(() => setFilled(true)))
    return () => cancelAnimationFrame(frame)
  }, [])

  const homeRef = useRef<HTMLDivElement>(null)
  const [arriving, setArriving] = useState<{
    sessionId: string
    from: Arrival
  } | null>(null)
  if (arriving && arriving.sessionId !== sessionId) setArriving(null)
  const arrival = arriving?.sessionId === sessionId ? arriving.from : null
  const finishArrival = useCallback(() => setArriving(null), [])
  // The session shown now, for the callbacks below: they keep their identity
  // for the pane's life, so the memoised home and conversation they are
  // handed render only for what they show, never because this pane did.
  const shownRef = useRef(sessionId)
  shownRef.current = sessionId
  const sendFromHome = useCallback(
    (text: string) => {
      const shown = shownRef.current
      const from = captureArrival(homeRef.current)
      setArriving(from ? { sessionId: shown, from } : null)
      void dispatch(sendMessage({ sessionId: shown, text, initiator: "person" })).then(
        (outcome) => {
          if (outcome === "not-asked")
            setArriving((current) => (current?.from === from ? null : current))
        },
      )
    },
    [dispatch],
  )
  const showHome = draft
  // A new session's home settles when its pane changes its shape, not as it appears.
  const homeShown = filled && showHome
  useLayoutEffect(() => {
    const home = homeRef.current
    if (!homeShown || !home) return
    return settleOnReshape(home)
  }, [homeShown, sessionId])
  const [picture] = usePictureInConversationsPreference()
  const pictured = picture === "on"
  return (
    <article
      className="workspace-pane"
      {...frame}
      data-focused={(focused && multi) || undefined}
      {...{ [focusedPaneAttribute]: focused || undefined }}
      aria-label={sessionTitle ?? (draft ? "New session" : "Empty pane")}
      {...focusHandlers}
    >
      {pictured && filled && listed && !showHome ? (
        <HeaderSliver moving={focused} />
      ) : null}
      <PaneHeader
        pane={key}
        sessionId={sessionId}
        multi={multi}
        titleShown={listed && !headingVisible}
      />
      <div className="workspace-pane-body" data-split-through>
        {!filled ? null : showHome ? (
          <div
            key={`home-${sessionId}`}
            ref={homeRef}
            className="workspace-pane-home"
            data-split-keeps="middle"
          >
            <PaneHome sessionId={sessionId} onSend={sendFromHome} />
          </div>
        ) : null}
        {filled && listed ? (
          <Conversation
            key={sessionId}
            sessionId={sessionId}
            onHeadingVisible={setHeadingVisible}
            arrival={arrival}
            onArrivalDone={finishArrival}
          />
        ) : null}
      </div>
    </article>
  )
})
