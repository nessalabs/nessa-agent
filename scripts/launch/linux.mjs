import { spawnSync } from "node:child_process"
import { existsSync } from "node:fs"

const APT =
  "sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf fakeroot"

function defaultHasDrm() {
  return existsSync("/dev/dri/card0") || existsSync("/dev/dri/renderD128")
}

function defaultPkgConfig(name) {
  return spawnSync("pkg-config", ["--exists", name]).status === 0
}

/**
 * @param {{ hasDrm?: () => boolean, pkgConfig?: (name: string) => boolean }} [deps]
 * @returns {import('./host.mjs').LaunchHost}
 */
export function createLinux({
  hasDrm = defaultHasDrm,
  pkgConfig = defaultPkgConfig,
} = {}) {
  return {
    kind: "linux",
    fastBundle: "deb",

    hasGui(env = process.env) {
      if (env.CI) return false
      return Boolean(env.DISPLAY || env.WAYLAND_DISPLAY)
    },

    prepareEnv(env = process.env) {
      // A reader who already chose a WebKit path is left alone — same as
      // `platform/linux/webkit.rs`.
      if (env.WEBKIT_DISABLE_DMABUF_RENDERER) return {}
      if (hasDrm()) return {}
      return {
        WEBKIT_DISABLE_DMABUF_RENDERER: "1",
        WEBKIT_DISABLE_COMPOSITING_MODE: "1",
      }
    },

    missingNative() {
      if (pkgConfig("webkit2gtk-4.1") && pkgConfig("gtk+-3.0")) return null
      return `Linux native deps missing. On Debian/Ubuntu:\n  ${APT}`
    },
  }
}

export const linux = createLinux()
