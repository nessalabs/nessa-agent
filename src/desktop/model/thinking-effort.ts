/**
 * The thinking control's rules, apart from its drawing
 * (`ui/thinking-control.tsx`): which level a key or the pointer picks, the
 * order the track's stops light in when the level changes, and where the
 * control's popover sits by its chip. The levels themselves are
 * `composer-options.ts`'s (`thinkingLevels`); nothing here names one — every
 * rule is by position among however many a model offers. Pixels and indexes
 * in, indexes and pixels out; nothing here reads the page.
 */
import type { Box } from "./tooltip-placement"

/**
 * The level a key moves to from `index` among `count`: the arrows step one
 * (right and up are more thinking), Home and End go to either end. Undefined
 * when the key is not one of these, or would not move — the control then
 * leaves the key alone.
 */
export function levelAfterKey(
  key: string,
  index: number,
  count: number,
): number | undefined {
  if (count === 0) return undefined
  const last = count - 1
  const next =
    key === "ArrowRight" || key === "ArrowUp"
      ? Math.min(last, index + 1)
      : key === "ArrowLeft" || key === "ArrowDown"
        ? Math.max(0, index - 1)
        : key === "Home"
          ? 0
          : key === "End"
            ? last
            : undefined
  return next === index ? undefined : next
}

/**
 * The stop under the pointer at `x`, on a track starting at `left` and
 * `width` wide, divided evenly among `count` stops. Past either end it is
 * the stop at that end, so a drag that overshoots still lands.
 */
export function stopAt(x: number, left: number, width: number, count: number): number {
  if (count <= 1 || width <= 0) return 0
  const along = Math.floor(((x - left) / width) * count)
  return Math.max(0, Math.min(count - 1, along))
}

/**
 * Where each stop falls in the wave that runs along the track when the
 * level changes from `from` to `to`: rising, the stops that light do so one
 * after another away from where it was (0 first); falling, those that dim go
 * back towards where it lands, the farthest first. Stops that do not change
 * are 0. The stylesheet multiplies these by its stagger token, which reduced
 * motion makes zero.
 */
export function stopDelays(from: number, to: number, count: number): number[] {
  return Array.from({ length: count }, (_, stop) => {
    if (to > from && stop > from && stop <= to) return stop - from - 1
    if (to < from && stop > to && stop <= from) return from - stop
    return 0
  })
}

/** How far along the scale a stop stands, from 0 (the least) to 1 (the most). */
export function stopRank(stop: number, count: number): number {
  return count <= 1 ? 1 : stop / (count - 1)
}

export type PopoverSide = "above" | "below"

export interface PopoverPlacement {
  readonly x: number
  readonly y: number
  readonly side: PopoverSide
}

/** The gap between the chip and its popover, and the least room kept from the window's edge. */
export const popoverSpacing = { gap: 10, margin: 12 } as const

/**
 * Where the popover sits: above its chip — the composer's controls are at
 * its foot — with its trailing edge on the chip's, shifted to stay inside
 * the window, and below the chip only when above would leave the window
 * while below would not.
 */
export function placePopover(
  anchor: Box,
  popover: { readonly width: number; readonly height: number },
  window: { readonly width: number; readonly height: number },
): PopoverPlacement {
  const { gap, margin } = popoverSpacing
  const trailing = anchor.left + anchor.width - popover.width
  const x = Math.max(margin, Math.min(trailing, window.width - margin - popover.width))
  const above = anchor.top - gap - popover.height
  const below = anchor.top + anchor.height + gap
  const side: PopoverSide =
    above < margin && below + popover.height <= window.height - margin ? "below" : "above"
  return { x: Math.round(x), y: Math.round(side === "above" ? above : below), side }
}
