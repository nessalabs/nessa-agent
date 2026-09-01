import { inTauri } from "./window"
import { browser } from "./browser"
import type { HostFeatures } from "./features"
import { linux } from "./linux"
import { macos } from "./macos"
import { other } from "./other"

/**
 * Picks the host features for this process. The shell calls this once and
 * holds the result; tests call it with a stubbed environment.
 */
export function resolveHost(
  ua = typeof navigator === "undefined" ? "" : navigator.userAgent,
  tauri = inTauri,
): HostFeatures {
  if (!tauri) return browser
  if (ua.includes("Mac")) return macos
  if (ua.includes("Linux")) return linux
  return other
}

/** The injected host for this page. */
export const host: HostFeatures = resolveHost()
