/**
 * The head of a column, the same in the workspace's session list, its
 * sidebar and Settings' page: the titlebar row, holding the column's own
 * action at its far end, and the column's title — inline in that row, after
 * the window's controls where they stand over the column, when it fits there
 * with room to breathe, else on a row of its own below (`titlePlacement`,
 * `model/column-title.ts`) — then whatever the column puts under it.
 *
 * Where an inline title starts is the column's stylesheet's to say, as the
 * row's `padding-left` (at or after `--desktop-titlebar-safe-start` wherever
 * the controls stand over the column), so a change of it — a sidebar
 * folding — resizes the row's content and is measured like any other.
 *
 * When the title changes rows it fades in at its new place and what is
 * beneath it glides by the row it gained or gave up, by transform and
 * opacity alone; if its column is sliding at that moment, the title holds
 * still at its own place (`holdStill`) rather than ride the slide from where
 * the column began, which may be under the window's controls.
 */
import { useLayoutEffect, useRef, useState, type ReactNode, type RefObject } from "react"
import { flushSync } from "react-dom"
import { holdStill } from "../adapters/hold-still"
import { durationToken, motionToken } from "../adapters/motion"
import { titlePlacement, type TitlePlacement } from "../model/column-title"
import "./column-header.css"

export function ColumnHeader({
  title,
  icon,
  action,
  heading: Heading = "h2",
  headingId,
  flipId,
  children,
}: {
  title?: string
  icon?: ReactNode
  /** The column's own action, at the titlebar row's far end. */
  action?: ReactNode
  heading?: "h1" | "h2"
  headingId?: string
  /** The title's `data-flip-id`, where a `FlipScope` slides it with its column. */
  flipId?: string
  children?: ReactNode
}) {
  const bar = useRef<HTMLDivElement>(null)
  const sizer = useRef<HTMLSpanElement>(null)
  const end = useRef<HTMLSpanElement>(null)
  const placement = useTitlePlacement(bar, sizer, end, title !== undefined)
  const titled = title !== undefined
  const heading = titled ? (
    <div
      className="desktop-column-title"
      data-placement={placement}
      data-flip={flipId ? "slide" : undefined}
      data-flip-id={flipId}
    >
      {icon}
      <Heading id={headingId}>{title}</Heading>
    </div>
  ) : null
  return (
    <>
      <div
        ref={bar}
        className="desktop-column-bar"
        data-title={titled ? placement : undefined}
        data-tauri-drag-region
      >
        {placement === "inline" ? heading : null}
        {titled ? (
          // The title as it would sit inline, measured and never seen.
          <span
            ref={sizer}
            className="desktop-column-title desktop-column-sizer"
            data-placement="inline"
            aria-hidden="true"
          >
            {icon}
            <span>{title}</span>
          </span>
        ) : null}
        <span ref={end} className="desktop-column-action">
          {action}
        </span>
      </div>
      {placement === "below" ? heading : null}
      {children}
    </>
  )
}

/** What follows the titlebar row in its column, whose place a change of rows moves. */
const following = (bar: HTMLElement) => {
  const after: HTMLElement[] = []
  for (let next = bar.nextElementSibling; next; next = next.nextElementSibling)
    if (next instanceof HTMLElement && !next.classList.contains("desktop-column-title"))
      after.push(next)
  return after
}

/**
 * The title's row, from the widths the page lays out: the titlebar row's
 * content (after the controls), the column's action, and the title set
 * inline. Decided before the frame paints, so no frame shows the title in the
 * row it is leaving.
 */
function useTitlePlacement(
  bar: RefObject<HTMLElement | null>,
  sizer: RefObject<HTMLElement | null>,
  end: RefObject<HTMLElement | null>,
  titled: boolean,
): TitlePlacement {
  const [placement, setPlacement] = useState<TitlePlacement>("below")
  const now = useRef<TitlePlacement | null>(null)
  useLayoutEffect(() => {
    const row = bar.current
    const title = sizer.current
    const action = end.current
    if (!titled || !row || !title || !action || typeof ResizeObserver === "undefined")
      return
    const widths = new Map<Element, number>()
    const observer = new ResizeObserver((entries) => {
      for (const entry of entries) widths.set(entry.target, entry.contentRect.width)
      const next = titlePlacement({
        room: (widths.get(row) ?? 0) - (widths.get(action) ?? 0),
        title: widths.get(title) ?? 0,
        now: now.current,
      })
      if (next === now.current) return
      const first = now.current === null
      now.current = next
      const below = following(row)
      const before = below.map((element) => element.offsetTop)
      flushSync(() => setPlacement(next))
      if (first) return
      const duration = durationToken(row, "--desktop-base")
      if (duration === 0) return
      const easing = motionToken(row, "--desktop-ease") ?? "ease"
      const moved = row.parentElement?.querySelector<HTMLElement>(
        ":scope > .desktop-column-title, :scope > .desktop-column-bar > .desktop-column-title[data-placement]:not(.desktop-column-sizer)",
      )
      if (moved) {
        // In the titlebar row it stands at its place from the first frame,
        // clear of the window's controls; below, it travels with its column.
        if (next === "inline") holdStill(moved)
        moved.animate([{ opacity: 0 }, { opacity: 1 }], { duration, easing })
      }
      below.forEach((element, index) => {
        const shift = before[index] - element.offsetTop
        if (Math.abs(shift) < 0.5) return
        element.animate(
          [{ transform: `translateY(${shift}px)` }, { transform: "none" }],
          { duration, easing, composite: "add" },
        )
      })
    })
    for (const element of [row, title, action]) observer.observe(element)
    return () => observer.disconnect()
  }, [bar, sizer, end, titled])
  return placement
}
