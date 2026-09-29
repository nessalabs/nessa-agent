/**
 * The thinking control's rules, apart from its drawing
 * (`ui/thinking-control.tsx`): which level a key picks, where along the
 * slider the pointer is and which level that snaps to, and how much room
 * the control's popover keeps by its chip. The levels themselves are
 * `composer-options.ts`'s (`thinkingLevelsFor`); nothing here names one — every
 * rule is by position among however many a model offers. Pixels and indexes
 * in, indexes and pixels out; nothing here reads the page.
 */

/**
 * The level a key moves to from `index` among `count`: the arrows and Page
 * Up and Down step one (right and up are more thinking) — the levels are
 * few, so a page is one — and Home and End go to either end, where a step
 * past the end stays. Undefined when the key is not one of these, or there
 * are no levels — the control then leaves the key alone.
 */
export function levelAfterKey(
  key: string,
  index: number,
  count: number,
): number | undefined {
  if (count === 0) return undefined
  const last = count - 1
  const next =
    key === "ArrowRight" || key === "ArrowUp" || key === "PageUp"
      ? Math.min(last, index + 1)
      : key === "ArrowLeft" || key === "ArrowDown" || key === "PageDown"
        ? Math.max(0, index - 1)
        : key === "Home"
          ? 0
          : key === "End"
            ? last
            : undefined
  return next
}

/**
 * Where along the slider the pointer at `x` stands, as a level position from
 * 0 to `count - 1` — between levels while it is dragged — on a track starting
 * at `left` and `width` wide, with the levels evenly spaced from end to end.
 * Past either end it is that end, so a drag that overshoots still lands.
 */
export function positionAt(
  x: number,
  left: number,
  width: number,
  count: number,
): number {
  if (count <= 1 || width <= 0) return 0
  const along = ((x - left) / width) * (count - 1)
  return Math.max(0, Math.min(count - 1, along))
}

/** The level a position snaps to: the nearest one. */
export function nearestLevel(position: number): number {
  return Math.round(position)
}

/**
 * How far along the track a position stands, from 0 (the least) to 1 (the
 * most). A model's only level stands at the end.
 */
export function fractionAlong(position: number, count: number): number {
  return count <= 1 ? 1 : position / (count - 1)
}

/**
 * The popover's room beside its chip: the gap between them, and the least
 * room kept from the window's edge. Where it sits is `placeTooltip`'s rule
 * (`tooltip-placement.ts`), above the chip with its trailing edge on the
 * chip's.
 */
export const popoverSpacing = { gap: 10, margin: 12 } as const
