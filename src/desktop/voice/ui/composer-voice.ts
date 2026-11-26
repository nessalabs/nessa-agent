/**
 * A conversation composer's voice button, from the window's call: it starts
 * a call about this session (asking for the key first when none is saved),
 * and shows pressed while the call talks to it. The call's switches are on
 * its tab (`voice-dock.tsx`). Undefined without a controller.
 */
import { useMemo } from "react"
import type { ComposerVoice } from "../../ui/composer"
import { useVoiceController, useVoiceSnapshot, useVoiceStart } from "./voice-context"

export function useComposerVoice(sessionId: string): ComposerVoice | undefined {
  const controller = useVoiceController()
  const snapshot = useVoiceSnapshot()
  const start = useVoiceStart()
  const open =
    snapshot !== undefined &&
    snapshot.sessionId === sessionId &&
    snapshot.phase !== "idle" &&
    snapshot.phase !== "failed" &&
    snapshot.phase !== "dropped"
  return useMemo(
    () =>
      controller && start
        ? {
            active: open,
            // Asked first when a call talks to another chat: starting ends it.
            start: () => start(sessionId),
          }
        : undefined,
    [controller, start, open, sessionId],
  )
}
