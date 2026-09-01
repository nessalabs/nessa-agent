/**
 * Windows launch host. Written to match Linux/macOS, not yet run on a
 * Windows box — `hasGui` is the guess a desktop session usually satisfies.
 * Native WebView2 / MSVC checks belong in `missingNative` once a Windows
 * machine has said which ones actually fail. Verify `pnpm launch` and
 * `pnpm app:fast` there before treating this as known-good.
 *
 * @returns {import('./host.mjs').LaunchHost}
 */
export function createWindows() {
  return {
    kind: "windows",
    // The analog of macOS `.app` / Linux `.deb`: the installer you actually run.
    fastBundle: "nsis",

    hasGui(env = process.env) {
      if (env.CI) return false
      // A service session is the one headless case we can name without a
      // Windows box. Interactive logons report Console or RDP-Tcp#N.
      if (env.SESSIONNAME === "Services") return false
      return true
    },

    prepareEnv() {
      return {}
    },

    missingNative() {
      return null
    },
  }
}

export const windows = createWindows()
