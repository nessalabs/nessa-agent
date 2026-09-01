import { linux } from "./linux.mjs"
import { macos } from "./macos.mjs"
import { windows } from "./windows.mjs"

/**
 * Picks the launch host for this process. Tests call it with a stubbed
 * platform so Linux, macOS, and Windows can be checked on one machine.
 *
 * @param {NodeJS.Platform} [platform]
 * @returns {import('./host.mjs').LaunchHost}
 */
export function resolveLaunch(platform = process.platform) {
  switch (platform) {
    case "linux":
      return linux
    case "darwin":
      return macos
    case "win32":
      return windows
    default:
      throw new Error(`no launch host for ${platform}`)
  }
}
