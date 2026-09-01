import { spawnSync } from "node:child_process"
import process from "node:process"

import { resolveLaunch } from "./resolve.mjs"

/**
 * Testing-shaped release: same binary as `app:build`, slow optimiser off.
 * Lives here rather than in package.json so Windows cmd/PowerShell do not
 * have to parse Unix `VAR=value` prefixes.
 */
export const FAST_PROFILE = {
  CARGO_PROFILE_RELEASE_LTO: "false",
  CARGO_PROFILE_RELEASE_CODEGEN_UNITS: "16",
  CARGO_PROFILE_RELEASE_OPT_LEVEL: "1",
  CARGO_PROFILE_RELEASE_STRIP: "false",
}

export const USAGE = `usage: pnpm launch [--web|--fast]

  (default)  Run the desktop app — Vite on :1420 and tauri dev
  --web      Run the UI in a browser only; window controls no-op
  --fast     Release build with the slow optimisations off
             (macOS .app / Linux .deb / Windows nsis)

On Linux without a display, the default falls back to --web automatically.
`

/**
 * @param {string[]} argv
 * @returns {{ mode: 'help' | 'web' | 'fast' | 'app' } | { mode: 'error', error: string }}
 */
export function parseArgs(argv) {
  const flag = argv[0]
  if (flag === "-h" || flag === "--help") return { mode: "help" }
  if (flag === "--web") return { mode: "web" }
  if (flag === "--fast") return { mode: "fast" }
  if (!flag) return { mode: "app" }
  return { mode: "error", error: `unknown option: ${flag} (try --web or --fast)` }
}

/**
 * @param {NodeJS.Platform} [platform]
 */
export function pnpmBin(platform = process.platform) {
  return platform === "win32" ? "pnpm.cmd" : "pnpm"
}

function commandOnPath(bin) {
  const finder = process.platform === "win32" ? "where" : "which"
  return spawnSync(finder, [bin], { stdio: "ignore" }).status === 0
}

/**
 * @param {string[]} args
 * @param {{ env?: NodeJS.ProcessEnv, cwd?: string, platform?: NodeJS.Platform }} [opts]
 */
export function runPnpm(
  args,
  { env = process.env, cwd, platform = process.platform } = {},
) {
  const result = spawnSync(pnpmBin(platform), args, {
    stdio: "inherit",
    env,
    cwd,
    // .cmd files need a shell on Windows; Unix execs pnpm directly.
    shell: platform === "win32",
  })
  if (result.error) {
    if (result.error.code === "ENOENT") {
      throw new Error("pnpm not found — run: corepack enable && corepack prepare")
    }
    throw result.error
  }
  return result.status ?? 1
}

function withSccache(env) {
  if (env.RUSTC_WRAPPER || !commandOnPath("sccache")) return env
  return { ...env, RUSTC_WRAPPER: "sccache" }
}

/**
 * @param {'web' | 'app' | 'fast'} mode
 * @param {{
 *   host?: import('./host.mjs').LaunchHost,
 *   env?: NodeJS.ProcessEnv,
 *   run?: typeof runPnpm,
 *   log?: (msg: string) => void,
 *   cwd?: string,
 * }} [opts]
 */
export function launch(
  mode,
  {
    host = resolveLaunch(),
    env = process.env,
    run = runPnpm,
    log = (msg) => console.error(msg),
    cwd,
  } = {},
) {
  const base = withSccache({ ...env })

  if (mode === "app" && !host.hasGui(base)) {
    log("→ no GUI on this machine; starting web dev server instead")
    mode = "web"
  }

  if (mode === "web") {
    if (host.hasGui(base)) return run(["dev", "--", "--open"], { env: base, cwd })
    log("→ open http://127.0.0.1:1420 in a browser")
    return run(["dev"], { env: base, cwd })
  }

  const missing = host.missingNative()
  if (missing) {
    log(missing)
    return 1
  }

  const prepared = { ...base, ...host.prepareEnv(base) }

  if (mode === "app") {
    return run(["app"], { env: prepared, cwd })
  }

  return run(["exec", "tauri", "build", "--bundles", host.fastBundle], {
    env: { ...prepared, ...FAST_PROFILE },
    cwd,
  })
}
