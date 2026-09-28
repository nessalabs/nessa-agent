/**
 * Sideways slides, as the page plays them, and holding something still on
 * screen while what it sits in slides.
 *
 * The workspace's FLIP (`workspace/adapters/dom/flip.tsx`) slides a column
 * that moved by `translateX`, from where it was to its place, and names each
 * of those animations `slideAnimation`. An element that appears inside a
 * column mid-slide — a title that changed rows because the column's room
 * changed — would ride that slide from wherever the column started, which may
 * be under the window's controls. `holdStill` gives it the slide's exact
 * opposite, on the same clock, so it stands at its own place from its first
 * frame while the column arrives around it.
 */

/** The id every sideways slide carries, so the ones in flight can be found and answered. */
export const slideAnimation = "desktop-slide"

/** The x of a `translateX(…px)` keyframe; 0 for anything else. */
function shiftOf(keyframe: Keyframe | undefined): number {
  const transform = typeof keyframe?.transform === "string" ? keyframe.transform : ""
  const match = /translateX\((-?[\d.]+)px\)/.exec(transform)
  return match ? Number.parseFloat(match[1]) : 0
}

/**
 * Cancels, for `element`, every slide its ancestors are playing: each gets a
 * counter-slide of the opposite shift, with the same duration and curve, at
 * the same point in its run. Returns what it started.
 */
export function holdStill(element: HTMLElement): Animation[] {
  const held: Animation[] = []
  for (let above = element.parentElement; above; above = above.parentElement) {
    for (const slide of above.getAnimations()) {
      if (slide.id !== slideAnimation || !(slide.effect instanceof KeyframeEffect))
        continue
      const shift = shiftOf(slide.effect.getKeyframes()[0])
      if (shift === 0) continue
      const timing = slide.effect.getTiming()
      const counter = element.animate(
        [{ transform: `translateX(${-shift}px)` }, { transform: "none" }],
        {
          duration: timing.duration,
          easing: timing.easing,
          composite: "add",
        },
      )
      if (slide.currentTime !== null) counter.currentTime = slide.currentTime
      held.push(counter)
    }
  }
  return held
}
