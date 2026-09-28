/**
 * Motion helpers the DOM adapters share. Durations and curves are the
 * stylesheet's tokens (`--desktop-*` in `styles.css`), read from the element
 * that moves, so script and CSS animate on one clock and one curve, and
 * reduced motion — which the stylesheet turns into zero durations under the
 * root's `data-motion` (`adapters/motion-preference.ts`) — reaches script
 * motion with no second check. A missing token is no motion.
 */

/** A curve token's value at `element`, e.g. the spring the stylesheet defines; null when unset. */
export function motionToken(element: Element, name: string): string | null {
  const value = getComputedStyle(element).getPropertyValue(name).trim()
  return value === "" ? null : value
}

/** A duration token in milliseconds at `element`; zero — no motion — when unset. */
export function durationToken(element: Element, name: string): number {
  const value = motionToken(element, name) ?? ""
  const parsed = value.endsWith("ms")
    ? Number.parseFloat(value)
    : value.endsWith("s")
      ? Number.parseFloat(value) * 1000
      : Number.NaN
  return Number.isFinite(parsed) ? parsed : 0
}
