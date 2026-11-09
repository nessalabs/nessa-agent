/**
 * A conversation composer's voice controls, from the window's call: the
 * button starts a call about this session, and while the call talks to it
 * the composer shows that call's switches. Undefined without a controller.
 */
import { useMemo } from "react"
import type { ComposerVoice } from "../../ui/composer"
import { useVoiceController, useVoiceSnapshot } from "./voice-context"

export function useComposerVoice(sessionId: string): ComposerVoice | undefined {
  const controller = useVoiceController()
  const snapshot = useVoiceSnapshot()
  const open =
    snapshot !== undefined &&
    snapshot.sessionId === sessionId &&
    snapshot.phase !== "idle" &&
    snapshot.phase !== "failed" &&
    snapshot.phase !== "dropped"
  const microphoneOn = snapshot?.microphoneOn ?? true
  const speakerOn = snapshot?.speakerOn ?? true
  return useMemo(
    () =>
      controller
        ? {
            active: open,
            start: () => void controller.start(sessionId),
            end: () => controller.end(),
            microphoneOn,
            speakerOn,
            setMicrophone: (on: boolean) => controller.setMicrophone(on),
            setSpeaker: (on: boolean) => controller.setSpeaker(on),
          }
        : undefined,
    [controller, open, microphoneOn, speakerOn, sessionId],
  )
}
