import {
  memo,
  startTransition,
  useCallback,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
} from "react"
import { focusPane, sendMessage } from "../../adapters/store/commands"
import {
  useWorkspaceDispatch,
  useWorkspaceSelector,
  useWorkspaceStore,
} from "../../adapters/store/hooks"
import {
  selectDraft,
  selectFocusedPaneKey,
  selectPaneSession,
  selectSession,
} from "../../adapters/store/selectors"
import { measureArrival, type Arrival } from "../../adapters/dom/arrival"
import { focusedPaneAttribute } from "../../adapters/dom/focus"
import { durationToken } from "../../adapters/dom/motion"
import type { PanePlacement } from "../../model/pane-sizing"
import { Conversation, PaneHome } from "./conversation"
import { PaneHeader } from "./pane-header"

/** Where a pane is drawn, as the custom properties the stylesheet places it by. */
function placementStyle(placement: PanePlacement): CSSProperties {
  return {
    "--cx": placement.x,
    "--cw": placement.width,
    "--ci": placement.column,
    "--cn": placement.columns,
    "--ry": placement.y,
    "--rh": placement.height,
    "--ri": placement.row,
    "--rn": placement.rows,
  } as CSSProperties
}

/**
 * One pane: its header, and a new session's home or a conversation. It
 * subscribes to which session it shows and whether it is focused; the
 * session's own content is read further down, so a reply streaming here
 * renders this pane's transcript and nothing else.
 *
 * A session or another pane carried over it (`adapters/dom/drag.ts`) lands
 * by zone: a side splits, the middle opens in place or swaps. Sending a new
 * session's first message measures its home first, so the composer can glide
 * into the conversation.
 */
export const Pane = memo(function Pane({
  placement,
  multi,
}: {
  placement: PanePlacement
  multi: boolean
}) {
  const key = placement.key
  const dispatch = useWorkspaceDispatch()
  const store = useWorkspaceStore()
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
  return (
    <article
      className="workspace-pane"
      style={placementStyle(placement)}
      data-flip="pane"
      data-flip-id={key}
      data-pane-key={key}
      data-corner={placement.corner || undefined}
      data-focused={(focused && multi) || undefined}
      {...{ [focusedPaneAttribute]: focused || undefined }}
      aria-label={sessionTitle ?? (draft ? "New session" : "Empty pane")}
      onPointerDown={() => {
        if (!focused) dispatch(focusPane({ pane: key }))
      }}
      onFocusCapture={() => {
        if (selectFocusedPaneKey(store.getState()) !== key)
          dispatch(focusPane({ pane: key }))
      }}
    >
      <PaneHeader
        pane={key}
        sessionId={sessionId}
        multi={multi}
        titleShown={listed && !headingVisible}
      />
      <div className="workspace-pane-body">
        {!filled ? null : showHome ? (
          <div
            key={`home-${sessionId}`}
            ref={homeRef}
            className="workspace-pane-home"
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
