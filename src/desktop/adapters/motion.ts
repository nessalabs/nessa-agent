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

/**
 * A curve token as a function of progress (0 to 1), for script motion that
 * cannot hand its curve to CSS, such as a scroll. Reads `cubic-bezier(…)`;
 * any other value (a keyword, a spring's `linear(…)`) is taken as linear.
 */
export function curveToken(element: Element, name: string): (t: number) => number {
  const match = /^cubic-bezier\(([^)]*)\)$/.exec(motionToken(element, name) ?? "")
  const points = match?.[1].split(",").map((part) => Number.parseFloat(part))
  if (!points || points.length !== 4 || points.some((point) => !Number.isFinite(point)))
    return (t) => t
  const [x1, y1, x2, y2] = points
  const along = (a: number, b: number, s: number) =>
    3 * a * s * (1 - s) ** 2 + 3 * b * s ** 2 * (1 - s) + s ** 3
  return (t) => {
    if (t <= 0) return 0
    if (t >= 1) return 1
    // The curve's parameter whose x is `t`, by bisection: x rises monotonically.
    let low = 0
    let high = 1
    for (let step = 0; step < 24; step++) {
      const middle = (low + high) / 2
      if (along(x1, x2, middle) < t) low = middle
      else high = middle
    }
    return along(y1, y2, (low + high) / 2)
  }
}
