/**
 * Where a floating thing sits beside the control it belongs to — a tooltip by
 * the control it names, the thinking control's popover by its chip: on the
 * side the control prefers (below for the window's own controls, above for
 * the composer's chips, beside for the side rail's icons), flipped to the
 * opposite side when it will not fit there or
 * would cover `avoid` — the window's traffic lights — and shifted along to
 * stay inside the window with a margin. Along the control it is centred, or
 * its trailing edge on the control's (`align: "end"`). It never covers the
 * control. Pixels in, pixels out; nothing here reads the page.
 */
export interface Box {
  readonly left: number
  readonly top: number
  readonly width: number
  readonly height: number
}

export type TooltipSide = "below" | "above" | "right" | "left"

export interface TooltipPlacement {
  readonly x: number
  readonly y: number
  readonly side: TooltipSide
}

/** The gap between a control and its tooltip, and the least room kept from the window's edge. */
export const tooltipSpacing = { gap: 6, margin: 8 } as const

const overlaps = (a: Box, b: Box) =>
  a.left < b.left + b.width &&
  b.left < a.left + a.width &&
  a.top < b.top + b.height &&
  b.top < a.top + a.height

export function placeTooltip(
  anchor: Box,
  tip: { readonly width: number; readonly height: number },
  window: { readonly width: number; readonly height: number },
  {
    prefer = "below",
    avoid,
    align = "centre",
    spacing = tooltipSpacing,
  }: {
    prefer?: TooltipSide
    avoid?: Box
    align?: "centre" | "end"
    spacing?: { readonly gap: number; readonly margin: number }
  } = {},
): TooltipPlacement {
  const { gap, margin } = spacing
  const across = prefer === "right" || prefer === "left"
  const clamp = (value: number, room: number, size: number) =>
    Math.max(margin, Math.min(value, room - margin - size))
  // Along the control: centred on it (or its trailing edge), kept inside the window.
  const x = across
    ? 0
    : clamp(
        align === "end"
          ? anchor.left + anchor.width - tip.width
          : anchor.left + anchor.width / 2 - tip.width / 2,
        window.width,
        tip.width,
      )
  const y = across
    ? clamp(anchor.top + anchor.height / 2 - tip.height / 2, window.height, tip.height)
    : 0
  // Off the control: a gap away on the chosen side.
  const at = (side: TooltipSide) => {
    switch (side) {
      case "below":
        return { x, y: anchor.top + anchor.height + gap }
      case "above":
        return { x, y: anchor.top - gap - tip.height }
      case "right":
        return { x: anchor.left + anchor.width + gap, y }
      case "left":
        return { x: anchor.left - gap - tip.width, y }
    }
  }
  const usable = (side: TooltipSide) => {
    const spot = at(side)
    const inside =
      spot.x >= margin &&
      spot.x + tip.width <= window.width - margin &&
      spot.y >= margin &&
      spot.y + tip.height <= window.height - margin
    const box = { left: spot.x, top: spot.y, width: tip.width, height: tip.height }
    return inside && !(avoid && overlaps(box, avoid))
  }
  const opposite: Record<TooltipSide, TooltipSide> = {
    below: "above",
    above: "below",
    right: "left",
    left: "right",
  }
  const other = opposite[prefer]
  const side = usable(prefer) || !usable(other) ? prefer : other
  const spot = at(side)
  return { x: Math.round(spot.x), y: Math.round(spot.y), side }
}
