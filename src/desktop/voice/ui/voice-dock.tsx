/**
 * What a conversation's pane shows above its composer while a call talks to
 * it: the orb, and over it one line — what the voice is saying, the
 * person's words as they are heard, or where the call stands.
 */
import { useCallback, useEffect } from "react"
import { StatusLabel } from "@nessa-ui/react/status-label"
import type { VoiceSnapshot } from "../application/voice-controller"
import { useVoiceController, useVoiceSnapshot } from "./voice-context"
import { orbTone, VoiceOrb } from "./voice-orb"

/** The bubble's line: what is said or heard now, else the call's state. */
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
        (snapshot.microphoneOn ? "Listening" : "Microphone off")
      )
    case "failed":
    case "dropped":
      return snapshot.notice ?? ""
    case "idle":
      return ""
  }
}

export function VoiceDock({ sessionId, agent }: { sessionId: string; agent?: string }) {
  const controller = useVoiceController()
  const snapshot = useVoiceSnapshot()
  // The pane holds the call while it shows this conversation.
  useEffect(() => controller?.show(sessionId), [controller, sessionId])
  const levels = useCallback(
    () => controller?.levels() ?? { input: 0, output: 0 },
    [controller],
  )
  if (!controller || !snapshot || snapshot.phase === "idle") return null
  if (snapshot.sessionId !== sessionId) return null
  if (snapshot.phase === "failed" || snapshot.phase === "dropped")
    return (
      <div className="voice-dock" data-phase={snapshot.phase}>
        <StatusLabel role="status" tone="warning" className="voice-notice">
          {snapshot.notice}
        </StatusLabel>
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
      <p className="voice-bubble" role="status" aria-live="polite">
        {line}
      </p>
      <VoiceOrb phase={snapshot.phase} tone={orbTone(agent)} levels={levels} />
    </div>
  )
}
