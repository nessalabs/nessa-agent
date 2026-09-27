import {
  memo,
  startTransition,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type DragEvent,
} from "react"
import {
  dropSession,
  focusPane,
  movePane,
  sendMessage,
} from "../../adapters/store/commands"
import {
  useWorkspaceDispatch,
  useWorkspaceSelector,
  useWorkspaceStore,
} from "../../adapters/store/hooks"
import {
  selectDraft,
  selectFocusedPaneKey,
  selectPaneSession,
  selectPanes,
  selectSession,
} from "../../adapters/store/selectors"
import { measureArrival, type Arrival } from "../../adapters/dom/arrival"
import {
  paneDragType,
  sessionDragType,
  useCarriedPane,
  usePaneDrag,
} from "../../adapters/dom/drag"
import { durationToken } from "../../adapters/dom/motion"
import { isFull, type Side, type Zone } from "../../model/pane-layout"
import { canPlace, type PanePlacement } from "../../model/pane-sizing"
import { useWorkspaceFrame } from "../workspace-frame"
import { Conversation, PaneHome } from "./conversation"
import { DropTarget } from "./drop-target"
import { EmptyPane } from "./empty-state"
import { PaneHeader } from "./pane-header"

/** How near a pane's edge, as a share of its size, a drop splits rather than opens in place. */
const edgeShare = 0.26

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
 * A session or another pane dropped on it lands by zone: a side splits, the
 * middle opens in place or swaps. Sending a new session's first message
 * measures its home first, so the composer can glide into the conversation.
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
  const frame = useWorkspaceFrame()
  const drag = usePaneDrag()
  const sessionId = useWorkspaceSelector((state) => selectPaneSession(state, key)) ?? ""
  const focused = useWorkspaceSelector((state) => selectFocusedPaneKey(state) === key)
  const draft = useWorkspaceSelector(
    (state) => selectDraft(state, sessionId) !== undefined,
  )
  const sessionTitle = useWorkspaceSelector(
    (state) => selectSession(state, sessionId)?.title,
  )
  const listed = sessionTitle !== undefined
  const lifted = useCarriedPane() === key
  const [headingVisible, setHeadingVisible] = useState(false)
  // A new pane answers on the frame it was asked for: its shell fades in at
  // once, and what it shows fills in over the next frames, under the fade, in
  // a transition React may slice, so a split never waits on a home's scene.
  const [filled, setFilled] = useState(false)
  useEffect(() => {
    const frame = requestAnimationFrame(() => startTransition(() => setFilled(true)))
    return () => cancelAnimationFrame(frame)
  }, [])
  const [drop, setDrop] = useState<{ zone: Zone; kind: "session" | "pane" } | null>(null)

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
  const sendFromHome = (text: string) => {
    handoff.current = sessionId
    const measured = measureArrival(homeRef.current)
    if (measured) {
      setArriving({ sessionId, arrival: measured })
      window.clearTimeout(leaveTimer.current)
      // The home stays over the conversation until its composer has landed.
      const home = homeRef.current ?? document.body
      const duration =
        durationToken(home, "--desktop-arrival") +
        durationToken(home, "--desktop-stagger") / 2
      leaveTimer.current = window.setTimeout(() => setArriving(null), duration)
    }
    void dispatch(sendMessage({ sessionId, text, initiator: "person" }))
  }
  const takeFocus = () => {
    const mine = handoff.current === sessionId
    handoff.current = null
    return mine
  }

  const full = () => {
    const panes = selectPanes(store.getState())
    return !panes || isFull(panes)
  }

  // The drop target nearest the pointer's edge, or the middle to open in place.
  const zoneFor = (event: DragEvent<HTMLElement>, moving?: number): Zone => {
    const box = event.currentTarget.getBoundingClientRect()
    const x = (event.clientX - box.left) / box.width
    const y = (event.clientY - box.top) / box.height
    const near: [Side, number][] = [
      ["left", x],
      ["right", 1 - x],
      ["top", y],
      ["bottom", 1 - y],
    ]
    const [side, distance] = near.reduce((a, b) => (b[1] < a[1] ? b : a))
    if (distance > edgeShare) return "center"
    const panes = selectPanes(store.getState())
    if (!panes) return "center"
    return canPlace(panes, side, key, frame.roomOf(key), moving) ? side : "center"
  }

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
      data-lifted={lifted || undefined}
      aria-label={sessionTitle ?? (draft ? "New session" : "Empty pane")}
      onPointerDown={() => {
        if (!focused) dispatch(focusPane({ pane: key }))
      }}
      onFocusCapture={() => {
        if (selectFocusedPaneKey(store.getState()) !== key)
          dispatch(focusPane({ pane: key }))
      }}
      onDragOver={(event) => {
        const types = event.dataTransfer.types
        const carryingPane = types.includes(paneDragType)
        if (!carryingPane && !types.includes(sessionDragType)) return
        const carried = drag.carried()
        const moving = carried?.kind === "pane" ? carried.pane : undefined
        if (carryingPane && (moving === undefined || moving === key)) return
        event.preventDefault()
        event.dataTransfer.dropEffect = carryingPane ? "move" : "copy"
        const zone = zoneFor(event, moving)
        const kind = carryingPane ? "pane" : "session"
        if (drop?.zone !== zone || drop.kind !== kind) setDrop({ zone, kind })
      }}
      onDragLeave={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null))
          setDrop(null)
      }}
      onDrop={(event) => {
        const zone = drop?.zone ?? "center"
        setDrop(null)
        const room = frame.roomOf(key)
        if (event.dataTransfer.types.includes(paneDragType)) {
          event.preventDefault()
          const carried = drag.carried()
          if (carried?.kind === "pane")
            dispatch(movePane({ pane: carried.pane, target: key, zone, room }))
          drag.end()
          return
        }
        const dropped = event.dataTransfer.getData(sessionDragType)
        if (!dropped) return
        event.preventDefault()
        dispatch(dropSession({ sessionId: dropped, target: key, zone, room }))
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
        {filled && !listed && !draft ? <EmptyPane pane={key} /> : null}
      </div>
      {drop ? <DropTarget zone={drop.zone} kind={drop.kind} full={full()} /> : null}
    </article>
  )
})
