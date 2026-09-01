/**
 * The injected host for this page, and the one seam to the desktop window.
 *
 * OS-specific behaviour lives in `macos.ts` / `linux.ts` / `browser.ts` /
 * `other.ts`. `resolveHost` picks one. Window commands live in `window.ts`
 * so the Rust event-name test still has a single file to grep.
 */

export { browser } from "./browser"
export type { CompositorKind, FrostKind, HostFeatures, HostKind } from "./features"
export { linux } from "./linux"
export { macos } from "./macos"
export { other } from "./other"
export { host, resolveHost } from "./resolve"
export type { PanelSize } from "./window"
export {
  flushCompositor,
  onFocusComposer,
  onLiveResize,
  onToggleSurface,
  onWindowResize,
  setFrosted,
  startResizeFromLeftEdge,
  windowSize,
} from "./window"
