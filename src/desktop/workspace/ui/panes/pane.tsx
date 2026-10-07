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
import { measureArrival, type Arrival } from "../../adapters/dom/arrival"
import { settleOnReshape } from "../../adapters/dom/home-shape"
import { focusedPaneAttribute } from "../../adapters/dom/focus"
import { durationToken } from "../../../adapters/motion"
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
 * new session's first message measures its home first, so the composer can
 * glide into the conversation.
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

  // Sending a first message: the home lifts away over the conversation that
  // replaces it, while its composer travels there as the conversation's own.
  const homeRef = useRef<HTMLDivElement>(null)
  // The arrival belongs to the session whose first message it carries: another
  // session opened in this pane meanwhile does not arrive.
  const [arriving, setArriving] = useState<{
    sessionId: string
    arrival: Arrival
  } | null>(null)
  const arrival = arriving?.sessionId === sessionId ? arriving.arrival : null
  // Another session opened here ends the arrival: coming back does not replay it.
  if (arriving && arriving.sessionId !== sessionId) setArriving(null)
  const leaveTimer = useRef(0)
  useEffect(() => () => window.clearTimeout(leaveTimer.current), [])
  // The caret follows the first message into the conversation.
  const handoff = useRef<string | null>(null)
  // The session shown now, for the callbacks below: they keep their identity
  // for the pane's life, so the memoised home and conversation they are
  // handed render only for what they show, never because this pane did.
  const shownRef = useRef(sessionId)
  shownRef.current = sessionId
  const sendFromHome = useCallback(
    (text: string) => {
      const shown = shownRef.current
      handoff.current = shown
      const measured = measureArrival(homeRef.current)
      if (measured) {
        setArriving({ sessionId: shown, arrival: measured })
        window.clearTimeout(leaveTimer.current)
        // The home stays over the conversation until its composer has landed.
        const home = homeRef.current ?? document.body
        const duration =
          durationToken(home, "--desktop-arrival") +
          durationToken(home, "--desktop-stagger") / 2
        leaveTimer.current = window.setTimeout(() => setArriving(null), duration)
      }
      void dispatch(sendMessage({ sessionId: shown, text, initiator: "person" }))
    },
    [dispatch],
  )
  const takeFocus = useCallback(() => {
    const mine = handoff.current === shownRef.current
    handoff.current = null
    return mine
  }, [])

  const showHome = draft || arrival !== null
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
            data-leaving={(!draft && arrival !== null) || undefined}
          >
            <PaneHome sessionId={sessionId} onSend={sendFromHome} />
          </div>
        ) : null}
        {filled && listed ? (
          <Conversation
            key={sessionId}
            sessionId={sessionId}
            arrival={arrival}
            takeFocus={takeFocus}
            onHeadingVisible={setHeadingVisible}
          />
        ) : null}
      </div>
    </article>
  )
})
