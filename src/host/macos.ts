import { WEST_HANDLE_NARROW, type HostFeatures } from "./features"

/** macOS Tauri: native frost, system-owned resize frame. */
export const macos: HostFeatures = {
  kind: "macos",
  frost: "native",
  westHandleClass: WEST_HANDLE_NARROW,
  capturePointerOnWestHandle: true,
  animateTranscript: true,
}
