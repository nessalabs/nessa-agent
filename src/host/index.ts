/**
 * The injected host for this page, and the one seam to the desktop window.
 *
 * OS-specific behaviour lives in `macos.ts` / `linux.ts` / `browser.ts` /
 * `other.ts`. `resolveHost` picks one. Window commands live in `window.ts`
 * so the Rust event-name test still has a single file to grep.
 */

export type { CompositorKind } from "./features"
export { host } from "./resolve"
export {
  chordMatches,
  matchesAccelerator,
  parseAccelerator,
  type KeyChord,
} from "./accelerator"
export {
  availableUpdate,
  chooseAttachmentFiles,
  closeSetupWindow,
  finishSetupWindow,
  retrySetupRecord,
  hasNativeHost,
  installUpdate,
  type ChosenAgent,
  loadChosenAgent,
  loadAssignedSurfaceCredential,
  flushCompositor,
  onFocusComposer,
  onLinkNotOpened,
  onLiveResize,
  onSummoned,
  onToggleSurface,
  onUpdateAvailable,
  onUpdateFailed,
  onUpdateProgress,
  onWindowResize,
  readAttachmentBytes,
  revealSetupWindow,
  setFrosted,
  startResizeFromLeftEdge,
  windowSize,
  windowSurface,
  loadShortcuts,
  type ChosenFile,
  type Downloaded,
  type LinkNotOpened,
  type Release,
  type SetupHandoff,
  type SetupWindowClose,
} from "./window"
