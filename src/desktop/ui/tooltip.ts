/**
 * How a control asks for the window's tooltip: spread `tooltip(...)` on it.
 * Its words, the chord that runs it (drawn quieter, on the same line), and
 * the side it prefers — below for the window's own controls, above for the
 * composer's chips. The control's accessible name stays its `aria-label`;
 * the tooltip only describes it where its words say more.
 *
 * The one tooltip that reads these is `adapters/use-window-tooltips.ts`,
 * mounted once for the window; the attribute names are said only here and
 * there.
 */
import type { TooltipSide } from "../model/tooltip-placement"

export interface TooltipAttributes {
  readonly "data-tooltip": string
  readonly "data-tooltip-shortcut"?: string
  readonly "data-tooltip-side"?: TooltipSide
}

export function tooltip(
  text: string,
  { shortcut, side }: { shortcut?: string; side?: TooltipSide } = {},
): TooltipAttributes {
  return {
    "data-tooltip": text,
    ...(shortcut ? { "data-tooltip-shortcut": shortcut } : {}),
    ...(side ? { "data-tooltip-side": side } : {}),
  }
}
