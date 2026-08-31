import { WEST_HANDLE_WIDE, type HostFeatures } from "./features"

/** Linux Tauri: CSS frost, page-owned 12px west handle. */
export const linux: HostFeatures = {
  kind: "linux",
  frost: "css",
  westHandleClass: WEST_HANDLE_WIDE,
  capturePointerOnWestHandle: false,
  animateTranscript: false,
}
