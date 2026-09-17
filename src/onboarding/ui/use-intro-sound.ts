import * as React from "react"

import { playCue, stopCue } from "./sound"

/**
 * The sound the setup window opens with.
 *
 * It plays once, at mount, alongside the light — the two are one event, so it
 * is not cued off a later step or replayed when setup moves on. Leaving setup
 * early stops it: the sound belongs to the opening, and the opening is over.
 */
export function useIntroSound(play: boolean) {
  React.useEffect(() => {
    if (!play) return
    playCue("intro")
    return () => stopCue("intro")
  }, [play])
}
