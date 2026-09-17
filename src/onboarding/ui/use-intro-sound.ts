import * as React from "react"

import bloomingGlass from "./blooming-glass.mp3"

/**
 * The sound the setup window opens with.
 *
 * It plays once, at mount, alongside the blob — the two are one event, so the
 * sound is not cued off a later step or replayed when setup moves on.
 *
 * Two things can silence it, both deliberately. A reduced-motion preference
 * turns the whole opening off, and a soundtrack to an animation that is not
 * playing is worse than no soundtrack; and a host that blocks autoplay without
 * a gesture rejects the play, which is swallowed rather than retried. A
 * startup sound that arrives late, after the first click, is a sound from
 * nowhere — better that it simply did not happen.
 */
export function useIntroSound(play: boolean) {
  React.useEffect(() => {
    if (!play) return
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return
    const audio = new Audio(bloomingGlass)
    // A sound nobody asked for and cannot turn off sits under the interface,
    // not over it.
    audio.volume = 0.35
    void audio.play().catch(() => {})
    // Leaving setup early stops it: the sound belongs to the opening, and the
    // opening is over.
    return () => audio.pause()
  }, [play])
}
