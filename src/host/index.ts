/**
 * The injected host for this page.
 *
 * OS-specific behaviour lives in `macos.ts` / `linux.ts` / `browser.ts` /
 * `other.ts`. `resolveHost` picks one. Shared window controls stay in
 * `host-window.ts` so the Rust event-name test still has a single file to
 * grep.
 */

export { browser } from "./browser"
export type { FrostKind, HostFeatures, HostKind } from "./features"
export { linux } from "./linux"
export { macos } from "./macos"
export { other } from "./other"
export { host, resolveHost } from "./resolve"
