import { WEST_HANDLE_NARROW, type HostFeatures } from "./features"

/** macOS Tauri: native frost, system-owned resize frame, layer compositor. */
export const macos: HostFeatures = {
  kind: "macos",
  frost: "native",
  compositor: "layer",
  westHandleClass: WEST_HANDLE_NARROW,
  capturePointerOnWestHandle: true,
  animateMount: true,
  streamText: true,
  emptyState: true,
}
