import { spawnSync } from "node:child_process"

function defaultXcodeSelect() {
  return spawnSync("xcode-select", ["-p"], { stdio: "ignore" }).status === 0
}

/**
 * @param {{ xcodeSelect?: () => boolean }} [deps]
 * @returns {import('./host.mjs').LaunchHost}
 */
export function createMacos({ xcodeSelect = defaultXcodeSelect } = {}) {
  return {
    kind: "macos",
    // The analog of "the .app, no dmg".
    fastBundle: "app",
    // The thing you ship: a .dmg wrapping the .app.
    releaseBundle: "dmg",

    hasGui(env = process.env) {
      // A Mac always has a window server for a logged-in session. CI is the
      // one case that is headless on purpose.
      return !env.CI
    },

    prepareEnv() {
      return {}
    },

    missingNative() {
      if (xcodeSelect()) return null
      return "Xcode command-line tools missing — run: xcode-select --install"
    },
  }
}

export const macos = createMacos()
