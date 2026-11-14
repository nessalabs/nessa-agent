/**
 * What a conversation's pane shows while a call talks to it: one line above
 * the composer — what the voice is saying, the person's words as they are
 * heard (never more than one line: the newest words, the older ones running
 * off its start), or where the call stands — and the glow on the composer's edge
 * (`voice-rim.tsx`).
 */
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react"
import { StatusLabel } from "@nessa-ui/react/status-label"
import { VoiceKeyButton } from "../../settings/ui/voice-key-row"
import { useVaultUnlock } from "../../../vault"
import type { VoiceSnapshot } from "../application/voice-controller"
import { useVoiceController, useVoiceSnapshot } from "./voice-context"
import { VoiceRim } from "./voice-rim"

/** The line: what is said or heard now, else the call's state. */
export function voiceLine(snapshot: VoiceSnapshot): string {
  switch (snapshot.phase) {
    case "connecting":
      return "Connecting…"
    case "speaking":
      return snapshot.saying.trim() || "Speaking…"
    case "working":
      return snapshot.hearing.trim() || "Working…"
    case "listening":
      return (
        snapshot.hearing.trim() ||
        (snapshot.microphoneOn ? (snapshot.hint ?? "Listening") : "Microphone off")
      )
    case "failed":
    case "dropped":
      return snapshot.notice ?? ""
    case "idle":
      return ""
  }
}

/**
 * Whether the line is words being said or heard, which run on as they
 * come, and not where the call stands.
 */
export function voiceLineRuns(snapshot: VoiceSnapshot): boolean {
  switch (snapshot.phase) {
    case "speaking":
      return snapshot.saying.trim() !== ""
    case "working":
    case "listening":
      return snapshot.hearing.trim() !== ""
    case "connecting":
    case "failed":
    case "dropped":
    case "idle":
      return false
  }
}

export function VoiceDock({ sessionId }: { sessionId: string }) {
  const controller = useVoiceController()
  const snapshot = useVoiceSnapshot()
  // The pane says it shows this conversation: a call talking to it has its
  // switches here, and on the pill once no pane shows it.
  useEffect(() => controller?.show(sessionId), [controller, sessionId])
  const unlock = useVaultUnlock()
  const levels = useCallback(
    () => controller?.levels() ?? { input: 0, output: 0 },
    [controller],
  )
  // Whether running words are longer than their line, so its start fades.
  const words = useRef<HTMLSpanElement>(null)
  const [overflows, setOverflows] = useState(false)
  const mine = snapshot?.sessionId === sessionId ? snapshot : undefined
  const shownLine = mine ? voiceLine(mine) : ""
  const runs = mine ? voiceLineRuns(mine) : false
  useLayoutEffect(() => {
    const span = words.current
    const line = span?.parentElement
    setOverflows(
      runs && span !== null && line != null && span.scrollWidth > line.clientWidth,
    )
  }, [shownLine, runs])
  if (!controller || !snapshot || snapshot.phase === "idle") return null
  if (snapshot.sessionId !== sessionId) return null
  if (snapshot.phase === "failed" || snapshot.phase === "dropped")
    return (
      <div className="voice-dock" data-phase={snapshot.phase}>
        <StatusLabel role="status" tone="warning" className="voice-notice">
          {snapshot.notice}
        </StatusLabel>
        {snapshot.fix === "unlock" ? (
          <button
            type="button"
            className="voice-notice-dismiss"
            data-voice-fix="unlock"
            onClick={() => {
              void unlock().then((open) => {
                // Unlocked, the call starts as the person first asked.
                if (open) void controller.start(sessionId)
              })
            }}
          >
            Unlock
          </button>
        ) : snapshot.fix === "key" ? (
          <VoiceKeyButton
            className="voice-notice-dismiss"
            onSaved={() => controller.dismiss()}
          >
            Add key
          </VoiceKeyButton>
        ) : null}
        <button
          type="button"
          className="voice-notice-dismiss"
          onClick={() => controller.dismiss()}
        >
          OK
        </button>
      </div>
    )
  const line = voiceLine(snapshot)
  return (
    <div className="voice-dock" data-phase={snapshot.phase}>
      <p
        className="voice-bubble"
        role="status"
        aria-live="polite"
        data-running={runs ? "" : undefined}
        data-overflow={overflows ? "" : undefined}
      >
        <span ref={words} className="voice-bubble-words">
          {line}
        </span>
      </p>
      <VoiceRim
        phase={snapshot.phase}
        microphoneOn={snapshot.microphoneOn}
        levels={levels}
      />
    </div>
  )
}
