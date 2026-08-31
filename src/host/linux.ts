import { WEST_HANDLE_WIDE, type HostFeatures } from "./features"

/** Linux Tauri: CSS frost, page-owned 12px west handle, layout compositor. */
export const linux: HostFeatures = {
  kind: "linux",
  frost: "css",
  compositor: "layout",
  westHandleClass: WEST_HANDLE_WIDE,
  capturePointerOnWestHandle: false,
  animateMount: false,
  streamText: false,
  emptyState: true,
}
