import { WEST_HANDLE_NARROW, type HostFeatures } from "./features"

/**
 * Plain browser (`pnpm dev`). There is no window frame and no native frost,
 * so the CSS filter is the frost and the west handle is a no-op.
 */
export const browser: HostFeatures = {
  kind: "browser",
  frost: "css",
  compositor: "layer",
  westHandleClass: WEST_HANDLE_NARROW,
  capturePointerOnWestHandle: true,
  animateMount: true,
  streamText: true,
  emptyState: true,
  flushOnTurn: false,
}
