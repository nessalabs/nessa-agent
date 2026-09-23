/**
 * Grows the detail sheet when its content grows, and at no other time.
 *
 * The Sheet moves its own panel: it follows a drag, it animates the expand
 * toggle, it tracks the window. A second animator reacting to every change in
 * the panel's height fought all three — a drag stuttered behind the finger,
 * the toggle stacked animations on the Sheet's own. So this listens to the
 * content, which only the turn changes, and treats the panel's height as
 * something to keep up to date with rather than something to act on.
 *
 * The decisions are here, apart from the DOM, so they can be tested with a
 * fake panel; `watchSheetGrowth` wires them to a real one.
 */

export type SheetPanel = {
  /** The panel's rendered height now, mid-animation included. */
  height(): number
  width(): number
  /** Expanded to full height: content scrolls, nothing grows. */
  expanded(): boolean
  /** Being moved by the Sheet itself — a drag, or its own settle or toggle. */
  heldBySheet(): boolean
  /** Starts the growth; null when motion is off. */
  animate(from: number, to: number): { cancel(): void } | null
}

export function sheetGrowth(panel: SheetPanel) {
  let settled = panel.height()
  let width = panel.width()
  let running: { cancel(): void } | null = null

  return {
    /** The content changed size: grow toward wherever the layout now is. */
    contentChanged() {
      // Mid-growth, start from where the panel is on screen, not where it was
      // headed: a second tool in a burst continues the motion instead of
      // snapping to the end of the first one.
      const from = running ? panel.height() : settled
      running?.cancel()
      running = null
      const to = panel.height()
      settled = to
      const nextWidth = panel.width()
      // A width change is the window or the layout moving, and its reflow is
      // not the turn gaining anything.
      const resized = Math.abs(nextWidth - width) >= 1
      width = nextWidth
      if (resized || panel.expanded() || panel.heldBySheet()) return
      if (from === 0 || Math.abs(to - from) < 1) return
      running = panel.animate(from, to)
    },
    /** The panel changed size on its own: remember it, never animate it. */
    panelChanged() {
      if (running) return
      settled = panel.height()
      width = panel.width()
    },
    /** The growth ran to its end. */
    finished() {
      running = null
      settled = panel.height()
    },
  }
}

/** A CSS duration in milliseconds; anything unreadable is 0, which means stay still. */
export function durationMilliseconds(value: string) {
  const trimmed = value.trim()
  const parsed = Number.parseFloat(trimmed)
  if (!Number.isFinite(parsed) || parsed < 0) return 0
  return trimmed.endsWith("ms") ? parsed : parsed * 1000
}

/**
 * Wires `sheetGrowth` to a real panel. The duration is the design system's own
 * token, which collapses to 0ms under `prefers-reduced-motion`, so the snap
 * comes back without a media query here to keep in step with the theme.
 */
export function watchSheetGrowth(content: HTMLElement, panel: HTMLElement) {
  let ours: Animation | null = null
  const style = () => getComputedStyle(panel)
  const growth = sheetGrowth({
    height: () => panel.getBoundingClientRect().height,
    width: () => panel.getBoundingClientRect().width,
    expanded: () => panel.dataset.expanded === "true",
    // The Sheet writes an inline height while a drag is live, and animates
    // the panel when it settles or toggles.
    heldBySheet: () =>
      panel.style.height !== "" ||
      panel.getAnimations().some((animation) => animation !== ours),
    animate(from, to) {
      const duration = durationMilliseconds(
        style().getPropertyValue("--nessa-motion-duration-normal"),
      )
      if (duration === 0) return null
      const animation = panel.animate([{ height: `${from}px` }, { height: `${to}px` }], {
        duration,
        easing:
          style().getPropertyValue("--nessa-motion-easing-standard").trim() || "ease",
      })
      ours = animation
      animation.addEventListener("finish", () => {
        if (ours !== animation) return
        ours = null
        growth.finished()
      })
      return {
        cancel() {
          if (ours === animation) ours = null
          animation.cancel()
        },
      }
    },
  })
  const observer = new ResizeObserver((entries) => {
    if (entries.some((entry) => entry.target === content)) growth.contentChanged()
    else growth.panelChanged()
  })
  observer.observe(content)
  observer.observe(panel)
  return () => {
    observer.disconnect()
    ours?.cancel()
  }
}
