import { WEST_HANDLE_NARROW, type HostFeatures } from "./features"

/** Windows and anything that is not macOS, Linux, or a plain browser. */
export const other: HostFeatures = {
  kind: "other",
  frost: "css",
  compositor: "layer",
  westHandleClass: WEST_HANDLE_NARROW,
  capturePointerOnWestHandle: true,
  animateMount: true,
  streamText: true,
  emptyState: true,
}
