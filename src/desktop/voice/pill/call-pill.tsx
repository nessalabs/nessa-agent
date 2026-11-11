/**
 * The call pill's face: where the call stands and which chat it talks to,
 * then the speaker, the microphone and End. It draws the picture it is given
 * and reports presses; the call is the desktop window's (`application/
 * call-pill.ts`).
 *
 * The chat's name is a button that brings the desktop window forward, and
 * the same stretch moves the pill when dragged, since the pill has no title
 * bar: a press that travels is a drag, one that does not is a click.
 */
import { useRef, type PointerEvent as ReactPointerEvent } from "react"
import { DesktopIcon } from "../../ui/icons"
import { pillWords, type CallPicture, type CallPress } from "../model/call-pill"
import "./call-pill.css"

/** How far a press travels, in CSS pixels, before it is a drag and not a click. */
const DRAG_AFTER = 3

export function CallPill({
  picture,
  onPress,
  onDrag,
}: {
  picture: CallPicture
  onPress: (press: CallPress) => void
  /** The person began dragging the pill by its name. */
  onDrag: () => void
}) {
  const pressed = useRef<{ x: number; y: number; dragged: boolean } | null>(null)
  const moved = (event: ReactPointerEvent) => {
    const from = pressed.current
    if (!from || from.dragged) return
    if (Math.hypot(event.clientX - from.x, event.clientY - from.y) < DRAG_AFTER) return
    from.dragged = true
    onDrag()
  }
  return (
    <div
      className="call-pill"
      role="group"
      aria-label="Voice call"
      data-phase={picture.phase}
      data-microphone={picture.microphoneOn ? "on" : "off"}
    >
      <button
        type="button"
        className="call-pill-name"
        aria-label={`Open ${picture.title}`}
        onPointerDown={(event) => {
          if (event.button !== 0) return
          pressed.current = { x: event.clientX, y: event.clientY, dragged: false }
        }}
        onPointerMove={moved}
        onPointerLeave={moved}
        onClick={() => {
          const dragged = pressed.current?.dragged === true
          pressed.current = null
          if (!dragged) onPress("open")
        }}
      >
        <span className="call-pill-light" aria-hidden="true" />
        <span className="call-pill-words">
          <span className="call-pill-title">{picture.title}</span>
          <span className="call-pill-state" role="status" aria-live="polite">
            {pillWords(picture)}
          </span>
        </span>
      </button>
      <button
        type="button"
        className="call-pill-switch"
        aria-label={picture.speakerOn ? "Mute voice" : "Unmute voice"}
        onClick={() => onPress(picture.speakerOn ? "speaker-off" : "speaker-on")}
      >
        <DesktopIcon name={picture.speakerOn ? "speaker" : "speakerOff"} />
      </button>
      <button
        type="button"
        className="call-pill-switch"
        aria-label={picture.microphoneOn ? "Mute microphone" : "Unmute microphone"}
        onClick={() => onPress(picture.microphoneOn ? "mute" : "unmute")}
      >
        <DesktopIcon name={picture.microphoneOn ? "microphone" : "microphoneOff"} />
      </button>
      <button
        type="button"
        className="call-pill-end"
        aria-label="End call"
        onClick={() => onPress("end")}
      >
        <DesktopIcon name="endCall" />
      </button>
    </div>
  )
}
