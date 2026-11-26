/**
 * What a conversation's pane shows while a call talks to it: one line above
 * the composer — what the voice is saying, the person's words as they are
 * heard (never more than one line: the newest words, the older ones running
 * off its start), or where the call stands — and the glow on the composer's edge
 * (`voice-rim.tsx`).
 */
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react"
import { Button } from "@nessa-ui/react/button"
import { DesktopIcon } from "../../ui/icons"
import { IconButton } from "../../ui/icon-button"
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
    case "paused":
      return "Paused"
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
    case "paused":
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
        {/*
          One quiet row: what stopped the call, the one thing that fixes it
          in place (if any), and a close. No box: it reads as the call's
          line, where the call's words would have been.
        */}
        <div
          className="voice-notice"
          role="status"
          data-voice-fix={snapshot.fix ?? "none"}
        >
          <DesktopIcon name="voice" className="voice-notice-mark" />
          <span className="voice-notice-text">{snapshot.notice}</span>
          {snapshot.fix === "unlock" ? (
            <Button
              variant="tinted"
              shape="pill"
              size="26"
              className="voice-notice-fix"
              data-voice-fix="unlock"
              onClick={() => {
                void unlock().then((open) => {
                  // Unlocked, the call starts as the person first asked.
                  if (open) void controller.start(sessionId)
                })
              }}
            >
              Unlock
            </Button>
          ) : snapshot.fix === "key" ? (
            <VoiceKeyButton
              className="voice-notice-fix"
              onSaved={() => controller.dismiss()}
            >
              Add key
            </VoiceKeyButton>
          ) : null}
          <IconButton
            icon="close"
            label="Dismiss"
            size="sm"
            shape="pill"
            tone="faint"
            className="voice-notice-close"
            data-voice-dismiss
            onClick={() => controller.dismiss()}
          />
        </div>
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
