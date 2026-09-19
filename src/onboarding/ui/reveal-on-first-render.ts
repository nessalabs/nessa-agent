/**
 * When the setup window is allowed to ask the host to put it on screen.
 *
 * The host builds that window hidden and places it — level, frame, Spaces —
 * without ordering it in, because a window is on screen the moment it exists
 * and a webview has painted nothing the moment it is created. Its own page asks
 * for the reveal, so that the first frame anyone sees already has setup in it
 * rather than a rectangle of nothing.
 *
 * The moment it may ask is its first render, and no later. It used to wait one
 * `requestAnimationFrame` past that, for an actual painted frame — which is a
 * wait that never ends: a hidden macOS window is not drawn, so its webview is
 * served no animation frames at all. The reveal was waiting for a paint that was
 * waiting for the reveal, and setup spent every launch hidden while its page ran
 * and played the opening sound to an empty screen.
 *
 * So this is the rule, written down where it can be tested: the page reports
 * that it has rendered, which is the one thing it can honestly know while it is
 * hidden, and the host decides what to do about it. Nothing here waits on a
 * clock that a window nobody can see does not run.
 */
export function revealOnFirstRender(reveal: () => void): undefined {
  reveal()
}
