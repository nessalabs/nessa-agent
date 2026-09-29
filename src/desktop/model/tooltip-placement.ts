/**
 * Where a floating thing sits beside the control it belongs to — a tooltip by
 * the control it names, the thinking control's popover by its chip: on the
 * side the control prefers (below for the window's own controls, above for
 * the composer's chips), flipped to the other when it will not fit there or
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

export type TooltipSide = "below" | "above"

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
  const along =
    align === "end"
      ? anchor.left + anchor.width - tip.width
      : anchor.left + anchor.width / 2 - tip.width / 2
  const x = Math.max(margin, Math.min(along, window.width - margin - tip.width))
  const yOn = (side: TooltipSide) =>
    side === "below" ? anchor.top + anchor.height + gap : anchor.top - gap - tip.height
  const usable = (side: TooltipSide) => {
    const y = yOn(side)
    const inside = y >= margin && y + tip.height <= window.height - margin
    const box = { left: x, top: y, width: tip.width, height: tip.height }
    return inside && !(avoid && overlaps(box, avoid))
  }
  const other: TooltipSide = prefer === "below" ? "above" : "below"
  const side = usable(prefer) || !usable(other) ? prefer : other
  return { x: Math.round(x), y: Math.round(yOn(side)), side }
}
