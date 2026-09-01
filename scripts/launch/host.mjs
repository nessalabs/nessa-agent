/**
 * The OS-shaped half of `just`. Shared code never mentions Linux,
 * macOS, or Windows. It talks to a LaunchHost: detect a GUI, prepare env,
 * name the fast and shipping bundles, refuse to compile when native deps
 * are missing. `resolveLaunch` injects one implementation.
 *
 * @typedef {'linux' | 'macos' | 'windows'} LaunchKind
 *
 * @typedef {object} LaunchHost
 * @property {LaunchKind} kind
 * @property {string} fastBundle  Tauri `--bundles` value for the testing-shaped release
 * @property {string} releaseBundle  Tauri `--bundles` value for the shipping installer
 * @property {(env?: NodeJS.ProcessEnv) => boolean} hasGui
 * @property {(env?: NodeJS.ProcessEnv) => Record<string, string>} prepareEnv
 * @property {() => string | null} missingNative  error text, or null when the host can compile
 */

export {}
