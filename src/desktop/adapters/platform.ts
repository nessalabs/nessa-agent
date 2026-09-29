/**
 * Which keyboard the window is on, read once from the browser: the one place
 * the desktop window asks. What the user agent means is `macUserAgent`'s
 * (`model/keyboard.ts`); a page with no navigator is not a Mac.
 */
import { macUserAgent } from "../model/keyboard"

export const isMac = macUserAgent(
  typeof navigator === "undefined" ? "" : navigator.userAgent,
)
