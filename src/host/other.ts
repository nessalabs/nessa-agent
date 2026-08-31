import { WEST_HANDLE_NARROW, type HostFeatures } from "./features"

/** Windows and anything that is not macOS, Linux, or a plain browser. */
export const other: HostFeatures = {
  kind: "other",
  frost: "css",
  westHandleClass: WEST_HANDLE_NARROW,
  capturePointerOnWestHandle: true,
  animateTranscript: true,
}
